//! Party state + invite management.
//!
//! A *party* is an opt-in grouping of up to [`MAX_PARTY`] clients
//! that share a chat scope and (when in a rift) a private rift
//! instance. Solo players are *not* in a party — there is simply
//! no row for them in the manager. A party comes into being the
//! moment one player's [`InviteRow`] is accepted by another, and
//! is dissolved when the last member leaves.
//!
//! The manager owns no Sim / world state — it's pure bookkeeping.
//! [`crate::Server`] glues it to the rift-instance map (so rift
//! entry can find a party's instance) and to the chat router (so
//! `chat_channel::PARTY` resolves recipients off `members`).
//!
//! ## Invariants
//!
//! * Every member of a `Party` is also keyed in `by_member`.
//! * A `ClientId` appears in `by_member` for at most one party.
//! * `leader` is always present in `members`.
//! * `members.len() <= MAX_PARTY`.
//! * Pending invites TTL out after [`INVITE_TTL`] without a
//!   reply.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use rift_net::messages::MAX_PARTY;
use rift_net::ClientId;

/// Stable party identity. Allocated monotonically inside
/// [`PartyManager`]; never reused for the lifetime of the server
/// process (avoids "you've been added to your old party" bugs
/// when an id loops).
#[derive(Copy, Clone, Hash, Eq, PartialEq, Debug, Default)]
pub struct PartyId(pub u64);

/// One pending /invite waiting for the invitee's `/accept` or
/// `/decline`. Stored in the manager keyed by inviter so the
/// invitee can shorthand `/accept` (most-recent invite) or
/// `/accept <name>` (specific inviter).
#[derive(Clone, Debug)]
pub struct InviteRow {
    pub inviter: ClientId,
    pub inviter_name: String,
    pub created_at: Instant,
}

/// How long a `/invite` survives without a reply before the
/// manager auto-evicts the row. Long enough for a player to
/// finish a chat conversation and react; short enough that
/// stale invites don't pile up across hours of online time.
pub const INVITE_TTL: Duration = Duration::from_secs(60);

/// One party's bookkeeping. Members are stored in join order
/// with the leader pinned at index 0. The `members` vec is the
/// authoritative roster the chat router and rift entry path
/// scan; `leader` is a convenience pointer into the same vec.
#[derive(Clone, Debug)]
pub struct Party {
    pub leader: ClientId,
    pub members: Vec<ClientId>,
}

/// Top-level party state. Owns every active [`Party`] +
/// [`InviteRow`] and the `ClientId → PartyId` reverse index.
#[derive(Default)]
pub struct PartyManager {
    parties: HashMap<PartyId, Party>,
    by_member: HashMap<ClientId, PartyId>,
    /// Pending invites keyed by `(inviter, invitee)` so a
    /// re-invite from the same player simply refreshes the
    /// timestamp instead of stacking duplicate rows.
    pending: HashMap<(ClientId, ClientId), InviteRow>,
    /// Monotonic id generator. Starts at 1 so a default-
    /// constructed `PartyId(0)` can stand in for "no party"
    /// in test code without colliding with a real party.
    next_id: u64,
}

/// Outcome of an `/invite` after the manager has had a chance
/// to validate the request. Server uses this to decide whether
/// to push a [`rift_net::ServerMsg::PartyInviteIncoming`] toast
/// to the invitee or a [`rift_net::ServerMsg::PartyError`] back
/// to the inviter.
#[derive(Debug)]
pub enum InviteOutcome {
    /// New invite recorded. Server should toast the invitee.
    Recorded,
    /// Same `(inviter, invitee)` invite already exists; the
    /// timestamp has been refreshed so it doesn't TTL out.
    /// No toast — the invitee already has one.
    Refreshed,
    /// Inviter and invitee are already in the same party.
    AlreadySameParty,
    /// Invitee is already in some *other* party. They have to
    /// `/leave` first.
    InviteeAlreadyInParty,
    /// Inviter's party is at [`MAX_PARTY`].
    InviterPartyFull,
}

/// Outcome of an `/accept`. `Joined` carries the resulting
/// party so the server can broadcast a fresh `PartyState` to
/// every member.
#[derive(Debug)]
pub enum AcceptOutcome {
    Joined(Party),
    /// No matching pending invite (either expired or never
    /// existed).
    NoSuchInvite,
    /// Invite is valid but the inviter's party is full now
    /// (someone else accepted first).
    PartyFull,
    /// Inviter has logged out / left their own party between
    /// invite and accept.
    InviterGone,
}

/// Outcome of a `/leave`, `/kick`, or disconnect-driven
/// removal. Carries the post-removal party (or `None` if the
/// removal dissolved it) so the server knows whether to
/// broadcast a fresh `PartyState` or one final empty
/// `PartyState`.
#[derive(Debug)]
pub enum RemoveOutcome {
    Solo,
    Updated(Party),
    Dissolved,
}

impl PartyManager {
    pub fn new() -> Self {
        Self {
            parties: HashMap::new(),
            by_member: HashMap::new(),
            pending: HashMap::new(),
            next_id: 1,
        }
    }

    /// Lookup the party containing `cid`, if any. `None` means
    /// the client is solo.
    pub fn party_of(&self, cid: ClientId) -> Option<&Party> {
        self.by_member.get(&cid).and_then(|id| self.parties.get(id))
    }

    /// Drop expired invite rows. Called once per server step
    /// before any party-mutating handler so the TTL is enforced
    /// independent of new traffic.
    pub fn evict_expired_invites(&mut self, now: Instant) {
        self.pending.retain(|_, row| now - row.created_at < INVITE_TTL);
    }

    /// Record an invite from `inviter` (named `inviter_name`
    /// for the toast) to `invitee`. See [`InviteOutcome`] for
    /// every refusal path.
    ///
    /// Doesn't validate "either side is in a rift" — that gate
    /// lives on the `Server` so it can read `client_instance`
    /// without taking `&PartyManager` across both borrows.
    pub fn record_invite(
        &mut self,
        inviter: ClientId,
        inviter_name: String,
        invitee: ClientId,
        now: Instant,
    ) -> InviteOutcome {
        if let Some(p) = self.party_of(inviter) {
            if p.members.contains(&invitee) {
                return InviteOutcome::AlreadySameParty;
            }
            if p.members.len() >= MAX_PARTY as usize {
                return InviteOutcome::InviterPartyFull;
            }
        }
        // Invitee already in *some* party (could be solo, in
        // which case `party_of` is None and we fall through).
        if self.party_of(invitee).is_some() {
            return InviteOutcome::InviteeAlreadyInParty;
        }
        let key = (inviter, invitee);
        if let Some(existing) = self.pending.get_mut(&key) {
            existing.created_at = now;
            existing.inviter_name = inviter_name;
            return InviteOutcome::Refreshed;
        }
        self.pending.insert(
            key,
            InviteRow {
                inviter,
                inviter_name,
                created_at: now,
            },
        );
        InviteOutcome::Recorded
    }

    /// Pull the most recent invite to `invitee`, optionally
    /// filtered by `from` (case-insensitive on the inviter
    /// name). Removes the matched row.
    fn take_invite(
        &mut self,
        invitee: ClientId,
        from: Option<&str>,
    ) -> Option<InviteRow> {
        let needle = from.map(|s| s.to_ascii_lowercase());
        let key = self
            .pending
            .iter()
            .filter(|((_, to), row)| {
                *to == invitee
                    && needle
                        .as_ref()
                        .map(|n| row.inviter_name.to_ascii_lowercase() == *n)
                        .unwrap_or(true)
            })
            .max_by_key(|(_, row)| row.created_at)
            .map(|(k, _)| *k);
        key.and_then(|k| self.pending.remove(&k))
    }

    /// Accept the most recent invite to `invitee`. On success
    /// merges them into the inviter's party (creating the party
    /// if the inviter is currently solo).
    pub fn accept_invite(
        &mut self,
        invitee: ClientId,
        from: Option<&str>,
    ) -> AcceptOutcome {
        let Some(row) = self.take_invite(invitee, from) else {
            return AcceptOutcome::NoSuchInvite;
        };
        // Inviter may have leave-d their own party between
        // invite and accept; in that case create a fresh
        // party from the inviter+invitee pair.
        let pid = match self.by_member.get(&row.inviter).copied() {
            Some(pid) => pid,
            None => {
                let pid = PartyId(self.next_id);
                self.next_id += 1;
                self.parties.insert(
                    pid,
                    Party {
                        leader: row.inviter,
                        members: vec![row.inviter],
                    },
                );
                self.by_member.insert(row.inviter, pid);
                pid
            }
        };
        let Some(party) = self.parties.get_mut(&pid) else {
            return AcceptOutcome::InviterGone;
        };
        if party.members.len() >= MAX_PARTY as usize {
            return AcceptOutcome::PartyFull;
        }
        if !party.members.contains(&invitee) {
            party.members.push(invitee);
            self.by_member.insert(invitee, pid);
        }
        // Drop any other pending invites to this invitee — they
        // are now in a party and can't accept a competing one
        // until they /leave.
        self.pending.retain(|(_, to), _| *to != invitee);
        AcceptOutcome::Joined(party.clone())
    }

    /// Decline the matching invite. Returns the inviter's
    /// `ClientId` so the server can drop a "X declined your
    /// invite" toast. `None` if the invite no longer exists.
    pub fn decline_invite(
        &mut self,
        invitee: ClientId,
        from: Option<&str>,
    ) -> Option<InviteRow> {
        self.take_invite(invitee, from)
    }

    /// Remove `cid` from whatever party they belong to. Hands
    /// off leadership to the next member if `cid` was the
    /// leader; dissolves the party if `cid` was the only
    /// member.
    pub fn leave(&mut self, cid: ClientId) -> RemoveOutcome {
        let Some(pid) = self.by_member.remove(&cid) else {
            return RemoveOutcome::Solo;
        };
        // Also clear any pending invites involving this
        // client — a player who logs out / leaves the party
        // shouldn't keep dangling rows around.
        self.pending
            .retain(|(inviter, invitee), _| *inviter != cid && *invitee != cid);
        let Some(party) = self.parties.get_mut(&pid) else {
            return RemoveOutcome::Solo;
        };
        party.members.retain(|m| *m != cid);
        if party.members.is_empty() {
            self.parties.remove(&pid);
            return RemoveOutcome::Dissolved;
        }
        if party.leader == cid {
            // Promote the longest-serving remaining member.
            party.leader = party.members[0];
        }
        let snapshot = party.clone();
        if snapshot.members.len() == 1 {
            // Singleton parties don't earn their keep — drop
            // the row so the lone member is back to "solo"
            // (no party row at all). This keeps the
            // `party_of(cid).is_none() == solo` invariant.
            let lone = snapshot.members[0];
            self.parties.remove(&pid);
            self.by_member.remove(&lone);
            RemoveOutcome::Solo
        } else {
            RemoveOutcome::Updated(snapshot)
        }
    }

    /// Leader-only kick. Returns `Some(updated)` on success or
    /// `None` if the caller wasn't the leader / target wasn't
    /// in the party.
    pub fn kick(
        &mut self,
        leader: ClientId,
        target: ClientId,
    ) -> Option<RemoveOutcome> {
        let pid = self.by_member.get(&leader).copied()?;
        let party = self.parties.get(&pid)?;
        if party.leader != leader || !party.members.contains(&target) || target == leader {
            return None;
        }
        Some(self.leave(target))
    }

    /// Leader-only promote. Returns `Some(updated)` on success.
    pub fn promote(
        &mut self,
        leader: ClientId,
        target: ClientId,
    ) -> Option<Party> {
        let pid = self.by_member.get(&leader).copied()?;
        let party = self.parties.get_mut(&pid)?;
        if party.leader != leader || !party.members.contains(&target) || target == leader {
            return None;
        }
        party.leader = target;
        // Re-order so the new leader is at index 0 (clients
        // render members in `members` order).
        party.members.retain(|m| *m != target);
        party.members.insert(0, target);
        Some(party.clone())
    }
}

#[cfg(test)]
mod tests {
    //! Pure-data tests for `PartyManager`. Covers the
    //! singleton-collapse path that drove the kick-while-in-
    //! rift PartyState bug, the leader-handoff after a
    //! leader leaves a 3-person party, kick / promote
    //! permission gates, and invite TTL eviction.
    use super::*;

    fn cid(raw: u64) -> ClientId {
        ClientId(raw)
    }

    /// Helper: make a party of `n` members with cid `1..=n`,
    /// led by cid 1. Uses synthetic invite-then-accept so the
    /// internal `by_member` index stays consistent with what
    /// real traffic produces.
    fn party_of_n(mgr: &mut PartyManager, n: u64, now: Instant) {
        for member in 2..=n {
            let _ = mgr.record_invite(cid(1), "leader".into(), cid(member), now);
            let outcome = mgr.accept_invite(cid(member), None);
            assert!(
                matches!(outcome, AcceptOutcome::Joined(_)),
                "invite #{member} should join, got {outcome:?}"
            );
        }
    }

    #[test]
    fn solo_party_has_no_row() {
        let mgr = PartyManager::new();
        assert!(mgr.party_of(cid(1)).is_none());
    }

    #[test]
    fn invite_then_accept_creates_party() {
        let mut mgr = PartyManager::new();
        let now = Instant::now();
        let r = mgr.record_invite(cid(1), "alpha".into(), cid(2), now);
        assert!(matches!(r, InviteOutcome::Recorded));
        let r = mgr.accept_invite(cid(2), None);
        let AcceptOutcome::Joined(party) = r else {
            panic!("expected Joined, got {r:?}");
        };
        assert_eq!(party.leader, cid(1));
        assert_eq!(party.members, vec![cid(1), cid(2)]);
        assert_eq!(mgr.party_of(cid(1)).map(|p| p.leader), Some(cid(1)));
        assert_eq!(mgr.party_of(cid(2)).map(|p| p.leader), Some(cid(1)));
    }

    #[test]
    fn invite_to_already_partied_player_refused() {
        // 1+2 form a party; 3 then tries to invite 2.
        let mut mgr = PartyManager::new();
        let now = Instant::now();
        party_of_n(&mut mgr, 2, now);
        let r = mgr.record_invite(cid(3), "outsider".into(), cid(2), now);
        assert!(matches!(r, InviteOutcome::InviteeAlreadyInParty));
    }

    #[test]
    fn duplicate_invite_refreshes_timestamp() {
        let mut mgr = PartyManager::new();
        let t0 = Instant::now();
        let _ = mgr.record_invite(cid(1), "alpha".into(), cid(2), t0);
        let t1 = t0 + Duration::from_secs(5);
        let r = mgr.record_invite(cid(1), "alpha".into(), cid(2), t1);
        assert!(matches!(r, InviteOutcome::Refreshed));
    }

    #[test]
    fn invite_ttl_evicts_stale_rows() {
        let mut mgr = PartyManager::new();
        let t0 = Instant::now();
        let _ = mgr.record_invite(cid(1), "alpha".into(), cid(2), t0);
        // Just under TTL: still there.
        mgr.evict_expired_invites(t0 + INVITE_TTL - Duration::from_millis(1));
        let r = mgr.accept_invite(cid(2), None);
        assert!(matches!(r, AcceptOutcome::Joined(_)));

        // Now a fresh invite that we'll let TTL out.
        let mut mgr = PartyManager::new();
        let _ = mgr.record_invite(cid(1), "alpha".into(), cid(2), t0);
        mgr.evict_expired_invites(t0 + INVITE_TTL + Duration::from_millis(1));
        let r = mgr.accept_invite(cid(2), None);
        assert!(matches!(r, AcceptOutcome::NoSuchInvite));
    }

    #[test]
    fn leave_two_person_party_collapses_remaining_to_solo() {
        // Regression: 2-person party where one member leaves
        // (or is kicked) used to leave the lone remainder
        // stuck in a singleton party with stale roster on
        // their UI. Manager must return `Solo` and drop the
        // party row entirely.
        let mut mgr = PartyManager::new();
        let now = Instant::now();
        party_of_n(&mut mgr, 2, now);
        let outcome = mgr.leave(cid(2));
        assert!(
            matches!(outcome, RemoveOutcome::Solo),
            "expected Solo (singleton collapsed), got {outcome:?}"
        );
        // Both clients should now be party-less.
        assert!(mgr.party_of(cid(1)).is_none(), "leader should be solo");
        assert!(mgr.party_of(cid(2)).is_none(), "leaver should be solo");
    }

    #[test]
    fn kick_two_person_party_collapses_to_solo() {
        // Regression mirror of the leave test, via /kick.
        // Same singleton-collapse logic must fire.
        let mut mgr = PartyManager::new();
        let now = Instant::now();
        party_of_n(&mut mgr, 2, now);
        let outcome = mgr.kick(cid(1), cid(2)).expect("leader kick");
        assert!(matches!(outcome, RemoveOutcome::Solo));
        assert!(mgr.party_of(cid(1)).is_none());
        assert!(mgr.party_of(cid(2)).is_none());
    }

    #[test]
    fn leader_leaves_three_person_party_promotes_next_member() {
        let mut mgr = PartyManager::new();
        let now = Instant::now();
        party_of_n(&mut mgr, 3, now);
        let outcome = mgr.leave(cid(1));
        let RemoveOutcome::Updated(party) = outcome else {
            panic!("expected Updated, got {outcome:?}");
        };
        // Surviving members are 2 and 3; the longest-serving
        // (cid 2) should inherit leadership.
        assert_eq!(party.leader, cid(2));
        assert_eq!(party.members, vec![cid(2), cid(3)]);
    }

    #[test]
    fn non_leader_cannot_kick() {
        let mut mgr = PartyManager::new();
        let now = Instant::now();
        party_of_n(&mut mgr, 3, now);
        // 2 is not the leader; this kick must refuse.
        assert!(mgr.kick(cid(2), cid(3)).is_none());
        // Roster unchanged.
        let party = mgr.party_of(cid(1)).unwrap();
        assert_eq!(party.members.len(), 3);
    }

    #[test]
    fn leader_cannot_kick_self() {
        let mut mgr = PartyManager::new();
        let now = Instant::now();
        party_of_n(&mut mgr, 2, now);
        assert!(mgr.kick(cid(1), cid(1)).is_none());
    }

    #[test]
    fn promote_moves_target_to_index_zero() {
        let mut mgr = PartyManager::new();
        let now = Instant::now();
        party_of_n(&mut mgr, 3, now);
        let party = mgr.promote(cid(1), cid(3)).expect("leader promote");
        assert_eq!(party.leader, cid(3));
        assert_eq!(party.members[0], cid(3));
    }

    #[test]
    fn non_leader_cannot_promote() {
        let mut mgr = PartyManager::new();
        let now = Instant::now();
        party_of_n(&mut mgr, 3, now);
        assert!(mgr.promote(cid(2), cid(3)).is_none());
    }

    #[test]
    fn party_full_invite_refused() {
        // MAX_PARTY currently 4; build to capacity then probe.
        let mut mgr = PartyManager::new();
        let now = Instant::now();
        party_of_n(&mut mgr, MAX_PARTY as u64, now);
        let r = mgr.record_invite(
            cid(1),
            "alpha".into(),
            cid(MAX_PARTY as u64 + 1),
            now,
        );
        assert!(matches!(r, InviteOutcome::InviterPartyFull));
    }

    #[test]
    fn accept_into_full_party_returns_party_full() {
        // Race: invite is recorded while there's still a
        // slot, but a competing accept fills it before this
        // one resolves.
        let mut mgr = PartyManager::new();
        let now = Instant::now();
        party_of_n(&mut mgr, MAX_PARTY as u64 - 1, now);
        // Record an invite for one more slot; then fill the
        // party via direct accept of a *different* invite.
        let _ = mgr.record_invite(
            cid(1),
            "alpha".into(),
            cid(MAX_PARTY as u64),
            now,
        );
        let _ = mgr.record_invite(
            cid(1),
            "alpha".into(),
            cid(MAX_PARTY as u64 + 1),
            now,
        );
        let r = mgr.accept_invite(cid(MAX_PARTY as u64), None);
        assert!(matches!(r, AcceptOutcome::Joined(_)));
        let r = mgr.accept_invite(cid(MAX_PARTY as u64 + 1), None);
        assert!(
            matches!(r, AcceptOutcome::PartyFull),
            "expected PartyFull, got {r:?}"
        );
    }

    #[test]
    fn decline_removes_invite() {
        let mut mgr = PartyManager::new();
        let now = Instant::now();
        let _ = mgr.record_invite(cid(1), "alpha".into(), cid(2), now);
        let row = mgr.decline_invite(cid(2), None);
        assert!(row.is_some());
        // Subsequent accept finds nothing.
        let r = mgr.accept_invite(cid(2), None);
        assert!(matches!(r, AcceptOutcome::NoSuchInvite));
    }

    #[test]
    fn accept_drops_competing_invites_to_same_invitee() {
        // Two parties invite the same player; once the player
        // accepts one, the other's invite row should be
        // dropped so a stale `/accept other` later doesn't
        // pull them into a second party.
        let mut mgr = PartyManager::new();
        let now = Instant::now();
        // Party A (1+3 first to give 1 a party row).
        let _ = mgr.record_invite(cid(1), "alpha".into(), cid(3), now);
        let _ = mgr.accept_invite(cid(3), None);
        // Party B (4+5 first).
        let _ = mgr.record_invite(cid(4), "bravo".into(), cid(5), now);
        let _ = mgr.accept_invite(cid(5), None);

        // Both parties invite cid(2).
        let _ = mgr.record_invite(cid(1), "alpha".into(), cid(2), now);
        let _ = mgr.record_invite(cid(4), "bravo".into(), cid(2), now);
        // cid(2) accepts party A.
        let r = mgr.accept_invite(cid(2), Some("alpha"));
        assert!(matches!(r, AcceptOutcome::Joined(_)));
        // Stray accept for party B must now fail \u2014 the row
        // was dropped.
        let r = mgr.accept_invite(cid(2), Some("bravo"));
        assert!(
            matches!(r, AcceptOutcome::NoSuchInvite),
            "expected NoSuchInvite, got {r:?}"
        );
    }
}
