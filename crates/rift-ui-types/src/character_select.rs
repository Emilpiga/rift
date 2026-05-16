//! Character-select screen view models + action enums.
//!
//! Read by the widget functions in `rift-ui::character_select`,
//! written by the host (`rift-client::game::character_select`)
//! every frame.

/// Per-frame snapshot for the "loading roster…" view.
///
/// The widget owns no state — the dots animation is driven by
/// `anim_time`, which the host advances. Account name is
/// pre-resolved at startup (Steam ticket or dev HMAC) and
/// passed in.
#[derive(Clone, Debug)]
pub struct LoadingRosterView<'a> {
    /// Display identity, e.g. `steam:76561198…` or `dev:alice`.
    /// Shown to the player so they can see whose roster is
    /// loading. Borrowed for the frame; never stored across
    /// the boundary.
    pub account_name: &'a str,
    /// Monotonic seconds. Only its fractional value matters
    /// (drives the trailing-dots animation).
    pub anim_time: f32,
}

/// Maximum roster size. Mirrors `rift_game::character::MAX_CHARACTERS`;
/// duplicated here so this crate can stay free of a `rift-game`
/// dep. Kept in sync via a `const_assert` in the host adapter.
pub const MAX_CHARACTER_SLOTS: usize = 5;

/// View of a single character row in the roster panel.
#[derive(Clone, Debug)]
pub struct RosterEntryView<'a> {
    pub name: &'a str,
    pub level: u32,
    /// Pre-formatted gender label ("Male" / "Female") so the
    /// widget crate doesn't need to depend on `rift-game`'s
    /// `Gender` enum.
    pub gender_label: &'a str,
}

/// Per-frame snapshot of the roster list view.
#[derive(Clone, Debug)]
pub struct RosterView<'a> {
    /// Filled slots in roster order. Length ≤ `MAX_CHARACTER_SLOTS`.
    pub entries: &'a [RosterEntryView<'a>],
    /// Currently-selected slot, if any. The widget renders the
    /// matching row's name container in the red `selected`
    /// style and enables the panel-level Play / Delete buttons.
    pub selected: Option<usize>,
    /// `true` if the next empty row should render the "+ Create
    /// New" button (i.e. the roster isn't full).
    pub allow_create: bool,
}

/// What the user did on the roster screen this frame. The host
/// pattern-matches and applies the side effect.
#[derive(Clone, Debug)]
pub enum RosterAction {
    None,
    /// Click on a row — host should update its selected index.
    Select(usize),
    /// Panel-level Play button pressed; the host has already
    /// resolved which slot via its tracked selection.
    Play,
    /// Panel-level Delete button pressed; same resolution as
    /// Play.
    Delete,
    /// User picked the "+ Create New" row.
    Create,
    Quit,
}

/// Per-frame view-model for the create-character form. The
/// `&mut` borrows let the widget edit the host-owned form
/// state in place (text field, gender toggle) without round-
/// tripping every keystroke through an action enum.
#[derive(Debug)]
pub struct CreateFormView<'a> {
    pub name: &'a mut String,
    /// `true` = Male, `false` = Female. Bool rather than the
    /// game's `Gender` enum to keep this crate free of
    /// `rift-game` deps.
    pub gender_is_male: &'a mut bool,
    pub skin_tone: &'a mut u8,
    pub hair_style: &'a mut u8,
    pub eyebrow_style: &'a mut u8,
    pub hair_color: &'a mut u8,
    pub eyebrow_color: &'a mut u8,
    pub chest_size: &'a mut u8,
    /// Monotonic seconds; drives the text-field caret blink.
    pub anim_time: f32,
}

/// User action on the create form.
#[derive(Clone, Debug)]
pub enum CreateAction {
    None,
    Confirm,
    Cancel,
}

/// View-model for the delete-confirmation modal.
#[derive(Clone, Debug)]
pub struct DeleteConfirmView<'a> {
    pub character_name: &'a str,
}

/// User action on the delete-confirm modal.
#[derive(Clone, Debug)]
pub enum DeleteAction {
    None,
    Confirm,
    Cancel,
}
