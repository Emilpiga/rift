//! Server-authoritative simulation: top-level orchestration.
//!
//! Submodules each own a slice of state (players, enemies,
//! projectiles, abilities, snapshots, floor lifecycle). This module
//! holds the [`Sim`] aggregate and the [`Sim::step`] loop that walks
//! the subsystems in order.
//!
//! Determinism: floor geometry comes from `rift_dungeon::Floor` —
//! the same generator the client runs — keyed by `(seed,
//! floor_index)`. We never replicate tiles or walls; clients
//! regenerate locally and trust the seed.

use std::collections::HashMap;

use glam::Vec3;
use hecs::Entity;
use rift_dungeon::Floor;
use rift_net::{
    messages::{InputCmd, Snapshot, WorldEvent},
    ClientId, NetId, NetTick,
};

pub mod ability;
pub mod channel;
pub mod debuff;
pub mod enemy;
pub mod floor;
pub mod loot;
pub mod player;
pub mod projectile;
pub mod snapshot;

pub use enemy::{enemy_anim, role, ServerEnemy};
pub use loot::ServerLoot;
pub use player::ServerPlayer;
pub use projectile::{ServerAoeZone, ServerProjectile};
pub use snapshot::VIEW_RANGE_SQ;

/// Maximum XZ distance (metres) between the picker and a ground
/// loot drop for a [`ClientMsg::PickUpLoot`] to succeed.
pub const PICKUP_RANGE: f32 = 2.0;

/// Top-level server simulation state. Owned by `Server`.
pub struct Sim {
    pub world: hecs::World,
    pub floor: Floor,
    pub floor_seed: u64,
    pub floor_index: u32,

    /// NetId allocators. Disjoint ranges so player / enemy /
    /// projectile / loot ids can never collide on the wire:
    /// - players:     `0x8000_0000..`     (high bit set)
    /// - enemies:     `0x0000_0001..0x2000_0000`
    /// - loot:        `0x2000_0000..0x4000_0000`
    /// - projectiles: `0x4000_0000..0x8000_0000`
    next_player_net_id: u32,
    next_enemy_net_id: u32,
    next_loot_net_id: u32,
    next_projectile_net_id: u32,

    /// Most recent input from each client, coalesced. Drained by
    /// `player::apply_inputs` on every step.
    pending_inputs: HashMap<ClientId, InputCmd>,
    /// `client_id → Entity` lookup so disconnect / input dispatch
    /// is O(1).
    sessions: HashMap<ClientId, Entity>,

    /// Active server-driven AoE zones (e.g. Rain of Arrows).
    aoe_zones: Vec<ServerAoeZone>,
    /// Per-client ability cooldowns.
    cooldowns: ability::CooldownTable,

    /// World events generated this tick. Drained by the server main
    /// loop and broadcast on `Channel::Event` (reliable).
    pending_events: Vec<WorldEvent>,
}

impl Sim {
    pub fn new(floor_seed: u64, floor_index: u32) -> Self {
        let floor = floor::generate(floor_seed, floor_index);
        let mut sim = Self {
            world: hecs::World::new(),
            floor,
            floor_seed,
            floor_index,
            next_player_net_id: 1,
            next_enemy_net_id: 1,
            next_loot_net_id: 0x2000_0000,
            next_projectile_net_id: 0x4000_0000,
            pending_inputs: HashMap::new(),
            sessions: HashMap::new(),
            aoe_zones: Vec::new(),
            cooldowns: HashMap::new(),
            pending_events: Vec::new(),
        };
        enemy::spawn_for_floor(
            &mut sim.world,
            &sim.floor,
            sim.floor_index,
            &mut sim.next_enemy_net_id,
        );
        sim
    }

    /// Switch to a different floor. Wipes all combat state and
    /// snaps every connected player to the new spawn position.
    /// Returns the spawn the server seated everyone at so the
    /// caller can put it in the broadcast `LoadFloor`.
    pub fn change_floor(&mut self, new_index: u32) -> Vec3 {
        self.floor_index = new_index;
        self.floor = floor::generate(self.floor_seed, new_index);
        let spawn = Vec3::new(self.floor.spawn_pos.x, 0.0, self.floor.spawn_pos.z);
        player::snap_all_to(&mut self.world, spawn);
        enemy::despawn_all(&mut self.world);
        projectile::despawn_all(&mut self.world);
        loot::despawn_all(&mut self.world);
        self.aoe_zones.clear();
        channel::clear_all(&mut self.world);
        ability::clear_cooldowns(&mut self.cooldowns);
        self.pending_inputs.clear();
        enemy::spawn_for_floor(
            &mut self.world,
            &self.floor,
            self.floor_index,
            &mut self.next_enemy_net_id,
        );
        log::info!(
            "sim: changed to floor {new_index} (seed={}) at spawn {spawn:?}",
            self.floor_seed
        );
        spawn
    }

    /// Spawn (or look up the existing) player entity for a freshly-
    /// Helloed client. Returns the allocated `NetId`.
    pub fn spawn_player(&mut self, client_id: ClientId) -> NetId {
        if let Some(&existing) = self.sessions.get(&client_id) {
            if let Ok(p) = self.world.get::<&ServerPlayer>(existing) {
                return p.net_id;
            }
        }
        let net_id = NetId(self.next_player_net_id | 0x8000_0000);
        self.next_player_net_id = self.next_player_net_id.wrapping_add(1).max(1);
        let spawn = Vec3::new(self.floor.spawn_pos.x, 0.0, self.floor.spawn_pos.z);
        let entity = self
            .world
            .spawn((ServerPlayer::fresh(client_id, net_id, spawn),));
        self.sessions.insert(client_id, entity);
        log::info!("sim: spawned player {client_id:?} as {net_id:?} at {spawn:?}");
        net_id
    }

    pub fn despawn_player(&mut self, client_id: ClientId) {
        if let Some(entity) = self.sessions.remove(&client_id) {
            let _ = self.world.despawn(entity);
            log::info!("sim: despawned player {client_id:?}");
        }
        self.pending_inputs.remove(&client_id);
        self.cooldowns.remove(&client_id);
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
        aim_dir: [f32; 2],
        placed_target: Option<[f32; 3]>,
        tick: NetTick,
    ) {
        ability::cast(
            &mut self.world,
            &self.sessions,
            &mut self.cooldowns,
            &mut self.aoe_zones,
            &mut self.pending_events,
            &mut self.next_projectile_net_id,
            client_id,
            ability_id,
            aim_dir,
            placed_target,
            tick,
        );
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
        );
    }

    /// Try to claim a ground-loot drop for `client_id`. Validates
    /// the picker is within [`PICKUP_RANGE`] of the loot row;
    /// rolls back silently on missing entity / out-of-range. On
    /// success returns the rolled [`rift_game::loot::Item`] (so
    /// the caller can persist it to the player's inventory) and
    /// despawns the loot entity \u2014 the caller broadcasts the
    /// claim so every client can drop their visual.
    pub fn try_pickup_loot(
        &mut self,
        client_id: ClientId,
        loot: NetId,
    ) -> Option<rift_game::loot::Item> {
        let &player_entity = self.sessions.get(&client_id)?;
        let player_pos = self
            .world
            .get::<&ServerPlayer>(player_entity)
            .ok()?
            .k
            .position;

        // Find the loot ECS entity by net id.
        let target = self
            .world
            .query::<&loot::ServerLoot>()
            .iter()
            .find(|(_, l)| l.net_id == loot)
            .map(|(e, l)| (e, l.position, l.item.clone()))?;
        let (loot_entity, loot_pos, item) = target;

        let dx = loot_pos.x - player_pos.x;
        let dz = loot_pos.z - player_pos.z;
        if dx * dx + dz * dz > PICKUP_RANGE * PICKUP_RANGE {
            return None;
        }
        let _ = self.world.despawn(loot_entity);
        // Push onto the picker's `ServerPlayer.inventory` so the
        // authoritative server-side bag stays in sync with what
        // the client mirrors. Long-term DB persistence is handled
        // by the caller (server main) which also has the
        // `PersistenceHandle`.
        if let Ok(mut p) = self.world.get::<&mut ServerPlayer>(player_entity) {
            p.inventory.push(item.clone());
            log::debug!(
                "sim: inventory for {:?} now has {} item(s)",
                client_id,
                p.inventory.len()
            );
        }
        Some(item)
    }

    /// Hydrate a freshly-spawned player's inventory from a
    /// pre-loaded list (typically the rows fetched by
    /// `PersistenceHandle::load_inventory_blocking`). Idempotent;
    /// replaces whatever was there. Called once during the
    /// `Hello` handshake right after `spawn_player`.
    pub fn set_player_inventory(
        &mut self,
        client_id: ClientId,
        items: Vec<rift_game::loot::Item>,
    ) {
        let Some(&entity) = self.sessions.get(&client_id) else {
            return;
        };
        if let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) {
            p.inventory = items;
        }
    }

    /// Drain world events generated this tick. Caller broadcasts on
    /// `Channel::Event`.
    pub fn drain_events(&mut self) -> Vec<WorldEvent> {
        std::mem::take(&mut self.pending_events)
    }

    /// Advance the simulation by one fixed timestep. `tick` is the
    /// server's current monotonic tick counter — channel ticks
    /// stamp it into their `WorldEvent::ChannelTick` so clients can
    /// interpolate against snapshot timing.
    pub fn step(&mut self, dt: f32, tick: NetTick) {
        // 1. Players: ingest inputs, integrate motion.
        player::apply_inputs(&mut self.world, &self.sessions, &mut self.pending_inputs);
        player::integrate_motion(&mut self.world, &self.floor, dt);

        // 2. Enemies: AI tick (queues melee damage), then
        //    integrate motion.
        let player_targets = player::target_positions(&self.world);
        let melee_damage = enemy::tick_ai(&mut self.world, &player_targets, dt);
        enemy::integrate_motion(&mut self.world, &self.floor, dt);

        // 3. Apply queued enemy → player melee damage.
        apply_player_damage(&mut self.world, &mut self.pending_events, melee_damage);

        // 4. Tick ability cooldowns.
        ability::tick_cooldowns(&mut self.cooldowns, dt);

        // 5. Snapshot enemies for collision queries, then run
        //    projectiles + AoE zones + channels against them.
        //    All damage paths share one `DeathCtx` so DoT and
        //    direct kills both run through `loot::finalise_kills`
        //    (which emits `Death`, rolls drops, and despawns).
        let enemies = enemy::snapshot_for_collision(&self.world);
        let mut ctx = loot::DeathCtx {
            events: &mut self.pending_events,
            next_loot_net_id: &mut self.next_loot_net_id,
            tick,
            floor_index: self.floor_index,
        };
        projectile::tick(&mut self.world, &enemies, &mut ctx, dt);
        projectile::tick_aoe(
            &mut self.world,
            &mut self.aoe_zones,
            &enemies,
            &mut ctx,
            dt,
        );
        channel::tick(
            &mut self.world,
            &enemies,
            &mut ctx,
            tick,
            dt,
        );

        // 6. Tick debuff stacks: decay durations, fire DoT damage,
        //    drop expired entries. Runs last so DoT events ride
        //    out on this frame's snapshot.
        debuff::tick(&mut self.world, &mut ctx, dt);

        // 7. Death-fade: tick the death timer on dying enemies and
        //    despawn rows whose timer hit zero. Kept separate from
        //    the kill path so the corpse stays in snapshots long
        //    enough for the client to play its `Death` clip.
        enemy::tick_dying(&mut self.world, dt);
    }

    /// Build the snapshot for one receiving client.
    pub fn build_snapshot(&self, tick: NetTick, ack_for: ClientId) -> Snapshot {
        snapshot::build(&self.world, tick, ack_for)
    }
}

/// Apply queued enemy → player melee damage and emit one `Damage`
/// event per applied hit.
fn apply_player_damage(
    world: &mut hecs::World,
    events: &mut Vec<WorldEvent>,
    pending: Vec<(Entity, f32)>,
) {
    for (player_entity, amount) in pending {
        if let Ok(mut p) = world.get::<&mut ServerPlayer>(player_entity) {
            p.hp = (p.hp - amount).max(0.0);
            let pos = p.k.position;
            let net_id = p.net_id;
            drop(p);
            events.push(WorldEvent::Damage {
                target: net_id,
                amount,
                crit: false,
                position: pos.to_array(),
            });
        }
    }
}
