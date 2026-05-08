//! Two-stage "Salvage Trash" confirmation timing helpers.
//!
//! The Salvage Trash button has a 2-stage commit: first click
//! arms it (label flips to "Confirm? Click again"), second
//! click within [`SALVAGE_CONFIRM_WINDOW_S`] actually fires the
//! bulk salvage. Auto-disarms after the window expires so a
//! stale arm can't surprise the player on a later open.
//!
//! Lives in its own module so the orchestrator and bag panel
//! can share the constant without one forcing a re-import on
//! the other.

use std::time::Instant;

use rift_game::loot::Item;

/// Window (seconds) the "Salvage Trash" button stays armed
/// after the first click. A second click within this window
/// commits the bulk salvage; otherwise the button auto-disarms
/// and the player has to click twice again.
pub const SALVAGE_CONFIRM_WINDOW_S: f64 = 3.0;

/// Process-wide monotonic epoch for confirmation timestamps.
/// Lazily initialised on first call so we don't pay an
/// `Instant::now()` cost during static init.
pub fn ui_now() -> f64 {
    use std::sync::OnceLock;
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let epoch = EPOCH.get_or_init(Instant::now);
    Instant::now().duration_since(*epoch).as_secs_f64()
}

/// Result of scanning the bag for items the bulk-salvage path
/// would consume. Mirrors the server's
/// `salvage_inventory_bulk` filter (Common+Magic only, skip
/// anchored) so the button label can preview the exact yield
/// before the player clicks.
#[derive(Clone, Copy, Debug, Default)]
pub struct BulkSalvagePreview {
    pub count: u32,
    pub yield_shards: u32,
}

impl BulkSalvagePreview {
    pub fn scan(items: &[Option<Item>]) -> Self {
        let mut c: u32 = 0;
        let mut y: u32 = 0;
        for slot in items.iter() {
            if let Some(it) = slot {
                if !it.anchored && (it.rarity as u8) <= rift_game::loot::Rarity::Magic as u8 {
                    c += 1;
                    y = y.saturating_add(rift_game::loot::salvage_yield(it.rarity, it.ilvl));
                }
            }
        }
        Self {
            count: c,
            yield_shards: y,
        }
    }
}
