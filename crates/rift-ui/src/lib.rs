//! # rift-ui
//!
//! All immediate-mode widget code for Rift's screens lives
//! here. This is the crate you edit to tweak the UI.
//!
//! ## Why this crate exists (separately from `rift-client`)
//!
//! Two reasons, in priority order:
//!
//! 1. **Hot-reload.** `rift-ui-hot` wraps this crate as a
//!    `cdylib` and `rift-client` loads that dynamic library
//!    via `hot-lib-reloader` in dev builds. When you save a
//!    file in `rift-ui`, only this crate (and the tiny
//!    `rift-ui-hot` shim) recompiles, the host re-loads the
//!    new `.dll` / `.so`, and your change is live in seconds
//!    — no relogin, no recharacter-select.
//! 2. **Iteration speed even without hot-reload.** Even when
//!    you `cargo build`, only the changed widget crate is
//!    rebuilt and re-linked, instead of relinking the entire
//!    `rift-client` binary (which pulls in vulkan, audio,
//!    networking, the entire engine). The `cargo check` loop
//!    on a UI tweak is dominated by `rift-ui` alone.
//!
//! ## The hot-reload contract
//!
//! Every public function in this crate that the host calls is
//! a candidate for live-reloading. To keep that safe:
//!
//! - **No state owned here may outlive a frame.** All
//!   persistent UI state (selected character index, typed
//!   text in the create form, scroll positions) lives in
//!   `rift-client` and is passed in by `&mut`.
//! - **No types defined here may cross the boundary.**
//!   Inputs and outputs use types from `rift-ui-types`. Use
//!   `&mut Ui` / `&mut Renderer` from `rift-engine` freely
//!   because those types live in a crate that does *not*
//!   reload (the engine is statically linked into the host).
//! - **Function signatures are the boundary.** Body changes
//!   are free; signature changes require a full restart.
//!   Plan your entry points to take broad, stable inputs
//!   (a whole `RosterView`, not 12 individual fields).
//! - **No `static mut`, no `lazy_static`, no `OnceCell`** in
//!   this crate. All those get reset on reload, surprising
//!   any code that captured a reference. Caches live in
//!   `rift-client`.
//!
//! ## Shape of a typical entry point
//!
//! ```ignore
//! pub fn frame_character_select(
//!     ui: &mut rift_ui_im::Ui,
//!     view: &rift_ui_types::CharacterSelectView,
//!     state: &mut rift_ui_types::CharacterSelectState,
//! ) -> rift_ui_types::SelectAction {
//!     // ... immediate-mode widget calls ...
//! }
//! ```
//!
//! Read-only `view` is built fresh by the host every frame
//! from authoritative game state. `state` is the small bag of
//! UI-only memory (text fields, hover indices) the host owns
//! and threads back in. The returned `SelectAction` is the
//! only way the widget communicates back; the host dispatches
//! it.
//!
//! ## Relationship to other crates
//!
//! See `rift-ui-types` crate-level docs for the full diagram.
//! In short:
//!
//! - depends on: `rift-ui-im` (UI primitives & traits), `rift-ui-types` (data)
//! - depended on by: `rift-client` (statically linked)
//! - MUST NOT depend on: `rift-engine`, `rift-client`, `rift-game`,
//!   `rift-net`, `rift-server`, `rift-persistence`.

#![forbid(unsafe_code)]

// Module scaffolding — populated as screens are ported. Each
// screen is its own module so a tweak to inventory doesn't
// invalidate the character-select compile cache.
pub mod character_select;
pub mod chat;
pub mod hud;
pub mod inventory;
pub mod pause_menu;
pub mod settings;
// pub mod chat;
