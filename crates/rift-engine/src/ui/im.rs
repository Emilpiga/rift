//! Re-export of the standalone `rift-ui-im` immediate-mode UI crate.
//!
//! The module used to live in-tree; it was extracted so `rift-ui`
//! can build as a Windows `dylib` without dragging the engine's
//! whole transitive object graph into its export table. The
//! engine continues to consume widgets via the same path
//! (`rift_engine::ui::im::*`) — call sites didn't have to change.
pub use rift_ui_im::*;
