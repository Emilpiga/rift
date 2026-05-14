//! Per-tick snapshot construction.
//!
//! Each connected client gets their own `Snapshot` — `ack_seq` is
//! per-client, and view culling is anchored on each viewer's
//! position so a populated rift floor doesn't blow past the
//! unreliable channel buffer with hundreds of irrelevant entities.

use glam::Vec3;
use rift_game::kinematic::Kinematic;
use rift_net::{
    messages::{entity_flags, EntityKind, EntitySnapshot, Snapshot},
    ClientId, NetTick,
};

use super::actor::{NetIdentity, Vitals};
use super::enemies::{enemy_anim, ServerEnemy};
use super::loot::ServerLoot;
use super::minions::{self, ServerMinion};
use super::player::ServerPlayer;
use super::projectile::ServerProjectile;
use super::shrine::ServerReviveShrine;

/// Sight range used to view-cull replicated entities (enemies +
/// projectiles) per receiving client. Squared to skip the sqrt.
pub const VIEW_RANGE_SQ: f32 = 35.0 * 35.0;

/// Build the snapshot for one receiving client.
pub fn build(world: &hecs::World, tick: NetTick, ack_for: ClientId) -> Snapshot {
    let mut entities = Vec::new();
    let mut ack_seq = 0;
    let mut viewer_pos: Option<Vec3> = None;

    let net_id_for_entity = |entity: hecs::Entity| -> Option<rift_net::NetId> {
        world.get::<&NetIdentity>(entity).ok().map(|id| id.net_id)
    };

    // Players first — every connected player ships every snapshot,
    // EXCEPT ghosts (risen-but-dead): a ghost is owner-only, so
    // their row is dropped from any snapshot whose `ack_for`
    // isn't them. Living teammates therefore see no avatar /
    // nameplate / health bar for the ghost, which is what we
    // want for distraction-free spectating.
    for (e, (p, identity, vitals, kinematic)) in world
        .query::<(&ServerPlayer, &NetIdentity, &Vitals, &Kinematic)>()
        .iter()
    {
        if p.is_ghost && p.client_id != ack_for {
            continue;
        }
        let mut flags: u8 = 0;
        if kinematic.airborne {
            flags |= entity_flags::AIRBORNE;
        }
        if vitals.is_dead() {
            flags |= entity_flags::DEAD;
        }
        if p.is_ghost {
            flags |= entity_flags::GHOST;
        }
        let effects = world
            .get::<&super::effect::EffectStack>(e)
            .map(|s| s.to_snapshot())
            .unwrap_or_default();
        entities.push(EntitySnapshot {
            net_id: identity.net_id,
            kind: EntityKind::Player {
                client_id: p.client_id,
                aim_yaw: kinematic.aim_yaw,
                locomotion: kinematic.locomotion,
                action: kinematic.action,
                action_start: p.action_start,
            },
            target_net_id: None,
            position: kinematic.position.to_array(),
            yaw: kinematic.yaw,
            velocity: kinematic.velocity.to_array(),
            health_pct: vitals.health_pct(),
            resource_pct: if p.stats.max_resource > 0.0 {
                (p.resource / p.stats.max_resource).clamp(0.0, 1.0)
            } else {
                1.0
            },
            flags,
            effects,
        });
        if p.client_id == ack_for {
            ack_seq = p.last_input_seq;
            viewer_pos = Some(kinematic.position);
        }
    }

    // View-culled enemies. Anim byte is `ATTACK` while the swing
    // window is active, `WALK` while moving, `IDLE` otherwise. The
    // debuff bitmask comes from any `EffectStack` component the
    // enemy carries (every enemy gets one at spawn).
    for (e, (en, identity, vitals, kinematic)) in world
        .query::<(&ServerEnemy, &NetIdentity, &Vitals, &Kinematic)>()
        .iter()
    {
        if !in_view(viewer_pos, kinematic.position) {
            continue;
        }
        let dying = en.dying_remaining > 0.0;
        let anim = if dying {
            enemy_anim::DEATH
        } else if en.attack_anim_remaining > 0.0 {
            enemy_anim::ATTACK
        } else if kinematic.velocity.length_squared() > 0.01 {
            enemy_anim::WALK
        } else {
            enemy_anim::IDLE
        };
        let effects = world
            .get::<&super::effect::EffectStack>(e)
            .map(|s| s.to_snapshot())
            .unwrap_or_default();
        let mut flags = 0u8;
        if dying {
            flags |= entity_flags::DEAD;
        }
        entities.push(EntitySnapshot {
            net_id: identity.net_id,
            kind: EntityKind::Enemy {
                role: en.role.to_wire_byte(),
                anim,
            },
            target_net_id: en.target_lock.and_then(net_id_for_entity),
            position: kinematic.position.to_array(),
            yaw: kinematic.yaw,
            velocity: kinematic.velocity.to_array(),
            health_pct: vitals.health_pct(),
            resource_pct: 1.0,
            flags,
            effects,
        });
    }

    // View-culled friendly minions. They use monster visuals on
    // the client, but stay a distinct wire kind so UI/targeting
    // never mistakes them for enemies.
    for (_e, (minion, identity, vitals, kinematic)) in world
        .query::<(&ServerMinion, &NetIdentity, &Vitals, &Kinematic)>()
        .iter()
    {
        if !in_view(viewer_pos, kinematic.position) {
            continue;
        }
        let effects = world
            .get::<&super::effect::EffectStack>(_e)
            .map(|s| s.to_snapshot())
            .unwrap_or_default();
        entities.push(EntitySnapshot {
            net_id: identity.net_id,
            kind: EntityKind::Minion {
                role: minion.role.to_wire_byte(),
                owner: minion.owner_net_id,
                anim: minions::anim_byte(minion, kinematic),
            },
            target_net_id: minion.target_lock,
            position: kinematic.position.to_array(),
            yaw: kinematic.yaw,
            velocity: kinematic.velocity.to_array(),
            health_pct: vitals.health_pct(),
            resource_pct: (minion.lifetime_remaining / minion.lifetime_max.max(0.001))
                .clamp(0.0, 1.0),
            flags: 0,
            effects,
        });
    }

    // View-culled projectiles. Yaw is derived from velocity so
    // client meshes orient correctly.
    for (_e, proj) in world.query::<&ServerProjectile>().iter() {
        if !in_view(viewer_pos, proj.position) {
            continue;
        }
        let yaw = (-proj.velocity.x).atan2(-proj.velocity.z);
        entities.push(EntitySnapshot {
            net_id: proj.net_id,
            kind: EntityKind::Projectile {
                ability: proj.ability_id.raw() as u16,
            },
            target_net_id: None,
            position: proj.position.to_array(),
            yaw,
            velocity: proj.velocity.to_array(),
            health_pct: 1.0,
            resource_pct: 1.0,
            flags: 0,
            effects: Vec::new(),
        });
    }

    // View-culled loot drops. Replicated so a freshly-joined
    // client also sees items already on the floor — the
    // `LootDropped` event only fires once at spawn.
    for (_e, loot_row) in world.query::<&ServerLoot>().iter() {
        if !in_view(viewer_pos, loot_row.position) {
            continue;
        }
        let (base_id, rarity, ilvl, affixes, anchored, unique_id, unique_pick) =
            loot_row.item.to_wire();
        let provenance = loot_row
            .item
            .provenance
            .as_ref()
            .map(|p| p.eligible.clone());
        entities.push(EntitySnapshot {
            net_id: loot_row.net_id,
            kind: EntityKind::Loot {
                item: rift_net::messages::ItemBlob {
                    base_id,
                    rarity,
                    ilvl,
                    affixes,
                    anchored,
                    unstable: loot_row.item.unstable,
                    provenance,
                    unique_id: unique_id.map(|s| s.to_string()),
                    unique_pick,
                    rift_touched: loot_row.item.rift_touched_to_wire(),
                },
            },
            target_net_id: None,
            position: loot_row.position.to_array(),
            yaw: 0.0,
            velocity: [0.0; 3],
            health_pct: 1.0,
            resource_pct: 1.0,
            flags: 0,
            effects: Vec::new(),
        });
    }

    // Revive shrines. No view-culling: shrines are floor-wide
    // landmarks the HUD wants to render even from a distance
    // (and there's at most one per floor anyway).
    for (_e, shrine) in world.query::<&ServerReviveShrine>().iter() {
        let progress_norm =
            (shrine.progress / rift_net::messages::SHRINE_CHANNEL_DURATION).clamp(0.0, 1.0);
        let progress = (progress_norm * 255.0).round() as u8;
        entities.push(EntitySnapshot {
            net_id: shrine.net_id,
            kind: EntityKind::ReviveShrine {
                progress,
                channelers: shrine.channelers,
                required: shrine.required,
            },
            target_net_id: None,
            position: shrine.position.to_array(),
            yaw: 0.0,
            velocity: [0.0; 3],
            health_pct: 1.0,
            resource_pct: 1.0,
            flags: 0,
            effects: Vec::new(),
        });
    }

    Snapshot {
        tick,
        ack_seq,
        entities,
    }
}

fn in_view(viewer: Option<Vec3>, pos: Vec3) -> bool {
    let Some(vp) = viewer else { return true };
    let dx = pos.x - vp.x;
    let dz = pos.z - vp.z;
    dx * dx + dz * dz <= VIEW_RANGE_SQ
}
