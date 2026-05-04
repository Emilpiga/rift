use crate::input::Input;
use crate::loot::inventory::{Equipment, Inventory};
use crate::loot::item::{Item, ItemSlot, ItemRarity};
use crate::renderer::overlay::OverlayBatch;
use winit::keyboard::KeyCode;

/// Which slot category an item source belongs to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ItemSource {
    Backpack(usize),
    Equipped(ItemSlot),
}

/// Drag-and-drop state.
#[derive(Clone, Debug)]
enum DragState {
    None,
    Dragging {
        source: ItemSource,
        /// Snapshot of the item being dragged (for rendering ghost).
        rarity: ItemRarity,
        _name: String,
    },
}

/// The inventory UI state machine.
pub struct InventoryUI {
    pub open: bool,
    drag: DragState,
    pub tooltip_item: Option<TooltipData>,
    pub compare_item: Option<TooltipData>,
    /// Set when an item is dragged out of the inventory (should be dropped on ground).
    pub dropped_item: Option<Item>,
}

/// Data for rendering a tooltip.
#[derive(Clone, Debug)]
pub struct TooltipData {
    pub name: String,
    pub rarity: ItemRarity,
    pub damage: f32,
    pub defense: f32,
    pub affixes: Vec<String>,
    pub screen_x: f32,
    pub screen_y: f32,
}

impl TooltipData {
    fn from_item(item: &Item, x: f32, y: f32) -> Self {
        let affixes = item.affixes.iter().map(|a| a.display()).collect();
        Self {
            name: item.display_name.clone(),
            rarity: item.rarity,
            damage: item.total_damage(),
            defense: item.total_defense(),
            affixes,
            screen_x: x,
            screen_y: y,
        }
    }
}

// Layout constants
const SLOT_SIZE: f32 = 40.0;
const SLOT_GAP: f32 = 4.0;
const INV_COLS: usize = 5;
const INV_ROWS: usize = 4;
const PANEL_PADDING: f32 = 12.0;
const HEADER_HEIGHT: f32 = 24.0;

/// Equipment slot positions relative to the equipment panel origin.
const EQUIP_LAYOUT: [(ItemSlot, f32, f32); 6] = [
    (ItemSlot::Helmet, 1.0, 0.0),   // top center
    (ItemSlot::Amulet, 2.0, 0.0),   // top right
    (ItemSlot::Chest, 1.0, 1.0),    // middle center
    (ItemSlot::Weapon, 0.0, 1.0),   // middle left
    (ItemSlot::Ring, 2.0, 1.0),     // middle right
    (ItemSlot::Boots, 1.0, 2.0),    // bottom center
];

impl InventoryUI {
    pub fn new() -> Self {
        Self {
            open: false,
            drag: DragState::None,
            tooltip_item: None,
            compare_item: None,
            dropped_item: None,
        }
    }

    /// Process input and update UI state. Returns true if the UI consumed the click.
    pub fn update(
        &mut self,
        input: &Input,
        inventory: &mut Inventory,
        equipment: &mut Equipment,
        screen_w: f32,
        screen_h: f32,
    ) -> bool {
        // Toggle inventory with Tab
        if input.key_just_pressed(KeyCode::Tab) {
            self.open = !self.open;
            if !self.open {
                self.drag = DragState::None;
                self.tooltip_item = None;
                self.compare_item = None;
            }
        }

        if !self.open {
            return false;
        }

        let (mx, my) = input.mouse_pos();
        let shift_held = input.is_key_held(KeyCode::ShiftLeft) || input.is_key_held(KeyCode::ShiftRight);

        // Compute panel positions
        let (eq_origin_x, eq_origin_y) = self.equip_panel_origin(screen_w, screen_h);
        let (inv_origin_x, inv_origin_y) = self.backpack_panel_origin(screen_w, screen_h);

        // Determine what the mouse is hovering over
        let hovered = self.hit_test(mx, my, eq_origin_x, eq_origin_y, inv_origin_x, inv_origin_y, inventory);

        // Update tooltip
        self.tooltip_item = None;
        self.compare_item = None;
        if let Some(source) = &hovered {
            if let Some(item) = self.get_item(source, inventory, equipment) {
                self.tooltip_item = Some(TooltipData::from_item(item, mx + 16.0, my));

                // Shift-compare: if hovering a backpack item, show equipped item in same slot
                if shift_held {
                    if let ItemSource::Backpack(idx) = source {
                        if let Some(slot) = inventory.backpack.get(*idx).and_then(|i| i.slot()) {
                            if let Some(equipped) = equipment.get(slot) {
                                self.compare_item = Some(TooltipData::from_item(
                                    equipped,
                                    mx + 240.0,
                                    my,
                                ));
                            }
                        }
                    } else if let ItemSource::Equipped(slot) = source {
                        // Hovering equipped, show best matching backpack item
                        if let Some(equipped) = equipment.get(*slot) {
                            let _ = equipped; // tooltip already set
                        }
                    }
                }
            }
        }

        // Handle left-click (start drag / drop)
        if input.left_clicked() {
            match &self.drag {
                DragState::None => {
                    // Start dragging if hovering an item
                    if let Some(source) = &hovered {
                        if let Some(item) = self.get_item(source, inventory, equipment) {
                            self.drag = DragState::Dragging {
                                source: *source,
                                rarity: item.rarity,
                                _name: item.display_name.clone(),
                            };
                        }
                    }
                    return hovered.is_some();
                }
                DragState::Dragging { source, .. } => {
                    let src = *source;
                    // Drop onto target
                    if let Some(target) = &hovered {
                        self.perform_drop(src, *target, inventory, equipment);
                    } else {
                        // Dropped outside inventory — drop item to ground
                        let item = self.take_item(&src, inventory, equipment);
                        if let Some(item) = item {
                            self.dropped_item = Some(item);
                        }
                    }
                    self.drag = DragState::None;
                    return true;
                }
            }
        }

        // Handle right-click (equip/unequip)
        if input.right_clicked() {
            if let Some(source) = &hovered {
                self.perform_right_click(*source, inventory, equipment);
                return true;
            }
        }

        hovered.is_some()
    }

    /// Right-click: equip from backpack, or unequip to backpack.
    fn perform_right_click(
        &self,
        source: ItemSource,
        inventory: &mut Inventory,
        equipment: &mut Equipment,
    ) {
        match source {
            ItemSource::Backpack(idx) => {
                // Equip the item (swap with current)
                if let Some(item) = inventory.remove_item(idx) {
                    if let Some(old) = equipment.equip(item) {
                        // Insert old item back at the same slot position
                        if idx <= inventory.backpack.len() {
                            inventory.backpack.insert(idx, old);
                        } else {
                            inventory.add_item(old);
                        }
                    }
                }
            }
            ItemSource::Equipped(slot) => {
                // Unequip to backpack
                if inventory.backpack.len() < inventory.max_backpack_size {
                    if let Some(item) = equipment.unequip(slot) {
                        inventory.add_item(item);
                    }
                }
            }
        }
    }

    /// Perform a drag-and-drop: move item from source to target.
    fn perform_drop(
        &self,
        source: ItemSource,
        target: ItemSource,
        inventory: &mut Inventory,
        equipment: &mut Equipment,
    ) {
        if source == target {
            return;
        }

        match (source, target) {
            // Backpack → Backpack: swap positions
            (ItemSource::Backpack(from), ItemSource::Backpack(to)) => {
                let len = inventory.backpack.len();
                if from < len && to < len {
                    inventory.backpack.swap(from, to);
                }
            }
            // Backpack → Equipment: equip, put old item in backpack slot
            (ItemSource::Backpack(idx), ItemSource::Equipped(_slot)) => {
                if let Some(item) = inventory.remove_item(idx) {
                    if let Some(old) = equipment.equip(item) {
                        // Insert back at same position
                        if idx <= inventory.backpack.len() {
                            inventory.backpack.insert(idx, old);
                        } else {
                            inventory.add_item(old);
                        }
                    }
                }
            }
            // Equipment → Backpack: unequip to that slot
            (ItemSource::Equipped(slot), ItemSource::Backpack(idx)) => {
                if let Some(item) = equipment.unequip(slot) {
                    // If target slot has an item, try to equip it
                    if idx < inventory.backpack.len() {
                        let target_item = inventory.backpack.remove(idx);
                        // Check if target item fits in the equipment slot
                        if target_item.slot() == Some(slot) {
                            equipment.equip(target_item);
                            inventory.backpack.insert(idx, item);
                        } else {
                            // Can't swap — put both back
                            inventory.backpack.insert(idx, target_item);
                            inventory.add_item(item);
                        }
                    } else {
                        inventory.add_item(item);
                    }
                }
            }
            // Equipment → Equipment: swap if same slot type (unlikely), otherwise ignore
            (ItemSource::Equipped(from_slot), ItemSource::Equipped(to_slot)) => {
                if from_slot != to_slot {
                    // Can't swap different slot types
                    return;
                }
            }
        }
    }

    fn get_item<'a>(
        &self,
        source: &ItemSource,
        inventory: &'a Inventory,
        equipment: &'a Equipment,
    ) -> Option<&'a Item> {
        match source {
            ItemSource::Backpack(idx) => inventory.backpack.get(*idx),
            ItemSource::Equipped(slot) => equipment.get(*slot),
        }
    }

    /// Remove an item from its source and return it.
    fn take_item(
        &self,
        source: &ItemSource,
        inventory: &mut Inventory,
        equipment: &mut Equipment,
    ) -> Option<Item> {
        match source {
            ItemSource::Backpack(idx) => {
                if *idx < inventory.backpack.len() {
                    Some(inventory.backpack.remove(*idx))
                } else {
                    None
                }
            }
            ItemSource::Equipped(slot) => equipment.unequip(*slot),
        }
    }

    fn hit_test(
        &self,
        mx: f32,
        my: f32,
        eq_x: f32,
        eq_y: f32,
        inv_x: f32,
        inv_y: f32,
        inventory: &Inventory,
    ) -> Option<ItemSource> {
        // Test equipment slots
        for &(slot, col, row) in &EQUIP_LAYOUT {
            let sx = eq_x + col * (SLOT_SIZE + SLOT_GAP);
            let sy = eq_y + row * (SLOT_SIZE + SLOT_GAP);
            if mx >= sx && mx < sx + SLOT_SIZE && my >= sy && my < sy + SLOT_SIZE {
                return Some(ItemSource::Equipped(slot));
            }
        }

        // Test backpack slots
        for i in 0..inventory.max_backpack_size {
            let col = i % INV_COLS;
            let row = i / INV_COLS;
            let sx = inv_x + col as f32 * (SLOT_SIZE + SLOT_GAP);
            let sy = inv_y + row as f32 * (SLOT_SIZE + SLOT_GAP);
            if mx >= sx && mx < sx + SLOT_SIZE && my >= sy && my < sy + SLOT_SIZE {
                return Some(ItemSource::Backpack(i));
            }
        }

        None
    }

    // --- Layout helpers ---

    fn equip_panel_origin(&self, screen_w: f32, screen_h: f32) -> (f32, f32) {
        // Equipment panel: left side of screen center
        let panel_w = 3.0 * (SLOT_SIZE + SLOT_GAP) + PANEL_PADDING * 2.0;
        let total_w = panel_w + PANEL_PADDING + self.backpack_panel_width();
        let start_x = (screen_w - total_w) / 2.0;
        let start_y = (screen_h - self.equip_panel_height()) / 2.0;
        (start_x + PANEL_PADDING, start_y + HEADER_HEIGHT + PANEL_PADDING)
    }

    fn backpack_panel_origin(&self, screen_w: f32, screen_h: f32) -> (f32, f32) {
        let eq_panel_w = 3.0 * (SLOT_SIZE + SLOT_GAP) + PANEL_PADDING * 2.0;
        let total_w = eq_panel_w + PANEL_PADDING + self.backpack_panel_width();
        let start_x = (screen_w - total_w) / 2.0 + eq_panel_w + PANEL_PADDING;
        let start_y = (screen_h - self.backpack_panel_height()) / 2.0;
        (start_x + PANEL_PADDING, start_y + HEADER_HEIGHT + PANEL_PADDING)
    }

    fn equip_panel_height(&self) -> f32 {
        3.0 * (SLOT_SIZE + SLOT_GAP) + HEADER_HEIGHT + PANEL_PADDING * 2.0
    }

    fn backpack_panel_width(&self) -> f32 {
        INV_COLS as f32 * (SLOT_SIZE + SLOT_GAP) + PANEL_PADDING * 2.0
    }

    fn backpack_panel_height(&self) -> f32 {
        INV_ROWS as f32 * (SLOT_SIZE + SLOT_GAP) + HEADER_HEIGHT + PANEL_PADDING * 2.0
    }

    // --- Rendering ---

    /// Render the full inventory UI into the overlay batch.
    pub fn render(
        &self,
        batch: &mut OverlayBatch,
        inventory: &Inventory,
        equipment: &Equipment,
        input: &Input,
        screen_w: f32,
        screen_h: f32,
    ) {
        if !self.open {
            return;
        }

        let (mx, my) = input.mouse_pos();

        // Semi-transparent background overlay
        batch.rect_px(0.0, 0.0, screen_w, screen_h, [0.0, 0.0, 0.0, 0.4], screen_w, screen_h);

        // --- Equipment Panel ---
        let eq_panel_w = 3.0 * (SLOT_SIZE + SLOT_GAP) + PANEL_PADDING * 2.0;
        let total_w = eq_panel_w + PANEL_PADDING + self.backpack_panel_width();
        let eq_panel_x = (screen_w - total_w) / 2.0;
        let eq_panel_y = (screen_h - self.equip_panel_height()) / 2.0;

        // Panel background
        batch.rect_px(
            eq_panel_x, eq_panel_y, eq_panel_w, self.equip_panel_height(),
            [0.08, 0.08, 0.12, 0.95], screen_w, screen_h,
        );
        // Header
        batch.text("Equipment", eq_panel_x + PANEL_PADDING, eq_panel_y + 4.0, 16.0, [0.9, 0.9, 0.9, 1.0], screen_w, screen_h);

        let (eq_origin_x, eq_origin_y) = self.equip_panel_origin(screen_w, screen_h);

        // Draw equipment slots
        for &(slot, col, row) in &EQUIP_LAYOUT {
            let sx = eq_origin_x + col * (SLOT_SIZE + SLOT_GAP);
            let sy = eq_origin_y + row * (SLOT_SIZE + SLOT_GAP);

            // Slot background
            let hovered = mx >= sx && mx < sx + SLOT_SIZE && my >= sy && my < sy + SLOT_SIZE;
            let bg_color = if hovered {
                [0.25, 0.25, 0.35, 0.9]
            } else {
                [0.15, 0.15, 0.22, 0.9]
            };
            batch.rect_px(sx, sy, SLOT_SIZE, SLOT_SIZE, bg_color, screen_w, screen_h);

            // Slot label
            let label = match slot {
                ItemSlot::Weapon => "W",
                ItemSlot::Helmet => "H",
                ItemSlot::Chest => "C",
                ItemSlot::Boots => "B",
                ItemSlot::Ring => "R",
                ItemSlot::Amulet => "A",
            };
            batch.text(label, sx + 2.0, sy + 2.0, 10.0, [0.4, 0.4, 0.5, 0.7], screen_w, screen_h);

            // Item fill
            if let Some(item) = equipment.get(slot) {
                let [r, g, b] = item.rarity.color();
                batch.rect_px(
                    sx + 4.0, sy + 4.0, SLOT_SIZE - 8.0, SLOT_SIZE - 8.0,
                    [r, g, b, 0.9], screen_w, screen_h,
                );
            }

            // Border
            let border_color = if hovered { [0.6, 0.6, 0.8, 0.9] } else { [0.3, 0.3, 0.4, 0.8] };
            self.draw_border(batch, sx, sy, SLOT_SIZE, SLOT_SIZE, border_color, screen_w, screen_h);
        }

        // --- Backpack Panel ---
        let bp_panel_x = eq_panel_x + eq_panel_w + PANEL_PADDING;
        let bp_panel_y = (screen_h - self.backpack_panel_height()) / 2.0;
        let bp_panel_w = self.backpack_panel_width();
        let bp_panel_h = self.backpack_panel_height();

        // Panel background
        batch.rect_px(
            bp_panel_x, bp_panel_y, bp_panel_w, bp_panel_h,
            [0.08, 0.08, 0.12, 0.95], screen_w, screen_h,
        );
        // Header
        batch.text("Backpack", bp_panel_x + PANEL_PADDING, bp_panel_y + 4.0, 16.0, [0.9, 0.9, 0.9, 1.0], screen_w, screen_h);

        let (inv_origin_x, inv_origin_y) = self.backpack_panel_origin(screen_w, screen_h);

        // Draw backpack grid
        for i in 0..inventory.max_backpack_size {
            let col = i % INV_COLS;
            let row = i / INV_COLS;
            let sx = inv_origin_x + col as f32 * (SLOT_SIZE + SLOT_GAP);
            let sy = inv_origin_y + row as f32 * (SLOT_SIZE + SLOT_GAP);

            let hovered = mx >= sx && mx < sx + SLOT_SIZE && my >= sy && my < sy + SLOT_SIZE;
            let bg_color = if hovered {
                [0.25, 0.25, 0.35, 0.9]
            } else {
                [0.12, 0.12, 0.18, 0.9]
            };
            batch.rect_px(sx, sy, SLOT_SIZE, SLOT_SIZE, bg_color, screen_w, screen_h);

            // Item
            if let Some(item) = inventory.backpack.get(i) {
                let [r, g, b] = item.rarity.color();
                batch.rect_px(
                    sx + 4.0, sy + 4.0, SLOT_SIZE - 8.0, SLOT_SIZE - 8.0,
                    [r, g, b, 0.9], screen_w, screen_h,
                );
            }

            let border_color = if hovered { [0.6, 0.6, 0.8, 0.9] } else { [0.2, 0.2, 0.3, 0.7] };
            self.draw_border(batch, sx, sy, SLOT_SIZE, SLOT_SIZE, border_color, screen_w, screen_h);
        }

        // --- Dragged item ghost ---
        if let DragState::Dragging { rarity, .. } = &self.drag {
            let [r, g, b] = rarity.color();
            batch.rect_px(
                mx - SLOT_SIZE / 2.0, my - SLOT_SIZE / 2.0,
                SLOT_SIZE, SLOT_SIZE,
                [r, g, b, 0.6], screen_w, screen_h,
            );
        }

        // --- Tooltips ---
        if let Some(tip) = &self.tooltip_item {
            self.render_tooltip(batch, tip, screen_w, screen_h);
        }
        if let Some(tip) = &self.compare_item {
            self.render_tooltip(batch, tip, screen_w, screen_h);
        }
    }

    fn render_tooltip(
        &self,
        batch: &mut OverlayBatch,
        tip: &TooltipData,
        screen_w: f32,
        screen_h: f32,
    ) {
        let name_size = 18.0;
        let stats_size = 15.0;
        let affix_size = 14.0;
        let line_h = 20.0;
        let padding = 12.0;

        let name_color = {
            let [r, g, b] = tip.rarity.color();
            [r, g, b, 1.0]
        };

        // Compute tooltip width based on content
        let name_w = batch.measure_text(&tip.name, name_size);
        let stats_text = format!("DMG: {:.0}  DEF: {:.0}", tip.damage, tip.defense);
        let stats_w = batch.measure_text(&stats_text, stats_size);
        let max_affix_w = tip.affixes.iter()
            .map(|a| batch.measure_text(a, affix_size))
            .fold(0.0_f32, f32::max);
        let content_w = name_w.max(stats_w).max(max_affix_w);
        let tip_w = (content_w + padding * 2.0).max(180.0);

        let num_affix_lines = tip.affixes.len();
        let total_lines = 1 + 1 + num_affix_lines; // name + stats + affixes
        let tip_h = padding * 2.0 + total_lines as f32 * line_h + 4.0;

        // Clamp tooltip position to screen
        let tx = tip.screen_x.min(screen_w - tip_w - 4.0).max(4.0);
        let ty = tip.screen_y.min(screen_h - tip_h - 4.0).max(4.0);

        // Background
        batch.rect_px(tx, ty, tip_w, tip_h, [0.05, 0.05, 0.08, 0.95], screen_w, screen_h);
        self.draw_border(batch, tx, ty, tip_w, tip_h, name_color, screen_w, screen_h);

        let mut y_cursor = ty + padding;

        // Name
        batch.text(&tip.name, tx + padding, y_cursor, name_size, name_color, screen_w, screen_h);
        y_cursor += line_h;

        // Stats line
        batch.text(&stats_text, tx + padding, y_cursor, stats_size, [0.7, 0.7, 0.7, 1.0], screen_w, screen_h);
        y_cursor += line_h;

        // Affixes
        for affix_text in &tip.affixes {
            batch.text(affix_text, tx + padding, y_cursor, affix_size, [0.4, 0.8, 0.4, 1.0], screen_w, screen_h);
            y_cursor += line_h;
        }
    }

    fn draw_border(
        &self,
        batch: &mut OverlayBatch,
        x: f32, y: f32, w: f32, h: f32,
        color: [f32; 4],
        screen_w: f32, screen_h: f32,
    ) {
        let t = 1.0; // border thickness
        batch.rect_px(x, y, w, t, color, screen_w, screen_h);         // top
        batch.rect_px(x, y + h - t, w, t, color, screen_w, screen_h); // bottom
        batch.rect_px(x, y, t, h, color, screen_w, screen_h);         // left
        batch.rect_px(x + w - t, y, t, h, color, screen_w, screen_h); // right
    }
}
