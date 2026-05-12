# Items, Affixes & Legendary Effects — Design Reference

> Living document. Section 1 captures what is **actually implemented today**
> (read directly from the codebase). Section 2 is intentionally short — it's
> the placeholder we'll fill in together once the as-built picture is agreed.
> Section 3 is the running gap list.

---

## 1. Where we are today

> **Status note (May 2026).** Section 1 has been rewritten to reflect
> the post-Phase-5 codebase. Phases 1–5 of the migration plan in §3
> have landed; Phase 6 (named roll bands & tooltip polish) and Phase 7
> (the long tail) are in progress. The Attribute × Element duo,
> family-locked bases, hand-authored uniques, the Resonance
> cross-family bonus slot, and the Rift-touched line are all live.
>
> **Archetype axis retired (May 2026).** The original design carried a
> third **Archetype** axis (`Projectile | Melee`) with matching
> `ProjectileDamage` / `MeleeDamage` / `BeamDamage` / `AoeDamage`
> stats. In practice those stats overlapped with the Element axis
> (every weapon already commits to an element family that scales the
> same abilities) and added a third rung that didn't carry independent
> information. The four `*Damage` stat variants, the `Archetype` enum,
> the `archetype` field on `BaseFamily`, and the `pct_*` / `res_*`
> archetype affix lines have all been removed. The shipped trio is now
> a **duo** — Attribute × Element — and the dagger / wand implicits
> that used to push `MeleeDamage` / `ProjectileDamage` now push
> `Strength` / `Agility` instead. §2 "where we want to be" still uses
> the trio wording where it documents the original intent; the active
> spec in §1 is the source of truth.

### 1.1 Crate layout

All loot rules live in the `rift-game` crate (engine-agnostic, no I/O), and
are consumed by the server (authoritative rolls + combat application) and
the client (tooltip rendering only). Persistence converts to/from stable
string ids.

| Concern                              | Where                                                                                  |
| ------------------------------------ | -------------------------------------------------------------------------------------- |
| Stat vocabulary                      | [crates/rift-game/src/stats.rs](crates/rift-game/src/stats.rs)                         |
| Family vocabulary (Attr/Elem/Arch)   | [crates/rift-game/src/loot/families.rs](crates/rift-game/src/loot/families.rs)         |
| Module index                         | [crates/rift-game/src/loot/mod.rs](crates/rift-game/src/loot/mod.rs)                   |
| Base items + slots + `BaseFamily`    | [crates/rift-game/src/loot/items.rs](crates/rift-game/src/loot/items.rs)               |
| Affix defs + the four pools          | [crates/rift-game/src/loot/affixes.rs](crates/rift-game/src/loot/affixes.rs)           |
| Hand-authored unique catalogue       | [crates/rift-game/src/loot/uniques.rs](crates/rift-game/src/loot/uniques.rs)           |
| Rarity tier rules                    | [crates/rift-game/src/loot/rarity.rs](crates/rift-game/src/loot/rarity.rs)             |
| Rolled item + roll pipeline          | [crates/rift-game/src/loot/item.rs](crates/rift-game/src/loot/item.rs)                 |
| Per-monster drop tables              | [crates/rift-game/src/loot/drops.rs](crates/rift-game/src/loot/drops.rs)               |
| Tooltip rendering                    | [crates/rift-game/src/loot/tooltip.rs](crates/rift-game/src/loot/tooltip.rs)           |
| Equipment slots → stat/mod aggregate | [crates/rift-game/src/loot/equipment.rs](crates/rift-game/src/loot/equipment.rs)       |
| Aggregated ability mods              | [crates/rift-game/src/loot/ability_mods.rs](crates/rift-game/src/loot/ability_mods.rs) |
| Wire codec                           | [crates/rift-game/src/loot/wire.rs](crates/rift-game/src/loot/wire.rs)                 |
| Seeded RNG                           | [crates/rift-game/src/loot/rng.rs](crates/rift-game/src/loot/rng.rs)                   |
| Server roll-on-kill                  | [crates/rift-server/src/sim/loot.rs](crates/rift-server/src/sim/loot.rs)               |
| Server transform consumer            | [crates/rift-server/src/sim/transforms.rs](crates/rift-server/src/sim/transforms.rs)   |
| Server proc consumer                 | [crates/rift-server/src/sim/procs.rs](crates/rift-server/src/sim/procs.rs)             |
| Persistence (Postgres JSONB)         | [crates/rift-persistence/src/lib.rs](crates/rift-persistence/src/lib.rs)               |
| Wire shape                           | [crates/rift-net/src/messages.rs](crates/rift-net/src/messages.rs)                     |

### 1.2 Equip slots

Ten physical slots on the body, declared in
[loot/items.rs](crates/rift-game/src/loot/items.rs):

`Weapon, Helm, Chest, Legs, Hands, Boots, Ring1, Ring2, Amulet, Shoulders`

The discriminant (declaration order) is the wire byte and the
`equipped_slot` SMALLINT in the DB — appending only.
Rings are interchangeable in `Ring1`/`Ring2`; everything else strict.

### 1.3 Base items (`BASE_ITEMS`)

A `BaseItem` row carries: stable `id`, display `name`, `ItemSlot` taxonomy
(`Weapon(WeaponKind) | Armor(ArmorKind) | Accessory(AccessoryKind)`),
target `equip_slot`, an authoritative `family: BaseFamily` (the
**Attribute / Element** lock — see §1.5), legacy
`allowed_tags` / `favored_tags` bitmasks (preserved during the
migration but no longer drive rolling), `implicit: &[(Stat, f32)]`,
`min_ilvl`, an `icon` registry key, and an optional `GenderedModel`
for paperdoll art.

Currently authored bases (**14**):

| id              | Name              | EquipSlot | Family (Attr / Elements)           | Implicit                       |
| --------------- | ----------------- | --------- | ---------------------------------- | ------------------------------ |
| staff_basic     | Apprentice Staff  | Weapon    | Intellect / [Fire, Ice, Lightning] | +8 % Fire Damage               |
| sword_basic     | Iron Sword        | Weapon    | Strength / [Physical]              | +10 % Physical Damage          |
| dagger_basic    | Hunter's Dagger   | Weapon    | Strength / [Physical]              | +3 Strength, +5 % Crit Chance  |
| wand_basic      | Carved Wand       | Weapon    | Agility / [Fire, Ice, Lightning]   | +3 Agility, +4 % CDR           |
| light_helm      | Leather Helm      | Helm      | wildcard / [Physical]              | +12 Armor, +15 Health          |
| light_shoulders | Leather Spaulders | Shoulders | wildcard (all axes)                | +2 % Evasion, +12 Health       |
| heavy_chest     | Plated Cuirass    | Chest     | Strength / [Physical]              | +24 Armor, +30 Health          |
| light_chest     | Studded Vest      | Chest     | wildcard (all axes)                | +5 % Evasion, +18 Health       |
| robe_chest      | Mage Robe         | Chest     | Intellect / any element            | +14 Health, +8 % Essence Regen |
| light_boots     | Leather Boots     | Boots     | wildcard (all axes)                | +5 % Move Speed, +3 % Evasion  |
| light_gloves    | Leather Gloves    | Hands     | Intellect / any element            | +3 % CDR                       |
| light_legs      | Leather Leggings  | Legs      | Strength / [Physical]              | +16 Armor, +20 Health          |
| ring_basic      | Plain Ring        | Ring1     | wildcard (all axes)                | (none)                         |
| amulet_basic    | Plain Amulet      | Amulet    | wildcard (all axes)                | +10 Health                     |

`BaseFamily` is declared in
[loot/families.rs](crates/rift-game/src/loot/families.rs):

```rust
struct BaseFamily {
    attribute: Option<Attribute>,         // None = wildcard
    element:   Option<&'static [Element]>,
}
impl BaseFamily { const WILDCARD: Self = Self { attribute: None, element: None }; }
enum Attribute { Strength, Agility, Intellect }
enum Element   { Physical, Fire, Ice, Lightning }
```

`BaseFamily::accepts_*` gate each trio-axis affix at roll time. An
affix on an axis the base **rejects** can still appear — but only via
the dedicated **Resonance** slot (§1.13), which exists explicitly to
break the family lock once per item.

The `allowed_tags` / `favored_tags` bitmasks remain on every row for
backwards compatibility (legacy items rehydrating from persistence
still resolve through the same struct), but the live roll pipeline
reads only `family`. Tags will be removed in Phase 7 once a DB
migration sweep confirms no persisted item depends on them.

### 1.4 Stats

`Stat` (enum in [stats.rs](crates/rift-game/src/stats.rs)) is the single
vocabulary used by affixes, implicits, tooltips, and the resolved
`CharacterStats` sheet:

- **Attributes** (the second axis — flat point totals, mirrored by
  the manual point-spend screen): `Strength`, `Agility`, `Intellect`.
- **Offensive:** `CritChance`, `CritDamage`, `AttackSpeed`
- **Damage buckets** (multiply abilities of the matching element):
  `PhysicalDamage`, `FireDamage`, `IceDamage`, `LightningDamage`. There
  is no separate `WeaponDamage` / `SpellDamage` bucket — weapon
  implicits push the Element axis directly. There is no separate
  archetype axis either: the original `ProjectileDamage` /
  `MeleeDamage` / `BeamDamage` / `AoeDamage` stats were retired in May
  2026 because they didn't carry information independent of the
  element family.
- **Defensive:** `Health`, `Armor`, `Evasion`, `HealthRegen`,
  `ElementalResist`, `HealingReceived`. `Armor` is the sole
  mitigation channel for physical damage (soft-capped flat
  reduction); `ElementalResist` covers Fire / Ice / Lightning.
- **Utility:** `MaxResource`, `CooldownReduction`, `ResourceRegen`,
  `MoveSpeed`, `Range`

`Stat::is_percent()` decides display + math; percent stats are always
0..1 internally (`+0.05` = `+5 %`).

### 1.5 Rarity

```rust
enum Rarity { Common = 0, Magic = 1, Rare = 2, Legendary = 3 }
```

| Rarity    | Bonus affix range | Duo shape (Attr × Element)      | Color (sRGB)       | Salvage base | What it unlocks                          |
| --------- | ----------------- | ------------------------------- | ------------------ | ------------ | ---------------------------------------- |
| Common    | 1 – 2             | Attribute only                  | (0.85, 0.85, 0.85) | 1            | Pure stats; occasionally a Perfect roll  |
| Magic     | 2 – 3             | Attribute + Element             | (0.40, 0.65, 1.00) | 3            | First synergy line                       |
| Rare      | 3 – 4             | Full duo                        | (1.00, 0.85, 0.30) | 8            | Ability **amplifiers** (dmg / CDR)       |
| Legendary | 3 – 4             | Full duo + hand-authored unique | (1.00, 0.45, 0.10) | 25           | Named unique effect (Transform/Proc/...) |

Salvage scales with ilvl: `base * (1 + ilvl/20)`.

Design intent (verbatim from
[rarity.rs](crates/rift-game/src/loot/rarity.rs)): rarity unlocks
**patterns**, not just bigger numbers.

### 1.6 Affix anatomy

```rust
struct AffixDef {
    id: &'static str,            // stable, used by save/load + wire
    name_template: &'static str, // "{}" replaced with formatted value
    effect: AffixEffect,
    roll: (f32, f32),            // range at ilvl = 1
    ilvl_scale: f32,             // linear growth per ilvl above 1
    tags: u32,                   // legacy bitmask (not read by roll pipeline)
    min_ilvl: u32,               // doesn't drop below this ilvl
    rarity_min: Rarity,          // and not below this tier
    weight: u32,                 // base selection weight
}
```

`AffixEffect` is the taxonomy of how an affix **interacts with the
combat layer** — see
[affixes.rs](crates/rift-game/src/loot/affixes.rs):

| Variant                                       | Pattern   | Carries                         | Where applied                                                                               |
| --------------------------------------------- | --------- | ------------------------------- | ------------------------------------------------------------------------------------------- |
| `Stat(Stat)`                                  | Stat      | flat or % value                 | `Equipment::active_affix_sum` → `CharacterStats`                                            |
| `AmplifyAbilityDamage(AbilityId)`             | Amplify   | `+value` damage mult            | `AbilityMods::damage_for(id)` (multiplicative stack)                                        |
| `ReduceAbilityCooldown(AbilityId)`            | Amplify   | `-value` cooldown               | `AbilityMods::cooldown_for(id)` (multiplicative, floored at 0.20)                           |
| `ExtraProjectiles(AbilityId)`                 | Modify    | integer adds                    | `AbilityMods::extra_projectiles_for(id)`                                                    |
| `TransformAbility(AbilityId, AbilityVariant)` | Transform | discrete variant token          | `AbilityMods::transform_for(id)` → server hooks in `sim/transforms.rs` and `sim/channel.rs` |
| `Proc(ProcEvent, ProcAction)`                 | Trigger   | proc chance is the rolled value | `AbilityMods::procs` → `sim/procs.rs::dispatch`                                             |

`AffixCategory` (in `affixes.rs`) classifies each def into one of
`Attribute | Element | Resonance | RiftTouched | Bonus`.
`category()` reads it from the effect + pool membership; the roll
pipeline (§1.8) walks one phase per category.

#### `AbilityVariant` (transforms — fully wired)

| Variant           | Concept                                    | Wired in server?                                                              |
| ----------------- | ------------------------------------------ | ----------------------------------------------------------------------------- |
| `FireballToBeam`  | Fireball morphs into a piercing beam       | **Yes** — `sim/ability.rs` Projectile arm → synthetic `FIREBALL_BEAM` channel |
| `FrostRayShatter` | Frost Ray detonates into shards on release | **Yes** — `sim/transforms.rs::on_channel_end`, ICD 6s, min hold 0.4s          |
| `WhirlwindVortex` | Whirlwind pulls enemies inward each tick   | **Yes** — `sim/channel.rs` AuraAroundCaster path                              |

#### `ProcEvent`

`OnCrit`, `OnHit`, `OnKill`, `OnDodge`, `OnLowHealth`.
`OnHit` / `OnCrit` fire from `projectile::apply_hits_to_enemies`;
`OnDodge` and `OnLowHealth` fire from `apply_player_damage`;
`OnKill` is **not yet wired** (needs killer attribution through
`loot::finalise_kills`). Low-HP threshold is 30 % with re-arm latch.

#### `ProcAction` (current shape)

The original three-variant declaration was simplified during Phase 4
— `ChainLightning` was retired in favour of just routing those procs
through `CastAbility(CHAIN_LIGHTNING_ABILITY_ID)` once a chain-lightning
ability exists.

| Action                         | Wired?                                                                                |
| ------------------------------ | ------------------------------------------------------------------------------------- |
| `Explosion { radius, damage }` | **Yes** — pushes a single-tick `ServerAoeZone`                                        |
| `CastAbility(AbilityId)`       | **Yes** — queues a free cast through `ability::dispatch_proc_cast`; cooldown-bypassed |

Proc-cast safety: when `CastAbility` targets a Channel ability (e.g.
Frost Ray, Mirrorglass + Embercrown stack), the dispatch arm
**skips** if the caster is already mid-channel (so the focused cast
isn't stomped, which would orphan its client VFX) and **clamps**
infinite-duration channels to a `PROC_CHANNEL_BURST_SECS = 0.6` burst
with `cancel_on_move = false`. See
[ability.rs](crates/rift-server/src/sim/ability.rs).

Proc rolls are deterministic from `(tick, salt, action_marker)`, so
identical hit identities produce identical outcomes.

### 1.7 The affix pools

The pool is now sharded into **three named constants** rather than a
single `AFFIX_POOL`, so the roll pipeline can address each phase
independently. All three live in
[affixes.rs](crates/rift-game/src/loot/affixes.rs); helper
`affix_iter()` chains them for lookup.

#### Main pool — `AFFIX_POOL` (~21 entries)

Carries everything the roll pipeline samples for **duo axes**
(Attribute / Element) and **bonus** rolls.

**Attribute axis** (signature: 1 line on most items, derived from
`base.family.attribute`):

| id               | Stat      | Roll @ ilvl 1 | Notes                                |
| ---------------- | --------- | ------------- | ------------------------------------ |
| `flat_strength`  | Strength  | 2 .. 5        | rolls only on Strength-family bases  |
| `flat_agility`   | Agility   | 2 .. 5        | rolls only on Agility-family bases   |
| `flat_intellect` | Intellect | 2 .. 5        | rolls only on Intellect-family bases |

(Wildcard bases skip the attribute signature; the bonus pass can still
pull one of these if the base accepts it.)

**Element axis** (signature: 1 line gated by `base.family.element`):

| id                     | Effect              | Roll @ ilvl 1 |
| ---------------------- | ------------------- | ------------- |
| `pct_physical_damage`  | PhysicalDamage (%)  | 8 .. 16 %     |
| `pct_fire_damage`      | FireDamage (%)      | 6 .. 14 %     |
| `pct_ice_damage`       | IceDamage (%)       | 6 .. 14 %     |
| `pct_lightning_damage` | LightningDamage (%) | 6 .. 14 %     |

**Defensive / utility (bonus pool — `weight > 0`, all rarities):**

`flat_health`, `flat_armor`, `flat_health_regen`,
`pct_attack_speed`, `pct_move_speed`, `pct_evasion`,
`pct_resource_regen`, `pct_elemental_resist`,
`pct_healing_received`, `pct_crit_chance`, `pct_crit_damage`,
`pct_cooldown` (rarity_min = Magic).

**Rare-tier — ability amplifiers (`rarity_min = Rare`):**

| id                  | Effect                                | Roll      |
| ------------------- | ------------------------------------- | --------- |
| `amp_frost_ray_dmg` | `AmplifyAbilityDamage(FROST_RAY)`     | +10..20 % |
| `amp_whirlwind_dmg` | `AmplifyAbilityDamage(WHIRLWIND)`     | +10..20 % |
| `cdr_frost_ray`     | `ReduceAbilityCooldown(FROST_RAY)`    | -5..12 %  |
| `cdr_evasive_roll`  | `ReduceAbilityCooldown(EVASIVE_ROLL)` | -5..12 %  |

**Slot-signature affixes (weight 0, never bonus-rolled):**

`flat_armor`, `flat_health`, `pct_cooldown`, `pct_crit_chance`,
`pct_crit_damage`, `pct_move_speed` — these enter items via the
per-slot signature pass (`signature_for` in `affixes.rs`). Note: in
the current implementation Weapons / Rings / Amulets get **no**
signature line; their identity comes entirely from the family-locked
Element axis.

**Legacy legendary-effect lines (still in the main pool — being
migrated to `UNIQUES`):**

| id                            | Effect                                         |
| ----------------------------- | ---------------------------------------------- |
| `mod_fire_ball_extra_proj`    | `ExtraProjectiles(FIRE_BALL)` — fixed +1       |
| `transform_frost_ray_shatter` | `TransformAbility(FROST_RAY, FrostRayShatter)` |

Phase 7 will retire these once the equivalent uniques (Embercrown,
"Frost Ray legendary") are confirmed authored.

#### Resonance pool — `RESONANCE_POOL` (7 entries)

A **separate** pool sampled in its own phase that **breaks the family
lock** — the resonance line specifically grants a duo axis the base
would otherwise reject. Resonance affixes carry `AffixCategory::Resonance`
and use the `res_*` id prefix.

| id                     | Grants                   |
| ---------------------- | ------------------------ |
| `res_physical_damage`  | Physical element synergy |
| `res_fire_damage`      | Fire element             |
| `res_ice_damage`       | Ice element              |
| `res_lightning_damage` | Lightning element        |
| `res_strength`         | Strength attribute       |
| `res_agility`          | Agility attribute        |
| `res_intellect`        | Intellect attribute      |

Drop chance per rarity (from `resonance_chance(rarity)`):
Common 0, Magic 0, Rare 5 %, Legendary 25 %.

#### Rift-touched pool — `RIFT_TOUCHED_POOL` (6 entries)

The endgame slot: only rolls on items dropped from monsters killed
inside an active rift, gated by `RIFT_TOUCHED_MIN_FLOOR` (currently
**1** — a debug-friendly value; Section 2's design north star is
20 and will be raised once endgame pacing settles). Chance per
eligible drop is `RIFT_TOUCHED_CHANCE = 0.20`, and the magnitude
scales by `RIFT_TOUCHED_DEPTH_SCALE = +10 %` per floor above the
floor minimum.

| id                      | Stat                  |
| ----------------------- | --------------------- |
| `rt_crit_chance`        | CritChance (%)        |
| `rt_elemental_resist`   | ElementalResist (%)   |
| `rt_cooldown_reduction` | CooldownReduction (%) |
| `rt_move_speed`         | MoveSpeed (%)         |
| `rt_resource_regen`     | ResourceRegen (%)     |
| `rt_range`              | Range (%)             |

#### Unique catalogue — `UNIQUES`

Hand-authored items with `LegendaryEffect` are no longer pool entries;
they live in [uniques.rs](crates/rift-game/src/loot/uniques.rs) and
get sampled in their own phase once a base has rolled to Legendary.
See §1.12 for the live roster.

### 1.8 The roll pipeline (`Item::roll`)

Now **seven phases**, each consulting one pool. See
[item.rs](crates/rift-game/src/loot/item.rs).

1. **Signature injection.** Deterministic per-`EquipSlot` lines via
   `signature_for(slot, rng)`. Every item gets these regardless of
   rarity. (Slot table below — the `pct_armor` bug noted in earlier
   revisions is **fixed**: Shoulders now returns `flat_armor`.)

   | Slot          | Signature                             |
   | ------------- | ------------------------------------- |
   | Weapon        | (none — identity comes from trio)     |
   | Hands         | `pct_crit_chance` + `pct_crit_damage` |
   | Helm          | `pct_cooldown`                        |
   | Shoulders     | `flat_armor`                          |
   | Chest         | `flat_health`                         |
   | Legs          | `flat_armor`                          |
   | Boots         | `pct_move_speed`                      |
   | Ring1 / Ring2 | (none — trio only)                    |
   | Amulet        | (none — trio only)                    |

2. **Duo rolls (Attribute × Element).** One pick per axis
   the base permits, gated by `BaseFamily::accepts_*`. Common gets
   Attribute only; Magic / Rare / Legendary get both.
   `pick_affix_in_category(AFFIX_POOL, category, ...)` does the
   weighted draw, skipping ids already on the item.

3. **Bonus rolls.** Fill remaining slots from
   `rarity.affix_count_range()` (Common 1–2 / Magic 2–3 / Rare 3–4 /
   Legendary 3–4). Candidate filter:

   ```text
   not in item's affix ids
   AND category == Bonus  (i.e. not Attribute/Element/Resonance/RiftTouched)
   AND min_ilvl <= ilvl
   AND rarity >= rarity_min
   AND weight > 0
   AND base.family.accepts_affix(def)   // family lock
   ```

4. **Legendary unique.** Iff `rarity == Legendary`, sample
   `UNIQUES.iter().filter(|u| u.matches(equip_slot, base_id))`
   weighted by `weight`. Stamps `Item::unique_id = Some(u.id)` and
   appends the unique's `legendary_effect` line. At most one per
   item.

5. **Resonance.** Roll `next_f32() < resonance_chance(rarity)`; on
   success draw one entry from `RESONANCE_POOL`. The chosen axis is
   recorded on the affix's `AffixCategory` and **bypasses** the family
   lock — this is the only legitimate way to attach an off-family
   duo line.

6. **Rift-touched.** When `loot::finalise_kills` rolls the drop and
   the kill happened inside a rift at `floor >= RIFT_TOUCHED_MIN_FLOOR`,
   it calls `Item::apply_rift_touched(floor, rng)` which draws from
   `RIFT_TOUCHED_POOL` with `RIFT_TOUCHED_CHANCE`. Magnitudes are
   scaled by `1.0 + (floor - MIN_FLOOR) * RIFT_TOUCHED_DEPTH_SCALE`.
   The line is stamped on `Item::rift_touched: Option<RolledRiftTouched>`.

7. **Anchored.** Iff Legendary, `next_f32() < ANCHORED_CHANCE`
   (1 / 5 000) sets `Item::anchored = true`. Purely persistence-side
   trait — no stat impact.

`roll_value` is shared across all phases:
`rng.frange(roll.0 + (ilvl-1)*ilvl_scale, roll.1 + (ilvl-1)*ilvl_scale)`,
or the fixed `roll.0` when the range is degenerate (Transform / fixed
Modify lines).

### 1.9 Item runtime fields

```rust
struct Item {
    base: &'static BaseItem,
    rarity: Rarity,
    ilvl: u32,
    affixes: Vec<RolledAffix>,   // [signatures.. , duo.. , bonus.. , unique?, resonance?]
    unique_id: Option<&'static str>,         // points into UNIQUES catalogue
    rift_touched: Option<RolledRiftTouched>, // depth-scaled endgame line
    anchored: bool,              // survives wipe-on-death (Legendary 1/5000)
    unstable: bool,              // picked up inside an active rift
    provenance: Option<LootProvenance>,
}

struct LootProvenance { eligible: Vec<[u8;16]> }  // character UUIDs
```

- **`required_level`** is _derived_: `max(1, ilvl, max(affix.min_ilvl))`.
- **`unstable`** lifecycle: rift drops are stable on the floor → flipped
  `true` at pickup-in-rift → flipped `false` on successful extraction →
  shattered on death/leave-without-extracting.
- **`anchored`** wins over wipe paths _unless_ the item is also
  `unstable` (loot picked up in a rift but not extracted is destroyed
  even if it rolled the anchored trait).
- **`unique_id`** and **`rift_touched`** survive persistence and the
  wire codec; the tooltip pulls them through the
  `TooltipLineKind::{Resonance, RiftTouched}` channels for distinct
  styling.
- **`provenance`** snapshots the party's character UUIDs at
  drop-time; `ShareWindow` time-gates outside-of-party pickups.
  Legacy items with `None` self-bind to the first picker.

### 1.10 Stat & ability-mod aggregation

- `Item::stats()` walks implicits + every `AffixEffect::Stat` and folds
  them into a `StatBlock`.
- `Equipment::active_affix_sum()` sums `Item::stats()` across the 10
  slots; this feeds `CharacterStats::compute`.
- `Equipment::ability_mods()` walks every equipped affix and
  produces an `AbilityMods`:

  ```rust
  struct AbilityMods {
      damage_mult: HashMap<AbilityId, f32>,         // multiplicative
      cooldown_mult: HashMap<AbilityId, f32>,       // multiplicative, floor 0.20
      extra_projectiles: HashMap<AbilityId, u32>,   // additive
      transforms: HashMap<AbilityId, AbilityVariant>, // last-applied wins
      procs: Vec<Proc>,                              // each entry rolls individually
  }
  ```

  Cached on the `ServerPlayer` and rebuilt by `recompute_stats` on every
  equip / unequip. Combat code does constant-time lookups.

### 1.11 Drop tables

[drops.rs](crates/rift-game/src/loot/drops.rs). Per-`MonsterRole` static
table, rolled with a seed of
`tick ^ enemy_net_id*0xBF58476D1CE4E5B9 ^ (floor_index << 48)`.

| Role    | Drop chance | Rarity weights C/M/R/L | Extra rolls  | Pool                                              |
| ------- | ----------- | ---------------------- | ------------ | ------------------------------------------------- |
| Brute   | 18 %        | 70 / 25 / 5 / 0        | 0            | DEFENSE\|MELEE bias + accessory                   |
| Stalker | 20 %        | 60 / 30 / 10 / 0       | 0            | SPEED\|CRIT bias + accessory                      |
| Caster  | 22 %        | 55 / 30 / 13 / 2       | 0            | CASTER\|ANY_ELEM + UTILITY + accessory            |
| Elite   | 85 %        | 20 / 45 / 30 / 5       | 0            | wildcard                                          |
| Boss    | 100 %       | 0 / 30 / 50 / 20       | 2 guaranteed | weapon, armor, **guaranteed legendary accessory** |

Item-level = `floor_index + 1` (clamped ≥ 1). Boss accessory entry has
`ilvl_offset = +2` and `rarity_override = Legendary`. If the rolled
`(base, rarity)` has zero valid affix candidates, rarity falls back to
Common rather than emitting an empty Legendary.

### 1.12 Tooltip rendering

[loot/tooltip.rs](crates/rift-game/src/loot/tooltip.rs). Top-down
order:

1. Display name (rarity-coloured by the renderer; unique items also
   surface their unique name through `unique_id`).
2. `Item Level N`
3. `Requires Level N`
4. `⚠ Unstable — extract to stabilise` (if set)
5. `⚓ Anchored — survives death` (if set)
6. Implicits (base lines)
7. **Signature block**, ordered `[primary, secondary?]`
8. `───` separator + bonus stat affixes
9. Duo affixes (Attribute / Element) — each tagged with
   its `AffixCategory` so the renderer can colour-stripe them.
10. `TooltipLineKind::Resonance` line (cyan stripe) — if the item
    rolled a resonance.
11. `TooltipLineKind::RiftTouched` line (violet stripe) — if rolled.
12. `★ <legendary unique line>` (gold by renderer) — if `unique_id`
    is set.
13. Synergy footer `→ Boosts <ability> (slot N) [<bucket>]` for each
    slotted ability the item helps (when a `Loadout` is supplied).

Each line shows a **named roll band** (`RollBand`: Crude / Fair / Fine
/ Pristine / Perfect) computed by `RollBand::from_percentile`, rendered
in the band's signature colour. The raw `[NN%]` percentile is no
longer displayed.

### 1.13 Persistence & wire

- `Item::to_persisted()` / `from_persisted()` round-trip through
  **stable string ids** — surviving pool reorders. Used by
  `rift-persistence` (Postgres JSONB column `affixes` of
  `[{id, v}, …]`). `unique_id`, `rift_touched`, `anchored`, and
  `provenance` ride alongside in their own JSONB fields.
- `Item::to_wire()` / `from_wire()` use **pool indices** (compact, but
  build-coupled — both peers must agree on `BASE_ITEMS` /
  `AFFIX_POOL` ordering). Used by `ItemBlob` in `rift-net::messages`.
- `unstable` is **not** in `to_wire`'s tuple: the carrier
  `ItemBlob` ships it separately and the constructor defaults
  `unstable = false`.
- `provenance` and `rift_touched` are wire-optional; legacy decode = `None`.

---

## 2. Where we want to be

### 2.0 North star, in one paragraph

**A Diablo-style baseline — readable, punchy, drops feel meaningful — with
a Rift twist that makes items _of the rift, not of the shop_.** Every
weapon respects its physical truth (a sword does not roll spell
scaling). Damage rolls follow a clean **Attribute × Element**
ladder that rarity climbs one rung at a time. (The original design
included a third **Archetype** rung — `Projectile | Melee` — that
shipped in Phase 1 and was retired in May 2026; see the status note
at the top of §1.) Procedural rolls are
honest stat blocks; **Legendaries are hand-authored named items** with
fixed unique effects — Embercrown, Stormcaller, Splinterstep — each one
a build seed. The _Rift_ part comes from the loop: every item you carry
inside a rift is **unstable**, lost on death unless you extract through
the boss-room portal; the rare **Anchored** trait is the only thing
that escapes that rule. Deep-rift drops carry a single **Rift-touched**
slot — one extra line, no clutter.

### 2.1 The Attribute × Element ladder

> **May 2026 retcon.** This section originally documented an
> **Attribute × Element × Archetype** trio. The Archetype axis was
> retired — see the status note at the top of §1 — and the shipped
> ladder is the duo below. The original three-axis prose has been
> pruned; the migration phase logs in §3 still reference "trio" where
> they describe historical work.

Every item has two axis tags it scales with:

- **Attribute** — `Strength | Agility | Intellect` (the core stat the
  item's identity binds to; also the class-scaling stat for damage)
- **Element** — `Physical | Fire | Ice | Lightning` (what it deals)

> The original design had a **Source** axis (`Weapon` / `Spell`).
> That axis was retired — it overlapped completely with the Element
> family (physical = weapon, elemental = spell) and added a rung
> with no real information. **Attribute** replaces it: items now
> also carry the flat point totals (`+14 Strength`, `+8 Intellect`)
> that previously could only come from the manual point-spend
> screen. Gear and the level-up screen are interchangeable inputs to
> the same `Attributes` block.
>
> A third **Archetype** axis (`Projectile | Melee`) shipped in
> Phase 1 and was retired in May 2026 — the per-shape damage stats it
> carried (`ProjectileDamage`, `MeleeDamage`, etc.) duplicated the
> Element axis in practice and have been folded back into the
> matching Element bucket.

Items roll **at most one line per axis**. This is the core rule
that fixes the "+Physical Damage on a staff with Fire affixes"
incoherence. A normal item is now:

```
[ Attribute line ] × [ Element line ] + defensives/utility + (legendary?)
```

Rarity gates how many of those axis lines actually roll:

| Rarity    | Attribute | Element | Bonus (defensives / crit / utility / ability-mod) | Legendary effect |
| --------- | :-------: | :-----: | :-----------------------------------------------: | :--------------: |
| Common    |     ✓     |         |                        1–2                        |                  |
| Magic     |     ✓     |    ✓    |                        2–3                        |                  |
| Rare      |     ✓     |    ✓    |                        3–4                        |                  |
| Legendary |     ✓     |    ✓    |                        3–4                        |  1 named unique  |

> Bonus-count **ranges** (not fixed) so some rares feel rare. Final
> numbers in §2.7.

**No double-dipping.** A sword cannot roll `+Strength` twice; a
staff cannot roll `+Fire Damage` AND `+Ice Damage`. The rolled axis
line wins the slot.

### 2.2 Base-item identity — physical truth first

A base item commits to which **Attribute** and **Element** family
its axis rolls are drawn from. Affixes outside those families are
simply not in its pool.

| Base                      | Attribute                  | Element-family                | Archetype hint      |
| ------------------------- | -------------------------- | ----------------------------- | ------------------- |
| Sword                     | `Strength`                 | `Physical`                    | `Melee`             |
| Dagger                    | `Strength`                 | `Physical`                    | `Melee`             |
| Bow / Crossbow _(future)_ | `Agility`                  | `Physical`                    | `Projectile`        |
| Staff                     | `Intellect`                | any of {Fire, Ice, Lightning} | (wildcard)          |
| Wand                      | `Agility`                  | any of {Fire, Ice, Lightning} | `Projectile`        |
| Robe (chest)              | `Intellect`                | (no element lock)             | (no archetype lock) |
| Heavy armor               | `Strength`                 | `Physical`                    | `Melee`             |
| Light armor               | wildcard                   | flexible                      | flexible            |
| Rings / Amulets           | wildcard (any combination) |                               |

Within the locked family, **non-damage** stats (crit, defensive,
utility, ability-mod) ride on top freely.

**Implicits stay** — they're the _only_ differentiator between two
weapons of the same family (a staff vs. a wand both roll Intellect
/ Fire affixes, but the staff has a fatter Fire Damage implicit and
the wand has CDR + Projectile Damage).
This matches the answer "implicit-only differentiation". Affix pools
themselves don't bias by weapon sub-type beyond the family lock.

### 2.3 Ring / Amulet — the wildcard slots

Accessories are the build-completion slots:

- Can roll any Source / Element / Archetype combo.
- Their axis trio is what makes a Fire-mage **wear matching rings**: if
  your staff is Fire and your amulet is Fire and your rings are Fire,
  you're committing.
- Resonance (§2.5) most often lands on accessories.

### 2.4 Legendary effects — named uniques

> Picked: **hand-authored Legendaries** with fixed unique effects.

The procedural pipeline produces **Rare-shaped** items. When a roll
would have produced a Legendary, instead pick from a **named unique
pool** filtered by `(equip_slot, source_family, element_family)`. Each
unique has:

- **Fixed name** (e.g. "Embercrown")
- **Fixed Source / Element / Archetype** lines at the high end of the
  roll band (or sometimes locked at perfect)
- **Fixed unique effect** — the build-defining line. Uses the existing
  `AffixEffect::Transform | Proc | ExtraProjectiles` machinery, plus a
  new `LegendaryEffect::Bespoke(impl_id)` escape hatch for one-off
  behaviours that don't fit the four patterns.
- **Bonus rolls** at the Legendary count, but rolled randomly inside
  the family lock so two `Embercrown` drops differ on the periphery.

**Catalog scale for v1:** stub 4–6 hero items to validate the system,
then expand catalogue-style (one row per unique). Suggested seed set:

| Name (working title)    | Base   | Family                         | Effect                                                                         |
| ----------------------- | ------ | ------------------------------ | ------------------------------------------------------------------------------ |
| **Embercrown**          | Helm   | Spell / Fire / AoE             | `TransformAbility(FIRE_BALL, FireballToBeam)` — wires the dormant variant      |
| **Stormcaller's Reach** | Staff  | Spell / Lightning / Projectile | `Proc(OnCrit, ChainLightning { 3, 35 })` — wires the dormant action            |
| **Splinterstep**        | Boots  | Weapon / Physical / Melee      | `Proc(OnDodge, Explosion { 3.0, 50 })` — already-wired action                  |
| **Tidebound Cuirass**   | Chest  | Spell / Ice                    | `TransformAbility(WHIRLWIND, WhirlwindVortex)` — wires dormant variant         |
| **Cleavebreaker**       | Sword  | Weapon / Physical / Melee      | `ExtraProjectiles(FIREBALL_VOLLEY)` with a rolled 1–2 range, scaling with ilvl |
| **Mirrorglass Amulet**  | Amulet | wildcard                       | `Proc(OnLowHealth, CastAbility(EVASIVE_ROLL))` — wires dormant action          |

The seed set is intentionally chosen so building it **also wires
every dormant `AbilityVariant` and `ProcAction`** listed in §3 — one
catalogue pass clears most of the gameplay-debt list.

> Adding a new unique = one row + (if needed) one match arm. The
> system stays additive.

### 2.5 Resonance — the deliberate cross-axis bonus

A small, distinctly-styled affix family that **breaks the one-per-axis
rule on purpose**. A staff with a Fire-source identity might
"resonate" with Ice, gaining a small Ice line. This is the _only_
sanctioned way to cross families on a procedural item.

- **Rolls in its own extra slot** — does not eat from the bonus
  budget. 5 % on Rare, 25 % on Legendary.
- **Always reads `Resonance: …`** in tooltip, distinct colour (think
  iridescent / cyan-shift) so the player sees it's a special bonus.
- Values are always **small** (50–70 % of the equivalent in-family
  affix). Resonance is a flavour win, not a power optimisation.
- Resonance lines target the _missing_ axis. A sword (Weapon /
  Physical / Melee) can resonate with Spell, Fire/Ice/Lightning, or
  Projectile/Beam/AoE — but not with a family it already owns.

### 2.6 Rift-touched — one extra slot, gated by floor depth

Past **floor 20**, every rift drop additionally rolls one
**Rift-touched** line. This is:

- **One slot, on top of the rarity budget** — no clutter creep.
- **Always positively-flavoured** to signal "this came from the deep":
  small global multipliers (`+8 % all damage`, `+6 % all resistances`,
  `+5 % ability cooldown`), or thematic flair (`Pulses with rift
energy: +1 to nearby ally damage`).
- Rolled value scales with floor depth above 20.
- Rendered with its own line glyph (`◈ Rift-touched: …`) so the player
  recognises the run depth at a glance.
- Hub vendors, starter drops, and any item that did not originate
  inside an active rift instance **cannot** carry Rift-touched lines.

This satisfies "I like the idea but don't want clutter" — exactly
**one line**, only past a threshold, and only on items the rift itself
produced.

### 2.7 Affix counts per rarity

Ranges, not fixed:

| Rarity    | Bonus affix count (range) | Attribute × Element | Legendary unique | Resonance odds | Rift-touched (floor 20+) |
| --------- | ------------------------- | ------------------- | ---------------- | -------------- | ------------------------ |
| Common    | 1–2                       | Attribute only      | —                | —              | yes                      |
| Magic     | 2–3                       | Attribute + Element | —                | —              | yes                      |
| Rare      | 3–4                       | full duo            | —                | 5 %            | yes                      |
| Legendary | 3–4                       | full duo + fixed    | yes (named)      | 25 %           | yes                      |

(Common's `1–2` is a real bump from today's `0`; the trade is that
Common is now the "vendor / stepping-stone" tier that _can_ be useful
when it high-rolls.)

### 2.8 Roll quality — named bands

The percentile `[NN%]` we already render becomes a **named band** with
a colour:

| Roll % of range | Band     | Tooltip styling         |
| --------------- | -------- | ----------------------- |
| 0 – 24 %        | Crude    | dim grey, no glyph      |
| 25 – 49 %       | Fair     | white                   |
| 50 – 74 %       | Fine     | soft blue               |
| 75 – 94 %       | Pristine | gold, small chevron `▲` |
| 95 – 100 %      | Perfect  | shimmering gold, `★▲`   |

A `Pristine` or `Perfect` Common-tier line that matches your build
should _plausibly_ beat a `Crude` Rare line. This is what makes
high-roll Commons matter without re-balancing the entire value table.

### 2.9 Common-tier drops — keep but reposition

Common still drops (we keep current drop tables broadly), but the new
1–2 bonus count and the named-bands UX shift its identity from
"vendor trash" to "occasionally a build keystone if it rolls
Perfect". The bulk-salvage path already handles the trash case.

### 2.10 Item names

- **Common / Magic / Rare** keep procedural prefix/suffix naming.
  Suggested shape: `<Adjective?> <BaseName> <of-Suffix?>` — adjective
  comes from a high-roll axis line, suffix from a high-roll bonus.
  _Example:_ `Searing Apprentice Staff of the Flame`.
- **Legendaries** display their authored unique name only
  (`Embercrown`), with the base name muted as a subtitle (`Helm`).
- **Anchored** prefix is appended in front of the name as today
  (`Anchored Embercrown` / `Anchored Iron Sword`).
- **Unstable** prefix wins over Anchored visually while inside a rift
  (already implemented).

### 2.11 Diegetic surface — what the player actually sees

- **Inside a rift**: every inventory & equipment slot has a faint
  cyan border (the "unstable" tint). The bottom-status bar reads
  "Loot is unstable until extracted" while you're inside.
- **Boss-room portal pair**: clear text labels — "Extract: stabilise
  your loot" / "Descend: deeper rift, harder fights, richer loot".
  Choosing extract plays a "purification" VFX that ripples through
  the inventory.
- **Anchored items** keep their visual marker through the unstable
  tint so the player can still tell at a glance which items would
  survive death.
- **Rift-touched lines** glow faintly on the tooltip; the floor
  number the item dropped at is shown in small text under the name.

### 2.12 Stat refactor implied by all of the above

To make the axis-rule honest, the live `Stat` vocabulary is
narrowed and re-grouped around the two axes:

- **Attribute axis** (replaces the earlier Source pool —
  `WeaponDamage` / `SpellDamage` were proposed but never
  shipped; weapon identity comes from the Element implicits
  instead): `Strength`, `Agility`, `Intellect`.
- Keep **Element**: `PhysicalDamage`, `FireDamage`, `IceDamage`,
  `LightningDamage`.
- **All other stats stay as today** (Crit, defensives, regen, etc.) —
  they live in the "bonus" pool, no axis rules.
- Drop the bias bitmasks (`tag::FIRE | tag::CASTER` etc.) — the
  _family lock_ on the base item replaces them. Tag-soup goes away;
  base-item identity is enough.

> The original spec also listed an **Archetype axis**
> (`ProjectileDamage` / `BeamDamage` / `AoeDamage` / `MeleeDamage`).
> Phase 1 shipped it as a `{Projectile, Melee}` enum with matching
> stats; it was retired in May 2026 once the duo proved sufficient.

This is a meaningful refactor (`signature_for` and `Item::roll`
restructure, every base needs to declare its family pair), but it's
the change that fixes the incoherence you started this conversation
with.

### 2.13 Crafting / agency

**Deferred.** Land the new rolling model first, get the named uniques
in, see how the loop feels before adding any reroll currency or
locking. The system is designed so a future "reroll one bonus affix"
or "graft a Resonance line" feature is one new operation, not a
re-design.

---

## 3. Migration plan — getting from §1 to §2

Ordered by dependency, not urgency. Each phase is sized so it can land
behind a feature flag (or as a single PR) without breaking persistence
or the wire protocol — the existing `to_persisted` / `from_persisted`
contract keeps old items readable while the pool churns underneath.

### Phase 0 — Triage bug fixes that block design work — **done**

> **Landed.** `signature_for(Shoulders)` now returns `["flat_armor"]`
> and the resolve-every-signature-id test in
> [affixes.rs](crates/rift-game/src/loot/affixes.rs)
> guards the contract. The `roll-quality` smoke test exists under
> [item.rs](crates/rift-game/src/loot/item.rs)
> as the `every_base_rolls_non_empty_at_each_rarity` test. The
> `is_legendary_effect` predicate is retained for filtering legacy
> bonus-pool legendary lines while Phase 7 retires them in favour of
> `UNIQUES`.

### Phase 1 — Family-locked bases (the structural change) — **done**

This is the change that fixes the original incoherence ("sword with
spell affixes"). Everything else builds on top.

> **Landed.** `loot/families.rs` declares `Attribute`, `Element`, and
> `Archetype` enums plus `BaseFamily { attribute, element, archetype
}` with a `WILDCARD` constant. (The original design's `Source` axis
> was folded into `Attribute` once classes settled — the role
> Strength/Agility/Intellect play _is_ the Weapon/Spell distinction
> the old Source axis encoded.) Every row in `BASE_ITEMS` carries a
> `family` field; the legacy `allowed_tags` / `favored_tags` masks
> are preserved for backwards-compat but ignored by the roll
> pipeline (slated for removal in Phase 7 after a DB sweep).
> `BaseFamily::accepts_*` is the family-lock predicate consulted by
> every trio/bonus pick.

> **Deferred from this phase:** the original plan listed an
> `Archetype::{Beam, AoE}` pair that never materialised. Beam and
> AoE damage are runtime stats but **don't roll** — abilities consume
> them through `BeamDamage` / `AoeDamage` stat buckets, not
> archetype affixes. The shipped enum is just `{Projectile, Melee}`.

### Phase 2 — The trio rolling pipeline — **done**

Restructure `Item::roll` ([item.rs L207](crates/rift-game/src/loot/item.rs#L207))
to follow the Source × Element × Archetype ladder from §2.1.

> **Landed (with revisions).** `Rarity::affix_count_range` now returns
> 1-2 / 2-3 / 3-4 / 3-4 ([rarity.rs](crates/rift-game/src/loot/rarity.rs)).
> `affixes.rs` exports `AffixCategory` + `category()` and the
> `affix_attribute / affix_element / affix_archetype` helpers, plus
> `signature_count` / `signature_for` (slim slot-defensive line
> only — the originally-proposed Vitality stat was dropped; Hands
> keeps the crit pair, Weapons / Rings / Amulets get no signature).
> The proposed Source axis (`WeaponDamage` / `SpellDamage`) was also
> cut — weapon identity comes from the Element × Archetype implicits
> on the base instead. The shipped trio is **Attribute × Element ×
> Archetype**, gated by `BaseFamily`.
> `Item::roll` ([item.rs](crates/rift-game/src/loot/item.rs)) now
> runs the five-phase pipeline: signature injection, family-locked
> Attribute × Element × Archetype trio (per the rarity ladder below),
> bonus rolls with a `HashSet<Stat>` dedupe set, Legendary effect
> (procedural — Phase 4 replaces with named uniques), Anchored.
> Three new invariant tests guard the result —
> `axis_lines_respect_family_lock`, `no_stat_appears_twice`,
> `rarity_gates_trio_shape`. 17 loot tests green.

1. **Split `AFFIX_POOL` into four logical sub-pools** (still one slice
   under the hood, tagged via the existing `AffixDef.effect`):
   - **Attribute pool** — `Stat(Strength | Agility | Intellect)`
     (this replaced the originally-proposed Source pool, which
     `WeaponDamage` / `SpellDamage` never shipped).
   - **Element pool** — `Stat(Physical|Fire|Ice|Lightning Damage)`.
   - **Archetype pool** — `Stat(Projectile|Beam|AoE|Melee Damage)`.
   - **Bonus pool** — everything else (crit, defensive, utility,
     regen, ability-amp / CDR).
     Helper: `fn category(&AffixDef) -> AffixCategory { Attribute | Element | Archetype | Bonus }`.

2. **Roll the trio per rarity, gated by base family.**

   ```text
   Common    : 1×Attribute                                  + 1..2 bonus
   Magic     : 1×Attribute + 1×(Element xor Archetype)      + 2..3 bonus
   Rare      : 1×Attribute + 1×Element + 1×Archetype        + 3..4 bonus
   Legendary : 1×Attribute + 1×Element + 1×Archetype        + 3..4 bonus  +named effect
   ```

   Magic's `Element xor Archetype` choice picks whichever the base has
   declared (a wand's archetype is always `Projectile`, so Magic wand
   rolls always picks Archetype; a staff with multi-archetype picks
   whichever rolls higher weight).

3. **Replace `signature_for(slot)`** with the trio + per-slot defensive
   signature:
   - Trio lines are determined by `(rarity, base.family)`, not slot.
   - Slot still injects **one defensive/utility signature** (Helm =
     CDR, Boots = MoveSpeed, etc.) but as a _bonus-pool_ line that
     counts against the bonus budget. The "fat slot signature" idea
     from today goes away because the trio carries the identity.

4. **Bonus-count ranges** from §2.7 — replace
   `rarity.affix_count_range()` with the new ranges. Inclusive on
   both ends; sample uniformly within.

5. **Defensive: no-duplicate-stat invariant.** Today the same `Stat`
   can roll twice via two affix ids. Enforce that within an item,
   no `Stat` appears more than once unless the affix is explicitly
   marked stackable. A small `HashSet<Stat>` during bonus rolling.

### Phase 3 — Resonance affixes — **done**

§2.5. New affix category `Resonance` that **breaks the family lock by
design** and rolls in its own extra slot.

> **Landed.** [affixes.rs](crates/rift-game/src/loot/affixes.rs) gained
> `RESONANCE_POOL` (10 entries — 2 source, 4 element, 4 archetype, all
> at ~60 % of the equivalent in-family roll, `min_ilvl: 5`,
> `rarity_min: Rarity::Rare`). `AffixCategory` extended with
> `Resonance`; `category()` returns it for any def whose id is
> prefixed `res_` (see `is_resonance`). `lookup()` chains
> `AFFIX_POOL` and `RESONANCE_POOL` so persisted resonance lines
> rehydrate transparently. `resonance_chance(rarity)` returns
> 0 / 0 / 0.05 / 0.25.
>
> `Item::roll` ([item.rs](crates/rift-game/src/loot/item.rs)) now
> runs **Phase 4½ — Resonance** between the legendary effect and
> the anchored roll: chance-gates against `resonance_chance`,
> filters `RESONANCE_POOL` to axes the base's `BaseFamily`
> _rejects_, drops already-used `Stat`s, and picks uniformly.
> Wildcard families (accessories) cannot resonate by
> construction — every axis is in-family, so the cross-family
> filter is empty.
>
> Four new invariant tests guard the result:
> `resonance_lines_are_always_cross_family`,
> `at_most_one_resonance_line_per_item`,
> `common_and_magic_never_resonate`, `every_resonance_id_resolves`.
> The Phase 2 `axis_lines_respect_family_lock` and
> `rarity_gates_trio_shape` tests were updated to skip resonance
> lines (they have their own contract). 21 loot tests green.
>
> **Deferred to Phase 6 / 7:** tooltip rendering still draws
> resonance lines through the default `ItemLineKind` branch (no
> cyan tint yet). That UX polish lands with the wider tooltip
> overhaul.

### Phase 4 — Named Legendary uniques — **mostly landed**

§2.4. The most user-visible phase. Replace the current procedural
legendary-effect roll with a hand-authored unique pool.

> **Landed.** [loot/uniques.rs](crates/rift-game/src/loot/uniques.rs)
> declares `UniqueDef` + `LegendaryEffect` and exports `UNIQUES`
> with five entries: **Embercrown** (`FireballToBeam`),
> **Splinterstep** (`OnDodge` → `Explosion`), **Cleavebreaker**
> (`ExtraProjectiles(FIREBALL_VOLLEY, +2)`), **Mirrorglass Amulet**
> (`OnCrit` → `CastAbility(random)`), and **Shardspire**
> (`FrostRayShatter`). `Item::roll` consults `UNIQUES.filter(|u|
u.matches(equip_slot, base_id))` in its Legendary phase and
> stamps `Item::unique_id`. The wire codec carries `unique_id`
> separately so a unique survives a pool reorder.
>
> **Still pending:** **Stormcaller's Reach** (Wand —
> `CastAbility(ChainLightning)`) and **Tidebound Cuirass** (Heavy
> Chest — `Bespoke` water-shield) from the §2.4 seed set. The
> Mirrorglass freeroll multi-effect entry is currently commented out
> in `UNIQUES` pending a roll site that can stamp multiple
> `LegendaryEffect`s on one item.

1. **New module: `loot/uniques.rs`.** Declarative table:
   ```rust
   pub struct UniqueDef {
       pub id:          &'static str,   // "embercrown"
       pub name:        &'static str,   // "Embercrown"
       pub equip_slot:  EquipSlot,
       pub family:      BaseFamily,     // required family for the base
       pub base_id:     Option<&'static str>, // None = any base in family
       pub effect:      LegendaryEffect,
       pub flavor:      &'static str,
   }
   pub static UNIQUES: &[UniqueDef] = &[ /* seed table from §2.4 */ ];
   ```
2. **`LegendaryEffect`** wraps the existing
   `AffixEffect::{TransformAbility, Proc, ExtraProjectiles}` plus a
   new `Bespoke(BespokeId)` variant for one-offs that don't fit the
   four patterns. `BespokeId` is a small enum the combat layer
   matches on — one arm per unique. Adding a unique = one row in
   `UNIQUES` + (if `Bespoke`) one match arm.
3. **Roll override.** In `Item::roll`, when the rolled rarity is
   `Legendary`, sample a `UniqueDef` matching `(equip_slot, family)`
   instead of rolling a procedural legendary-effect affix. The
   trio + bonus rolls happen as for a Rare; the unique's `effect`
   attaches as the legendary line.
4. **Seed the catalogue** with the 6 items from §2.4. The seed
   deliberately wires every dormant `AbilityVariant` and
   `ProcAction`:
   - `Embercrown` wires `FireballToBeam` (server hook).
   - `Stormcaller's Reach` wires `ProcAction::CastAbility<ChainLightning>` (dispatcher arm).
   - `Splinterstep` uses already-wired `OnDodge` + `Explosion`.
   - `Cleavebreaker` exercises ranged `ExtraProjectiles(FIREBALL_VOLLEY)` (verify the volley consumer respects the additive count).
   - `Mirrorglass Amulet` wires `ProcAction::CastAbility` (dispatcher arm) + a free `EVASIVE_ROLL` path that bypasses cooldown.
   - `Shardspire (Staff, Ice)` — `FrostRayShatter` (already wired — gets the existing transform a named home)
5. **Persistence.** Store unique id as a separate columnO
   (`unique_id: Option<String>`) so a unique survives a pool reorder
   and so the renderer can pick the authored name without inferring.
   Wire shape gets a `unique_id: Option<u16>` field on `ItemBlob`.
6. **Tooltip:** authored name as the headline, base name as subtitle.

### Phase 5 — Rift-touched line — **landed**

§2.6. One extra slot, only past a floor threshold, only on
rift-origin items.

> **Landed.** `RIFT_TOUCHED_POOL` (6 entries) +
> `AffixCategory::RiftTouched` + `TooltipLineKind::RiftTouched` are
> live. `Item::rift_touched: Option<RolledRiftTouched>` carries the
> stamp through persistence and wire. The roll site in
> [server/sim/loot.rs](crates/rift-server/src/sim/loot.rs) checks
> `floor_index >= RIFT_TOUCHED_MIN_FLOOR` and rift-origin kills, then
> calls `Item::apply_rift_touched(floor, rng)`.
>
> **One value to revisit:** `RIFT_TOUCHED_MIN_FLOOR` is currently
> set to **1** (debug/bring-up convenience); the §2 design north star
> is **20**. Bumping it is a one-line change once endgame floor pacing
> stabilises.

1. **New affix category** `RiftTouched`, new
   `ItemLineKind::RiftTouched` for the renderer.
2. **Persisted flag** on `Item` — `rift_touched: Option<RiftTouched>`
   carrying the rolled value and floor depth it was earned at.
   Optional field, defaults `None`, fully wire/persistence
   backwards-compatible.
3. **Roll site:** `server::sim::loot::drop_for_enemy`. After
   `Item::roll`, if `floor_index >= 20` and the kill happened
   inside a rift instance, sample one `RiftTouched` line scaled by
   `(floor_index - 20)`.
4. **Wipe semantics:** Rift-touched is **not** auto-stripped at
   extraction (the line is a record of how deep the item came from).
5. **Curate the rift-touched line pool** — small, ~6 lines:
   `+X % all damage`, `+X % all resistances`, `-X % all
cooldowns`, `+X % movement speed`, `+X resource regen`,
   `+X % to all on-X procs`. Magnitudes scale with floor depth, not
   ilvl.

### Phase 6 — Named roll bands & tooltip polish — **partially landed**

§2.8. Pure UX. No gameplay change.

> **Landed.** `rift-ui-types::RollBand` exists with the
> `{Crude, Fair, Fine, Pristine, Perfect}` variants,
> `RollBand::from_percentile` and per-band colours. The tooltip
> renderer surfaces the band name instead of the raw `[NN%]`.
>
> **Still pending:** the inventory **compare-tooltip** path still
> doesn't always thread `Loadout`, so the synergy footer is
> intermittent on the comparison panel. Cyan tint for resonance lines
> and violet for rift-touched lines is now wired through
> `TooltipLineKind` but the inventory-grid mini-tooltip uses the
> default colour pass. Both are queued for the next UI pass.

1. **`rift-ui-types`:** new `RollBand { Crude, Fair, Fine, Pristine, Perfect }`.
   Constructed from the existing `roll_percentile`.
2. **`Item::tooltip`:** suffix the band name + chevron glyph instead of
   raw `[NN%]`. Colour comes from a new `RollBand::color()`.
3. **Compare-tooltip path** ([`rift-ui/src/inventory/mod.rs`](crates/rift-ui/src/inventory/mod.rs)):
   make sure the comparison renderer threads `Loadout` so the synergy
   footer always appears when relevant. (Listed in original §3 as a
   gap.)

### Phase 7 — Naming, polish, and the long tail

1. **Procedural name generator** for Common/Magic/Rare items
   (§2.10): `<Adjective?> <BaseName> <of-Suffix?>`. Adjective drawn
   from the top-rolled axis line, suffix from the top-rolled bonus.
   Pure presentation; rolled item state is unchanged.
2. **Legendary names** — driven by `UniqueDef::name`.
3. **Catalogue expansion** — once the system feels right, expand
   `BASE_ITEMS` toward §3-original's "Catalogue thinness" list:
   - second-and-third bases per armor slot (Heavy / Light / Robe
     parity across Helm / Hands / Legs / Boots / Shoulders),
   - second / third weapon bases per family (long-sword, claymore,
     orb, focus, bow, crossbow),
   - ring / amulet variants with distinct implicit themes.
4. **Diegetic surface work** from §2.11 — unstable tint pass on
   inventory frames, portal label rewrite, rift-touched glow.
5. **Authoring tests** that walk `UNIQUES`, `BASE_ITEMS`, `AFFIX_POOL`
   and assert every cross-reference resolves at compile time / first
   boot.

### Cross-cutting concerns

- **Wire / persistence forward-compat.** Three new optional fields
  may need adding:
  - `unique_id: Option<u16>` on the wire / `Option<String>` in persistence.
  - `rift_touched: Option<RiftTouched>` (single value + depth) — wire
    can serialise as `(value: f32, depth: u16)?`.
  - `resonance: Option<RolledAffix>` is **not** needed — resonance is
    just another entry in the existing `affixes: Vec<RolledAffix>`,
    distinguished by its `AffixDef::category`.
    Each is additive; old client/server builds decode them as `None`.
- **Migration of existing player inventories.** No re-rolling. Old
  items just exist with their pre-refactor affix mix until salvaged.
- **Server-authoritative.** All §2 randomness happens server-side
  (already true); the client renders what `ItemBlob` arrives with.

### Items intentionally left out of this plan (still deferred)

- Crafting / reroll currency (§2.13).
- Set bonuses.
- Affix tier hierarchy (T1/T2/T3 within a single effect).
- Vendor / gambler / target-farming.
- Anything past Legendary (no Mythic tier).
- `OnKill` proc event wiring — interesting but unblocks no §2 goal;
  picked up when a unique wants it.

---

_Last updated: §1 reflects the codebase as of this commit. §2 is the
agreed design target. §3 is the migration roadmap — phases land
sequentially, each behind a flag if needed._
