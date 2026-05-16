//! Which rasterised face in the overlay atlas paints a string.

/// Body copy uses PT Serif; headers / panel titles use Share Tech.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UiTextFont {
    #[default]
    Body,
    Header,
}
