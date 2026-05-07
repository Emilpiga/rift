//! Per-tick snapshot construction.
//!
//! Each connected client gets their own `Snapshot` — `ack_seq` is
//! per-client, and view culling is anchored on each viewer's
//! position so a populated rift floor doesn't blow past the
//! unreliable channel buffer with hundreds of irrelevant entities.

use glam::Vec3;
use rift_net::{
    messages::{entity_flags, EntityKind, EntitySnapshot, Snapshot},
    ClientId, NetTick,
};

use super::enemy::{enemy_anim, ServerEnemy};
use super::loot::ServerLoot;
use super::player::ServerPlayer;
use super::projectile::ServerProjectile;
use super::shrine::ServerReviveShrine;

/// Sight range used to view-cull replicated entities (enemies +
/// projectiles) per receiving client. Squared to skip the sqrt.
pub const VIEW_RANGE_SQ: f32 = 35.0 * 35.0;

/// Build the snapshot for one receiving client.
pub fn build(world: &hecs::World, tick: NetTick, ack_for: ClientId) -> Snapshot {
    let mut entities = Vec::new();
    let mut ack_seq = 0;
    let mut viewer_pos: Option<Vec3> = None;

    // Players first — every connected player ships every snapshot,
    // EXCEPT ghosts (risen-but-dead): a ghost is owner-only, so
    // their row is dropped from any snapshot whose `ack_for`
    // isn't them. Living teammates therefore see no avatar /
    // nameplate / health bar for the ghost, which is what we
    // want for distraction-free spectating.
    for (_e, p) in world.query::<&ServerPlayer>().iter() {
        if p.is_ghost && p.client_id != ack_for {
            continue;
        }
        let mut flags: u8 = 0;
        if p.k.airborne {
            flags |= entity_flags::AIRBORNE;
        }
        if p.hp <= 0.0 {
            flags |= entity_flags::DEAD;
        }
        if p.is_ghost {
            flags |= entity_flags::GHOST;
        }
        entities.push(EntitySnapshot {
            net_id: p.net_id,
            kind: EntityKind::Player {
                client_id: p.client_id,
                aim_yaw: p.k.aim_yaw,
                locomotion: p.k.locomotion,
                action: p.k.action,
                action_start: NetTick(0),
            },
            position: p.k.position.to_array(),
            yaw: p.k.yaw,
            velocity: p.k.velocity.to_array(),
            health_pct: (p.hp / p.hp_max).clamp(0.0, 1.0),
            flags,
        });
        if p.client_id == ack_for {
            ack_seq = p.last_input_seq;
            viewer_pos = Some(p.k.position);
        }
    }

    // View-culled enemies. Anim byte is `ATTACK` while the swing
    // window is active, `WALK` while moving, `IDLE` otherwise. The
    // debuff bitmask comes from any `DebuffStack` component the
    // enemy carries (every enemy gets one at spawn).
    for (e, en) in world.query::<&ServerEnemy>().iter() {
        if !in_view(viewer_pos, en.k.position) {
            continue;
        }
        let dying = en.dying_remaining > 0.0;
        let anim = if dying {
            enemy_anim::DEATH
        } else if en.attack_anim_remaining > 0.0 {
            enemy_anim::ATTACK
        } else if en.k.velocity.length_squared() > 0.01 {
            enemy_anim::WALK
        } else {
            enemy_anim::IDLE
        };
        let debuffs = world
            .get::<&super::debuff::DebuffStack>(e)
            .map(|s| s.bitmask())
            .unwrap_or(0);
        let mut flags = 0u8;
        if dying {
            flags |= entity_flags::DEAD;
        }
        entities.push(EntitySnapshot {
            net_id: en.net_id,
            kind: EntityKind::Enemy { role: en.role, anim, debuffs },
            position: en.k.position.to_array(),
            yaw: en.k.yaw,
            velocity: en.k.velocity.to_array(),
            health_pct: (en.hp / en.hp_max.max(0.001)).clamp(0.0, 1.0),
            flags,
        });
    }

    // View-culled projectiles. Yaw is derived from velocity so
    // client meshes orient correctly.
    for (_e, proj) in world.query::<&ServerProjectile>().iter() {
        if !in_view(viewer_pos, proj.position) {
            continue;
        }
        let yaw = (-proj.velocity.x).atan2(-proj.velocity.z);
        entities.push(EntitySnapshot {
            net_id: proj.net_id,
            kind: EntityKind::Projectile {
                ability: proj.ability_id as u16,
            },
            position: proj.position.to_array(),
            yaw,
            velocity: proj.velocity.to_array(),
            health_pct: 1.0,
            flags: 0,
        });
    }

    // View-culled loot drops. Replicated so a freshly-joined
    // client also sees items already on the floor — the
    // `LootDropped` event only fires once at spawn.
    for (_e, loot_row) in world.query::<&ServerLoot>().iter() {
        if !in_view(viewer_pos, loot_row.position) {
            continue;
        }
        let (base_id, rarity, ilvl, affixes) = loot_row.item.to_wire();
        entities.push(EntitySnapshot {
            net_id: loot_row.net_id,
            kind: EntityKind::Loot {
                item: rift_net::messages::ItemBlob {
                    base_id,
                    rarity,
                    ilvl,
                    affixes,
                },
            },
            position: loot_row.position.to_array(),
            yaw: 0.0,
            velocity: [0.0; 3],
            health_pct: 1.0,
            flags: 0,
        });
    }

    // Revive shrines. No view-culling: shrines are floor-wide
    // landmarks the HUD wants to render even from a distance
    // (and there's at most one per floor anyway).
    for (_e, shrine) in world.query::<&ServerReviveShrine>().iter() {
        let progress_norm = (shrine.progress / rift_net::messages::SHRINE_CHANNEL_DURATION)
            .clamp(0.0, 1.0);
        let progress = (progress_norm * 255.0).round() as u8;
        entities.push(EntitySnapshot {
            net_id: shrine.net_id,
            kind: EntityKind::ReviveShrine {
                progress,
                channelers: shrine.channelers,
                required: shrine.required,
            },
            position: shrine.position.to_array(),
            yaw: 0.0,
            velocity: [0.0; 3],
            health_pct: 1.0,
            flags: 0,
        });
    }

    Snapshot { tick, ack_seq, entities }
}

fn in_view(viewer: Option<Vec3>, pos: Vec3) -> bool {
    let Some(vp) = viewer else { return true };
    let dx = pos.x - vp.x;
    let dz = pos.z - vp.z;
    dx * dx + dz * dz <= VIEW_RANGE_SQ
}
