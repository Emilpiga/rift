//! Pixel-emitting trait used by every widget.
//!
//! `rift-engine::renderer::OverlayBatch` is the real implementation;
//! widgets see only this trait so the UI crate compiles without
//! depending on Vulkan/ash/winit. See the crate-level docs for
//! why that matters (dylib export-table size limit).
//!
//! Coordinate system: pixels with top-left origin, matching what
//! the overlay renderer expects. `screen_w` / `screen_h` are
//! passed through to the implementation so it can convert to NDC
//! without a stateful "current viewport" field.

/// Sink for immediate-mode draw commands.
///
/// All methods take dimensions in pixels (top-left origin) and a
/// `screen_w` / `screen_h` pair so the implementation can convert
/// to whatever clip-space it needs. Colours are RGBA in `[0.0,
/// 1.0]`.
pub trait DrawList {
    /// Filled rectangle.
    fn rect_px(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    );

    /// Filled rectangle with rounded corners. `radius <= 0.0`
    /// degrades to a sharp rectangle so callers can pass the
    /// theme's `corner_radius` unconditionally.
    fn rounded_rect_px(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        color: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    );

    /// Vertical-gradient rectangle: `top` colour at the upper
    /// edge, `bot` at the lower. Same primitive cost as `rect_px`
    /// — the engine's overlay shader already multiplies a
    /// per-vertex colour through the white atlas pixel, so
    /// just sets distinct colours on the top vs bottom verts.
    fn rect_px_grad_v(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        top: [f32; 4],
        bot: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    );

    /// Vertical-gradient rounded rectangle. Continuous gradient
    /// across the rounded shape (corners and edge bands all
    /// interpolate from `top` to `bot` based on their y).
    fn rounded_rect_px_grad_v(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        top: [f32; 4],
        bot: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    );

    /// Filled rect with one colour per corner: `tl`, `tr`,
    /// `bl`, `br`. Bilinear interpolation across the quad,
    /// same primitive cost as `rect_px`. Lets widgets fade
    /// a bevel band along the horizontal axis (e.g. left and
    /// right corners transparent, centre opaque) without
    /// stacking translucent rects.
    #[allow(clippy::too_many_arguments)]
    fn rect_px_grad4(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        tl: [f32; 4],
        tr: [f32; 4],
        bl: [f32; 4],
        br: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    );

    /// Filled rounded rect with an elliptical radial gradient:
    /// `centre` colour at the geometric centre, lerping out
    /// to `edge` along the bounding ellipse with a smooth
    /// (smoothstep) falloff. Used for the soft "polished
    /// blood-red" hotspot on the primary action button —
    /// gives a real oval highlight instead of a stack of
    /// horizontal bands.
    #[allow(clippy::too_many_arguments)]
    fn rounded_rect_px_radial(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        edge: [f32; 4],
        centre: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    );

    /// Same as [`Self::rounded_rect_px_radial`] but the
    /// fragment shader applies a procedural cloud-noise
    /// modulation per pixel, so the surface reads as
    /// textured stone / hammered metal rather than a flat
    /// gradient. Implementations that don't support the
    /// noise sentinel may fall back to the smooth version.
    #[allow(clippy::too_many_arguments)]
    fn rounded_rect_px_radial_noisy(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        edge: [f32; 4],
        centre: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    );

    /// True rounded outline of constant pixel thickness —
    /// the corner runs are real arcs, unlike the four-edge-
    /// rect approximation in the Ui helper. Use for inset
    /// hairlines (no fill behind to mask corner gaps).
    #[allow(clippy::too_many_arguments)]
    fn rounded_outline_px(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        thickness: f32,
        color: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    );

    /// Rasterise `text` at pixel position `(x, y)` (top-left
    /// anchor of the first glyph's bbox), in `size`-pixel cap
    /// height. Returns the advance width consumed.
    fn text(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        size: f32,
        color: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    ) -> f32;

    /// Predict the rendered width of `text` at `size`-pixel cap
    /// height. Pure measurement — no draw side-effects. Used by
    /// widgets that need to right-align or centre text without
    /// actually emitting it first.
    fn measure_text(&self, text: &str, size: f32) -> f32;

    /// Draw a previously-registered icon (e.g. an item / class
    /// glyph). `name` matches the key the engine registered the
    /// icon under (typically the PNG filename sans extension).
    /// `tint` is multiplied with the icon RGBA — pass white to
    /// keep the source colours. Returns `false` on an unknown
    /// name so the caller can fall back to a placeholder.
    fn icon(
        &mut self,
        name: &str,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        tint: [f32; 4],
        screen_w: f32,
        screen_h: f32,
    ) -> bool;
}
