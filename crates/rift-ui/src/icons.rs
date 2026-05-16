use rift_ui_im::{Color, Pos2, Rect, Ui};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiIcon {
    Add,
    Back,
    Cancel,
    Character,
    Check,
    Delete,
    Edit,
    Exit,
    Filter,
    Gear,
    Invite,
    Palette,
    Play,
    Portal,
    Recycle,
    Sort,
    Stats,
    Whisper,
    Book,
    Bag,
    Damage,
    Healing,
    Shield,
    Threat,
    Volume,
    Monitor,
    Male,
    Female,
    CaretLeft,
    CaretRight,
    CaretUp,
    CaretDown,
}

pub fn draw_placeholder_icon(ui: &mut Ui<'_>, rect: Rect, icon: UiIcon, color: Color) {
    let theme = *ui.theme();
    let s = theme.scale;
    let c = rect.center();
    let w = rect.width().min(rect.height());
    let r = w * 0.34;
    let thin = (1.3 * s).max(1.0);
    let line = (1.8 * s).max(1.0);

    match icon {
        UiIcon::Add => {
            ui.draw_line(
                Pos2::new(c.x - r, c.y),
                Pos2::new(c.x + r, c.y),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x, c.y - r),
                Pos2::new(c.x, c.y + r),
                line,
                color,
            );
        }
        UiIcon::Back => {
            ui.draw_line(
                Pos2::new(c.x + r, c.y - r),
                Pos2::new(c.x - r, c.y),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x - r, c.y),
                Pos2::new(c.x + r, c.y + r),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x - r, c.y),
                Pos2::new(c.x + r * 0.7, c.y),
                line,
                color,
            );
        }
        UiIcon::Cancel => {
            ui.draw_line(
                Pos2::new(c.x - r, c.y - r),
                Pos2::new(c.x + r, c.y + r),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x + r, c.y - r),
                Pos2::new(c.x - r, c.y + r),
                line,
                color,
            );
        }
        UiIcon::Character => {
            ui.draw_circle(Pos2::new(c.x, c.y - r * 0.45), r * 0.34, color);
            ui.draw_line(
                Pos2::new(c.x, c.y - r * 0.05),
                Pos2::new(c.x, c.y + r * 0.85),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x - r * 0.62, c.y + r * 0.15),
                Pos2::new(c.x + r * 0.62, c.y + r * 0.15),
                line,
                color,
            );
        }
        UiIcon::Check => {
            ui.draw_line(
                Pos2::new(c.x - r, c.y),
                Pos2::new(c.x - r * 0.25, c.y + r * 0.7),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x - r * 0.25, c.y + r * 0.7),
                Pos2::new(c.x + r, c.y - r * 0.65),
                line,
                color,
            );
        }
        UiIcon::Delete => {
            let bin = Rect::from_xywh(c.x - r * 0.75, c.y - r * 0.35, r * 1.5, r * 1.25);
            ui.draw_outline(bin, thin, color);
            ui.draw_line(
                Pos2::new(bin.x() - r * 0.1, bin.y() - r * 0.25),
                Pos2::new(bin.max.x + r * 0.1, bin.y() - r * 0.25),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x - r * 0.35, bin.y() - r * 0.45),
                Pos2::new(c.x + r * 0.35, bin.y() - r * 0.45),
                line,
                color,
            );
        }
        UiIcon::Edit => {
            ui.draw_line(
                Pos2::new(c.x - r * 0.72, c.y + r * 0.72),
                Pos2::new(c.x + r * 0.58, c.y - r * 0.58),
                line,
                color,
            );
            ui.draw_triangle(
                Pos2::new(c.x + r * 0.58, c.y - r * 0.58),
                Pos2::new(c.x + r, c.y - r),
                Pos2::new(c.x + r * 0.82, c.y - r * 0.28),
                color,
            );
            ui.draw_line(
                Pos2::new(c.x - r * 0.9, c.y + r),
                Pos2::new(c.x - r * 0.2, c.y + r * 0.78),
                line,
                color,
            );
        }
        UiIcon::Exit => {
            let door = Rect::from_xywh(c.x - r, c.y - r, r * 1.05, r * 2.0);
            ui.draw_outline(door, thin, color);
            ui.draw_line(
                Pos2::new(c.x - r * 0.1, c.y),
                Pos2::new(c.x + r, c.y),
                line,
                color,
            );
            ui.draw_triangle(
                Pos2::new(c.x + r, c.y),
                Pos2::new(c.x + r * 0.45, c.y - r * 0.45),
                Pos2::new(c.x + r * 0.45, c.y + r * 0.45),
                color,
            );
        }
        UiIcon::Filter => {
            ui.draw_line(
                Pos2::new(c.x - r, c.y - r),
                Pos2::new(c.x + r, c.y - r),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x - r, c.y - r),
                Pos2::new(c.x - r * 0.2, c.y),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x + r, c.y - r),
                Pos2::new(c.x + r * 0.2, c.y),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x - r * 0.2, c.y),
                Pos2::new(c.x, c.y + r),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x + r * 0.2, c.y),
                Pos2::new(c.x, c.y + r),
                line,
                color,
            );
        }
        UiIcon::Gear => {
            ui.draw_circle(c, r * 0.34, color);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::TAU / 8.0;
                let p0 = Pos2::new(c.x + a.cos() * r * 0.62, c.y + a.sin() * r * 0.62);
                let p1 = Pos2::new(c.x + a.cos() * r, c.y + a.sin() * r);
                ui.draw_line(p0, p1, line, color);
            }
        }
        UiIcon::Invite => {
            ui.draw_circle(Pos2::new(c.x - r * 0.35, c.y - r * 0.25), r * 0.3, color);
            ui.draw_line(
                Pos2::new(c.x - r * 0.35, c.y + r * 0.15),
                Pos2::new(c.x - r * 0.35, c.y + r),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x + r * 0.35, c.y - r * 0.15),
                Pos2::new(c.x + r * 0.35, c.y + r * 0.75),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x - r * 0.1, c.y + r * 0.3),
                Pos2::new(c.x + r * 0.8, c.y + r * 0.3),
                line,
                color,
            );
        }
        UiIcon::Palette => {
            ui.draw_circle(c, r, color);
            ui.draw_circle(
                Pos2::new(c.x - r * 0.35, c.y - r * 0.25),
                r * 0.16,
                Color::rgba(0.0, 0.0, 0.0, 0.35),
            );
            ui.draw_circle(
                Pos2::new(c.x + r * 0.25, c.y - r * 0.35),
                r * 0.16,
                Color::rgba(0.0, 0.0, 0.0, 0.35),
            );
            ui.draw_circle(
                Pos2::new(c.x + r * 0.15, c.y + r * 0.35),
                r * 0.18,
                Color::rgba(0.0, 0.0, 0.0, 0.35),
            );
        }
        UiIcon::Play => ui.draw_triangle(
            Pos2::new(c.x - r * 0.55, c.y - r),
            Pos2::new(c.x - r * 0.55, c.y + r),
            Pos2::new(c.x + r, c.y),
            color,
        ),
        UiIcon::Portal => {
            ui.draw_circle(c, r, color);
            ui.draw_circle(c, r * 0.55, Color::rgba(0.0, 0.0, 0.0, 0.30));
        }
        UiIcon::Recycle => {
            ui.draw_line(
                Pos2::new(c.x - r, c.y + r * 0.15),
                Pos2::new(c.x - r * 0.2, c.y - r),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x - r * 0.2, c.y - r),
                Pos2::new(c.x + r * 0.55, c.y - r * 0.35),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x + r * 0.55, c.y - r * 0.35),
                Pos2::new(c.x + r * 0.2, c.y - r * 0.35),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x + r * 0.7, c.y + r),
                Pos2::new(c.x - r * 0.45, c.y + r),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x - r * 0.45, c.y + r),
                Pos2::new(c.x - r, c.y + r * 0.15),
                line,
                color,
            );
        }
        UiIcon::Sort => {
            for i in 0..3 {
                let y = c.y - r + i as f32 * r;
                ui.draw_line(
                    Pos2::new(c.x - r, y),
                    Pos2::new(c.x + r * (0.3 + i as f32 * 0.35), y),
                    line,
                    color,
                );
            }
            ui.draw_line(
                Pos2::new(c.x + r, c.y - r),
                Pos2::new(c.x + r, c.y + r),
                thin,
                color,
            );
            ui.draw_triangle(
                Pos2::new(c.x + r, c.y + r),
                Pos2::new(c.x + r * 0.72, c.y + r * 0.55),
                Pos2::new(c.x + r * 1.28, c.y + r * 0.55),
                color,
            );
        }
        UiIcon::Stats => {
            let bw = r * 0.38;
            for (i, h) in [0.65, 1.25, 0.95].iter().enumerate() {
                let x = c.x - r + i as f32 * bw * 1.45;
                ui.draw_rect(Rect::from_xywh(x, c.y + r - r * h, bw, r * h), color);
            }
        }
        UiIcon::Whisper => {
            let bubble = Rect::from_xywh(c.x - r, c.y - r * 0.75, r * 2.0, r * 1.35);
            ui.draw_outline(bubble, thin, color);
            ui.draw_triangle(
                Pos2::new(c.x - r * 0.2, bubble.max.y),
                Pos2::new(c.x - r * 0.55, c.y + r),
                Pos2::new(c.x + r * 0.25, bubble.max.y),
                color,
            );
        }
        UiIcon::Book => {
            let book = Rect::from_xywh(c.x - r, c.y - r * 0.78, r * 2.0, r * 1.56);
            ui.draw_outline(book, thin, color);
            ui.draw_line(
                Pos2::new(c.x, book.y()),
                Pos2::new(c.x, book.max.y),
                thin,
                color,
            );
            ui.draw_line(
                Pos2::new(book.x() + r * 0.25, c.y - r * 0.25),
                Pos2::new(c.x - r * 0.18, c.y - r * 0.25),
                thin,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x + r * 0.18, c.y - r * 0.25),
                Pos2::new(book.max.x - r * 0.25, c.y - r * 0.25),
                thin,
                color,
            );
        }
        UiIcon::Bag => {
            let bag = Rect::from_xywh(c.x - r, c.y - r * 0.35, r * 2.0, r * 1.25);
            ui.draw_outline(bag, thin, color);
            ui.draw_line(
                Pos2::new(c.x - r * 0.45, bag.y()),
                Pos2::new(c.x - r * 0.25, c.y - r),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x + r * 0.45, bag.y()),
                Pos2::new(c.x + r * 0.25, c.y - r),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x - r * 0.25, c.y - r),
                Pos2::new(c.x + r * 0.25, c.y - r),
                line,
                color,
            );
        }
        UiIcon::Damage => {
            ui.draw_line(
                Pos2::new(c.x - r * 0.8, c.y + r),
                Pos2::new(c.x + r * 0.65, c.y - r),
                line,
                color,
            );
            ui.draw_triangle(
                Pos2::new(c.x + r * 0.65, c.y - r),
                Pos2::new(c.x + r * 0.22, c.y - r * 0.72),
                Pos2::new(c.x + r * 0.52, c.y - r * 0.38),
                color,
            );
        }
        UiIcon::Healing => {
            ui.draw_line(
                Pos2::new(c.x - r, c.y),
                Pos2::new(c.x + r, c.y),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x, c.y - r),
                Pos2::new(c.x, c.y + r),
                line,
                color,
            );
        }
        UiIcon::Shield => {
            ui.draw_line(
                Pos2::new(c.x, c.y - r),
                Pos2::new(c.x - r, c.y - r * 0.45),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x, c.y - r),
                Pos2::new(c.x + r, c.y - r * 0.45),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x - r, c.y - r * 0.45),
                Pos2::new(c.x - r * 0.55, c.y + r * 0.65),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x + r, c.y - r * 0.45),
                Pos2::new(c.x + r * 0.55, c.y + r * 0.65),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x - r * 0.55, c.y + r * 0.65),
                Pos2::new(c.x, c.y + r),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x + r * 0.55, c.y + r * 0.65),
                Pos2::new(c.x, c.y + r),
                line,
                color,
            );
        }
        UiIcon::Threat => {
            ui.draw_line(
                Pos2::new(c.x - r, c.y),
                Pos2::new(c.x - r * 0.25, c.y - r * 0.55),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x - r * 0.25, c.y - r * 0.55),
                Pos2::new(c.x + r * 0.5, c.y),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x + r * 0.5, c.y),
                Pos2::new(c.x - r * 0.25, c.y + r * 0.55),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x - r * 0.25, c.y + r * 0.55),
                Pos2::new(c.x - r, c.y),
                line,
                color,
            );
            ui.draw_circle(Pos2::new(c.x - r * 0.18, c.y), r * 0.18, color);
        }
        UiIcon::Volume => {
            ui.draw_rect(
                Rect::from_xywh(c.x - r, c.y - r * 0.35, r * 0.45, r * 0.70),
                color,
            );
            ui.draw_triangle(
                Pos2::new(c.x - r * 0.55, c.y - r * 0.35),
                Pos2::new(c.x + r * 0.1, c.y - r),
                Pos2::new(c.x + r * 0.1, c.y + r),
                color,
            );
            ui.draw_line(
                Pos2::new(c.x + r * 0.35, c.y - r * 0.55),
                Pos2::new(c.x + r * 0.7, c.y - r * 0.9),
                thin,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x + r * 0.35, c.y + r * 0.55),
                Pos2::new(c.x + r * 0.7, c.y + r * 0.9),
                thin,
                color,
            );
        }
        UiIcon::Monitor => {
            let screen = Rect::from_xywh(c.x - r, c.y - r * 0.75, r * 2.0, r * 1.35);
            ui.draw_outline(screen, thin, color);
            ui.draw_line(
                Pos2::new(c.x, screen.max.y),
                Pos2::new(c.x, c.y + r),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x - r * 0.5, c.y + r),
                Pos2::new(c.x + r * 0.5, c.y + r),
                line,
                color,
            );
        }
        UiIcon::Male => {
            ui.draw_circle(Pos2::new(c.x - r * 0.2, c.y + r * 0.2), r * 0.55, color);
            ui.draw_line(
                Pos2::new(c.x + r * 0.18, c.y - r * 0.18),
                Pos2::new(c.x + r, c.y - r),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x + r, c.y - r),
                Pos2::new(c.x + r, c.y - r * 0.35),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x + r, c.y - r),
                Pos2::new(c.x + r * 0.35, c.y - r),
                line,
                color,
            );
        }
        UiIcon::Female => {
            ui.draw_circle(Pos2::new(c.x, c.y - r * 0.2), r * 0.55, color);
            ui.draw_line(
                Pos2::new(c.x, c.y + r * 0.35),
                Pos2::new(c.x, c.y + r),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x - r * 0.45, c.y + r * 0.72),
                Pos2::new(c.x + r * 0.45, c.y + r * 0.72),
                line,
                color,
            );
        }
        UiIcon::CaretLeft => ui.draw_triangle(
            Pos2::new(c.x - r * 0.75, c.y),
            Pos2::new(c.x + r * 0.45, c.y - r * 0.85),
            Pos2::new(c.x + r * 0.45, c.y + r * 0.85),
            color,
        ),
        UiIcon::CaretRight => ui.draw_triangle(
            Pos2::new(c.x + r * 0.75, c.y),
            Pos2::new(c.x - r * 0.45, c.y - r * 0.85),
            Pos2::new(c.x - r * 0.45, c.y + r * 0.85),
            color,
        ),
        UiIcon::CaretUp => ui.draw_triangle(
            Pos2::new(c.x, c.y - r * 0.72),
            Pos2::new(c.x - r * 0.85, c.y + r * 0.38),
            Pos2::new(c.x + r * 0.85, c.y + r * 0.38),
            color,
        ),
        UiIcon::CaretDown => ui.draw_triangle(
            Pos2::new(c.x, c.y + r * 0.72),
            Pos2::new(c.x - r * 0.85, c.y - r * 0.38),
            Pos2::new(c.x + r * 0.85, c.y - r * 0.38),
            color,
        ),
    }
}

/// Line-art silhouette for an empty equipment paperdoll cell (`EquipSlotIdx.0`).
/// Matches immediate-mode icon weight used by [`draw_placeholder_icon`].
pub fn draw_equip_slot_placeholder(ui: &mut Ui<'_>, rect: Rect, slot_idx: u8, color: Color) {
    let theme = *ui.theme();
    let s = theme.scale;
    let c = rect.center();
    let w = rect.width().min(rect.height());
    let r = w * 0.31;
    let line = (1.48 * s).max(1.0);
    let thin = (1.15 * s).max(1.0);

    match slot_idx {
        // Weapon — diagonal blade + guard hint.
        0 => {
            ui.draw_line(
                Pos2::new(c.x - r * 0.78, c.y + r * 0.88),
                Pos2::new(c.x + r * 0.88, c.y - r * 0.78),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x - r * 0.35, c.y + r * 0.42),
                Pos2::new(c.x + r * 0.42, c.y - r * 0.35),
                thin,
                color,
            );
        }
        // Helm — dome + cheek plates + brim.
        1 => {
            ui.draw_line(
                Pos2::new(c.x - r * 0.68, c.y + r * 0.42),
                Pos2::new(c.x - r * 0.55, c.y - r * 0.38),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x + r * 0.68, c.y + r * 0.42),
                Pos2::new(c.x + r * 0.55, c.y - r * 0.38),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x - r * 0.55, c.y - r * 0.38),
                Pos2::new(c.x + r * 0.55, c.y - r * 0.38),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x - r * 0.72, c.y + r * 0.42),
                Pos2::new(c.x + r * 0.72, c.y + r * 0.42),
                thin,
                color,
            );
        }
        // Chest — torso wedge + waist.
        2 => {
            ui.draw_line(
                Pos2::new(c.x, c.y - r * 0.62),
                Pos2::new(c.x - r * 0.62, c.y + r * 0.72),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x, c.y - r * 0.62),
                Pos2::new(c.x + r * 0.62, c.y + r * 0.72),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x - r * 0.58, c.y + r * 0.48),
                Pos2::new(c.x + r * 0.58, c.y + r * 0.48),
                thin,
                color,
            );
        }
        // Legs — waist + two shafts.
        3 => {
            ui.draw_line(
                Pos2::new(c.x - r * 0.52, c.y - r * 0.58),
                Pos2::new(c.x + r * 0.52, c.y - r * 0.58),
                thin,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x - r * 0.28, c.y - r * 0.52),
                Pos2::new(c.x - r * 0.38, c.y + r * 0.88),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x + r * 0.28, c.y - r * 0.52),
                Pos2::new(c.x + r * 0.38, c.y + r * 0.88),
                line,
                color,
            );
        }
        // Hands — paired glove blocks.
        4 => {
            let gw = r * 0.52;
            let gh = r * 0.78;
            ui.draw_outline(
                Rect::from_xywh(c.x - r * 0.92, c.y - gh * 0.42, gw, gh),
                thin,
                color,
            );
            ui.draw_outline(
                Rect::from_xywh(c.x + r * 0.92 - gw, c.y - gh * 0.42, gw, gh),
                thin,
                color,
            );
        }
        // Boots — paired soles tilted outward.
        5 => {
            ui.draw_line(
                Pos2::new(c.x - r * 0.22, c.y - r * 0.52),
                Pos2::new(c.x - r * 0.62, c.y + r * 0.82),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x + r * 0.22, c.y - r * 0.52),
                Pos2::new(c.x + r * 0.62, c.y + r * 0.82),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x - r * 0.72, c.y + r * 0.82),
                Pos2::new(c.x - r * 0.18, c.y + r * 0.82),
                thin,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x + r * 0.18, c.y + r * 0.82),
                Pos2::new(c.x + r * 0.72, c.y + r * 0.82),
                thin,
                color,
            );
        }
        // Ring — circular band + facet core (finger A).
        6 => {
            let rr = r * 0.58;
            let ring = Rect::from_xywh(c.x - rr, c.y - rr, rr * 2.0, rr * 2.0);
            ui.draw_rounded_outline(ring, rr, thin, color);
            ui.draw_line(
                Pos2::new(c.x, c.y - rr * 0.35),
                Pos2::new(c.x, c.y + rr * 0.35),
                thin,
                color,
            );
        }
        // Second ring — offset facet so symmetric rings read differently at a glance.
        7 => {
            let rr = r * 0.58;
            let ring = Rect::from_xywh(c.x - rr, c.y - rr, rr * 2.0, rr * 2.0);
            ui.draw_rounded_outline(ring, rr, thin, color);
            ui.draw_line(
                Pos2::new(c.x - rr * 0.32, c.y - rr * 0.28),
                Pos2::new(c.x + rr * 0.32, c.y + rr * 0.28),
                thin,
                color,
            );
        }
        // Amulet — chain stub + teardrop pendant.
        8 => {
            ui.draw_line(
                Pos2::new(c.x, c.y - r * 0.88),
                Pos2::new(c.x, c.y - r * 0.18),
                thin,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x - r * 0.42, c.y + r * 0.72),
                Pos2::new(c.x, c.y - r * 0.12),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x + r * 0.42, c.y + r * 0.72),
                Pos2::new(c.x, c.y - r * 0.12),
                line,
                color,
            );
        }
        // Shoulders — mirrored pauldrons + collar tie.
        9 => {
            ui.draw_line(
                Pos2::new(c.x - r, c.y + r * 0.22),
                Pos2::new(c.x - r * 0.42, c.y - r * 0.48),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x + r, c.y + r * 0.22),
                Pos2::new(c.x + r * 0.42, c.y - r * 0.48),
                line,
                color,
            );
            ui.draw_line(
                Pos2::new(c.x - r * 0.42, c.y - r * 0.48),
                Pos2::new(c.x + r * 0.42, c.y - r * 0.48),
                thin,
                color,
            );
        }
        _ => draw_placeholder_icon(ui, icon_rect_center(rect, w * 0.52), UiIcon::Gear, color),
    }
}

pub fn icon_rect_left(rect: Rect, size: f32, pad: f32) -> Rect {
    Rect::from_xywh(
        rect.x() + pad,
        rect.y() + (rect.height() - size) * 0.5,
        size,
        size,
    )
}

pub fn icon_rect_center(rect: Rect, size: f32) -> Rect {
    Rect::from_xywh(
        rect.x() + (rect.width() - size) * 0.5,
        rect.y() + (rect.height() - size) * 0.5,
        size,
        size,
    )
}
