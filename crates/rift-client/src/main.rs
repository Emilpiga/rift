use std::{net::SocketAddr, time::Duration};

use glam::Vec3;
use rift_client::game::{EquipRequest, GameState, NetTransitionRequest, StashRequest};
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
        // The frame is split into named phases so the dispatch
        // table stays readable as we add features. Order matters:
        //
        //   1. `net_pre_phase` drives the network layer and applies
        //      any server-pushed floor transition before the SP
        //      update sees it.
        //   2. `sync_entities_from_snapshot` writes the latest
        //      snapshot into the ECS so animation / camera /
        //      render-sync (run inside `state.update`) observe
        //      authoritative transforms this frame.
        //   3. `handle_world_events` drains reliable events and
        //      spawns combat-text, blood decals, AoE visuals, etc.
        //   4. `state.update` runs the regular SP frame.
        //   5. `forward_client_commands` ships the user's
        //      intentions (input, casts, pickups, equip swaps).
        //   6. `apply_server_pushed_state` integrates server
        //      replies that are safe to apply after the SP frame
        //      (inventory / equipment / XP / progress / claims).
        if self.net.is_some() {
            self.net_pre_phase(renderer, input, dt);
            self.sync_entities_from_snapshot(renderer, dt);
            self.handle_world_events(renderer);
        }

        self.state.update(renderer, input, dt);

        if self.net.is_some() {
            self.forward_client_commands();
            self.apply_server_pushed_state(renderer);
        }
    }

    fn shutdown(&mut self, renderer: &mut Renderer) {
        self.state.shutdown(renderer);
    }
}

/// Per-phase helpers for [`RiftApp::update`]. Pulled out so each
/// phase's responsibility is named and individually skimmable.
/// Every helper short-circuits when `self.net` is `None`, so SP /
/// offline mode pays nothing for the split.
impl RiftApp {
    /// Drive the network layer and apply any server-driven floor
    /// transition. Has to run before [`Self::sync_entities_from_snapshot`]
    /// so freshly-spawned avatars land in the new world rather
    /// than the old one.
    fn net_pre_phase(&mut self, renderer: &mut Renderer, input: &Input, dt: f32) {
        let Self { state, net } = self;
        let Some(net) = net.as_mut() else { return };

        // Forward any pending roster lookup the player just
        // confirmed on the account-entry screen. Must happen
        // before `net.step` so the request goes out on the same
        // frame.
        if let Some(account_name) = state.net.roster_request.take() {
            net.request_roster(account_name);
        }

        net.step(Duration::from_secs_f32(dt), Some(input));

        // Drain any roster reply the server has sent and feed it
        // into the character-select screen so it can leave the
        // loading view.
        if let Some(entries) = net.take_roster() {
            state.apply_server_roster(entries);
        }

        // Keep the SP regen path using the server's seed.
        state.net.floor_seed = Some(net.rift_seed());

        // Apply any server-driven floor transition.
        if let Some(pf) = net.take_pending_floor() {
            state.net.floor_seed = Some(pf.seed);
            state.apply_net_transition(renderer, pf.index);
            // Snap the freshly-spawned local Player to the
            // server's spawn point so the next snapshot's
            // correction error is small.
            use rift_engine::ecs::components::{LocalPlayer, Player, Transform};
            if let Some((entity, _)) = state
                .world
                .query::<(&Player, &LocalPlayer)>()
                .iter()
                .map(|(e, _)| (e, ()))
                .next()
            {
                if let Ok(mut t) = state.world.get::<&mut Transform>(entity) {
                    t.position = pf.spawn_pos;
                }
            }
        }
    }

    /// Reconcile every ECS entity that's driven off the latest
    /// snapshot: the local player's predicted transform, remote
    /// avatars (animation + skinning), enemies, projectiles, and
    /// loot pillars. Must run before `state.update` so the SP
    /// camera + render-sync see authoritative positions this
    /// frame.
    fn sync_entities_from_snapshot(&mut self, renderer: &mut Renderer, dt: f32) {
        let Self { state, net } = self;
        let Some(net) = net.as_mut() else { return };

        net.sync_local_player(&mut state.world);
        net.sync_avatars(&mut state.world, renderer, &mut state.anim_cache);
        net.sync_enemies(
            &mut state.world,
            renderer,
            &mut state.floor_mgr.monsters,
        );
        net.sync_projectiles(renderer, dt);

        // Spawn loot-pillar visuals from snapshot rows. The
        // `LootDropped` event is the fast path for fresh kills
        // (zero-snapshot latency), but a freshly-joined client
        // also needs to see drops that were already on the
        // floor. `spawn_loot_drop_visual` is idempotent on loot
        // id so the two paths can both fire safely.
        for re in net.remote.values() {
            if let rift_net::messages::EntityKind::Loot { item } = &re.kind {
                // Skip rows whose claim we've already resolved —
                // a snapshot in flight when the server despawned
                // the loot would otherwise resurrect the pillar
                // after `resolve_loot_claim` tore it down.
                if state.loot.claimed_ids.contains(&re.net_id) {
                    continue;
                }
                rift_client::game::state::on_loot_dropped(
                    &mut state.loot,
                    renderer,
                    re.net_id,
                    re.position,
                    item.clone(),
                );
            }
        }
    }

    /// Drain reliable world events from the server and dispatch
    /// each one to a kind-specific handler. Most are visual side
    /// effects (combat text, blood decals, AoE emitters); damage
    /// is the only one with an authoritative gameplay effect on
    /// the client (the local Health component, mirrored from
    /// `health_pct` in the snapshot).
    fn handle_world_events(&mut self, renderer: &mut Renderer) {
        // Drain into a local Vec so we don't hold a mutable
        // borrow on `self.net` while the per-event handlers also
        // need to touch it (read `our_net_id`, `avatar_entities`,
        // `last_positions`).
        let events = match self.net.as_mut() {
            Some(net) => net.drain_events(),
            None => return,
        };
        for ev in events {
            self.handle_world_event(ev, renderer);
        }
    }

    fn handle_world_event(
        &mut self,
        ev: rift_net::messages::WorldEvent,
        renderer: &mut Renderer,
    ) {
        use rift_net::messages::WorldEvent;
        match ev {
            WorldEvent::Damage { target, amount, crit, position } => {
                self.handle_damage_event(target, amount, crit, position, renderer);
            }
            WorldEvent::Death { entity, .. } => {
                self.handle_death_event(entity, renderer);
            }
            WorldEvent::AbilityCast { caster, ability, dir, target, origin, .. } => {
                self.handle_ability_cast_event(caster, ability, dir, target, origin, renderer);
            }
            WorldEvent::Hit { target, .. } => {
                log::debug!("net: Hit target={target:?}");
            }
            WorldEvent::LootDropped { loot, item, position } => {
                self.handle_loot_dropped_event(loot, item, position, renderer);
            }
            WorldEvent::ChannelTick { caster, ability, position, dir, .. } => {
                self.handle_channel_tick_event(caster, ability, position, dir);
            }
            WorldEvent::ChannelEnd { caster, ability } => {
                self.handle_channel_end_event(caster, ability);
            }
        }
    }

    /// Spawn floating combat text for damage we just took or
    /// dealt. Direct hits go through `spawn_player_damage` so
    /// they're styled distinctly from damage we dealt out.
    fn handle_damage_event(&mut self, target: rift_net::NetId, amount: f32, crit: bool, position: [f32; 3], renderer: &mut Renderer) {
        let Self { state, net } = self;
        let Some(net) = net.as_mut() else { return };
        let world_pos = Vec3::from_array(position);
        if Some(target) == net.our_net_id() {
            state.combat_text.spawn_player_damage(world_pos, amount);
        } else {
            state.combat_text.spawn_damage(world_pos, amount, crit);
            // Visceral blood spurt on enemy hits. Anchored a
            // little above the snapshot position so droplets fly
            // from chest height rather than the feet. Skipped
            // for self-hits — the local player already has the
            // red vignette to communicate "you got hit".
            renderer.vfx_system.spawn(
                rift_engine::renderer::vfx::presets::blood_hit_spurt(Vec3::Y),
                world_pos + Vec3::new(0.0, 1.0, 0.0),
            );
        }
    }

    /// Drop a blood decal at the dying entity's last known
    /// position. The snapshot may have already culled the row by
    /// the time the reliable Death event arrives, so we rely on
    /// `NetClient.last_positions` which persists across
    /// snapshots.
    fn handle_death_event(&mut self, entity: rift_net::NetId, renderer: &mut Renderer) {
        log::info!("net: Death entity={entity:?}");
        let Self { state, net } = self;
        let Some(net) = net.as_mut() else { return };
        if let Some(&pos) = net.last_positions.get(&entity) {
            // Persistent floor stain.
            state.decals.spawn_blood(pos, &state.wall_aabbs, renderer);
            // Big visceral burst on top of it. Anchored at
            // chest height so the upward cone reads as the kill
            // shot rather than ground splatter.
            renderer.vfx_system.spawn(
                rift_engine::renderer::vfx::presets::blood_splatter(Vec3::Y),
                pos + Vec3::new(0.0, 1.0, 0.0),
            );
        }
        // Remote player death: play the death clip on their
        // avatar so observers see them topple instead of just
        // freezing in their last pose. The local player's death
        // clip is driven from `trigger_player_death` (catch-all
        // health detect) since the snapshot's hp=0 row arrives
        // before the reliable Death event.
        if Some(entity) != net.our_net_id() {
            if let Some(&avatar) = net.avatar_entities.get(&entity) {
                rift_client::game::state::on_remote_death(&mut state.world, avatar);
            }
        }
    }

    /// Spawn the AoE-zone visual for a server-confirmed cast and
    /// trigger the upper-body cast pose on remote casters. The
    /// local caster's pose is already running from `tick_combat`
    /// the moment the input fired, so we pass `caster_avatar =
    /// None` for our own casts to skip the pose hop.
    fn handle_ability_cast_event(
        &mut self,
        caster: rift_net::NetId,
        ability: u16,
        dir: [f32; 2],
        target: Option<[f32; 3]>,
        origin: [f32; 3],
        renderer: &mut Renderer,
    ) {
        log::debug!("net: AbilityCast caster={caster:?} ability={ability}");
        let aim = Vec3::new(dir[0], 0.0, dir[1]);
        let cast_origin = Vec3::from_array(origin);
        let target_pos = target.map(Vec3::from_array);
        let Some(def) = rift_game::abilities::from_wire_id(ability as u8) else {
            return;
        };

        let Self { state, net } = self;
        let Some(net) = net.as_mut() else { return };
        let caster_avatar = if Some(caster) == net.our_net_id() {
            None
        } else {
            net.avatar_entities.get(&caster).copied()
        };
        rift_client::game::state::on_remote_ability_cast(
            state,
            renderer,
            &def,
            aim,
            cast_origin,
            target_pos,
            caster_avatar,
        );
    }

    fn handle_loot_dropped_event(
        &mut self,
        loot: rift_net::NetId,
        item: rift_net::messages::ItemBlob,
        position: [f32; 3],
        renderer: &mut Renderer,
    ) {
        log::debug!(
            "net: LootDropped loot={loot:?} base_id={} rarity={} at {:?}",
            item.base_id,
            item.rarity,
            position
        );
        let pos = Vec3::from_array(position);
        rift_client::game::state::on_loot_dropped(
            &mut self.state.loot,
            renderer,
            loot,
            pos,
            item,
        );
    }

    /// Per-tick channel visuals (beam impact, whirlwind sweep).
    /// The cast pose is already running off the initial
    /// `AbilityCast`; this just keeps the per-tick effect
    /// position+aim fresh on every observer.
    fn handle_channel_tick_event(
        &mut self,
        caster: rift_net::NetId,
        ability: u16,
        position: [f32; 3],
        dir: [f32; 2],
    ) {
        log::trace!("net: ChannelTick caster={caster:?} ability={ability}");
        let pos = Vec3::from_array(position);
        let aim = Vec3::new(dir[0], 0.0, dir[1]);
        rift_client::game::state::on_channel_tick(
            &mut self.state,
            caster,
            ability as u8,
            pos,
            aim,
        );
    }

    /// Tear down the cast pose on remote avatars (and on us if
    /// the server timed us out before the local timeout did).
    fn handle_channel_end_event(&mut self, caster: rift_net::NetId, ability: u16) {
        log::debug!("net: ChannelEnd caster={caster:?} ability={ability}");
        let Self { state, net } = self;
        let Some(net) = net.as_mut() else { return };

        let is_local = Some(caster) == net.our_net_id();
        let entity = if is_local {
            state
                .world
                .query::<(
                    &rift_engine::ecs::components::Player,
                    &rift_engine::ecs::components::LocalPlayer,
                )>()
                .iter()
                .map(|(e, _)| e)
                .next()
        } else {
            net.avatar_entities.get(&caster).copied()
        };
        rift_client::game::state::on_channel_end(state, caster, ability as u8, entity, is_local);
    }

    /// Forward this frame's local intentions to the server:
    /// profile + aim, hub↔rift transitions, ability casts,
    /// channel ends, loot pickups, and equip / unequip
    /// requests. None of these read server-pushed state, so
    /// they all live in one phase.
    fn forward_client_commands(&mut self) {
        let Self { state, net } = self;
        let Some(net) = net.as_mut() else { return };

        // Push the chosen profile to the wire as soon as
        // character-select completes. Until this fires the
        // `NetClient` holds back its `Hello`, so the server
        // never sees a placeholder profile.
        if let Some(profile) = state.net.profile.take() {
            use rift_net::messages::Gender as NetGender;
            let gender = match profile.gender {
                rift_game::character::Gender::Male => NetGender::Male,
                rift_game::character::Gender::Female => NetGender::Female,
            };
            // Account name is filled in alongside the profile by
            // the character-select screen. Empty string is a
            // safety net — the entry view doesn't let an empty
            // name confirm, so this should never fire.
            let account_name = state.net.account_name.take().unwrap_or_default();
            net.set_profile(ClientProfile {
                account_name,
                character_name: profile.name,
                class_id: "hero".to_string(),
                gender,
            });
        }

        // After SP code wrote the freshly-computed cursor aim
        // onto the local Player component, copy it to the net
        // client so the next outbound `InputCmd` ships it to
        // the server. The server then replicates it via the
        // `EntityKind::Player.aim_yaw` field, keeping remote
        // spine-twists in sync with where this player is
        // pointing.
        let aim = state
            .world
            .query::<(
                &rift_engine::ecs::components::Player,
                &rift_engine::ecs::components::LocalPlayer,
            )>()
            .iter()
            .map(|(_, (p, _))| p.aim_dir)
            .next()
            .unwrap_or(Vec3::ZERO);
        net.set_aim(aim);

        // Floor transition requests. The actual world
        // regeneration happens later when the server's
        // `LoadFloor` arrives.
        if let Some(req) = state.net.transition.take() {
            match req {
                NetTransitionRequest::EnterRift => net.request_enter_rift(),
                NetTransitionRequest::ReturnToHub => net.request_return_to_hub(),
            }
        }

        // Locally-issued ability casts — drained every frame so
        // a held key doesn't build up a backlog.
        for cast in state.net.casts.drain(..) {
            net.request_cast(cast.ability_id, cast.origin, cast.aim_dir, cast.placed_target);
        }

        // Channel-end requests (button release / movement-cancel
        // during a hold-to-channel ability).
        for ability_id in state.channel.pending_ends.drain(..) {
            net.request_end_channel(ability_id);
        }

        // Loot-pickup requests. The server validates range and
        // broadcasts `LootClaimed` on success; we tear down the
        // visual + add the item to inventory when that
        // confirmation arrives.
        for loot_id in state.loot.pending_pickups.drain(..) {
            net.request_pickup_loot(loot_id);
        }

        // Equip / unequip requests. UI pushes these in response
        // to clicks; the server is authoritative and replies
        // with fresh Inventory/Equipment syncs.
        for req in state
            .loot
            .pending_equip_requests
            .drain(..)
            .collect::<Vec<_>>()
        {
            match req {
                EquipRequest::Equip { inventory_index } => {
                    net.request_equip_item(inventory_index);
                }
                EquipRequest::Unequip { slot } => {
                    net.request_unequip_item(slot);
                }
                EquipRequest::SwapBag { a, b } => {
                    net.request_swap_inventory_slots(a, b);
                }
                EquipRequest::DropToWorld { inventory_index } => {
                    net.request_drop_inventory_item(inventory_index);
                }
                EquipRequest::UnequipToSlot { slot, inventory_index } => {
                    net.request_unequip_to_bag_slot(slot, inventory_index);
                }
            }
        }

        // Stash session toggles. Pushed by the F-prompt at the
        // hub chest. `true` opens, `false` closes.
        for open in state.net.stash_session_requests.drain(..).collect::<Vec<_>>() {
            if open {
                net.request_open_stash();
            } else {
                net.request_close_stash();
            }
        }

        // Loadout-slot changes. Pushed by the spellbook UI; the
        // server is authoritative and replies with a fresh
        // `ServerMsg::Loadout`.
        for (slot_index, ability_id) in state
            .net
            .pending_loadout_changes
            .drain(..)
            .collect::<Vec<_>>()
        {
            net.request_set_loadout_slot(slot_index, ability_id);
        }

        // Stash transfer requests (deposit / withdraw). Drained
        // alongside equip requests; the server replies with
        // fresh InventorySync + StashSync.
        for req in state
            .loot
            .pending_stash_requests
            .drain(..)
            .collect::<Vec<_>>()
        {
            match req {
                StashRequest::Deposit { inventory_index } => {
                    net.request_deposit_to_stash(inventory_index);
                }
                StashRequest::Withdraw { stash_index } => {
                    net.request_withdraw_from_stash(stash_index);
                }
                StashRequest::Swap { a, b } => {
                    net.request_swap_stash_slots(a, b);
                }
                StashRequest::DepositToSlot {
                    inventory_index,
                    stash_index,
                } => {
                    net.request_deposit_to_stash_slot(inventory_index, stash_index);
                }
                StashRequest::WithdrawToSlot {
                    stash_index,
                    inventory_index,
                } => {
                    net.request_withdraw_from_stash_slot(stash_index, inventory_index);
                }
            }
        }
    }

    /// Apply server-pushed UI state that's safe to integrate
    /// after the SP frame ran: loot-claim confirmations,
    /// inventory + equipment mirrors, the XP bar, and the rift
    /// progress meter. None of these affect input handling for
    /// the next frame, so they live here instead of in
    /// `net_pre_phase`.
    fn apply_server_pushed_state(&mut self, renderer: &mut Renderer) {
        let Self { state, net } = self;
        let Some(net) = net.as_mut() else { return };

        // Loot-claim confirmations. `claimed_by == our_client_id`
        // means *we* picked it up; everyone else just removes
        // the visual.
        let our_client_id = net.our_client_id();
        for (loot, claimed_by) in net.drain_loot_claims() {
            let mine = our_client_id == Some(claimed_by);
            state.resolve_loot_claim(renderer, loot, mine);
        }

        // Loot-pickup rejections (server enforced cap, etc.).
        // The drop is left on the ground; we drop our pending
        // request and warn the player so they know to make room.
        for (loot, reason) in net.drain_pickup_rejections() {
            state.loot.pending_pickups.retain(|id| *id != loot);
            match reason {
                rift_net::messages::PickupRejectReason::InventoryFull => {
                    state.warn_inventory_full();
                }
            }
        }

        // Authoritative inventory mirror. Items arrive as
        // `ItemBlob` so we round-trip through `Item::from_wire`;
        // rows that fail to decode (mismatched build) are
        // dropped — the next sync will correct it.
        if let Some(blobs) = net.drain_inventory_sync() {
            let items: Vec<Option<rift_game::loot::Item>> = blobs
                .into_iter()
                .map(|opt| {
                    opt.and_then(|b| {
                        rift_game::loot::Item::from_wire(b.base_id, b.rarity, b.ilvl, &b.affixes)
                    })
                })
                .collect();
            let filled = items.iter().filter(|s| s.is_some()).count();
            log::info!("client: hydrated mp_inventory with {} item(s)", filled);
            state.loot.items = items;
        }

        // Authoritative equipment mirror. Same shape as
        // inventory; failed-to-decode rows are dropped.
        if let Some(slots) = net.drain_equipment_sync() {
            let mut equip = rift_game::loot::Equipment::new();
            for (slot_byte, blob) in slots {
                let Some(slot) = rift_game::loot::EquipSlot::from_u8(slot_byte) else {
                    continue;
                };
                let Some(item) = rift_game::loot::Item::from_wire(
                    blob.base_id,
                    blob.rarity,
                    blob.ilvl,
                    &blob.affixes,
                ) else {
                    continue;
                };
                equip.set(slot, Some(item));
            }
            log::info!("client: hydrated equipment with {} slot(s)", equip.count());
            state.loot.equipment = equip;
            state.player_state.recompute_stats(&state.loot.equipment);
        }

        // Authoritative stash mirror. Decoded the same way as
        // the bag — failed-to-decode rows are dropped and the
        // next sync corrects.
        if let Some(blobs) = net.drain_stash_sync() {
            let items: Vec<Option<rift_game::loot::Item>> = blobs
                .into_iter()
                .map(|opt| {
                    opt.and_then(|b| {
                        rift_game::loot::Item::from_wire(b.base_id, b.rarity, b.ilvl, &b.affixes)
                    })
                })
                .collect();
            let filled = items.iter().filter(|s| s.is_some()).count();
            log::info!("client: hydrated stash with {} item(s)", filled);
            state.loot.stash_items = items;
        }

        // Authoritative XP / level snapshots.
        if let Some((level, xp, xp_to_next)) = net.drain_character_stats() {
            let prev_level = state.player_state.experience.level;
            state.player_state.experience.level = level.max(1);
            state.player_state.experience.current_xp = xp;
            state.player_state.experience.total_xp =
                state.player_state.experience.total_xp.max(xp);
            state.player_state.experience.set_xp_to_next(xp_to_next);
            if prev_level != level {
                state.player_state.recompute_stats(&state.loot.equipment);
                state.level_up_flash = 1.0;
                log::info!("client: leveled up to {level}");
            }
        }

        // Authoritative ability-loadout snapshots. Server is the
        // source of truth; mutate `PlayerState::loadout` and
        // re-materialize the runtime `AbilitySlot`.
        if let Some(slots) = net.drain_loadout() {
            state.player_state.loadout = rift_game::loadout::Loadout::from_slots(slots);
            state.player_state.abilities = state.player_state.loadout.materialize();
        }

        // Authoritative rift-progress snapshots.
        if let Some((progress, required, boss_spawned, boss_killed, complete)) =
            net.drain_rift_progress()
        {
            state.rift.progress = progress as f32;
            state.rift.progress_required = required.max(1) as f32;
            state.rift.boss_spawned = boss_spawned;
            state.rift.boss_killed = boss_killed;
            state.rift.floor_complete = complete;
        }
    }
}

/// Parsed command-line arguments. Tiny, ad-hoc — clap is overkill for
/// two flags. Once we grow more options we'll graduate.
struct Args {
    connect: Option<SocketAddr>,
}

/// Compile-time default server address baked into the client. Set
/// at build time via the `RIFT_DEFAULT_SERVER` env var (read by
/// `build.rs`); falls back to the local dev server. Players who
/// just double-click `rift.exe` connect here without needing a
/// flag. Override at runtime with `--connect`, `RIFT_SERVER`, or
/// `--offline`.
const DEFAULT_SERVER: Option<&str> = option_env!("RIFT_DEFAULT_SERVER");

fn parse_args() -> Args {
    let mut connect: Option<SocketAddr> = None;
    let mut explicit_offline = false;
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--connect" => {
                let v = iter.next().expect("--connect requires an address");
                connect = Some(v.parse().expect("invalid --connect address"));
            }
            "--offline" => {
                explicit_offline = true;
            }
            "--help" | "-h" => {
                eprintln!(
                    "rift [--connect host:port] [--offline]\n\
                     \n\
                     Defaults to the server baked in at build time\n\
                     (RIFT_DEFAULT_SERVER), or to the RIFT_SERVER env\n\
                     var if set. Pass --offline to skip multiplayer\n\
                     entirely."
                );
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }
    // Resolution order: explicit --connect > $RIFT_SERVER >
    // compile-time default. --offline trumps everything.
    if connect.is_none() && !explicit_offline {
        if let Ok(env_addr) = std::env::var("RIFT_SERVER") {
            if !env_addr.is_empty() {
                connect = Some(
                    env_addr
                        .parse()
                        .expect("invalid RIFT_SERVER address"),
                );
            }
        }
    }
    if connect.is_none() && !explicit_offline {
        if let Some(default) = DEFAULT_SERVER {
            connect = Some(
                default
                    .parse()
                    .expect("invalid RIFT_DEFAULT_SERVER baked at build time"),
            );
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
