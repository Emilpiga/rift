//! Per-player accessors and persistence-side setters on
//! [`Sim`]. Split out of `sim/mod.rs`. Pure `impl Sim` block —
//! every method is already defined on `Sim` and migrated here
//! verbatim.

use glam::Vec3;
use rift_net::ids::ClientId;
use rift_net::NetId;

use super::effect;
use super::player::ServerPlayer;
use super::{snapshot_talents, trim_trailing_none, Sim};

impl Sim {
    /// `true` if `client_id` is currently a ghost (risen-but-dead).
    /// Used by the message dispatch in `main.rs` to silently drop
    /// gameplay actions (cast, loot pickup, drop) for spectators.
    pub fn is_ghost(&self, client_id: ClientId) -> bool {
        let Some(&entity) = self.sessions.get(&client_id) else {
            return false;
        };
        self.world
            .get::<&ServerPlayer>(entity)
            .map(|p| p.is_ghost)
            .unwrap_or(false)
    }

    /// Hydrate a freshly-spawned player's inventory from a
    /// pre-loaded list (typically the rows fetched by
    /// `PersistenceHandle::load_inventory_blocking`). Idempotent;
    /// replaces whatever was there. Called once during the
    /// `Hello` handshake right after `spawn_player`. `equipment`
    /// is the parallel set of pre-equipped items (rows whose
    /// persisted `equipped_slot` was non-null).
    pub fn set_player_inventory(
        &mut self,
        client_id: ClientId,
        items: Vec<Option<rift_game::loot::Item>>,
        equipment: rift_game::loot::Equipment,
    ) {
        let Some(&entity) = self.sessions.get(&client_id) else {
            return;
        };
        if let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) {
            p.inventory = items;
            trim_trailing_none(&mut p.inventory);
            p.equipment = equipment;
            p.recompute_stats();
        }
    }

    /// Hydrate a freshly-spawned player's level + XP from the
    /// persisted `CharacterRecord`. Restores `Experience`
    /// directly (`current_xp` rolls inside one level, `total_xp`
    /// is the sum). Recomputes stats so the HP pool reflects
    /// the loaded level. Idempotent.
    pub fn set_player_experience(&mut self, client_id: ClientId, level: u32, total_xp: u64) {
        let Some(&entity) = self.sessions.get(&client_id) else {
            return;
        };
        if let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) {
            p.experience.level = level.max(1);
            p.experience.total_xp = total_xp;
            // Derive `current_xp` (XP into the current level)
            // from `(total_xp, level)`. We don't persist
            // current_xp separately yet, so re-deriving keeps
            // the bar accurate after a reload. The XP curve
            // lives in `rift_game::experience` so server and
            // client agree byte-for-byte.
            let xp_for_levels = rift_game::experience::total_xp_for_level(p.experience.level);
            p.experience.current_xp = total_xp.saturating_sub(xp_for_levels);
            p.level = p.experience.level;
            p.recompute_stats();
            p.hp = p.hp_max;
        }
    }

    /// Bind the persistent character UUID onto the live
    /// [`ServerPlayer`] component so loot drops can stamp
    /// pickup-eligibility lineage and the pickup gate can
    /// match the picker against
    /// [`rift_game::loot::LootProvenance`]. Called once at
    /// hello time after [`Self::set_player_experience`] from
    /// the cached [`rift_persistence::CharacterRecord`].
    pub fn set_player_character_id(
        &mut self,
        client_id: ClientId,
        character_id: rift_persistence::Uuid,
    ) {
        let Some(&entity) = self.sessions.get(&client_id) else {
            return;
        };
        if let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) {
            p.character_id = Some(character_id);
        }
    }

    /// Read a player's authoritative XP / level snapshot for the
    /// initial `CharacterStats` reply pushed at Welcome time.
    pub fn player_stats_snapshot(&self, client_id: ClientId) -> Option<(u32, u64, u64)> {
        let &entity = self.sessions.get(&client_id)?;
        let p = self.world.get::<&ServerPlayer>(entity).ok()?;
        Some((
            p.experience.level,
            p.experience.current_xp,
            p.experience.xp_to_next_level(),
        ))
    }

    /// Read a player's authoritative `(hp, hp_max)` pair. Used
    /// by the party-state broadcaster so frame health bars
    /// stay live across membership changes. `None` when the
    /// client has no entity in this sim (hub players if asked
    /// of the rift sim, or vice versa).
    pub fn player_health(&self, client_id: ClientId) -> Option<(f32, f32)> {
        let &entity = self.sessions.get(&client_id)?;
        let p = self.world.get::<&ServerPlayer>(entity).ok()?;
        Some((p.hp, p.hp_max))
    }

    /// Replace the entire ability loadout for `client_id`. Used
    /// at hydrate time to restore the persisted bar after a
    /// fresh `Hello`. No-op when the client isn't connected.
    pub fn set_player_loadout(&mut self, client_id: ClientId, slots: [u8; 6]) {
        let Some(&entity) = self.sessions.get(&client_id) else {
            return;
        };
        if let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) {
            p.loadout = rift_game::loadout::Loadout::from_slots(slots);
        }
    }

    /// Snapshot of the authoritative ability loadout. Used by
    /// the session handler to push `ServerMsg::Loadout` to the
    /// owning client at Welcome time and after every accepted
    /// `SetLoadoutSlot`.
    pub fn player_loadout_snapshot(&self, client_id: ClientId) -> Option<[u8; 6]> {
        let &entity = self.sessions.get(&client_id)?;
        let p = self.world.get::<&ServerPlayer>(entity).ok()?;
        Some(p.loadout.to_wire_bytes())
    }

    /// Restore the player's talent investment from the persisted
    /// [`rift_persistence::CharacterRecord`]. `pairs` is the
    /// flat `(id, rank)` array from `characters.talents`;
    /// `unspent` is the matching `characters.talent_unspent`
    /// count. Idempotent.
    ///
    /// Unknown talent ids (e.g. content removed between
    /// versions) are silently dropped and their would-be ranks
    /// re-credited to `unspent_points`, so a player who logs
    /// in after a content delete doesn't permanently lose
    /// points to dead ids.
    pub fn set_player_talents(&mut self, client_id: ClientId, pairs: &[i16], unspent: u32) {
        let Some(&entity) = self.sessions.get(&client_id) else {
            return;
        };
        let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) else {
            return;
        };
        // Rebuild the tree from scratch so any node not in
        // `pairs` is implicitly at rank 0.
        let mut tree = rift_game::talents::fresh_character_tree();
        let mut total_spent: u32 = 0;
        let mut orphaned: u32 = 0;
        for chunk in pairs.chunks_exact(2) {
            let id = rift_game::talents::TalentId(chunk[0] as u16);
            let rank = chunk[1].max(0) as u8;
            if rank == 0 {
                continue;
            }
            match tree.nodes.iter_mut().find(|n| n.id == id) {
                Some(node) => {
                    node.current_rank = rank.min(node.max_rank);
                    total_spent += node.current_rank as u32;
                }
                None => orphaned += rank as u32,
            }
        }
        tree.total_spent = total_spent;
        // Re-credit orphaned ranks so dead content doesn't
        // permanently sink the player's points.
        tree.unspent_points = unspent.saturating_add(orphaned);
        p.talents = tree;
    }

    /// Snapshot of the authoritative talent tree for the
    /// `ServerMsg::TalentsSync` push. Returns a flat list of
    /// `(talent_id, rank)` pairs for every invested node
    /// (rank ≥ 1) plus the unspent-point count.
    pub fn player_talents_snapshot(&self, client_id: ClientId) -> Option<(Vec<(u16, u8)>, u32)> {
        let &entity = self.sessions.get(&client_id)?;
        let p = self.world.get::<&ServerPlayer>(entity).ok()?;
        let invested: Vec<(u16, u8)> = p
            .talents
            .nodes
            .iter()
            .filter(|n| n.current_rank >= 1)
            .map(|n| (n.id.0, n.current_rank))
            .collect();
        Some((invested, p.talents.unspent_points))
    }

    /// Apply one [`ClientMsg::InvestTalent`]. Returns the fresh
    /// invested-pairs + unspent snapshot on success, or `None`
    /// if the invest was rejected (unknown id, prereqs unmet,
    /// at max rank, no unspent points).
    pub fn invest_talent_for_player(
        &mut self,
        client_id: ClientId,
        talent_id: u16,
    ) -> Option<(Vec<(u16, u8)>, u32)> {
        let &entity = self.sessions.get(&client_id)?;
        let mut p = self.world.get::<&mut ServerPlayer>(entity).ok()?;
        let id = rift_game::talents::TalentId(talent_id);
        if !p.talents.invest(id) {
            return None;
        }
        Some(snapshot_talents(&p.talents))
    }

    /// Apply one [`ClientMsg::RespecTalent`] — refund every rank
    /// of `talent_id`. Returns the fresh invested-pairs +
    /// unspent snapshot on success, or `None` if the refund was
    /// rejected (unknown id, no ranks invested, would orphan a
    /// downstream node — see `TALENT_TREE.md` §7).
    pub fn respec_talent_for_player(
        &mut self,
        client_id: ClientId,
        talent_id: u16,
    ) -> Option<(Vec<(u16, u8)>, u32)> {
        let &entity = self.sessions.get(&client_id)?;
        let mut p = self.world.get::<&mut ServerPlayer>(entity).ok()?;
        let id = rift_game::talents::TalentId(talent_id);
        if p.talents.refund_one(id) == 0 {
            return None;
        }
        Some(snapshot_talents(&p.talents))
    }

    /// Apply one [`ClientMsg::RespecAllTalents`] — wipe every
    /// invested point. Always succeeds for a known session;
    /// returns the fresh empty-invested-list + unspent snapshot.
    pub fn respec_all_talents_for_player(
        &mut self,
        client_id: ClientId,
    ) -> Option<(Vec<(u16, u8)>, u32)> {
        let &entity = self.sessions.get(&client_id)?;
        let mut p = self.world.get::<&mut ServerPlayer>(entity).ok()?;
        p.talents.refund_all();
        Some(snapshot_talents(&p.talents))
    }

    /// Mutate one slot of the player's ability bar. Validates:
    /// - `slot_index` is in range *and* unlocked at the player's
    ///   current level (per `loadout::SLOT_UNLOCK_LEVELS`)
    /// - `ability_id` is either the empty-slot sentinel or a
    ///   player-castable ability whose own `unlock_level` the
    ///   player has reached.
    /// Returns the freshly-updated full loadout (so the caller
    /// can persist + reply in one go) or `None` if the request
    /// was rejected.
    pub fn set_player_loadout_slot(
        &mut self,
        client_id: ClientId,
        slot_index: u8,
        ability_id: u8,
    ) -> Option<[u8; 6]> {
        let slot_idx = slot_index as usize;
        if slot_idx >= rift_game::loadout::SLOT_COUNT {
            return None;
        }
        let &entity = self.sessions.get(&client_id)?;
        let mut p = self.world.get::<&mut ServerPlayer>(entity).ok()?;
        let player_level = p.experience.level;
        if !rift_game::loadout::is_slot_unlocked(slot_idx, player_level) {
            return None;
        }
        let ability = rift_game::abilities::AbilityWireId::new(ability_id);
        // Allow either the empty sentinel (clearing the slot) or
        // an unlocked player-castable ability.
        let allow_empty = ability == rift_game::loadout::EMPTY_SLOT;
        let allow_ability = rift_game::loadout::is_player_ability(ability)
            && rift_game::loadout::is_ability_unlocked(ability, &p.talents);
        if !allow_empty && !allow_ability {
            return None;
        }
        p.loadout.set_slot(slot_idx, ability);
        Some(p.loadout.to_wire_bytes())
    }

    /// Spawn (or look up the existing) player entity for a freshly-
    /// Helloed client. Returns the allocated `NetId`. Initial
    /// [`CharacterStats`] are baked into [`ServerPlayer::fresh`]
    /// from the hero config.
    pub fn spawn_player(&mut self, client_id: ClientId) -> NetId {
        if let Some(&existing) = self.sessions.get(&client_id) {
            if let Ok(p) = self.world.get::<&ServerPlayer>(existing) {
                return p.net_id;
            }
        }
        let net_id = NetId(self.next_player_net_id | 0x8000_0000);
        self.next_player_net_id = self.next_player_net_id.wrapping_add(1).max(1);
        let spawn = Vec3::new(self.floor.spawn_pos.x, 0.0, self.floor.spawn_pos.z);
        let entity = self.world.spawn((
            ServerPlayer::fresh(client_id, net_id, spawn),
            effect::EffectStack::default(),
        ));
        self.sessions.insert(client_id, entity);
        log::info!("sim: spawned player {client_id:?} as {net_id:?} at {spawn:?}");
        net_id
    }

    pub fn despawn_player(&mut self, client_id: ClientId) {
        if let Some(entity) = self.sessions.remove(&client_id) {
            let _ = self.world.despawn(entity);
            log::info!("sim: despawned player {client_id:?}");
        }
        self.pending_inputs.remove(&client_id);
        self.cooldowns.remove(&client_id);
    }

    /// Lift a player out of this Sim entirely: removes the ECS
    /// entity and every per-client side-table entry, returning
    /// the components so the caller can re-spawn the player in a
    /// different Sim (hub ↔ rift movement). Returns `None` if
    /// the client wasn't registered with this Sim.
    ///
    /// Cooldowns are intentionally dropped — moving floors is a
    /// fresh start for the GCD bar, mirroring the
    /// pre-refactor `change_floor` behaviour. Bag, equipment,
    /// stash, level, XP, HP percent are all preserved on the
    /// returned [`ServerPlayer`].
    pub fn extract_player(
        &mut self,
        client_id: ClientId,
    ) -> Option<(ServerPlayer, effect::EffectStack)> {
        let entity = self.sessions.remove(&client_id)?;
        // `world.remove::<(A, B)>` would tear both off in one go,
        // but we want to be defensive about partial state
        // (EffectStack might in theory be missing): pull each
        // component independently and fall back to default for
        // the optional one.
        let player = self.world.remove_one::<ServerPlayer>(entity).ok()?;
        let effects = self
            .world
            .remove_one::<effect::EffectStack>(entity)
            .unwrap_or_default();
        let _ = self.world.despawn(entity);
        self.pending_inputs.remove(&client_id);
        self.cooldowns.remove(&client_id);
        log::info!(
            "sim: extracted player {client_id:?} from floor {}",
            self.floor_index
        );
        Some((player, effects))
    }

    /// Drop a previously-extracted player into this Sim. The
    /// player keeps their existing [`NetId`] (so client-side
    /// avatar tracking survives the move) but is snapped to this
    /// floor's spawn position and re-healed per the same policy
    /// as [`Self::change_floor`] (full heal on hub, living-only
    /// heal on rift). Returns the player's NetId for convenience.
    pub fn inject_player(
        &mut self,
        client_id: ClientId,
        mut player: ServerPlayer,
        effects: effect::EffectStack,
    ) -> NetId {
        let spawn = Vec3::new(self.floor.spawn_pos.x, 0.0, self.floor.spawn_pos.z);
        player.k.position = spawn;
        player.k.velocity = Vec3::ZERO;
        player.k.vy = 0.0;
        player.k.airborne = false;
        // HP / ghost reset matches `change_floor`'s policy: hub
        // is a safe respawn point that wipes death state,
        // rift entries top up living players but leave ghosts as
        // ghosts (they joined to spectate).
        if self.floor_index == 0 {
            player.hp = player.hp_max;
            player.is_ghost = false;
            player.ghost_rise_timer = None;
        } else if !player.is_ghost {
            player.hp = player.hp_max;
        }
        let net_id = player.net_id;
        let entity = self.world.spawn((player, effects));
        self.sessions.insert(client_id, entity);
        log::info!(
            "sim: injected player {client_id:?} ({net_id:?}) into floor {}",
            self.floor_index
        );
        net_id
    }
}
