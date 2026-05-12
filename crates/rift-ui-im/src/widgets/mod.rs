//! Reusable composite widgets.
//!
//! Each module here is independent — pull in only what you need.
//! Widgets call into the [`Ui`](super::Ui) primitive draw helpers
//! and never touch `OverlayBatch` directly, so layer ordering and
//! input claiming stay consistent.

pub mod banner;
pub mod button;
pub mod frame;
pub mod inline_edit;
pub mod item_slot;
pub mod mini_button;
pub mod progress_bar;
pub mod text_field;
pub mod tooltip;
pub mod two_stage_confirm;

pub use banner::{Banner, BannerStyle};
pub use button::{Button, ButtonSize, ButtonVariant};
pub use frame::Frame;
pub use inline_edit::{InlineEditOutcome, InlineEditState};
pub use item_slot::{ItemSlot, SlotInteraction};
pub use mini_button::{MiniButton, MiniButtonFills, MiniButtonResponse};
pub use progress_bar::{hp_color, ProgressBar};
pub use text_field::{label, text_field, title, TextField};
pub use tooltip::{item_tooltip_lines, tooltip_at_mouse, Tooltip, TooltipLine, TooltipLineDecor};
pub use two_stage_confirm::{TwoStageConfirm, TwoStageOutcome};
