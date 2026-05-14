# Finished Talent Tree Blueprint - Draft 1

This document is the working blueprint for the actual finished talent tree. It is intentionally more concrete than `TALENT_TREE.md`: every node has an id, grouping, route, prerequisites, and implementation status. Once this design feels good, the implementation in `crates/rift-game/src/talents/` should become a direct translation of this file.

## Design Targets

- Target size: large final tree, roughly 120-140 nodes.
- First implementation strategy: hybrid. The final tree is visible in-game, but unfinished nodes clearly show WIP status.
- Routes: Warrior, Mage, Healer, Summoner.
- Branches per route: two main sub-archetypes.
- Hybrid access: meaningful cost. A second route should take about 5-8 points before useful payoff.
- Node mix: path stats, ability unlocks, support modifiers, procs, and keystones.
- Keystone style: mixed drawbacks, but drawbacks are reserved for capstones only. Mid-route keystones should be pure upside or low-friction identity picks.
- WIP presentation: unfinished nodes are visible in-game but locked. Players can inspect them, but cannot spend points on them until their ability/system status is Ready or Tuning.
- Hybrid posture: hybrid builds are openly supported. Intra-route bridge nodes can use either-lane prerequisites; cross-route hybrid payoffs should use explicit Synergy nodes so the tree rewards planned archetype pairings instead of accidental route bleed.
- Fifth route: reserve a compact placeholder route now, without adding playable nodes yet.

## Status Tags

- Ready: can be implemented with current ability/stat/effect systems or very small glue.
- Needs Ability: requires a new ability definition, animation, VFX, or server behavior.
- Needs System: requires a new underlying mechanic such as taunt, shield, corpse, pet AI, brittle, or aura handling.
- Tuning: implementable, but numbers should be revisited after playtesting.
- First Slice WIP: intentionally prioritized for the first playable slice, but still locked until its listed ability/system requirements exist.

## Prerequisite Expression Syntax

The implementation can start with simple `all(...)` support because that matches the current prerequisite model. The blueprint uses the richer expression syntax now so we do not have to redesign the tree when hybrid and bridge behavior lands.

- `all(1, 2)`: every listed node must have at least one rank.
- `any(1, 2)`: at least one listed node must have at least one rank.
- `all(1, any(2, 3))`: nested expressions are allowed for future schema support.
- A single id like `1000` is shorthand for `all(1000)`.
- Cross-route Synergy nodes should use `all(route_anchor_a, route_anchor_b)`.
- Intra-route bridge nodes may use `any(...)` when they are meant to support either lane.

## Layout Rules

- Hub is ring 0 and should sit at the center.
- Connector chains are short spokes from hub to route entries.
- Each route has a shared entry, then splits into two lanes.
- Cross-route travel goes through hub connector chains only.
- Ability support clusters sit next to the ability they modify.
- Keystones sit at the end of branch lanes or at major branch joins.
- Every node has an explicit graph-space coordinate in this document. Coordinates are draft layout anchors and can be fine-tuned in-game later.
- Coordinate convention: `(x, y)` in graph units, hub centered at `(0, 0)`, Warrior to the east, Mage north, Healer south, Summoner west.

## Point Economy

- Level 1 starts with 1 unspent point.
- Each level grants 1 point.
- Bonus point sources can be added later from bosses and quests.
- Final tree size in this draft: 132 nodes.
- A focused route path to one capstone should cost about 22-30 points.
- A hybrid path into a second route should cost about 6 points before the first meaningful off-route ability.
- Cross-route Synergy nodes should usually cost 10-16 total prerequisite points counting both route entries, connector travel, and the synergy node itself. They should be strong enough to justify a hybrid build, but weaker than a route capstone.

## Fifth Route Placeholder

The fifth route is intentionally unnamed and unplayable for now. Reserve the `Route::Fifth` concept, `TalentId` range `500..599`, and a future hub spoke roughly between Mage and Warrior at graph angle northeast. No placeholder nodes count toward the 132-node draft total.

Suggested future connector reservation:

| Id  | Ring | Group                   | Name        | Type    | Effect                | Prereq | Status      | Coord      |
| --- | ---- | ----------------------- | ----------- | ------- | --------------------- | ------ | ----------- | ---------- |
| 510 | 1    | Fifth Route Placeholder | Placeholder | Stat x1 | Reserved future route | 4      | Placeholder | (125,-125) |
| 511 | 1    | Fifth Route Placeholder | Placeholder | Stat x1 | Reserved future route | 510    | Placeholder | (190,-190) |

## Synergy Node Model

Synergy nodes are the intentional bridge between different class archetypes. They are not shortcuts between routes and they are not generic stat passives. A synergy node says: "you have invested enough in two fantasies that the build now gets a coordinated payoff."

### Synergy Rules

- Synergy nodes live visually between the two participating routes, closer to the hub than capstones but deeper than connector chains.
- A synergy node requires one anchor from each participating route. This is an `all(...)` prerequisite by design, unlike intra-route bridge nodes that may use `any(...)` prerequisites.
- Each synergy should name a specific play pattern, not a vague stat pile. Good: "frost minions pull chilled enemies." Weak: "+4% damage and +4% minion damage."
- Synergies should be optional payoffs, not mandatory pass-through nodes. A pure route should never path through a cross-route synergy to reach its own capstone.
- Synergy effects should usually be 1-rank nodes. If a synergy has ranks, each rank should broaden or tune the hybrid behavior rather than become raw scaling.
- Synergy power budget should sit below capstones. A synergy can define a hybrid build, but capstones remain the deepest single-route identity rewards.
- Synergy nodes should be visible even while their underlying systems are WIP. They follow the same visible-but-locked rule as other Needs Ability / Needs System nodes.

### Synergy Placement

Synergy nodes use reserved id ranges per route pair. They do not count toward the 132-node core-tree total until we decide to promote them into the playable tree.

| Range      | Pair               | Visual Placement                                  |
| ---------- | ------------------ | ------------------------------------------------- |
| 6100..6199 | Warrior + Mage     | Northeast quadrant between Warrior and Mage       |
| 6200..6299 | Mage + Summoner    | Northwest quadrant between Mage and Summoner      |
| 6300..6399 | Summoner + Healer  | Southwest quadrant between Summoner and Healer    |
| 6400..6499 | Healer + Warrior   | Southeast quadrant between Healer and Warrior     |
| 6500..6599 | Warrior + Summoner | Outer constellation ring, expensive exotic hybrid |
| 6600..6699 | Mage + Healer      | Outer constellation ring, caster-support hybrid   |

### Draft Synergy Nodes

| Id   | Pair               | Name              | Type    | Effect                                                                                                    | Prereq          | Status          | Coord       |
| ---- | ------------------ | ----------------- | ------- | --------------------------------------------------------------------------------------------------------- | --------------- | --------------- | ----------- |
| 6100 | Warrior + Mage     | Flame Reaver      | Synergy | Melee hits against burning enemies create a small fire cleave; Fire Wave empowers your next melee commit. | all(1014, 2012) | Needs System    | (540,-390)  |
| 6101 | Warrior + Mage     | Frostbreaker      | Synergy | Staggering a chilled or brittle enemy triggers a small shatter burst.                                     | all(1032, 2033) | Needs System    | (470,-300)  |
| 6200 | Mage + Summoner    | Cold Star Pact    | Synergy | Void minions deal bonus damage to chilled enemies and can extend Chill on hit.                            | all(2033, 4010) | First Slice WIP | (-300,-520) |
| 6201 | Mage + Summoner    | Rift Combustion   | Synergy | Burning enemies killed near a void summon collapse into a small unstable rift.                            | all(2016, 4013) | Needs System    | (-430,-430) |
| 6300 | Summoner + Healer  | Grave Mercy       | Synergy | Healing a raised minion splashes a smaller heal to you; overhealing minions briefly shields them.         | all(3031, 4030) | Needs System    | (-430,430)  |
| 6301 | Summoner + Healer  | Last Blessing     | Synergy | When a raised minion expires, it releases a small heal or damage pulse based on nearby allies/enemies.    | all(3037, 4037) | Needs System    | (-560,520)  |
| 6400 | Healer + Warrior   | Consecrated Steel | Synergy | Melee commits inside your healing effects gain armor and deal minor holy splash damage.                   | all(1034, 3030) | Needs System    | (430,430)   |
| 6401 | Healer + Warrior   | Frontline Prayer  | Synergy | Shielding or healing yourself briefly empowers the next Shield Bash, Ground Slam, or Melee Attack.        | all(1031, 3003) | Needs System    | (520,350)   |
| 6500 | Warrior + Summoner | Bloodbound Legion | Synergy | Your low-HP melee bonuses also empower nearby minions at reduced value.                                   | all(1017, 4003) | Needs System    | (0,-1350)   |
| 6600 | Mage + Healer      | Radiant Frost     | Synergy | Frost hits against enemies recently damaged by holy spells can create a small protective shield on you.   | all(2030, 3010) | Needs System    | (0,1350)    |

### Synergy Balance Guardrails

- A synergy should require at least one non-connector investment in each route. Route entry alone is not enough.
- A synergy should not grant a route's capstone fantasy early. It can echo a capstone theme, but not replace the capstone.
- A synergy should create a new rotation or target priority. If it only increases a number, demote it to a stat/modifier node inside a route.
- A synergy should usually mention two concrete mechanics, one from each participating route.
- Long-diagonal synergies, such as Warrior + Summoner or Mage + Healer, should be rarer and more expensive than adjacent-route synergies. Place them on an outer constellation ring so they read as special commitments rather than central hub options.
- When a synergy depends on WIP systems from both routes, keep it visible but locked until both halves exist.

## Layout Coordinates

These coordinates are the draft visual layout for the full tree. They are intentionally separate from the node tables so the mechanical design stays readable while the UI still has explicit anchors.

### Hub Coordinates

| Id  | Coord    | Id  | Coord    | Id  | Coord     | Id  | Coord     |
| --- | -------- | --- | -------- | --- | --------- | --- | --------- |
| 1   | (0,-80)  | 2   | (70,-50) | 3   | (85,20)   | 4   | (35,80)   |
| 5   | (-35,80) | 6   | (-85,20) | 7   | (-70,-50) | 8   | (0,-130)  |
| 9   | (130,0)  | 10  | (-130,0) | 100 | (-115,55) | 101 | (-170,90) |
| 110 | (140,0)  | 111 | (220,0)  | 210 | (0,-180)  | 211 | (0,-260)  |
| 310 | (0,180)  | 311 | (0,260)  | 410 | (-180,0)  | 411 | (-260,0)  |

### Warrior Coordinates

| Id   | Coord      | Id   | Coord       | Id   | Coord      | Id   | Coord      |
| ---- | ---------- | ---- | ----------- | ---- | ---------- | ---- | ---------- |
| 1000 | (320,0)    | 1001 | (390,60)    | 1002 | (390,-60)  | 1003 | (430,-120) |
| 1010 | (530,-150) | 1011 | (650,-230)  | 1012 | (760,-285) | 1013 | (760,-215) |
| 1014 | (650,-90)  | 1015 | (760,-135)  | 1016 | (760,-45)  | 1017 | (820,-90)  |
| 1018 | (940,-90)  | 1019 | (1080,-150) | 1020 | (1080,-30) | 1030 | (530,150)  |
| 1031 | (650,90)   | 1032 | (760,45)    | 1033 | (760,135)  | 1034 | (650,230)  |
| 1035 | (760,205)  | 1036 | (760,280)   | 1037 | (820,230)  | 1038 | (940,230)  |
| 1039 | (1080,170) | 1040 | (1080,290)  | 1050 | (900,70)   | 1051 | (1020,70)  |

### Mage Coordinates

| Id   | Coord        | Id   | Coord        | Id   | Coord       | Id   | Coord       |
| ---- | ------------ | ---- | ------------ | ---- | ----------- | ---- | ----------- |
| 2000 | (0,-360)     | 2001 | (-60,-430)   | 2002 | (60,-430)   | 2003 | (0,-500)    |
| 2010 | (-130,-430)  | 2011 | (-210,-500)  | 2012 | (-260,-590) | 2013 | (-260,-700) |
| 2014 | (-360,-760)  | 2015 | (-160,-760)  | 2016 | (-260,-870) | 2017 | (-260,-990) |
| 2018 | (-360,-1120) | 2019 | (-160,-1120) | 2030 | (180,-520)  | 2031 | (290,-560)  |
| 2032 | (290,-480)   | 2033 | (180,-630)   | 2034 | (180,-740)  | 2035 | (290,-800)  |
| 2036 | (70,-800)    | 2037 | (180,-900)   | 2038 | (180,-1010) | 2039 | (80,-1140)  |
| 2040 | (280,-1140)  | 2050 | (0,-920)     | 2051 | (0,-1040)   | 2052 | (0,-1170)   |

### Healer Coordinates

| Id   | Coord      | Id   | Coord       | Id   | Coord      | Id   | Coord       |
| ---- | ---------- | ---- | ----------- | ---- | ---------- | ---- | ----------- |
| 3000 | (0,360)    | 3001 | (-70,430)   | 3002 | (70,430)   | 3003 | (0,500)     |
| 3010 | (180,520)  | 3011 | (290,480)   | 3012 | (290,560)  | 3013 | (180,640)   |
| 3014 | (180,760)  | 3015 | (290,820)   | 3016 | (70,820)   | 3017 | (180,940)   |
| 3018 | (80,1070)  | 3019 | (280,1070)  | 3020 | (80,1200)  | 3030 | (-180,520)  |
| 3031 | (-290,480) | 3032 | (-290,560)  | 3033 | (-180,640) | 3034 | (-290,700)  |
| 3035 | (-70,700)  | 3036 | (-180,820)  | 3037 | (-180,940) | 3038 | (-280,1070) |
| 3039 | (-80,1070) | 3040 | (-280,1200) | 3050 | (0,900)    | 3051 | (0,1020)    |

### Summoner Coordinates

| Id   | Coord        | Id   | Coord        | Id   | Coord        | Id   | Coord        |
| ---- | ------------ | ---- | ------------ | ---- | ------------ | ---- | ------------ |
| 4000 | (-360,0)     | 4001 | (-430,-70)   | 4002 | (-430,70)    | 4003 | (-500,0)     |
| 4010 | (-550,-180)  | 4011 | (-650,-240)  | 4012 | (-650,-120)  | 4013 | (-760,-180)  |
| 4014 | (-870,-240)  | 4015 | (-870,-120)  | 4016 | (-980,-180)  | 4017 | (-1110,-260) |
| 4018 | (-1110,-100) | 4019 | (-1240,-320) | 4020 | (-1240,-200) | 4030 | (-550,180)   |
| 4031 | (-650,120)   | 4032 | (-650,240)   | 4033 | (-760,180)   | 4034 | (-870,120)   |
| 4035 | (-870,240)   | 4036 | (-980,180)   | 4037 | (-1110,180)  | 4038 | (-1240,100)  |
| 4039 | (-1240,260)  | 4040 | (-1370,100)  | 4050 | (-1000,0)    | 4051 | (-1130,0)    |

## Hub - 20 Nodes

The hub gives generic identity and creates expensive but readable route access. The first connector node for each route hangs off a different hub passive to avoid messy crossing edges.

| Id  | Ring | Group              | Name          | Type    | Effect                                       | Prereq | Status       |
| --- | ---- | ------------------ | ------------- | ------- | -------------------------------------------- | ------ | ------------ |
| 1   | 0    | Core               | Vigor         | Stat x3 | +3% max HP per rank                          | -      | Ready        |
| 2   | 0    | Core               | Might         | Stat x3 | +3% damage per rank                          | -      | Ready        |
| 3   | 0    | Core               | Keen Edge     | Stat x3 | +1% crit chance per rank                     | -      | Ready        |
| 4   | 0    | Core               | Focus         | Stat x3 | +2% cooldown reduction per rank              | -      | Ready        |
| 5   | 0    | Core               | Toughness     | Stat x3 | +3% armor per rank                           | -      | Ready        |
| 6   | 0    | Core               | Swift Step    | Stat x3 | +2% move speed per rank                      | -      | Ready        |
| 7   | 0    | Core               | Reflexes      | Stat x3 | +2% attack speed per rank                    | -      | Ready        |
| 8   | 0    | Core               | Precision     | Stat x3 | +5% crit damage per rank                     | -      | Ready        |
| 9   | 0    | Core               | Recovery      | Stat x3 | +2% health recovery or potion value per rank | -      | Needs System |
| 10  | 0    | Core               | Discipline    | Stat x3 | +2% control resistance per rank              | -      | Needs System |
| 100 | 0    | Movement           | Tumbler       | Stat x3 | +3% move speed per rank                      | 6      | Ready        |
| 110 | 1    | Warrior Connector  | Strength      | Stat x1 | +2% damage                                   | 2      | Ready        |
| 111 | 1    | Warrior Connector  | Endurance     | Stat x1 | +2% max HP                                   | 110    | Ready        |
| 210 | 1    | Mage Connector     | Insight       | Stat x1 | +1% crit chance                              | 3      | Ready        |
| 211 | 1    | Mage Connector     | Channeling    | Stat x1 | +2% cooldown reduction                       | 210    | Ready        |
| 310 | 1    | Healer Connector   | Compassion    | Stat x1 | +2% max HP                                   | 1      | Ready        |
| 311 | 1    | Healer Connector   | Devotion      | Stat x1 | +2% cooldown reduction                       | 310    | Ready        |
| 410 | 1    | Summoner Connector | Ominous Bond  | Stat x1 | +2% damage                                   | 8      | Ready        |
| 411 | 1    | Summoner Connector | Rift Sympathy | Stat x1 | +1% crit chance                              | 410    | Ready        |

## Warrior Route - 28 Nodes

Fantasy: direct physical combat with two branches.

- Berserker lane: low-HP aggression, charges, Whirlwind, execute pressure.
- Vanguard lane: armor, stagger, control, defensive retaliation.

### Warrior Shared Entry

| Id   | Ring | Group | Name          | Type     | Effect                      | Prereq | Status       |
| ---- | ---- | ----- | ------------- | -------- | --------------------------- | ------ | ------------ |
| 1000 | 1    | Entry | Sword Slash   | Unlock   | Unlock Melee Attack         | 111    | Ready        |
| 1001 | 1    | Entry | Heavy Strikes | Modifier | Melee Attack +15% damage    | 1000   | Ready        |
| 1002 | 1    | Entry | Reach         | Modifier | Melee Attack +10% arc/range | 1000   | Needs System |
| 1003 | 1    | Entry | Battle Tempo  | Stat x3  | +3% attack speed per rank   | 1000   | Ready        |

### Berserker Lane

| Id   | Ring | Group              | Name             | Type     | Effect                                                                        | Prereq | Status        |
| ---- | ---- | ------------------ | ---------------- | -------- | ----------------------------------------------------------------------------- | ------ | ------------- |
| 1010 | 2    | Berserker Path     | Blood Rush       | Stat x3  | +4% damage per rank while above 80% stamina/energy                            | 1003   | Needs System  |
| 1011 | 2    | Berserker Path     | Charge           | Unlock   | Unlock Charge gap closer                                                      | 1010   | Needs Ability |
| 1012 | 2    | Charge Support     | Momentum         | Modifier | Charge cooldown -0.8s                                                         | 1011   | Needs Ability |
| 1013 | 2    | Charge Support     | Shoulder Break   | Modifier | Charge staggers first enemy hit                                               | 1011   | Needs System  |
| 1014 | 2    | Berserker Path     | Whirlwind        | Unlock   | Unlock Whirlwind                                                              | 1010   | Ready         |
| 1015 | 2    | Whirlwind Support  | Wider Spin       | Modifier | Whirlwind +15% damage/radius                                                  | 1014   | Tuning        |
| 1016 | 2    | Whirlwind Support  | Endless Rotation | Modifier | Whirlwind cooldown -0.5s                                                      | 1014   | Ready         |
| 1017 | 3    | Berserker Path     | Wounded Fury     | Stat x3  | +6% damage per rank below 50% HP                                              | 1014   | Needs System  |
| 1018 | 3    | Berserker Keystone | Berserker        | Keystone | Below 50% HP: +30% melee damage                                               | 1017   | Needs System  |
| 1019 | 4    | Berserker Capstone | Executioner      | Keystone | Melee crits against enemies below 30% HP execute or deal massive bonus damage | 1018   | Needs System  |
| 1020 | 4    | Berserker Capstone | No Retreat       | Proc     | Taking lethal damage once per floor leaves you at 1 HP and grants 3s fury     | 1018   | Needs System  |

### Vanguard Lane

| Id   | Ring | Group             | Name          | Type     | Effect                                                           | Prereq | Status        |
| ---- | ---- | ----------------- | ------------- | -------- | ---------------------------------------------------------------- | ------ | ------------- |
| 1030 | 2    | Vanguard Path     | Iron Posture  | Stat x3  | +5% armor per rank                                               | 1001   | Ready         |
| 1031 | 2    | Vanguard Path     | Shield Bash   | Unlock   | Unlock Shield Bash                                               | 1030   | Needs Ability |
| 1032 | 2    | Shield Support    | Crumple       | Modifier | Shield Bash applies stronger stagger                             | 1031   | Needs System  |
| 1033 | 2    | Shield Support    | Guard Break   | Modifier | Shield Bash deals +25% damage to armored/staggered enemies       | 1031   | Needs System  |
| 1034 | 2    | Vanguard Path     | Ground Slam   | Unlock   | Unlock Ground Slam cone/AoE melee attack                         | 1030   | Needs Ability |
| 1035 | 2    | Slam Support      | Aftershock    | Modifier | Ground Slam leaves a delayed second hit                          | 1034   | Needs System  |
| 1036 | 2    | Slam Support      | Shockwave     | Modifier | Ground Slam range +20%                                           | 1034   | Needs Ability |
| 1037 | 3    | Vanguard Path     | Hold the Line | Proc     | Standing still briefly grants armor and stagger resistance       | 1034   | Needs System  |
| 1038 | 3    | Vanguard Keystone | Unbreakable   | Keystone | +25% armor and stagger immunity during commits                   | 1037   | Needs System  |
| 1039 | 4    | Vanguard Capstone | Retaliation   | Keystone | Blocking or mitigating a heavy hit releases a cone counterattack | 1038   | Needs System  |
| 1040 | 4    | Vanguard Capstone | Banner of War | Unlock   | Unlock a short-lived banner that buffs allies and taunts enemies | 1038   | Needs Ability |

### Warrior Bridge Nodes

| Id   | Ring | Group  | Name              | Type    | Effect                                            | Prereq          | Status       |
| ---- | ---- | ------ | ----------------- | ------- | ------------------------------------------------- | --------------- | ------------ |
| 1050 | 3    | Bridge | Blood and Steel   | Stat x2 | +4% damage and +4% armor per rank                 | any(1017, 1037) | Ready        |
| 1051 | 3    | Bridge | Brutal Discipline | Proc    | Crits reduce defensive ability cooldowns slightly | 1050            | Needs System |

## Mage Route - 28 Nodes

Fantasy: elemental spellcasting with two branches.

- Pyromancer lane: burns, explosions, Fireball, Fire Wave.
- Cryomancer lane: slows, brittle, beams, shatter control.

### Mage Shared Entry

| Id   | Ring | Group | Name          | Type    | Effect                          | Prereq | Status |
| ---- | ---- | ----- | ------------- | ------- | ------------------------------- | ------ | ------ |
| 2000 | 1    | Entry | Fireball      | Unlock  | Unlock Fireball                 | 211    | Ready  |
| 2001 | 1    | Entry | Intellect     | Stat x3 | +5% spell damage per rank       | 2000   | Ready  |
| 2002 | 1    | Entry | Arcane Focus  | Stat x2 | +3% crit chance per rank        | 2000   | Ready  |
| 2003 | 1    | Entry | Quick Casting | Stat x3 | +2% cooldown reduction per rank | 2000   | Ready  |

### Pyromancer Lane

| Id   | Ring | Group             | Name            | Type     | Effect                                                                      | Prereq | Status       |
| ---- | ---- | ----------------- | --------------- | -------- | --------------------------------------------------------------------------- | ------ | ------------ |
| 2010 | 2    | Fireball Support  | Fireball Volley | Modifier | Fireball fires +2 projectiles                                               | 2000   | Ready        |
| 2011 | 2    | Fireball Support  | Kindling        | Modifier | Fireball +15% damage                                                        | 2000   | Ready        |
| 2012 | 2    | Pyro Path         | Ignite          | Proc     | Spell crits have a chance to apply Burn                                     | 2011   | Needs System |
| 2013 | 2    | Pyro Path         | Fire Wave       | Unlock   | Unlock Fire Wave                                                            | 2012   | Ready        |
| 2014 | 2    | Fire Wave Support | Wave Rider      | Modifier | Fire Wave +15% damage                                                       | 2013   | Ready        |
| 2015 | 2    | Fire Wave Support | Backdraft       | Modifier | Fire Wave pulls burning enemies slightly inward                             | 2013   | Needs System |
| 2016 | 3    | Pyro Path         | Combustion      | Proc     | Burning enemies explode on death for area fire damage                       | 2015   | Needs System |
| 2017 | 3    | Pyro Keystone     | Burning Crits   | Keystone | Crits apply Burn                                                            | 2016   | Needs System |
| 2018 | 4    | Pyro Capstone     | Inferno Heart   | Keystone | Burning enemies take ramping damage, but your non-fire damage is reduced    | 2017   | Needs System |
| 2019 | 4    | Pyro Capstone     | Phoenix Spark   | Proc     | Once per floor, lethal damage detonates nearby enemies and restores some HP | 2017   | Needs System |

### Cryomancer Lane

| Id   | Ring | Group             | Name           | Type     | Effect                                                                                | Prereq | Status        |
| ---- | ---- | ----------------- | -------------- | -------- | ------------------------------------------------------------------------------------- | ------ | ------------- |
| 2030 | 2    | Cryo Path         | Frost Ray      | Unlock   | Unlock Frost Ray                                                                      | 2002   | Ready         |
| 2031 | 2    | Frost Ray Support | Piercing Frost | Modifier | Frost Ray pierces +1 target                                                           | 2030   | Ready         |
| 2032 | 2    | Frost Ray Support | Glacial Edge   | Modifier | Frost Ray +15% damage                                                                 | 2030   | Ready         |
| 2033 | 2    | Cryo Path         | Chill          | Proc     | Frost hits slow enemies                                                               | 2030   | Needs System  |
| 2034 | 2    | Cryo Path         | Ice Lance      | Unlock   | Unlock Ice Lance single-target projectile                                             | 2033   | Needs Ability |
| 2035 | 2    | Ice Lance Support | Splinter       | Modifier | Ice Lance splits on chilled targets                                                   | 2034   | Needs System  |
| 2036 | 2    | Ice Lance Support | Deep Freeze    | Modifier | Ice Lance can freeze brittle enemies                                                  | 2034   | Needs System  |
| 2037 | 3    | Cryo Path         | Brittle        | Proc     | Repeated cold hits apply Brittle; next heavy hit shatters                             | 2036   | Needs System  |
| 2038 | 3    | Cryo Keystone     | Absolute Zero  | Keystone | Frozen enemies take greatly increased damage                                          | 2037   | Needs System  |
| 2039 | 4    | Cryo Capstone     | Shatterstorm   | Keystone | Shattering an enemy launches ice shards at nearby enemies                             | 2038   | Needs System  |
| 2040 | 4    | Cryo Capstone     | Beam Conduit   | Keystone | Fireball can become a beam-style spell if no Fireball projectile modifiers are active | 2038   | Needs System  |

### Mage Bridge Nodes

| Id   | Ring | Group  | Name              | Type     | Effect                                                                                                       | Prereq          | Status       |
| ---- | ---- | ------ | ----------------- | -------- | ------------------------------------------------------------------------------------------------------------ | --------------- | ------------ |
| 2050 | 3    | Bridge | Thermal Shock     | Proc     | Fire damage against chilled enemies, or frost damage against burning enemies, deals bonus damage             | any(2016, 2037) | Needs System |
| 2051 | 3    | Bridge | Elemental Savant  | Stat x2  | +4% fire and frost damage per rank                                                                           | 2050            | Ready        |
| 2052 | 4    | Bridge | Unstable Elements | Keystone | Alternating fire and frost spells grants stacking damage, but repeating the same element consumes the stacks | 2051            | Needs System |

## Healer Route - 28 Nodes

Fantasy: solo-capable support with two branches.

- Battle Priest lane: holy offense, self-sustain, damage through healing rhythm.
- Restoration lane: HoTs, shields, protection, multiplayer value.

### Healer Shared Entry

| Id   | Ring | Group | Name       | Type     | Effect                           | Prereq | Status |
| ---- | ---- | ----- | ---------- | -------- | -------------------------------- | ------ | ------ |
| 3000 | 1    | Entry | Mend       | Unlock   | Unlock Heal Target               | 311    | Ready  |
| 3001 | 1    | Entry | Vitality   | Stat x3  | +5% max HP per rank              | 3000   | Ready  |
| 3002 | 1    | Entry | Faith      | Stat x3  | +5% healing/holy damage per rank | 3000   | Tuning |
| 3003 | 1    | Entry | Quick Mend | Modifier | Heal Target cooldown -0.5s       | 3000   | Ready  |

### Battle Priest Lane

| Id   | Ring | Group                  | Name                 | Type     | Effect                                                                                                  | Prereq | Status        |
| ---- | ---- | ---------------------- | -------------------- | -------- | ------------------------------------------------------------------------------------------------------- | ------ | ------------- |
| 3010 | 2    | Battle Priest Path     | Smite                | Unlock   | Unlock Smite holy projectile/strike                                                                     | 3002   | Needs Ability |
| 3011 | 2    | Smite Support          | Consecrated Force    | Modifier | Smite +20% damage against damaged enemies                                                               | 3010   | Needs Ability |
| 3012 | 2    | Smite Support          | Radiant Splash       | Modifier | Smite splashes healing to you or lowest ally                                                            | 3010   | Needs System  |
| 3013 | 2    | Battle Priest Path     | Zeal                 | Stat x3  | +3% attack/cast speed per rank after healing                                                            | 3012   | Needs System  |
| 3014 | 2    | Battle Priest Path     | Holy Nova            | Unlock   | Unlock point-blank holy AoE that damages enemies and heals allies                                       | 3013   | Needs Ability |
| 3015 | 2    | Nova Support           | Blinding Light       | Modifier | Holy Nova briefly blinds or disorients enemies                                                          | 3014   | Needs System  |
| 3016 | 2    | Nova Support           | Overflow             | Modifier | Overhealing with Holy Nova becomes a short shield                                                       | 3014   | Needs System  |
| 3017 | 3    | Battle Priest Keystone | Battle Prayer        | Keystone | Heals grant +10% damage for 4s                                                                          | 3016   | Needs System  |
| 3018 | 4    | Battle Priest Capstone | Wrathful Benediction | Keystone | Your offensive holy spells also heal you, but direct heals cost longer cooldowns                        | 3017   | Needs System  |
| 3019 | 4    | Battle Priest Capstone | Martyr's Flame       | Proc     | Taking damage charges your next heal or Smite                                                           | 3017   | Needs System  |
| 3020 | 4    | Battle Priest Capstone | Judgement Day        | Modifier | Smite and Holy Nova deal bonus damage to enemies recently healed by your enemies or affected by shields | 3018   | Needs System  |

### Restoration Lane

| Id   | Ring | Group                | Name            | Type     | Effect                                                          | Prereq | Status        |
| ---- | ---- | -------------------- | --------------- | -------- | --------------------------------------------------------------- | ------ | ------------- |
| 3030 | 2    | Restoration Path     | Regeneration    | Unlock   | Unlock Heal over Time                                           | 3001   | Ready         |
| 3031 | 2    | HoT Support          | Lingering Mend  | Modifier | Heal over Time +15% effect                                      | 3030   | Ready         |
| 3032 | 2    | HoT Support          | Steady Flow     | Modifier | Heal over Time cooldown -0.5s                                   | 3030   | Ready         |
| 3033 | 2    | Restoration Path     | Safeguard       | Unlock   | Unlock a targeted shield                                        | 3031   | Needs Ability |
| 3034 | 2    | Shield Support       | Reinforced Ward | Modifier | Safeguard shield +20%                                           | 3033   | Needs System  |
| 3035 | 2    | Shield Support       | Shared Shelter  | Modifier | Safeguard grants a smaller shield to nearby allies              | 3033   | Needs System  |
| 3036 | 3    | Restoration Path     | Sanctuary Field | Unlock   | Unlock ground zone that heals allies over time                  | 3035   | Needs Ability |
| 3037 | 3    | Restoration Keystone | Sanctuary       | Keystone | Healed targets gain a small shield equal to 10% of the heal     | 3036   | Needs System  |
| 3038 | 4    | Restoration Capstone | Second Sunrise  | Keystone | Once per floor, your heal prevents death on an ally or yourself | 3037   | Needs System  |
| 3039 | 4    | Restoration Capstone | Warden's Circle | Proc     | Standing in your healing zone grants damage reduction           | 3037   | Needs System  |
| 3040 | 4    | Restoration Capstone | Wellspring      | Modifier | Sanctuary Field pulses faster on low-HP allies                  | 3038   | Needs System  |

### Healer Bridge Nodes

| Id   | Ring | Group  | Name             | Type    | Effect                                                                              | Prereq          | Status       |
| ---- | ---- | ------ | ---------------- | ------- | ----------------------------------------------------------------------------------- | --------------- | ------------ |
| 3050 | 3    | Bridge | Harmonic Prayer  | Proc    | Healing after dealing damage, or dealing damage after healing, grants a small bonus | any(3013, 3032) | Needs System |
| 3051 | 3    | Bridge | Grace Under Fire | Stat x2 | +4% healing and +4% armor per rank                                                  | 3050            | Ready        |

## Summoner Route - 28 Nodes

Fantasy: void summons and necromancy. Avoid generic animal pets. The route should feel eerie, unstable, and tactical.

- Voidcaller lane: summoned void entities, portals, unstable minions.
- Necromancer lane: corpses, death procs, raised minions.

### Summoner Shared Entry

| Id   | Ring | Group | Name              | Type    | Effect                                         | Prereq | Status        |
| ---- | ---- | ----- | ----------------- | ------- | ---------------------------------------------- | ------ | ------------- |
| 4000 | 1    | Entry | Void Familiar     | Unlock  | Unlock a small void familiar summon            | 411    | Needs Ability |
| 4001 | 1    | Entry | Pet Mastery       | Stat x3 | +5% minion damage per rank                     | 4000   | Needs System  |
| 4002 | 1    | Entry | Binder's Focus    | Stat x3 | +3% minion health per rank                     | 4000   | Needs System  |
| 4003 | 1    | Entry | Unstable Sympathy | Proc    | Your crits briefly empower your active minions | 4000   | Needs System  |

### Voidcaller Lane

| Id   | Ring | Group               | Name                | Type     | Effect                                                                     | Prereq | Status        |
| ---- | ---- | ------------------- | ------------------- | -------- | -------------------------------------------------------------------------- | ------ | ------------- |
| 4010 | 2    | Voidcaller Path     | Riftling Swarm      | Unlock   | Unlock multiple short-lived void riftlings                                 | 4001   | Needs Ability |
| 4011 | 2    | Riftling Support    | Many Mouths         | Modifier | Riftling Swarm summons +1 riftling                                         | 4010   | Needs System  |
| 4012 | 2    | Riftling Support    | Hungering Riftlings | Modifier | Riftlings deal bonus damage to low-HP enemies                              | 4010   | Needs System  |
| 4013 | 2    | Voidcaller Path     | Void Gate           | Unlock   | Unlock a portal that periodically spawns void entities                     | 4011   | Needs Ability |
| 4014 | 2    | Gate Support        | Wide Aperture       | Modifier | Void Gate spawns faster but lasts slightly shorter                         | 4013   | Needs System  |
| 4015 | 2    | Gate Support        | Collapse            | Modifier | Void Gate explodes when it expires                                         | 4013   | Needs System  |
| 4016 | 3    | Voidcaller Path     | Bonded              | Keystone | Your minions inherit your crit chance                                      | 4015   | Needs System  |
| 4017 | 4    | Voidcaller Capstone | Beyond the Veil     | Keystone | You can maintain one extra summon, but your own direct damage is reduced   | 4016   | Needs System  |
| 4018 | 4    | Voidcaller Capstone | Event Horizon       | Proc     | Minion hits have a small chance to pull enemies toward their target        | 4016   | Needs System  |
| 4019 | 4    | Voidcaller Capstone | Rift Sovereign      | Keystone | Void Gate becomes permanent until recast, but reserves part of your max HP | 4017   | Needs System  |
| 4020 | 4    | Voidcaller Capstone | Starved Threshold   | Proc     | When a void minion expires, it briefly weakens nearby enemies              | 4017   | Needs System  |

### Necromancer Lane

| Id   | Ring | Group                | Name               | Type     | Effect                                                                     | Prereq | Status        |
| ---- | ---- | -------------------- | ------------------ | -------- | -------------------------------------------------------------------------- | ------ | ------------- |
| 4030 | 2    | Necromancer Path     | Raise Husk         | Unlock   | Raise a temporary corpse husk from a slain enemy                           | 4002   | Needs Ability |
| 4031 | 2    | Husk Support         | Bone Memory        | Modifier | Raised husks inherit a small part of the slain enemy's damage              | 4030   | Needs System  |
| 4032 | 2    | Husk Support         | Grave Pace         | Modifier | Raised husks move faster and decay slower                                  | 4030   | Needs System  |
| 4033 | 2    | Necromancer Path     | Corpse Burst       | Unlock   | Detonate a corpse or husk for area damage                                  | 4031   | Needs Ability |
| 4034 | 2    | Burst Support        | Black Powder       | Modifier | Corpse Burst radius +20%                                                   | 4033   | Needs System  |
| 4035 | 2    | Burst Support        | Bone Shrapnel      | Modifier | Corpse Burst fires fragments at nearby enemies                             | 4033   | Needs System  |
| 4036 | 3    | Necromancer Path     | Death Tax          | Proc     | Enemies killed by minions have a chance to leave a usable corpse           | 4035   | Needs System  |
| 4037 | 3    | Necromancer Keystone | Necromancer        | Keystone | Slain enemies have a chance to rise as minions                             | 4036   | Needs System  |
| 4038 | 4    | Necromancer Capstone | Army of the Hollow | Keystone | Raised minions last much longer, but individual minions deal less damage   | 4037   | Needs System  |
| 4039 | 4    | Necromancer Capstone | Last Rites         | Proc     | Consuming a corpse heals your minions and damages nearby enemies           | 4037   | Needs System  |
| 4040 | 4    | Necromancer Capstone | Black Communion    | Proc     | When a raised minion dies, nearby raised minions gain attack speed briefly | 4038   | Needs System  |

### Summoner Bridge Nodes

| Id   | Ring | Group  | Name             | Type    | Effect                                                          | Prereq          | Status       |
| ---- | ---- | ------ | ---------------- | ------- | --------------------------------------------------------------- | --------------- | ------------ |
| 4050 | 3    | Bridge | Empty Choir      | Proc    | When a void minion dies near a corpse, it can raise a weak husk | any(4015, 4036) | Needs System |
| 4051 | 3    | Bridge | Pact Mathematics | Stat x2 | +4% minion damage and +4% minion health per rank                | 4050            | Needs System |

## Cross-Route Hybrid Hooks

These are not direct edges. They are design notes for archetypes that should become attractive through Synergy nodes after paying connector and route-entry costs.

- Warrior + Healer: Vanguard plus Restoration should create a durable frontliner. Watch nodes: Banner of War, Sanctuary, Warden's Circle.
- Warrior + Mage: Berserker plus Pyromancer should create a risky melee fire build. Watch nodes: Wounded Fury, Burning Crits, Thermal Shock.
- Mage + Summoner: Cryomancer plus Voidcaller should create control-heavy minion play. Watch nodes: Brittle, Event Horizon, Empty Choir.
- Healer + Summoner: Restoration plus Necromancer should create sustain-heavy minion play. Watch nodes: Last Rites, Sanctuary Field.
- Warrior + Summoner: Berserker plus minion empowerment should create a dangerous blood-pact build. Watch nodes: Wounded Fury, Unstable Sympathy, Bloodbound Legion.
- Mage + Healer: Cryomancer plus Battle Priest should create a defensive spellcaster. Watch nodes: Frost Ray, Smite, Radiant Frost.

## First Playable Slice

The first playable slice should prove the whole tree shape without requiring every final system. It includes all four route identities and one cross-route Synergy node. The full blueprint remains visible; nodes outside this slice stay visible-but-locked until their systems are ready.

### First-Slice Content

| Area     | First playable content                                             | Notes                                                                                                   |
| -------- | ------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------- |
| Core     | Punch, Evasive Roll, hub passives, route connectors                | Punch and Evasive Roll are baseline abilities; the tree starts at hub passives and route connectors.    |
| Warrior  | Melee Attack, Whirlwind, basic Warrior passives/modifiers          | Avoid Shield Bash, Ground Slam, Charge, and stagger systems until later.                                |
| Mage     | Fireball, Fire Wave, Frost Ray, basic Mage passives/modifiers      | Add lightweight Chill support so Cold Star Pact can work; document Brittle/Freeze as future extensions. |
| Healer   | Heal Target, Heal over Time, basic Healer passives/modifiers       | Smite/Holy Nova can remain visible-but-locked unless picked up during this slice.                       |
| Summoner | Void Familiar, Riftling Swarm, basic minion damage/health passives | Riftling Swarm should be a true multi-minion ability immediately; necromancy/corpses remain later.      |
| Synergy  | Cold Star Pact (`6200`)                                            | First hybrid proof: Frost/Chill plus Void minions.                                                      |

### First-Slice Implementation Order

1. Implement the static blueprint data, status tags, coordinates, and visible-but-locked node behavior.
2. Add prerequisite expression support for `all(...)` and `any(...)`, or internally lower `any(...)` bridge nodes into equivalent temporary data.
3. Wire talent ability modifiers into the same aggregation path used by item ability mods.
4. Build the four basic route slices: Warrior basic, Mage basic, Healer basic, Summoner basic.
5. Implement lightweight Chill and minion-hit hooks required for Cold Star Pact. Chill may start as a simple slow/debuff, but its data should leave room for later Brittle and Freeze interactions.
6. Make Cold Star Pact the first playable Synergy node; keep all other Synergy nodes visible-but-locked.
7. Add keystone plumbing for active keystones, even if keystone behavior remains mostly WIP at first.

## Resolved Decisions From Draft 1 Review

- WIP nodes are visible but locked.
- Hybrid builds should be openly supported. Intra-route bridge nodes can use either-lane prerequisites; cross-route payoff should use explicit Synergy nodes with one anchor from each participating route.
- The document owns explicit draft layout coordinates. In-game layout can be tuned later from these anchors.
- Keystone drawbacks are capstone-only. Mid-route keystones should be clean upside.
- A fifth route placeholder is reserved now, but it is not part of the current playable tree.
- Fifth route placeholder ids use the compact `500..599` range for now.
- The blueprint uses prerequisite expression syntax: `all(...)` for required sets and `any(...)` for either-lane bridge access.
- The first playable slice includes Warrior, Mage, Healer, Summoner, and Cold Star Pact as the first Synergy node.
- Long-diagonal Synergy nodes sit on an outer constellation ring.
- First-slice Riftling Swarm is a true multi-minion ability, not a single-summon stand-in.
- First-slice Chill can be lightweight for now, but should be documented and structured as the seed for future Brittle and Freeze mechanics.
- Route direction/layout judgement is deferred until after migrating the blueprint into the in-game talent tree.

## Open Questions For Next Pass

- After the blueprint is visible in-game, do the route directions feel right visually: Warrior east, Mage north, Healer south, Summoner west, fifth route northeast?
