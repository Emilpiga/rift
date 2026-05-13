//! Exit / descend vote handling and hub-respawn timer methods on
//! [`Sim`]. Split out of the main `sim/mod.rs`. Pure `impl Sim`
//! block — every method is already defined on `Sim` and migrated
//! here verbatim.

use std::collections::HashMap;

use rift_net::ids::ClientId;
use rift_net::messages::VoteChoice;
use rift_net::NetId;

use super::actor::{NetIdentity, Vitals};
use super::player::ServerPlayer;
use super::{vote, ExitVoteRequest, Sim};

impl Sim {
    /// Outcome of [`Self::request_exit_vote`]: either an
    /// instant-pass (solo, must be exited immediately by the
    /// caller), an opened vote window, or a refusal (cooldown,
    /// already in hub, dead, etc.).
    ///
    /// See the `ExitVoteRequest` enum below for variants.

    /// Handle a [`rift_net::ClientMsg::RiftExitVoteStart`] from
    /// `client_id`. Solo players (one connected) get an instant
    /// `Pass` outcome — caller wipes dead-player loot and
    /// transitions to the hub. Multiplayer parties get a fresh
    /// vote window opened with the initiator auto-recorded as
    /// `Yes`; subsequent ticks resolve via [`Self::tick_exit_vote`].
    ///
    /// Silently rejected (returns `Refused`) if:
    /// - we're already in the hub,
    /// - the caster is in the down-pose (dead but not yet a
    ///   ghost — the rise timer hasn't elapsed),
    /// - a vote is already active,
    /// - the cooldown timer hasn't expired yet.
    ///
    /// Ghost initiators are refused: a ghost could otherwise
    /// gatekeep their living teammates inside the rift by
    /// repeatedly opening votes (or by being the lone holdout
    /// initiator on a vote whose other voters can't even see
    /// them). Ghosts also can't cast on an open vote (the roll
    /// is built from living players only). Party-wipe recovery
    /// is handled by the existing hub-respawn timer.
    pub fn request_exit_vote(&mut self, client_id: ClientId) -> ExitVoteRequest {
        if self.floor_index == 0 {
            return ExitVoteRequest::Refused;
        }
        if self.exit_vote.is_some() || self.exit_vote_cooldown > 0.0 {
            return ExitVoteRequest::Refused;
        }
        let Some(&entity) = self.sessions.get(&client_id) else {
            return ExitVoteRequest::Refused;
        };
        let initiator_alive = match (
            self.world.get::<&ServerPlayer>(entity),
            self.world.get::<&Vitals>(entity),
        ) {
            (Ok(p), Ok(vitals)) => !vitals.is_dead() && !p.is_ghost,
            _ => false,
        };
        // Down-pose (dead pre-rise) and ghosts are both refused.
        // Only living teammates can open or cast on a vote.
        if !initiator_alive {
            return ExitVoteRequest::Refused;
        }
        // Build the living-voter roll up front so we know whether
        // we're solo or party. Ghosts are never voters.
        let mut roll: HashMap<NetId, VoteChoice> = HashMap::new();
        let mut initiator_net_id: Option<NetId> = None;
        for (_e, (p, identity, vitals)) in self
            .world
            .query::<(&ServerPlayer, &NetIdentity, &Vitals)>()
            .iter()
        {
            if vitals.is_dead() {
                continue;
            }
            roll.insert(identity.net_id, VoteChoice::Pending);
            if p.client_id == client_id {
                initiator_net_id = Some(identity.net_id);
            }
        }
        // Solo: alive caller is the only living player.
        if roll.len() <= 1 {
            log::info!("vote: solo exit by {:?} instant pass", client_id);
            return ExitVoteRequest::Pass;
        }
        // Multiplayer: stamp the initiator as Yes immediately.
        if let Some(nid) = initiator_net_id {
            roll.insert(nid, VoteChoice::Yes);
        }
        log::info!(
            "vote: opened by {:?} ({} living voters)",
            client_id,
            roll.len()
        );
        self.exit_vote = Some(vote::ExitVote {
            kind: rift_net::messages::VoteKind::Exit,
            time_remaining: vote::VOTE_DURATION,
            votes: roll,
        });
        self.exit_vote_dirty = true;
        ExitVoteRequest::Opened
    }

    /// Handle a [`rift_net::ClientMsg::RequestEnterRift`] received
    /// while currently on a rift floor. Solo parties bypass this
    /// path and fall through to instant transition. Multiplayer
    /// parties open a 15s ready-check vote so one player pressing
    /// F at the exit portal doesn't yank everyone else into the
    /// next floor unprepared. Same shape + lifetime as
    /// [`Self::request_exit_vote`]; only `kind` and the
    /// resolution path differ (see [`Self::tick_exit_vote`]).
    pub fn request_descend_vote(&mut self, client_id: ClientId) -> ExitVoteRequest {
        if self.floor_index == 0 {
            // Hub \u2192 first floor is always instant. Caller falls
            // through to the transition path.
            return ExitVoteRequest::Refused;
        }
        if self.exit_vote.is_some() || self.exit_vote_cooldown > 0.0 {
            return ExitVoteRequest::Refused;
        }
        let Some(&entity) = self.sessions.get(&client_id) else {
            return ExitVoteRequest::Refused;
        };
        let initiator_alive = match (
            self.world.get::<&ServerPlayer>(entity),
            self.world.get::<&Vitals>(entity),
        ) {
            (Ok(p), Ok(vitals)) => !vitals.is_dead() && !p.is_ghost,
            _ => false,
        };
        if !initiator_alive {
            return ExitVoteRequest::Refused;
        }
        let mut roll: HashMap<NetId, VoteChoice> = HashMap::new();
        let mut initiator_net_id: Option<NetId> = None;
        for (_e, (p, identity, vitals)) in self
            .world
            .query::<(&ServerPlayer, &NetIdentity, &Vitals)>()
            .iter()
        {
            if vitals.is_dead() {
                continue;
            }
            roll.insert(identity.net_id, VoteChoice::Pending);
            if p.client_id == client_id {
                initiator_net_id = Some(identity.net_id);
            }
        }
        if roll.len() <= 1 {
            log::info!("vote: solo descend by {:?} instant pass", client_id);
            return ExitVoteRequest::Pass;
        }
        if let Some(nid) = initiator_net_id {
            roll.insert(nid, VoteChoice::Yes);
        }
        log::info!(
            "vote: descend opened by {:?} ({} living voters)",
            client_id,
            roll.len()
        );
        self.exit_vote = Some(vote::ExitVote {
            kind: rift_net::messages::VoteKind::Descend,
            time_remaining: vote::VOTE_DURATION,
            votes: roll,
        });
        self.exit_vote_dirty = true;
        ExitVoteRequest::Opened
    }

    /// Handle a [`rift_net::ClientMsg::RiftExitVoteCast`] from
    /// `client_id`. Silently no-ops when no vote is active, the
    /// caster isn't on the voter roll, or the caster has already
    /// voted. Sets the dirty flag so the main loop broadcasts a
    /// fresh `RiftExitVote` next iteration.
    pub fn cast_exit_vote(&mut self, client_id: ClientId, yes: bool) {
        let Some(vote) = self.exit_vote.as_mut() else {
            return;
        };
        let Some(&entity) = self.sessions.get(&client_id) else {
            return;
        };
        let Some(net_id) = self
            .world
            .get::<&NetIdentity>(entity)
            .ok()
            .map(|identity| identity.net_id)
        else {
            return;
        };
        let Some(slot) = vote.votes.get_mut(&net_id) else {
            return;
        };
        if !matches!(slot, VoteChoice::Pending) {
            // No changing your mind.
            return;
        }
        *slot = if yes { VoteChoice::Yes } else { VoteChoice::No };
        log::info!(
            "vote: {:?} cast {}",
            client_id,
            if yes { "YES" } else { "NO" }
        );
        self.exit_vote_dirty = true;
    }

    /// Per-tick: decrement the active vote's deadline / cooldown
    /// and resolve once outcome is known. Returns the resolution
    /// so the main loop can wipe dead-player loot + transition to
    /// the hub on a `Pass`.
    pub fn tick_exit_vote(&mut self, dt: f32) -> vote::TickOutcome {
        // Cooldown countdown (independent of any active vote).
        if self.exit_vote_cooldown > 0.0 {
            let prev = self.exit_vote_cooldown;
            self.exit_vote_cooldown = (prev - dt).max(0.0);
            // Mark dirty when we cross integer-second boundaries
            // so the HUD ring animates smoothly. Cheap: at most
            // one extra broadcast per second.
            if prev.ceil() != self.exit_vote_cooldown.ceil() {
                self.exit_vote_dirty = true;
            }
        }
        let Some(vote) = self.exit_vote.as_mut() else {
            return vote::TickOutcome::Idle;
        };
        let prev_remaining = vote.time_remaining;
        vote.time_remaining = (prev_remaining - dt).max(0.0);
        if prev_remaining.ceil() != vote.time_remaining.ceil() {
            // Tick boundary: HUD countdown ring updates.
            self.exit_vote_dirty = true;
        }
        match vote::resolve(vote) {
            vote::TickOutcome::Idle => vote::TickOutcome::Idle,
            vote::TickOutcome::Passed(kind) => {
                log::info!("vote: passed unanimously ({:?})", kind);
                self.exit_vote = None;
                self.exit_vote_cooldown = 0.0;
                self.exit_vote_dirty = true;
                vote::TickOutcome::Passed(kind)
            }
            vote::TickOutcome::Fizzled => {
                log::info!(
                    "vote: fizzled (no/timeout) — {}s cooldown",
                    vote::VOTE_COOLDOWN as u32
                );
                self.exit_vote = None;
                self.exit_vote_cooldown = vote::VOTE_COOLDOWN;
                self.exit_vote_dirty = true;
                vote::TickOutcome::Fizzled
            }
        }
    }

    /// Drain the dirty flag and produce a wire-shape
    /// [`VoteState`] reflecting the current sim state. The main
    /// loop ships this as `ServerMsg::RiftExitVote` whenever it
    /// returns `Some`.
    pub fn take_exit_vote_update(&mut self) -> Option<rift_net::messages::VoteState> {
        if !self.exit_vote_dirty {
            return None;
        }
        self.exit_vote_dirty = false;
        Some(vote::build_state(
            self.exit_vote.as_ref(),
            self.exit_vote_cooldown,
        ))
    }

    /// `true` once the post-death countdown has elapsed. Consumes
    /// the request — callers are expected to immediately drive
    /// `change_floor(0)`. Returns `false` while the timer is
    /// still running, or when no death is pending.
    pub fn take_hub_respawn_request(&mut self) -> bool {
        match self.hub_respawn_timer {
            Some(t) if t <= 0.0 => {
                self.hub_respawn_timer = None;
                true
            }
            _ => false,
        }
    }
}
