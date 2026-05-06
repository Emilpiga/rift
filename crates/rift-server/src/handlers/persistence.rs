//! Persistence-touching helpers shared across the dispatch
//! handlers: per-tick XP saves, character-record loads, account
//! roster lookups. These are split out of `main.rs` so the
//! database integration points live in one file.

use rift_net::{ClientId, Gender};
use rift_net::messages::RosterEntry;
use rift_persistence::{CharacterRecord, Uuid};

use super::gender_from_i16;
use crate::Server;

impl Server {
    /// Persist the latest XP / level snapshot for the supplied
    /// client. Fire-and-forget; failure logs but doesn't block
    /// gameplay. Mutates the in-memory `record` so subsequent
    /// `Save` calls (e.g. on disconnect) carry the latest values.
    /// Total XP is provided by the sim alongside the level so the
    /// XP curve never has to be recomputed here.
    pub(crate) fn persist_xp_for(&mut self, client_id: ClientId, level: u32, total_xp: u64) {
        let Some(s) = self.sessions.get_mut(client_id) else {
            return;
        };
        let Some(rec) = s.record.as_mut() else { return };
        rec.level = level as i32;
        rec.xp = total_xp.min(i32::MAX as u64) as i32;
        if let Some(handle) = &self.persistence {
            let _ = handle.save(rec.clone());
        }
    }

    /// Resolve the persistent record for a session's `Hello`. If
    /// persistence is disabled (no DB), or the worker fails the
    /// query, we synthesize a fresh record so the player can still
    /// play — their progress just won't survive a restart. The
    /// fallback record uses a random UUID so subsequent saves on
    /// the same name don't collide with a real DB row by accident.
    pub(crate) fn load_character_record(
        &self,
        account_name: &str,
        character_name: &str,
        class_id: &str,
        gender: Gender,
    ) -> CharacterRecord {
        let gender_id = gender as i16;
        if let Some(handle) = &self.persistence {
            match handle.load_or_create_blocking(
                account_name.to_string(),
                character_name.to_string(),
                class_id.to_string(),
                gender_id,
            ) {
                Ok(rec) => {
                    log::info!(
                        "persistence: loaded {} on account {} (level={}, xp={})",
                        rec.name,
                        account_name,
                        rec.level,
                        rec.xp,
                    );
                    return rec;
                }
                Err(e) => {
                    log::warn!(
                        "persistence: load_or_create failed for account={account_name:?} name={character_name:?}: {e}; using in-memory record"
                    );
                }
            }
        }
        // Fallback: ephemeral record. `id` is a fresh UUID so the
        // periodic `Save` UPDATE simply targets zero rows — that's
        // a no-op, not an error.
        CharacterRecord {
            id: Uuid::new_v4(),
            account_id: Uuid::new_v4(),
            name: character_name.to_string(),
            class_id: class_id.to_string(),
            gender: gender_id,
            level: 1,
            xp: 0,
        }
    }

    /// Resolve `account_name` to its character roster. Falls back
    /// to an empty list when persistence is disabled or the DB
    /// query fails — the client is then free to create a fresh
    /// character, which load_character_record will persist on the
    /// next Hello.
    pub(crate) fn lookup_roster(&self, account_name: &str) -> Vec<RosterEntry> {
        let Some(handle) = &self.persistence else { return Vec::new() };
        match handle.list_account_characters_blocking(account_name.to_string()) {
            Ok((_account_id, records)) => records
                .into_iter()
                .map(|r| RosterEntry {
                    character_name: r.name,
                    class_id: r.class_id,
                    gender: gender_from_i16(r.gender),
                    level: r.level.max(0) as u32,
                })
                .collect(),
            Err(e) => {
                log::warn!(
                    "persistence: list_account_characters failed for {account_name:?}: {e}; returning empty roster"
                );
                Vec::new()
            }
        }
    }

    /// Fire a fire-and-forget save for every session that has a
    /// persisted record attached. Called from the periodic
    /// auto-save tick. Cheap when no characters are connected or
    /// when persistence is disabled.
    pub(crate) fn auto_save_all(&self) {
        let Some(handle) = &self.persistence else { return };
        let mut count = 0usize;
        for s in self.sessions.iter() {
            if let Some(rec) = &s.record {
                if handle.save(rec.clone()) {
                    count += 1;
                }
            }
        }
        if count > 0 {
            log::debug!("persistence: auto-save queued for {count} character(s)");
        }
    }
}
