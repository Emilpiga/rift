//! Outbound commands the client sends back to the server: per-frame
//! input, aim, ability casts, hub↔rift transitions, loot pickup,
//! and equip/unequip.
//!
//! Local prediction lives here too — `send_input` predicts the
//! command against the kinematic state and pushes it into
//! `input_history` so the next snapshot can replay-on-reconcile.

use glam::{Quat, Vec3};
use rift_engine::Input;
use rift_net::{
    messages::{button_bits, InputCmd},
    Channel, ClientMsg, NetId,
};
use winit::keyboard::KeyCode;

use super::NetClient;

impl NetClient {
    /// Build and ship a single `InputCmd` from the engine's current
    /// input state. Also predicts the command locally against
    /// `predicted` and stashes it in `input_history` so the next
    /// snapshot can replay-on-top during reconciliation.
    pub(super) fn send_input(&mut self, input: &Input, dt: f32) {
        self.input_seq = self.input_seq.wrapping_add(1);

        // Dead-and-not-yet-risen players don't send input. We
        // still bump `input_seq` so `ack_seq` keeps advancing
        // once snapshots resume, freeze the predicted kinematic,
        // and ship a zero-input command so the server's
        // coalescer notices the seq bump and the client's
        // reconcile path doesn't replay any pre-death movement
        // against the new state. Buttons / movement are all
        // zero so nothing actually happens server-side either
        // way.
        //
        // Ghosts (`local_ghost`) are a special case: they're
        // still flagged DEAD from the HP point of view but the
        // server accepts WASD input from them so they can scout
        // ahead. We still gate ability/attack buttons (server
        // rejects them anyway) so the spectate camera doesn't
        // light up FX trying to predict casts.
        let pinned = self.local_dead && !self.local_ghost;
        let mut buttons: u16 = 0;
        let mut dx = 0.0f32;
        let mut dz = 0.0f32;
        if !pinned {
            // WASD → camera-relative move axis, matching the SP
            // `player_input_system`. We rotate the raw axis by the
            // active camera yaw before sending so the wire payload is
            // already in world space — the server doesn't know about
            // cameras and shouldn't have to.
            if input.is_key_held(KeyCode::KeyW) {
                dz -= 1.0;
                buttons |= button_bits::MOVE_FORWARD;
            }
            if input.is_key_held(KeyCode::KeyS) {
                dz += 1.0;
                buttons |= button_bits::MOVE_BACK;
            }
            if input.is_key_held(KeyCode::KeyA) {
                dx -= 1.0;
                buttons |= button_bits::MOVE_LEFT;
            }
            if input.is_key_held(KeyCode::KeyD) {
                dx += 1.0;
                buttons |= button_bits::MOVE_RIGHT;
            }
            // Jump input intentionally not bound — see
            // `player_action_pre_system` in rift-engine for the
            // rationale (ARPG, no jump in gameplay loop).
        }

        // Rotate the raw input axis by camera yaw so "W" means
        // "forward from where the camera is looking", same as SP.
        let cam_yaw = input.camera_yaw();
        let world = Quat::from_rotation_y(cam_yaw) * Vec3::new(dx, 0.0, dz);
        let mut x = world.x;
        let mut z = world.z;
        let len2 = x * x + z * z;
        if len2 > 1.0 {
            let inv = 1.0 / len2.sqrt();
            x *= inv;
            z *= inv;
        }

        let cmd = InputCmd {
            seq: self.input_seq,
            tick_estimate: self.last_server_tick,
            move_dir: [x, z],
            aim_dir: self.pending_aim,
            buttons,
            cast_target: None,
        };

        // Predict locally so the local avatar moves immediately,
        // and stash for replay-on-reconcile. Skipped while dead:
        // the server has frozen our corpse, and any local
        // integration would just be undone (and re-replayed) on
        // the next snapshot, jittering the death-animation
        // avatar in place.
        if !pinned && self.predicted_ready {
            if let Some(floor) = self.predict_floor.as_ref() {
                rift_game::kinematic::apply_input(
                    &mut self.predicted,
                    cmd.move_dir,
                    cmd.aim_dir,
                    cmd.buttons,
                    self.predicted_move_speed,
                );
                rift_game::kinematic::integrate(&mut self.predicted, floor, dt);
            }
        }
        // Bound history at 2 seconds of input so it can't grow
        // unbounded if the server stops acking for any reason.
        // While dead, skip pushing entirely — the snapshot path
        // cleared `input_history` on death and there's nothing
        // for the server to act on anyway.
        if !pinned {
            if self.input_history.len() >= 128 {
                self.input_history.pop_front();
            }
            self.input_history.push_back((self.input_seq, dt, cmd));
        }

        self.send(Channel::Snapshot, &ClientMsg::Input(cmd));
    }

    /// Update the aim direction shipped on the next outbound input.
    /// Call once per frame from the binary after `GameState::update`
    /// has computed the cursor → world aim, so the value travels to
    /// the server promptly. Pass `Vec3::ZERO` to clear (server then
    /// falls back to body yaw for the spine-twist on remotes).
    pub fn set_aim(&mut self, aim: Vec3) {
        // Drop the y component — aim is a horizontal direction on
        // the wire. Renormalise so a zero-length input cleanly
        // reads as "no aim" on the server side.
        let len = (aim.x * aim.x + aim.z * aim.z).sqrt();
        if len > 1.0e-4 {
            self.pending_aim = [aim.x / len, aim.z / len];
        } else {
            self.pending_aim = [0.0, 0.0];
        }
    }

    /// Update the local player's predicted `move_speed`. The game
    /// loop calls this once per frame with `PlayerState::stats().
    /// move_speed`, so Boots/MoveSpeed affixes feel snappy on the
    /// local avatar without waiting for a server reconcile.
    pub fn set_predicted_move_speed(&mut self, move_speed: f32) {
        // Clamp to a sane floor so a malformed sheet can't freeze
        // local movement. The authoritative server still uses its
        // own value, so this is purely a UX guard.
        self.predicted_move_speed = move_speed.max(0.5);
    }

    /// Arm the auth handshake: tell the net layer the player
    /// has confirmed an identity and we should ship `Hello` as
    /// soon as renet reports the connection live. Idempotent;
    /// safe to call multiple times. The actual credential is
    /// minted by the installed [`crate::auth::Signer`] at send
    /// time, so this method takes no identity argument \u2014 the
    /// signer is the authority.
    ///
    /// Name is historical: the pre-auth flow had a separate
    /// `RequestRoster` packet keyed on a typed account name.
    /// That role now belongs to `Authenticated`, which carries\n    /// the roster as part of the auth reply.
    pub fn arm_auth(&mut self) {
        if !self.auth_armed {
            self.auth_armed = true;
            // Force a fresh `Hello` if the previous one was\n            // sent against a now-stale signer. In practice\n            // this is a no-op (callers arm before connect),\n            // but it costs nothing to be defensive.
            self.auth_sent = false;
        }
    }

    /// Drain the most recent roster reply, if any. Returns `None`
    /// while we're still waiting for the server. The caller takes
    /// ownership; subsequent calls return `None` until a fresh
    /// roster lands.
    pub fn take_roster(&mut self) -> Option<Vec<rift_net::messages::RosterEntry>> {
        self.roster.take()
    }

    /// Ask the server to advance to the next floor (or, if currently
    /// in the hub, enter the rift). Server is the authority on
    /// whether the request is honoured; if accepted, every client
    /// receives a reliable `LoadFloor`.
    pub fn request_enter_rift(&mut self) {
        log::info!("net: -> RequestEnterRift");
        self.send(Channel::Control, &ClientMsg::RequestEnterRift);
    }

    /// Forward a portal-modal proposal. Server may resolve
    /// instantly (Solo / Matchmade with no party) or open a
    /// per-member confirm prompt (Party / Matchmade with a
    /// party). The reply path is `ServerMsg::PortalPrompt`
    /// for awaiting members and `LoadFloor` for the proposer.
    pub fn request_propose_rift_entry(&mut self, start_floor: u32, mode: u8) {
        log::info!("net: -> ProposeRiftEntry(floor={start_floor}, mode={mode})");
        self.send(
            Channel::Control,
            &ClientMsg::ProposeRiftEntry { start_floor, mode },
        );
    }

    /// Reply to a per-member portal confirm prompt.
    pub fn request_portal_confirm(&mut self, accept: bool) {
        log::info!("net: -> PortalConfirm(accept={accept})");
        self.send(Channel::Control, &ClientMsg::PortalConfirm { accept });
    }

    /// Forward an arbitrary party-control message (invite /
    /// accept / decline / leave / kick / promote). Sender is
    /// the chat slash-command parser and the right-click
    /// context menu.
    pub fn send_party_msg(&mut self, msg: ClientMsg) {
        log::info!("net: -> {msg:?}");
        self.send(Channel::Control, &msg);
    }

    /// Ask the server to teleport the session back to the hub.
    pub fn request_return_to_hub(&mut self) {
        log::info!("net: -> RequestReturnToHub");
        self.send(Channel::Control, &ClientMsg::RequestReturnToHub);
    }

    /// Send a best-effort clean leave before the client reconnects
    /// to surface character select again.
    pub fn request_goodbye(&mut self) {
        log::info!("net: -> Goodbye");
        self.send(Channel::Control, &ClientMsg::Goodbye);
    }

    /// Open the rift exit vote. Sent when the local player
    /// presses F at the rift-spawn portal. Server validates the
    /// caster is alive and on a non-hub floor; on success it
    /// either instantly transitions a solo player to the hub or
    /// broadcasts a fresh `RiftExitVote` so every party member's
    /// HUD lights up.
    pub fn request_exit_vote_start(&mut self) {
        log::info!("net: -> RiftExitVoteStart");
        self.send(Channel::Control, &ClientMsg::RiftExitVoteStart);
    }

    /// Cast the local player's vote on the active rift exit
    /// vote. `yes = true` for [F]/[Y], `false` for [N]. Silently
    /// ignored server-side if no vote is active or the caster
    /// has already voted.
    pub fn request_exit_vote_cast(&mut self, yes: bool) {
        log::info!("net: -> RiftExitVoteCast({yes})");
        self.send(Channel::Control, &ClientMsg::RiftExitVoteCast { yes });
    }

    /// Push the local player's revive-shrine channel intent
    /// up to the server. `Some(shrine)` while F is held in
    /// range; `None` on release / out-of-range. The client
    /// edge-triggers on transitions, so this stays cheap.
    pub fn request_set_shrine_channel(&mut self, shrine: Option<rift_net::NetId>) {
        log::info!("net: -> SetShrineChannel({shrine:?})");
        self.send(Channel::Control, &ClientMsg::SetShrineChannel { shrine });
    }

    /// Ask the server to fire an ability. Server is the authority on
    /// cooldown / range / damage; on success it spawns projectiles
    /// (replicated via snapshots) and emits `WorldEvent`s reliably.
    /// `aim_dir` should be the XZ-plane unit direction.
    pub fn request_cast(
        &mut self,
        ability_id: rift_game::abilities::AbilityWireId,
        origin: Vec3,
        aim_dir: Vec3,
        placed_target: Option<Vec3>,
        target_net_id: Option<rift_net::NetId>,
    ) {
        let aim = Vec3::new(aim_dir.x, 0.0, aim_dir.z).normalize_or_zero();
        // Locally-predicted side-effects of the cast on the
        // shared `Kinematic` state. Today only Evasive Roll has a
        // movement effect that the prediction loop has to mirror;
        // every other ability either spawns server-side projectiles
        // (no kinematic side-effect on the caster) or runs a
        // channel that the server drives via separate messages.
        if ability_id == rift_game::abilities::id::EVASIVE_ROLL {
            rift_game::kinematic::start_roll(&mut self.predicted, aim);
        }
        let msg = ClientMsg::CastAbility {
            ability_id: ability_id.raw(),
            origin: origin.to_array(),
            aim_dir: [aim.x, aim.z],
            placed_target: placed_target.map(|v| v.to_array()),
            target_net_id,
        };
        self.send(Channel::Event, &msg);
    }

    /// Tell the server to end the current channel for `ability_id`.
    /// Sent on button release / movement-cancel during a
    /// hold-to-channel ability. Server silently ignores if the
    /// caller isn't actually channeling that ability so duplicate
    /// release packets are safe.
    pub fn request_end_channel(&mut self, ability_id: rift_game::abilities::AbilityWireId) {
        let msg = ClientMsg::EndChannel {
            ability_id: ability_id.raw(),
        };
        self.send(Channel::Event, &msg);
    }

    /// Ask the server to swap the ability in `slot_index` for the
    /// one with `ability_id`. Server validates the ability is
    /// player-castable; on accept it mirrors the change to the
    /// persisted record and replies with a fresh
    /// [`ServerMsg::Loadout`].
    pub fn request_set_loadout_slot(
        &mut self,
        slot_index: u8,
        ability_id: rift_game::abilities::AbilityWireId,
    ) {
        log::debug!("net: -> SetLoadoutSlot slot={slot_index} ability={ability_id}");
        self.send(
            Channel::Control,
            &ClientMsg::SetLoadoutSlot {
                slot_index,
                ability_id: ability_id.raw(),
            },
        );
    }

    /// Ask the server to spend one talent point on `talent_id`.
    /// Server validates the invest (prereqs satisfied, current
    /// rank below max, unspent points > 0); on accept it
    /// mirrors the change to the persisted record and replies
    /// with a fresh [`rift_net::messages::ServerMsg::TalentsSync`].
    pub fn request_invest_talent(&mut self, talent_id: u16) {
        log::debug!("net: -> InvestTalent talent_id={talent_id}");
        self.send(Channel::Control, &ClientMsg::InvestTalent { talent_id });
    }

    /// Ask the server to lesser-respec a single talent node —
    /// refund every rank of `talent_id` if doing so would not
    /// orphan a downstream node. Server replies with a fresh
    /// `TalentsSync` on accept; silent no-op on rejection.
    pub fn request_respec_talent(&mut self, talent_id: u16) {
        log::debug!("net: -> RespecTalent talent_id={talent_id}");
        self.send(Channel::Control, &ClientMsg::RespecTalent { talent_id });
    }

    /// Ask the server to greater-respec — wipe every invested
    /// point on the tree. Always accepted; reply is the fresh
    /// (empty-invested) `TalentsSync`.
    pub fn request_respec_all_talents(&mut self) {
        log::debug!("net: -> RespecAllTalents");
        self.send(Channel::Control, &ClientMsg::RespecAllTalents);
    }

    /// Ask the server to consume the bag item at
    /// `inventory_index`. `target_arg` is the consumable's
    /// dispatch payload \u2014 `u16::MAX` for self-targeted
    /// consumables, or e.g. a `TalentId` for a
    /// `LesserRespecToken`. Server validates the slot holds a
    /// consumable, dispatches by `ConsumableKind`, burns the
    /// token on success, and replies with fresh `InventorySync`
    /// (always) + `TalentsSync` (when the consumable touched
    /// the tree).
    pub fn request_use_item(&mut self, inventory_index: u32, target_arg: u16) {
        log::debug!("net: -> UseItem idx={inventory_index} target_arg={target_arg}");
        self.send(
            Channel::Control,
            &ClientMsg::UseItem {
                inventory_index,
                target_arg,
            },
        );
    }

    /// Ask the server to claim a ground-loot drop on our behalf.
    /// Server validates range and broadcasts [`ServerMsg::LootClaimed`]
    /// on success; clients tear down their visuals on receipt.
    pub fn request_pickup_loot(&mut self, loot: NetId) {
        log::debug!("net: -> PickUpLoot {loot:?}");
        self.send(Channel::Control, &ClientMsg::PickUpLoot { net_id: loot });
    }

    /// Ask the server to equip the picker's bag item at
    /// `inventory_index` into its canonical slot. Server replies
    /// with fresh `InventorySync` + `EquipmentSync` after
    /// applying the swap; the client never optimistically
    /// mutates its mirror.
    pub fn request_equip_item(&mut self, inventory_index: u32) {
        log::debug!("net: -> EquipItem idx={inventory_index}");
        self.send(Channel::Control, &ClientMsg::EquipItem { inventory_index });
    }

    /// Ask the server to move whatever's currently in `slot`
    /// back into the picker's bag. `slot` is the byte from
    /// `EquipSlot::to_u8`.
    pub fn request_unequip_item(&mut self, slot: u8) {
        log::debug!("net: -> UnequipItem slot={slot}");
        self.send(Channel::Control, &ClientMsg::UnequipItem { slot });
    }

    /// Begin a stash session. Server validates we're in the hub
    /// and replies with a fresh `StashSync`.
    pub fn request_open_stash(&mut self) {
        log::debug!("net: -> OpenStash");
        self.send(Channel::Control, &ClientMsg::OpenStash);
    }

    /// End the active stash session. Future deposit / withdraw
    /// requests are dropped server-side until a fresh
    /// `OpenStash` arrives.
    pub fn request_close_stash(&mut self) {
        log::debug!("net: -> CloseStash");
        self.send(Channel::Control, &ClientMsg::CloseStash);
    }

    /// Move the bag item at `inventory_index` into stash tab
    /// `tab_index`. Server replies with fresh `InventorySync`
    /// + `StashSync`.
    pub fn request_deposit_to_stash(&mut self, inventory_index: u32, tab_index: u8) {
        log::debug!("net: -> DepositToStash idx={inventory_index} tab={tab_index}");
        self.send(
            Channel::Control,
            &ClientMsg::DepositToStash {
                inventory_index,
                tab_index,
            },
        );
    }

    /// Move the stash item at `(tab_index, stash_index)` back
    /// into the bag. Server replies with fresh `InventorySync`
    /// + `StashSync`.
    pub fn request_withdraw_from_stash(&mut self, tab_index: u8, stash_index: u32) {
        log::debug!("net: -> WithdrawFromStash tab={tab_index} idx={stash_index}");
        self.send(
            Channel::Control,
            &ClientMsg::WithdrawFromStash {
                tab_index,
                stash_index,
            },
        );
    }

    /// Deposit the bag item at `inventory_index` into a specific
    /// `(tab_index, stash_index)`. Swap-or-place: if the stash
    /// slot is occupied, the prior occupant moves back to the
    /// freed bag slot. Server replies with fresh
    /// `InventorySync` + `StashSync`.
    pub fn request_deposit_to_stash_slot(
        &mut self,
        inventory_index: u32,
        tab_index: u8,
        stash_index: u32,
    ) {
        log::debug!(
            "net: -> DepositToStashSlot inv={inventory_index} tab={tab_index} stash={stash_index}"
        );
        self.send(
            Channel::Control,
            &ClientMsg::DepositToStashSlot {
                inventory_index,
                tab_index,
                stash_index,
            },
        );
    }

    /// Withdraw the stash item at `(tab_index, stash_index)`
    /// into a specific `inventory_index`. Mirror of
    /// `request_deposit_to_stash_slot`.
    pub fn request_withdraw_from_stash_slot(
        &mut self,
        tab_index: u8,
        stash_index: u32,
        inventory_index: u32,
    ) {
        log::debug!(
            "net: -> WithdrawFromStashSlot tab={tab_index} stash={stash_index} inv={inventory_index}"
        );
        self.send(
            Channel::Control,
            &ClientMsg::WithdrawFromStashSlot {
                tab_index,
                stash_index,
                inventory_index,
            },
        );
    }

    /// Spend shards on a new stash tab. Server validates cost.
    pub fn request_buy_stash_tab(&mut self) {
        log::debug!("net: -> BuyStashTab");
        self.send(Channel::Control, &ClientMsg::BuyStashTab);
    }

    /// Equip a stash item directly (bypassing the bag).
    pub fn request_equip_from_stash(&mut self, tab_index: u8, stash_index: u32) {
        log::debug!("net: -> EquipFromStash tab={tab_index} idx={stash_index}");
        self.send(
            Channel::Control,
            &ClientMsg::EquipFromStash {
                tab_index,
                stash_index,
            },
        );
    }

    /// Unequip the item in `slot` directly into a specific
    /// stash cell (bypassing the bag).
    pub fn request_unequip_to_stash_slot(&mut self, slot: u8, tab_index: u8, stash_index: u32) {
        log::debug!("net: -> UnequipToStashSlot slot={slot} tab={tab_index} idx={stash_index}");
        self.send(
            Channel::Control,
            &ClientMsg::UnequipToStashSlot {
                slot,
                tab_index,
                stash_index,
            },
        );
    }

    /// Auto-sort the bag. Server compacts items by rarity /
    /// ilvl / footprint area.
    pub fn request_sort_inventory(&mut self) {
        log::debug!("net: -> SortInventory");
        self.send(Channel::Control, &ClientMsg::SortInventory);
    }

    /// Auto-sort one stash tab.
    pub fn request_sort_stash_tab(&mut self, tab_index: u8) {
        log::debug!("net: -> SortStashTab tab={tab_index}");
        self.send(Channel::Control, &ClientMsg::SortStashTab { tab_index });
    }

    /// Rename `tab_index`. The server clamps the name and
    /// rejects empty strings.
    pub fn request_rename_stash_tab(&mut self, tab_index: u8, name: String) {
        log::debug!("net: -> RenameStashTab tab={tab_index} name={name:?}");
        self.send(
            Channel::Control,
            &ClientMsg::RenameStashTab { tab_index, name },
        );
    }

    /// Recolor `tab_index`. `color` is packed `0xRRGGBB`.
    pub fn request_recolor_stash_tab(&mut self, tab_index: u8, color: u32) {
        log::debug!("net: -> RecolorStashTab tab={tab_index} color={color:#08X}");
        self.send(
            Channel::Control,
            &ClientMsg::RecolorStashTab { tab_index, color },
        );
    }

    /// Reorder the bag: swap the items at slots `a` and `b`.
    /// Either may be empty (past the current bag length) — the
    /// server will grow the bag with placeholders to fit and
    /// then trim back down.
    pub fn request_swap_inventory_slots(&mut self, a: u32, b: u32) {
        log::debug!("net: -> SwapInventorySlots {a} <-> {b}");
        self.send(Channel::Control, &ClientMsg::SwapInventorySlots { a, b });
    }

    /// Reorder the stash: swap the items at slots `a` and `b`.
    /// Either may be empty (past the current stash length) —
    /// the server will grow the stash with placeholders to fit
    /// and then trim back down. Requires an open stash session.
    pub fn request_swap_stash_slots(&mut self, tab_index: u8, a: u32, b: u32) {
        log::debug!("net: -> SwapStashSlots tab={tab_index} {a} <-> {b}");
        self.send(
            Channel::Control,
            &ClientMsg::SwapStashSlots { tab_index, a, b },
        );
    }

    /// Drop the bag item at `inventory_index` onto the ground at
    /// the picker's current position. Server spawns a fresh
    /// `ServerLoot` entity and broadcasts `LootDropped`.
    pub fn request_drop_inventory_item(&mut self, inventory_index: u32) {
        log::debug!("net: -> DropInventoryItem idx={inventory_index}");
        self.send(
            Channel::Control,
            &ClientMsg::DropInventoryItem { inventory_index },
        );
    }

    /// Drop the equipped item in `slot` onto the ground at
    /// the player's position. Server validates + responds
    /// with fresh `InventorySync` + `EquipmentSync`.
    pub fn request_drop_equipped_item(&mut self, slot: u8) {
        log::debug!("net: -> DropEquippedItem slot={slot}");
        self.send(Channel::Control, &ClientMsg::DropEquippedItem { slot });
    }

    /// Salvage the bag item at `inventory_index` for shards.
    /// Server validates the slot and the item's anchored flag,
    /// then replies with fresh `InventorySync` + `ShardsSync`.
    pub fn request_salvage_inventory_item(&mut self, inventory_index: u32) {
        log::debug!("net: -> SalvageInventoryItem idx={inventory_index}");
        self.send(
            Channel::Control,
            &ClientMsg::SalvageInventoryItem { inventory_index },
        );
    }

    /// Bulk-salvage every non-anchored bag item whose rarity is
    /// at most `rarity_max` (Common=0, Magic=1, Rare=2,
    /// Legendary=3). Server replies with fresh
    /// `InventorySync` + `ShardsSync`.
    pub fn request_salvage_inventory_bulk(&mut self, rarity_max: u8) {
        log::debug!("net: -> SalvageInventoryBulk rarity_max={rarity_max}");
        self.send(
            Channel::Control,
            &ClientMsg::SalvageInventoryBulk { rarity_max },
        );
    }

    /// Move the equipped item at `slot` into the bag at
    /// `inventory_index`. Drag-drop counterpart to
    /// `request_unequip_item` (which always appends to the end).
    pub fn request_unequip_to_bag_slot(&mut self, slot: u8, inventory_index: u32) {
        log::debug!("net: -> UnequipToBagSlot slot={slot} idx={inventory_index}");
        self.send(
            Channel::Control,
            &ClientMsg::UnequipToBagSlot {
                slot,
                inventory_index,
            },
        );
    }

    /// Swap the contents of two equipment slots (e.g. ring1
    /// ↔ ring2). Server validates `Equipment::accepts` in
    /// both directions and rejects mismatched pairs.
    pub fn request_swap_equip_slots(&mut self, a: u8, b: u8) {
        log::debug!("net: -> SwapEquipSlots {a} <-> {b}");
        self.send(Channel::Control, &ClientMsg::SwapEquipSlots { a, b });
    }

    /// Ship a chat line. `target` is meaningful only on the
    /// whisper channel; ignored everywhere else. Length /
    /// rate-limit / channel validation happen server-side.
    pub fn send_chat(&mut self, channel: u8, target: Option<String>, text: String) {
        log::debug!("net: -> ChatSend channel={channel} target={target:?}");
        self.send(
            Channel::Control,
            &ClientMsg::ChatSend {
                channel,
                target,
                text,
            },
        );
    }

    /// Drain every inbound chat line received since the last
    /// drain. Called once per frame by the binary; the entries
    /// flow into `GameState.chat`.
    pub fn take_pending_chats(&mut self) -> Vec<super::PendingChat> {
        self.pending_chats.drain(..).collect()
    }
}
