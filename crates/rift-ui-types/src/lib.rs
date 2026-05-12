//! # rift-ui-types
//!
//! Plain-data types shared across the UI hot-reload boundary.
//!
//! ## Why this crate exists
//!
//! The Rift UI is built for fast iteration via dynamic library
//! reloading (see `rift-ui-hot`). For that to work without
//! crashes, **every type that crosses the boundary between the
//! host binary (`rift-client`) and the hot-reloadable widget
//! crate (`rift-ui`) must be defined in a third crate that
//! neither of them owns**. That third crate is this one.
//!
//! If the host and the loaded library each had their own copy
//! of, say, `RosterView` (because they each compiled their own
//! version from a `pub struct` defined inside `rift-ui`), then
//! after a reload the host would still hold the *old* layout
//! while the new library would write the *new* layout into the
//! same memory. That is undefined behaviour and will crash —
//! sometimes immediately, sometimes 30 seconds later in a
//! totally unrelated codepath.
//!
//! By contract: any `struct`, `enum`, or trait whose value is
//! read on one side of the reload boundary and produced on the
//! other side **must live here**.
//!
//! ## What goes in this crate
//!
//! - **View models**: `RosterView`, `CreateFormView`,
//!   `InventoryView`, etc. — lightweight snapshots of game
//!   state shaped for the UI to render. Built fresh by
//!   `rift-client` every frame from the authoritative game
//!   state. Plain Copy/Clone data, no `&` to other game
//!   structs.
//! - **Action enums**: `SelectAction`, `InventoryAction`,
//!   `HudAction`. Returned by widget functions; the host
//!   matches on them and dispatches to game logic. Plain
//!   data.
//! - **Theme**: colors, paddings, font sizes, spacing
//!   constants. Loaded from `assets/ui/theme.toml` at startup
//!   and (in dev) hot-reloaded on file change. `serde`-
//!   deserializable.
//! - **IDs / handles**: stable opaque identifiers for
//!   characters, items, etc. Never raw pointers.
//!
//! ## What does NOT go in this crate
//!
//! - Anything from `rift-engine` (`Renderer`, `Ui`, GPU
//!   resources). Those types are owned by the host and crossed
//!   over the boundary by `&mut` reference, not by value, so
//!   they don't need to live here.
//! - Anything from `rift-game` whose layout changes during
//!   active development (full `CharacterProfile`, full ECS
//!   components). Wrap them into a flatter view model here
//!   instead.
//! - Functions, methods on widget structs, closures. Those
//!   are the *behaviour* and live in `rift-ui`.
//!
//! ## Relationship to other crates
//!
//! ```text
//!     rift-ui-types  (this crate — pure data)
//!         ▲   ▲
//!         │   │
//!     rift-ui   rift-client (host)
//!         ▲       │
//!         │       │ loads dynamically in dev,
//!         │       │ statically in release
//!     rift-ui-hot ┘
//! ```
//!
//! `rift-ui` and `rift-client` both depend on `rift-ui-types`,
//! never on each other directly. The host calls into widget
//! functions through the `rift-ui-hot` shim.

#![forbid(unsafe_code)]

// Module scaffolding — populated as screens are ported over to
// the hot-reload pipeline. Each screen gets its own submodule
// so a tweak to the inventory view models doesn't force a
// recompile of the character-select view models.
pub mod character_select;
pub mod chat;
pub mod hud;
pub mod inventory;
pub mod pause_menu;
pub mod settings;
// pub mod theme;
