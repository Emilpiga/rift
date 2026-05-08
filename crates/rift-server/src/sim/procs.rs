//! Legendary `Proc` consumption (event → action triggers).
//!
//! A proc is `(ProcEvent, ProcAction, chance)`. When `event`
//! happens to a player, roll `chance`; on success, run
//! `action`. Today's events are `OnCrit`, `OnHit`, `OnKill`,
//! `OnDodge`, `OnLowHealth`; today's actions are
//! `CastAbility`, `Explosion`, `ChainLightning`.
//!
//! No procs are wired through to gameplay yet; this module is
//! the dedicated home for them so adding the first one is one
//! match arm in one file rather than scattered changes. Each
//! hook function filters by event and dispatches by action;
//! the calling pipeline forwards the player's equipped proc
//! list (`player.ability_mods.procs`) at the appropriate site.
//!
//! # Adding a new proc
//!
//! 1. Add the new `ProcEvent` / `ProcAction` variant in
//!    `rift_game::loot::affixes`.
//! 2. Add a hook function here for that event (or extend an
//!    existing one).
//! 3. Call the hook from the appropriate combat-pipeline site
//!    (`projectile::apply_hits_to_enemies` for `OnHit` /
//!    `OnCrit`, the death-resolution path for `OnKill`, the
//!    Evasive Roll dispatch for `OnDodge`, the HP-cross edge
//!    in `Sim::step` for `OnLowHealth`).

#![allow(dead_code)]

use rift_game::loot::ability_mods::Proc;
use rift_game::loot::ProcEvent;

use super::combat_ctx::CombatCtx;

/// Hook fired by the projectile / channel hit pipeline after
/// damage has been applied. **Stub** — wiring this in requires
/// a chance roll seeded off the same hit identity the meter
/// uses, plus per-action effect routes. Left in place so the
/// call site can be added preemptively without inventing a
/// new module later.
pub fn on_hit(
    _world: &mut hecs::World,
    _ctx: &mut CombatCtx<'_>,
    procs: &[Proc],
) {
    for p in procs {
        if !matches!(p.event, ProcEvent::OnHit) {
            continue;
        }
        let _ = p; // TODO: roll p.chance, dispatch p.action.
    }
}

/// Hook fired on a critical strike. Filters on `OnCrit`.
pub fn on_crit(
    _world: &mut hecs::World,
    _ctx: &mut CombatCtx<'_>,
    procs: &[Proc],
) {
    for p in procs {
        if !matches!(p.event, ProcEvent::OnCrit) {
            continue;
        }
        let _ = p;
    }
}

/// Hook fired when an enemy dies to the caster's damage.
/// Filters on `OnKill`.
pub fn on_kill(
    _world: &mut hecs::World,
    _ctx: &mut CombatCtx<'_>,
    procs: &[Proc],
) {
    for p in procs {
        if !matches!(p.event, ProcEvent::OnKill) {
            continue;
        }
        let _ = p;
    }
}

/// Hook fired when an evasion roll succeeds. Filters on
/// `OnDodge`.
pub fn on_dodge(
    _world: &mut hecs::World,
    _ctx: &mut CombatCtx<'_>,
    procs: &[Proc],
) {
    for p in procs {
        if !matches!(p.event, ProcEvent::OnDodge) {
            continue;
        }
        let _ = p;
    }
}

/// Hook fired the tick the player crosses below the
/// `OnLowHealth` threshold (one-shot per dip — the calling
/// pipeline owns the edge detection so this hook only sees
/// transitions, not steady-state).
pub fn on_low_health(
    _world: &mut hecs::World,
    _ctx: &mut CombatCtx<'_>,
    procs: &[Proc],
) {
    for p in procs {
        if !matches!(p.event, ProcEvent::OnLowHealth) {
            continue;
        }
        let _ = p;
    }
}
