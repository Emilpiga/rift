//! Semantic animation and skeleton bindings.
//!
//! Clip and joint *names* belong in profiles. Gameplay systems should
//! ask for semantic keys (`PunchJab`, `CastHand`, etc.) and let the
//! profile decide which concrete asset name satisfies that key.

use std::{collections::HashMap, sync::Arc};

use crate::animation::BoundClip;
use crate::ecs::components::AnimationSet;
use crate::renderer::mesh::SkinnedMesh;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AnimClipKey {
    Idle,
    Walk,
    Jog,
    Sprint,
    CastEnter,
    CastShoot,
    CastExit,
    ChannelLoop,
    HitReact,
    Death,
    GhostRise,
    EnemyAttack,
    PunchJab,
    PunchCross,
    MeleeSwing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JointKey {
    SpineRoot,
    CastHand,
    WeaponHand,
    LeftFoot,
    RightFoot,
}

#[derive(Clone, Copy, Debug)]
pub struct ClipSpec {
    pub key: AnimClipKey,
    pub names: &'static [&'static str],
}

#[derive(Clone, Copy, Debug)]
pub struct AnimProfile {
    pub clips: &'static [ClipSpec],
    pub in_place: &'static [AnimClipKey],
}

impl AnimProfile {
    pub fn names_for(self, key: AnimClipKey) -> &'static [&'static str] {
        self.clips
            .iter()
            .find(|spec| spec.key == key)
            .map(|spec| spec.names)
            .unwrap_or(&[])
    }

    pub fn is_in_place_clip_name(self, name: &str) -> bool {
        let lowered = name.to_ascii_lowercase();
        self.in_place.iter().any(|&key| {
            self.names_for(key)
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(&lowered))
        })
    }
}

pub const PLAYER_PROFILE: AnimProfile = AnimProfile {
    clips: &[
        ClipSpec {
            key: AnimClipKey::Idle,
            names: &["idle_loop", "idle", "tpose"],
        },
        ClipSpec {
            key: AnimClipKey::Walk,
            names: &["walk_loop", "walk", "walk_fwd", "walk_forward_loop"],
        },
        ClipSpec {
            key: AnimClipKey::Jog,
            names: &[
                "jog_fwd",
                "jog_forward",
                "jog_forward_loop",
                "jog_loop",
                "jog",
                "run_loop",
                "run",
            ],
        },
        ClipSpec {
            key: AnimClipKey::Sprint,
            names: &[
                "sprint_loop",
                "sprint",
                "sprint_fwd",
                "sprint_forward_loop",
                "run_loop",
                "run",
            ],
        },
        ClipSpec {
            key: AnimClipKey::CastEnter,
            names: &["Spell_Simple_Enter", "Spell_Enter", "Cast_Enter"],
        },
        ClipSpec {
            key: AnimClipKey::CastShoot,
            names: &[
                "Spell_Simple_Shoot",
                "Spell_Shoot",
                "Cast_Shoot",
                "Spell_Simple_Enter",
            ],
        },
        ClipSpec {
            key: AnimClipKey::CastExit,
            names: &["Spell_Simple_Exit", "Spell_Exit", "Cast_Exit"],
        },
        ClipSpec {
            key: AnimClipKey::ChannelLoop,
            names: &[
                "Spell_Simple_Idle_Loop",
                "Spell_Idle_Loop",
                "Cast_Loop",
                "Channel_Loop",
                "Spell_Double_Shoot",
                "Spell_Simple_Enter",
                "Spell_Enter",
                "Cast_Enter",
            ],
        },
        ClipSpec {
            key: AnimClipKey::HitReact,
            names: &["Hit_Head", "Hit_Chest", "HitRecieve", "HitReceive", "Hit"],
        },
        ClipSpec {
            key: AnimClipKey::Death,
            names: &["Death01", "Death_01", "Death", "Death02", "Death_02"],
        },
        ClipSpec {
            key: AnimClipKey::GhostRise,
            names: &["LayToIdle"],
        },
        ClipSpec {
            key: AnimClipKey::PunchJab,
            names: &["Punch_Jab", "Punch", "Punch_Cross", "Sword_Attack"],
        },
        ClipSpec {
            key: AnimClipKey::PunchCross,
            names: &["Punch_Cross", "Punch", "Punch_Jab", "Sword_Attack"],
        },
        ClipSpec {
            key: AnimClipKey::MeleeSwing,
            names: &["Sword_Attack"],
        },
    ],
    in_place: &[
        AnimClipKey::MeleeSwing,
        AnimClipKey::PunchJab,
        AnimClipKey::PunchCross,
    ],
};

pub const MONSTER_PROFILE: AnimProfile = AnimProfile {
    clips: &[
        ClipSpec {
            key: AnimClipKey::Idle,
            names: &["Idle", "Idle_Loop"],
        },
        ClipSpec {
            key: AnimClipKey::Walk,
            names: &["Walk", "Walk_Loop"],
        },
        ClipSpec {
            key: AnimClipKey::HitReact,
            names: &["HitRecieve", "HitReceive", "Hit"],
        },
        ClipSpec {
            key: AnimClipKey::EnemyAttack,
            names: &["Bite_Front", "Bite", "Attack"],
        },
        ClipSpec {
            key: AnimClipKey::Death,
            names: &["Death"],
        },
    ],
    in_place: &[],
};

#[derive(Clone, Default)]
pub struct AnimBindings {
    clips: HashMap<AnimClipKey, Arc<BoundClip>>,
}

impl AnimBindings {
    pub fn resolve(profile: AnimProfile, set: &AnimationSet) -> Self {
        let mut clips = HashMap::new();
        for spec in profile.clips {
            if let Some(clip) = set.find_any(spec.names) {
                clips.insert(spec.key, clip);
            }
        }
        Self { clips }
    }

    pub fn get(&self, key: AnimClipKey) -> Option<Arc<BoundClip>> {
        self.clips.get(&key).cloned()
    }
}

#[derive(Clone, Default)]
pub struct SkeletonBindings {
    joints: HashMap<JointKey, u32>,
    pub upper_body_mask: Vec<f32>,
    pub yaw_only_mask: Vec<f32>,
}

impl SkeletonBindings {
    pub fn resolve_player(mesh: &SkinnedMesh) -> Self {
        let mut joints = HashMap::new();
        if let Some(idx) = mesh.spine_root_joint() {
            joints.insert(JointKey::SpineRoot, idx as u32);
        }
        if let Some(idx) = mesh.left_hand_joint() {
            joints.insert(JointKey::CastHand, idx as u32);
        }
        if let Some(idx) = mesh.left_hand_joint() {
            joints.insert(JointKey::WeaponHand, idx as u32);
        }
        let (left_foot, right_foot) = mesh.foot_joints();
        if let Some(idx) = left_foot {
            joints.insert(JointKey::LeftFoot, idx as u32);
        }
        if let Some(idx) = right_foot {
            joints.insert(JointKey::RightFoot, idx as u32);
        }
        let (upper_body_mask, yaw_only_mask) = mesh.upper_body_mask_with_axis();
        Self {
            joints,
            upper_body_mask,
            yaw_only_mask,
        }
    }

    pub fn get(&self, key: JointKey) -> Option<u32> {
        self.joints.get(&key).copied()
    }
}
