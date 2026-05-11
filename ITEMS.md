# Items, Affixes & Legendary Effects — Design Reference

> Living document. Section 1 captures what is **actually implemented today**
> (read directly from the codebase). Section 2 is intentionally short — it's
> the placeholder we'll fill in together once the as-built picture is agreed.
> Section 3 is the running gap list.

---

## 1. Where we are today

### 1.1 Crate layout

All loot rules live in the `rift-game` crate (engine-agnostic, no I/O), and
are consumed by the server (authoritative rolls + combat application) and
the client (tooltip rendering only). Persistence converts to/from stable
string ids.

| Concern                              | Where                                                                                  |
| ------------------------------------ | -------------------------------------------------------------------------------------- |
| Stat vocabulary                      | [crates/rift-game/src/stats.rs](crates/rift-game/src/stats.rs)                         |
| Module index                         | [crates/rift-game/src/loot/mod.rs](crates/rift-game/src/loot/mod.rs)                   |
| Base items + slots + tags            | [crates/rift-game/src/loot/items.rs](crates/rift-game/src/loot/items.rs)               |
| Affix defs + the `AFFIX_POOL`        | [crates/rift-game/src/loot/affixes.rs](crates/rift-game/src/loot/affixes.rs)           |
| Rarity tier rules                    | [crates/rift-game/src/loot/rarity.rs](crates/rift-game/src/loot/rarity.rs)             |
| Rolled item + tooltip                | [crates/rift-game/src/loot/item.rs](crates/rift-game/src/loot/item.rs)                 |
| Per-monster drop tables              | [crates/rift-game/src/loot/drops.rs](crates/rift-game/src/loot/drops.rs)               |
| Equipment slots → stat/mod aggregate | [crates/rift-game/src/loot/equipment.rs](crates/rift-game/src/loot/equipment.rs)       |
| Aggregated ability mods              | [crates/rift-game/src/loot/ability_mods.rs](crates/rift-game/src/loot/ability_mods.rs) |
| Seeded RNG                           | [crates/rift-game/src/loot/rng.rs](crates/rift-game/src/loot/rng.rs)                   |
| Server roll-on-kill                  | [crates/rift-server/src/sim/loot.rs](crates/rift-server/src/sim/loot.rs#L157)          |
| Server transform consumer            | [crates/rift-server/src/sim/transforms.rs](crates/rift-server/src/sim/transforms.rs)   |
| Server proc consumer                 | [crates/rift-server/src/sim/procs.rs](crates/rift-server/src/sim/procs.rs)             |
| Persistence (Postgres JSONB)         | [crates/rift-persistence/src/lib.rs](crates/rift-persistence/src/lib.rs#L60)           |
| Wire shape                           | [crates/rift-net/src/messages.rs](crates/rift-net/src/messages.rs#L1237)               |

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
target `equip_slot`, `allowed_tags` / `favored_tags` bitmasks, `implicit:
&[(Stat, f32)]`, `min_ilvl`, an `icon` registry key, and an optional
`GenderedModel` for paperdoll art.

Currently authored bases (15):

| id              | Name              | EquipSlot | Allowed tags                          | Favored          | Implicit                           |
| --------------- | ----------------- | --------- | ------------------------------------- | ---------------- | ---------------------------------- |
| staff_basic     | Apprentice Staff  | Weapon    | ANY_ELEMENT \| CASTER \| UTIL \| CRIT | ANY_ELEM\|CASTER | +8% Spell Damage                   |
| sword_basic     | Iron Sword        | Weapon    | MELEE\|CRIT\|SPEED\|DEF\|UTIL         | MELEE\|CRIT      | +10% Weapon Damage                 |
| dagger_basic    | Hunter's Dagger   | Weapon    | MELEE\|CRIT\|SPEED\|UTIL              | CRIT\|SPEED      | +6% Weapon Damage, +5% Crit Chance |
| wand_basic      | Carved Wand       | Weapon    | ANY_ELEM\|CASTER\|UTIL\|SPEED         | CASTER\|UTIL     | +6% Spell Damage, +4% CDR          |
| light_helm      | Leather Helm      | Helm      | DEF\|MELEE\|CRIT\|UTIL                | DEF\|MELEE       | +12 Armor, +15 Health              |
| light_shoulders | Leather Spaulders | Shoulders | SPEED\|CRIT\|DEF\|UTIL                | SPEED\|CRIT      | +2% Evasion, +12 Health            |
| heavy_chest     | Plated Cuirass    | Chest     | DEF\|MELEE\|UTIL                      | DEF\|MELEE       | +24 Armor, +30 Health              |
| light_chest     | Studded Vest      | Chest     | DEF\|SPEED\|CRIT\|UTIL                | SPEED\|CRIT      | +5% Evasion, +18 Health            |
| robe_chest      | Mage Robe         | Chest     | ANY_ELEM\|CASTER\|UTIL\|DEF           | CASTER\|UTIL     | +14 Health, +8% Essence Regen      |
| light_boots     | Leather Boots     | Boots     | SPEED\|CRIT\|DEF\|UTIL                | SPEED            | +5% Move Speed, +3% Evasion        |
| light_gloves    | Leather Gloves    | Hands     | ANY_ELEM\|CASTER\|UTIL                | CASTER\|ANY_ELEM | +3% CDR                            |
| light_legs      | Leather Leggings  | Legs      | DEF\|MELEE\|UTIL                      | DEF              | +16 Armor, +20 Health              |
| ring_basic      | Plain Ring        | Ring1     | ALL                                   | (none)           | (none)                             |
| amulet_basic    | Plain Amulet      | Amulet    | ALL                                   | (none)           | +10 Health                         |

Bias works through bitmask intersection on a fixed tag set:

```
FIRE | ICE | LIGHTNING | CRIT | SPEED | DEFENSE | CASTER | MELEE | UTILITY
```

Affixes whose `tags & base.allowed_tags == 0` cannot roll;
affixes hitting `favored_tags` get **2× weight**. No per-affix
special-casing.

### 1.4 Stats

`Stat` (enum in [stats.rs](crates/rift-game/src/stats.rs#L40)) is the single
vocabulary used by affixes, implicits, tooltips, and the resolved
`CharacterStats` sheet:

- **Offensive:** `CritChance`, `CritDamage`, `AttackSpeed`
- **Damage buckets** (multiply abilities of the matching shape/element):
  `WeaponDamage`, `SpellDamage`, `PhysicalDamage`, `FireDamage`, `IceDamage`,
  `LightningDamage`, `ProjectileDamage`, `BeamDamage`, `AoeDamage`,
  `MeleeDamage`
- **Defensive:** `Health`, `Vitality` (distinct from Health so the
  guaranteed slot line stacks), `Armor`, `Evasion`, `HealthRegen`,
  `ElementalResist`, `HealingReceived`. `Armor` is the sole
  mitigation channel for physical damage (soft-capped flat
  reduction); `ElementalResist` covers Fire / Ice / Lightning.
- **Utility:** `MaxResource`, `CooldownReduction`, `ResourceRegen`,
  `MoveSpeed`

`Stat::is_percent()` decides display + math; percent stats are always
0..1 internally (`+0.05` = `+5 %`).

### 1.5 Rarity

```rust
enum Rarity { Common = 0, Magic = 1, Rare = 2, Legendary = 3 }
```

| Rarity    | Bonus affix count | Color (sRGB)       | Salvage base | What it unlocks                       |
| --------- | ----------------- | ------------------ | ------------ | ------------------------------------- |
| Common    | 0                 | (0.85, 0.85, 0.85) | 1            | Pure stats only                       |
| Magic     | 1                 | (0.40, 0.65, 1.00) | 3            | Synergistic stat clusters             |
| Rare      | 2                 | (1.00, 0.85, 0.30) | 8            | Ability **amplifiers** (dmg / CDR)    |
| Legendary | 3                 | (1.00, 0.45, 0.10) | 25           | + 1 effect: Modify / Transform / Proc |

Salvage scales with ilvl: `base * (1 + ilvl/20)`.

Design intent (verbatim from
[rarity.rs](crates/rift-game/src/loot/rarity.rs#L7)): rarity unlocks
**patterns**, not just bigger numbers.

### 1.6 Affix anatomy

```rust
struct AffixDef {
    id: &'static str,            // stable, used by save/load + wire
    name_template: &'static str, // "{}" replaced with formatted value
    effect: AffixEffect,
    roll: (f32, f32),            // range at ilvl = 1
    ilvl_scale: f32,             // linear growth per ilvl above 1
    tags: u32,                   // synergy mask
    min_ilvl: u32,               // doesn't drop below this ilvl
    rarity_min: Rarity,          // and not below this tier
    weight: u32,                 // base selection weight
}
```

`AffixEffect` is the taxonomy of how an affix **interacts with the combat
layer** — see [affixes.rs](crates/rift-game/src/loot/affixes.rs#L48):

| Variant                                       | Pattern   | Carries                         | Where applied                                                                               |
| --------------------------------------------- | --------- | ------------------------------- | ------------------------------------------------------------------------------------------- |
| `Stat(Stat)`                                  | Stat      | flat or % value                 | `Equipment::active_affix_sum` → `CharacterStats`                                            |
| `AmplifyAbilityDamage(AbilityId)`             | Amplify   | `+value` damage mult            | `AbilityMods::damage_for(id)` (multiplicative stack)                                        |
| `ReduceAbilityCooldown(AbilityId)`            | Amplify   | `-value` cooldown               | `AbilityMods::cooldown_for(id)` (multiplicative, floored at 0.20)                           |
| `ExtraProjectiles(AbilityId)`                 | Modify    | integer adds                    | `AbilityMods::extra_projectiles_for(id)`                                                    |
| `TransformAbility(AbilityId, AbilityVariant)` | Transform | discrete variant token          | `AbilityMods::transform_for(id)` → server hooks in `sim/transforms.rs` and `sim/channel.rs` |
| `Proc(ProcEvent, ProcAction)`                 | Trigger   | proc chance is the rolled value | `AbilityMods::procs` → `sim/procs.rs::dispatch`                                             |

`is_legendary_effect(effect)` is the gate function used by the roll
pipeline; today it returns `true` for `Transform`, `Proc`, and
`ExtraProjectiles`.

#### `AbilityVariant` (transforms — declared)

| Variant           | Concept                                    | Wired in server?                                                     |
| ----------------- | ------------------------------------------ | -------------------------------------------------------------------- |
| `FireballToBeam`  | Fireball morphs into a piercing beam       | **No** (declared only)                                               |
| `FrostRayShatter` | Frost Ray detonates into shards on release | **Yes** — `sim/transforms.rs::on_channel_end`, ICD 6s, min hold 0.4s |
| `WhirlwindVortex` | Whirlwind pulls enemies inward each tick   | **No** (declared only)                                               |

#### `ProcEvent` (declared, partially wired)

`OnCrit`, `OnHit`, `OnKill`, `OnDodge`, `OnLowHealth`.
`OnHit` / `OnCrit` fire from `projectile::apply_hits_to_enemies`;
`OnDodge` and `OnLowHealth` fire from `apply_player_damage`;
`OnKill` is **not yet wired** (needs killer attribution through
`loot::finalise_kills`). Low-HP threshold is 30 % with re-arm latch.

#### `ProcAction` (declared, partially wired)

| Action                                   | Wired?                                         |
| ---------------------------------------- | ---------------------------------------------- |
| `Explosion { radius, damage }`           | **Yes** — pushes a single-tick `ServerAoeZone` |
| `CastAbility(AbilityId)`                 | **No** (declared, dispatcher arm is a no-op)   |
| `ChainLightning { max_targets, damage }` | **No** (declared, dispatcher arm is a no-op)   |

Proc rolls are deterministic from `(tick, salt, action_marker)`, so
identical hit identities produce identical outcomes.

### 1.7 The current `AFFIX_POOL`

Total: **26 entries**, partitioned by intent:

**Common-tier — pure stats (`weight > 0`, available at Common+):**

| id                   | Stat              | Roll @ ilvl 1 | ilvl_scale | Tags                 |
| -------------------- | ----------------- | ------------- | ---------- | -------------------- |
| `flat_health`        | Health            | 10 .. 25      | 4.0        | DEFENSE\|UTILITY     |
| `flat_armor`         | Armor             | 4 .. 9        | 2.0        | DEFENSE\|MELEE       |
| `pct_attack_speed`   | AttackSpeed (%)   | 4 .. 8 %      | 0.3 %/lvl  | SPEED\|MELEE\|CASTER |
| `pct_move_speed`     | MoveSpeed (%)     | 3 .. 7 %      | 0.1 %/lvl  | SPEED\|UTILITY       |
| `pct_evasion`        | Evasion (%)       | 3 .. 7 %      | 0.2 %/lvl  | SPEED\|DEFENSE       |
| `pct_resource_regen` | ResourceRegen (%) | 5 .. 12 %     | 0.4 %/lvl  | UTILITY\|CASTER      |
| `flat_health_regen`  | HealthRegen       | 1 .. 3        | 0.4        | DEFENSE\|UTILITY     |

| `pct_elemental_resist` | ElementalResist (%) | 3 .. 6 % | 0.2 %/lvl | DEFENSE\|CASTER |
| `pct_healing_received` | HealingReceived (%) | 5 .. 12 % | 0.3 %/lvl | DEFENSE\|UTILITY |

**Magic-tier — clusters (`rarity_min = Magic`):**

| id                     | Stat                  | Roll @ ilvl 1 | Tags              |
| ---------------------- | --------------------- | ------------- | ----------------- |
| `pct_crit_chance`      | CritChance (%)        | 2 .. 5 %      | CRIT              |
| `pct_crit_damage`      | CritDamage (%)        | 10 .. 25 %    | CRIT              |
| `pct_cooldown`         | CooldownReduction (%) | 3 .. 6 %      | UTILITY\|CASTER   |
| `pct_fire_damage`      | FireDamage (%)        | 6 .. 14 %     | FIRE\|CASTER      |
| `pct_ice_damage`       | IceDamage (%)         | 6 .. 14 %     | ICE\|CASTER       |
| `pct_lightning_damage` | LightningDamage (%)   | 6 .. 14 %     | LIGHTNING\|CASTER |

**Rare-tier — ability amplifiers (`rarity_min = Rare`):**

| id                  | Effect                                | Roll      |
| ------------------- | ------------------------------------- | --------- |
| `amp_frost_ray_dmg` | `AmplifyAbilityDamage(FROST_RAY)`     | +10..20 % |
| `amp_whirlwind_dmg` | `AmplifyAbilityDamage(WHIRLWIND)`     | +10..20 % |
| `cdr_frost_ray`     | `ReduceAbilityCooldown(FROST_RAY)`    | -5..12 %  |
| `cdr_evasive_roll`  | `ReduceAbilityCooldown(EVASIVE_ROLL)` | -5..12 %  |

**Legendary-tier — gameplay-changing (`rarity_min = Legendary`):**

| id                            | Effect                                         | Notes                  |
| ----------------------------- | ---------------------------------------------- | ---------------------- |
| `mod_fire_ball_extra_proj`    | `ExtraProjectiles(FIRE_BALL)` — fixed +1       | min_ilvl 15, weight 10 |
| `transform_frost_ray_shatter` | `TransformAbility(FROST_RAY, FrostRayShatter)` | min_ilvl 15, weight 8  |

**Slot-signature affixes (weight 0, never bonus-rolled):**

`flat_vitality`, `pct_weapon_damage`, `pct_spell_damage`,
`pct_physical_damage`, `pct_projectile_damage`, `pct_beam_damage`,
`pct_aoe_damage`, `pct_melee_damage` — these only enter items via the
deterministic per-slot signature pass (see § 1.9).

> **Today's truth:** only **2 legendary-effect lines are authored**
> (one `Modify`, one `Transform`). No `Proc` affix exists in the pool
> yet, even though the dispatcher in `sim/procs.rs` is wired for
> `Explosion`. The other two transforms (`FireballToBeam`,
> `WhirlwindVortex`) are declared variants with no `AffixDef` row and
> no server hook.

### 1.8 The roll pipeline (`Item::roll`)

[item.rs L207-281](crates/rift-game/src/loot/item.rs#L207). Four phases:

1. **Signature injection.** Deterministic per-`EquipSlot` lines via
   `signature_for(slot, rng)`. Every item gets these regardless of
   rarity. Composition:

   | Slot          | Signature                                                 |
   | ------------- | --------------------------------------------------------- |
   | Weapon        | Vitality + WeaponDamage + SpellDamage                     |
   | Hands         | Vitality + CritChance + CritDamage                        |
   | Helm          | Vitality + CooldownReduction                              |
   | Shoulders     | Vitality + (`pct_armor` — **note: id missing from pool**) |
   | Chest         | Vitality + Health (`flat_health`)                         |
   | Legs          | Vitality + Armor (`flat_armor`)                           |
   | Boots         | Vitality + MoveSpeed                                      |
   | Ring1 / Ring2 | Vitality + one of `{fire, ice, lightning, physical}` dmg  |
   | Amulet        | Vitality + one of `{projectile, beam, aoe, melee}` dmg    |

   ⚠ **Live bug:** `Shoulders` requests id `"pct_armor"` which does
   not exist in the pool — `lookup` returns `None` and the line is
   silently dropped. (Tracked in § 3.)

2. **Bonus rolls.** Pull `rarity.affix_count_range()` lines from the pool
   (Common: 0, Magic: 1, Rare: 2, Legendary: 3). The candidate filter is:

   ```text
   not in signature ids
   AND not is_legendary_effect
   AND tags & base.allowed_tags != 0
   AND min_ilvl <= ilvl
   AND rarity >= rarity_min
   AND weight > 0
   ```

   Selection is weighted, with `weight ×= 2` when
   `tags & base.favored_tags != 0`. No duplicate ids per item.

3. **Legendary effect.** Iff `rarity == Legendary`, pick one weighted
   pick from the legendary-effect pool (`is_legendary_effect && tags &
base.allowed_tags != 0 && min_ilvl <= ilvl`). At most one per item.

4. **Anchored roll.** Iff Legendary, `next_f32() < ANCHORED_CHANCE`
   (1 / 5 000) sets `Item::anchored = true`. Purely persistence-side
   trait — no stat impact.

`roll_value` is shared across all three roll phases:
`rng.frange(roll.0 + (ilvl-1)*ilvl_scale, roll.1 + (ilvl-1)*ilvl_scale)`,
or the fixed `roll.0` when the range is degenerate (Transform / fixed
Modify lines).

### 1.9 Item runtime fields

```rust
struct Item {
    base: &'static BaseItem,
    rarity: Rarity,
    ilvl: u32,
    affixes: Vec<RolledAffix>,   // [signatures.. , bonus.. , legendary?]
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

[item.rs L313-446](crates/rift-game/src/loot/item.rs#L313). Top-down
order:

1. Display name (rarity-coloured by the renderer)
2. `Item Level N`
3. `Requires Level N`
4. `⚠ Unstable — extract to stabilise` (if set)
5. `⚓ Anchored — survives death` (if set)
6. Implicits (base lines)
7. **Signature block**, ordered `[primary, Vitality, secondary?]`
8. `───` separator + bonus stat affixes
9. Amplify / CDR affixes
10. `★ <legendary line>` (gold by renderer)
11. Synergy footer `→ Boosts <ability> (slot N) [<bucket>]` for each
    slotted ability the item helps (when a `Loadout` is supplied)

Each line shows a roll-quality percentile `[NN%]` when the affix has a
non-degenerate range.

### 1.13 Persistence & wire

- `Item::to_persisted()` / `from_persisted()` round-trip through
  **stable string ids** — surviving pool reorders. Used by
  `rift-persistence` (Postgres JSONB column `affixes` of
  `[{id, v}, …]`).
- `Item::to_wire()` / `from_wire()` use **pool indices** (compact, but
  build-coupled — both peers must agree on `BASE_ITEMS` /
  `AFFIX_POOL` ordering). Used by `ItemBlob` in `rift-net::messages`.
- `unstable` is **not** in `to_wire`'s 5-tuple: the carrier
  `ItemBlob` ships it separately and the constructor defaults
  `unstable = false`.
- `provenance` is wire-optional; legacy decode = `None`.

---

## 2. Where we want to be

### 2.0 North star, in one paragraph

**A Diablo-style baseline — readable, punchy, drops feel meaningful — with
a Rift twist that makes items _of the rift, not of the shop_.** Every
weapon respects its physical truth (a sword does not roll spell
scaling). Damage rolls follow a clean **Attribute × Element × Archetype**
ladder that rarity climbs one rung at a time. Procedural rolls are
honest stat blocks; **Legendaries are hand-authored named items** with
fixed unique effects — Embercrown, Stormcaller, Splinterstep — each one
a build seed. The _Rift_ part comes from the loop: every item you carry
inside a rift is **unstable**, lost on death unless you extract through
the boss-room portal; the rare **Anchored** trait is the only thing
that escapes that rule. Deep-rift drops carry a single **Rift-touched**
slot — one extra line, no clutter.

### 2.1 The Attribute × Element × Archetype ladder

Every item has three axis tags it scales with:

- **Attribute** — `Strength | Agility | Intellect` (the core stat the
  item's identity binds to; also the class-scaling stat for damage)
- **Element** — `Physical | Fire | Ice | Lightning` (what it deals)
- **Archetype** — `Projectile | Melee` (its shape)

> The original design had a **Source** axis (`Weapon` / `Spell`).
> That axis was retired — it overlapped completely with the Element
> family (physical = weapon, elemental = spell) and added a fourth
> rung with no real information. **Attribute** replaces it as the
> third axis: items now also carry the flat point totals
> (`+14 Strength`, `+8 Intellect`) that previously could only come
> from the manual point-spend screen. Gear and the level-up screen
> are interchangeable inputs to the same `Attributes` block.

Items roll **at most one line per axis**. This is the core rule
that fixes the "+Weapon Damage on a staff with Fire affixes"
incoherence. A normal item is now:

```
[ Attribute line ] × [ Element line ] × [ Archetype line ] + defensives/utility + (legendary?)
```

Rarity gates how many of those axis lines actually roll:

| Rarity    | Attribute | Element | Archetype | Bonus (defensives / crit / utility / ability-mod) | Legendary effect |
| --------- | :-------: | :-----: | :-------: | :-----------------------------------------------: | :--------------: |
| Common    |     ✓     |         |           |                        1–2                        |                  |
| Magic     |     ✓     | ✓ _or_  |     ✓     |                        2–3                        |                  |
| Rare      |     ✓     |    ✓    |     ✓     |                        3–4                        |                  |
| Legendary |     ✓     |    ✓    |     ✓     |                        3–4                        |  1 named unique  |

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
weapons of the same family (a staff vs. a wand both roll Spell+Fire,
but the staff has a fatter Spell Damage implicit and the wand has CDR).
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

| Name (working title)    | Base   | Family                         | Effect                                                                    |
| ----------------------- | ------ | ------------------------------ | ------------------------------------------------------------------------- |
| **Embercrown**          | Helm   | Spell / Fire / AoE             | `TransformAbility(FIRE_BALL, FireballToBeam)` — wires the dormant variant |
| **Stormcaller's Reach** | Staff  | Spell / Lightning / Projectile | `Proc(OnCrit, ChainLightning { 3, 35 })` — wires the dormant action       |
| **Splinterstep**        | Boots  | Weapon / Physical / Melee      | `Proc(OnDodge, Explosion { 3.0, 50 })` — already-wired action             |
| **Tidebound Cuirass**   | Chest  | Spell / Ice                    | `TransformAbility(WHIRLWIND, WhirlwindVortex)` — wires dormant variant    |
| **Cleavebreaker**       | Sword  | Weapon / Physical / Melee      | `ExtraProjectiles(MULTI_SHOT)` with a rolled 1–2 range, scaling with ilvl |
| **Mirrorglass Amulet**  | Amulet | wildcard                       | `Proc(OnLowHealth, CastAbility(EVASIVE_ROLL))` — wires dormant action     |

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

| Rarity    | Bonus affix count (range) | Attribute × Element × Archetype    | Legendary unique | Resonance odds | Rift-touched (floor 20+) |
| --------- | ------------------------- | ---------------------------------- | ---------------- | -------------- | ------------------------ |
| Common    | 1–2                       | Attribute only                     | —                | —              | yes                      |
| Magic     | 2–3                       | Attribute + (Element or Archetype) | —                | —              | yes                      |
| Rare      | 3–4                       | full trio                          | —                | 5 %            | yes                      |
| Legendary | 3–4                       | full trio + fixed                  | yes (named)      | 25 %           | yes                      |

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

To make the trio-rule honest, the live `Stat` vocabulary needs to be
narrowed and re-grouped:

- Keep **Source**: `WeaponDamage`, `SpellDamage`.
- Keep **Element**: `PhysicalDamage`, `FireDamage`, `IceDamage`,
  `LightningDamage`.
- Keep **Archetype**: `ProjectileDamage`, `BeamDamage`, `AoeDamage`,
  `MeleeDamage`.
- **All other stats stay as today** (Crit, defensives, regen, etc.) —
  they live in the "bonus" pool, no axis rules.
- Drop the bias bitmasks (`tag::FIRE | tag::CASTER` etc.) — the
  _family lock_ on the base item replaces them. Tag-soup goes away;
  base-item identity is enough.

This is a meaningful refactor (`signature_for` and `Item::roll`
restructure, every base needs to declare its family triple), but it's
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

### Phase 0 — Triage bug fixes that block design work

These are landings the new design can't sit on top of cleanly. None
of them require schema/wire changes.

- **Fix the `pct_armor` signature bug.** Today
  `signature_for(Shoulders)` returns `"pct_armor"`, which is not in
  `AFFIX_POOL` — `lookup` returns `None` and the line is silently
  dropped. Change Shoulders' signature to `flat_armor` (matching
  `Legs`) and assert in a test that every id returned by
  `signature_for` resolves.
  [`affixes.rs` L488](crates/rift-game/src/loot/affixes.rs#L488)
- **Add a `roll-quality test`** that every base in `BASE_ITEMS` can
  produce a non-empty `Item::roll` at each rarity tier (catches
  base-vs-pool drift early — e.g. a tag mask that filters every
  affix out).
- **Decide the deprecation path** for the existing
  `is_legendary_effect` predicate. In the new model effects only
  attach via named uniques, so the predicate stays useful for
  filtering bonus rolls but is no longer the only entry point.

### Phase 1 — Family-locked bases (the structural change)

This is the change that fixes the original incoherence ("sword with
spell affixes"). Everything else builds on top.

1. **Introduce `Family` types** in `rift-game::loot`:

   ```rust
   enum Source     { Weapon, Spell }
   enum Element    { Physical, Fire, Ice, Lightning }
   enum Archetype  { Projectile, Beam, AoE, Melee }
   ```

   These already exist on `Ability` (see `abilities.rs`'s `Scaling`,
   `Element`, `Archetype` — same vocabulary). Either re-use them, or
   move them to a shared `families` module and re-export.

2. **Replace `BaseItem::allowed_tags` / `favored_tags`** with:

   ```rust
   pub struct BaseFamily {
       pub source:    Option<Source>,    // None = wildcard (accessories)
       pub element:   Option<&'static [Element]>, // staff = [Fire,Ice,Lightning]
       pub archetype: Option<&'static [Archetype]>,
   }
   ```

   `None` slots are the wildcard / accessory case. A single-element
   weapon (e.g. a future "Embercoal Wand") just authors `Some(&[Fire])`.

3. **Update every base item row.** Source-of-truth migration:

   | Base                      | Source                                | Element                        | Archetype             |
   | ------------------------- | ------------------------------------- | ------------------------------ | --------------------- |
   | staff_basic               | Spell                                 | any (`[Fire, Ice, Lightning]`) | any (`[AoE, Beam]`)   |
   | wand_basic                | Spell                                 | any                            | `[Projectile]`        |
   | sword_basic               | Weapon                                | `[Physical]`                   | `[Melee]`             |
   | dagger_basic              | Weapon                                | `[Physical]`                   | `[Melee]`             |
   | heavy_chest / legs / helm | Weapon                                | `[Physical]`                   | (none — defence only) |
   | light\_\*                 | (either, flexible — author per piece) |                                |
   | robe_chest                | Spell                                 | (none — element-flex)          | (none)                |
   | ring_basic, amulet_basic  | wildcard (`None`/`None`/`None`)       |                                |

4. **Delete the `tag::*` bitmask module** once nothing reads it.
   Keep it during the migration so the legacy path keeps compiling.

> Forward compatibility: items already in the DB still rehydrate via
> `from_persisted` using affix string ids; the family lock only
> influences _new_ rolls. Old items with off-family affixes simply
> exist as legacy outliers and will be salvaged out by players.

### Phase 2 — The trio rolling pipeline — **done**

Restructure `Item::roll` ([item.rs L207](crates/rift-game/src/loot/item.rs#L207))
to follow the Source × Element × Archetype ladder from §2.1.

> **Landed.** `Rarity::affix_count_range` now returns 1-2 / 2-3 / 3-4 /
> 3-4 ([rarity.rs](crates/rift-game/src/loot/rarity.rs)).
> `affixes.rs` exports `AffixCategory` + `category()` and the
> `affix_source / affix_element / affix_archetype` helpers, plus
> slimmed `signature_count` / `signature_for` (Vitality + one
> slot-defensive line; Hands keeps the crit pair).
> `Item::roll` ([item.rs](crates/rift-game/src/loot/item.rs)) now
> runs the five-phase pipeline: signature injection, family-locked
> Source × Element × Archetype trio (per the rarity ladder below),
> bonus rolls with a `HashSet<Stat>` dedupe set, Legendary effect
> (procedural — Phase 4 replaces with named uniques), Anchored.
> Three new invariant tests guard the result —
> `axis_lines_respect_family_lock`, `no_stat_appears_twice`,
> `rarity_gates_trio_shape`. 17 loot tests green.

1. **Split `AFFIX_POOL` into four logical sub-pools** (still one slice
   under the hood, tagged via the existing `AffixDef.effect`):
   - **Source pool** — `Stat(WeaponDamage | SpellDamage)`.
   - **Element pool** — `Stat(Physical|Fire|Ice|Lightning Damage)`.
   - **Archetype pool** — `Stat(Projectile|Beam|AoE|Melee Damage)`.
   - **Bonus pool** — everything else (crit, defensive, utility,
     regen, ability-amp / CDR).
     Helper: `fn category(&AffixDef) -> AffixCategory { Source | Element | Archetype | Bonus }`.

2. **Roll the trio per rarity, gated by base family.**

   ```text
   Common    : 1×Source                              + 1..2 bonus
   Magic     : 1×Source + 1×(Element xor Archetype)  + 2..3 bonus
   Rare      : 1×Source + 1×Element + 1×Archetype    + 3..4 bonus
   Legendary : 1×Source + 1×Element + 1×Archetype    + 3..4 bonus  +named effect
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

### Phase 4 — Named Legendary uniques

§2.4. The most user-visible phase. Replace the current procedural
legendary-effect roll with a hand-authored unique pool.

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
   - `Stormcaller's Reach` wires `ProcAction::ChainLightning` (dispatcher arm).
   - `Splinterstep` uses already-wired `OnDodge` + `Explosion`.
   - `Tidebound Cuirass` wires `WhirlwindVortex` (server hook).
   - `Cleavebreaker` exercises ranged `ExtraProjectiles(MULTI_SHOT)` (verify multishot consumer).
   - `Mirrorglass Amulet` wires `ProcAction::CastAbility` (dispatcher arm) + a free `EVASIVE_ROLL` path that bypasses cooldown.
5. **Persistence.** Store unique id as a separate column
   (`unique_id: Option<String>`) so a unique survives a pool reorder
   and so the renderer can pick the authored name without inferring.
   Wire shape gets a `unique_id: Option<u16>` field on `ItemBlob`.
6. **Tooltip:** authored name as the headline, base name as subtitle.

### Phase 5 — Rift-touched line

§2.6. One extra slot, only past a floor threshold, only on
rift-origin items.

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

### Phase 6 — Named roll bands & tooltip polish

§2.8. Pure UX. No gameplay change.

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
