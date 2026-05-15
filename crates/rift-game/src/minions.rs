//! Player-owned minion presentation policy.
//!
//! Server simulation owns minion gameplay and snapshots only the
//! compact replicated state. Client crates interpret the rows using
//! this shared policy so future summons opt into grounded/floating
//! behavior deliberately instead of accreting role-specific constants
//! in rendering or network-sync code.

use crate::monsters::MonsterRole;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MinionMotionVisual {
    Grounded,
    Floating {
        hover_height: f32,
        bob_amplitude: f32,
        bob_speed: f32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MinionPresentation {
    pub visual_scale: f32,
    pub motion: MinionMotionVisual,
    pub hud_effect: Option<u8>,
}

impl MinionPresentation {
    pub const GROUNDED: Self = Self {
        visual_scale: 1.0,
        motion: MinionMotionVisual::Grounded,
        hud_effect: None,
    };

    pub fn hover_height(self) -> Option<f32> {
        match self.motion {
            MinionMotionVisual::Floating { hover_height, .. } => Some(hover_height),
            MinionMotionVisual::Grounded => None,
        }
    }

    pub fn bob(self, phase: f32) -> f32 {
        match self.motion {
            MinionMotionVisual::Floating { bob_amplitude, .. } => phase.sin() * bob_amplitude,
            MinionMotionVisual::Grounded => 0.0,
        }
    }

    pub fn bob_speed(self) -> f32 {
        match self.motion {
            MinionMotionVisual::Floating { bob_speed, .. } => bob_speed,
            MinionMotionVisual::Grounded => 0.0,
        }
    }
}

pub fn presentation_for_role(role: MonsterRole) -> MinionPresentation {
    match role {
        MonsterRole::Wraith => MinionPresentation {
            visual_scale: 0.30,
            motion: MinionMotionVisual::Floating {
                hover_height: 1.15,
                bob_amplitude: 0.18,
                bob_speed: 3.4,
            },
            hud_effect: Some(crate::effects::id::VOID_FAMILIAR),
        },
        MonsterRole::Riftling => MinionPresentation {
            visual_scale: 1.0,
            motion: MinionMotionVisual::Grounded,
            hud_effect: Some(crate::effects::id::RIFTLING_SWARM),
        },
        _ => MinionPresentation::GROUNDED,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraith_minion_is_the_void_familiar_presentation() {
        let presentation = presentation_for_role(MonsterRole::Wraith);
        assert_eq!(presentation.visual_scale, 0.30);
        assert_eq!(presentation.hover_height(), Some(1.15));
        assert_eq!(
            presentation.hud_effect,
            Some(crate::effects::id::VOID_FAMILIAR)
        );
    }

    #[test]
    fn non_wraith_minions_default_to_grounded_without_hud_effect() {
        for role in [
            MonsterRole::Brute,
            MonsterRole::Stalker,
            MonsterRole::Caster,
            MonsterRole::Elite,
            MonsterRole::Boss,
            MonsterRole::Mindbinder,
        ] {
            let presentation = presentation_for_role(role);
            assert_eq!(presentation.motion, MinionMotionVisual::Grounded);
            assert_eq!(presentation.visual_scale, 1.0);
            assert_eq!(presentation.hud_effect, None);
        }
    }

    #[test]
    fn riftling_minions_are_grounded_with_swarm_hud_effect() {
        let presentation = presentation_for_role(MonsterRole::Riftling);
        assert_eq!(presentation.motion, MinionMotionVisual::Grounded);
        assert_eq!(presentation.visual_scale, 1.0);
        assert_eq!(
            presentation.hud_effect,
            Some(crate::effects::id::RIFTLING_SWARM)
        );
    }
}
