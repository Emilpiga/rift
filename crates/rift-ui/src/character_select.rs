//! Character-select widgets.
//!
//! Each function takes `&mut Ui` plus a view-model from
//! `rift-ui-types` and returns an action enum (or nothing).
//! No state lives in this crate; the host owns the roster
//! and the current selection.

use rift_ui_im::{
    widgets::{label, text_field},
    Button, ButtonSize, Color, Column, Frame, Id, Pad, PanelHeader, Pos2, Rect, Row, Sized, Stroke,
    Ui, Vec2,
};
use rift_ui_types::character_select::{
    CreateAction, CreateFormView, DeleteAction, DeleteConfirmView, LoadingRosterView, RosterAction,
    RosterEntryView, RosterView, MAX_CHARACTER_SLOTS,
};

use crate::icons::{draw_placeholder_icon, icon_rect_center, icon_rect_left, UiIcon};

// ─── helpers ─────────────────────────────────────────────────

/// Left-side floating stone panel rect. Narrow (~30 % of
/// screen) on purpose — the 3D character preview lives in
/// the centre and wants the rest of the screen.
fn panel_rect(ui: &Ui<'_>) -> Rect {
    let s = ui.screen_size();
    let w = (s.x * 0.30).clamp(380.0, 560.0);
    let margin = s.x * 0.04;
    Rect::from_xywh(margin, s.y * 0.08, w, s.y * 0.84)
}

// ─── frame_loading_roster ────────────────────────────────────

pub fn frame_loading_roster(ui: &mut Ui<'_>, view: &LoadingRosterView<'_>) {
    let panel = panel_rect(ui);
    let theme = *ui.theme();

    Frame::stone(&theme).show(ui, panel, |ui, body| {
        let s = theme.scale;
        let header_h = 44.0 * s;
        let header = Rect::from_xywh(panel.x(), panel.y(), panel.width(), header_h);
        let (_, content) = body.split_off_top(header_h);
        PanelHeader::new("ACCOUNT")
            .subtitle("Retrieving roster")
            .show(ui, header);

        let dots = match (view.anim_time * 1.5) as i32 % 4 {
            0 => "",
            1 => ".",
            2 => "..",
            _ => "...",
        };
        let card = Rect::from_xywh(
            content.x() + 8.0 * s,
            content.y() + 24.0 * s,
            content.width() - 16.0 * s,
            112.0 * s,
        );
        Frame::inset(&theme)
            .with_fill(Color::rgba(0.06, 0.04, 0.12, 0.48))
            .with_stroke(Stroke::new(1.0, Color::rgba(0.52, 0.38, 0.82, 0.58)))
            .with_padding(Pad::all(0.0))
            .show_only(ui, card);

        let label_text = format!("Loading roster for '{}'{dots}", view.account_name);
        let label_w = ui.measure_text(&label_text, theme.fonts.size_md);
        ui.draw_text(
            Pos2::new(
                card.x() + (card.width() - label_w) * 0.5,
                card.y() + 24.0 * s,
            ),
            &label_text,
            theme.fonts.size_md,
            theme.colors.text,
        );
        draw_loading_runes(ui, card, view.anim_time);
    });
}

// ─── roster row ──────────────────────────────────────────────

/// Render one filled roster row: level badge ▸ avatar ▸ name
/// container. The whole row is clickable for selection. The
/// name container fills the remaining width and switches to
/// the red `selected` style when this row is the current pick.
///
/// Returns `true` if the row was clicked this frame.
fn render_filled_row(
    ui: &mut Ui<'_>,
    rect: Rect,
    profile: &RosterEntryView<'_>,
    selected: bool,
    id: Id,
) -> bool {
    let theme = *ui.theme();
    let s = theme.scale;
    let hovered = ui.interact_hover(id, rect);
    let clicked = hovered && ui.input().left_clicked();

    let row_fill = if selected {
        Color::rgba(0.16, 0.06, 0.26, 0.92)
    } else if hovered {
        Color::rgba(
            theme.colors.bg_stone_alt.0[0],
            theme.colors.bg_stone_alt.0[1],
            theme.colors.bg_stone_alt.0[2],
            theme.colors.bg_stone_alt.0[3] * 0.85,
        )
    } else {
        theme.colors.bg_stone
    };
    Frame::inset(&theme)
        .with_fill(row_fill)
        .with_stroke(Stroke::new(
            if selected { 2.0 } else { 1.0 },
            if selected {
                Color::rgba(0.72, 0.52, 0.96, 0.88)
            } else {
                theme.colors.border_stone
            },
        ))
        .with_padding(Pad::all(0.0))
        .show_only(ui, rect);
    if selected || hovered {
        ui.draw_gradient_rect(
            Rect::from_xywh(
                rect.x() + 2.0 * s,
                rect.y() + 2.0 * s,
                (rect.width() - 4.0 * s).max(0.0),
                rect.height() * 0.40,
            ),
            Color::rgba(0.68, 0.52, 1.0, if selected { 0.14 } else { 0.08 }),
            Color::rgba(0.68, 0.52, 1.0, 0.0),
        );
    }

    // Internal layout: pad → level badge → gap → name.
    let pad = 10.0 * s;
    let badge_w = 56.0 * s;
    let gap = 10.0 * s;

    let cx = rect.x() + pad;
    let cy = rect.y() + pad;
    let inner_h = rect.height() - pad * 2.0;

    let badge_rect = Rect::from_xywh(cx, cy, badge_w, inner_h);
    Frame::inset(&theme)
        .with_fill(Color::rgba(0.10, 0.06, 0.18, 0.96))
        .with_stroke(Stroke::new(1.0, Color::rgba(0.68, 0.52, 0.92, 0.70)))
        .with_padding(Pad::all(0.0))
        .show_only(ui, badge_rect);
    ui.draw_gradient_rect(
        Rect::from_xywh(
            badge_rect.x() + 1.0 * s,
            badge_rect.y() + 1.0 * s,
            (badge_rect.width() - 2.0 * s).max(0.0),
            badge_rect.height() * 0.45,
        ),
        Color::rgba(0.78, 0.62, 1.0, 0.18),
        Color::rgba(0.78, 0.62, 1.0, 0.0),
    );
    let lvl_text = format!("{}", profile.level);
    let lvl_size = theme.fonts.size_lg;
    let lvl_w = ui.measure_text(&lvl_text, lvl_size);
    ui.draw_text(
        Pos2::new(
            badge_rect.x() + (badge_rect.width() - lvl_w) * 0.5,
            badge_rect.y() + (badge_rect.height() - lvl_size) * 0.5,
        ),
        &lvl_text,
        lvl_size,
        theme.colors.text,
    );
    // "Lv" caption above the number.
    ui.draw_text(
        Pos2::new(badge_rect.x() + 4.0 * s, badge_rect.y() + 2.0 * s),
        "Lv",
        theme.fonts.size_sm * 0.85,
        theme.colors.text_muted,
    );

    let name_x = badge_rect.max.x + gap;
    let name_rect = Rect::from_xywh(name_x, cy, (rect.max.x - pad) - name_x, inner_h);
    Frame::inset(&theme)
        .with_fill(if selected {
            Color::rgba(0.18, 0.07, 0.28, 0.94)
        } else {
            Color::rgba(0.05, 0.04, 0.09, 0.92)
        })
        .with_stroke(Stroke::new(
            1.0,
            if selected {
                Color::rgba(0.78, 0.58, 0.98, 0.75)
            } else {
                Color::rgba(0.42, 0.34, 0.62, 0.52)
            },
        ))
        .with_padding(Pad::all(0.0))
        .show_only(ui, name_rect);
    ui.draw_gradient_rect(
        Rect::from_xywh(
            name_rect.x() + 1.0 * s,
            name_rect.y() + 1.0 * s,
            (name_rect.width() - 2.0 * s).max(0.0),
            name_rect.height() * 0.46,
        ),
        if selected {
            Color::rgba(0.72, 0.48, 0.98, 0.16)
        } else {
            Color::rgba(0.56, 0.42, 0.82, 0.10)
        },
        Color::rgba(0.0, 0.0, 0.0, 0.0),
    );
    let gender_band = Rect::from_xywh(
        name_rect.x() + 1.0 * s,
        name_rect.max.y - 20.0 * s,
        (name_rect.width() - 2.0 * s).max(0.0),
        19.0 * s,
    );
    ui.draw_rect(
        gender_band,
        if selected {
            Color::rgba(0.12, 0.06, 0.18, 0.45)
        } else {
            Color::rgba(0.0, 0.0, 0.0, 0.20)
        },
    );

    // Name text (left-aligned, vertical centre). Sub-line
    // below: gender label in a muted tone so the row reads
    // at a glance.
    let name_size = theme.fonts.size_lg;
    let sub_size = theme.fonts.size_sm;
    let name_y = name_rect.y() + name_rect.height() * 0.5 - name_size * 0.75;
    ui.draw_text(
        Pos2::new(name_rect.x() + 12.0 * s, name_y),
        profile.name,
        name_size,
        theme.colors.text,
    );
    ui.draw_text(
        Pos2::new(name_rect.x() + 12.0 * s, name_y + name_size + 2.0 * s),
        profile.gender_label,
        sub_size,
        if selected {
            theme.colors.text
        } else {
            theme.colors.text_dim
        },
    );

    clicked
}

// ─── frame_roster ────────────────────────────────────────────

pub fn frame_roster(ui: &mut Ui<'_>, view: &RosterView<'_>) -> RosterAction {
    let panel = panel_rect(ui);
    let theme = *ui.theme();
    let mut action = RosterAction::None;

    Frame::stone(&theme).show(ui, panel, |ui, body| {
        let s = theme.scale;
        let header_h = 46.0 * s;
        let header = Rect::from_xywh(panel.x(), panel.y(), panel.width(), header_h);
        let (_, body) = body.split_off_top(header_h);
        PanelHeader::new("CHARACTERS")
            .subtitle("Select a hero")
            .right_text(&format!("{}/{}", view.entries.len(), MAX_CHARACTER_SLOTS))
            .show(ui, header);

        // Single vertical Column owning the whole panel body
        // below the title gutter. Items: 5 roster rows + Play
        // + a secondary (Delete | Quit) row, all separated by
        // the same `gap`. That guarantees identical margins
        // between every element and stops the rows from
        // expanding into the Play button when the panel grows
        // tall \u2014 row height is fixed.
        let row_h = 60.0 * s;
        let play_h = 64.0 * s;
        let secondary_h = 36.0 * s;
        let gap = 10.0 * s;

        let (_, body_below_header) = body.split_off_top(14.0 * s);
        let mut col = Column::new(body_below_header).gap(gap);
        for _ in 0..MAX_CHARACTER_SLOTS {
            col = col.item(Sized::fixed(row_h));
        }
        col = col
            .item(Sized::flex(1.0)) // slack so the footer hugs the bottom
            .item(Sized::fixed(play_h))
            .item(Sized::fixed(secondary_h));
        let cells = col.layout();
        let row_rects = &cells[..MAX_CHARACTER_SLOTS];
        let play_rect = cells[MAX_CHARACTER_SLOTS + 1];
        let secondary_row = cells[MAX_CHARACTER_SLOTS + 2];

        let filled = view.entries.len();
        for (i, row_rect) in row_rects.iter().copied().enumerate() {
            let id = Id::root("char_select").child(("row", i));

            if let Some(profile) = view.entries.get(i) {
                let selected = view.selected == Some(i);
                if render_filled_row(ui, row_rect, profile, selected, id) {
                    action = RosterAction::Select(i);
                }
            } else if i == filled && view.allow_create {
                // "+ Create new" row uses the Normal button so
                // it's clearly secondary to the Red Play CTA.
                let create = Button::new("  Create New Character").show_with_id(
                    ui,
                    Id::root("char_select").child(("create_slot", i)),
                    row_rect,
                );
                draw_placeholder_icon(
                    ui,
                    icon_rect_left(row_rect, 18.0 * s, 12.0 * s),
                    UiIcon::Add,
                    theme.colors.text,
                );
                if create.clicked {
                    action = RosterAction::Create;
                }
            } else {
                // Empty / locked slot \u2014 dashed placeholder.
                Frame::inset(&theme)
                    .with_fill(Color::rgba(0.0, 0.0, 0.0, 0.18))
                    .with_stroke(Stroke::new(1.0, Color::rgba(0.52, 0.42, 0.72, 0.38)))
                    .with_padding(Pad::all(0.0))
                    .show_only(ui, row_rect);
                ui.draw_text(
                    Pos2::new(
                        row_rect.x() + 14.0 * s,
                        row_rect.y() + row_rect.height() * 0.5 - 7.0 * s,
                    ),
                    "(empty slot)",
                    theme.fonts.size_sm,
                    theme.colors.text_muted,
                );
            }
        }

        // \u2500\u2500\u2500 footer \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500
        let has_selection = view.selected.is_some();

        let play = Button::red("PLAY")
            .large()
            .enabled(has_selection)
            .show_with_id(ui, Id::root("char_select").child("play"), play_rect);
        draw_placeholder_icon(
            ui,
            icon_rect_left(play_rect, 22.0 * s, 16.0 * s),
            UiIcon::Play,
            theme.colors.text,
        );
        if play.clicked {
            action = RosterAction::Play;
        }

        let secondary = Row::new(secondary_row).gap(gap).equal(2).layout();
        let del = Button::red("  Delete")
            .small()
            .enabled(has_selection)
            .show_with_id(ui, Id::root("char_select").child("delete"), secondary[0]);
        draw_placeholder_icon(
            ui,
            icon_rect_left(secondary[0], 16.0 * s, 9.0 * s),
            UiIcon::Delete,
            theme.colors.text,
        );
        let quit = Button::new("  Quit").small().show_with_id(
            ui,
            Id::root("char_select").child("quit"),
            secondary[1],
        );
        draw_placeholder_icon(
            ui,
            icon_rect_left(secondary[1], 16.0 * s, 9.0 * s),
            UiIcon::Exit,
            theme.colors.text,
        );
        if del.clicked {
            action = RosterAction::Delete;
        } else if quit.clicked {
            action = RosterAction::Quit;
        }
    });

    action
}

// ─── frame_create ────────────────────────────────────────────

pub fn frame_create(ui: &mut Ui<'_>, view: &mut CreateFormView<'_>) -> CreateAction {
    let panel = panel_rect(ui);
    let theme = *ui.theme();
    let mut action = CreateAction::None;

    Frame::stone(&theme).show(ui, panel, |ui, body| {
        let s = theme.scale;
        let header_h = 46.0 * s;
        let header = Rect::from_xywh(panel.x(), panel.y(), panel.width(), header_h);
        let (_, body) = body.split_off_top(header_h);
        PanelHeader::new("CREATE CHARACTER")
            .subtitle("Create your hero")
            .show(ui, header);

        // Reserve the title gutter at the top *and* the
        // footer at the bottom up front, then lay the form
        // contents into the remaining area as a single
        // vertical Column. Each form section is a fixed-
        // height strip (label + control) so the spacing
        // adapts to scale automatically. Without splitting
        // off the title gutter the form's first cell would
        // start at body.min and overlap the title text.
        let footer_h = 56.0 * s;
        let (_, after_title) = body.split_off_top(20.0 * s);
        let (form_area, footer_area) = after_title.split_off_bottom(footer_h);

        let label_h = 22.0 * s;
        let field_h = 50.0 * s;
        let gender_h = 40.0 * s;
        let option_h = 34.0 * s;
        let section_gap = 8.0 * s;
        let group_gap = 12.0 * s;

        let form = Column::new(form_area)
            .gap(section_gap)
            .item(Sized::fixed(label_h)) // "Name" label
            .item(Sized::fixed(field_h)) // name field
            .item(Sized::fixed(group_gap)) // spacer
            .item(Sized::fixed(label_h)) // "Gender" label
            .item(Sized::fixed(gender_h)) // gender row
            .item(Sized::fixed(label_h)) // "Chest" label
            .item(Sized::fixed(option_h)) // chest row
            .item(Sized::fixed(label_h)) // "Skin" label
            .item(Sized::fixed(option_h)) // skin row
            .item(Sized::fixed(label_h)) // "Hair" label
            .item(Sized::fixed(option_h)) // hair row
            .item(Sized::fixed(label_h)) // "Hair color" label
            .item(Sized::fixed(option_h)) // hair color row
            .item(Sized::fixed(label_h)) // "Eyebrows" label
            .item(Sized::fixed(option_h)) // eyebrow row
            .item(Sized::fixed(label_h)) // "Eyebrow color" label
            .item(Sized::fixed(option_h)) // eyebrow color row
            .item(Sized::flex(1.0)) // remaining slack
            .layout();

        label(ui, form[0].min, "Name");
        let name_resp = text_field(
            ui,
            Id::root("char_select").child("create_name"),
            form[1],
            view.name,
            "Type a name…",
            18,
            view.anim_time,
        );

        label(ui, form[3].min, "Gender");
        // Gender toggle: 50/50 split, identical sizes.
        let gender_cells = Row::new(form[4]).gap(12.0 * s).equal(2).layout();
        let male_active = *view.gender_is_male;
        let female_active = !*view.gender_is_male;
        let male_btn = if male_active {
            Button::active("  Male")
        } else {
            Button::new("  Male")
        };
        let female_btn = if female_active {
            Button::active("  Female")
        } else {
            Button::new("  Female")
        };
        if male_btn.show(ui, gender_cells[0]).clicked {
            *view.gender_is_male = true;
        }
        draw_placeholder_icon(
            ui,
            icon_rect_left(gender_cells[0], 18.0 * s, 12.0 * s),
            UiIcon::Male,
            theme.colors.text,
        );
        if female_btn.show(ui, gender_cells[1]).clicked {
            *view.gender_is_male = false;
        }
        draw_placeholder_icon(
            ui,
            icon_rect_left(gender_cells[1], 18.0 * s, 12.0 * s),
            UiIcon::Female,
            theme.colors.text,
        );

        label(ui, form[5].min, "Chest");
        value_slider(
            ui,
            Id::root("char_select").child("chest_size"),
            form[6],
            view.chest_size,
            "Shape",
        );
        label(ui, form[7].min, "Skin");
        option_stepper(ui, form[8], "Tone", view.skin_tone, 10);
        label(ui, form[9].min, "Hair");
        option_stepper(ui, form[10], "Style", view.hair_style, 3);
        label(ui, form[11].min, "Hair color");
        hue_picker(
            ui,
            Id::root("char_select").child("hair_color"),
            form[12],
            view.hair_color,
        );
        label(ui, form[13].min, "Eyebrows");
        option_stepper(ui, form[14], "Shape", view.eyebrow_style, 2);
        label(ui, form[15].min, "Eyebrow color");
        hue_picker(
            ui,
            Id::root("char_select").child("eyebrow_color"),
            form[16],
            view.eyebrow_color,
        );

        // Footer: CONFIRM (red) + Cancel (neutral) split
        // 50/50 so the affordances read as a deliberate pair.
        let footer_cells = Row::new(footer_area).gap(12.0 * s).equal(2).layout();
        let can_confirm = !view.name.trim().is_empty();
        let confirm = Button::red("  CONFIRM")
            .size(ButtonSize::Large)
            .enabled(can_confirm)
            .show_with_id(
                ui,
                Id::root("char_select").child("create_confirm"),
                footer_cells[0],
            );
        draw_placeholder_icon(
            ui,
            icon_rect_left(footer_cells[0], 22.0 * s, 14.0 * s),
            UiIcon::Check,
            theme.colors.text,
        );
        let cancel = Button::new("  Cancel")
            .size(ButtonSize::Large)
            .show_with_id(
                ui,
                Id::root("char_select").child("create_cancel"),
                footer_cells[1],
            );
        draw_placeholder_icon(
            ui,
            icon_rect_left(footer_cells[1], 22.0 * s, 14.0 * s),
            UiIcon::Cancel,
            theme.colors.text,
        );
        let enter = ui.input().enter_just_pressed() && name_resp.focused && can_confirm;
        if (confirm.clicked || enter) && can_confirm {
            action = CreateAction::Confirm;
        } else if cancel.clicked {
            action = CreateAction::Cancel;
        }
    });

    action
}

fn option_stepper(ui: &mut Ui<'_>, rect: Rect, label_text: &str, value: &mut u8, count: u8) {
    let theme = *ui.theme();
    let s = theme.scale;
    let cells = Row::new(rect)
        .gap(8.0 * s)
        .item(Sized::fixed(42.0 * s))
        .item(Sized::flex(1.0))
        .item(Sized::fixed(42.0 * s))
        .layout();
    if Button::new("").show(ui, cells[0]).clicked {
        *value = value.wrapping_add(count - 1) % count;
    }
    draw_placeholder_icon(
        ui,
        icon_rect_center(cells[0], 14.0 * s),
        UiIcon::CaretLeft,
        theme.colors.text,
    );
    let text = format!("{label_text} {}", (*value % count) + 1);
    let w = ui.measure_text(&text, theme.fonts.size_md);
    ui.draw_text(
        Pos2::new(
            cells[1].x() + (cells[1].width() - w) * 0.5,
            cells[1].y() + (cells[1].height() - theme.fonts.size_md) * 0.5,
        ),
        &text,
        theme.fonts.size_md,
        theme.colors.text,
    );
    if Button::new("").show(ui, cells[2]).clicked {
        *value = value.wrapping_add(1) % count;
    }
    draw_placeholder_icon(
        ui,
        icon_rect_center(cells[2], 14.0 * s),
        UiIcon::CaretRight,
        theme.colors.text,
    );
}

fn value_slider(ui: &mut Ui<'_>, id: Id, rect: Rect, value: &mut u8, label_text: &str) {
    let theme = *ui.theme();
    let s = theme.scale;
    let cells = Row::new(rect)
        .gap(10.0 * s)
        .item(Sized::fixed(74.0 * s))
        .item(Sized::flex(1.0))
        .layout();
    let percent = ((*value as f32 / 255.0) * 100.0).round() as i32;
    let text = format!("{label_text} {percent}%");
    ui.draw_text(
        Pos2::new(
            cells[0].x(),
            cells[0].y() + (cells[0].height() - theme.fonts.size_sm) * 0.5,
        ),
        &text,
        theme.fonts.size_sm,
        theme.colors.text_muted,
    );

    let bar = cells[1].shrink2(Pad::symmetric(0.0, 7.0 * s));
    ui.draw_grad4_rect(
        bar,
        theme.colors.bg_panel_alt,
        theme.colors.accent,
        theme.colors.bg_panel_alt,
        theme.colors.accent,
    );
    ui.draw_outline(bar, 1.0 * s, theme.colors.border_stone);
    let hovered = ui.interact_hover(id, bar);
    if hovered && (ui.input().left_just_pressed() || ui.input().left_mouse_held()) {
        let mx = ui.input().mouse_pos().0;
        let t = ((mx - bar.x()) / bar.width().max(1.0)).clamp(0.0, 1.0);
        *value = (t * 255.0).round() as u8;
    }

    let knob_x = bar.x() + (*value as f32 / 255.0) * bar.width();
    let knob = Rect::from_xywh(
        knob_x - 4.0 * s,
        bar.y() - 4.0 * s,
        8.0 * s,
        bar.height() + 8.0 * s,
    );
    ui.draw_rect(knob, Color::rgba(0.02, 0.018, 0.015, 0.88));
    ui.draw_outline(knob, 1.0 * s, theme.colors.text);
}

fn hue_picker(ui: &mut Ui<'_>, id: Id, rect: Rect, value: &mut u8) {
    let theme = *ui.theme();
    let s = theme.scale;
    let cells = Row::new(rect)
        .gap(10.0 * s)
        .item(Sized::fixed(34.0 * s))
        .item(Sized::flex(1.0))
        .layout();
    let swatch = cells[0].shrink2(Pad::symmetric(3.0 * s, 3.0 * s));
    let color = hue_color(*value);
    ui.draw_rect(swatch, color);
    ui.draw_outline(swatch, 1.0 * s, theme.colors.border_strong);

    let bar = cells[1].shrink2(Pad::symmetric(0.0, 7.0 * s));
    draw_hue_bar(ui, bar);
    let hovered = ui.interact_hover(id, bar);
    if hovered && (ui.input().left_just_pressed() || ui.input().left_mouse_held()) {
        let mx = ui.input().mouse_pos().0;
        let t = ((mx - bar.x()) / bar.width().max(1.0)).clamp(0.0, 1.0);
        *value = (t * 255.0).round() as u8;
    }

    let knob_x = bar.x() + (*value as f32 / 255.0) * bar.width();
    let knob = Rect::from_xywh(
        knob_x - 3.0 * s,
        bar.y() - 3.0 * s,
        6.0 * s,
        bar.height() + 6.0 * s,
    );
    ui.draw_rect(knob, Color::rgba(0.02, 0.018, 0.015, 0.88));
    ui.draw_outline(knob, 1.0 * s, theme.colors.text);
}

fn draw_hue_bar(ui: &mut Ui<'_>, rect: Rect) {
    const STOPS: [[f32; 3]; 7] = [
        [0.78, 0.22, 0.22],
        [0.78, 0.62, 0.22],
        [0.48, 0.78, 0.22],
        [0.22, 0.78, 0.50],
        [0.22, 0.52, 0.78],
        [0.58, 0.22, 0.78],
        [0.78, 0.22, 0.22],
    ];
    for i in 0..(STOPS.len() - 1) {
        let t0 = i as f32 / (STOPS.len() - 1) as f32;
        let t1 = (i + 1) as f32 / (STOPS.len() - 1) as f32;
        let x0 = rect.x() + rect.width() * t0;
        let x1 = rect.x() + rect.width() * t1;
        let l = Color::rgba(STOPS[i][0], STOPS[i][1], STOPS[i][2], 1.0);
        let r = Color::rgba(STOPS[i + 1][0], STOPS[i + 1][1], STOPS[i + 1][2], 1.0);
        ui.draw_grad4_rect(
            Rect::from_xywh(x0, rect.y(), x1 - x0, rect.height()),
            l,
            r,
            l,
            r,
        );
    }
    ui.draw_outline(rect, 1.0, ui.theme().colors.border_stone);
}

fn hue_color(value: u8) -> Color {
    let rgb = hsv_to_rgb(value as f32 / 255.0, 0.72, 0.78);
    Color::rgba(rgb[0], rgb[1], rgb[2], 1.0)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h = h.fract() * 6.0;
    let i = h.floor();
    let f = h - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    match i as i32 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

// ─── frame_delete_confirm ────────────────────────────────────

pub fn frame_delete_confirm(ui: &mut Ui<'_>, view: &DeleteConfirmView<'_>) -> DeleteAction {
    let screen = ui.screen_rect();
    ui.with_layer(rift_ui_im::Layer::Modal, |ui| {
        ui.draw_rect(screen, Color::rgba(0.0, 0.0, 0.0, 0.55));
    });

    let theme = *ui.theme();
    let s = ui.screen_size();
    let sc = theme.scale;
    let mw = 460.0 * sc;
    let mh = 220.0 * sc;
    let modal_rect = Rect::from_xywh((s.x - mw) * 0.5, (s.y - mh) * 0.5, mw, mh);

    let mut action = DeleteAction::None;
    ui.with_layer(rift_ui_im::Layer::Modal, |ui| {
        Frame::stone(&theme)
            .with_padding(Pad::all(20.0 * sc))
            .show(ui, modal_rect, |ui, body| {
                let header_h = 42.0 * sc;
                let header =
                    Rect::from_xywh(modal_rect.x(), modal_rect.y(), modal_rect.width(), header_h);
                let (_, body) = body.split_off_top(header_h);
                PanelHeader::new("DELETE CHARACTER")
                    .title_color(Color::rgba(0.88, 0.72, 1.0, 1.0))
                    .show(ui, header);
                label(
                    ui,
                    body.min + Vec2::new(0.0, 22.0 * sc),
                    &format!("\"{}\" will be permanently removed.", view.character_name),
                );
                // Modal footer: matched-size Delete (red) and
                // Cancel (neutral) sit on a single baseline,
                // splitting the modal width equally so the
                // affordances read as a deliberate pair.
                let btn_h = 50.0 * sc;
                let footer = body.bottom(btn_h);
                let cells = Row::new(footer).gap(12.0 * sc).equal(2).layout();
                let yes = Button::red("  DELETE")
                    .size(ButtonSize::Large)
                    .show_with_id(ui, Id::root("char_select").child("del_yes"), cells[0]);
                draw_placeholder_icon(
                    ui,
                    icon_rect_left(cells[0], 22.0 * sc, 14.0 * sc),
                    UiIcon::Delete,
                    theme.colors.text,
                );
                let no = Button::new("  Cancel")
                    .size(ButtonSize::Large)
                    .show_with_id(ui, Id::root("char_select").child("del_no"), cells[1]);
                draw_placeholder_icon(
                    ui,
                    icon_rect_left(cells[1], 22.0 * sc, 14.0 * sc),
                    UiIcon::Cancel,
                    theme.colors.text,
                );
                if yes.clicked {
                    action = DeleteAction::Confirm;
                } else if no.clicked {
                    action = DeleteAction::Cancel;
                }
            });
    });
    action
}

fn draw_loading_runes(ui: &mut Ui<'_>, rect: Rect, anim_time: f32) {
    let theme = *ui.theme();
    let s = theme.scale;
    let count = 9;
    let dot = 8.0 * s;
    let gap = 8.0 * s;
    let total_w = count as f32 * dot + (count - 1) as f32 * gap;
    let start_x = rect.x() + (rect.width() - total_w) * 0.5;
    let y = rect.max.y - 34.0 * s;
    let active = ((anim_time * 8.0) as usize) % count;
    for i in 0..count {
        let alpha = if i == active { 0.95 } else { 0.22 };
        let x = start_x + i as f32 * (dot + gap);
        let r = Rect::from_xywh(x, y, dot, dot);
        ui.draw_rect(r, Color::rgba(0.72, 0.52, 0.98, alpha));
        ui.draw_outline(r, 1.0, Color::rgba(0.10, 0.06, 0.16, 0.82));
    }
}
