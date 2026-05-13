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
//!
//! Engine boundary
//! ---------------
//! This crate intentionally does NOT depend on `rift-engine`. The
//! two concerns that would otherwise pull it in — emitting
//! pixels (Vulkan overlay) and reading raw input (winit) — are
//! abstracted behind the [`DrawList`] and [`UiInput`] traits
//! defined here. The engine provides production impls in
//! `rift_engine::renderer::OverlayBatch` / `rift_engine::input::Input`.
//! Decoupling here is what keeps `rift-ui`'s dylib export count
//! under Windows' 65 535-symbol cap so the UI can be hot-reloaded.

mod anchor;
mod color;
mod draw_list;
mod fit;
mod id;
mod im_key;
mod layer;
mod layout;
mod rect;
mod response;
mod state;
mod theme;
mod ui;
mod ui_input;
pub mod widgets;
mod world_ui;

pub use anchor::Anchor;
pub use color::{Color, Stroke};
pub use draw_list::DrawList;
pub use fit::FitScale;
pub use id::Id;
pub use im_key::ImKey;
pub use layer::Layer;
pub use layout::{Column, CrossAlign, Row, Sized};
pub use rect::{Pad, Pos2, Rect, Vec2};
pub use response::Response;
pub use state::{DragState, Modal, TextSelection, UiState};
pub use theme::{Colors, Fonts, Spacing, Theme, DEFAULT_THEME};
pub use ui::{DragSourceResponse, DroppedPayload, Ui, UiOutput};
pub use ui_input::{KeyEvent, UiInput};
pub use widgets::{
    hp_color, item_tooltip_lines, Banner, BannerStyle, Button, ButtonSize, ButtonVariant, Frame,
    InlineEditOutcome, InlineEditState, ItemSlot, MiniButton, MiniButtonFills, MiniButtonResponse,
    PanZoom, PanZoomState, PanZoomTransform, ProgressBar, SlotInteraction, TextField, Tooltip,
    TooltipLine, TooltipLineDecor, TwoStageConfirm, TwoStageOutcome,
};
pub use world_ui::WorldUi;
