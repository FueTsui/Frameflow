use eframe::egui::{self, Color32, FontFamily, FontId, Stroke, TextStyle};
use std::path::{Path, PathBuf};

pub const ACCENT: Color32 = Color32::from_rgb(0, 95, 184);
const FLUENT_ICONS: &str = "frameflow-fluent-icons";

#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color32,
    pub panel: Color32,
    pub card: Color32,
    pub line: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub accent_text: Color32,
    pub tint: Color32,
    pub hover: Color32,
    pub danger: Color32,
}

impl Palette {
    pub fn get(ctx: &egui::Context) -> Self {
        if ctx.style().visuals.dark_mode {
            Self {
                bg: c(39, 39, 39),
                panel: c(32, 32, 32),
                card: c(45, 45, 45),
                line: c(61, 61, 61),
                text: c(245, 245, 245),
                muted: c(189, 189, 189),
                accent: c(96, 205, 255),
                accent_text: c(32, 32, 32),
                tint: c(47, 61, 68),
                hover: c(52, 52, 52),
                danger: c(255, 153, 164),
            }
        } else {
            Self {
                bg: c(249, 249, 249),
                panel: c(243, 243, 243),
                card: Color32::WHITE,
                line: c(224, 224, 224),
                text: c(26, 26, 26),
                muted: c(96, 96, 96),
                accent: ACCENT,
                accent_text: Color32::WHITE,
                tint: c(230, 240, 249),
                hover: c(237, 237, 237),
                danger: c(196, 43, 28),
            }
        }
    }
}

const fn c(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

fn load_font(fonts: &mut egui::FontDefinitions, name: &str, path: &Path, index: u32) -> bool {
    if let Ok(bytes) = std::fs::read(path) {
        let mut data = egui::FontData::from_owned(bytes);
        data.index = index;
        fonts.font_data.insert(name.into(), data.into());
        true
    } else {
        false
    }
}

pub fn configure(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mut icon_font_loaded = false;
    if cfg!(target_os = "windows") {
        // Use installed system faces; fonts are never redistributed in the package.
        let font_dir = std::env::var_os("WINDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:/Windows"))
            .join("Fonts");
        for (name, filename) in [
            ("segoe-variable", "SegUIVar.ttf"),
            ("segoe-ui", "segoeui.ttf"),
        ] {
            if load_font(&mut fonts, name, &font_dir.join(filename), 0) {
                fonts
                    .families
                    .entry(FontFamily::Proportional)
                    .or_default()
                    .insert(0, name.into());
                break;
            }
        }
        // The second face in msyh.ttc is Microsoft YaHei UI, the Windows CJK UI face.
        if load_font(&mut fonts, "cjk-ui", &font_dir.join("msyh.ttc"), 1) {
            let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
            proportional.insert(proportional.len().min(1), "cjk-ui".into());
            fonts
                .families
                .entry(FontFamily::Monospace)
                .or_default()
                .push("cjk-ui".into());
        } else if load_font(&mut fonts, "cjk-ui", &font_dir.join("simhei.ttf"), 0) {
            for family in [FontFamily::Proportional, FontFamily::Monospace] {
                fonts
                    .families
                    .entry(family)
                    .or_default()
                    .push("cjk-ui".into());
            }
        }
        icon_font_loaded = load_font(
            &mut fonts,
            FLUENT_ICONS,
            &font_dir.join("SegoeIcons.ttf"),
            0,
        );
        if icon_font_loaded {
            fonts.families.insert(
                FontFamily::Name(FLUENT_ICONS.into()),
                vec![FLUENT_ICONS.into()],
            );
        }
    } else {
        let candidates: &[&str] = if cfg!(target_os = "macos") {
            &[
                "/System/Library/Fonts/PingFang.ttc",
                "/System/Library/Fonts/STHeiti Light.ttc",
                "/System/Library/Fonts/Hiragino Sans GB.ttc",
            ]
        } else {
            &["/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc"]
        };
        for path in candidates {
            if load_font(&mut fonts, "cjk", Path::new(path), 0) {
                for family in [FontFamily::Proportional, FontFamily::Monospace] {
                    fonts.families.entry(family).or_default().push("cjk".into());
                }
                break;
            }
        }
    }
    ctx.data_mut(|data| data.insert_temp(egui::Id::new(FLUENT_ICONS), icon_font_loaded));
    ctx.set_fonts(fonts);
    ctx.all_styles_mut(|style| {
        style
            .text_styles
            .insert(TextStyle::Body, FontId::proportional(14.0));
        style
            .text_styles
            .insert(TextStyle::Button, FontId::proportional(14.0));
        style
            .text_styles
            .insert(TextStyle::Small, FontId::proportional(12.0));
        style
            .text_styles
            .insert(TextStyle::Heading, FontId::proportional(28.0));
        // Fluent's Windows type ramp: caption 12, body 14, subtitle 20,
        // title 28. Section labels use 16 to keep dense export forms readable.
        style.text_styles.insert(
            TextStyle::Name("section".into()),
            FontId::proportional(16.0),
        );
        style.text_styles.insert(
            TextStyle::Name("subtitle".into()),
            FontId::proportional(20.0),
        );
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(12.0, 6.0);
        style.spacing.interact_size.y = 32.0;
        style.spacing.icon_width = 20.0;
        style.spacing.icon_width_inner = 12.0;
        style.spacing.icon_spacing = 8.0;
        style.spacing.indent = 24.0;
        style.spacing.slider_rail_height = 4.0;
        style.spacing.combo_height = 240.0;
        style.spacing.scroll = egui::style::ScrollStyle {
            bar_width: 10.0,
            floating_width: 3.0,
            floating_allocated_width: 8.0,
            bar_inner_margin: 4.0,
            dormant_handle_opacity: 0.45,
            ..egui::style::ScrollStyle::floating()
        };
        style.animation_time = 0.15;
        // These are egui's built-in hover explanations, independent of the
        // app's explicit tooltips. Keep them disabled for every appearance.
        style.explanation_tooltips = false;
        style.url_in_tooltip = false;
        style.visuals.window_corner_radius = 8.into();
        style.visuals.menu_corner_radius = 8.into();
        style.visuals.widgets.noninteractive.corner_radius = 4.into();
        style.visuals.widgets.inactive.corner_radius = 4.into();
        style.visuals.widgets.hovered.corner_radius = 4.into();
        style.visuals.widgets.active.corner_radius = 4.into();
        style.visuals.widgets.open.corner_radius = 4.into();
        style.visuals.widgets.hovered.expansion = 0.0;
        style.visuals.widgets.active.expansion = 0.0;
        style.visuals.widgets.open.expansion = 0.0;
        style.visuals.interact_cursor = None;
    });
}

pub fn fluent_icon_font(ctx: &egui::Context, size: f32) -> Option<FontId> {
    let loaded = ctx
        .data(|data| data.get_temp::<bool>(egui::Id::new(FLUENT_ICONS)))
        .unwrap_or(false);
    loaded.then(|| FontId::new(size, FontFamily::Name(FLUENT_ICONS.into())))
}

/// Fluent's compact outline chevron for `ComboBox::icon`.
pub fn combo_chevron(
    ui: &egui::Ui,
    rect: egui::Rect,
    visuals: &egui::style::WidgetVisuals,
    open: bool,
) {
    let center = rect.center();
    let half_width = (rect.width().min(rect.height()) * 0.25).min(4.0);
    let half_height = half_width * 0.5;
    let direction = if open { -1.0 } else { 1.0 };
    ui.painter().add(egui::Shape::line(
        vec![
            center + egui::vec2(-half_width, -half_height * direction),
            center + egui::vec2(0.0, half_height * direction),
            center + egui::vec2(half_width, -half_height * direction),
        ],
        Stroke::new(1.25_f32, visuals.fg_stroke.color),
    ));
}

/// Outline chevron animated from right to down for `CollapsingHeader::icon`.
pub fn collapse_chevron(ui: &mut egui::Ui, openness: f32, response: &egui::Response) {
    let center = response.rect.center();
    let half_height = (response.rect.width().min(response.rect.height()) * 0.25).min(4.0);
    let rotation =
        egui::emath::Rot2::from_angle(openness.clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2);
    let points = [
        egui::vec2(-half_height * 0.5, -half_height),
        egui::vec2(half_height * 0.5, 0.0),
        egui::vec2(-half_height * 0.5, half_height),
    ]
    .into_iter()
    .map(|offset| center + rotation * offset)
    .collect();
    ui.painter().add(egui::Shape::line(
        points,
        Stroke::new(1.25_f32, ui.style().interact(response).fg_stroke.color),
    ));
}

pub fn apply(ctx: &egui::Context) -> Palette {
    sync_window_theme(ctx);
    let p = Palette::get(ctx);
    ctx.style_mut(|s| {
        s.visuals.override_text_color = Some(p.text);
        s.visuals.weak_text_color = Some(p.muted);
        s.visuals.panel_fill = p.bg;
        s.visuals.window_fill = p.card;
        s.visuals.window_stroke = Stroke::new(1.0_f32, p.line);
        s.visuals.extreme_bg_color = p.bg;
        s.visuals.text_edit_bg_color = Some(p.card);
        s.visuals.faint_bg_color = p.panel;
        s.visuals.code_bg_color = p.panel;
        s.visuals.hyperlink_color = p.accent;
        s.visuals.error_fg_color = p.danger;
        s.visuals.selection.bg_fill = p.tint;
        s.visuals.selection.stroke = Stroke::new(1.0_f32, p.accent);
        s.visuals.widgets.noninteractive.bg_fill = p.panel;
        s.visuals.widgets.noninteractive.weak_bg_fill = p.panel;
        s.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.line);
        s.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, p.text);
        s.visuals.widgets.inactive.bg_fill = p.line;
        s.visuals.widgets.inactive.weak_bg_fill = p.card;
        s.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, p.line);
        s.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, p.text);
        s.visuals.widgets.hovered.bg_fill = p.hover;
        s.visuals.widgets.hovered.weak_bg_fill = p.hover;
        s.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, p.line);
        s.visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, p.text);
        s.visuals.widgets.active.bg_fill = p.tint;
        s.visuals.widgets.active.weak_bg_fill = p.tint;
        s.visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, p.accent);
        s.visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, p.text);
        s.visuals.widgets.open.bg_fill = p.hover;
        s.visuals.widgets.open.weak_bg_fill = p.hover;
        s.visuals.widgets.open.bg_stroke = Stroke::new(1.0_f32, p.line);
        s.visuals.widgets.open.fg_stroke = Stroke::new(1.0_f32, p.text);
    });
    p
}

fn sync_window_theme(ctx: &egui::Context) {
    // egui's style preference does not itself update the native window frame.
    // Reapply manual appearance after an OS theme change too: winit's Windows
    // WM_SETTINGCHANGE handler can restore the system frame colors.
    let preference = ctx.options(|options| options.theme_preference);
    let state = (preference, ctx.system_theme());
    let id = egui::Id::new(("frameflow-native-theme", ctx.viewport_id()));
    let changed = ctx.data_mut(|data| {
        if data.get_temp::<(egui::ThemePreference, Option<egui::Theme>)>(id) == Some(state) {
            false
        } else {
            data.insert_temp(id, state);
            true
        }
    });
    if changed {
        let theme = match preference {
            egui::ThemePreference::Light => egui::SystemTheme::Light,
            egui::ThemePreference::Dark => egui::SystemTheme::Dark,
            egui::ThemePreference::System => egui::SystemTheme::SystemDefault,
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(theme));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn native_theme_commands(ctx: &egui::Context, os_theme: egui::Theme) -> Vec<egui::SystemTheme> {
        let output = ctx.run(
            egui::RawInput {
                system_theme: Some(os_theme),
                ..Default::default()
            },
            |ctx| {
                apply(ctx);
            },
        );
        output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .iter()
            .filter_map(|command| match command {
                egui::ViewportCommand::SetTheme(theme) => Some(*theme),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn appearance_changes_sync_native_frame_and_unchanged_frames_emit_no_commands() {
        let ctx = egui::Context::default();
        for (preference, expected) in [
            (egui::ThemePreference::Light, egui::SystemTheme::Light),
            (egui::ThemePreference::Dark, egui::SystemTheme::Dark),
            (
                egui::ThemePreference::System,
                egui::SystemTheme::SystemDefault,
            ),
        ] {
            ctx.set_theme(preference);
            assert_eq!(native_theme_commands(&ctx, egui::Theme::Light), [expected]);
            assert!(native_theme_commands(&ctx, egui::Theme::Light).is_empty());
        }
    }

    #[test]
    fn manual_frame_theme_is_reasserted_after_an_os_theme_change() {
        let ctx = egui::Context::default();
        ctx.set_theme(egui::ThemePreference::Light);
        assert_eq!(
            native_theme_commands(&ctx, egui::Theme::Light),
            [egui::SystemTheme::Light]
        );
        assert_eq!(
            native_theme_commands(&ctx, egui::Theme::Dark),
            [egui::SystemTheme::Light]
        );
        assert!(native_theme_commands(&ctx, egui::Theme::Dark).is_empty());
        assert!(!ctx.style().visuals.dark_mode);
    }

    #[test]
    fn system_frame_theme_remains_automatic_when_os_appearance_changes() {
        let ctx = egui::Context::default();
        ctx.set_theme(egui::ThemePreference::System);
        for os_theme in [egui::Theme::Light, egui::Theme::Dark] {
            assert_eq!(
                native_theme_commands(&ctx, os_theme),
                [egui::SystemTheme::SystemDefault]
            );
            assert_eq!(ctx.style().visuals.dark_mode, os_theme == egui::Theme::Dark);
            assert!(native_theme_commands(&ctx, os_theme).is_empty());
        }
    }
}
