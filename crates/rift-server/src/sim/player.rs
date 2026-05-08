//! Connected-player state, input ingestion, and movement integration.
//!
//! Players are pure data: no AI, no scripted state machines. Every
//! tick the latest coalesced `InputCmd` is fed through the shared
//! `rift-game` integrator (which the client mirrors verbatim for
//! prediction).

use std::collections::HashMap;

use hecs::Entity;
use rift_dungeon::Floor;
use rift_net::{
    messages::{button_bits, InputCmd},
    ClientId, NetId,
};
use rift_game::attributes::Attributes;
use rift_game::experience::{Experience, LevelUpReward};
use rift_game::hero::HERO;
use rift_game::kinematic::{self, loco, Kinematic};
use rift_game::loadout::Loadout;
use rift_game::stats::CharacterStats;

/// Default per-player level until the persisted level field is
/// wired through. Drives `CharacterStats::compute`.
pub const DEFAULT_LEVEL: u32 = 1;

/// Default tab name handed out when the persistence layer has
/// no row for `tab_index = 0` — matches the on-creation "Tab 1"
/// label so existing characters with stashed items see a
/// sensible page header on their first post-migration login.
pub const DEFAULT_STASH_TAB_COLOR: u32 = 0x6E6E78;

/// Server-side stash page. One [`Item`] grid per tab plus a
/// player-chosen name + color. Sparse like the bag — `None`
/// is an empty slot the player carved out.
#[derive(Clone, Debug)]
pub struct StashTab {
    pub name: String,
    /// Packed `0xRRGGBB`.
    pub color: u32,
    pub items: Vec<Option<rift_game::loot::Item>>,
}

impl StashTab {
    /// Build a fresh empty tab with the default color and the
    /// "Tab N" auto-name.
    pub fn fresh(index: usize) -> Self {
        Self {
            name: format!("Tab {}", index + 1),
            color: DEFAULT_STASH_TAB_COLOR,
            items: Vec::new(),
        }
    }
}

/// Component bundle for a connected player.
#[derive(Clone, Debug)]
pub struct ServerPlayer {
    pub client_id: ClientId,
    pub net_id: NetId,
    pub k: Kinematic,
    pub hp_max: f32,
    pub hp: f32,
    /// Current essence pool (universal ability resource).
    /// Server-authoritative. Drained at cast time
    /// (`Ability::resource_cost`) and per-tick during channels
    /// (`Ability::channel_cost_per_sec`); regenerates at
    /// `stats.essence_regen` per second after a short
    /// `essence_regen_pause`.
    pub essence: f32,
    /// Seconds remaining before passive essence regen resumes.
    /// Set every time a cost is paid; ticks down each
    /// `Sim::step`. While > 0 the regen branch is skipped so
    /// the bar visibly hitches after a spend.
    pub essence_regen_pause: f32,
    /// Last input `seq` we successfully applied. Echoed back in
    /// snapshots so the client can prune its prediction buffer.
    pub last_input_seq: u32,
    /// In-memory inventory of items the player has picked up this
    /// session. Authoritative on the server. Persisted via the
    /// `inventory_items` table; rows live across sessions and
    /// floor transitions. The bag mirror only \u2014 anything
    /// currently equipped sits in [`ServerPlayer::equipment`].
    ///
    /// **Sparse**: an `inventory[i] == None` is an empty slot the
    /// player has carved out via drag-and-drop. The vector is
    /// trimmed of trailing `None`s after every mutation so its
    /// length tracks the highest occupied slot + 1.
    pub inventory: Vec<Option<rift_game::loot::Item>>,
    /// Currently-equipped items, keyed by
    /// [`rift_game::loot::EquipSlot`]. Authoritative on the
    /// server. Stat / damage formulas read from this set; the
    /// bag never contributes affixes.
    pub equipment: rift_game::loot::Equipment,
    /// Per-character private stash. Lives in its own DB tables
    /// (`stash_items` + `stash_tabs`) and is hydrated alongside
    /// `inventory` at Hello time. Items here are pure storage
    /// — they never contribute to stats and aren't visible from
    /// the bag UI unless the player explicitly opens the chest.
    /// Each tab is a separately-named, color-coded page;
    /// players start with one free tab and pay shards to
    /// unlock more (see `Sim::buy_stash_tab`).
    pub stash: Vec<StashTab>,
    /// Whether the owner currently has an active stash session
    /// (i.e. is interacting with the chest in the hub). Gates
    /// `DepositToStash` / `WithdrawFromStash` so out-of-band
    /// transfer requests are dropped server-side.
    pub stash_open: bool,
    /// Character level. Used by [`CharacterStats::compute`] for
    /// HP-per-level scaling.
    pub level: u32,
    /// Authoritative XP / level state. Source of truth for the
    /// `ServerMsg::CharacterStats` reply pushed to the owning
    /// client. `level` is mirrored in the dedicated `level`
    /// field above so `CharacterStats::compute` can stay
    /// pure-arg.
    pub experience: Experience,
    /// `Attributes::for_class` allocation until per-character
    /// attribute points are persisted.
    pub attrs: Attributes,
    /// Cached snapshot of the derived combat stats. Recomputed
    /// whenever inputs change ([`Self::recompute_stats`]); read
    /// by the cast / projectile / channel pipelines so client
    /// and server agree on the formulas.
    pub stats: CharacterStats,
    /// Cached aggregated ability mods (Amplify / Modify /
    /// Transform / Trigger) from the currently-equipped gear.
    /// Built once per equipment change and consulted at cast
    /// time so the dispatch hot path doesn't re-walk every
    /// equipped item per cast. Mirrors how `stats` is treated.
    pub ability_mods: rift_game::loot::AbilityMods,
    /// Per-variant internal cooldowns for legendary
    /// `TransformAbility` finishers (see
    /// [`super::transforms::TransformCds`]). Ticked from
    /// `Sim::step`; read+set by the transform dispatcher.
    pub transform_cds: super::transforms::TransformCds,
    /// Authoritative ability bar. Casts are gated against this so
    /// a client can't fire an ability they haven't slotted.
    /// Persisted via the `characters.loadout` column; mutated
    /// through `ClientMsg::SetLoadoutSlot`.
    pub loadout: Loadout,
    /// `true` once a dead player has finished their down-pose
    /// timer and entered ghost mode — they can move freely,
    /// trigger the rift exit-vote portal, but can't cast,
    /// loot, or be targeted by AI. Cleared on heal_all().
    pub is_ghost: bool,
    /// Countdown (seconds) from death until ghost rise. `None`
    /// while alive or already a ghost. Set to
    /// `GHOST_RISE_DELAY` the tick HP first crosses 0; ticked
    /// down in `Sim::step`. When it reaches 0 we flip
    /// `is_ghost = true` and clear the timer.
    pub ghost_rise_timer: Option<f32>,
    /// `Some(shrine_net_id)` while this player is holding the
    /// channel intent on a revive shrine. Set / cleared by
    /// `ClientMsg::ToggleShrineChannel`. Auto-cleared by
    /// `shrine::tick` when the player walks out of range,
    /// dies, or the shrine despawns.
    pub channeling_shrine: Option<NetId>,
    /// Persistent salvage currency ("shards"). Minted by
    /// salvaging items in the bag (yield scales with rarity
    /// and ilvl) and spent on stash expansion / future
    /// crafting. Mirrored to the owning client via
    /// `ServerMsg::ShardsSync`. Persisted on the
    /// `characters.shards` column; loaded at hello time
    /// alongside XP.
    pub shards: u32,
}

impl ServerPlayer {
    /// Build a fresh player record. Stats are computed from the
    /// hero config + default attributes + empty equipment, so a
    /// freshly-spawned character matches `CharacterStats::baseline`.
    pub fn fresh(client_id: ClientId, net_id: NetId, spawn: glam::Vec3) -> Self {
        let attrs = Attributes::for_class(HERO.primary_attribute);
        let equipment = rift_game::loot::Equipment::new();
        let stats = CharacterStats::compute(
            &attrs,
            DEFAULT_LEVEL,
            &equipment.active_affix_sum(),
            &rift_game::stats::StatModifiers::new(),
        );
        let ability_mods = equipment.ability_mods();
        let hp_max = stats.max_hp;
        let max_essence = stats.max_essence;
        Self {
            client_id,
            net_id,
            k: Kinematic {
                position: spawn,
                velocity: glam::Vec3::ZERO,
                yaw: 0.0,
                aim_yaw: 0.0,
                locomotion: loco::IDLE,
                vy: 0.0,
                airborne: false,
                ..Default::default()
            },
            hp_max,
            hp: hp_max,
            essence: max_essence,
            essence_regen_pause: 0.0,
            last_input_seq: 0,
            inventory: Vec::new(),
            equipment,
            stash: vec![StashTab::fresh(0)],
            stash_open: false,
            level: DEFAULT_LEVEL,
            attrs,
            stats,
            ability_mods,
            transform_cds: super::transforms::TransformCds::default(),
            experience: Experience::new(),
            loadout: Loadout::default_hero(),
            is_ghost: false,
            ghost_rise_timer: None,
            channeling_shrine: None,
            shards: 0,
        }
    }

    /// Recompute [`Self::stats`] from the current equipment /
    /// attributes / level. Rescales `hp` so the same percentage
    /// of max HP is preserved across an `hp_max` change (e.g.
    /// equipping a +Health item heals to the same fraction of
    /// the new pool).
    ///
    /// Call after any mutation that changes a `compute` input:
    /// equip / unequip, attribute respec (TBD), level up (TBD).
    pub fn recompute_stats(&mut self) {
        let new_stats = CharacterStats::compute(
            &self.attrs,
            self.level,
            &self.equipment.active_affix_sum(),
            &rift_game::stats::StatModifiers::new(),
        );
        let hp_pct = if self.hp_max > 0.0 {
            (self.hp / self.hp_max).clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.hp_max = new_stats.max_hp;
        self.hp = new_stats.max_hp * hp_pct;
        // Mirror the HP-rescale on essence so equipping a
        // `+Max Essence` item heals the pool to the same
        // fraction it was at before the resize — no surprise
        // "swap rings, lose 30 essence" moments.
        let essence_max_old = self.stats.max_essence.max(1.0);
        let essence_pct = (self.essence / essence_max_old).clamp(0.0, 1.0);
        self.essence = new_stats.max_essence * essence_pct;
        self.stats = new_stats;
        // Equipment changes also rotate the affix-driven
        // gameplay-changing mods (extra projectiles, transforms,
        // procs, per-ability damage). Recompute alongside stats
        // so equip / unequip is one consistent edge.
        self.ability_mods = self.equipment.ability_mods();
    }

    /// Per-cast damage scalar — `stats.damage / class.base_damage`.
    /// Multiplied into each ability's authored `base_damage` so a
    /// freshly-spawned (no gear, default attrs) character deals
    /// the authored numbers, and gear / attributes scale every
    /// ability uniformly.
    pub fn damage_scalar(&self) -> f32 {
        if HERO.base_damage <= 0.0 {
            1.0
        } else {
            self.stats.damage / HERO.base_damage
        }
    }

    /// Grant XP. If one or more level-ups happen, the cached
    /// `level` field is bumped, [`recompute_stats`] is called so
    /// the HP pool reflects the new tier, and the (possibly
    /// empty) reward list is returned for the caller to act on
    /// (granting attribute / talent points lives a layer up).
    ///
    /// Dead-or-ghosting players are short-circuited entirely:
    /// the heal-to-full on level-up would otherwise resurrect a
    /// player who took a killing blow and earned XP in the same
    /// tick (e.g. their ability finished a kill the same frame
    /// it killed them) — server flips `hp = 0` and arms
    /// `ghost_rise_timer`, but `is_ghost` doesn't latch until
    /// the rise delay elapses, so we can't gate purely on that
    /// flag. Treating any of {hp<=0, is_ghost, ghost timer
    /// armed} as "not earning XP this tick" closes the window.
    pub fn grant_xp(&mut self, amount: u64) -> Vec<LevelUpReward> {
        if self.is_dead_or_ghosting() {
            return Vec::new();
        }
        let rewards = self.experience.grant_xp(amount);
        if !rewards.is_empty() {
            self.level = self.experience.level;
            // Heal-to-full feel on level up: keep current % then
            // top off the gained HP. We just call recompute and
            // then refill so a fresh-level character isn't stuck
            // at the pre-level-up HP fraction.
            self.recompute_stats();
            self.hp = self.hp_max;
        }
        rewards
    }

    /// `true` if this player is in any "not actively playing"
    /// state — dead (hp≤0), risen ghost, or in the down-pose
    /// waiting to rise. Used to gate XP / heal / vote-init paths
    /// that would otherwise have inconsistent behaviour during
    /// the death→ghost transition window.
    pub fn is_dead_or_ghosting(&self) -> bool {
        self.hp <= 0.0 || self.is_ghost || self.ghost_rise_timer.is_some()
    }

    /// Universal essence-cost gate. Returns `true` and deducts
    /// `cost` if the player can afford it; returns `false`
    /// otherwise (caller should reject the cast). A successful
    /// spend also pauses passive regen for [`ESSENCE_SPEND_PAUSE`]
    /// seconds so the bar visibly hitches after a cast.
    /// Treats `cost <= 0.0` as a free, no-op success so the
    /// existing free-cast abilities (Melee, Evasive Roll,
    /// triggers) keep working.
    pub fn try_spend_essence(&mut self, cost: f32) -> bool {
        if cost <= 0.0 {
            return true;
        }
        if self.essence + 1e-3 < cost {
            return false;
        }
        self.essence -= cost;
        self.essence_regen_pause = ESSENCE_SPEND_PAUSE;
        true
    }

    /// Drain `cost` essence without an affordability check.
    /// Used by the channel tick to bleed essence each frame:
    /// if the pool empties mid-channel the channel itself is
    /// ended cleanly by [`super::channel::tick`], so we just
    /// clamp at zero here.
    pub fn drain_essence(&mut self, amount: f32) {
        if amount <= 0.0 {
            return;
        }
        self.essence = (self.essence - amount).max(0.0);
        self.essence_regen_pause = ESSENCE_SPEND_PAUSE;
    }

    /// Per-tick essence regen. Counts down the post-spend pause
    /// first; once it elapses, restores `stats.essence_regen` per
    /// second up to `stats.max_essence`. Dead / ghost players
    /// don't regen so a downed player can't sneak a cast off the
    /// instant they rise.
    pub fn tick_essence(&mut self, dt: f32) {
        if self.is_dead_or_ghosting() {
            return;
        }
        if self.essence_regen_pause > 0.0 {
            self.essence_regen_pause = (self.essence_regen_pause - dt).max(0.0);
            return;
        }
        if self.essence < self.stats.max_essence {
            self.essence = (self.essence + self.stats.essence_regen * dt)
                .min(self.stats.max_essence);
        }
    }
}

/// Seconds passive essence regen pauses after every spend (cast
/// upfront cost or channel per-tick drain). Matches the feel of
/// classic ARPG resource bars: spam-cast and the bar visibly
/// hitches; let go and it ramps back smoothly.
pub const ESSENCE_SPEND_PAUSE: f32 = 0.6;

/// Edge-triggered button bits we forward across input coalescing so a
/// brief press never gets dropped between server ticks.
const STICKY_BUTTONS: u16 = button_bits::JUMP
    | button_bits::ROLL
    | button_bits::INTERACT
    | button_bits::ATTACK
    | button_bits::ABILITY_1
    | button_bits::ABILITY_2
    | button_bits::ABILITY_3
    | button_bits::ABILITY_4
    | button_bits::ABILITY_5
    | button_bits::ABILITY_6;

/// Merge a fresh input into a possibly-already-pending one for the
/// same client. Drops out-of-order packets and OR-folds sticky
/// buttons forward.
pub fn merge_pending(pending: &mut HashMap<ClientId, InputCmd>, client_id: ClientId, cmd: InputCmd) {
    if let Some(existing) = pending.get(&client_id) {
        if cmd.seq.wrapping_sub(existing.seq) as i32 <= 0 {
            return;
        }
    }
    let mut merged = cmd;
    if let Some(existing) = pending.get(&client_id) {
        merged.buttons |= existing.buttons & STICKY_BUTTONS;
    }
    pending.insert(client_id, merged);
}

/// Apply the latest pending input for each connected player. Drains
/// `pending`. Records the applied `seq` on each `ServerPlayer` so
/// the next snapshot's `ack_seq` is correct.
pub fn apply_inputs(
    world: &mut hecs::World,
    sessions: &HashMap<ClientId, Entity>,
    pending: &mut HashMap<ClientId, InputCmd>,
) {
    let inputs: Vec<(ClientId, InputCmd)> = pending.drain().collect();
    for (client_id, cmd) in inputs {
        if let Some(&entity) = sessions.get(&client_id) {
            if let Ok(mut p) = world.get::<&mut ServerPlayer>(entity) {
                p.last_input_seq = cmd.seq;
                // Dead-but-not-yet-risen players are pinned in
                // the down pose: zero velocity, drop input. Once
                // they've risen as a ghost they regain movement
                // (but `cast_ability` still rejects them, so the
                // attack/ability button bits below are harmless).
                if p.hp <= 0.0 && !p.is_ghost {
                    p.k.velocity = glam::Vec3::ZERO;
                    continue;
                }
                kinematic::apply_input(&mut p.k, cmd.move_dir, cmd.aim_dir, cmd.buttons);
            }
        }
    }
}

/// Integrate every player's velocity against the floor's wall grid.
pub fn integrate_motion(world: &mut hecs::World, floor: &Floor, dt: f32) {
    for (_e, p) in world.query_mut::<&mut ServerPlayer>() {
        kinematic::integrate(&mut p.k, floor, dt);
    }
}

/// Snapshot every *living* player's `(entity, position)` into a
/// Vec, suitable for use as the AI target list during the enemy
/// tick. Ghosts (`hp <= 0.0` while in a rift floor) are filtered
/// out so AI / enemy projectiles don't aim at corpses.
pub fn target_positions(world: &hecs::World) -> Vec<(Entity, glam::Vec3)> {
    world
        .query::<&ServerPlayer>()
        .iter()
        .filter(|(_, p)| p.hp > 0.0)
        .map(|(e, p)| (e, p.k.position))
        .collect()
}

/// Reset every player's kinematic state to a fresh spawn pose.
/// Called from the floor-change path so a held key doesn't slide
/// the freshly-loaded floor's start position.
pub fn snap_all_to(world: &mut hecs::World, spawn: glam::Vec3) {
    for (_e, p) in world.query_mut::<&mut ServerPlayer>() {
        p.k.position = spawn;
        p.k.velocity = glam::Vec3::ZERO;
        p.k.vy = 0.0;
        p.k.airborne = false;
        p.k.locomotion = loco::IDLE;
    }
}

/// Restore every player to full HP and clear any ghost state.
/// Called from the hub-return path (manual exit-vote, party
/// wipe respawn, login) where the team is back in the safe
/// zone and should arrive alive.
pub fn heal_all(world: &mut hecs::World) {
    for (_e, p) in world.query_mut::<&mut ServerPlayer>() {
        p.hp = p.hp_max;
        p.is_ghost = false;
        p.ghost_rise_timer = None;
    }
}

/// Restore living players to full HP but leave ghosts as
/// ghosts. Called when the team advances deeper into the rift
/// (boss-kill auto-advance): living players carry on full, but
/// dead teammates follow along still in spectator mode rather
/// than getting a free resurrection. Players in the down-pose
/// (post-death, pre-rise) keep their armed `ghost_rise_timer`
/// so the rise still triggers on the new floor.
pub fn heal_living(world: &mut hecs::World) {
    for (_e, p) in world.query_mut::<&mut ServerPlayer>() {
        if p.is_dead_or_ghosting() {
            continue;
        }
        p.hp = p.hp_max;
    }
}
