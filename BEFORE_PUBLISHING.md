# Rift — Before Publishing

A living, opinionated checklist of everything that needs to land
before a public launch. Scope is **first-paid-or-public-beta
release**, not "perfect game forever". Cross items off as they
land. Add new items the moment they're discovered — better here
than in someone's head.

Sections are ordered by **risk** (a launch is impossible without
the top sections, merely embarrassing without the bottom ones).
Inside a section, ordering is rough priority.

---

## Launch scope targets

The numerical "vertical slice" we're aiming at for v1. Detail
checklists below the fold; this is the headline shape so we
know when we're done adding content vs. starting to polish.

| Pillar             | Target for v1                                   |
| ------------------ | ----------------------------------------------- |
| Biomes             | **1**, fully polished end-to-end                |
| Boss fights        | **1**, multi-phase, memorable                   |
| Enemy archetypes   | **3–4**, each with a unique telegraphed ability |
| Abilities (player) | **12–20** total, every one excellent            |
| Loot loop          | Strong: meaningful upgrades on a clear cadence  |
| Extraction tension | Strong: real risk/reward to leaving with loot   |
| Atmosphere         | Excellent: audio + lighting + VFX cohere        |

Anything that doesn't serve one of these pillars is out of
scope for v1 and goes in a "post-launch" pile (not in this
file).

---

## 0. Showstoppers (no launch without these)

### Account & auth

The plan is **Steam-only** for the published build — no
passwords, no email recovery, no in-house credential store.
Steam's session ticket is the identity; the dev HMAC path
(see below) exists only for local development.

- [x] Wire-protocol split for authenticated handshake.
      `Hello { protocol_version, auth_ticket: Vec<u8> }` →
      `Authenticated { your_client_id, display_name, roster }`
      → `EnterWorld { character_name, class_id, gender }` →
      `Welcome`. The ticket is opaque on the wire: the dev
      issuer produces an HMAC envelope (schema in
      `rift-net::auth_dev`); the Steam issuer produces
      `[steam_id_u64_LE | raw_GetAuthSessionTicket_bytes]`.
      `Hello.auth_ticket: Vec<u8>`, `PROTOCOL_VERSION = 5`.
- [x] Server-side auth resolver (`rift-server::auth`) with
      `AccountKey::{ Steam(u64), Dev(String) }`, an
      `AuthConfig::from_env()` reader, and a `Verifier`
      enum picked once at startup (`Steam` when built with
      `--features steam-auth`, `Dev` otherwise). 10 dev-auth
      unit tests passing. Loud WARN log when
      `RIFT_DEV_AUTH_KEY` is set; ERROR on a malformed key.
- [x] Client-side signer (`rift-client::auth`) with
      `Signer::{ Dev, Steam }`, env-driven config, and a
      `mint()` that produces a fresh credential per `Hello`.
      Dev path uses a random `dev-XXXXXX` identity per
      process or a stable `RIFT_DEV_USER` override. Connect
      is hard-refused without a working signer.
- [x] Persistence migration `20260510000000_account_key.sql`
      adds `accounts.account_key` (`"steam:..."` /
      `"dev:..."`) + `accounts.display_name`, backfilled
      additively. Promoting `account_key` to NOT NULL UNIQUE
      and rekeying lookups off `name` is a follow-up
      migration.
- [x] Dev key bootstrap. `scripts/rift.{sh,ps1}` auto-generate
      a 32-byte `RIFT_DEV_AUTH_KEY` into `.env.dev-auth`
      (gitignored) on first run so client + server agree
      locally without manual setup. `DEPLOYMENT.md` documents
      the env vars and the production rule (never set the
      dev key, build with `--features steam-auth`).
- [x] **Real Steam ticket verification.**
      `rift-server::auth::steam::SteamVerifier` now owns a
      Steamworks Game Server SDK `Server` handle
      (`ServerMode::Authentication`) with `set_product` +
      `set_game_description` + `log_on_anonymous` so ticket
      validation callbacks actually fire. A 20 Hz callback-pump
      thread (RAII-joined on drop) drives
      `SteamServersConnected` / `SteamServerConnectFailure` /
      `SteamServersDisconnected` to a shared
      `AtomicBool`, and `verify()` blocks briefly on that
      flag so the first `Hello` doesn't race the anonymous
      logon. `ValidateAuthTicketResponse` results route
      through `Arc<Mutex<HashMap<u64, mpsc::SyncSender>>>`
      keyed by `SteamID`; `verify()` uses `recv_timeout(5s)`
      and always calls `end_authentication_session` on the
      way out. Family-Sharing sessions are logged but
      allowed. Replaces the rejecting Web-API stub. Built
      with `--features steam-auth`; `STEAM_WEBAPI_KEY` is no
      longer used (Web API was the wrong tier for a
      published game-server build).
- [x] **Real Steam ticket minting on the client.**
      `rift-client::auth::steam::SteamSigner::from_env`
      runs `steamworks::Client::init_app(RIFT_STEAM_APPID,
    default 480 = Spacewar)`, writes `steam_appid.txt`,
      and spawns a matching 20 Hz pump. `mint()` calls
      `authentication_session_ticket(NetworkingIdentity::new())`
      and ships the bytes as
      `[steam_id_u64_LE | raw_ticket]` inside the opaque
      `Hello.auth_ticket: Vec<u8>` (PROTOCOL_VERSION 5).
      The `steam-auth` cargo feature is now load-bearing,
      not scaffolding.
- [x] **Lock down dev auth in production builds.**
      `AuthConfig::from_env` refuses to enable the dev path
      when built with `--features steam-auth`: if
      `RIFT_DEV_AUTH_KEY` is also set we log a loud `WARN`
      and use the Steam verifier anyway. A production box
      built with the feature flag cannot accidentally accept
      HMAC creds.
- [x] **Switch persistence reads off `accounts.name` and
      onto `account_key`.** Migration
      `20260511000000_account_key_required.sql` promotes
      `account_key` to `NOT NULL UNIQUE`, drops the legacy
      `accounts.name` UNIQUE + NOT NULL constraints, and the
      `rift-persistence` API now keys
      `load_or_create_blocking` / `list_account_characters_blocking`
      on the issuer-tagged storage form (`"dev:..."` /
      `"steam:..."`) plus a separate `display_name`. Server
      threads both through `ClientSession.account_name` +
      `account_display_name` and `AccountKey::legacy_account_name()`
      is gone. Dropping the now-vestigial `accounts.name`
      column entirely is a follow-up cleanup migration.
- [ ] Per-account session tokens / reconnect path. Once the
      Steam ticket is verified the server should mint a
      short-lived session token the client can present on
      reconnect within a grace window without a fresh ticket
      round-trip.
- [ ] Rate-limit `Hello` per source IP to defang ticket
      replay / credential stuffing. The HMAC dev path
      already enforces a 60 s replay window
      (`DEV_AUTH_REPLAY_WINDOW_SECS`); the Steam path needs
      its own anti-replay (cache `(steam_id, ticket_hash)`
      with a TTL).
- [ ] Account deletion / right-to-erasure surface. With
      Steam as the identity provider this is mostly "delete
      the row keyed on `steam:<id>`", but it needs a
      reachable UX path (in-game button or a documented
      contact channel) — see also section 2.

### Server hardening

- [x] Validate every wire field server-side. `character_name`
      length, UTF-8 sanity, ability ids, slot indices,
      inventory positions — anything currently trusted because
      "the client wouldn't send that" must be re-validated.
      _(First pass: name/class_id length+control-char gate on
      Hello, length cap on roster lookup, length cap on party
      invite/kick/promote names. Inventory/loadout indices and
      ability ids are already option-checked by the underlying
      sim ops; chat already trims+rate-limits. Movement input
      validation tracked separately below.)_
- [x] Speedhack / teleport detection on `ClientMsg` movement
      input (the comment in `messages.rs:603` already flags
      this). Reject inputs that exceed the
      class's max move speed by more than a small tolerance.
      _(First pass: `Sim::ingest_input` zeroes non-finite
      `move_dir` / `aim_dir` / `cast_target` axes and drops
      the whole input if either direction's magnitude exceeds
      ~2× unit length. The kinematic itself already clamps
      `move_dir` to unit length and applies a fixed
      `PLAYER_SPEED`, so a hostile client can't translate a
      large `move_dir` into faster movement; this guard
      closes the NaN-corruption hole and the obvious DoS
      values. A real teleport-distance check between
      consecutive server-authoritative positions per tick is
      still TODO.)_
- [ ] Cap per-client message rate (renet has channel
      back-pressure; tune it and add a hard kick on sustained
      flooding).
- [ ] Inventory / equipment ops must re-check ownership and
      stack size on the server. Drag-drop is currently driven
      by the client — assume malicious inputs.
- [ ] Loot pickup must verify proximity + claim ownership
      window server-side, not trust a `PickUpLoot` packet.
- [x] Crash-safe `Sim` step: a panic in one player's frame
      shouldn't take the whole server down. Wrap the per-tick
      loop in `catch_unwind` (or move per-floor sims into
      isolated tasks so one floor crashing only drops that
      floor's players).
      _(Done: `simulate_one_tick` wraps `hub.step` and each
      `instance.sim.step` in `catch_unwind` individually. A
      panic in one floor logs and is contained; the rest of
      the server keeps ticking. Booting the floor's players
      back to the hub on a poisoned tick is still TODO.)_

### Persistence safety

- [ ] Database backups. Automated nightly + manual
      pre-deploy snapshot. Document restore procedure.
- [ ] Migrations are forward-only and tested on a copy of
      production before release. `sqlx migrate` with a
      pre-deploy dry-run.
- [ ] Inventory / stash writes are transactional. Today a
      crash mid-write could shred a player's stash.
- [ ] Soft-delete characters instead of hard-delete so a
      misclick (or a bug) is recoverable.
- [ ] Item-dupe audit: write a query that flags any item
      `id` that appears in more than one inventory or stash
      row simultaneously. Run weekly.

### Networking

- [ ] Replace `ServerAuthentication::Unsecure` /
      `ClientAuthentication::Unsecure` in `rift-net::transport`
      with netcode.io's `Secure` mode. Requires an auth service
      that signs connect tokens with a private key the game
      server also holds; load that key from an env var
      (`RIFT_CONNECT_TOKEN_KEY`, 32 bytes hex), never check it
      into source, and document rotation in `DEPLOYMENT.md`.
      Until this lands the server trusts whatever `client_id`
      the binary claims, which means anyone can impersonate
      anyone else's account.
- [x] Set the netcode protocol version to a release value
      and treat protocol-mismatch on `Hello` as a hard reject
      with a user-visible "client out of date" message (today
      the client just hangs).

---

## 1. Reliability & ops

- [x] Structured logging with severity levels (already on
      `env_logger`; add request id / client id to every per-
      client log line so multi-player issues are debuggable).
      _(First pass: `Server::client_tag(from)` returns a
      `[cid=N char=Foo]` Display tag. Wired into the highest-
      value sites — login Reject lines, Hello info, persistence
      load warnings, connect/disconnect events, final-save
      drops. Lower-traffic `log::debug!` lines still use
      `{from:?}` since they're only on at -vv anyway.)_
- [ ] Server-side metrics endpoint: tick time histogram,
      per-floor sim cost, connected client count, dropped
      packet rate, snapshot size. Even a Prometheus scrape on
      a sidecar port is enough.
- [ ] Crash dumps / stack traces from clients ship to a
      collection endpoint so we hear about reproducible
      crashes without a player having to email us.
- [ ] Health check endpoint for fly.io's TCP probe (we're UDP
      so the probe needs a separate small TCP listener that
      reports "sim is ticking" not just "process is up").
- [ ] Document the runbook: "server is down" / "DB is full" /
      "all clients disconnect at once" — first three things to
      check, where the dashboards live, how to roll back.
- [x] Server graceful-shutdown signal handler that flushes
      pending DB writes before exit (today SIGTERM may drop
      the in-flight inventory write).
      _(Done via `ctrlc` crate: SIGINT / SIGTERM / Windows
      console-close set an `AtomicBool` the main loop polls;
      on shutdown we run a final `auto_save_all` and call
      `PersistenceHandle::shutdown_blocking` so the worker
      drains its mailbox before the process exits.)_

---

## 2. Privacy / legal / store-front

- [ ] Privacy policy. Even a minimal "we store your account
      name and play data on a server in <region>; we use
      <provider> for hosting" page is required by most app
      stores and arguably by GDPR / CCPA.
- [ ] Terms of service / EULA. Standard boilerplate is fine
      to start.
- [ ] Account deletion path. GDPR right-to-erasure means a
      player must be able to request their account + characters
      removed.
- [x] Open-source license audit. Every dep in `Cargo.lock`
      that requires attribution must appear in an in-game
      "Licenses" screen or a `THIRD_PARTY.txt` next to the
      binary. `cargo about` automates this.
      _(Done at the bundle level: `about.toml` + `about.hbs`
      drive `scripts/gen-third-party.sh` / `.ps1`, which
      `package-client.{sh,ps1}` invokes to stage
      `THIRD_PARTY.txt` (≈7 k lines, every transitive dep)
      next to the release binary. In-game "Licenses" screen
      that surfaces the same content from inside the client
      is still TODO.)_
- [ ] Asset license audit. Every model in `assets/models/`
      and texture in `assets/textures/` needs documented
      provenance — store-fronts will ask. Track in
      `assets/CREDITS.md`.
- [ ] Trademark / name search on "Rift" + any class names.
      Pick a final published title that isn't already a
      registered game.

---

## 3. Game content & feel (player-facing minimum)

### Content gates

- [ ] Tutorial / first-run flow. Today the player drops into
      the hub with no on-screen guidance. At minimum: a tooltip
      sequence on first login covering move, attack, ability,
      open inventory, talk to portal.
- [ ] At least one full progression arc playable end-to-end
      without dev knowledge: hub → first rift → boss → loot
      → equip → next rift. Bug-bash this loop with a fresh
      player.
- [ ] Death and respawn feel finished. Right now the ghost-mode
      transition exists but the UX around "what just happened"
      and "here's how to get back" is sparse.

### Combat polish

- [x] Server-side LOS gating for AoE / beam / channel damage
      (Frost Ray no longer hits through walls).
- [x] Enemy nav routes around walls + props with wall-proximity
      cost so they don't shoulder-scrape geometry.
- [x] Combat-meter `damage taken` per-ability breakdown — landed.
      Enemy attacks are tagged with stable wire ability ids and the
      TAKEN tab now mirrors the DMG / HPS rollup.
- [ ] Audit every ability's tooltip and damage numbers against
      `CharacterStats::compute`. Drift between displayed and
      actual damage is a top-3 player complaint generator.
- [ ] Boss fight pacing pass. One full clear should feel
      cinematic, not stretched.

### Multiplayer feel

- [ ] Generic player targeting. Today only heal abilities can
      target another player. Extend the targeting system to
      arbitrary player picks (click a player, click a
      nameplate) so the HUD can show the target's portrait /
      name / class / level and route a context menu off it.
  - [ ] Target portrait + nameplate slot in the HUD,
        symmetrical with the local player frame.
  - [ ] Right-click context menu on a targeted player and on
        their nameplate: **Invite** (party / group), **Whisper**
        for now; **Inspect**, **Trade**, **Report** as the
        underlying systems land.
  - [ ] Wire `/whisper <name> <text>` and `/invite <name>`
        slash-command equivalents so the same actions work
        without a cursor.
- [ ] Player nameplates with health bar above the head, party
      colour-coded so allies read at a glance.
- [ ] Party-vs-stranger distinction (today every other player
      is just "another player"). Even a join-code or
      friend-list flow lets two people deliberately play
      together.
- [ ] Loot ownership / allocation rules across players in the
      same rift. Personal-loot vs free-for-all is a design
      decision the wire protocol can already support, but
      neither is enforced today.
- [ ] Player-to-player trade flow (or explicit "no trading"
      decision) before launch. Both have legal / economy
      implications, can't ship without picking one.
- [ ] Chat: at minimum a local + party + system channel with
      profanity filter toggle.
- [ ] Reconnect flow: a dropped client should be able to
      rejoin its in-progress rift within a grace window
      without losing the run.

### Itemisation

- [x] Weapon icons wired in. The four weapon `BaseItem::icon`
      fields point at `loot/Weapons/1..4` (`assets/icons/loot/Weapons/`).
      Inventory now shows real glyphs for staves / swords / daggers /
      wands instead of the letter fallback.
- [ ] One full pass on affix tiers + weights so the loot
      curve doesn't have obvious dead zones.
- [ ] Stash UI sort + search. Long-term play makes "find that
      one helmet" miserable without it.

### Equipment models (visual loot)

- [ ] Real 3D models for every equipped item slot. Today the
      character body shows a base outfit regardless of what's
      worn — equipment doesn't render. We need:
  - [ ] One mesh per `BaseItem` (or at minimum per visual
        family — e.g. all "light helms" share a model, all
        "plate helms" share another) for both male and female
        rigs.
  - [ ] Slot attach points on the character rig (head, chest,
        shoulders L/R, hands L/R, legs, boots, weapon main /
        off) wired so the renderer can draw the equipped
        mesh in the right spot every frame.
  - [ ] `BaseItem::models` (currently always `None`) populated
        with the per-slot mesh path; renderer reads it from
        the equipment snapshot.
  - [ ] Tint / dye support if affixes affect colour, or just
        a stable per-rarity edge tint.
- [ ] Weapon-in-hand pose adjustments per `WeaponKind` (staff
      held two-handed, dagger reverse-grip, wand pistol-grip)
      so the swing animations read correctly.
- [ ] LOD pass on the per-character mesh stack so a full party
      doesn't tank framerate when every player is decked out.

### Balancing

- [ ] End-to-end damage / EHP balance pass against rift
      scaling. The affix system, `CharacterStats::compute`,
      and rift difficulty curve all exist independently \u2014
      they have not been balanced together. Concretely:
  - [ ] Define target TTK (time-to-kill) curves for the
        baseline class at each rift tier: trash mob, elite,
        boss. Document the target in this file or a sibling
        balance doc.
  - [ ] Define target survival window for the player at the
        same tiers (how long before a rested player dies if
        they stand still in a pack).
  - [ ] Author a benchmark scenario (fixed seed, fixed
        equipment) the server can run headless and dump
        damage / EHP numbers from. Re-run after every balance
        change so regressions are caught.
  - [ ] Pass on enemy HP, damage, and pack composition per
        `FloorConfig` tier so the curves above actually hold.
  - [ ] Pass on affix value ranges per ilvl so a max-rolled
        item is exciting but not mandatory.
  - [ ] Pass on ability base damage so each ability is a
        viable build choice at endgame, not a strict ranking.
  - [ ] Loot-drop rate + currency-drop rate tuned so the
        upgrade cadence matches the difficulty curve (player
        should feel a power bump every ~N rifts).

### Ability system overhaul

- [ ] Author one full set of abilities per archetype, each
      complete with icon + SFX + VFX + animation + tooltip:
  - [ ] Physical / melee set (gap-close, AoE cleave,
        execute, defensive cooldown, mobility).
  - [ ] Caster / DPS set (hard-hitting nuke, AoE, channel,
        utility / control, mobility).
  - [ ] Healer / support set (single-target heal, AoE / HoT
        heal, party buff, cleanse / dispel, defensive
        cooldown for an ally).
  - [ ] Tank / utility set (taunt-equivalent threat tool,
        damage-reduction cooldown, party utility, gap-close
        or charge, AoE pull / control).
- [ ] Custom ability icons per ability (today most slots use
      the engine's stock icon set or class-letter fallbacks).
      Drop into `assets/icons/abilities/` and reference via
      `Ability::icon`.
- [ ] Per-ability SFX coverage check: cast start, mid-channel
      loop where applicable, impact, and a unique
      cancel / interrupt sound.
- [ ] Per-ability VFX pass: cast telegraph at the player,
      projectile / beam / AoE shape, on-hit reaction. Reuse
      the declarative `Effect = Vec<Layer>` system so each
      ability is a `presets/.../*.rs` file.
- [ ] Per-ability animation hookup: cast clip blends in
      cleanly with locomotion, animation length matches
      cast time, and channel abilities loop their cast
      pose without snapping.
- [ ] Ability registry pass once content is in: every
      `Ability` entry has a stable id, tooltip, scaling tag,
      resource cost, cooldown, and the four assets above.
      No half-wired stubs at launch.

### Enemies & boss

- [ ] More enemy types beyond Brute / Stalker / Caster.
      Target a roster diverse enough that no two adjacent
      rifts feel like the same encounter set:
  - [ ] One ranged kiter that maintains distance.
  - [ ] One charger that telegraphs a long line dash.
  - [ ] One support / buffer that empowers nearby allies
        (cleanse priority target).
  - [ ] One summoner that periodically spawns minions
        (kill-the-summoner gameplay).
  - [ ] One stationary hazard / turret enemy.
  - [ ] One self-destruct / suicide bomber that forces
        movement.
- [ ] Per-enemy unique ability, not just stat-padded melee.
      Each role above ships with at least one telegraphed
      special (wind-up, animation, VFX, SFX) so the player
      can read and react to it.
- [ ] Elite modifier rebalance / refresh now that enemy
      roster is wider \u2014 some current modifiers (e.g.
      Vampiric) overlap awkwardly with new mechanics.
- [ ] Boss fight overhaul. Today's boss is a stat-bag with
      a few attacks; target a multi-phase fight that's
      genuinely challenging and memorable:
  - [ ] Three distinct phases triggered at HP thresholds,
        each with its own ability set and VFX language.
  - [ ] One mechanic per phase that punishes standing
        still (zone denial), one that punishes stacking
        (split / spread), one that requires interrupting.
  - [ ] Add-spawn phase tied to a summoner-style mechanic
        so the kill-priority decision matters.
  - [ ] Telegraph language: every boss ability has a
        readable wind-up the player can react to. No
        instant-cast big damage.
  - [ ] Enrage timer / soft-enrage so a 30-minute attrition
        clear isn't viable.
  - [ ] Distinct boss SFX and a music track that escalates
        per phase.
  - [ ] Loot table tuned to the boss's slot \u2014 dedicated
        guaranteed-rarity drop on first clear of the
        floor's tier per session.

### Atmosphere — lighting progression by depth

The single highest-ROI atmosphere lever we have. The renderer
already does directional + point-light shadows, post-processing,
and volumetrics; lean into them with a deliberate per-depth
lighting theme so descending the rift _feels_ different, not
just numerically harder.

- [ ] Per-floor `LightingTheme` data driven off rift depth.
      Every theme bundles: ambient colour + intensity, directional
      light colour / intensity / angle, fog colour + density,
      bloom intensity / threshold, vignette strength,
      torch / point-light tint, optional volumetric haze
      colour + density, post-processing tint / contrast.
- [ ] Three reference themes for v1, with smooth blends in
      between as floor depth ramps:
  - [ ] **Early rifts** — warm torchlight, readable shadows,
        grounded fantasy dungeon. Saturated orange points,
        neutral ambient, low fog.
  - [ ] **Mid rifts** — colder palette, blue fog, less fire,
        magical ambience. Cyan / desaturated points,
        cooler ambient, mid fog density.
  - [ ] **Deep rifts** — impossible lighting: red volumetric
        haze, moving shadows, flickering darkness, void glow,
        non-physical bloom. Push bloom past photorealism,
        let post tint deviate from neutral.
- [ ] Smooth blend between themes as floor index increases so
      the player notices the shift over a run, not a hard
      step every N floors.
- [ ] Per-theme audio cues (low ambient drone deepens with
      depth, ambient SFX density rises) wired to the same
      depth curve so audio + lighting move together.
- [ ] Boss-room override: the boss arena ignores the floor's
      ambient theme and uses the boss-fight palette (escalating
      per phase, see `Enemies & boss`).

### Audio

- [ ] At least one music track per zone (hub, rift, boss).
- [ ] Combat SFX coverage check: every player ability has a
      cast + impact sound; every enemy attack has a wind-up
      tell + impact.
- [ ] Master / music / sfx volume sliders persist across
      sessions.

---

## 4. UX / accessibility / polish

### Visual UI overhaul

- [ ] HUD visual pass. Today's HUD is functional but reads as
      programmer-art: flat panels, default-feel borders, no
      visual hierarchy beyond the action bar. Goals:
  - [ ] Concrete art direction doc: palette, panel chrome,
        iconography style, typography scale. Pick one and
        stick to it across every screen.
  - [ ] Consistent panel frame across HUD / inventory /
        character / talents / stash / settings (today each
        screen draws its own frame from scratch).
  - [ ] Action-bar visual upgrade: cooldown sweep readability,
        resource-cost pip on each slot, charge-stack badges.
  - [ ] Health / resource / XP bar art pass with tick-mark
        notches at meaningful values.
  - [ ] Damage / heal / loot text has a final visual pass
        (font, outline, drift curve) so combat reads at a
        glance.
  - [ ] Tooltip system: consistent header / body / affix
        block layout, rarity-coloured frames, comparison
        view when hovering an item with one already equipped.
- [ ] Title screen / character-select art pass. First
      impression carries disproportionate weight.
- [ ] Loading-screen visuals (today it's a blank screen with a
      progress bar).
- [ ] Cursor: replace OS arrow with a custom in-game cursor
      that signals hover state (clickable / draggable /
      attack target).
- [ ] Death and level-up flourishes (full-screen tint, vignette,
      one-line message). They sell impact at zero gameplay cost.

### Settings & accessibility

- [ ] Resolution + windowed/fullscreen toggles in a Settings
      menu (today's launch always opens full-window borderless
      on the primary monitor — fine for dev, not for shipping).
- [ ] Key rebinding. Even a fixed list of "movement / abilities
      / UI" actions is enough.
- [ ] Mouse-cursor sensitivity slider; raw mouse vs OS mouse
      toggle for low-input-lag preference.
- [ ] Colour-blind safe palette pass on damage text, rarity
      colours, and minimap markers.
- [ ] Subtitles toggle even if there's no VO yet — sets the
      foundation for later.
- [ ] In-game "Report a bug" button that captures the last 30 s
      of log + a screenshot and posts to a collection endpoint.

---

## 5. Performance

- [ ] Profile a 4-player full-rift fight on the lowest
      target-spec GPU. Identify the worst frame and either
      fix it or document the spec floor.
- [ ] Server tick budget: 200 enemies + 8 players + 20 AoE
      zones must stay under 16 ms tick at the chosen tick rate.
- [ ] Client memory ceiling. Do a 1-hour soak, watch for any
      growth that suggests a leak.
- [ ] First-launch asset load time. Streaming icon load is
      done; check the same path for models / textures and add
      a loading screen that doesn't lie about progress.

---

## 6. Build, distribution, updates

- [ ] CI that builds Linux + Windows release binaries and
      uploads them as artifacts on every tagged commit.
- [ ] Code-signing for the Windows binary so SmartScreen
      doesn't scare players off. (Mac too if we ship there.)
- [ ] Auto-update path. Even a manual "your client is out of
      date, download here" link triggered by the connect-time
      protocol-version check is enough for v1.
- [ ] Server image is reproducible: pinned base image, locked
      Cargo.lock checked in, build args documented in
      `Dockerfile.server`.
- [ ] Tag a `v0.1.0-launch` git tag and a matching docker
      image tag at the moment we go live, so a rollback target
      exists.

---

## 7. Community / support

- [ ] Public Discord / forum / subreddit before launch day.
      Players will look for it; if there's nothing, every
      issue lands as a 1-star review.
- [ ] FAQ doc covering "how do I connect", "how do I move
      my save", "what specs do I need".
- [ ] Patch-notes habit. First patch after launch sets the
      tone — write notes even for tiny changes.

---

## 8. Documentation hygiene (low risk but easy wins)

- [x] Resolve the in-source dangling-doc `TODO` markers in
      `rift-net/src/lib.rs` and `rift-engine/src/assets.rs`.
      Pointed-at docs (`docs/multiplayer.md`, `notes/assets.md`)
      didn't exist and were never going to; pointers removed.
      The `(TODO: auth)` marker in `messages.rs` stays — it
      tracks a real launch blocker (section 0).
- [x] `ARCHITECTURE.md` rewritten as a one-page "how the
      crates fit together" reference reflecting the actual
      current state (forward Vulkan, server-authoritative
      sim, the workspace dependency graph). The phased
      "render a triangle next" plan is gone.

---

## How to use this list

- One item, one line, no nuance hidden in prose. If something
  needs nuance, link to the issue / doc that holds it.
- A box ticked here means **shippable**, not "I started it".
  Half-done work stays unchecked.
- New showstoppers go in section 0 the moment they're found,
  even if it pushes the launch date.
