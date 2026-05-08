# Rift — Refactor & Scalability TODO

Living document. Add / cross off as we go. Keep entries short — the
goal is shared awareness, not a spec.

---

## In progress

### Combat-meter: `damage taken` per-ability breakdown

The DMG / HPS tabs in `MeterUi` (bottom-right HUD) already drive a
click-to-expand per-ability rollup, fed by the
`MeterAbilityBreakdown` rows the server attaches to every
`MeterEntry`. The `TAKEN` tab can't do the same yet because enemy
damage doesn't go through the standard ability pipeline — every
hit currently fans out as a bare `(Entity, f32)` tuple in
`AiOutcome::melee_damage`, and Stalker dashes / Brute & Boss melee
swings don't carry their `Spec::ability_id`. The `lookup` registry
in `rift-game::abilities` already has wire ids for the boss
attacks (`ARCANE_BOLT`, `ARCANE_FAN`, `GROUND_SLAM`,
`SUMMON_BRUTES`), but the regular brute / stalker melee swings
have no ability entry at all.

**Plan when we pick this up:**

- [ ] Author registry entries for every enemy attack (brute melee,
      stalker dash, caster auto, …) in `rift-game::abilities` so
      every hit can be tagged with a wire-stable `ability_id`.
- [ ] Replace `Vec<(Entity, f32)>` damage queues with a struct that
      carries `ability_id: u8` (mirror of `MeterEvent::DamageDealt`).
      Touches `AiOutcome::melee_damage`,
      `projectile::tick(...)`, `projectile::tick_aoe(...)`,
      `channel::tick(...)`, and the boss / brute / stalker tick
      sites.
- [ ] Pipe through `apply_player_damage` and credit
      `meters.entry(client_id).by_ability` so the breakdown is
      symmetric with DMG / HPS.
- [ ] Drop the "show ability rows" gate on the TAKEN tab and let the
      same expand UI render them.

---

## Item layering — strategy

Goal: one source of truth per concern, with the gameplay/data side
in `rift-game` and only the rendering glue in `rift-engine` /
`rift-client`.

| Layer                                                                                         | Crate         | Responsibility                                                 |
| --------------------------------------------------------------------------------------------- | ------------- | -------------------------------------------------------------- |
| Item definitions, base items, affixes, rarity, drop tables, RNG, inventory math               | `rift-game`   | Pure data + rules. No Vulkan, no hecs.                         |
| Wire item blob (`ItemBlob`, base-id ↔ index map)                                              | `rift-net`    | Stable wire format. Re-exports nothing.                        |
| Item _entity_ component bundle on the world (e.g. `Loot { item: ItemSlot }` for ground drops) | `rift-engine` | Just the ECS plumbing, no item fields baked into engine types. |
| Pickup visuals (pillar mesh, glow emitter, F-prompt tooltip)                                  | `rift-client` | Pure presentation. Reads `ItemSlot` via `rift-game` API.       |
| Inventory UI / equipment UI                                                                   | `rift-client` | Reads inventory/equipment from `GameState`.                    |

**Status:** `rift-engine` no longer owns any item / inventory /
equipment types. The only loot-shaped things it still defines are
pure presentation primitives that take a `[f32;3]` color
parameter — `Mesh::loot_orb`, `EmitterConfig::loot_beam` /
`loot_beam_base`. Those are correctly placed.

`Rarity::color()` lives in `rift-game::loot::rarity` and is fine
there: `Rarity` is canonical gameplay data and the ARPG colour
ramp (white / blue / yellow / orange) is part of the game's
identity, not just a UI choice. The client reads it; nothing
upstream of `rift-game` needs to know about colours.

**Remaining actions:**

- [x] `BaseItem::icon: &'static str` added and populated for every
      entry in `BASE_ITEMS`. Armor / accessory bases point at
      `loot/<Slot>/<Name>_N`; weapon bases use `""` until weapon
      icons land. `MpInventoryUI` calls `batch.icon` first, falls
      back to the first-letter glyph when unknown.
- [x] Overlay icon discovery walks `assets/icons/` recursively.
      Subdirectories scope into the registry key
      (`loot/Boots/Boots_1`), so flat ability icons (`Hunter_3`)
      and slot-scoped item icons can't collide.
- [x] Naming scheme: relative path stem with forward slashes,
      driven by the directory layout under `assets/icons/`. New
      icon? Drop the file in the right folder; the engine picks
      it up next launch.
- [ ] Source weapon icons (`assets/icons/loot/Weapons/*` or
      similar) and fill in the four weapon `BaseItem::icon`
      fields. Until then weapons use the placeholder glyph.

---

## Character stats — uniform model

✅ **Initial pass done.** `CharacterStats` lives in
`crates/rift-game/src/stats.rs` and is computed via a pure
`CharacterStats::compute(class, attrs, level, equipment)` fn.
`PlayerState::stats()` is the single client-side accessor;
`render_hud` reads `stats.max_hp` instead of the dead
`max_hp_bonus: f32` parameter; legacy `AttributeScaling` is
`#[deprecated]` and unused.

**Remaining:**

- [x] Buffs / debuffs / talent passives feed `compute` via the
      new `StatModifiers { flat, percent }` aggregate.
      `TalentTree::stat_modifiers()` materialises invested ranks
      into the same shape; the client `recompute_stats` already
      threads them through. Buff sources can `extend()` more
      modifiers in once a buff system lands.
- [x] Cache the snapshot on `PlayerState`. `cached_stats` is
      populated in `with_profile` and refreshed via
      `recompute_stats` whenever `EquipmentSync` lands; the HUD
      and spawn paths read it through `stats() -> &CharacterStats`
      / `max_hp()` instead of recomputing every frame.
- [x] Server-side: `ServerPlayer` carries a `CharacterStats`
      snapshot recomputed on equip / unequip / inventory load.
      `ability::cast` scales each ability's base damage by
      `stats.damage / class.base_damage` and stamps
      `crit_chance` / `crit_damage` onto every projectile / AoE /
      channel; per-hit crit rolls in `apply_hits_to_enemies` use
      a deterministic `crit_seed` mixed from `(tick, target,
caster)` so server and any future spectator agree.

---

## High-impact architectural debt

### 1. `GameState` is a bag of fields — ✅ extracted

The 16 multiplayer / channel / loot / loading fields previously
inline on `GameState` are now four sub-structs:

- `NetState` — `floor_seed`, `transition`, `casts`, `profile`,
  `account_name`, `roster_request`.
- `ChannelState` — `active`, `visuals`, `pending_ends`.
- `LootClientState` — `drops`, `pending_pickups`, `claimed_ids`,
  `items`.
- `LoadingState` — `phase`, `monster_index`.

Access goes through `state.net.casts`, `state.loot.drops`, etc.
`GameState` now reads as an orchestrator (world, rift, player,
sub-systems, a few flags).

**Remaining cleanup:**

- [ ] Move `app_state`, `character_select`, `anim_cache` into a
      future `AppShellState` once the entering-world phase machine
      stabilises.
- [ ] Sub-struct types live in `state.rs`; consider promoting them
      to a `sub_state.rs` module if the file keeps growing.

### 2. Server `main.rs` mixes dispatch + simulation — ✅ done

`handle_client_msg` is now a flat dispatch table; per-message
bodies live in their own `handle_*` methods (`handle_hello`,
`handle_pick_up_loot`, etc.). Adding a new message variant is
one method + one match arm.

### 3. `Vec::remove(0)` in icon streaming — ✅ done

Replaced front-pop with a `next_icon_idx: usize` cursor in
`OverlayRenderer::step_load_icons`.

### 4. String-keyed hot-path lookups — ✅ partial

`AnimationSet::get` no longer allocates a lowercased `String` on
every lookup; it does a case-insensitive linear scan over the
(small, < 30 entry) clip table. That kills the per-frame heap
traffic from `find_any` calls in `locomotion_anim_system` and the
spell-phase clip selector.

**Remaining:**

- [ ] If profiling later shows the linear scan itself is hot,
      cache the resolved `Arc<BoundClip>` for each well-known
      candidate set on a per-entity `LocomotionClips` component
      so per-frame work becomes a struct field read.

### 5. Two-letter ability abbreviations duplicated — ✅ done

The hand-curated `match state.ability.name { ... }` is gone;
`ability_abbrev(name: &str)` derives initials at draw time
(skipping connector words like "of" / "for"). New abilities work
out of the box.

---

## Medium-impact

### 6. Snapshot scaling / interest management — ✅ verified

`rift-server::sim::snapshot::build` already applies a per-viewer
distance filter (`VIEW_RANGE_SQ = 35²`) to enemies, projectiles,
and loot. Players are intentionally always-replicated so the
roster stays correct. Floor-level scoping is implicit: each
client's `Sim` only carries the entities for its current floor.

**Optional follow-ups (only if a profile points here):**

- [ ] Bucket entities into a coarse spatial grid so the per-tick
      filter is sub-linear in entity count instead of O(n) per
      viewer.
- [ ] Tune `VIEW_RANGE_SQ` per entity kind (loot can be tighter,
      bosses wider) once we have data on what feels right.

### 7. Persistence sweep — ✅ verified

All three `*_blocking` calls in `rift-server/src/main.rs` live on
the login / character-select path (`handle_hello`'s
`load_or_create_blocking` + `load_inventory_blocking`,
`lookup_roster`'s `list_account_characters_blocking`). None run
on the gameplay tick path.

### 8. Wire-id ↔ definition split for items — ✅ done

`BaseItem` is the single static definition (`BASE_ITEMS` const
slice); the wire encodes by index via `Item::to_wire` /
`from_wire`, and persistence uses the `id: &'static str` for
rebuild stability. Mirrors the ability pattern.

### 9. Per-frame clones in net_client — ✅ partial

`InterpSample` is `Copy`; the three `.clone()` calls in the
snapshot interp-buffer hot path are now no-ops. Remaining
`String::clone` calls in `net_client.rs` happen once per
handshake / per Hello, not per frame.

---

## Low-impact / hygiene

### 10. `rift-engine` is large

Renderer + ECS components + AI nav + physics + audio + (until just
now) loot. Compile-time win possible by splitting `ai`, `physics`,
or `audio` into their own crates.

- [ ] Defer until compile times become an issue.

### 11. `unused_variable: panel_w` warnings — ✅ done

Prefixed with `_` in `character_select.rs`. rift-client builds
warning-free.

---

## Done (most recent first)

- `StatModifiers` aggregate added to `CharacterStats::compute`:
  flat / percent channels, talents materialise into it via
  `TalentTree::stat_modifiers()`, future buff systems just
  `extend()` more modifiers in. Server passes
  `StatModifiers::new()` for now.
- `PlayerState.cached_stats`: HUD no longer recomputes the
  affix sum + multiplier math every frame. Refreshed only when
  `EquipmentSync` arrives.
- Floating combat text wired to `WorldEvent::Damage`: the client
  consumes the reliable damage stream and routes through
  `CombatTextSystem` — red drift-down for hits on the local
  player, gold "N!" for crits, orange for big hits, white
  otherwise. `crit` flag is server-authoritative so the colour
  can't lie.
- Server-authoritative damage / crit: `ServerPlayer.stats` is
  computed from `CharacterStats::compute` and used by
  `ability::cast`; per-hit crit rolls live in
  `apply_hits_to_enemies` with a deterministic seed.
- VFX rewrite: declarative `Effect = Vec<Layer>` system with
  `Particles { spawn, emission, forces, color, sprite, blend }`
  and `Ribbon { width, gradients, noise }`. Frost Ray rebuilt as
  a ribbon + hand-base swirl; legacy `ParticleSystem` /
  `EmitterConfig` deleted entirely.
- Multiplayer equipment system: `MpEquipment` (one `Option<Item>`
  per `EquipSlot`), `EquipItem` / `UnequipItem` wire messages,
  `EquipmentSync` snapshot, drag-from-inventory UI, and
  `active_affix_sum() -> StatBlock` plumbed into
  `PlayerState::stats()`. Server is authoritative; clients mirror
  via the dual `InventorySync` + `EquipmentSync` reply pattern.
- Legacy single-player `EquipmentUI` / `InventoryUI` (in
  `rift-engine/src/ui/`) deleted; `MpInventoryUI` is the only UI
  path. `rift-engine/src/ui/` now hosts only `combat_text`.
- Parallel icon decode: `step_load_icons` runs PNG decompress +
  Catmull-Rom resize across cores via rayon; per-step budget
  bumped to 128. ~330-icon load drops from ~10 frames to 2-3.
- Smooth body-yaw follow: stationary players slew their body
  yaw toward aim yaw with an exponential pull (τ ≈ 170 ms) so
  the spine-twist clamp never has to hard-snap when the cursor
  sweeps past the back of the character. Tick-rate independent;
  server and client converge for the same total `dt`.
- AOI verified: `snapshot::build` already filters enemies /
  projectiles / loot per viewer at 35 m. Documented and crossed
  off (was a TODO false positive).
- Equipment-slot system added as a concrete blocker for the
  remaining `CharacterStats` work. `BaseItem::equip_slot` exists;
  what's missing is the multiplayer `MpEquipment` / wire
  messages / UI.
- `AnimationSet::get` allocation eliminated: case-insensitive
  linear scan over the small clip map, no per-frame `to_ascii_lowercase`
  string allocs.
- `InterpSample` made `Copy`; three per-snapshot `.clone()` calls
  in `net_client` are now no-ops.
- Persistence sweep verified — all `*_blocking` calls live on
  login / roster paths, never on the gameplay tick.
- Item wire-id split confirmed: `BASE_ITEMS` indexed for the
  wire, `id: &'static str` for persistence; mirrors abilities.
- Server message dispatch flattened: `handle_client_msg` is now a
  table of one-line arms calling `handle_hello`,
  `handle_pick_up_loot`, etc. Hello's ~140-line body lives in
  its own method.
- HUD ability abbreviation derived from name initials
  (`ability_abbrev("Mark for Death")` → `"MD"`); hand-curated
  match table removed.
- `panel_w` unused-variable warnings cleared in `character_select.rs`.
- Item icons: `BaseItem::icon` field + recursive icon discovery.
  `assets/icons/loot/<Slot>/*.png` is now picked up; registry
  keys use forward-slashed relative stems
  (`loot/Boots/Boots_1`) so flat ability icons can't collide
  with slot-scoped item icons. `MpInventoryUI` draws the icon
  with a glyph fallback.
- Extracted four sub-structs from `GameState` (`NetState`,
  `ChannelState`, `LootClientState`, `LoadingState`); 16
  scattered fields collapse to four grouped slots, all access
  routed through `state.net.*` / `state.loot.*` /
  `state.channel.*` / `state.loading.*`.
- Unified stat vocabulary: `Stat`, `StatBlock`, and the new
  `CharacterStats` snapshot all live in `rift-game/src/stats.rs`.
  `loot::stats` is gone; `crate::stats::*` is the single import
  path. `AttributeScaling` removed entirely.
- Unified `CharacterStats` snapshot (`rift-game/src/stats.rs`):
  pure `compute(class, attrs, level, equipment)` consolidates the
  formulas previously scattered across `AttributeScaling`,
  `PlayerState::max_hp`, and the dead `max_hp_bonus` HUD param.
- Removed gameplay item types from `rift-engine`; only colour-
  parameterised presentation primitives (`Mesh::loot_orb`,
  `EmitterConfig::loot_beam*`) remain. Item layering boundary
  now matches the table above.
- Replaced `Vec::remove(0)` cursor in `step_load_icons` with a
  forward-index cursor (no more O(n) per icon).
- Centralised ability icon names on `Ability::icon`; engine now
  auto-discovers `assets/icons/*.png` instead of a hardcoded list.
- Streaming icon load with progress, single-submit batched upload
  per `step_load_icons` call.
- Action bar / HP / XP bar sizing pass.
