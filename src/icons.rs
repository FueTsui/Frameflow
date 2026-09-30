use crate::theme::Palette;
use eframe::egui::{self, Color32, Rect, Response, Sense, Shape, Stroke, Ui, Vec2, pos2, vec2};

#[derive(Clone, Copy)]
pub enum Icon {
    Menu,
    Convert,
    Compress,
    Trim,
    Audio,
    Gif,
    Snapshot,
    Remux,
    Plus,
    Folder,
    Play,
    Stop,
    Close,
    Settings,
    File,
    Code,
    Refresh,
}

pub fn paint(ui: &Ui, rect: Rect, icon: Icon, color: Color32) {
    let r = Rect::from_center_size(rect.center(), Vec2::splat(rect.width().min(rect.height())));
    // PUA code points are only used with the actual Segoe Fluent Icons family.
    // Missing system fonts or glyphs use the self-contained vector shapes below.
    let glyph = fluent_glyph(icon);
    if let Some(font) = crate::theme::fluent_icon_font(ui.ctx(), r.width())
        && ui.fonts_mut(|fonts| fonts.has_glyph(&font, glyph))
    {
        ui.painter()
            .text(r.center(), egui::Align2::CENTER_CENTER, glyph, font, color);
        return;
    }
    let at = |x: f32, y: f32| {
        pos2(
            r.left() + r.width() * x / 24.0,
            r.top() + r.height() * y / 24.0,
        )
    };
    let p = ui.painter();
    let s = Stroke::new(1.6_f32, color);
    let line = |points: &[(f32, f32)]| {
        p.add(Shape::line(
            points.iter().map(|&(x, y)| at(x, y)).collect(),
            s,
        ));
    };
    let circle = |x, y, radius| {
        p.circle_stroke(at(x, y), r.width() * radius / 24.0, s);
    };
    let box_stroke = |x1, y1, x2, y2| {
        p.rect_stroke(
            Rect::from_min_max(at(x1, y1), at(x2, y2)),
            3.0,
            s,
            egui::StrokeKind::Inside,
        );
    };
    match icon {
        Icon::Menu => {
            line(&[(4., 6.), (20., 6.)]);
            line(&[(4., 12.), (20., 12.)]);
            line(&[(4., 18.), (20., 18.)]);
        }
        Icon::Convert => {
            line(&[(4., 7.), (19., 7.), (15., 3.)]);
            line(&[(20., 17.), (5., 17.), (9., 21.)]);
        }
        Icon::Compress => {
            line(&[(3., 3.), (9., 9.), (9., 4.)]);
            line(&[(9., 9.), (4., 9.)]);
            line(&[(21., 21.), (15., 15.), (15., 20.)]);
            line(&[(15., 15.), (20., 15.)]);
        }
        Icon::Trim => {
            circle(6., 6., 3.);
            circle(6., 18., 3.);
            line(&[(8., 8.), (20., 20.)]);
            line(&[(8., 16.), (20., 4.)]);
        }
        Icon::Audio => {
            line(&[(9., 17.), (9., 5.), (20., 3.), (20., 15.)]);
            circle(6., 18., 3.);
            circle(17., 16., 3.);
        }
        Icon::Gif => {
            box_stroke(3., 4., 21., 20.);
            line(&[(10., 8.), (16., 12.), (10., 16.), (10., 8.)]);
        }
        Icon::Snapshot => {
            box_stroke(3., 6., 21., 21.);
            line(&[(8., 6.), (9., 3.), (15., 3.), (16., 6.)]);
            circle(12., 13., 4.);
        }
        Icon::Remux => {
            line(&[(4., 7.), (12., 3.), (20., 7.), (12., 11.), (4., 7.)]);
            line(&[(4., 12.), (12., 16.), (20., 12.)]);
            line(&[(4., 17.), (12., 21.), (20., 17.)]);
        }
        Icon::Plus => {
            line(&[(12., 5.), (12., 19.)]);
            line(&[(5., 12.), (19., 12.)]);
        }
        Icon::Folder => {
            line(&[
                (3., 7.),
                (3., 5.),
                (10., 5.),
                (12., 8.),
                (21., 8.),
                (21., 20.),
                (3., 20.),
                (3., 7.),
            ]);
        }
        Icon::Play => {
            p.add(Shape::convex_polygon(
                vec![at(8., 4.), at(20., 12.), at(8., 20.)],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Stop => {
            p.rect_filled(Rect::from_min_max(at(6., 6.), at(18., 18.)), 2.0, color);
        }
        Icon::Close => {
            line(&[(6., 6.), (18., 18.)]);
            line(&[(6., 18.), (18., 6.)]);
        }
        Icon::Settings => {
            circle(12., 12., 4.);
            circle(12., 12., 8.);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::TAU / 8.;
                line(&[
                    (12. + 8. * a.cos(), 12. + 8. * a.sin()),
                    (12. + 11. * a.cos(), 12. + 11. * a.sin()),
                ]);
            }
        }
        Icon::File => {
            line(&[
                (14., 3.),
                (5., 3.),
                (5., 21.),
                (19., 21.),
                (19., 8.),
                (14., 3.),
                (14., 8.),
                (19., 8.),
            ]);
            line(&[(9., 13.), (15., 13.)]);
            line(&[(9., 17.), (14., 17.)]);
        }
        Icon::Code => {
            line(&[(8., 6.), (2., 12.), (8., 18.)]);
            line(&[(16., 6.), (22., 12.), (16., 18.)]);
            line(&[(14., 4.), (10., 20.)]);
        }
        Icon::Refresh => {
            line(&[
                (20., 9.),
                (17., 4.),
                (10., 3.),
                (5., 6.),
                (3., 12.),
                (6., 18.),
                (12., 21.),
                (19., 18.),
            ]);
            line(&[(20., 3.), (20., 9.), (14., 9.)]);
        }
    }
}

fn fluent_glyph(icon: Icon) -> char {
    // https://learn.microsoft.com/windows/apps/design/iconography/segoe-fluent-icons-font
    match icon {
        Icon::Menu => '\u{e700}',
        Icon::Convert => '\u{e8ab}',  // Switch
        Icon::Compress => '\u{f012}', // ZipFolder
        Icon::Trim => '\u{e8c6}',     // Cut
        Icon::Audio => '\u{e8d6}',    // Audio
        Icon::Gif => '\u{e714}',      // Video
        Icon::Snapshot => '\u{e722}', // Camera
        Icon::Remux => '\u{e7b8}',    // Package
        Icon::Plus => '\u{e710}',     // Add
        Icon::Folder => '\u{e8b7}',   // Folder
        Icon::Play => '\u{e768}',     // Play
        Icon::Stop => '\u{e71a}',     // Stop
        Icon::Close => '\u{e711}',    // Cancel
        Icon::Settings => '\u{e713}', // Settings
        Icon::File => '\u{e8a5}',     // Document
        Icon::Code => '\u{e943}',     // Code
        Icon::Refresh => '\u{e72c}',  // Refresh
    }
}

pub fn button(
    ui: &mut Ui,
    icon: Icon,
    text: &str,
    accessible_label: &str,
    width: f32,
    primary: bool,
    p: Palette,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 32.0), Sense::click());
    let enabled = ui.is_enabled();
    let fill = if !enabled {
        p.hover
    } else if primary {
        p.accent
    } else if response.is_pointer_button_down_on() {
        p.panel
    } else if response.hovered() {
        p.hover
    } else {
        p.card
    };
    ui.painter().rect(
        rect,
        4.0,
        fill,
        Stroke::new(1.0_f32, if primary && enabled { p.accent } else { p.line }),
        egui::StrokeKind::Inside,
    );
    if primary && enabled && response.hovered() {
        let opacity = if response.is_pointer_button_down_on() {
            28
        } else {
            14
        };
        ui.painter()
            .rect_filled(rect, 4.0, Color32::from_black_alpha(opacity));
    }
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(2.),
            6.,
            Stroke::new(2_f32, p.accent),
            egui::StrokeKind::Outside,
        );
    }
    let color = if !enabled {
        p.muted
    } else if primary {
        p.accent_text
    } else {
        p.text
    };
    let center = if text.is_empty() {
        rect.center()
    } else {
        pos2(rect.left() + 21.0, rect.center().y)
    };
    paint(
        ui,
        Rect::from_center_size(center, vec2(16.0, 16.0)),
        icon,
        color,
    );
    if !text.is_empty() {
        ui.painter().text(
            pos2(rect.left() + 37., rect.center().y),
            egui::Align2::LEFT_CENTER,
            text,
            egui::FontId::proportional(14.0),
            color,
        );
    }
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, accessible_label)
    });
    response
}

pub fn app_icon() -> egui::IconData {
    let image = image::load_from_memory_with_format(
        include_bytes!("../assets/Frameflow.ico"),
        image::ImageFormat::Ico,
    )
    .expect("bundled Frameflow icon must be valid")
    .into_rgba8();
    egui::IconData {
        width: image.width(),
        height: image.height(),
        rgba: image.into_raw(),
    }
}
