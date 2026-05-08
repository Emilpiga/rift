//! Chat: in-memory history + per-channel routing + rate
//! limiting + per-player mute lists.
//!
//! Lives at the top level (alongside `session.rs`) rather than
//! inside `sim/` because chat is server-global state, not
//! per-floor world state. The `Server` struct owns one
//! [`ChatHistory`] and threads it through every emit / route
//! site.
//!
//! Routing model:
//!
//! * [`chat_channel::GLOBAL`] — every connected client.
//! * [`chat_channel::HUB`]    — every client currently on
//!   floor 0.
//! * [`chat_channel::FLOOR`]  — every client currently on the
//!   sender's floor.
//! * [`chat_channel::PARTY`]  — every client whose
//!   `party_id` matches the sender's. Party stub: each client
//!   defaults to `party_id = client_id`, so PARTY echoes back
//!   to the sender alone until a real party system fills in
//!   shared ids.
//! * [`chat_channel::WHISPER`] — sender + named recipient
//!   only.
//! * [`chat_channel::SYSTEM`] — server-emit-only; the wire
//!   path rejects client sends.
//!
//! All routing is done by name + client id at the call site;
//! the per-recipient mute filter is applied in [`route`]
//! after the recipient set has been resolved.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use rift_net::messages::{chat_channel, CHAT_MAX_LEN};
use rift_net::ClientId;

/// How many lines of `GLOBAL` + `SYSTEM` history we replay to
/// a freshly-connected client right after their `Hello` is
/// accepted. Enough to give context ("oh, the boss died 30 s
/// ago") without flooding the scrollback. Per-channel; the
/// total replay is up to `2 * REPLAY_HISTORY` lines.
pub const REPLAY_HISTORY: usize = 25;

/// How many lines per channel we retain in memory total.
/// Independent of [`REPLAY_HISTORY`] so the buffer can absorb
/// short bursts without dropping the older lines a fresh joiner
/// would want to replay.
const HISTORY_CAP: usize = 100;

/// Per-player chat rate limit: at most [`RATE_LIMIT_MAX`]
/// messages within any rolling [`RATE_LIMIT_WINDOW`]. Applied
/// before routing so a spammer can't fan out to every connected
/// client with one packet. Tuned to allow normal conversation
/// (a few lines per second in a heated chat) while shutting
/// down obvious flood attempts.
const RATE_LIMIT_MAX: usize = 5;
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(5);

/// One stored chat line. Held in [`ChatHistory`] for replay and
/// flushed straight into [`ServerMsg::Chat`] on send.
#[derive(Clone, Debug)]
pub struct ChatEntry {
    pub channel: u8,
    /// Sender's character name. `None` for system events.
    pub sender: Option<String>,
    /// Whisper recipient's character name (whisper channel
    /// only).
    pub target: Option<String>,
    pub text: String,
}

/// Per-channel ring buffer of chat history. Bounded at
/// [`HISTORY_CAP`] entries per channel so the buffer never
/// grows unbounded.
#[derive(Default)]
pub struct ChatHistory {
    per_channel: HashMap<u8, VecDeque<ChatEntry>>,
}

impl ChatHistory {
    pub fn push(&mut self, entry: ChatEntry) {
        let buf = self.per_channel.entry(entry.channel).or_default();
        if buf.len() == HISTORY_CAP {
            buf.pop_front();
        }
        buf.push_back(entry);
    }

    /// Last `n` entries on `channel`, oldest first. Returns an
    /// empty iterator for unknown / empty channels.
    pub fn recent(&self, channel: u8, n: usize) -> impl Iterator<Item = &ChatEntry> {
        let slice: Vec<&ChatEntry> = self
            .per_channel
            .get(&channel)
            .map(|buf| {
                let start = buf.len().saturating_sub(n);
                buf.iter().skip(start).collect::<Vec<_>>()
            })
            .unwrap_or_default();
        slice.into_iter()
    }
}

/// Per-player chat rate-limiter. Token bucket-ish: a sliding
/// window of recent send timestamps; sends are rejected when
/// the window holds [`RATE_LIMIT_MAX`] entries.
#[derive(Default, Debug)]
pub struct ChatRateLimit {
    sends: VecDeque<Instant>,
}

impl ChatRateLimit {
    /// Returns `true` if a fresh send is allowed *and* records
    /// the timestamp. Returns `false` if the player is over
    /// the limit, in which case nothing is recorded — the
    /// rejection is invisible to subsequent calls.
    pub fn try_consume(&mut self, now: Instant) -> bool {
        let cutoff = now - RATE_LIMIT_WINDOW;
        while let Some(front) = self.sends.front() {
            if *front < cutoff {
                self.sends.pop_front();
            } else {
                break;
            }
        }
        if self.sends.len() >= RATE_LIMIT_MAX {
            return false;
        }
        self.sends.push_back(now);
        true
    }
}

/// Trim and clamp a chat body to [`CHAT_MAX_LEN`] characters.
/// Returns `None` if the message is empty after trimming
/// (silent drop — clients that send empty lines on Enter
/// shouldn't generate broadcast traffic).
pub fn sanitise_text(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let clamped: String = trimmed.chars().take(CHAT_MAX_LEN).collect();
    Some(clamped)
}

/// Wire id sanity check. The wire schema reserves `SYSTEM` for
/// server-emitted lines; client sends with that id are dropped
/// so clients can't impersonate system events. Also rejects
/// any future-extension byte we haven't taught the server
/// about.
pub fn is_client_sendable(channel: u8) -> bool {
    matches!(
        channel,
        chat_channel::GLOBAL
            | chat_channel::HUB
            | chat_channel::FLOOR
            | chat_channel::PARTY
            | chat_channel::WHISPER
    )
}

/// One resolved recipient. Just the `ClientId` for now — the
/// send-loop fans out by id and queries the session manager
/// when it needs a display name.
#[derive(Clone, Debug)]
pub struct Recipient {
    pub client_id: ClientId,
}
