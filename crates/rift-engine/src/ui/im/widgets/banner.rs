//! Centered "card with a label" — the visual primitive shared by
//! the HUD prompts, portal banners, level-up shouts, vote-cooldown
//! notice, loot pickup hint, and the various small status pills.
//!
//! Three flavours covered by the same builder:
//!
//! - **Pill** — a single line of text inside a rounded background,
//!   no extra chrome. Used for "THE HUB", "ENTER THE PORTAL".
//! - **Toast** — a panel-framed card with padding; used for the
//!   F-prompt, loot prompt, vote cooldown.
//! - **Floating text** — no background at all, for the level-up
//!   shout that fades in/out.
//!
//! Positioning uses screen-relative anchors (`y_factor` in `0..1`)
//! so the banner stays in the same visual slot regardless of
//! resolution. Width auto-sizes to the text plus padding; the
//! caller can clamp via [`Banner::min_width`] when several
//! adjacent banners should agree on a width.

use super::super::{
    color::Color, ui::Ui,
};
use super::frame::Frame;
use crate::ui::im::rect::{Pad, Pos2, Rect};

/// Visual style of the background.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BannerStyle {
    /// Theme `Frame::panel` chrome (border, dim fill, padding).
    Panel,
    /// A single rounded rect — no border. Cheap and unobtrusive.
    Pill,
    /// No background at all, just text. For ephemeral shouts.
    None,
}

/// One centered banner. Built via the chained-setter pattern.
pub struct Banner<'a> {
    text: &'a str,
    text_size: Option<f32>,
    text_color: Option<Color>,
    fill: Option<Color>,
    style: BannerStyle,
    pad: Option<Pad>,
    /// `0..1` factor along the screen height where the banner top sits.
    y_factor: f32,
    min_width: Option<f32>,
}

impl<'a> Banner<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            text_size: None,
            text_color: None,
            fill: None,
            style: BannerStyle::Panel,
            pad: None,
            y_factor: 0.10,
            min_width: None,
        }
    }

    pub fn pill(mut self) -> Self {
        self.style = BannerStyle::Pill;
        self
    }

    pub fn floating(mut self) -> Self {
        self.style = BannerStyle::None;
        self
    }

    pub fn text_size(mut self, sz: f32) -> Self {
        self.text_size = Some(sz);
        self
    }

    pub fn text_color(mut self, c: Color) -> Self {
        self.text_color = Some(c);
        self
    }

    pub fn fill(mut self, c: Color) -> Self {
        self.fill = Some(c);
        self
    }

    pub fn pad(mut self, p: Pad) -> Self {
        self.pad = Some(p);
        self
    }

    /// Anchor along the screen height as a `0..1` factor.
    pub fn y_factor(mut self, f: f32) -> Self {
        self.y_factor = f;
        self
    }

    /// Clamp the outer width to at least this many pixels — the
    /// caller is expected to pass a *theme-scaled* value.
    pub fn min_width(mut self, w: f32) -> Self {
        self.min_width = Some(w);
        self
    }

    /// Draw the banner; returns the final outer rect so callers
    /// can stack additional banners or draw decorations relative
    /// to it.
    pub fn show(self, ui: &mut Ui<'_>) -> Rect {
        let theme = *ui.theme();
        let s = theme.scale;
        let screen = ui.screen_size();

        let text_size = self.text_size.unwrap_or(theme.fonts.size_md);
        let text_color = self.text_color.unwrap_or(theme.colors.text);
        let pad = self.pad.unwrap_or_else(|| match self.style {
            BannerStyle::Panel => Pad::symmetric(18.0 * s, 6.0 * s),
            BannerStyle::Pill => Pad::symmetric(14.0 * s, 4.0 * s),
            BannerStyle::None => Pad::ZERO,
        });

        let text_w = ui.measure_text(self.text, text_size);
        let inner_w = text_w.max(self.min_width.unwrap_or(0.0));
        let outer_w = inner_w + pad.left + pad.right;
        let outer_h = text_size + pad.top + pad.bottom;
        let outer_x = (screen.x - outer_w) * 0.5;
        let outer_y = screen.y * self.y_factor;
        let rect = Rect::from_xywh(outer_x, outer_y, outer_w, outer_h);

        match self.style {
            BannerStyle::Panel => {
                let mut frame = Frame::panel(&theme).with_padding(pad);
                if let Some(c) = self.fill {
                    frame = frame.with_fill(c);
                }
                frame.show(ui, rect, |ui, body| {
                    let tw = ui.measure_text(self.text, text_size);
                    ui.draw_text(
                        Pos2::new(body.x() + (inner_w - tw) * 0.5, body.y()),
                        self.text,
                        text_size,
                        text_color,
                    );
                });
            }
            BannerStyle::Pill => {
                let fill = self
                    .fill
                    .unwrap_or(Color::rgba(0.08, 0.10, 0.16, 0.80));
                ui.draw_rounded_rect(rect, theme.spacing.corner_radius, fill);
                let tw = ui.measure_text(self.text, text_size);
                ui.draw_text(
                    Pos2::new(
                        rect.x() + (rect.width() - tw) * 0.5,
                        rect.y() + (rect.height() - text_size) * 0.5,
                    ),
                    self.text,
                    text_size,
                    text_color,
                );
            }
            BannerStyle::None => {
                let tw = ui.measure_text(self.text, text_size);
                ui.draw_text(
                    Pos2::new((screen.x - tw) * 0.5, outer_y),
                    self.text,
                    text_size,
                    text_color,
                );
            }
        }

        rect
    }
}

/// Convenience helper for the very common case: a small panel-framed
/// toast at a `y_factor` with theme defaults. Returns the rect.
pub fn toast(ui: &mut Ui<'_>, text: &str, y_factor: f32) -> Rect {
    Banner::new(text).y_factor(y_factor).show(ui)
}
