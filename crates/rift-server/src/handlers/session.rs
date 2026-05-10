//! Login / session lifecycle handlers. Owns the full `Hello`
//! flow split into named phases so each step is short and
//! independently testable.

use rift_net::{AuthCredential, Channel, ClientId, Gender, NetId, ServerMsg, PROTOCOL_VERSION};

use super::{item_to_blob, place_at_slot_index};
use crate::Server;

/// Maximum number of characters (Unicode scalars) accepted in a
/// player-supplied account or character name. Mirrors the input
/// cap the client's character-select screen already enforces
/// (`text_field(.., 18, ..)` in `character_select.rs`); we
/// re-check here because the wire is server-authoritative — a
/// hand-rolled or modded client can send anything.
const MAX_NAME_CHARS: usize = 18;

/// Maximum length of a `class_id` string the client may declare
/// at login. Longer than the longest content table id we ever
/// expect (`necromancer` is the current max at 11) but short
/// enough that a hostile client can't pad memory by sending
/// megabytes of `class_id`.
const MAX_CLASS_ID_CHARS: usize = 32;

/// Trim whitespace, then validate that `value` looks like a
/// reasonable player-supplied display name: non-empty after
/// trim, at most `max_chars` Unicode scalars long, and free of
/// control characters (which would otherwise mangle log lines,
/// chat broadcasts, and the character-select UI). Returns the
/// trimmed name on success, or an `Err` whose payload is a
/// short user-facing reason for the rejection.
fn validate_name(value: &str, max_chars: usize, what: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{what} cannot be empty."));
    }
    let len = trimmed.chars().count();
    if len > max_chars {
        return Err(format!(
            "{what} is too long ({len} characters, maximum is {max_chars})."
        ));
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Err(format!("{what} contains invalid control characters."));
    }
    Ok(trimmed.to_string())
}

impl Server {
    /// Handle a fresh client's `Hello`: validate protocol, resolve
    /// the auth credential into an account identity, look up that
    /// account's character roster, and reply with
    /// [`ServerMsg::Authenticated`]. Spawning into the simulation
    /// is deferred to [`Self::handle_enter_world`] (driven by the
    /// follow-up [`rift_net::ClientMsg::EnterWorld`]) so the
    /// player can browse and pick from the roster post-auth.
    ///
    /// **NOTE (Phase 1):** Credential resolution is currently a
    /// pass-through — `Dev` identities are accepted as-is and
    /// `Steam` is rejected with a "not yet wired" reason. Real
    /// HMAC verification + Steam Web API calls land in Phase 2.
    pub(crate) fn handle_hello(
        &mut self,
        from: ClientId,
        protocol_version: u16,
        auth: AuthCredential,
    ) {
        if protocol_version != PROTOCOL_VERSION {
            // User-visible string: the client surfaces this verbatim
            // and exits, so it has to read like an end-user message,
            // not a debug log line. Direction of mismatch picks the
            // right "your client is out of date" / "server is out
            // of date" wording so a player knows whether to update
            // or to wait for the operator.
            let direction = if protocol_version < PROTOCOL_VERSION {
                "Your client is out of date. Please update to the latest version."
            } else {
                "The server is out of date. Please wait for the operator to update it."
            };
            let reason = format!(
                "{direction} (protocol: client v{protocol_version}, server v{PROTOCOL_VERSION})"
            );
            log::warn!("{} rejecting Hello: {reason}", self.client_tag(from));
            self.send_to(from, Channel::Control, &ServerMsg::Reject { reason });
            return;
        }

        // Phase 2 credential resolution. `auth::resolve` runs
        // the issuer-appropriate verification (HMAC-SHA256 for
        // Dev, Steam Web API for Steam — currently a stub) and
        // returns an issuer-tagged `AccountKey`. Phase 3 will
        // promote this onto a background task so a slow Steam
        // round-trip can't stall the sim loop.
        let account_key = match crate::auth::resolve(&self.auth_config, &auth) {
            Ok(k) => k,
            Err(err) => {
                let reason = err.user_message();
                log::warn!(
                    "{} rejecting Hello: auth failed ({err:?})",
                    self.client_tag(from)
                );
                self.send_to(from, Channel::Control, &ServerMsg::Reject { reason });
                return;
            }
        };

        // Persistence is still keyed on the legacy bare
        // `account_name` string; Phase 4's migration moves it
        // onto the issuer-tagged storage form. Until then we
        // pass the legacy form into `lookup_roster` /
        // `load_player_state` so existing rows keep loading.
        let account_name = account_key.legacy_account_name();

        // Re-validate the resolved identity before it touches
        // persistence or any broadcast. Even with auth in place
        // a hostile or buggy client can ship pathological
        // identity strings — empty, megabyte-long, or laced
        // with control characters that would mangle log lines
        // and chat fan-out.
        let account_name = match validate_name(&account_name, MAX_NAME_CHARS, "Account name") {
            Ok(s) => s,
            Err(reason) => {
                log::warn!("{} rejecting Hello: {reason}", self.client_tag(from));
                self.send_to(from, Channel::Control, &ServerMsg::Reject { reason });
                return;
            }
        };

        log::info!(
            "{} Hello: authenticated as account={account_name:?}",
            self.client_tag(from)
        );

        // Stash the resolved identity on the session so the
        // follow-up `EnterWorld` can hydrate the chosen
        // character without re-running auth.
        if let Some(s) = self.sessions.get_mut(from) {
            s.account_name = Some(account_name.clone());
        }

        let roster = self.lookup_roster(&account_name);
        let display_name = account_name.clone();
        self.send_to(
            from,
            Channel::Control,
            &ServerMsg::Authenticated {
                your_client_id: from,
                display_name,
                roster,
            },
        );
    }

    /// Handle an authenticated client's `EnterWorld`: validate
    /// the requested character, hydrate persistent state, spawn
    /// into the live hub sim, and broadcast the join. The
    /// heaviest of the dispatch arms; kept as its own method so
    /// [`Self::handle_client_msg`] reads as a flat dispatch
    /// table.
    ///
    /// Refuses with `Reject` if the session has not completed
    /// the `Hello → Authenticated` round-trip yet (no
    /// `account_name` stashed). That can only happen with a
    /// custom or out-of-date client, since the stock client
    /// gates the EnterWorld send on a received `Authenticated`.
    pub(crate) fn handle_enter_world(
        &mut self,
        from: ClientId,
        character_name: String,
        class_id: String,
        gender: Gender,
    ) {
        let Some(account_name) = self.sessions.get(from).and_then(|s| s.account_name.clone())
        else {
            let reason = "Cannot enter world: client has not authenticated.".to_string();
            log::warn!("{} rejecting EnterWorld: {reason}", self.client_tag(from));
            self.send_to(from, Channel::Control, &ServerMsg::Reject { reason });
            return;
        };

        // Re-validate every client-supplied string before we let
        // it touch persistence, the sim, or any broadcast. The
        // client UI already caps these at 18 chars, but the wire
        // is server-authoritative — a modded client can send
        // anything, including empty strings, megabyte payloads,
        // or embedded control characters that would mangle log
        // lines and chat fan-out. Failures send a Reject the
        // client can surface verbatim and exit cleanly.
        let character_name = match validate_name(&character_name, MAX_NAME_CHARS, "Character name")
        {
            Ok(s) => s,
            Err(reason) => {
                log::warn!("{} rejecting EnterWorld: {reason}", self.client_tag(from));
                self.send_to(from, Channel::Control, &ServerMsg::Reject { reason });
                return;
            }
        };
        let class_id = match validate_name(&class_id, MAX_CLASS_ID_CHARS, "Class id") {
            Ok(s) => s,
            Err(reason) => {
                log::warn!("{} rejecting EnterWorld: {reason}", self.client_tag(from));
                self.send_to(from, Channel::Control, &ServerMsg::Reject { reason });
                return;
            }
        };

        log::info!(
            "{} EnterWorld: account={account_name:?} name={character_name:?} class={class_id:?} gender={gender:?}",
            self.client_tag(from)
        );

        // Phased login: load → spawn → hydrate → reply → announce.
        // Each step is a small named method so the dispatch is
        // legible and individual stages are unit-testable in
        // isolation later if we want.
        self.load_player_state(from, &account_name, &character_name, &class_id, gender);
        let net_id = self.spawn_player_session(from, &class_id);
        let loaded_bag = self.hydrate_player_state(from);
        self.send_initial_packets(from, net_id, &loaded_bag);
        self.announce_join(from, net_id, character_name, class_id, gender);
    }

    /// Resolve the persisted character row (or fall back to an
    /// in-memory record) and stash all profile fields on the
    /// session. Mutating `account_name` etc. happens here so the
    /// rest of the login flow can read them off the session
    /// without having to thread parameters around.
    fn load_player_state(
        &mut self,
        from: ClientId,
        account_name: &str,
        character_name: &str,
        class_id: &str,
        gender: Gender,
    ) {
        // Block here on purpose: the load happens once per session
        // and we need level/xp before the world spawn. Falls back
        // to an in-memory record if persistence is disabled or
        // unreachable, so dev iteration without `docker compose up`
        // still works.
        let record = self.load_character_record(account_name, character_name, class_id, gender);
        if let Some(s) = self.sessions.get_mut(from) {
            s.account_name = Some(account_name.to_string());
            s.character_name = Some(character_name.to_string());
            s.class_id = Some(class_id.to_string());
            s.gender = Some(gender);
            s.record = Some(record);
        }
    }

    /// Spawn the player into the simulation and stash the net id
    /// on their session. The net id is what the client uses to
    /// recognize itself in subsequent snapshots.
    fn spawn_player_session(&mut self, from: ClientId, _class_id: &str) -> NetId {
        // New connections always land in the hub. Per-client
        // floor tracking starts here so the dispatch routes the
        // correct Sim for every subsequent message.
        let net_id = self.hub.spawn_player(from);
        self.client_floor.insert(from, 0);
        if let Some(s) = self.sessions.get_mut(from) {
            s.net_id = Some(net_id);
        }
        net_id
    }

    /// Push the persisted XP/level and inventory back into the
    /// freshly-spawned sim entity. Returns the loaded bag so the
    /// caller can replicate it to the picker without re-querying
    /// the simulation.
    fn hydrate_player_state(&mut self, from: ClientId) -> Vec<Option<rift_game::loot::Item>> {
        // XP / level: rebuild `current_xp` from `(total_xp, level)`
        // and recompute stats so the player isn't reset to level 1
        // on every login.
        if let Some(rec) = self.sessions.get(from).and_then(|s| s.record.as_ref()) {
            let level = rec.level.max(1) as u32;
            let xp = rec.xp.max(0) as u64;
            self.hub.set_player_experience(from, level, xp);
            let loadout = super::loadout_to_u8(rec.loadout);
            self.hub.set_player_loadout(from, loadout);
            let shards = rec.shards.max(0) as u32;
            self.hub.set_player_shards(from, shards);
            // Mirror the persisted character UUID onto the
            // live `ServerPlayer` so the loot subsystem can
            // stamp `LootProvenance` onto every fresh drop
            // and gate `try_pickup_loot` against the picker's
            // identity. Hub-only at hello time; per-rift sims
            // re-mirror on entry via the same call site.
            self.hub.set_player_character_id(from, rec.id);
        }

        // Inventory: skipped when persistence is disabled (dev
        // mode) or the load fails — the empty bag is still a
        // valid game state.
        let mut loaded_items: Vec<Option<rift_game::loot::Item>> = Vec::new();
        let mut loaded_equipment = rift_game::loot::Equipment::new();
        let Some(handle) = &self.persistence else {
            return loaded_items;
        };
        let Some(rec_id) = self.sessions.record_id(from) else {
            return loaded_items;
        };
        match handle.load_inventory_blocking(rec_id) {
            Ok(rows) => {
                // Each persisted row decodes either into the bag
                // (at its persisted `slot_index`) or directly into
                // an equipment slot, depending on `equipped_slot`.
                // A row whose slot byte is unknown (mismatched
                // build) falls through to the bag's first free
                // slot so the player never silently loses the item.
                for r in rows {
                    let Some(item) = rift_game::loot::Item::from_persisted(
                        &r.base_id,
                        r.rarity as u8,
                        r.ilvl as u16,
                        &r.affixes,
                        r.anchored,
                        super::provenance_from_persisted(r.provenance),
                    ) else {
                        continue;
                    };
                    match r.equipped_slot {
                        Some(b) => match rift_game::loot::EquipSlot::from_u8(b as u8) {
                            Some(slot) if rift_game::loot::Equipment::accepts(slot, &item) => {
                                loaded_equipment.set(slot, Some(item));
                            }
                            _ => place_at_slot_index(&mut loaded_items, r.slot_index, item),
                        },
                        None => place_at_slot_index(&mut loaded_items, r.slot_index, item),
                    }
                }
                let bag_filled = loaded_items.iter().filter(|s| s.is_some()).count();
                log::info!(
                    "{} loaded {} bag item(s) + {} equipped",
                    self.client_tag(from),
                    bag_filled,
                    loaded_equipment.count()
                );
                self.hub
                    .set_player_inventory(from, loaded_items.clone(), loaded_equipment);
            }
            Err(e) => log::warn!(
                "{} persistence: load_inventory failed: {e}",
                self.client_tag(from)
            ),
        }
        // Hydrate the per-character stash. Eager-load it at
        // Hello time so the chest interaction path stays
        // synchronous — the actual `StashSync` packet is held
        // back until the player opens the chest, which keeps
        // login lean.
        match handle.load_stash_blocking(rec_id) {
            Ok((tab_rows, item_rows)) => {
                use crate::sim::player::DEFAULT_STASH_TAB_COLOR;
                use crate::sim::StashTab;
                // Build the dense [0..n) tab list. If the
                // database has no `stash_tabs` rows yet (fresh
                // character or pre-migration save) we seed at
                // least one default tab so the player always
                // has somewhere to put items.
                let mut tabs: Vec<StashTab> = if tab_rows.is_empty() {
                    vec![StashTab::fresh(0)]
                } else {
                    let max_idx = tab_rows
                        .iter()
                        .map(|t| t.tab_index as usize)
                        .max()
                        .unwrap_or(0);
                    let mut tabs: Vec<StashTab> =
                        (0..=max_idx).map(|i| StashTab::fresh(i)).collect();
                    for row in &tab_rows {
                        if let Some(t) = tabs.get_mut(row.tab_index as usize) {
                            t.name = row.name.clone();
                            t.color = (row.color as u32) & 0x00FF_FFFF;
                        }
                    }
                    // If a row has a 0 color (e.g. legacy seed),
                    // fall back to the default neutral color so
                    // the tab strip stays readable.
                    for t in tabs.iter_mut() {
                        if t.color == 0 {
                            t.color = DEFAULT_STASH_TAB_COLOR;
                        }
                    }
                    tabs
                };
                let mut total_items = 0usize;
                for r in item_rows {
                    let Some(item) = rift_game::loot::Item::from_persisted(
                        &r.base_id,
                        r.rarity as u8,
                        r.ilvl as u16,
                        &r.affixes,
                        r.anchored,
                        super::provenance_from_persisted(r.provenance),
                    ) else {
                        continue;
                    };
                    let tab_idx = (r.tab_index as usize).min(tabs.len().saturating_sub(1));
                    place_at_slot_index(&mut tabs[tab_idx].items, r.slot_index, item);
                    total_items += 1;
                }
                log::info!(
                    "{} loaded {} stash item(s) across {} tab(s)",
                    self.client_tag(from),
                    total_items,
                    tabs.len(),
                );
                self.hub.set_player_stash(from, tabs);
            }
            Err(e) => log::warn!(
                "{} persistence: load_stash failed: {e}",
                self.client_tag(from)
            ),
        }
        loaded_items
    }

    /// Send the just-welcomed client every "here's your initial
    /// state" packet they need before snapshots start landing:
    /// `Welcome`, the bag + equipment mirrors, the XP bar, and
    /// the rift-progress meter.
    fn send_initial_packets(
        &mut self,
        from: ClientId,
        net_id: NetId,
        loaded_bag: &[Option<rift_game::loot::Item>],
    ) {
        let welcome = ServerMsg::Welcome {
            your_client_id: from,
            your_net_id: net_id,
            // New connections start in the hub regardless of
            // what the rift sim is doing, so the welcome carries
            // the hub's seed/index. If the player walks into the
            // rift portal later, a follow-up `LoadFloor` will
            // hand them the rift coordinates.
            floor_seed: self.hub.floor_seed,
            floor_index: self.hub.floor_index,
            tick: self.tick,
        };
        self.send_to(from, Channel::Control, &welcome);

        // Replicate the persisted bag to the picker so their
        // local mirror matches the server bag the moment they
        // enter the world. Sent on the reliable Control channel
        // so it can't be dropped. Sent unconditionally so an
        // empty bag definitively clears any stale UI state.
        let blobs: Vec<Option<rift_net::messages::ItemBlob>> = loaded_bag
            .iter()
            .map(|s| s.as_ref().map(item_to_blob))
            .collect();
        self.send_to(
            from,
            Channel::Control,
            &ServerMsg::InventorySync { items: blobs },
        );

        // Replicate the equipped set even when empty: the client
        // uses an empty `EquipmentSync` as a definitive "you have
        // nothing equipped" signal, which lets it clear any
        // stale UI state from a previous session on the same
        // process (rare but cheap to be correct about).
        let equip_pairs = self.hub.player_equipment(from);
        let equip_blobs: Vec<(u8, rift_net::messages::ItemBlob)> = equip_pairs
            .iter()
            .map(|(slot, it)| (slot.to_u8(), item_to_blob(it)))
            .collect();
        self.send_to(
            from,
            Channel::Control,
            &ServerMsg::EquipmentSync { slots: equip_blobs },
        );

        // Initial XP / level snapshot so the HUD bar is correct
        // before the first kill.
        if let Some((level, xp, xp_to_next)) = self.hub.player_stats_snapshot(from) {
            self.send_to(
                from,
                Channel::Control,
                &ServerMsg::CharacterStats {
                    level,
                    xp,
                    xp_to_next,
                },
            );
        }
        // Initial ability-loadout snapshot so the client's HUD
        // bar shows whatever was persisted (or the default for
        // a brand-new character).
        if let Some(slots) = self.hub.player_loadout_snapshot(from) {
            self.send_to(from, Channel::Control, &ServerMsg::Loadout { slots });
        }
        // Initial shard balance so the HUD readout is correct
        // before the player salvages anything.
        if let Some(shards) = self.hub.player_shards(from) {
            self.send_to(
                from,
                Channel::Control,
                &ServerMsg::ShardsSync { amount: shards },
            );
        }
        // Initial rift-progress snapshot: hub players see a
        // fresh / pristine bar regardless of the active rift's
        // state. They'll get the real numbers when they walk
        // through the portal.
        let rp = self.hub.rift_progress();
        self.send_to(
            from,
            Channel::Control,
            &ServerMsg::RiftProgress {
                progress: rp.progress,
                required: rp.required,
                boss_spawned: rp.boss_spawned,
                boss_killed: rp.boss_killed,
                floor_complete: rp.floor_complete,
            },
        );

        // Initial party state: empty (`leader: None, members:
        // []`) until the player accepts an invite. Sent so the
        // client UI starts in a known-good "solo" state instead
        // of inferring it from absence-of-message.
        self.send_to(
            from,
            Channel::Control,
            &ServerMsg::PartyState {
                leader: None,
                members: Vec::new(),
            },
        );

        // Initial deepest-cleared-floor watermark so the portal
        // modal can clamp its floor stepper to [1, deepest+1].
        let deepest = self
            .sessions
            .get(from)
            .and_then(|s| s.record.as_ref())
            .map(|r| r.deepest_cleared_floor.max(0) as u32)
            .unwrap_or(0);
        self.send_to(
            from,
            Channel::Control,
            &ServerMsg::DeepestFloorCleared { value: deepest },
        );
    }

    /// Catch the newcomer up on every already-connected player,
    /// then broadcast their own `PlayerJoined` to the room.
    fn announce_join(
        &mut self,
        from: ClientId,
        net_id: NetId,
        character_name: String,
        class_id: String,
        gender: Gender,
    ) {
        let already_here: Vec<ServerMsg> = self
            .sessions
            .iter()
            .filter(|s| s.client_id != from)
            .filter_map(|s| {
                Some(ServerMsg::PlayerJoined {
                    net_id: s.net_id?,
                    client_id: s.client_id,
                    character_name: s.character_name.clone()?,
                    class_id: s.class_id.clone()?,
                    gender: s.gender?,
                })
            })
            .collect();
        for msg in already_here {
            self.send_to(from, Channel::Control, &msg);
        }
        // Catch the newcomer up on every existing player's
        // current visible equipment so their avatars spawn
        // already dressed instead of starting bare and popping
        // their gear in on the next equip change.
        let peer_visuals: Vec<(ClientId, Vec<u16>)> = self
            .sessions
            .iter()
            .filter(|s| s.client_id != from)
            .map(|s| s.client_id)
            .map(|cid| (cid, self.current_visible_base_ids(cid)))
            .collect();
        for (cid, base_ids) in peer_visuals {
            self.send_to(
                from,
                Channel::Control,
                &ServerMsg::PeerEquipmentVisuals {
                    client_id: cid,
                    base_ids,
                },
            );
        }
        let joined = ServerMsg::PlayerJoined {
            net_id,
            client_id: from,
            character_name: character_name.clone(),
            class_id,
            gender,
        };
        self.broadcast(Channel::Control, &joined);

        // Tell every existing player about the newcomer's
        // visible equipment too. Most of the time this is the
        // empty default-loadout, but if the player has a
        // persisted bag with equipped art-bearing items the
        // others will see them dressed on first frame.
        self.broadcast_peer_equipment_visuals(from);

        // Chat: replay the *prior* GLOBAL+SYSTEM history to
        // the joiner first, then announce the join. If we
        // announced first, the announcement would land in the
        // history buffer *and* the live broadcast — the
        // joiner would see "X joined" twice.
        self.replay_chat_history_to(from);
        self.emit_system_global(&format!("{character_name} joined."));
    }

    /// Apply a `ClientMsg::SetLoadoutSlot`. Validates against the
    /// authoritative `ServerPlayer.loadout`, mirrors the change
    /// into the persisted `CharacterRecord`, and pushes a fresh
    /// `ServerMsg::Loadout` snapshot back to the client. Silent
    /// no-op on a rejected slot/ability — the client's HUD will
    /// stay on the last accepted snapshot.
    pub(crate) fn handle_set_loadout_slot(
        &mut self,
        from: ClientId,
        slot_index: u8,
        ability_id: u8,
    ) {
        let Some(slots) = self
            .sim_for_client_mut(from)
            .set_player_loadout_slot(from, slot_index, ability_id)
        else {
            return;
        };
        // Mirror into the cached `CharacterRecord` so the next
        // periodic `save` tick writes the new loadout. Persistence
        // is fire-and-forget; the client doesn't wait on it.
        let saved_record = if let Some(s) = self.sessions.get_mut(from) {
            if let Some(rec) = s.record.as_mut() {
                let mut as_i16 = [0i16; 6];
                for (i, &slot) in slots.iter().enumerate() {
                    as_i16[i] = slot as i16;
                }
                rec.loadout = as_i16;
                Some(rec.clone())
            } else {
                None
            }
        } else {
            None
        };
        if let (Some(handle), Some(rec)) = (&self.persistence, saved_record) {
            let _ = handle.save(rec);
        }
        self.send_to(from, Channel::Control, &ServerMsg::Loadout { slots });
    }
}
