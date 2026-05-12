//! Party message handlers.
//!
//! Each `pub(crate) fn handle_party_*` is a sibling-impl on
//! [`Server`] dispatched by the top-level `match` in
//! `main.rs::handle_client_msg`. Validation that depends on
//! cross-cutting state (rift-instance presence, session
//! lookup, persistence) lives here; pure-bookkeeping lives
//! in [`crate::party::PartyManager`].

use std::time::Instant;

use rift_net::messages::{PartyMember, ServerMsg};
use rift_net::{Channel, ClientId};

use crate::party::{AcceptOutcome, InviteOutcome, RemoveOutcome};
use crate::Server;

/// Maximum number of characters accepted in any party-message
/// `name` field. Player display names are themselves capped at
/// 18 by `validate_name` in the session handler; we use a
/// slightly looser cap here purely as DoS protection so a
/// modded client can't make us format multi-megabyte error
/// toasts. Anything that survives this gate gets fed through
/// `find_session_by_name` next, which only matches real names.
const PARTY_NAME_MAX: usize = 64;

/// Quick "is this string short enough to be a plausible name"
/// check. Returns `true` if `value` (after trim) is non-empty
/// and at most `PARTY_NAME_MAX` chars. Caller is responsible
/// for emitting a `party_error` on `false`.
fn name_within_limits(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty() && trimmed.chars().count() <= PARTY_NAME_MAX
}

impl Server {
    /// Resolve a character name to its `ClientId`. Case-
    /// insensitive (mirrors how whispers find their target).
    /// Returns `None` if no welcomed session matches the name.
    fn find_session_by_name(&self, name: &str) -> Option<ClientId> {
        let needle = name.to_ascii_lowercase();
        self.sessions
            .iter()
            .find(|s| {
                s.character_name
                    .as_deref()
                    .map(|n| n.to_ascii_lowercase() == needle)
                    .unwrap_or(false)
            })
            .map(|s| s.client_id)
    }

    /// Look up the inviter's character name for a fresh invite
    /// toast. Falls back to a placeholder when the inviter
    /// somehow lacks a session row (shouldn't happen post-
    /// Welcome but keeps the toast non-empty).
    fn name_of(&self, cid: ClientId) -> String {
        self.sessions
            .get(cid)
            .and_then(|s| s.character_name.clone())
            .unwrap_or_else(|| "A player".to_string())
    }

    /// Build a [`PartyMember`] row for `cid` from live session
    /// + sim state. Falls back to safe defaults when fields
    /// aren't loaded yet (pre-Welcome).
    fn snapshot_member(&self, cid: ClientId) -> Option<PartyMember> {
        let session = self.sessions.get(cid)?;
        let character_name = session.character_name.clone()?;
        let class_id = session.class_id.clone().unwrap_or_default();
        let (level, _xp) = session
            .record
            .as_ref()
            .map(|r| (r.level.max(0) as u32, r.xp.max(0) as u64))
            .unwrap_or((1, 0));
        let deepest = session
            .record
            .as_ref()
            .map(|r| r.deepest_cleared_floor.max(0) as u32)
            .unwrap_or(0);
        let (hp, hp_max) = self
            .sim_for_client(cid)
            .player_health(cid)
            .unwrap_or((1.0, 1.0));
        let floor = self.floor_for_client(cid);
        Some(PartyMember {
            character_name,
            class_id,
            level,
            hp,
            hp_max,
            floor,
            deepest_cleared_floor: deepest,
        })
    }

    /// Build a fresh [`ServerMsg::PartyState`] for `viewer`
    /// based on whatever party they currently belong to (or
    /// `members: []` when solo).
    pub(crate) fn build_party_state_for(&self, viewer: ClientId) -> ServerMsg {
        let Some(party) = self.parties.party_of(viewer) else {
            return ServerMsg::PartyState {
                leader: None,
                members: Vec::new(),
            };
        };
        let leader = self
            .sessions
            .get(party.leader)
            .and_then(|s| s.character_name.clone());
        let members: Vec<PartyMember> = party
            .members
            .iter()
            .filter_map(|cid| self.snapshot_member(*cid))
            .collect();
        ServerMsg::PartyState { leader, members }
    }

    /// Rebroadcast a fresh `PartyState` to every client that is
    /// currently in a party. Called on a 1 Hz cadence from the
    /// main loop so party-frame HUDs see live hp / level /
    /// floor updates (the per-event broadcasts only fire on
    /// join / leave / kick / promote).
    pub(crate) fn broadcast_party_states(&mut self) {
        // Snapshot the viewer set first so we don't hold any
        // borrow into `self` across the per-viewer build + send.
        let viewers: Vec<ClientId> = self
            .sessions
            .iter()
            .map(|s| s.client_id)
            .filter(|cid| self.parties.party_of(*cid).is_some())
            .collect();
        for cid in viewers {
            let msg = self.build_party_state_for(cid);
            self.send_to(cid, Channel::Control, &msg);
        }
    }

    /// Drive the broadcast / cleanup chain after a remove
    /// (`/leave`, `/kick`, disconnect). `removed` carries the
    /// post-remove state of the party.
    ///
    /// `pre_remove_members` is the party's roster as it stood
    /// *before* the remove fired, including `leaver`. The
    /// singleton-collapse path in [`crate::party::PartyManager::leave`]
    /// returns `Solo` after dropping the lone remaining member
    /// from `by_member`, so without this list we'd never know
    /// who was orphaned and their PartyUi would keep showing
    /// the now-defunct party. Pass an empty slice when the
    /// caller has no roster handy (e.g. disconnect of a solo
    /// client).
    pub(crate) fn broadcast_party_after_remove(
        &mut self,
        leaver: ClientId,
        removed: RemoveOutcome,
        pre_remove_members: &[ClientId],
    ) {
        // Always push an empty `PartyState` to the leaver so
        // their frames widget hides.
        let leaver_msg = ServerMsg::PartyState {
            leader: None,
            members: Vec::new(),
        };
        self.send_to(leaver, Channel::Control, &leaver_msg);
        match removed {
            RemoveOutcome::Solo => {
                // Either the leaver wasn't in a party (no-op)
                // or the remove dropped the party to a single
                // remaining member who was then collapsed to
                // solo. Push an empty PartyState to every
                // pre-remove member so any orphaned client
                // hides its roster.
                for cid in pre_remove_members {
                    if *cid == leaver {
                        continue;
                    }
                    self.send_to(*cid, Channel::Control, &leaver_msg);
                }
            }
            RemoveOutcome::Dissolved => { /* nobody left to notify */ }
            RemoveOutcome::Updated(party) => {
                for cid in party.members {
                    let msg = self.build_party_state_for(cid);
                    self.send_to(cid, Channel::Control, &msg);
                }
            }
        }
    }

    /// Soft-error reply: emit a `PartyError` toast plus a
    /// matching SYSTEM chat line so the player sees the
    /// reason whether or not their HUD surfaces toasts.
    fn party_error(&mut self, to: ClientId, reason: &str) {
        let msg = ServerMsg::PartyError {
            reason: reason.to_string(),
        };
        self.send_to(to, Channel::Control, &msg);
        self.emit_system_to(to, reason);
    }

    pub(crate) fn handle_party_invite(&mut self, from: ClientId, name: String) {
        if !name_within_limits(&name) {
            self.party_error(from, "That name isn't valid.");
            return;
        }
        let trimmed = name.trim();
        if trimmed.is_empty() {
            self.party_error(from, "Invite who?");
            return;
        }
        let Some(invitee) = self.find_session_by_name(trimmed) else {
            self.party_error(from, &format!("No player named {trimmed} is online."));
            return;
        };
        if invitee == from {
            self.party_error(from, "You can't invite yourself.");
            return;
        }
        // Block cross-rift invites in either direction —
        // simpler model, no "stranded in hub" UX.
        if self.instance_for_client(from).is_some() {
            self.party_error(from, "You can't invite while in a rift.");
            return;
        }
        if self.instance_for_client(invitee).is_some() {
            self.party_error(from, &format!("{trimmed} is currently inside a rift."));
            return;
        }
        let inviter_name = self.name_of(from);
        let outcome =
            self.parties
                .record_invite(from, inviter_name.clone(), invitee, Instant::now());
        match outcome {
            InviteOutcome::Recorded | InviteOutcome::Refreshed => {
                let toast = ServerMsg::PartyInviteIncoming { from: inviter_name };
                self.send_to(invitee, Channel::Control, &toast);
                self.emit_system_to(from, &format!("Invited {trimmed} to your party."));
            }
            InviteOutcome::AlreadySameParty => {
                self.party_error(from, &format!("{trimmed} is already in your party."));
            }
            InviteOutcome::InviteeAlreadyInParty => {
                self.party_error(from, &format!("{trimmed} is already in another party."));
            }
            InviteOutcome::InviterPartyFull => {
                self.party_error(from, "Your party is full.");
            }
        }
    }

    pub(crate) fn handle_party_accept(&mut self, from: ClientId, which: Option<String>) {
        let outcome = self.parties.accept_invite(from, which.as_deref());
        match outcome {
            AcceptOutcome::Joined(party) => {
                let leader_name = self.name_of(party.leader);
                let joiner_name = self.name_of(from);
                // Push fresh PartyState to every member.
                for cid in &party.members {
                    let msg = self.build_party_state_for(*cid);
                    self.send_to(*cid, Channel::Control, &msg);
                }
                // System ping into PARTY chat scope.
                let line = format!("{joiner_name} joined the party.");
                for cid in &party.members {
                    self.emit_system_to(*cid, &line);
                }
                log::info!("party: {joiner_name} joined party led by {leader_name}");
            }
            AcceptOutcome::NoSuchInvite => {
                self.party_error(from, "No pending invite to accept.");
            }
            AcceptOutcome::PartyFull => {
                self.party_error(from, "That party is full.");
            }
            AcceptOutcome::InviterGone => {
                self.party_error(from, "That party no longer exists.");
            }
        }
    }

    pub(crate) fn handle_party_decline(&mut self, from: ClientId, which: Option<String>) {
        let Some(row) = self.parties.decline_invite(from, which.as_deref()) else {
            self.party_error(from, "No pending invite to decline.");
            return;
        };
        let invitee_name = self.name_of(from);
        self.emit_system_to(
            row.inviter,
            &format!("{invitee_name} declined your party invite."),
        );
        self.emit_system_to(
            from,
            &format!("Declined the invite from {}.", row.inviter_name),
        );
    }

    pub(crate) fn handle_party_leave(&mut self, from: ClientId) {
        if self.parties.party_of(from).is_none() {
            self.party_error(from, "You're not in a party.");
            return;
        }
        let leaver_name = self.name_of(from);
        // Snapshot the roster before the remove fires so the
        // singleton-collapse path can still notify the
        // orphaned member (their party row is gone post-leave).
        let pre_members: Vec<ClientId> = self
            .parties
            .party_of(from)
            .map(|p| p.members.clone())
            .unwrap_or_default();
        let outcome = self.parties.leave(from);
        // Notify the *remaining* members before fan-out so the
        // SYSTEM line is in their scrollback alongside the new
        // PartyState.
        if let RemoveOutcome::Updated(ref party) = outcome {
            let line = format!("{leaver_name} left the party.");
            for cid in &party.members {
                self.emit_system_to(*cid, &line);
            }
        }
        self.broadcast_party_after_remove(from, outcome, &pre_members);
        // If the leaver was awaiting (or hosting) a portal
        // proposal, tear that down too so nobody is stuck
        // staring at a Confirm modal for a party they no
        // longer belong to.
        self.cancel_portal_proposal_for(from);
    }

    pub(crate) fn handle_party_kick(&mut self, from: ClientId, name: String) {
        if !name_within_limits(&name) {
            self.party_error(from, "That name isn't valid.");
            return;
        }
        let trimmed = name.trim();
        let Some(target) = self.find_session_by_name(trimmed) else {
            self.party_error(from, &format!("No player named {trimmed} is online."));
            return;
        };
        // Snapshot before the kick so the singleton-collapse
        // path (2-person party → 1 → dissolved-to-solo) can
        // still tell the remaining lone member to hide their
        // party UI.
        let pre_members: Vec<ClientId> = self
            .parties
            .party_of(from)
            .map(|p| p.members.clone())
            .unwrap_or_default();
        let Some(outcome) = self.parties.kick(from, target) else {
            self.party_error(from, "Only the party leader can kick members.");
            return;
        };
        let kicker_name = self.name_of(from);
        let target_name = self.name_of(target);
        // Tell the kickee directly.
        self.emit_system_to(
            target,
            &format!("You were kicked from the party by {kicker_name}."),
        );
        // Tell the rest of the party (and the kicker).
        if let RemoveOutcome::Updated(ref party) = outcome {
            let line = format!("{kicker_name} kicked {target_name} from the party.");
            for cid in &party.members {
                self.emit_system_to(*cid, &line);
            }
        }
        self.broadcast_party_after_remove(target, outcome, &pre_members);
        // Same reasoning as `handle_party_leave`: the
        // kickee shouldn't be left holding a stale Confirm
        // modal for a party they were just kicked from.
        self.cancel_portal_proposal_for(target);

        // If the kicked player was inside a rift instance,
        // boot them back to the hub. A kicked player has no
        // business riding along on someone else's run, and
        // staying inside a private instance they were just
        // kicked from would leave them stuck in a party of
        // one (server-side) inside a rift they never picked.
        if self.instance_for_client(target).is_some() {
            log::info!("party: kickee {target:?} ({target_name}) was in a rift; returning to hub");
            self.move_client_to_hub(target);
        }
    }

    pub(crate) fn handle_party_promote(&mut self, from: ClientId, name: String) {
        if !name_within_limits(&name) {
            self.party_error(from, "That name isn't valid.");
            return;
        }
        let trimmed = name.trim();
        let Some(target) = self.find_session_by_name(trimmed) else {
            self.party_error(from, &format!("No player named {trimmed} is online."));
            return;
        };
        let Some(party) = self.parties.promote(from, target) else {
            self.party_error(from, "Only the party leader can promote members.");
            return;
        };
        let new_leader_name = self.name_of(target);
        let line = format!("{new_leader_name} is now the party leader.");
        for cid in &party.members {
            self.emit_system_to(*cid, &line);
            let msg = self.build_party_state_for(*cid);
            self.send_to(*cid, Channel::Control, &msg);
        }
    }
}
