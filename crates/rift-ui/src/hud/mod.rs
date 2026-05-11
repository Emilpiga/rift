//! HUD widgets — bottom-center vitals stack + ability action
//! bar. Hot-reloadable; reads pre-flattened views from
//! `rift_ui_types::hud` so no `rift_game` types appear here.
//!
//! Visual language matches the inventory: carved-stone frame
//! chrome, gold hairlines, theme.scale-driven sizing. Bars use
//! `rift_ui_im::ProgressBar`; the bar stack is wrapped in a
//! `Frame::stone` so it reads as a single plaque-mounted
//! cluster rather than three floating rectangles.

mod ability_bar;
mod minimap;
mod vitals;

pub use ability_bar::{frame_ability_bar, VITALS_BOTTOM_OFFSET_BASE};
pub use minimap::frame_minimap;
pub use vitals::frame_vitals;
