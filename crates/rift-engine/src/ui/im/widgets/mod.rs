//! Reusable composite widgets.
//!
//! Each module here is independent — pull in only what you need.
//! Widgets call into the [`Ui`](super::Ui) primitive draw helpers
//! and never touch `OverlayBatch` directly, so layer ordering and
//! input claiming stay consistent.

pub mod button;
pub mod frame;
pub mod item_slot;
pub mod progress_bar;
pub mod text_field;
pub mod tooltip;

pub use button::{Button, ButtonVariant};
pub use frame::Frame;
pub use item_slot::{ItemSlot, SlotInteraction};
pub use progress_bar::{hp_color, ProgressBar};
pub use text_field::{label, text_field, title, TextField};
pub use tooltip::{item_tooltip_lines, tooltip_at_mouse, Tooltip, TooltipLine};
