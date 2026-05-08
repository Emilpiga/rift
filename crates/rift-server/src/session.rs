//! Per-client session bookkeeping.
//!
//! Pulled out of `main.rs` so the top-level `Server` doesn't have
//! to spell out the data layout every time it touches a session,
//! and so the lookup pattern can graduate from a linear scan over
//! a `Vec` to an O(1) `HashMap`. We still cap the player count at
//! `MAX_CLIENTS` (4) for now, but the hash-keyed access also
//! removes a class of bugs around stale session indices.

use std::collections::{HashMap, HashSet};

use rift_net::{ClientId, Gender, NetId};
use rift_persistence::CharacterRecord;

use crate::chat::ChatRateLimit;

/// Per-connected-client state the server tracks. Most fields are
/// `None` until the client's `Hello` has been processed and we've
/// resolved their persisted character.
pub struct ClientSession {
    pub client_id: ClientId,
    /// `None` until the client's `Hello` has been processed.
    pub character_name: Option<String>,
    /// Account display name supplied with `Hello`. Used to
    /// resolve / create the persistent `accounts` row that owns
    /// this session's character.
    pub account_name: Option<String>,
    /// Profile fields (set on Hello). `None` until welcomed.
    pub class_id: Option<String>,
    pub gender: Option<Gender>,
    /// Net id assigned to this player by the simulation. `None`
    /// until the player has been spawned (post-Hello).
    pub net_id: Option<NetId>,
    /// Authoritative persisted state for this character. `None` if
    /// persistence is disabled (no `--database-url`) or if the load
    /// failed and we fell back to in-memory only.
    pub record: Option<CharacterRecord>,
    /// Character names this player has muted. Whisper / chat
    /// from a muted name is silently dropped on the recipient's
    /// `send_to` step; the sender never sees the mute (so they
    /// can't trivially probe for mutes). Cleared on disconnect
    /// — mute persistence lands with whisper history.
    pub mute_list: HashSet<String>,
    /// Per-player chat rate limiter. Sliding window; sends
    /// over the cap are silently dropped server-side.
    pub chat_bucket: ChatRateLimit,
}

impl ClientSession {
    pub fn new(client_id: ClientId) -> Self {
        Self {
            client_id,
            character_name: None,
            account_name: None,
            class_id: None,
            gender: None,
            net_id: None,
            record: None,
            mute_list: HashSet::new(),
            chat_bucket: ChatRateLimit::default(),
        }
    }
}

/// Hash-keyed pool of active client sessions.
///
/// Wraps `HashMap<ClientId, ClientSession>` so the rest of the
/// server can use intent-revealing methods (`get`, `record_id`,
/// `iter`) instead of repeatedly writing
/// `self.sessions.iter().find(|s| s.client_id == from)`.
#[derive(Default)]
pub struct SessionManager {
    sessions: HashMap<ClientId, ClientSession>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a freshly-connected session. If a session for the
    /// same id somehow already exists it is replaced — that
    /// shouldn't happen in practice (renet emits a clean
    /// disconnect first) but we don't want to silently keep a
    /// stale row either.
    pub fn insert(&mut self, session: ClientSession) {
        self.sessions.insert(session.client_id, session);
    }

    /// Drop the session for `id`, returning it so the caller can
    /// fire any final teardown (e.g. last persistence save).
    pub fn remove(&mut self, id: ClientId) -> Option<ClientSession> {
        self.sessions.remove(&id)
    }

    pub fn get(&self, id: ClientId) -> Option<&ClientSession> {
        self.sessions.get(&id)
    }

    pub fn get_mut(&mut self, id: ClientId) -> Option<&mut ClientSession> {
        self.sessions.get_mut(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &ClientSession> {
        self.sessions.values()
    }

    /// Convenience accessor for the persisted record id, which is
    /// what most persistence calls actually need.
    pub fn record_id(&self, id: ClientId) -> Option<rift_persistence::Uuid> {
        self.sessions
            .get(&id)
            .and_then(|s| s.record.as_ref())
            .map(|r| r.id)
    }
}
