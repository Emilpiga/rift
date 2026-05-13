# Talent Tree Design

> Status: design draft. Not yet implemented. Supersedes the existing
> tier-based `hunter_tree()` content in
> [crates/rift-game/src/talents.rs](crates/rift-game/src/talents.rs).

---

## 1. Vision

A new character has **no abilities except a baseline neutral attack
(Punch)**. Every other ability — fireballs, heals, swords, pets —
is unlocked by spending **talent points** on nodes in a single
shared **Path-of-Exile-style talent tree**.

The tree has a **central hub** with four (extensible) **routes**
radiating outward:

- **Warrior** — melee, sword swings, charges, parries.
- **Mage** — elemental projectiles & AoE (Fire / Ice / Lightning).
- **Healer** — heals, buffs, support utility.
- **Summoner** — pets / minions.

Players are **not** locked to one route. The tree is connected
through the hub, so you can walk into another route — but doing so
is **expensive** (see §5). The route you invest in most defines your
playstyle and what's available to you; secondary routes give
splashes of utility.

Design goals, in priority order:

1. **Build identity** — every character's tree state is a
   readable, distinct build.
2. **Meaningful choices** — points are scarce relative to nodes,
   so investing is a real trade-off.
3. **Hybrid-friendly but specialization-rewarded** — pure builds
   feel strongest at what they do; hybrids feel flexible but never
   dominant.
4. **Extensible** — adding a fifth route or a new keystone never
   requires touching unrelated content.

---

## 2. Starter State (Level 1)

A fresh character at level 1 has:

| Thing              | Value                                                                                                                               |
| ------------------ | ----------------------------------------------------------------------------------------------------------------------------------- |
| Abilities unlocked | **Punch** only                                                                                                                      |
| Talent points      | **1** (spendable immediately)                                                                                                       |
| Loadout slots      | 6 total, gated by `SLOT_UNLOCK_LEVELS`                                                                                              |
| Default loadout    | Slot 0 = Punch (swappable), rest empty                                                                                              |
| Dodge roll         | _Open question — currently `EVASIVE_ROLL` is talent-gated like everything else; recommended: keep it locked behind a tier-1 talent_ |

### 2.1 Punch — the neutral ability

A built-in unarmed attack that uses the existing `Punch_Jab` and
`Punch_Cross` animation clips. Mechanics:

- One ability id (e.g. `id::PUNCH`).
- **Auto-alternating** clip selection: each successive swing
  alternates `Punch_Jab` → `Punch_Cross` → `Punch_Jab` → …
  Internal state lives on the player; reset after a short idle
  window (~1.0s) so the next combat opens with a Jab.
- **Moveable while casting**: no `forward_step`, no locked
  `attack_dir`, no animation root-motion. Movement curve goes
  through normal locomotion; the swing only plays as an upper-body
  animation.
- Default-equipped in **slot 0**. Can be **swapped out** by the
  player like any other ability. Pre-mapped so the player can
  always fight from frame one.
- No talent investment required, no level gate, no respec
  refund.

This is the only ability with this "always available, fully
mobile, no commit" profile. Every other melee ability commits the
character (locks aim, applies `forward_step`) per the existing
`ActionProfile` system.

---

## 3. Tree Topology

A **single shared tree**, drawn as a graph with a central hub.

```
                    ┌── Mage route ──┐
                    │                │
   Summoner route ──┼──── HUB ──────┼── Warrior route
                    │                │
                    └─ Healer route ─┘
```

### 3.1 Hub

A small cluster of **route-agnostic nodes** in the centre:

- A few generic passives (+max HP, +damage, +crit chance).
- Each route's "entry node" connects from the hub via a short
  **connector chain** (~2-3 cheap passive nodes).
- The hub itself is the only multi-route junction; you cannot
  hop directly from a deep Warrior node to a deep Mage node.

### 3.2 Routes

Each route radiates outward from its entry node and forms its own
sub-graph. Within a route, nodes are organized roughly in **rings**
by depth from the hub:

- **Ring 1 (entry)** — first ability unlock + cheap stat passives.
- **Ring 2** — second/third abilities + modifier nodes for ring-1
  abilities + small keystones.
- **Ring 3** — high-impact abilities, more modifiers, mid keystones.
- **Ring 4 (outer)** — capstone keystones, top-end ability mods,
  build-defining nodes.

Each route ships with ~25-35 nodes; 8-12 keystones spread across
all routes (≈2-3 per route at varying depths).

### 3.3 Edges (prerequisites)

Each node has a list of prerequisite node ids. To spend a point on
a node, **all** prerequisites must have at least one point. This
is what makes the tree a graph rather than a flat list, and what
forces the "you must walk through the connector chain to reach
another route" property.

This replaces the current tier-by-points-spent gate (see §10).

---

## 4. Node Types

The tree uses **five** node kinds. The existing
`TalentEffect` enum maps closely already; we need one new variant
(`UnlockAbility`) and a few touch-ups.

| Kind                 | Effect                                                                                                                         | Notes                                                 |
| -------------------- | ------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------- |
| **Stat passive**     | `PercentBonus` / `FlatBonus(TalentStat, …)`                                                                                    | Already exists. Stackable via `max_rank`.             |
| **Ability unlock**   | `UnlockAbility { id: AbilityId }` _(new variant)_                                                                              | One-shot. Single rank, single point.                  |
| **Ability modifier** | `AbilityMod { target, modifier }`                                                                                              | Already exists. Adds projectiles, pierce, cdr, etc.   |
| **Passive proc**     | `PassiveProc { … }`                                                                                                            | Already exists. X% chance on hit / on crit / on cast. |
| **Keystone**         | Discriminated subtype (new) — single rank, "rule-changing" effect. E.g. `Keystone::CritsApplyBurn`, `Keystone::HealsAlsoBuff`. | High impact, may carry a drawback.                    |

### 4.1 Where ability-modifier nodes live

**Hard rule:** an ability-modifier node lives in the **same route**
as the ability it modifies, and is **topologically downstream** of
the ability's unlock node. Concretely: the modifier node lists the
ability's unlock node (directly or transitively) as a prerequisite.

Result: a player cannot encounter `+1 projectile to Fireball`
before they could conceivably have unlocked Fireball. The tree's
prerequisite graph enforces this; the talent editor / data
validator will reject any modifier node whose prerequisite closure
does not include its target ability's unlock node.

### 4.2 Keystones

Keystones are **rule-changing**, deliberately fewer in number (8-12
total across the tree), and worded so they fit a single sentence:

- Warrior — _"Your melee swings can crit; crits stagger."_
- Mage — _"Crits apply Burn for 3s."_
- Healer — _"Your heals also grant +10% damage for 4s."_
- Summoner — _"Minions inherit your crit chance."_

A keystone is **one rank, one point**. Some carry a drawback (PoE
convention) — to be authored per keystone.

---

## 5. Branching: "Free but Expensive"

This is the central tuning lever. Players can freely walk into any
route, but the tree's topology makes pure builds stronger than
hybrids at what they specialize in.

The mechanism is **purely topological** — no per-route point
counters, no "primary route" flag, no scaling node costs:

1. **All cross-route travel goes through the hub.** Routes connect
   only via the hub's connector chains.
2. **Connector chains cost real points.** Each route entry costs
   ~2-3 connector passives that you'd otherwise skip if you stayed
   in your own route.
3. **Route-cluster passives only count same-route investments.**
   Some passive nodes are clustered as a "Fire mastery cluster"
   (e.g. ring-3 of Mage) and only count points spent **inside that
   route subgraph**. A hybrid player still gets the local stats,
   just not the cluster bonus.
4. **Capstone keystones require deep same-route prerequisites.**
   A ring-4 capstone keystone requires a long prerequisite chain
   that, in practice, only a pure-route build can afford.

The net effect:

- **Pure build**: spends every point on one route → reaches
  capstone keystones, gets cluster bonuses, full identity.
- **Hybrid build**: spends across two routes → loses ~6 points to
  connector chains, can't reach either capstone, no cluster
  bonuses on either side, but gains 2-3 mid-route abilities and
  modifiers from the splash route. Real and viable, just not as
  strong inside any single specialty.

No artificial tax scaling, no "first-node-costs-2-points" magic.
Everything is a normal "1 point per rank" cost; the graph shape
does the work.

---

## 6. Point Economy

| Source           | Points      | Notes                                         |
| ---------------- | ----------- | --------------------------------------------- |
| Level 1 starter  | 1           | So the player can immediately pick something. |
| Every level      | 1           | Existing `experience.rs` already grants this. |
| Quest rewards    | ~5-10 total | Major story beats only.                       |
| Boss kills       | ~5-10 total | Unique boss kills, one-time grants.           |
| **Total at cap** | **~75-85**  | Level cap 60 → 60 from levels + ~15-25 bonus. |

Tree size: ~120-140 nodes total. Total points at cap ≈ **65% of
node count**, so the player physically cannot have everything.
This is the meaningful-choice tuning target — every point spent is
a point not spent elsewhere.

Level slot unlocks: existing `SLOT_UNLOCK_LEVELS` array continues
to govern when the 6 action-bar slots unlock. **Talent unlocks an
ability; level unlocks the slot you'd put it in.**

---

## 7. Respec

Two distinct rare-drop items from dungeons:

| Token                    | Rarity    | Effect                                                        |
| ------------------------ | --------- | ------------------------------------------------------------- |
| **Greater Respec Token** | Very rare | Refund **all** points; tree wipes back to 0.                  |
| **Lesser Respec Token**  | Uncommon  | Refund **one chosen** node's points (all ranks of that node). |

Rules:

- Refunded points become unspent points immediately.
- A refund must respect prerequisites: if removing the point
  would orphan a downstream node, the refund is rejected (or, as
  an alternative we can decide later, the orphaned nodes also
  refund cascade-style). **Default: reject orphaning refunds**;
  player must lesser-respec leaves first or use a greater token.
- Tokens are consumed on use.

Drop-rate tuning is downstream of the loot tables.

---

## 8. Routes — Sample Content

This is the **initial** content pass for the four launch routes.
Names are placeholder. Numbers are tuning placeholders.

### 8.1 Warrior (melee)

| Ring | Node                 | Type     | Effect                                              |
| ---- | -------------------- | -------- | --------------------------------------------------- |
| 1    | Sword Slash          | Unlock   | Unlock `MELEE_ATTACK` ability.                      |
| 1    | Toughness (×3 ranks) | Stat     | +5% MaxHP per rank.                                 |
| 2    | Whirlwind            | Unlock   | Unlock `WHIRLWIND` ability.                         |
| 2    | Heavy Strikes        | Stat     | +15% melee damage.                                  |
| 2    | Reach                | Modifier | `MELEE_ATTACK` arc radius +10%.                     |
| 3    | Charge               | Unlock   | New gap-closer ability (to author).                 |
| 3    | Berserker            | Keystone | Below 50% HP: +30% melee damage, −15% defense.      |
| 4    | Executioner          | Keystone | Melee crits below 30% HP execute (or +200% damage). |

### 8.2 Mage (elemental)

| Ring | Node            | Type     | Effect                                           |
| ---- | --------------- | -------- | ------------------------------------------------ |
| 1    | Fireball        | Unlock   | Unlock `FIRE_BALL`.                              |
| 1    | Intellect (×3)  | Stat     | +5% spell damage per rank.                       |
| 2    | Fireball Volley | Modifier | `FIRE_BALL` fires +2 extra projectiles in a fan. |
| 2    | Frost Ray       | Unlock   | Unlock `FROST_RAY`.                              |
| 3    | Fire Wave       | Unlock   | Unlock `FIRE_WAVE`.                              |
| 3    | Burning Crits   | Keystone | Crits apply Burn (3s DoT).                       |
| 4    | Beam Conduit    | Keystone | `FIRE_BALL` becomes the `FIREBALL_BEAM` variant. |

### 8.3 Healer (support)

| Ring | Node              | Type     | Effect                                            |
| ---- | ----------------- | -------- | ------------------------------------------------- |
| 1    | Mend              | Unlock   | Unlock `HEAL_TARGET`.                             |
| 1    | Vitality (×3)     | Stat     | +5% MaxHP per rank.                               |
| 2    | Regeneration      | Unlock   | Unlock `HEAL_OVER_TIME_TARGET`.                   |
| 2    | Empowered Healing | Modifier | Heals are +20% effective.                         |
| 3    | Battle Prayer     | Keystone | Heals also grant target +10% damage for 4s.       |
| 4    | Sanctuary         | Keystone | Healed targets gain a small shield (10% of heal). |

### 8.4 Summoner (pets)

| Ring | Node             | Type     | Effect                                             |
| ---- | ---------------- | -------- | -------------------------------------------------- |
| 1    | Summon Wolf      | Unlock   | New pet ability (to author).                       |
| 1    | Pet Mastery (×3) | Stat     | +5% pet damage per rank.                           |
| 2    | Pack Tactics     | Modifier | Summon Wolf summons +1 wolf.                       |
| 3    | Summon Golem     | Unlock   | New larger, tankier pet (to author).               |
| 3    | Bonded           | Keystone | Pets inherit your crit chance.                     |
| 4    | Necromancer      | Keystone | Killed enemies have X% chance to rise as a minion. |

Connectors (hub-to-route): 2-3 cheap stat passives per route, each
+2-3% of a generic stat.

---

## 9. UI Flow

(High-level — full UI design is a separate pass.)

- **Tree screen** opens via the existing UI hook for `TalentTree`.
- **Hub view** is the default camera framing; player can pan to
  inspect any route.
- **Node tooltip** shows: name, description, current rank /
  max rank, cost (1 point), prerequisite list (with visual
  indicators for met/unmet).
- **Spend button** on selectable nodes (prereqs met, has unspent
  points). Spending is **instant** and **persisted server-side**
  the same frame.
- **Unspent counter** prominently displayed.
- **Filter toggles**: "Show only my route" / "Hide locked" / "Show
  only unlocks" (no stat passives). Quality-of-life later.
- **Respec UI**: invoked from inventory when a respec token is
  used. Greater token = single confirm. Lesser token = click the
  node to refund.

---

## 10. Migration from Existing System

The current talent system in
[crates/rift-game/src/talents.rs](crates/rift-game/src/talents.rs)
will be **substantially refactored**, not discarded. Concretely:

### 10.1 Keep

- `TalentId(u16)` newtype.
- `TalentNode { id, name, description, max_rank, current_rank, prerequisites, effect }` struct shape.
- `TalentEffect::PercentBonus`, `FlatBonus`, `AbilityMod`,
  `PassiveProc`.
- `TalentStat`, `AbilityModifier` enums.
- `TalentTree { nodes, unspent_points, total_spent }` and its
  spend/refund methods.
- `compute_bonuses` / `stat_modifiers` aggregation pipeline.
- The `talent_points: u32` field in
  [crates/rift-game/src/experience.rs](crates/rift-game/src/experience.rs)
  and its "+1 per level" grant.

### 10.2 Add

- `TalentEffect::UnlockAbility { id: AbilityId }` variant.
- `TalentEffect::Keystone(KeystoneId)` variant + an
  `enum KeystoneId { … }` for type-safe rule-changing effects.
- `Route` enum (or just data-tagged via node ids) for filtering /
  UI grouping. Probably `pub enum Route { Hub, Warrior, Mage, Healer, Summoner }`.
- Per-node `route: Route` field (or implicit from node id range).
- A **graph validator** at startup that ensures every
  ability-modifier node's prerequisite closure contains its target
  ability's unlock node.

### 10.3 Remove / Replace

- **`tier: u8`** field on `TalentNode` — gone. Tiered gating
  (`points_required_for_tier`) is **replaced** by per-node
  `prerequisites: Vec<TalentId>` exclusively. No global
  total-spent gate.
- **`hunter_tree()`** — discarded as the single source of content.
  Replaced by four functions (`warrior_branch()`,
  `mage_branch()`, `healer_branch()`, `summoner_branch()`) +
  `hub_nodes()` that together build the full tree.
- **`Ability::unlock_level: u32`** — repurposed or removed.
  Abilities are no longer gated by character level (except via
  loadout slot unlocks). The new gate is: _ability is castable iff
  its unlock node has rank ≥ 1_. The `unlock_level` field may stay
  in the registry as a soft hint (e.g. "this ability appears in
  ring 3 of its route") but is not enforced.
- **Loadout slot 0 default** — change `default_hero()` from
  `FIRE_BALL` to `PUNCH`. Add `PUNCH` to the ability registry.

### 10.4 Migration plan (rollout)

1. Add `PUNCH` ability (registry + animation wiring + auto-alternating clip selection).
2. Add `UnlockAbility` and `Keystone` variants to `TalentEffect`.
3. Add `Route` enum, add `route` field to `TalentNode`.
4. Remove `tier`, replace `hunter_tree()` with new content functions.
5. Add the four route content functions (warrior/mage/healer/summoner) + hub.
6. Add graph validator (debug assertion at startup).
7. Gate ability casting on `UnlockAbility` rank.
8. Update `default_hero()` loadout to slot 0 = Punch.
9. Add respec-token items + UI hooks.
10. Tune point economy, keystone effects, modifier numbers.

Existing player save data referencing old talent ids will need a
migration. Cleanest is a one-shot reset (since the tree
fundamentally changes shape).

---

## 11. Resolved Decisions

These were open questions in an earlier draft and have been
answered:

1. **Dodge roll (`EVASIVE_ROLL`)**: gated behind a **tier-1 hub
   talent** in the movement cluster of the hub. Not always-available
   like Punch. Costs 1 point, single rank, unlocks the existing
   `EVASIVE_ROLL` ability for casting.
2. **Tree scope**: **per-character**. `TalentTree` continues to
   live on the character entity. No account-shared progression.
3. **AbilityVariant ↔ talents**: ability variants
   (`FireballToBeam`, etc.) **must synergize with legendary item
   effects** rather than being driven exclusively by talents.
   See §12.
4. **Refund cascade**: **reject orphaning refunds** is confirmed.
   Players must lesser-respec leaves before interior nodes, or
   use a Greater token.
5. **Hub size**: **6-10 generic-passive nodes** in the hub,
   small cheap bonuses, plus the dodge-roll unlock and connector
   chains into each route.

## 12. Legendary Item Synergy

Legendary items are a **parallel build axis** to the talent tree
and must mesh cleanly with it. Both systems modify the same
underlying ability data (extra projectiles, cooldowns, variants,
on-hit procs), so they need a shared contract or they will fight
each other.

Design constraints:

- **Variants come from legendaries first, talents second.** An
  ability variant like `FireballToBeam` is the kind of build-defining
  transformation a legendary item should grant ("Inferno's
  Conduit: your Fireball becomes a sustained beam"). Talents may
  reach the same variant via a deep keystone (§8.2 Beam Conduit)
  but it should not be the _only_ path — legendaries are the
  primary source of variants.
- **Stacking rules are additive, not multiplicative.** If a
  legendary grants +2 projectiles and the talent tree grants +1,
  the player gets +3 — no special-cases. This is already how
  `AbilityModifier::ExtraProjectiles` aggregates.
- **Talents amplify; legendaries transform.** Talent nodes
  should generally tweak numbers (more projectiles, faster
  cooldown, more damage). Legendaries should change _kinds_
  (Fireball becomes a Beam, Whirlwind pulls enemies in, Heal
  also damages undead). This keeps each system's identity
  distinct so a player can read a build at a glance.
- **Keystones are the exception.** Keystones are the talent
  tree's way to access transformation-level changes, but they
  are rare, single-rank, and deep in the tree — comparable in
  power to a legendary's signature effect.
- **No talent should hard-require a legendary.** A build planned
  on the tree must function with white/blue gear; legendaries
  should feel like build _amplifiers_, not gates.

Concrete implementation note: both legendary effects and talent
modifiers should flow into the same `AbilityModifier` /
`AbilityVariant` aggregation step at cast time, with legendaries
applied first and talents second (or vice versa — pick one and
stick to it). The exact ordering needs a separate design pass
once the legendary item system is sketched.

## 13. Keystone Drawbacks

**Decision: some keystones carry drawbacks, designer's discretion
per keystone** (D2/D4-style, not blanket PoE-style).

A "drawback" is a paired negative on the keystone itself,
expressed in a single sentence with a "but":

- _Berserker_: "+30% melee damage **but −15% defense.**"
- _Burning Crits_: "Crits apply Burn **but you can no longer crit
  burning enemies.**"

Authoring rules:

- **Powerful keystones get drawbacks.** If a keystone is
  build-defining and tips the power curve significantly (top of
  a route, capstone, ring-4), it should carry a paired negative
  that the player must build around.
- **Flavour-only keystones do not.** Keystones that just "let you
  do a cool thing" without dominating the build (e.g.
  _Necromancer_ — slain enemies sometimes rise as minions) have
  no drawback; they're rewards for going deep, not faustian
  bargains.
- **The drawback must be on-theme.** Berserker trades defense
  because berserkers fight recklessly. Burning Crits loses
  crit-on-burning because the fantasy is "set them on fire and
  let it cook" — not "crit them more". A keystone whose drawback
  has to be invented from scratch is probably a keystone whose
  upside is too strong.
- **Drawbacks are talent-tree-only.** Legendary items never carry
  drawbacks (§12); only keystones do. This keeps the two systems
  feeling distinct: legendaries are pure upside transformations,
  keystones are commitments.

Implementation: the new `Keystone(KeystoneId)` variant in
`TalentEffect` (§10.2) already supports this — the `KeystoneId`
fans out to a hand-authored block that applies both the positive
and the negative effect. No schema change beyond what's already
planned.
