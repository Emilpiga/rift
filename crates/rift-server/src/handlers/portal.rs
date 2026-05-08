//! Portal-modal proposal flow.
//!
//! When a player presses F at the rift portal the client opens
//! a modal letting them pick:
//! - **Solo** — spin up a new private 1-cap instance for them
//!   only.
//! - **Party** — spin up a new private instance for the whole
//!   party. Every other party member sees a confirm modal
//!   ([`ServerMsg::PortalPrompt`]); they ride along iff they
//!   reply with [`ClientMsg::PortalConfirm { accept: true }`]
//!   inside [`PROMPT_TIMEOUT`].
//! - **Matchmade** — join the first open matchmaking instance
//!   on the chosen `start_floor`, or open a new one. The
//!   proposer's party (after opt-in, like Party) ports in
//!   together; remaining slots are filled by other matchmade
//!   proposers.
//!
//! Server validates `start_floor` against the *minimum*
//! `deepest_cleared_floor + 1` of the proposing party so
//! nobody is dragged past their cleared content.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use rift_net::messages::{party_mode, ServerMsg};
use rift_net::{Channel, ClientId};

use crate::instance::RiftInstanceId;
use crate::Server;

/// How long a [`PendingProposal`] waits for confirmations
/// before the server auto-resolves with whoever has accepted
/// so far. Matches the descend / exit vote feel.
pub const PROMPT_TIMEOUT: Duration = Duration::from_secs(30);

/// One in-flight portal proposal, indexed by proposer
/// [`ClientId`] in [`crate::Server::pending_portal_proposals`].
/// Carries everything `tick_pending_portal_proposals` needs to
/// resolve the proposal once timeouts or replies have rolled
/// in.
pub struct PendingProposal {
    pub start_floor: u32,
    pub mode: u8,
    pub created_at: Instant,
    /// Member ClientIds the modal was sent to (the proposer's
    /// party minus the proposer themselves). Drained as
    /// `awaiting` shrinks.
    pub awaiting: HashSet<ClientId>,
    /// Members who replied `accept = true`. Always includes
    /// the proposer (auto-confirmed at proposal time).
    pub confirmed: HashSet<ClientId>,
}

impl Server {
    /// Cap `start_floor` against the proposing party's
    /// minimum `deepest_cleared_floor + 1`. `1` floor (Solo)
    /// behaves like a party of one — nobody is held back.
    fn cap_start_floor(&self, proposer: ClientId, requested: u32) -> u32 {
        let mut min_cleared = self
            .sessions
            .get(proposer)
            .and_then(|s| s.record.as_ref())
            .map(|r| r.deepest_cleared_floor.max(0) as u32)
            .unwrap_or(0);
        if let Some(party) = self.parties.party_of(proposer) {
            for cid in &party.members {
                let v = self
                    .sessions
                    .get(*cid)
                    .and_then(|s| s.record.as_ref())
                    .map(|r| r.deepest_cleared_floor.max(0) as u32)
                    .unwrap_or(0);
                if v < min_cleared {
                    min_cleared = v;
                }
            }
        }
        let cap = min_cleared.saturating_add(1).max(1);
        requested.clamp(1, cap)
    }

    pub(crate) fn handle_propose_rift_entry(
        &mut self,
        from: ClientId,
        start_floor: u32,
        mode: u8,
    ) {
        log::info!(
            "portal: ProposeRiftEntry from {from:?} floor={start_floor} mode={mode}"
        );
        if self.instance_for_client(from).is_some() {
            self.party_error_one(from, "You're already inside a rift.");
            return;
        }
        // If the proposer already has an in-flight proposal
        // (re-press of F at the portal before the previous
        // round resolved) close the old awaiting modals
        // before opening fresh ones \u2014 otherwise the previous
        // peers see two stacked confirm prompts.
        self.cancel_portal_proposal_for(from);
        let start_floor = self.cap_start_floor(from, start_floor);

        // Solo always bypasses the party-confirm prompt — even
        // if the proposer is in a party. Solo means "drop
        // everyone else, run alone".
        if mode == party_mode::SOLO || self.parties.party_of(from).is_none() {
            log::info!(
                "portal: bypass-prompt path (mode_solo={} party_of_none={}) -> start_run",
                mode == party_mode::SOLO,
                self.parties.party_of(from).is_none()
            );
            self.start_run(from, start_floor, mode, vec![from]);
            return;
        }

        // Party / Matchmade: collect every other member into
        // `awaiting` and push the modal. Members who don't
        // reply inside PROMPT_TIMEOUT are treated as decline.
        let party = self.parties.party_of(from).cloned();
        let other_members: Vec<ClientId> = party
            .as_ref()
            .map(|p| p.members.iter().copied().filter(|m| *m != from).collect())
            .unwrap_or_default();

        // No other members? Shouldn't happen since we just
        // checked `party_of(from).is_none()` above, but if a
        // singleton party slipped in just start the run.
        if other_members.is_empty() {
            log::info!(
                "portal: party-of-one fallback (party.members={:?}) -> start_run",
                party.as_ref().map(|p| p.members.clone()).unwrap_or_default(),
            );
            self.start_run(from, start_floor, mode, vec![from]);
            return;
        }

        log::info!(
            "portal: prompt-path proposer={from:?} other_members={other_members:?}"
        );

        let proposer_name = self
            .sessions
            .get(from)
            .and_then(|s| s.character_name.clone())
            .unwrap_or_else(|| "A player".to_string());
        let prompt = ServerMsg::PortalPrompt {
            proposer: proposer_name,
            start_floor,
            mode,
            seconds_remaining: PROMPT_TIMEOUT.as_secs() as u32,
        };
        for cid in &other_members {
            self.send_to(*cid, Channel::Control, &prompt);
        }
        let mut awaiting: HashSet<ClientId> = other_members.iter().copied().collect();
        awaiting.remove(&from);
        let mut confirmed = HashSet::new();
        confirmed.insert(from);
        self.pending_portal_proposals.insert(
            from,
            PendingProposal {
                start_floor,
                mode,
                created_at: Instant::now(),
                awaiting,
                confirmed,
            },
        );
    }

    pub(crate) fn handle_portal_confirm(&mut self, from: ClientId, accept: bool) {
        log::info!("portal: PortalConfirm from {from:?} accept={accept}");
        // Find the proposal we belong to. A confirm from a
        // member with no matching proposal is silently
        // dropped — happens if the proposer cancelled or the
        // proposal already resolved.
        let proposer_key = self
            .pending_portal_proposals
            .iter()
            .find_map(|(k, p)| if p.awaiting.contains(&from) { Some(*k) } else { None });
        let Some(proposer) = proposer_key else { return };
        let resolved_now;
        {
            let Some(prop) = self.pending_portal_proposals.get_mut(&proposer) else {
                return;
            };
            prop.awaiting.remove(&from);
            if accept {
                prop.confirmed.insert(from);
            }
            resolved_now = prop.awaiting.is_empty();
        }
        if !accept {
            // Tell the decliner the modal is closed for them.
            self.send_to(from, Channel::Control, &ServerMsg::PortalPromptClosed);
        }
        if resolved_now {
            self.resolve_portal_proposal(proposer);
        }
    }

    /// Per-tick: time out any proposal whose 30 s window has
    /// elapsed. Called from the main loop alongside the chat
    /// invite TTL eviction.
    pub(crate) fn tick_portal_proposals(&mut self, now: Instant) {
        let expired: Vec<ClientId> = self
            .pending_portal_proposals
            .iter()
            .filter_map(|(k, p)| {
                if now.duration_since(p.created_at) >= PROMPT_TIMEOUT {
                    Some(*k)
                } else {
                    None
                }
            })
            .collect();
        for k in expired {
            self.resolve_portal_proposal(k);
        }
    }

    fn resolve_portal_proposal(&mut self, proposer: ClientId) {
        let Some(prop) = self.pending_portal_proposals.remove(&proposer) else {
            return;
        };
        log::info!(
            "portal: resolve proposer={proposer:?} confirmed={:?} timed_out_awaiting={:?}",
            prop.confirmed,
            prop.awaiting,
        );
        // Tell every still-awaiting member their modal is gone
        // (treat them as decline by default).
        for cid in &prop.awaiting {
            self.send_to(*cid, Channel::Control, &ServerMsg::PortalPromptClosed);
        }
        // Abort the proposal entirely if every other member
        // either declined or timed out. The proposer is
        // always in `confirmed` (auto-confirmed at proposal
        // time), so "everyone else said no" is the
        // `confirmed.len() == 1 && proposer in confirmed`
        // case. Surface a system-line + PartyError toast so
        // the proposer understands why they weren't dropped
        // into a rift.
        if prop.confirmed.len() == 1 && prop.confirmed.contains(&proposer) {
            log::info!(
                "portal: aborting proposer={proposer:?} - no party member accepted"
            );
            self.party_error_one(
                proposer,
                "Nobody accepted the rift entry.",
            );
            return;
        }
        let confirmed: Vec<ClientId> = prop.confirmed.iter().copied().collect();
        self.start_run(proposer, prop.start_floor, prop.mode, confirmed);
    }

    /// Common path: spin up (or join) the right instance and
    /// move every confirmed member into it.
    fn start_run(
        &mut self,
        proposer: ClientId,
        start_floor: u32,
        mode: u8,
        confirmed: Vec<ClientId>,
    ) {
        // Filter to only members still hub-side and online.
        let movers: Vec<ClientId> = confirmed
            .into_iter()
            .filter(|cid| {
                self.sessions.get(*cid).is_some()
                    && self.instance_for_client(*cid).is_none()
            })
            .collect();
        if movers.is_empty() {
            return;
        }

        let seed: u64 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
            ^ proposer.0.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let instance_id: RiftInstanceId = match mode {
            party_mode::MATCHMAKE => {
                // Look up an open matchmade instance with the
                // right start floor, falling back to a fresh
                // one if none has room.
                let live_counts: std::collections::HashMap<
                    RiftInstanceId,
                    u8,
                > = {
                    let mut m = std::collections::HashMap::new();
                    for (id, _) in self.instances.iter() {
                        m.insert(
                            *id,
                            self.clients_in_instance(*id).len() as u8,
                        );
                    }
                    m
                };
                let count_lookup = |id: RiftInstanceId| {
                    live_counts.get(&id).copied().unwrap_or(0)
                };
                if let Some(existing) = self
                    .instances
                    .find_open_matchmade(start_floor, count_lookup)
                {
                    existing
                } else {
                    self.instances
                        .create_matchmade(start_floor, seed)
                }
            }
            _ => {
                // Solo / Party = Private instance sized to
                // the confirmed mover count.
                let capacity = movers.len().clamp(1, rift_net::messages::MAX_PARTY as usize) as u8;
                self.instances
                    .create_private(start_floor, capacity, seed)
            }
        };

        // Cap movers at remaining capacity (matchmade fill).
        let already_in = self.clients_in_instance(instance_id).len() as u8;
        let cap = self
            .instances
            .get(instance_id)
            .map(|i| i.capacity)
            .unwrap_or(rift_net::messages::MAX_PARTY);
        let remaining = cap.saturating_sub(already_in) as usize;
        for cid in movers.into_iter().take(remaining) {
            self.move_client_to_instance(cid, instance_id);
        }
    }

    /// One-shot system error to a single client. Wraps
    /// `emit_system_to` so the portal handler can use the
    /// same error surface as `handlers/party.rs` without a
    /// cross-module call chain.
    fn party_error_one(&mut self, to: ClientId, reason: &str) {
        let msg = ServerMsg::PartyError {
            reason: reason.to_string(),
        };
        self.send_to(to, Channel::Control, &msg);
        self.emit_system_to(to, reason);
    }

    /// Disconnect / re-propose / kick cleanup. Two paths:
    ///   * `cid` is the **proposer** of an in-flight
    ///     proposal \u2014 drop the proposal and tell every
    ///     awaiting member their modal is gone.
    ///   * `cid` is an **awaiting confirmer** in someone
    ///     else's proposal \u2014 remove them from `awaiting`.
    ///     If that empties the set, resolve the proposal
    ///     immediately rather than letting it sit for the
    ///     full 30 s timeout.
    /// Idempotent: a no-op if `cid` is not involved in any
    /// proposal.
    pub(crate) fn cancel_portal_proposal_for(&mut self, cid: ClientId) {
        // Path 1: proposer.
        if let Some(prop) = self.pending_portal_proposals.remove(&cid) {
            log::info!(
                "portal: cancel proposer={cid:?} awaiting={:?}",
                prop.awaiting,
            );
            for awaiting in &prop.awaiting {
                self.send_to(*awaiting, Channel::Control, &ServerMsg::PortalPromptClosed);
            }
            // Don't fall through \u2014 a proposer can't also be
            // awaiting their own proposal.
            return;
        }
        // Path 2: awaiting confirmer in someone else's proposal.
        let host = self
            .pending_portal_proposals
            .iter()
            .find_map(|(k, p)| if p.awaiting.contains(&cid) { Some(*k) } else { None });
        let Some(host) = host else { return };
        let resolved_now;
        {
            let Some(prop) = self.pending_portal_proposals.get_mut(&host) else {
                return;
            };
            prop.awaiting.remove(&cid);
            resolved_now = prop.awaiting.is_empty();
            log::info!(
                "portal: drop awaiting cid={cid:?} from proposer={host:?} resolved_now={resolved_now}"
            );
        }
        if resolved_now {
            self.resolve_portal_proposal(host);
        }
    }
}
