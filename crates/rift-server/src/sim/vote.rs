//! Rift exit vote state machine.
//!
//! When a living player presses F at the rift-spawn portal, the
//! client sends [`rift_net::ClientMsg::RiftExitVoteStart`]. The
//! sim resolves it via [`Sim::request_exit_vote`]:
//!
//! - **Solo** (one connected player): instant exit. Caller does
//!   not call into this module — the sim short-circuits to the
//!   wipe-and-transition path directly.
//! - **Multiplayer** (2+ connected players): opens a 15s vote
//!   window, auto-records the initiator as `Yes`, returns a
//!   freshly-built [`VoteState`] for the main loop to broadcast.
//!
//! While the window is open, [`Sim::cast_exit_vote`] mutates the
//! roll. Each tick, [`Sim::tick_exit_vote`] decrements the
//! deadline and resolves once one of:
//!
//! - **All living voters voted Yes** → exit succeeds. Caller
//!   wipes ghost loot and transitions everyone to the hub.
//! - **Any voter voted No** → fizzle, 60s cooldown.
//! - **Deadline elapsed** with at least one Pending → fizzle.
//!
//! The `dirty` flag is raised on every state-changing op so the
//! main loop knows when to broadcast a fresh `RiftExitVote`.

use std::collections::HashMap;

use rift_net::{
    messages::{VoteChoice, VoteKind, VoteState},
    NetId,
};

/// Active vote-window length (seconds).
pub const VOTE_DURATION: f32 = 15.0;
/// Minimum gap between vote attempts after a fizzle (seconds).
pub const VOTE_COOLDOWN: f32 = 60.0;

/// Active vote-window state. `None` on [`Sim`] when no vote is
/// open.
#[derive(Clone, Debug)]
pub struct ExitVote {
    /// What this vote is asking the party to decide. Carried
    /// through to the wire `VoteState` so the HUD title is
    /// correct and to [`TickOutcome::Passed`] so the main loop
    /// knows whether to descend or return to the hub on a Yes
    /// resolution.
    pub kind: VoteKind,
    /// Seconds remaining on the active window. Counts down to
    /// zero in [`Sim::tick_exit_vote`]; resolution happens at
    /// or below zero.
    pub time_remaining: f32,
    /// One row per *living* player on the rift floor at the
    /// moment the vote opened. The map preserves identity
    /// (same `NetId`s appear across `RiftExitVote` broadcasts);
    /// ordering for the wire payload is rebuilt in
    /// [`build_state`] sorted by `NetId.0` ascending.
    pub votes: HashMap<NetId, VoteChoice>,
}

/// Outcome of a single [`Sim::tick_exit_vote`] step. Returned to
/// the main loop so it knows how to react to the resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickOutcome {
    /// Either no vote is open, or the open vote has not yet
    /// resolved this tick.
    Idle,
    /// Vote resolved with unanimous Yes. The kind selects the
    /// caller's response: `Exit` wipes ghost loot and
    /// transitions to the hub; `Descend` advances to the next
    /// rift floor.
    Passed(VoteKind),
    /// Vote resolved with a No or expired with pending votes.
    /// Caller arms the cooldown.
    Fizzled,
}

/// Build a wire-shape [`VoteState`] from the sim's internal
/// representation. Voter rows are sorted by `NetId.0` so the
/// HUD layout is stable across broadcasts.
pub fn build_state(
    active: Option<&ExitVote>,
    cooldown_remaining: f32,
) -> VoteState {
    if let Some(v) = active {
        let mut voters: Vec<(NetId, VoteChoice)> = v
            .votes
            .iter()
            .map(|(nid, c)| (*nid, *c))
            .collect();
        voters.sort_by_key(|(nid, _)| nid.0);
        VoteState {
            kind: v.kind,
            active: true,
            time_remaining: v.time_remaining.max(0.0),
            cooldown_remaining: cooldown_remaining.max(0.0),
            voters,
        }
    } else {
        VoteState {
            // Idle state has no "current" kind; the HUD ignores
            // it whenever `active` is false. Default to Exit
            // for backwards-compatible cooldown-banner display.
            kind: VoteKind::Exit,
            active: false,
            time_remaining: 0.0,
            cooldown_remaining: cooldown_remaining.max(0.0),
            voters: Vec::new(),
        }
    }
}

/// Resolve an active vote: returns `Passed` if every vote is
/// `Yes`, `Fizzled` if any is `No` or any are still `Pending`
/// when the deadline elapses, or `Idle` if neither condition
/// holds yet.
pub fn resolve(vote: &ExitVote) -> TickOutcome {
    let mut any_no = false;
    let mut any_pending = false;
    for c in vote.votes.values() {
        match c {
            VoteChoice::No => any_no = true,
            VoteChoice::Pending => any_pending = true,
            VoteChoice::Yes => {}
        }
    }
    if any_no {
        return TickOutcome::Fizzled;
    }
    if any_pending {
        if vote.time_remaining <= 0.0 {
            TickOutcome::Fizzled
        } else {
            TickOutcome::Idle
        }
    } else {
        // No `No`s, no `Pending`s — everyone said Yes.
        TickOutcome::Passed(vote.kind)
    }
}
