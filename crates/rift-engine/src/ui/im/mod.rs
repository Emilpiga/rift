//! Immediate-mode UI core.
//!
//! See `ARCHITECTURE.md` (UI section) for the layered design.
//! Quick map:
//!
//! - [`Ui`]            — per-frame context, threads through every draw call
//! - [`UiState`]       — cross-frame state (focus, hover, drag, modal stack)
//! - [`Theme`]         — colors / spacing / font sizes
//! - [`Id`]            — stable widget identity for hover & focus
//! - [`Rect`] / [`Pos2`] / [`Vec2`] / [`Pad`] — geometry primitives
//! - [`Color`] / [`Stroke`] — style primitives
//! - [`Response`]      — return value from interactive widgets
//! - [`Layer`]         — z-ordering bucket for draw commands
//!
//! Widgets (button, text_field, item_slot, …) ship in subsequent
//! landings and live alongside this module under `widgets/`.

mod color;
mod id;
mod layer;
mod anchor;
mod fit;
mod rect;
mod response;
mod state;
mod theme;
mod ui;
pub mod widgets;
mod world_ui;

pub use anchor::Anchor;
pub use fit::FitScale;
pub use color::{Color, Stroke};
pub use id::Id;
pub use layer::Layer;
pub use rect::{Pad, Pos2, Rect, Vec2};
pub use response::Response;
pub use state::{DragState, Modal, UiState};
pub use theme::{Colors, Fonts, Spacing, Theme, DEFAULT_THEME};
pub use ui::{DragSourceResponse, DroppedPayload, Ui, UiOutput};
pub use widgets::{
    hp_color, item_tooltip_lines, Banner, BannerStyle, Button, ButtonVariant, Frame, ItemSlot,
    ProgressBar, SlotInteraction, TextField, Tooltip, TooltipLine,
};
pub use world_ui::WorldUi;
