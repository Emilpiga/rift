//! Player input + ability cast/channel intake on [`Sim`].
//! Split out of `sim/mod.rs`. Pure `impl Sim` block — every
//! method is defined on `Sim` and migrated here verbatim.

use glam::Vec3;
use hecs::Entity;
use rift_net::ids::ClientId;
use rift_net::messages::{InputCmd, WorldEvent};
use rift_net::{NetId, NetTick};

use super::player::ServerPlayer;
use super::{ability, channel, combat_ctx, effect, player, shrine, Sim};

impl Sim {
    /// Set the player's revive-shrine channel intent. `Some`
    /// requires alive + within [`SHRINE_INTERACT_RADIUS`] of
    /// the named shrine. `None` always succeeds (release F,
    /// walk out of range, etc.). Idempotent.
    pub fn set_shrine_channel(&mut self, client_id: ClientId, shrine: Option<NetId>) {
        use rift_net::messages::SHRINE_INTERACT_RADIUS;
        let Some(&entity) = self.sessions.get(&client_id) else { return };
        let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) else { return };
        match shrine {
            None => {
                p.channeling_shrine = None;
            }
            Some(id) => {
                if p.is_dead_or_ghosting() {
                    return;
                }
                drop(p);
                let Some((_, shrine_pos)) = shrine::find(&self.world, id) else {
                    return;
                };
                let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) else { return };
                let dist_sq = (p.k.position - shrine_pos).length_squared();
                if dist_sq > SHRINE_INTERACT_RADIUS * SHRINE_INTERACT_RADIUS {
                    return;
                }
                p.channeling_shrine = Some(id);
            }
        }
    }

    /// Stash an input from a client — coalesced against any earlier
    /// input still pending for the same client this tick.
    pub fn ingest_input(&mut self, client_id: ClientId, cmd: InputCmd) {
        player::merge_pending(&mut self.pending_inputs, client_id, cmd);
    }

    /// Forward a `ClientMsg::CastAbility` to the ability dispatch.
    pub fn cast_ability(
        &mut self,
        client_id: ClientId,
        ability_id: u8,
        client_origin: [f32; 3],
        aim_dir: [f32; 2],
        placed_target: Option<[f32; 3]>,
        target_net_id: Option<NetId>,
        tick: NetTick,
    ) {
        let aim = {
            let v = glam::Vec2::from(aim_dir).normalize_or_zero();
            Vec3::new(v.x, 0.0, v.y)
        };
        let intent = ability::CombatIntent::Player {
            client_id,
            ability_id,
            client_origin: Vec3::from_array(client_origin),
            aim,
            placed_target: placed_target.map(Vec3::from),
            target_net_id,
        };
        let Some(accepted) = ability::submit(
            &self.world,
            &self.sessions,
            &mut self.cooldowns,
            &self.floor,
            intent,
        ) else {
            return;
        };
        // Player casts emit the AbilityCast wire event right at
        // cast time — there's no separate windup/resolve split
        // for the player path today. AI casts emit their own
        // `EnemyCast::Start` event up in the AI tick before
        // dispatch ever runs, so dispatch never has to.
        self.pending_events.push(WorldEvent::AbilityCast {
            caster: accepted.caster,
            ability: accepted.ability_id as u16,
            origin: accepted.origin.to_array(),
            dir: [accepted.aim.x, accepted.aim.z],
            target: accepted.placed_target.map(|t| t.to_array()),
            start_tick: tick,
        });
        // Player casts don't currently produce summons or
        // player-damage rows, but the kernel sinks need valid
        // references regardless.
        let mut summons: Vec<(Vec3, rift_game::monsters::MonsterRole, f32)> = Vec::new();
        let mut player_damage: Vec<combat_ctx::PlayerHit> = Vec::new();
        let mut player_heals: Vec<(Entity, f32)> = Vec::new();
        let no_targets: [(Entity, Vec3); 0] = [];
        let mut sinks = ability::DispatchSinks {
            aoe_zones: &mut self.aoe_zones,
            events: &mut self.pending_events,
            next_projectile_net_id: &mut self.next_projectile_net_id,
            player_damage: &mut player_damage,
            player_heals: &mut player_heals,
            summons: &mut summons,
            player_targets: &no_targets,
        };
        ability::dispatch(&mut self.world, accepted, &mut sinks, tick);
        debug_assert!(
            summons.is_empty() && player_damage.is_empty(),
            "player cast emitted enemy-shaped effects",
        );
        // Apply queued heals — clamped at hp_max. Healing is
        // scaled by the target's healing-received multiplier
        // (Necrotic ⇒ 0.5×) so direct heals honour the same
        // debuff that HoT ticks do. The pre-mult `Heal` event
        // pushed by `dispatch` is rewritten in place with the
        // post-mult amount so floating combat text matches the
        // HP actually restored.
        for (target, amount) in player_heals {
            let debuff_mult = self
                .world
                .get::<&effect::EffectStack>(target)
                .map(|s| s.healing_received_mult())
                .unwrap_or(1.0);
            let stat_mult = self
                .world
                .get::<&player::ServerPlayer>(target)
                .map(|p| (1.0 + p.stats.healing_received).max(0.0))
                .unwrap_or(1.0);
            let mult = debuff_mult * stat_mult;
            let scaled = amount * mult;
            // Pre-heal HP so the meter credits effective HP
            // restored (i.e. excludes overheal). Overheal would
            // inflate healer rankings without reflecting any
            // real impact on survivability.
            let mut effective = 0.0_f32;
            if let Ok(mut p) = self.world.get::<&mut player::ServerPlayer>(target) {
                if !p.is_dead_or_ghosting() {
                    let before = p.hp;
                    p.hp = (p.hp + scaled).min(p.hp_max);
                    effective = p.hp - before;
                }
            }
            if effective > 0.0 {
                self.meters.entry(client_id).add_healing(ability_id, effective);
            }
            // Patch the trailing Heal event(s) for this target
            // with the post-mult amount. Walk from the back
            // since dispatch just pushed them; stop on the
            // first non-Heal so we don't rewrite history.
            if (mult - 1.0).abs() > f32::EPSILON {
                for ev in self.pending_events.iter_mut().rev() {
                    match ev {
                        WorldEvent::Heal { amount: a, .. } if (*a - amount).abs() < f32::EPSILON => {
                            *a = scaled;
                            break;
                        }
                        WorldEvent::AbilityCast { .. } => break,
                        _ => continue,
                    }
                }
            }
        }
    }

    /// Forward a `ClientMsg::EndChannel` request — cancels the
    /// caller's matching active channel (if any). Silently no-ops
    /// if the player isn't channeling that ability so a duplicate
    /// release packet doesn't error.
    pub fn end_channel(&mut self, client_id: ClientId, ability_id: u8) {
        let Some(&entity) = self.sessions.get(&client_id) else { return };
        channel::cancel(
            &mut self.world,
            entity,
            ability_id,
            &mut self.pending_events,
            &mut self.next_projectile_net_id,
        );
    }
}
