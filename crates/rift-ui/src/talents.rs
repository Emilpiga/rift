//! Talent-tree panel widget.
//!
//! Renders a graph view of every `TalentNodeView` the host built
//! this frame, with auto-laid-out positions, prereq edges, route
//! tints, hover tooltip, click-to-invest, fuzzy search filter,
//! and live gating visualisation. The widget owns no persistent
//! state — `TalentPanelState` lives on the host and is threaded
//! through every frame, matching the spellbook / text-field
//! pattern.
//!
//! Pan + zoom are provided by [`rift_ui_im::PanZoom`]; the
//! widget only contributes talent-specific concerns (auto-
//! layout math + visuals).

use rift_ui_im::{
    widgets::title, Color, Frame, Id, Layer, PanZoom, PanZoomState, PanZoomTransform, Pos2, Rect,
    TextField, Tooltip, TooltipLine, TooltipLineDecor, Ui, Vec2,
};
use rift_ui_types::talents::{
    TalentNodeKind, TalentNodeView, TalentPanelState, TalentRouteView, TalentTreeAction,
    TalentTreeView,
};

// ─── Layout constants (world space, pre-zoom) ────────────────

/// Radius of an ordinary stat / proc / modifier node, in world
/// pixels at zoom 1.0.
const NODE_RADIUS: f32 = 22.0;
/// Radius bump for `UnlockAbility` nodes — they're the visual
/// anchors of each route so they read as bigger / more
/// important.
const UNLOCK_NODE_RADIUS: f32 = 28.0;
/// Radius bump for `Keystone` nodes (route capstones).
const KEYSTONE_NODE_RADIUS: f32 = 32.0;
/// Distance from the hub origin to the first route ring.
/// Sits well outside the hub passive outer ring (≈ 1.55 ×
/// `HUB_RING_RADIUS`) so route entry nodes never collide with
/// the hub graph.
const ROUTE_BASE_RADIUS: f32 = 300.0;
/// Distance between consecutive route rings.
const ROUTE_RING_STEP: f32 = 150.0;
/// Width of the angular wedge each route gets (radians).
/// Slightly less than π/2 so the four routes have visible
/// gaps between them.
const ROUTE_WEDGE: f32 = std::f32::consts::FRAC_PI_2 * 0.85;
/// Radius the hub passives sit on.
const HUB_RING_RADIUS: f32 = 130.0;
/// Edge thickness in world pixels at zoom 1.0.
const EDGE_THICKNESS: f32 = 3.0;

// ─── Route palette ───────────────────────────────────────────

fn route_tint(route: TalentRouteView) -> Color {
    match route {
        // Hub passives wear a warm neutral so they read as
        // "everyone's nodes" instead of belonging to any class.
        TalentRouteView::Hub => Color::rgba(0.78, 0.72, 0.55, 1.0),
        TalentRouteView::Warrior => Color::rgba(0.85, 0.32, 0.28, 1.0),
        TalentRouteView::Mage => Color::rgba(0.32, 0.55, 0.90, 1.0),
        TalentRouteView::Healer => Color::rgba(0.42, 0.82, 0.50, 1.0),
        TalentRouteView::Summoner => Color::rgba(0.70, 0.45, 0.90, 1.0),
    }
}

/// Centre angle (radians, screen convention: 0 = right, π/2 =
/// down) of the wedge each route occupies. Auto-layout uses
/// this as the spine direction; per-node angle is offset
/// inside `± ROUTE_WEDGE / 2`.
fn route_centre_angle(route: TalentRouteView) -> f32 {
    use std::f32::consts::{FRAC_PI_2, PI};
    match route {
        // Hub passives are arranged on a small ring around
        // the origin, not on a spine; the angle is computed
        // separately in `layout_hub`.
        TalentRouteView::Hub => 0.0,
        // Warrior = right, Mage = up, Healer = down,
        // Summoner = left. Screen y grows down, so "up" is
        // `-π/2`.
        TalentRouteView::Warrior => 0.0,
        TalentRouteView::Mage => -FRAC_PI_2,
        TalentRouteView::Healer => FRAC_PI_2,
        TalentRouteView::Summoner => PI,
    }
}

// ─── Public entry point ─────────────────────────────────────

/// Render the talent panel and return any action triggered this
/// frame.
///
/// `panel_state` owns persistent UI memory (open flag, pan/zoom,
/// search text). The widget mutates it freely.
pub fn frame_talent_panel(
    ui: &mut Ui<'_>,
    state: &mut TalentPanelState,
    view: &TalentTreeView<'_>,
) -> Option<TalentTreeAction> {
    if !state.open {
        return None;
    }

    let theme = *ui.theme();
    let screen = ui.screen_size();
    // Modal dim — same treatment as the spellbook so the two
    // panels read as a matched pair.
    ui.with_layer(Layer::Modal, |ui| {
        ui.draw_rect(
            Rect::from_xywh(0.0, 0.0, screen.x, screen.y),
            Color::rgba(0.0, 0.0, 0.0, 0.55),
        );
    });

    // Fullscreen panel — the talent tree wants the whole
    // viewport so the four routes have room to spread without
    // overlapping. We still draw the standard panel chrome on
    // top of the modal dim so the surface reads as a proper
    // window rather than HUD overlay paint.
    let panel = Rect::from_xywh(0.0, 0.0, screen.x, screen.y);
    Frame::panel(&theme).show_only(ui, panel);

    let inner_pad = theme.spacing.inner_pad();
    let section_gap = theme.spacing.section_gap();
    let row_gap = theme.spacing.row_gap();

    // ── Header ───────────────────────────────────────────
    title(ui, panel.min + Vec2::new(inner_pad, inner_pad), "Talents");

    // Unspent-points pill (top-right). Tinted gold when the
    // player has unspent points; dim grey otherwise so the
    // player notices when they have a point banked.
    let pill_text = format!("{} unspent", view.unspent_points);
    let pill_color = if view.unspent_points > 0 {
        theme.colors.accent
    } else {
        theme.colors.text_dim
    };
    let pill_w = ui.measure_text(&pill_text, theme.fonts.size_md);
    ui.draw_text(
        Pos2::new(
            panel.max.x - pill_w - inner_pad,
            panel.min.y + inner_pad + 4.0,
        ),
        &pill_text,
        theme.fonts.size_md,
        pill_color,
    );
    ui.draw_text(
        panel.min + Vec2::new(inner_pad, inner_pad + theme.fonts.size_lg + row_gap * 0.5),
        "Click an investable node to spend a point. Drag to pan, scroll to zoom.",
        theme.fonts.size_sm,
        theme.colors.text_dim,
    );

    // ── Search field (top, below header text) ───────────
    let header_h = inner_pad + theme.fonts.size_lg + row_gap + theme.fonts.size_sm + section_gap;
    let search_w = 240.0 * theme.scale;
    let search_h = 28.0 * theme.scale;
    let search_rect = Rect::from_xywh(
        panel.min.x + inner_pad,
        panel.min.y + header_h,
        search_w,
        search_h,
    );
    let search_id = Id::root("rift::talents::search");
    let search_resp = TextField::new(search_id)
        .placeholder("Filter…")
        .max_chars(24)
        .show(ui, search_rect, &mut state.search, 0.0);
    let search_norm = state.search.to_ascii_lowercase();
    // ESC behaviour is owned by the host (see `ui_phase.rs`):
    // when the panel is open it routes Esc / N to
    // `talents_panel.close()` via raw key polling so the host-
    // wide text-capture flag (which we set to silence WASD /
    // hotbar) doesn't also swallow the close hotkey. The
    // widget itself stays out of keyboard close-routing to
    // avoid open-frame double-toggling.
    let _ = search_resp;

    // ── Canvas viewport ─────────────────────────────────
    let viewport = Rect::from_xywh(
        panel.min.x + inner_pad,
        search_rect.max.y + section_gap,
        panel.width() - inner_pad * 2.0,
        panel.max.y - search_rect.max.y - section_gap - inner_pad,
    );
    // Clip every draw inside `viewport` so dragged-out nodes
    // can't bleed onto the panel chrome / search field.
    let mut pan_state = pan_state_proxy(state);
    let transform = PanZoom::new(viewport)
        .zoom_range(TalentPanelState::ZOOM_MIN, TalentPanelState::ZOOM_MAX)
        .show(ui, Id::root("rift::talents::canvas"), &mut pan_state);
    apply_pan_state(state, &pan_state);

    let mut action: Option<TalentTreeAction> = None;
    let mut hover_id: Option<u16> = None;
    let mut hover_screen: Option<Pos2> = None;

    ui.with_clip(viewport, |ui| {
        // Layout pass — compute world position for every node.
        let layout = layout_nodes(view);

        // ── Aztec labyrinth backdrop ────────────────────
        //
        // A multi-band engraved-stone pattern that:
        //   * Centres the eye on the hub (stepped sun-stone
        //     centrepiece at world origin).
        //   * Wraps every meaningful tree radius (hub ring,
        //     route entry ring, route tier-2 ring) in a
        //     paired-circle band so the chips read as
        //     "stations along this band" rather than floating
        //     points.
        //   * Connects the bands with stepped rungs at the
        //     exact angles where nodes live (4 cardinals for
        //     route spines, 8 π/8-offset slots for hub
        //     passives, the diagonal for the dodge cluster).
        //
        // Everything draws at low alpha in a warm-stone tint
        // so the engravings recede behind the chips.
        {
            use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, FRAC_PI_8, TAU};
            let hub_centre = transform.world_to_screen(Pos2::new(0.0, 0.0));
            let s = |v: f32| transform.scale(v);

            // ── Palette ──
            let stone = Color::rgba(0.46, 0.40, 0.30, 0.34);
            let stone_dim = Color::rgba(0.38, 0.34, 0.26, 0.22);
            let stone_bright = Color::rgba(0.66, 0.56, 0.38, 0.50);
            let stone_glow = Color::rgba(0.78, 0.66, 0.42, 0.65);
            let stone_dark = Color::rgba(0.06, 0.06, 0.08, 0.55);

            // ── Polyline primitives ──
            let polar = |a: f32, r: f32| -> Pos2 {
                Pos2::new(
                    hub_centre.x + a.cos() * r,
                    hub_centre.y + a.sin() * r,
                )
            };
            let arc = |ui: &mut Ui<'_>, r: f32, a0: f32, a1: f32, segs: usize, w: f32, c: Color| {
                if r <= 1.0 {
                    return;
                }
                let mut prev = polar(a0, r);
                for k in 1..=segs {
                    let t = k as f32 / segs as f32;
                    let a = a0 + (a1 - a0) * t;
                    let next = polar(a, r);
                    ui.draw_line(prev, next, w, c);
                    prev = next;
                }
            };
            let ring = |ui: &mut Ui<'_>, r: f32, w: f32, c: Color| {
                arc(ui, r, 0.0, TAU, 96, w, c);
            };
            let radial = |ui: &mut Ui<'_>, r0: f32, r1: f32, a: f32, w: f32, c: Color| {
                ui.draw_line(polar(a, r0), polar(a, r1), w, c);
            };
            // I-bar / step rung: a radial segment with short
            // perpendicular caps at both ends — the building
            // block of the Aztec ladder pattern between bands.
            let i_bar = |ui: &mut Ui<'_>, r0: f32, r1: f32, a: f32, cap: f32, w: f32, c: Color| {
                radial(ui, r0, r1, a, w, c);
                let cap0 = cap / r0.max(1.0);
                let cap1 = cap / r1.max(1.0);
                arc(ui, r0, a - cap0, a + cap0, 4, w, c);
                arc(ui, r1, a - cap1, a + cap1, 4, w, c);
            };
            // Crenellated band: two concentric rings with
            // stepped notches alternating inside/outside,
            // reads as a maze wall.
            let crenel_band =
                |ui: &mut Ui<'_>, r_in: f32, r_out: f32, count: usize, w: f32, c: Color| {
                    ring(ui, r_in, w, c);
                    ring(ui, r_out, w, c);
                    let bw = r_out - r_in;
                    for k in 0..count {
                        let a = (k as f32 / count as f32) * TAU;
                        let inward = k % 2 == 0;
                        let tick_r0 = if inward { r_in } else { r_out };
                        let tick_r1 = if inward { r_in + bw * 0.55 } else { r_out - bw * 0.55 };
                        radial(ui, tick_r0, tick_r1, a, w, c);
                        // Tiny perpendicular cap at the
                        // free end gives the "T" silhouette.
                        let cap_a = (bw * 0.25) / tick_r1.max(1.0);
                        arc(ui, tick_r1, a - cap_a, a + cap_a, 4, w, c);
                    }
                };
            // Stepped (square-wave) meander running along an
            // arc — Aztec/Greek-key style. `count` is the
            // number of step periods around the full ring.
            let meander = |ui: &mut Ui<'_>,
                           r_in: f32,
                           r_out: f32,
                           count: usize,
                           w: f32,
                           c: Color| {
                let bw = r_out - r_in;
                let r_mid_lo = r_in + bw * 0.30;
                let r_mid_hi = r_in + bw * 0.70;
                for k in 0..count {
                    let a0 = (k as f32 / count as f32) * TAU;
                    let a1 = ((k as f32 + 0.5) / count as f32) * TAU;
                    let a2 = ((k as f32 + 1.0) / count as f32) * TAU;
                    // Square wave: low arc → riser → high arc
                    // → faller → repeat.
                    arc(ui, r_mid_lo, a0, a1, 4, w, c);
                    radial(ui, r_mid_lo, r_mid_hi, a1, w, c);
                    arc(ui, r_mid_hi, a1, a2, 4, w, c);
                    radial(ui, r_mid_lo, r_mid_hi, a2, w, c);
                }
            };

            // ── Outer concentric labyrinth bands ──
            //
            // Each band is a pair of thin rings with stepped
            // crenellations between them. The pair widths
            // and tick counts are tuned so:
            //   * Band 1 sits inside the hub passive ring.
            //   * Band 2 wraps the hub passive ring itself.
            //   * Band 3 wraps the route entry chips.
            //   * Band 4 wraps the route tier-2 chips.

            // Band 1 — inner hub backbone (between rose and
            // connector R2 stations).
            let b1_in = s(HUB_RING_RADIUS * 0.78);
            let b1_out = s(HUB_RING_RADIUS * 0.92);
            crenel_band(ui, b1_in, b1_out, 16, 1.0, stone_dim);

            // Band 2 — hub passive ring (8 slots × 2 ticks =
            // 16 crenellations align with the slots).
            let b2_in = s(HUB_RING_RADIUS * 1.42);
            let b2_out = s(HUB_RING_RADIUS * 1.68);
            crenel_band(ui, b2_in, b2_out, 16, 1.1, stone);

            // Band 3 — route entry ring.
            let b3_in = s(ROUTE_BASE_RADIUS - 30.0);
            let b3_out = s(ROUTE_BASE_RADIUS + 30.0);
            crenel_band(ui, b3_in, b3_out, 24, 1.0, stone);

            // Band 4 — route tier-2 (faintest, furthest out).
            let b4_in = s(ROUTE_BASE_RADIUS + ROUTE_RING_STEP - 30.0);
            let b4_out = s(ROUTE_BASE_RADIUS + ROUTE_RING_STEP + 30.0);
            crenel_band(ui, b4_in, b4_out, 32, 1.0, stone_dim);

            // ── Stepped-meander filler bands ──
            //
            // Two thin meander rings sit between the major
            // bands, filling the negative space with a
            // continuous Aztec key. Period counts (16 / 24)
            // align with the band crenellations so step
            // peaks line up across bands.
            meander(
                ui,
                s(HUB_RING_RADIUS * 1.05),
                s(HUB_RING_RADIUS * 1.32),
                16,
                1.0,
                stone_dim,
            );
            meander(
                ui,
                s(HUB_RING_RADIUS * 1.85),
                s(ROUTE_BASE_RADIUS - 40.0),
                24,
                1.0,
                stone_dim,
            );

            // ── Rungs: route spines ──
            //
            // Bright I-bar ladders along each route's cardinal
            // axis, connecting the hub passive band to the
            // route entry band. Reads as "this is the channel
            // your chips travel along".
            for route in [
                TalentRouteView::Warrior,
                TalentRouteView::Mage,
                TalentRouteView::Healer,
                TalentRouteView::Summoner,
            ] {
                let a = route_centre_angle(route);
                let tint = route_tint(route);
                let rung = Color::rgba(
                    tint.0[0] * 0.55 + 0.25,
                    tint.0[1] * 0.55 + 0.22,
                    tint.0[2] * 0.55 + 0.18,
                    0.40,
                );
                // Spine from the hub passive band's outer
                // edge to the route entry band's inner edge.
                i_bar(ui, b2_out, b3_in, a, s(7.0), 1.4, rung);
                // And one further out into the route tier-2
                // band, lighter to fade outward.
                i_bar(
                    ui,
                    b3_out,
                    b4_in,
                    a,
                    s(6.0),
                    1.2,
                    Color::rgba(rung.0[0], rung.0[1], rung.0[2], 0.28),
                );
            }

            // ── Rungs: hub passive slots ──
            //
            // Short stone rungs at the 8 π/8-offset slot
            // angles connect the inner backbone (band 1) to
            // the passive ring (band 2), so every hub passive
            // visually sits on a spoke of its own.
            for k in 0..8 {
                let a = FRAC_PI_8 + (k as f32) * (TAU / 8.0);
                i_bar(ui, b1_out, b2_in, a, s(5.0), 1.0, stone);
            }

            // ── Dodge cluster rung ──
            //
            // Diagonal stone rung from the centrepiece out
            // to the dodge chain at -π/4, anchoring it the
            // same way the cardinals are anchored.
            {
                let a = -FRAC_PI_4;
                i_bar(ui, s(HUB_RING_RADIUS * 0.60), b2_in, a, s(5.0), 1.0, stone);
            }

            // ── Sun-stone centrepiece ──
            //
            // Nested rings + stepped rays in the Aztec sun-
            // stone vocabulary. Three concentric rings, with
            // wedge-shaped rays poking outward at the 8 slot
            // angles and longer tapered arrows at the 4
            // cardinals.

            // Outer wreath ring.
            let r_wreath = s(HUB_RING_RADIUS * 0.62);
            ring(ui, r_wreath, 1.2, stone_bright);
            // Mid ring.
            let r_mid = s(HUB_RING_RADIUS * 0.42);
            ring(ui, r_mid, 1.2, stone_bright);
            // Inner ring (sigil floor).
            let r_inner = s(HUB_RING_RADIUS * 0.22);
            ring(ui, r_inner, 1.4, stone_glow);

            // 8 stepped rays between the wreath and inner
            // backbone band. Each ray is a slim trapezoid
            // outlined by two converging radials + a chord
            // cap — the standard Aztec sun-ray silhouette.
            for k in 0..8 {
                let a = FRAC_PI_8 + (k as f32) * (TAU / 8.0);
                let half = TAU / 64.0; // narrow wedge
                let r0 = r_wreath;
                let r1 = s(HUB_RING_RADIUS * 0.74);
                // Two side rails of the trapezoid.
                ui.draw_line(polar(a - half, r0), polar(a - half * 0.4, r1), 1.0, stone);
                ui.draw_line(polar(a + half, r0), polar(a + half * 0.4, r1), 1.0, stone);
                // Cap.
                arc(ui, r1, a - half * 0.4, a + half * 0.4, 4, 1.0, stone);
            }

            // 4 cardinal arrowheads — longer, brighter, with
            // a step shoulder midway to give them the carved
            // sun-stone weight.
            for k in 0..4 {
                let a = (k as f32) * FRAC_PI_2;
                let half_lo = TAU / 32.0;
                let half_mid = TAU / 56.0;
                let half_tip = TAU / 96.0;
                let r0 = r_wreath;
                let r_shoulder = s(HUB_RING_RADIUS * 0.80);
                let r_tip = s(HUB_RING_RADIUS * 0.92);
                // Left rail
                ui.draw_line(
                    polar(a - half_lo, r0),
                    polar(a - half_mid, r_shoulder),
                    1.4,
                    stone_bright,
                );
                ui.draw_line(
                    polar(a - half_mid, r_shoulder),
                    polar(a - half_tip, r_tip),
                    1.4,
                    stone_bright,
                );
                // Right rail
                ui.draw_line(
                    polar(a + half_lo, r0),
                    polar(a + half_mid, r_shoulder),
                    1.4,
                    stone_bright,
                );
                ui.draw_line(
                    polar(a + half_mid, r_shoulder),
                    polar(a + half_tip, r_tip),
                    1.4,
                    stone_bright,
                );
                // Step shoulder cap (horizontal lintel).
                arc(
                    ui,
                    r_shoulder,
                    a - half_mid,
                    a + half_mid,
                    4,
                    1.4,
                    stone_bright,
                );
                // Tip cap.
                arc(ui, r_tip, a - half_tip, a + half_tip, 3, 1.4, stone_bright);
            }

            // 8 short tick marks on the inner ring — small
            // glyph studs at the same slot angles as the
            // outer-ring passives, completing the visual
            // call-and-response between centre and rim.
            for k in 0..8 {
                let a = FRAC_PI_8 + (k as f32) * (TAU / 8.0);
                let p0 = polar(a, r_inner * 0.78);
                let p1 = polar(a, r_inner * 1.05);
                ui.draw_line(p0, p1, 1.2, stone_glow);
            }
            // 4 longer arms on the inner ring — same idea
            // for the cardinal route spines.
            for k in 0..4 {
                let a = (k as f32) * FRAC_PI_2;
                let p0 = polar(a, r_inner * 0.55);
                let p1 = polar(a, r_inner * 1.15);
                ui.draw_line(p0, p1, 1.6, stone_glow);
            }

            // Tiny dark pip at the very centre — the still
            // point everything radiates from. Drawn last so
            // the gold core lands on top of every ring.
            let pip_r = transform.scale(7.0);
            if pip_r >= 1.0 {
                ui.draw_circle(hub_centre, pip_r, stone_dark);
                ui.draw_circle(hub_centre, pip_r * 0.55, stone_glow);
            }
        }

        // ── Edges first (so nodes draw on top) ──────────
        // Active edges (prereq invested) glow in the route
        // tint via a single shader-rasterised quad —
        // `Ui::draw_glow_line` produces the bloom falloff
        // perpendicular to the segment. Locked edges fall
        // back to a thin dim plain line so they read as
        // topology without competing for attention.
        for (idx, node) in view.nodes.iter().enumerate() {
            let to = transform.world_to_screen(layout[idx]);
            for prereq_idx in &node.prereq_indices {
                let i = *prereq_idx as usize;
                if i >= view.nodes.len() {
                    continue;
                }
                let from = transform.world_to_screen(layout[i]);
                let edge_active = view.nodes[i].current_rank >= 1;
                let core = transform.scale(EDGE_THICKNESS);
                if edge_active {
                    let tint = route_tint(node.route);
                    let halo = transform.scale(EDGE_THICKNESS * 4.0);
                    ui.draw_glow_line(from, to, core, halo, tint.with_alpha(0.95));
                } else {
                    ui.draw_line(from, to, core, Color::rgba(0.30, 0.30, 0.36, 0.50));
                }
            }
        }

        // ── Nodes ───────────────────────────────────────
        // Two-pass: hit-test in screen space first (so the
        // tooltip / click only fires for the topmost node
        // under the cursor), then draw.
        let (mx, my) = ui.input().mouse_pos();
        let cursor = Pos2::new(mx, my);
        let left_click = ui.input().left_clicked();

        // Find topmost node under cursor (iterate in reverse so
        // later-drawn nodes shadow earlier ones on overlap).
        for (idx, node) in view.nodes.iter().enumerate().rev() {
            let screen_pos = transform.world_to_screen(layout[idx]);
            let r = transform.scale(node_radius(node.kind));
            if dist2(cursor, screen_pos) <= r * r && viewport.contains(cursor) {
                hover_id = Some(node.id);
                hover_screen = Some(screen_pos);
                if left_click && node.investable {
                    action = Some(TalentTreeAction::Invest { talent_id: node.id });
                }
                break;
            }
        }

        // Draw every node.
        for (idx, node) in view.nodes.iter().enumerate() {
            let screen_pos = transform.world_to_screen(layout[idx]);
            let r = transform.scale(node_radius(node.kind));

            // Search filter: nodes whose name doesn't match
            // the typed prefix fade to a very dim ghost so the
            // player can still see the graph shape but the
            // matches pop.
            let matches_search = if search_norm.is_empty() {
                true
            } else {
                node.name.to_ascii_lowercase().contains(&search_norm)
            };

            draw_node(
                ui,
                node,
                screen_pos,
                r,
                hover_id == Some(node.id),
                matches_search,
            );
        }
    });

    // ── Hover tooltip ────────────────────────────────────
    state.last_hover_id = hover_id;
    if let (Some(id), Some(pos)) = (hover_id, hover_screen) {
        if let Some(node) = view.nodes.iter().find(|n| n.id == id) {
            draw_tooltip(ui, node, pos, transform.scale(node_radius(node.kind)));
        }
    }

    action
}

// ─── Layout ──────────────────────────────────────────────────

/// Auto-layout: hub nodes ring the centre, route nodes radiate
/// out along their spine direction. Ring index is the BFS
/// depth from any hub node (or the hub centre for connectors).
fn layout_nodes(view: &TalentTreeView<'_>) -> Vec<Pos2> {
    let n = view.nodes.len();
    let mut positions = vec![Pos2::new(0.0, 0.0); n];

    // BFS depth per node — used as the ring index for route
    // nodes. Hub nodes are treated as depth 0 (the inner ring).
    let depths = compute_depths(view);

    // ── Hub layout ──────────────────────────────────────
    //
    // The hub has three structurally distinct groups and they
    // must NOT share angular real estate, otherwise the BFS
    // sort that worked for routes piles every passive onto a
    // single ring and the edges go everywhere. We classify
    // by `TalentId` range (see `rift-game/src/talents/hub.rs`
    // for the canonical layout) and place each group on its
    // own ring:
    //
    //   * Dodge cluster (ids 100, 101) — inner ring, top.
    //   * Connector chains (ids 110+, 210+, 310+, 410+) —
    //     radial spokes per target route, two nodes deep.
    //   * Generic passives (ids 1..=8) — outer ring, slotted
    //     into 8 fixed angular positions so connector edges
    //     point at the adjacent slot rather than crossing
    //     the hub diagonally.
    //
    // The id-range coupling is intentional: the hub topology
    // is hand-authored, so a hand-tuned visual layout is the
    // right level of detail (versus a generic force-directed
    // graph layout that would still need anchor hints to read
    // cleanly).
    // Dodge cluster radii are inlined below; see that block
    // for the reasoning behind the choice.
    const HUB_CONNECTOR_R1: f32 = HUB_RING_RADIUS * 0.62;
    const HUB_CONNECTOR_R2: f32 = HUB_RING_RADIUS * 1.05;
    const HUB_PASSIVE_R: f32 = HUB_RING_RADIUS * 1.55;
    // 8 angular slots offset by π/8 from the cardinal spokes
    // (so passives sit in the inter-spoke arcs, never on top
    // of a connector). Indexed CW starting just CW of the
    // right-pointing Warrior spoke.
    let passive_slots: [f32; 8] = {
        use std::f32::consts::PI;
        [
            PI / 8.0,        // 0: CW of Warrior
            3.0 * PI / 8.0,  // 1: CW of Healer
            5.0 * PI / 8.0,  // 2: CCW of Healer
            7.0 * PI / 8.0,  // 3: CW of Summoner
            -7.0 * PI / 8.0, // 4: CCW of Summoner
            -5.0 * PI / 8.0, // 5: CW of Mage
            -3.0 * PI / 8.0, // 6: CCW of Mage
            -PI / 8.0,       // 7: CCW of Warrior
        ]
    };
    // Map hub passive id → outer-ring slot index. Chosen so
    // each connector chain's *first* prereq passive sits in
    // the slot adjacent to that connector's spoke, keeping
    // the prereq edge short and tangential.
    //
    //   id 1 Vigor      → slot 1 (CW of Healer)    — prereqs Compassion
    //   id 2 Might      → slot 0 (CW of Warrior)   — prereqs Strength
    //   id 3 Keen Edge  → slot 6 (CCW of Mage)     — prereqs Insight
    //   id 8 Precision  → slot 3 (CW of Summoner)  — prereqs Command
    //
    // The other four passives fill the remaining slots; the
    // exact slot doesn't matter for them since no connector
    // edge points at them.
    fn passive_slot_for_id(id: u16) -> usize {
        match id {
            1 => 1,
            2 => 0,
            3 => 6,
            4 => 2, // Focus
            5 => 4, // Toughness
            6 => 5, // Swift Step
            7 => 7, // Reflexes
            8 => 3,
            _ => 0,
        }
    }
    // Which route does this connector chain feed? Detected
    // by the canonical id banding (see `rift-game/.../hub.rs`).
    fn connector_target(id: u16) -> Option<TalentRouteView> {
        match id {
            110..=119 => Some(TalentRouteView::Warrior),
            210..=219 => Some(TalentRouteView::Mage),
            310..=319 => Some(TalentRouteView::Healer),
            410..=419 => Some(TalentRouteView::Summoner),
            _ => None,
        }
    }

    // First pass: bucket every hub node.
    let mut dodge: Vec<usize> = Vec::new();
    let mut connectors_by_route: std::collections::BTreeMap<u32, Vec<usize>> = Default::default();
    let mut generic_passives: Vec<usize> = Vec::new();
    for (i, node) in view.nodes.iter().enumerate() {
        if node.route != TalentRouteView::Hub {
            continue;
        }
        let id = node.id;
        if matches!(id, 100..=109) {
            dodge.push(i);
        } else if let Some(route) = connector_target(id) {
            // Use the route discriminant as the BTreeMap key so
            // chain ordering is deterministic across runs.
            let key = match route {
                TalentRouteView::Warrior => 0u32,
                TalentRouteView::Mage => 1,
                TalentRouteView::Healer => 2,
                TalentRouteView::Summoner => 3,
                TalentRouteView::Hub => 4,
            };
            connectors_by_route.entry(key).or_default().push(i);
        } else {
            generic_passives.push(i);
        }
    }

    // Generic passives: outer ring, slotted by id.
    for &i in &generic_passives {
        let slot = passive_slot_for_id(view.nodes[i].id);
        let angle = passive_slots[slot];
        positions[i] = Pos2::new(angle.cos() * HUB_PASSIVE_R, angle.sin() * HUB_PASSIVE_R);
    }

    // Connectors: two radii per spoke, ordered by id (which
    // matches the §8 chain order — 110 → 111, 210 → 211, etc).
    let connector_radii = [HUB_CONNECTOR_R1, HUB_CONNECTOR_R2];
    for (_route_key, mut chain) in connectors_by_route {
        // Sort by id so the chain root sits closest to centre.
        chain.sort_by_key(|&i| view.nodes[i].id);
        let target = connector_target(view.nodes[chain[0]].id).unwrap_or(TalentRouteView::Hub);
        let angle = route_centre_angle(target);
        for (slot, &i) in chain.iter().enumerate() {
            let r = connector_radii
                .get(slot)
                .copied()
                .unwrap_or(HUB_CONNECTOR_R2);
            positions[i] = Pos2::new(angle.cos() * r, angle.sin() * r);
        }
    }

    // Dodge cluster: a short radial chain along the top-right
    // diagonal (-π/4). All four route spokes run on cardinal
    // axes, so the diagonal is the maximum-distance direction
    // from every connector chain — the chips can't touch the
    // Mage or Warrior R1 nodes regardless of inner-ring size.
    //
    // Laid out as a chain along the diagonal rather than as
    // an arc, so Tumbler → Evasive Roll reads as a short
    // spoke of its own and the chips never bunch up against
    // each other (Evasive Roll is an Unlock chip with a
    // larger radius — fanning along an arc had them tangent
    // even at narrow arc width).
    if !dodge.is_empty() {
        let mut sorted = dodge.clone();
        sorted.sort_by_key(|&i| view.nodes[i].id);
        let dodge_angle = -std::f32::consts::FRAC_PI_4;
        // Two radii: first at the inner hub band, second
        // out past the connector R1 ring so the Unlock chip
        // has room without colliding with anything on the
        // adjacent cardinal spokes.
        let dodge_radii = [HUB_RING_RADIUS * 0.55, HUB_RING_RADIUS * 0.95];
        for (k, &i) in sorted.iter().enumerate() {
            let r = dodge_radii.get(k).copied().unwrap_or(HUB_RING_RADIUS * 0.95);
            positions[i] = Pos2::new(dodge_angle.cos() * r, dodge_angle.sin() * r);
        }
    }

    // ── Route layout ────────────────────────────────────
    // Modifier nodes whose only prereq is an `Unlock` ability
    // node get pulled off the BFS ring and clustered around
    // that ability as satellites — they read as "support
    // talents for this ability", not as "the next thing on
    // the spine". The general progression continues outward
    // along the spine through stat passives instead.
    let mut satellite_of: std::collections::HashMap<usize, Vec<usize>> = Default::default();
    let mut is_satellite: std::collections::HashSet<usize> = Default::default();
    for (i, node) in view.nodes.iter().enumerate() {
        if !matches!(node.kind, TalentNodeKind::Modifier) {
            continue;
        }
        if node.prereq_indices.len() != 1 {
            continue;
        }
        let parent = node.prereq_indices[0] as usize;
        if parent >= view.nodes.len() {
            continue;
        }
        if matches!(view.nodes[parent].kind, TalentNodeKind::Unlock) {
            satellite_of.entry(parent).or_default().push(i);
            is_satellite.insert(i);
        }
    }

    for route in [
        TalentRouteView::Warrior,
        TalentRouteView::Mage,
        TalentRouteView::Healer,
        TalentRouteView::Summoner,
    ] {
        let spine = route_centre_angle(route);
        // Group by depth so we can spread nodes inside a
        // single ring across the wedge angle.
        let mut per_ring: std::collections::BTreeMap<u32, Vec<usize>> = Default::default();
        for (i, node) in view.nodes.iter().enumerate() {
            if node.route != route {
                continue;
            }
            if is_satellite.contains(&i) {
                continue;
            }
            // Route depth = BFS depth - 1 (since the connector
            // sits at hub depth 0). Clamp at 0 just in case.
            let d = depths[i].saturating_sub(1);
            per_ring.entry(d).or_default().push(i);
        }
        for (ring, indices) in per_ring {
            let count = indices.len() as f32;
            let radius = ROUTE_BASE_RADIUS + ring as f32 * ROUTE_RING_STEP;
            // Spread the ring nodes across the wedge. If
            // there's a single node it sits exactly on the
            // spine; multiple nodes fan out symmetrically.
            for (slot, &i) in indices.iter().enumerate() {
                let t = if count > 1.0 {
                    (slot as f32 / (count - 1.0)) - 0.5
                } else {
                    0.0
                };
                let angle = spine + t * ROUTE_WEDGE;
                positions[i] = Pos2::new(angle.cos() * radius, angle.sin() * radius);
            }
        }
    }

    // ── Satellite placement (modifiers around their ability) ──
    // Place each modifier perpendicular to the radial vector
    // through its parent ability so they flank the ability
    // tangentially instead of crowding the spine. A small
    // outward bias keeps the satellite from sitting under the
    // incoming edge from the previous ring.
    const SATELLITE_RADIUS: f32 = 78.0;
    for (parent, sats) in &satellite_of {
        let p = positions[*parent];
        let len = (p.x * p.x + p.y * p.y).sqrt().max(1.0);
        let rx = p.x / len;
        let ry = p.y / len;
        // 90° rotation of the radial → tangent.
        let tx = -ry;
        let ty = rx;
        let count = sats.len();
        for (k, &sat_idx) in sats.iter().enumerate() {
            let t = if count > 1 {
                2.0 * (k as f32 / (count - 1) as f32) - 1.0
            } else {
                1.0
            };
            let bias = SATELLITE_RADIUS * 0.35;
            let ox = tx * (t * SATELLITE_RADIUS) + rx * bias;
            let oy = ty * (t * SATELLITE_RADIUS) + ry * bias;
            positions[sat_idx] = Pos2::new(p.x + ox, p.y + oy);
        }
    }

    positions
}

/// BFS depth of each node from the hub. Hub nodes are depth 0.
/// Route nodes are 1 + min(depth of prereqs). Unreachable
/// nodes fall back to depth 1 so they still get a sensible ring.
fn compute_depths(view: &TalentTreeView<'_>) -> Vec<u32> {
    let n = view.nodes.len();
    let mut depths = vec![u32::MAX; n];
    let mut queue: std::collections::VecDeque<usize> = Default::default();
    for (i, node) in view.nodes.iter().enumerate() {
        if node.route == TalentRouteView::Hub {
            depths[i] = 0;
            queue.push_back(i);
        }
    }
    // Build a reverse adjacency: for each node, which nodes
    // list it as a prereq?
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, node) in view.nodes.iter().enumerate() {
        for &p in &node.prereq_indices {
            let p = p as usize;
            if p < n {
                dependents[p].push(i);
            }
        }
    }
    while let Some(u) = queue.pop_front() {
        let next = depths[u].saturating_add(1);
        for &v in &dependents[u] {
            if next < depths[v] {
                depths[v] = next;
                queue.push_back(v);
            }
        }
    }
    // Fall back to ring 1 for anything we couldn't reach.
    for d in depths.iter_mut() {
        if *d == u32::MAX {
            *d = 1;
        }
    }
    depths
}

/// If `node` is a hub→route connector (a hub node whose name
/// contains the route name), return that route. The content
/// authors name the four connector nodes "Path of the
/// Warrior" / "Path of the Mage" / etc. — we lean on that
/// convention rather than carrying an extra field on the
/// view.
#[allow(dead_code)] // superseded by id-banded `connector_target`
                    // in the hub layout pass — kept around as
                    // documentation of the previous name-based
                    // detection until the auto-layout proves
                    // out in playtest.
fn route_for_connector(node: &TalentNodeView<'_>) -> Option<TalentRouteView> {
    if node.route != TalentRouteView::Hub {
        return None;
    }
    let n = node.name.to_ascii_lowercase();
    if n.contains("warrior") {
        Some(TalentRouteView::Warrior)
    } else if n.contains("mage") {
        Some(TalentRouteView::Mage)
    } else if n.contains("healer") {
        Some(TalentRouteView::Healer)
    } else if n.contains("summoner") {
        Some(TalentRouteView::Summoner)
    } else {
        None
    }
}

fn node_radius(kind: TalentNodeKind) -> f32 {
    match kind {
        TalentNodeKind::Unlock => UNLOCK_NODE_RADIUS,
        TalentNodeKind::Keystone => KEYSTONE_NODE_RADIUS,
        TalentNodeKind::Connector => NODE_RADIUS * 0.8,
        _ => NODE_RADIUS,
    }
}

fn dist2(a: Pos2, b: Pos2) -> f32 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    dx * dx + dy * dy
}

// ─── Node drawing ───────────────────────────────────────────

fn draw_node(
    ui: &mut Ui<'_>,
    node: &TalentNodeView<'_>,
    centre: Pos2,
    radius: f32,
    hovered: bool,
    matches_search: bool,
) {
    let tint = route_tint(node.route);

    // Gating drives the master alpha. The shader bakes the
    // bevel + indented gradient + halo curves; we just hand
    // it a tint and a brightness scalar.
    let invested = node.current_rank > 0;
    let maxed = node.current_rank >= node.max_rank;

    let base_alpha = if invested {
        1.0
    } else if node.investable {
        0.85
    } else if node.prereqs_met {
        0.55
    } else {
        0.32
    };
    let search_alpha = if matches_search { 1.0 } else { 0.30 };
    let alpha = base_alpha * search_alpha;

    // Real glow disc — one quad, shader-rasterised. The halo
    // extent is implicit (the shader hard-codes 35 % beyond
    // the solid disc), we just pass a non-zero hint so the
    // quad is sized generously.
    let halo = radius * 0.55;
    ui.draw_glow_disc(centre, radius, halo, tint.with_alpha(alpha));

    // Investable nodes get a second, slightly larger disc
    // overlay at low alpha — additive read with the disc
    // below produces an extra-bright rim so the eye is drawn
    // to "you can spend a point here".
    if node.investable && !maxed {
        ui.draw_glow_disc(
            centre,
            radius * 1.05,
            halo * 1.4,
            tint.with_alpha(0.45 * search_alpha),
        );
    }

    // Maxed-out nodes get a gold confirmation disc layered
    // behind the route-tint disc. Same shader path, just gold.
    if maxed && invested {
        ui.draw_glow_disc(
            centre,
            radius * 1.10,
            halo * 1.2,
            Color::rgba(0.95, 0.78, 0.32, 0.55 * search_alpha),
        );
        // Redraw the tint disc on top so the chip still reads
        // as its route tint, with the gold halo bleeding
        // outside.
        ui.draw_glow_disc(centre, radius, halo, tint.with_alpha(alpha));
    }

    // Keystones get an extra-wide outer halo so capstones
    // read as "build-defining" without changing the chip
    // itself. Drawn behind (lower in queue) so it bleeds
    // around the disc, not over it.
    if matches!(node.kind, TalentNodeKind::Keystone) {
        ui.draw_glow_disc(
            centre,
            radius * 1.15,
            halo * 2.0,
            tint.with_alpha(0.50 * search_alpha),
        );
        ui.draw_glow_disc(centre, radius, halo, tint.with_alpha(alpha));
    }

    // Hover halo — bright white disc behind the chip so the
    // hit-test affordance is unambiguous. Sits at the same
    // radius as the chip so the halo extends just beyond
    // the chip's own halo band.
    if hovered {
        ui.draw_glow_disc(
            centre,
            radius * 1.02,
            halo * 1.5,
            Color::rgba(1.0, 1.0, 1.0, 0.40),
        );
        ui.draw_glow_disc(centre, radius, halo, tint.with_alpha(alpha));
    }

    // Rank text centred on the node when invested. Skip for
    // single-rank unlocks — the chip itself is the indicator.
    if node.current_rank > 0 && node.max_rank > 1 {
        let s = ui.theme().fonts.size_sm;
        let label = format!("{}/{}", node.current_rank, node.max_rank);
        let w = ui.measure_text(&label, s);
        ui.draw_text(
            Pos2::new(centre.x - w * 0.5, centre.y - s * 0.5),
            &label,
            s,
            Color::rgba(1.0, 1.0, 1.0, alpha),
        );
    }
}

// ─── Tooltip ────────────────────────────────────────────────

fn draw_tooltip(ui: &mut Ui<'_>, node: &TalentNodeView<'_>, node_pos: Pos2, node_radius_px: f32) {
    let theme = *ui.theme();
    // Header is tinted by the node's route (mirrors how item
    // tooltips tint the title by rarity).
    let header_color = route_tint(node.route);
    // Reserve roughly one line per data row plus a divider
    // and the hint footer so the Vec rarely reallocates.
    let mut lines: Vec<TooltipLine<'_>> = Vec::with_capacity(node.tooltip_lines.len() + 5);

    // Subtitle: route name · node kind. Small dim text under
    // the header, like the item-level / req-level line on
    // item tooltips.
    let subtitle = format!("{} · {}", route_label(node.route), kind_label(node.kind));
    lines.push(TooltipLine::new(
        &subtitle,
        theme.fonts.size_sm,
        theme.colors.text_dim,
    ));

    // Top divider — separates the title block from the body.
    lines.push(
        TooltipLine::new("", theme.fonts.size_sm, theme.colors.text_dim)
            .decor(TooltipLineDecor::Divider),
    );

    // Rank progress line. Pip glyphs so the player can read
    // the progress visually as well as numerically; investable
    // rank colourised gold to match the accent on the hint.
    let rank_pips = render_rank_pips(node.current_rank, node.max_rank);
    let rank_line = format!("Rank {}/{}    {}", node.current_rank, node.max_rank, rank_pips);
    let rank_color = if node.current_rank >= node.max_rank {
        theme.colors.accent
    } else if node.current_rank > 0 {
        theme.colors.text
    } else {
        theme.colors.text_dim
    };
    lines.push(TooltipLine::new(&rank_line, theme.fonts.size_md, rank_color));

    // Body lines. The first entry from the view is the node's
    // description (long form), subsequent entries are the
    // per-rank effect summary. We size the description at
    // size_md (the inventory tooltip's "stat" tier) so it
    // reads as primary copy, and the effect lines at size_sm
    // dim so they sit as supporting detail.
    let mut body_iter = node.tooltip_lines.iter();
    if let Some(first) = body_iter.next() {
        lines.push(TooltipLine::new(
            first.as_str(),
            theme.fonts.size_md,
            theme.colors.text,
        ));
    }
    for line in body_iter {
        lines.push(TooltipLine::new(
            line.as_str(),
            theme.fonts.size_sm,
            theme.colors.text_dim,
        ));
    }

    // Bottom divider — separates the body from the action
    // footer. Skipped on maxed nodes so the maxed-state line
    // doesn't read as floating.
    lines.push(
        TooltipLine::new("", theme.fonts.size_sm, theme.colors.text_dim)
            .decor(TooltipLineDecor::Divider),
    );

    // Hint footer. Investable → gold call-to-action; missing
    // prereqs → red warning; missing points → dim neutral;
    // maxed → dim. Matches the item-tooltip pattern of using
    // colour to encode actionability without an extra glyph.
    let hint = if node.current_rank >= node.max_rank {
        "Maxed."
    } else if node.investable {
        "Click to invest."
    } else if node.prereqs_met {
        "Need an unspent talent point."
    } else {
        "Prerequisites not met."
    };
    let hint_color = if node.investable {
        theme.colors.accent
    } else if node.current_rank >= node.max_rank {
        theme.colors.text_dim
    } else if !node.prereqs_met {
        Color::rgba(0.96, 0.40, 0.40, 1.0)
    } else {
        theme.colors.text_dim
    };
    lines.push(TooltipLine::new(hint, theme.fonts.size_sm, hint_color));

    // Anchor next to the node's bounding box so the tooltip
    // never occludes the chip the player is hovering.
    let anchor = Rect::from_xywh(
        node_pos.x - node_radius_px,
        node_pos.y - node_radius_px,
        node_radius_px * 2.0,
        node_radius_px * 2.0,
    );
    Tooltip::new()
        .header(node.name)
        .header_color(header_color)
        .min_width(240.0)
        .pad(10.0)
        .anchor_to(anchor)
        .show(ui, node_pos, &lines);
}

/// "● ● ○" style rank progress. Filled glyphs for invested
/// ranks, hollow for the remaining ranks. Returns an empty
/// string when `max_rank <= 1` so single-rank unlock nodes
/// don't show a meaningless one-pip indicator.
fn render_rank_pips(current: u8, max: u8) -> String {
    if max <= 1 {
        return String::new();
    }
    let mut s = String::with_capacity(max as usize * 2);
    for i in 0..max {
        if i > 0 {
            s.push(' ');
        }
        if i < current {
            s.push('\u{25CF}'); // ●
        } else {
            s.push('\u{25CB}'); // ○
        }
    }
    s
}

fn route_label(r: TalentRouteView) -> &'static str {
    match r {
        TalentRouteView::Hub => "Hub",
        TalentRouteView::Warrior => "Warrior",
        TalentRouteView::Mage => "Mage",
        TalentRouteView::Healer => "Healer",
        TalentRouteView::Summoner => "Summoner",
    }
}

fn kind_label(k: TalentNodeKind) -> &'static str {
    match k {
        TalentNodeKind::Stat => "Passive",
        TalentNodeKind::Modifier => "Modifier",
        TalentNodeKind::Unlock => "Unlock",
        TalentNodeKind::Proc => "Proc",
        TalentNodeKind::Keystone => "Keystone",
        TalentNodeKind::Connector => "Connector",
    }
}

// ─── PanZoom state bridge ───────────────────────────────────
//
// `TalentPanelState` keeps its pan/zoom in a self-contained
// shape (plain tuples, no `Vec2`) so it doesn't leak the
// `rift-ui-im` types into the public view-model crate. These
// two helpers copy the state in/out of a real `PanZoomState`
// for the duration of one frame.

fn pan_state_proxy(state: &TalentPanelState) -> PanZoomState {
    PanZoomState {
        pan: Vec2::new(state.pan.0, state.pan.1),
        zoom: state.zoom,
        dragging: state.dragging,
        last_cursor: Pos2::new(state.last_cursor.0, state.last_cursor.1),
    }
}

fn apply_pan_state(state: &mut TalentPanelState, pz: &PanZoomState) {
    state.pan = (pz.pan.x, pz.pan.y);
    state.zoom = pz.zoom;
    state.dragging = pz.dragging;
    state.last_cursor = (pz.last_cursor.x, pz.last_cursor.y);
}

fn _transform_unused(_t: &PanZoomTransform) {}
