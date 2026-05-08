//! Chat dispatch + system-event emit helpers.
//!
//! Two surfaces:
//!
//! * [`Server::handle_chat_send`] — handles a
//!   [`ClientMsg::ChatSend`]: validates length, applies the
//!   per-player rate limit, resolves the recipient set from
//!   the wire channel, and fans the resulting
//!   [`ServerMsg::Chat`] out to each recipient (filtered
//!   through their personal mute list).
//! * `emit_system_*` — server-side convenience helpers used
//!   by the connect / disconnect / death / boss-kill / floor /
//!   level-up hooks to push a system line into the right
//!   channel without going through the client-message path.
//!
//! Routing reads `client_floor` for FLOOR / HUB scoping and
//! `ClientSession::party_id` for PARTY scoping. Mute filtering
//! happens at the recipient level (after the recipient set is
//! resolved) so a sender can't probe for who has them muted.

use std::time::Instant;

use rift_net::{
    messages::chat_channel, Channel, ClientId, ServerMsg,
};

use crate::chat::{
    is_client_sendable, sanitise_text, ChatEntry, Recipient, REPLAY_HISTORY,
};
use crate::Server;

impl Server {
    /// Handle a [`ClientMsg::ChatSend`] from `from`. Validates
    /// the channel byte, the body, and the rate-limit; on
    /// accept, builds the recipient set and broadcasts.
    /// Silently drops on every failure (rate-limited, empty
    /// after trim, unknown channel, sender not yet welcomed).
    pub(crate) fn handle_chat_send(
        &mut self,
        from: ClientId,
        channel: u8,
        target: Option<String>,
        text: String,
    ) {
        // 1. Schema gate: clients can't impersonate SYSTEM
        //    lines, and we silently drop forward-extension
        //    bytes the server doesn't yet route.
        if !is_client_sendable(channel) {
            log::debug!("chat: dropping send from {from:?}: channel {channel} not client-sendable");
            return;
        }

        // 2. Sender must be welcomed (we need their character
        //    name for the broadcast).
        let Some(sender_name) = self
            .sessions
            .get(from)
            .and_then(|s| s.character_name.clone())
        else {
            log::debug!("chat: dropping send from {from:?}: no character name");
            return;
        };

        // 3. Body validation.
        let Some(body) = sanitise_text(&text) else {
            return;
        };

        // 4. Rate limit. Bucket lives on the session so it
        //    survives across messages but resets on reconnect.
        let now = Instant::now();
        let allowed = self
            .sessions
            .get_mut(from)
            .map(|s| s.chat_bucket.try_consume(now))
            .unwrap_or(false);
        if !allowed {
            log::debug!("chat: rate-limited send from {from:?}");
            return;
        }

        // 5. Resolve recipients. Each branch returns a `Vec`
        //    of `(ClientId, character_name)` pairs that the
        //    routing pass below will fan the message out to.
        //    Whisper has its own UnknownRecipient path that
        //    short-circuits with a system reply.
        let recipients: Vec<Recipient> = match channel {
            chat_channel::GLOBAL => self.recipients_global(),
            chat_channel::HUB => self.recipients_on_floor(0),
            chat_channel::FLOOR => {
                let f = self.floor_for_client(from);
                self.recipients_on_floor(f)
            }
            chat_channel::PARTY => {
                let pid = self.sessions.get(from).map(|s| s.party_id).unwrap_or(0);
                self.recipients_in_party(pid)
            }
            chat_channel::WHISPER => {
                let target_name = match target.as_deref().map(|s| s.trim()) {
                    Some(t) if !t.is_empty() => t.to_string(),
                    _ => {
                        // Malformed whisper (no name). Reply
                        // with a system line so the typer
                        // knows their /w syntax was wrong.
                        self.emit_system_to(
                            from,
                            "Whisper requires a recipient name.",
                        );
                        return;
                    }
                };
                match self.find_recipient_by_name(&target_name) {
                    Some(r) => {
                        // Whisper goes to the named recipient
                        // *and* echoes back to the sender so
                        // they see their own message.
                        let mut v = vec![r];
                        if v[0].client_id != from {
                            v.push(Recipient {
                                client_id: from,
                                character_name: sender_name.clone(),
                            });
                        }
                        v
                    }
                    None => {
                        self.emit_system_to(
                            from,
                            &format!("No player named '{target_name}' is online."),
                        );
                        return;
                    }
                }
            }
            _ => return,
        };

        // 6. Build entry, store in history (so future replays
        //    can see it for the public channels), and fan out
        //    after applying recipient mute filters.
        let target_field = if channel == chat_channel::WHISPER {
            target.as_deref().map(|s| s.trim().to_string())
        } else {
            None
        };
        let entry = ChatEntry {
            channel,
            sender: Some(sender_name.clone()),
            target: target_field.clone(),
            text: body.clone(),
        };
        // Only retain history for the broadly-public channels;
        // PARTY / WHISPER / FLOOR are conversational and
        // shouldn't replay to a hub joiner who wasn't part of
        // the conversation.
        if matches!(channel, chat_channel::GLOBAL) {
            self.chat.push(entry.clone());
        }

        let msg = ServerMsg::Chat {
            channel,
            sender: Some(sender_name.clone()),
            target: target_field,
            text: body,
        };
        self.fan_out(&recipients, &sender_name, &msg);
    }

    /// Fan `msg` out to every entry in `recipients`, skipping
    /// any whose `mute_list` contains the sender. The mute
    /// filter is recipient-side so a sender can't probe for
    /// who has them muted.
    pub(crate) fn fan_out(
        &mut self,
        recipients: &[Recipient],
        sender_name: &str,
        msg: &ServerMsg,
    ) {
        for r in recipients {
            // Skip if recipient has muted the sender. Pulled
            // through the session manager so the check is
            // O(1) per recipient.
            if let Some(s) = self.sessions.get(r.client_id) {
                if s.mute_list.contains(sender_name) {
                    continue;
                }
            }
            self.send_to(r.client_id, Channel::Control, msg);
        }
    }

    /// Every welcomed client.
    fn recipients_global(&self) -> Vec<Recipient> {
        self.sessions
            .iter()
            .filter_map(|s| {
                Some(Recipient {
                    client_id: s.client_id,
                    character_name: s.character_name.clone()?,
                })
            })
            .collect()
    }

    /// Every welcomed client on the given floor index.
    fn recipients_on_floor(&self, floor_index: u32) -> Vec<Recipient> {
        self.sessions
            .iter()
            .filter(|s| self.floor_for_client(s.client_id) == floor_index)
            .filter_map(|s| {
                Some(Recipient {
                    client_id: s.client_id,
                    character_name: s.character_name.clone()?,
                })
            })
            .collect()
    }

    /// Every welcomed client whose `party_id` matches.
    fn recipients_in_party(&self, party_id: u64) -> Vec<Recipient> {
        self.sessions
            .iter()
            .filter(|s| s.party_id == party_id)
            .filter_map(|s| {
                Some(Recipient {
                    client_id: s.client_id,
                    character_name: s.character_name.clone()?,
                })
            })
            .collect()
    }

    /// Look up a welcomed client by character name (case-
    /// insensitive). Returns `None` if no match.
    fn find_recipient_by_name(&self, name: &str) -> Option<Recipient> {
        let needle = name.to_ascii_lowercase();
        self.sessions.iter().find_map(|s| {
            let cn = s.character_name.as_ref()?;
            if cn.to_ascii_lowercase() == needle {
                Some(Recipient {
                    client_id: s.client_id,
                    character_name: cn.clone(),
                })
            } else {
                None
            }
        })
    }

    // ── System emit helpers ───────────────────────────────────

    /// Push a system line to every welcomed client. Used by
    /// connect / disconnect / boss-kill announcements.
    pub(crate) fn emit_system_global(&mut self, text: &str) {
        let entry = ChatEntry {
            channel: chat_channel::SYSTEM,
            sender: None,
            target: None,
            text: text.to_string(),
        };
        self.chat.push(entry.clone());
        let msg = ServerMsg::Chat {
            channel: chat_channel::SYSTEM,
            sender: None,
            target: None,
            text: text.to_string(),
        };
        // System messages bypass the mute filter (and aren't
        // attributed to a sender to filter on anyway).
        let recipients: Vec<ClientId> =
            self.sessions.iter().map(|s| s.client_id).collect();
        for cid in recipients {
            self.send_to(cid, Channel::Control, &msg);
        }
        log::info!("[chat:SYSTEM] {text}");
    }

    /// Push a system line to every welcomed client currently
    /// on `floor_index`. Used by death + floor-entered
    /// announcements.
    pub(crate) fn emit_system_floor(&mut self, floor_index: u32, text: &str) {
        let msg = ServerMsg::Chat {
            channel: chat_channel::SYSTEM,
            sender: None,
            target: None,
            text: text.to_string(),
        };
        let recipients: Vec<ClientId> = self
            .sessions
            .iter()
            .filter(|s| self.floor_for_client(s.client_id) == floor_index)
            .map(|s| s.client_id)
            .collect();
        for cid in recipients {
            self.send_to(cid, Channel::Control, &msg);
        }
        log::info!("[chat:SYSTEM/floor={floor_index}] {text}");
    }

    /// Push a system line to a single client. Used for
    /// per-player events (level-up, whisper-target-not-found,
    /// /mute confirmations).
    pub(crate) fn emit_system_to(&mut self, to: ClientId, text: &str) {
        let msg = ServerMsg::Chat {
            channel: chat_channel::SYSTEM,
            sender: None,
            target: None,
            text: text.to_string(),
        };
        self.send_to(to, Channel::Control, &msg);
    }

    /// Replay recent global + system history to a freshly-
    /// welcomed client so they have context. Called from
    /// `announce_join`. PARTY / FLOOR / WHISPER are not
    /// replayed (they were never part of the joiner's view).
    pub(crate) fn replay_chat_history_to(&mut self, to: ClientId) {
        // Snapshot the entries we want to send before going
        // through `send_to` so the borrow on `self.chat` ends
        // first.
        let entries: Vec<ChatEntry> = self
            .chat
            .recent(chat_channel::GLOBAL, REPLAY_HISTORY)
            .chain(self.chat.recent(chat_channel::SYSTEM, REPLAY_HISTORY))
            .cloned()
            .collect();
        for entry in entries {
            let msg = ServerMsg::Chat {
                channel: entry.channel,
                sender: entry.sender,
                target: entry.target,
                text: entry.text,
            };
            self.send_to(to, Channel::Control, &msg);
        }
    }
}
