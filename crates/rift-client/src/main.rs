use std::{net::SocketAddr, time::Duration};

use glam::Vec3;
use rift_client::game::{GameState, NetTransitionRequest};
use rift_client::net_client::{ClientProfile, NetClient};
use rift_engine::{App, Input, LoadStatus, Renderer, Window};
struct RiftApp {
    state: GameState,
    /// Optional networking session. `Some` only when the binary was
    /// launched with `--connect`. The rest of `update` runs exactly
    /// the same in both modes — Phase 2 just does the handshake and
    /// log; Phase 3 will start steering the world from snapshots.
    net: Option<NetClient>,
}

impl App for RiftApp {
    fn load_step(&mut self, renderer: &mut Renderer) -> anyhow::Result<LoadStatus> {
        self.state.load_step(renderer)
    }

    fn update(&mut self, renderer: &mut Renderer, input: &Input, dt: f32) {
        if let Some(net) = self.net.as_mut() {
            // Forward any pending roster lookup the player just
            // confirmed on the account-entry screen. This must
            // happen *before* `net.step` so the request goes out
            // on the same frame.
            if let Some(account_name) = self.state.pending_roster_request.take() {
                net.request_roster(account_name);
            }

            net.step(Duration::from_secs_f32(dt), Some(input));

            // Drain any roster reply the server has sent and feed
            // it into the character-select screen so it can leave
            // the loading view.
            if let Some(entries) = net.take_roster() {
                self.state.apply_server_roster(entries);
            }

            // Keep the SP regen path using the server's seed.
            self.state.net_floor_seed = Some(net.rift_seed());
            // Apply any server-driven floor transition BEFORE we
            // sync local/remote players, so freshly-spawned avatars
            // land in the new world rather than the old one.
            if let Some(pf) = net.take_pending_floor() {
                self.state.net_floor_seed = Some(pf.seed);
                self.state.apply_net_transition(renderer, pf.index);
                // Snap the freshly-spawned local Player to the
                // server's spawn point so the next snapshot's
                // correction error is small.
                use rift_engine::ecs::components::{LocalPlayer, Player, Transform};
                if let Some((entity, _)) = self
                    .state
                    .world
                    .query::<(&Player, &LocalPlayer)>()
                    .iter()
                    .map(|(e, _)| (e, ()))
                    .next()
                {
                    if let Ok(mut t) = self.state.world.get::<&mut Transform>(entity) {
                        t.position = pf.spawn_pos;
                    }
                }
            }
            // Stomp the local Player's Transform BEFORE the SP
            // update so `camera_follow_system` and
            // `render_sync_system` (both inside `state.update`)
            // observe the predicted position.
            net.sync_local_player(&mut self.state.world);
            // Reconcile remote-player ECS state from the latest
            // snapshot BEFORE `state.update`, so animation +
            // skinning + render-sync (all driven from inside
            // `state.update`) see the new Transform/Velocity for
            // remote players the same frame they arrive.
            net.sync_avatars(
                &mut self.state.world,
                renderer,
                &mut self.state.anim_cache,
            );
            net.sync_enemies(
                &mut self.state.world,
                renderer,
                &mut self.state.floor_mgr.monsters,
            );
            net.sync_projectiles(renderer);
            // Spawn loot-pillar visuals from snapshot rows. The
            // `LootDropped` event is the fast path for fresh kills
            // (zero-snapshot latency), but a freshly-joined client
            // also needs to see drops that were already on the
            // floor. `spawn_loot_drop_visual` is idempotent on
            // loot id so the two paths can both fire safely.
            for re in net.remote.values() {
                if let rift_net::messages::EntityKind::Loot { item } = &re.kind {
                    rift_client::game::state::spawn_loot_drop_visual(
                        &mut self.state,
                        renderer,
                        re.net_id,
                        re.position,
                        item.clone(),
                    );
                }
            }
            // Drain reliable world events from the server (damage
            // numbers, deaths, ability casts). For now we just log
            // damage — the SP `EnemyAnim.last_hp` system already
            // plays hit-react clips off the snapshot's `health_pct`
            // delta, so hit visuals come for free. Death animations
            // are similarly driven by the SP system once we cut the
            // entity's HP to zero on the client. Cast effects /
            // floating combat text land in a follow-up.
            for ev in net.drain_events() {
                use rift_net::messages::WorldEvent;
                match ev {
                    WorldEvent::Damage { target, amount, position, .. } => {
                        log::debug!(
                            "net: Damage target={target:?} amount={amount:.1} at {position:?}"
                        );
                    }
                    WorldEvent::Death { entity, .. } => {
                        log::info!("net: Death entity={entity:?}");
                        // Drop a blood decal at the dying entity's
                        // last known position. The snapshot may
                        // have already culled the row by the time
                        // the reliable Death event arrives, so we
                        // rely on `NetClient.last_positions` which
                        // persists across snapshots.
                        if let Some(&pos) = net.last_positions.get(&entity) {
                            self.state.decals.spawn_blood(
                                pos,
                                &self.state.wall_aabbs,
                                renderer,
                            );
                        }
                    }
                    WorldEvent::AbilityCast { caster, ability, dir, target, origin, .. } => {
                        log::debug!(
                            "net: AbilityCast caster={caster:?} ability={ability}"
                        );
                        let aim = glam::Vec3::new(dir[0], 0.0, dir[1]);
                        let cast_origin = glam::Vec3::from_array(origin);
                        let target_pos = target.map(glam::Vec3::from_array);
                        let def = rift_game::abilities::from_wire_id(ability as u8);

                        // AoE-zone visuals (e.g. Rain of Fire) need
                        // to play on *every* connected client \u2014
                        // including the caster, who currently
                        // returns out of `tick_combat` after sending
                        // the placement and never spawns the local
                        // emitter. Driving the visual off the
                        // server's `AbilityCast` event guarantees
                        // every observer sees the same effect at
                        // the same authoritative position.
                        if let Some(def) = &def {
                            rift_client::game::state::spawn_ability_aoe_visual(
                                renderer,
                                def,
                                cast_origin,
                                aim,
                                target_pos,
                            );
                        }

                        // Cast pose: skip our own cast \u2014
                        // `tick_combat` already triggered the local
                        // animation when the input fired. For remote
                        // avatars, look up their ECS entity and play
                        // the upper-body cast clip via
                        // `SpellCast.begin`.
                        if Some(caster) != net.our_net_id() {
                            if let Some(&entity) = net.avatar_entities.get(&caster) {
                                if let Some(def) = def {
                                    rift_client::game::state::trigger_remote_cast(
                                        &mut self.state.world,
                                        entity,
                                        &def,
                                        aim,
                                    );
                                }
                            }
                        }
                    }
                    WorldEvent::Hit { target, .. } => {
                        log::debug!("net: Hit target={target:?}");
                    }
                    WorldEvent::LootDropped { loot, item, position } => {
                        log::debug!(
                            "net: LootDropped loot={loot:?} base_id={} rarity={} at {:?}",
                            item.base_id,
                            item.rarity,
                            position
                        );
                        let pos = glam::Vec3::from_array(position);
                        rift_client::game::state::spawn_loot_drop_visual(
                            &mut self.state,
                            renderer,
                            loot,
                            pos,
                            item,
                        );
                    }
                    // Channel ticks/ends drive per-tick visuals
                    // (beam impact, whirlwind sweep). The cast
                    // animation itself is already running off
                    // the initial `AbilityCast` event; we don't
                    // need anything beyond a debug log today.
                    WorldEvent::ChannelTick { caster, ability, position, dir, .. } => {
                        log::trace!(
                            "net: ChannelTick caster={caster:?} ability={ability}"
                        );
                        // Drive per-tick beam visuals on whoever's
                        // channeling. Skipped here for our own
                        // entity \u2014 the local frame will pick the
                        // matching position+dir out of state.
                        let pos = glam::Vec3::from_array(position);
                        let aim = glam::Vec3::new(dir[0], 0.0, dir[1]);
                        rift_client::game::state::push_channel_visual(
                            &mut self.state,
                            caster,
                            ability as u8,
                            pos,
                            aim,
                        );
                    }
                    WorldEvent::ChannelEnd { caster, ability } => {
                        log::debug!(
                            "net: ChannelEnd caster={caster:?} ability={ability}"
                        );
                        // Tear down the cast pose on remote avatars
                        // (and on us if the server timed us out
                        // before the local timeout did).
                        if Some(caster) == net.our_net_id() {
                            self.state.active_channel = None;
                        }
                        let entity = if Some(caster) == net.our_net_id() {
                            self.state.world
                                .query::<(&rift_engine::ecs::components::Player, &rift_engine::ecs::components::LocalPlayer)>()
                                .iter()
                                .map(|(e, _)| e)
                                .next()
                        } else {
                            net.avatar_entities.get(&caster).copied()
                        };
                        if let Some(entity) = entity {
                            if let Ok(mut cast) = self
                                .state
                                .world
                                .get::<&mut rift_engine::ecs::components::SpellCast>(entity)
                            {
                                cast.cancel();
                            }
                        }
                        rift_client::game::state::clear_channel_visual(
                            &mut self.state,
                            caster,
                            ability as u8,
                        );
                    }
                }
            }
        }
        self.state.update(renderer, input, dt);
        // After SP code wrote the freshly-computed cursor aim onto
        // the local Player component, copy it to the net client so
        // the next outbound `InputCmd` ships it to the server. The
        // server then replicates it to the other clients via the
        // `EntityKind::Player.aim_yaw` field, keeping remote
        // spine-twists in sync with where this player is pointing.
        if let Some(net) = self.net.as_mut() {
            // Push the chosen profile to the wire as soon as
            // character-select completes. Until this fires the
            // `NetClient` holds back its `Hello`, so the server
            // never sees a placeholder profile.
            if let Some(profile) = self.state.pending_profile.take() {
                use rift_net::messages::Gender as NetGender;
                let gender = match profile.gender {
                    rift_game::character::Gender::Male => NetGender::Male,
                    rift_game::character::Gender::Female => NetGender::Female,
                };
                // Account name is filled in alongside the profile
                // by the character-select screen. Empty string is
                // a safety net — the entry view doesn't let an
                // empty name confirm, so this should never fire.
                let account_name = self
                    .state
                    .pending_account_name
                    .take()
                    .unwrap_or_default();
                net.set_profile(ClientProfile {
                    account_name,
                    character_name: profile.name,
                    class_id: profile.class.0.to_string(),
                    gender,
                });
            }
            let aim = self
                .state
                .world
                .query::<(&rift_engine::ecs::components::Player, &rift_engine::ecs::components::LocalPlayer)>()
                .iter()
                .map(|(_, (p, _))| p.aim_dir)
                .next()
                .unwrap_or(Vec3::ZERO);
            net.set_aim(aim);
            // Forward any SP-suppressed transition request to the
            // server. The actual world regeneration happens later
            // when the server's `LoadFloor` arrives.
            if let Some(req) = self.state.pending_net_request.take() {
                match req {
                    NetTransitionRequest::EnterRift => net.request_enter_rift(),
                    NetTransitionRequest::ReturnToHub => net.request_return_to_hub(),
                }
            }
            // Forward any locally-issued ability casts to the
            // server. Drained every frame so a held key doesn't
            // build up a backlog.
            for cast in self.state.pending_net_casts.drain(..) {
                net.request_cast(
                    cast.ability_id,
                    cast.origin,
                    cast.aim_dir,
                    cast.placed_target,
                );
            }
            // Forward channel-end requests (button release /
            // movement-cancel during a hold-to-channel ability).
            for ability_id in self.state.pending_end_channels.drain(..) {
                net.request_end_channel(ability_id);
            }
            // Forward loot-pickup requests. The server validates
            // range and broadcasts `LootClaimed` on success; we
            // tear down the visual + add the item to inventory
            // when that confirmation arrives.
            for loot_id in self.state.pending_loot_pickups.drain(..) {
                net.request_pickup_loot(loot_id);
            }
            // Apply loot-claim confirmations from the server.
            // `claimed_by == our_client_id` means *we* picked it
            // up; everyone else just removes the visual.
            let our_client_id = net.our_client_id();
            for (loot, claimed_by) in net.drain_loot_claims() {
                let mine = our_client_id == Some(claimed_by);
                self.state.resolve_loot_claim(renderer, loot, mine);
            }
        }
    }

    fn shutdown(&mut self, renderer: &mut Renderer) {
        self.state.shutdown(renderer);
    }
}

/// Parsed command-line arguments. Tiny, ad-hoc — clap is overkill for
/// two flags. Once we grow more options we'll graduate.
struct Args {
    connect: Option<SocketAddr>,
}

fn parse_args() -> Args {
    let mut connect: Option<SocketAddr> = None;
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--connect" => {
                let v = iter.next().expect("--connect requires an address");
                connect = Some(v.parse().expect("invalid --connect address"));
            }
            "--help" | "-h" => {
                eprintln!("rift [--connect host:port]");
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }
    Args { connect }
}

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let args = parse_args();

    let net = if let Some(addr) = args.connect {
        // Open the connection now so the handshake happens in
        // parallel with character-select. The cosmetic profile is
        // pushed via `set_profile` once the player picks one, which
        // also unblocks Hello.
        Some(NetClient::connect(addr)?)
    } else {
        None
    };

    let window = Window::new("Rift Crawler", 1280, 720);
    window.run(RiftApp {
        state: GameState::new(),
        net,
    })
}
