//! Typed, bounded FFmpeg composition filters. File paths are input arguments, never graph text.
//! Filter semantics: https://ffmpeg.org/ffmpeg-filters.html

use eframe::egui;
use serde::{Deserialize, Serialize};
use std::{ffi::OsString, path::PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rotation {
    #[default]
    None,
    Clockwise90,
    HalfTurn,
    CounterClockwise90,
}

impl Rotation {
    fn label(self) -> &'static str {
        match self {
            Self::None => "不旋转",
            Self::Clockwise90 => "顺时针 90°",
            Self::HalfTurn => "180°",
            Self::CounterClockwise90 => "逆时针 90°",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OverlayPosition {
    TopLeft,
    TopRight,
    BottomLeft,
    #[default]
    BottomRight,
    Center,
}

impl OverlayPosition {
    fn label(self) -> &'static str {
        match self {
            Self::TopLeft => "左上",
            Self::TopRight => "右上",
            Self::BottomLeft => "左下",
            Self::BottomRight => "右下",
            Self::Center => "居中",
        }
    }

    fn coordinates(self, margin: u32) -> (String, String) {
        let left = format!("min({margin},max(0,main_w-overlay_w))");
        let right = format!("max(0,main_w-overlay_w-{margin})");
        let top = format!("min({margin},max(0,main_h-overlay_h))");
        let bottom = format!("max(0,main_h-overlay_h-{margin})");
        match self {
            Self::TopLeft => (left, top),
            Self::TopRight => (right, top),
            Self::BottomLeft => (left, bottom),
            Self::BottomRight => (right, bottom),
            Self::Center => ("(main_w-overlay_w)/2".into(), "(main_h-overlay_h)/2".into()),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct WatermarkOptions {
    /// Activation belongs to the selected local asset and resets with that asset on restart.
    #[serde(skip)]
    pub enabled: bool,
    #[serde(skip)]
    pub path: Option<PathBuf>,
    pub position: OverlayPosition,
    pub width_percent: u32,
    pub opacity: f64,
    pub margin: u32,
}

impl Default for WatermarkOptions {
    fn default() -> Self {
        Self {
            enabled: false,
            path: None,
            position: OverlayPosition::BottomRight,
            width_percent: 20,
            opacity: 1.0,
            margin: 16,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompositionLayout {
    #[default]
    None,
    Horizontal,
    Vertical,
    PictureInPicture,
}

impl CompositionLayout {
    fn label(self) -> &'static str {
        match self {
            Self::None => "关闭",
            Self::Horizontal => "左右并排",
            Self::Vertical => "上下排列",
            Self::PictureInPicture => "画中画",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CompositionOptions {
    #[serde(skip)]
    pub layout: CompositionLayout,
    #[serde(skip)]
    pub files: Vec<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct MixTrack {
    pub path: PathBuf,
    pub gain: f64,
    pub offset_seconds: f64,
    pub duration_seconds: f64,
}

impl MixTrack {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            gain: 1.0,
            offset_seconds: 0.0,
            duration_seconds: 0.0,
        }
    }
}

impl Default for MixTrack {
    fn default() -> Self {
        Self::new(PathBuf::new())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct MixOptions {
    #[serde(skip)]
    pub enabled: bool,
    pub include_source: bool,
    pub source_gain: f64,
    #[serde(skip)]
    pub tracks: Vec<MixTrack>,
}

impl Default for MixOptions {
    fn default() -> Self {
        Self {
            enabled: false,
            include_source: true,
            source_gain: 1.0,
            tracks: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EffectsOptions {
    pub rotation: Rotation,
    pub flip_horizontal: bool,
    pub flip_vertical: bool,
    pub watermark: WatermarkOptions,
    pub composition: CompositionOptions,
    pub mix: MixOptions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectInputKind {
    Image,
    Video,
    Audio,
}

#[derive(Clone, Debug)]
pub struct EffectInput {
    pub path: PathBuf,
    pub kind: EffectInputKind,
}

pub fn has_video_effects(options: &EffectsOptions) -> bool {
    options.rotation != Rotation::None
        || options.flip_horizontal
        || options.flip_vertical
        || options.watermark.enabled
        || options.composition.layout != CompositionLayout::None
}

pub fn has_audio_effects(options: &EffectsOptions) -> bool {
    options.mix.enabled
}

/// This order is also used by build_graph. Do not add or reorder inputs between the two calls.
pub fn inputs(options: &EffectsOptions) -> Vec<EffectInput> {
    let mut inputs = Vec::new();
    if options.watermark.enabled
        && let Some(path) = &options.watermark.path
    {
        inputs.push(EffectInput {
            path: path.clone(),
            kind: EffectInputKind::Image,
        });
    }
    if options.composition.layout != CompositionLayout::None {
        inputs.extend(
            options
                .composition
                .files
                .iter()
                .cloned()
                .map(|path| EffectInput {
                    path,
                    kind: EffectInputKind::Video,
                }),
        );
    }
    if options.mix.enabled {
        inputs.extend(options.mix.tracks.iter().map(|track| EffectInput {
            path: track.path.clone(),
            kind: EffectInputKind::Audio,
        }));
    }
    inputs
}

pub fn append_input_args(input: &EffectInput, args: &mut Vec<OsString>) {
    if input.kind == EffectInputKind::Image {
        args.extend(["-loop", "1", "-framerate", "25"].map(OsString::from));
    }
    args.push("-i".into());
    args.push(input.path.as_os_str().to_owned());
}

pub fn validate(options: &EffectsOptions) -> Result<(), String> {
    if options.watermark.enabled {
        if options.watermark.path.is_none() {
            return Err("请选择水印图片。".into());
        }
        if !(1..=100).contains(&options.watermark.width_percent)
            || !options.watermark.opacity.is_finite()
            || !(0.0..=1.0).contains(&options.watermark.opacity)
            || options.watermark.margin > 2000
        {
            return Err("水印尺寸、透明度或边距无效。".into());
        }
    }
    if options.composition.layout != CompositionLayout::None
        && !(1..=3).contains(&options.composition.files.len())
    {
        return Err("画面合成需要添加 1 至 3 个视频。".into());
    }
    if options.mix.enabled {
        if options.mix.tracks.len() > 8 {
            return Err("混音最多添加 8 个音频。".into());
        }
        if !options.mix.source_gain.is_finite() || !(0.0..=4.0).contains(&options.mix.source_gain) {
            return Err("原声音量应为 0 至 4 倍。".into());
        }
        for track in &options.mix.tracks {
            if !track.gain.is_finite()
                || !(0.0..=4.0).contains(&track.gain)
                || !track.offset_seconds.is_finite()
                || !(0.0..=864000.0).contains(&track.offset_seconds)
                || !track.duration_seconds.is_finite()
                || !(0.0..=864000.0).contains(&track.duration_seconds)
            {
                return Err("混音音量、开始时间或时长无效。".into());
            }
        }
    }
    for input in inputs(options) {
        if !input.path.is_file() {
            return Err(format!("附加素材不存在：{}", input.path.display()));
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
pub struct VideoBase<'a> {
    pub label: &'a str,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Default)]
pub struct EffectGraph {
    pub filters: Vec<String>,
    /// Unbracketed: either the supplied input specifier or a generated graph label.
    pub video_label: Option<String>,
    pub audio_label: Option<String>,
}

fn label_valid(label: &str) -> bool {
    !label.is_empty()
        && label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == ':' || c == '_')
}

fn even(value: u32) -> u32 {
    value.max(2).div_ceil(2) * 2
}

fn fitted(width: u32, height: u32) -> String {
    format!(
        "scale={width}:{height}:force_original_aspect_ratio=decrease:force_divisible_by=2,pad={width}:{height}:(ow-iw)/2:(oh-ih)/2,setsar=1"
    )
}

/// All external inputs start at their own beginning. Short videos freeze their final frame;
/// mixed audio pads with silence. Both timelines are capped to the selected main-file duration.
pub fn build_graph(
    options: &EffectsOptions,
    base_video: Option<VideoBase<'_>>,
    base_audio: Option<&str>,
    first_extra_input: usize,
    duration: f64,
) -> Result<EffectGraph, String> {
    validate(options)?;
    if base_video.is_some_and(|video| !label_valid(video.label))
        || base_audio.is_some_and(|label| !label_valid(label))
    {
        return Err("无效的滤镜输入。".into());
    }
    let timed = options.composition.layout != CompositionLayout::None || has_audio_effects(options);
    if timed && (!duration.is_finite() || duration <= 0.0) {
        return Err("添加效果前需要有效的素材时长。".into());
    }
    let mut graph = EffectGraph {
        video_label: base_video.map(|video| video.label.to_owned()),
        audio_label: base_audio.map(str::to_owned),
        ..Default::default()
    };
    let watermark_input = options.watermark.enabled.then_some(first_extra_input);
    let video_input_start = first_extra_input + usize::from(options.watermark.enabled);
    let composition_count = if options.composition.layout == CompositionLayout::None {
        0
    } else {
        options.composition.files.len()
    };
    let audio_input_start = video_input_start + composition_count;
    if has_video_effects(options) {
        let base = base_video.ok_or("当前素材没有可处理的视频画面。")?;
        if base.width == 0 || base.height == 0 || base.width > 16384 || base.height > 16384 {
            return Err("画面尺寸无效。".into());
        }
        let (mut width, mut height) = (even(base.width), even(base.height));
        let mut first_filters = vec!["settb=AVTB".to_owned(), "setpts=PTS-STARTPTS".to_owned()];
        match options.rotation {
            Rotation::None => (),
            Rotation::Clockwise90 => {
                first_filters.push("transpose=clock".into());
                std::mem::swap(&mut width, &mut height);
            }
            Rotation::HalfTurn => {
                first_filters.push("hflip".into());
                first_filters.push("vflip".into());
            }
            Rotation::CounterClockwise90 => {
                first_filters.push("transpose=cclock".into());
                std::mem::swap(&mut width, &mut height);
            }
        }
        if options.flip_horizontal {
            first_filters.push("hflip".into());
        }
        if options.flip_vertical {
            first_filters.push("vflip".into());
        }
        first_filters.push("setsar=1".into());
        graph.filters.push(format!(
            "[{}]{}[fx_base]",
            base.label,
            first_filters.join(",")
        ));
        let mut current = "fx_base".to_owned();
        if composition_count > 0 {
            match options.composition.layout {
                CompositionLayout::Horizontal | CompositionLayout::Vertical => {
                    graph
                        .filters
                        .push(format!("[{current}]{}[fx_tile0]", fitted(width, height)));
                    let mut labels = "[fx_tile0]".to_owned();
                    for n in 0..composition_count {
                        graph.filters.push(format!("[{}:V:0]settb=AVTB,setpts=PTS-STARTPTS,{},tpad=stop_mode=clone:stop_duration={duration:.6},trim=duration={duration:.6}[fx_tile{}]",video_input_start+n,fitted(width,height),n+1));
                        labels.push_str(&format!("[fx_tile{}]", n + 1));
                    }
                    let filter = if options.composition.layout == CompositionLayout::Horizontal {
                        width *= (composition_count + 1) as u32;
                        "hstack"
                    } else {
                        height *= (composition_count + 1) as u32;
                        "vstack"
                    };
                    graph.filters.push(format!(
                        "{labels}{filter}=inputs={}:shortest=1[fx_comp]",
                        composition_count + 1
                    ));
                    current = "fx_comp".into();
                }
                CompositionLayout::PictureInPicture => {
                    let (small_w, small_h) = (even(width / 3), even(height / 3));
                    for n in 0..composition_count {
                        graph.filters.push(format!("[{}:V:0]settb=AVTB,setpts=PTS-STARTPTS,{},tpad=stop_mode=clone:stop_duration={duration:.6},trim=duration={duration:.6}[fx_pip{}]",video_input_start+n,fitted(small_w,small_h),n));
                        let position = [
                            OverlayPosition::BottomRight,
                            OverlayPosition::TopRight,
                            OverlayPosition::BottomLeft,
                        ][n];
                        let (x, y) = position.coordinates(16);
                        let next = format!("fx_comp{n}");
                        graph.filters.push(format!("[{current}][fx_pip{n}]overlay=x='{x}':y='{y}':shortest=1:eof_action=repeat[{next}]"));
                        current = next;
                    }
                }
                CompositionLayout::None => unreachable!(),
            }
        }
        if let Some(index) = watermark_input {
            let watermark = &options.watermark;
            let mark_width = even(width * watermark.width_percent / 100);
            // Limit tall images to the canvas height while retaining their aspect ratio.
            graph.filters.push(format!("[{index}:v:0]setpts=PTS-STARTPTS,scale={mark_width}:{height}:force_original_aspect_ratio=decrease,format=rgba,colorchannelmixer=aa={:.6}[fx_mark]",watermark.opacity));
            let (x, y) = watermark.position.coordinates(watermark.margin);
            graph.filters.push(format!("[{current}][fx_mark]overlay=x='{x}':y='{y}':shortest=1:eof_action=repeat[fx_watermark]"));
            current = "fx_watermark".into();
        }
        graph.video_label = Some(current);
    }
    if options.mix.enabled {
        let mut labels = Vec::new();
        if options.mix.include_source
            && let Some(base) = base_audio
        {
            graph.filters.push(format!("[{base}]asetpts=PTS-STARTPTS,volume={:.6},apad=whole_dur={duration:.6},atrim=duration={duration:.6}[fx_asource]",options.mix.source_gain));
            labels.push("[fx_asource]".to_owned());
        }
        for (n, track) in options.mix.tracks.iter().enumerate() {
            if track.offset_seconds >= duration {
                return Err(format!("第 {} 个混音素材开始时间超出导出时长。", n + 1));
            }
            let available = duration - track.offset_seconds;
            let length = if track.duration_seconds == 0.0 {
                available
            } else {
                track.duration_seconds.min(available)
            };
            let delay = (track.offset_seconds * 1000.0).round() as u64;
            graph.filters.push(format!("[{}:a:0]atrim=duration={length:.6},asetpts=PTS-STARTPTS,volume={:.6},adelay={delay}:all=1,apad=whole_dur={duration:.6},atrim=duration={duration:.6}[fx_a{n}]",audio_input_start+n,track.gain));
            labels.push(format!("[fx_a{n}]"));
        }
        if labels.is_empty() {
            return Err("混音需要原声音轨或附加音频。".into());
        }
        // Disabling normalization keeps volume controls literal. A peak limiter prevents clipping.
        graph.filters.push(format!("{}amix=inputs={}:duration=longest:dropout_transition=0:normalize=0,alimiter=limit=0.95:level=0:latency=1,atrim=duration={duration:.6}[fx_mix]",labels.join(""),labels.len()));
        graph.audio_label = Some("fx_mix".into());
    }
    Ok(graph)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectAction {
    ChooseWatermark,
    AddVideos,
    AddMixAudio,
}

fn path_label(ui: &mut egui::Ui, path: &std::path::Path) {
    let details_id = ui.make_persistent_id(("effect-file-details", path));
    let mut details = ui.data(|data| data.get_temp::<bool>(details_id).unwrap_or(false));
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("路径").clicked() {
                    details = !details;
                }
                ui.add(
                    egui::Label::new(path.file_name().unwrap_or_default().to_string_lossy())
                        .halign(egui::Align::Min)
                        .truncate()
                        .show_tooltip_when_elided(false),
                );
            });
        });
        if details {
            ui.label(path.to_string_lossy());
            if ui.small_button("复制路径").clicked() {
                ui.ctx().copy_text(path.to_string_lossy().into_owned());
            }
        }
    });
    ui.data_mut(|data| data.insert_temp(details_id, details));
}

/// Rendering only; file-dialog requests are returned to the app's asynchronous dialog worker.
pub fn edit_ui(
    ui: &mut egui::Ui,
    options: &mut EffectsOptions,
    show_video: bool,
    show_audio: bool,
) -> Option<EffectAction> {
    let mut action = None;
    let capture_effect = std::env::var_os("FRAMEFLOW_SCREENSHOT")
        .and_then(|_| std::env::var("FRAMEFLOW_CAPTURE_EFFECT").ok());
    if show_video {
        egui::CollapsingHeader::new("旋转与翻转")
            .icon(crate::theme::collapse_chevron)
            .default_open(capture_effect.as_deref() == Some("rotation"))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("旋转");
                    egui::ComboBox::from_id_salt("effects-rotation")
                        .icon(crate::theme::combo_chevron)
                        .width(160.0)
                        .selected_text(options.rotation.label())
                        .show_ui(ui, |ui| {
                            for rotation in [
                                Rotation::None,
                                Rotation::Clockwise90,
                                Rotation::HalfTurn,
                                Rotation::CounterClockwise90,
                            ] {
                                ui.selectable_value(
                                    &mut options.rotation,
                                    rotation,
                                    rotation.label(),
                                );
                            }
                        });
                });
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(&mut options.flip_horizontal, "水平翻转");
                    ui.checkbox(&mut options.flip_vertical, "垂直翻转");
                });
            });
        egui::CollapsingHeader::new("图片水印")
            .icon(crate::theme::collapse_chevron)
            .default_open(capture_effect.as_deref() == Some("watermark"))
            .show(ui, |ui| {
                ui.checkbox(&mut options.watermark.enabled, "启用水印");
                if options.watermark.enabled {
                    if ui.button("选择图片").clicked() {
                        action = Some(EffectAction::ChooseWatermark);
                    }
                    if let Some(path) = &options.watermark.path {
                        path_label(ui, path);
                    }
                    egui::Grid::new("watermark-properties")
                        .num_columns(2)
                        .spacing(egui::vec2(16.0, 12.0))
                        .show(ui, |ui| {
                            ui.label("位置");
                            egui::ComboBox::from_id_salt("effects-watermark-position")
                                .icon(crate::theme::combo_chevron)
                                .width(160.0)
                                .selected_text(options.watermark.position.label())
                                .show_ui(ui, |ui| {
                                    for position in [
                                        OverlayPosition::TopLeft,
                                        OverlayPosition::TopRight,
                                        OverlayPosition::BottomLeft,
                                        OverlayPosition::BottomRight,
                                        OverlayPosition::Center,
                                    ] {
                                        ui.selectable_value(
                                            &mut options.watermark.position,
                                            position,
                                            position.label(),
                                        );
                                    }
                                });
                            ui.end_row();
                            ui.label("相对宽度");
                            ui.add(
                                egui::Slider::new(&mut options.watermark.width_percent, 1..=100)
                                    .suffix("%"),
                            );
                            ui.end_row();
                            ui.label("不透明度");
                            let mut opacity = options.watermark.opacity * 100.0;
                            if ui
                                .add(egui::Slider::new(&mut opacity, 0.0..=100.0).suffix("%"))
                                .changed()
                            {
                                options.watermark.opacity = opacity / 100.0;
                            }
                            ui.end_row();
                            ui.label("边距");
                            ui.add(
                                egui::DragValue::new(&mut options.watermark.margin)
                                    .range(0..=2000)
                                    .suffix(" px"),
                            );
                            ui.end_row();
                        });
                }
            });
        egui::CollapsingHeader::new("画面合成")
            .icon(crate::theme::collapse_chevron)
            .default_open(capture_effect.as_deref() == Some("composition"))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("排列");
                    egui::ComboBox::from_id_salt("effects-composition")
                        .icon(crate::theme::combo_chevron)
                        .width(160.0)
                        .selected_text(options.composition.layout.label())
                        .show_ui(ui, |ui| {
                            for layout in [
                                CompositionLayout::None,
                                CompositionLayout::Horizontal,
                                CompositionLayout::Vertical,
                                CompositionLayout::PictureInPicture,
                            ] {
                                ui.selectable_value(
                                    &mut options.composition.layout,
                                    layout,
                                    layout.label(),
                                );
                            }
                        });
                });
                if options.composition.layout != CompositionLayout::None {
                    ui.small("最多添加 3 个视频；短片末帧停留。");
                    if ui
                        .add_enabled(
                            options.composition.files.len() < 3,
                            egui::Button::new("添加视频"),
                        )
                        .clicked()
                    {
                        action = Some(EffectAction::AddVideos);
                    }
                    let mut remove = None;
                    for (index, path) in options.composition.files.iter().enumerate() {
                        ui.horizontal(|ui| {
                            if ui.small_button("移除").clicked() {
                                remove = Some(index);
                            }
                            path_label(ui, path);
                        });
                    }
                    if let Some(index) = remove {
                        options.composition.files.remove(index);
                    }
                }
            });
    }
    if show_audio {
        egui::CollapsingHeader::new("音频混音")
            .icon(crate::theme::collapse_chevron)
            .default_open(capture_effect.as_deref() == Some("mix"))
            .show(ui, |ui| {
                ui.checkbox(&mut options.mix.enabled, "启用混音");
                if options.mix.enabled {
                    ui.checkbox(&mut options.mix.include_source, "混入所选音轨");
                    if options.mix.include_source {
                        ui.add(
                            egui::Slider::new(&mut options.mix.source_gain, 0.0..=4.0)
                                .text("源音轨音量"),
                        );
                    }
                    if ui
                        .add_enabled(options.mix.tracks.len() < 8, egui::Button::new("添加音频"))
                        .clicked()
                    {
                        action = Some(EffectAction::AddMixAudio);
                    }
                    ui.small("开始时间相对导出片段；时长 0 表示至结束。");
                    let mut remove = None;
                    for (index, track) in options.mix.tracks.iter_mut().enumerate() {
                        ui.push_id(index, |ui| {
                            ui.separator();
                            ui.horizontal(|ui| {
                                if ui.small_button("移除").clicked() {
                                    remove = Some(index);
                                }
                                path_label(ui, &track.path);
                            });
                            ui.add(egui::Slider::new(&mut track.gain, 0.0..=4.0).text("音量"));
                            ui.horizontal_wrapped(|ui| {
                                ui.label("开始");
                                ui.add(
                                    egui::DragValue::new(&mut track.offset_seconds)
                                        .speed(0.1)
                                        .range(0.0..=864000.0)
                                        .fixed_decimals(3)
                                        .suffix(" 秒"),
                                );
                                ui.label("时长");
                                ui.add(
                                    egui::DragValue::new(&mut track.duration_seconds)
                                        .speed(0.1)
                                        .range(0.0..=864000.0)
                                        .fixed_decimals(3)
                                        .suffix(" 秒"),
                                );
                            });
                        });
                    }
                    if let Some(index) = remove {
                        options.mix.tracks.remove(index);
                    }
                }
            });
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::Path,
        process::{Command, Output, Stdio},
        sync::atomic::{AtomicU64, Ordering},
    };

    struct Fixture {
        directory: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let directory = std::env::temp_dir().join(format!(
                "frameflow-effects-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&directory).unwrap();
            Self { directory }
        }
        fn image(&self, name: &str, color: Option<[u8; 3]>) -> PathBuf {
            let path = self.directory.join(name);
            let mut bytes = b"P6\n320 240\n255\n".to_vec();
            for y in 0..240 {
                for x in 0..320 {
                    let pixel = color.unwrap_or(match (x < 160, y < 120) {
                        (true, true) => [255, 0, 0],
                        (false, true) => [0, 255, 0],
                        (true, false) => [0, 0, 255],
                        (false, false) => [255, 255, 0],
                    });
                    bytes.extend(pixel);
                }
            }
            fs::write(&path, bytes).unwrap();
            path
        }
        fn audio(&self, name: &str, sample: i16) -> PathBuf {
            let path = self.directory.join(name);
            let size = 8000_u32 * 2;
            let mut bytes = b"RIFF".to_vec();
            bytes.extend((36 + size).to_le_bytes());
            bytes.extend(b"WAVEfmt ");
            bytes.extend(16_u32.to_le_bytes());
            bytes.extend(1_u16.to_le_bytes());
            bytes.extend(1_u16.to_le_bytes());
            bytes.extend(8000_u32.to_le_bytes());
            bytes.extend(16000_u32.to_le_bytes());
            bytes.extend(2_u16.to_le_bytes());
            bytes.extend(16_u16.to_le_bytes());
            bytes.extend(b"data");
            bytes.extend(size.to_le_bytes());
            for _ in 0..8000 {
                bytes.extend(sample.to_le_bytes());
            }
            fs::write(&path, bytes).unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    fn ffmpeg() -> Option<PathBuf> {
        let candidates = [
            PathBuf::from("tools/bin/ffmpeg.exe"),
            PathBuf::from("ffmpeg"),
        ];
        for candidate in candidates {
            let mut command = Command::new(&candidate);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x08000000);
            }
            if command
                .arg("-version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
            {
                return Some(candidate);
            }
        }
        assert!(
            std::env::var_os("FRAMEFLOW_REQUIRE_GPU").is_none(),
            "Release effects QA requires FFmpeg"
        );
        None
    }

    fn execute(ffmpeg: &Path, args: &[OsString]) -> Output {
        let mut command = Command::new(ffmpeg);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let output = command
            .args(["-hide_banner", "-loglevel", "error", "-nostdin"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "FFmpeg failed: {}\n{args:?}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn video_pixels(ffmpeg: &Path, source: &Path, options: &EffectsOptions) -> Vec<u8> {
        let graph = build_graph(
            options,
            Some(VideoBase {
                label: "0:V:0",
                width: 320,
                height: 240,
            }),
            None,
            1,
            1.0,
        )
        .unwrap();
        let mut args = vec![OsString::from("-i"), source.as_os_str().to_owned()];
        for input in inputs(options) {
            append_input_args(&input, &mut args);
        }
        args.extend([
            OsString::from("-filter_complex"),
            graph.filters.join(";").into(),
            "-map".into(),
            format!("[{}]", graph.video_label.unwrap()).into(),
        ]);
        args.extend(
            [
                "-frames:v",
                "1",
                "-pix_fmt",
                "rgb24",
                "-f",
                "rawvideo",
                "pipe:1",
            ]
            .map(OsString::from),
        );
        execute(ffmpeg, &args).stdout
    }

    fn pixel(pixels: &[u8], width: usize, x: usize, y: usize) -> [u8; 3] {
        pixels[(y * width + x) * 3..(y * width + x) * 3 + 3]
            .try_into()
            .unwrap()
    }
    fn close_color(actual: [u8; 3], expected: [u8; 3]) {
        for n in 0..3 {
            assert!(
                (actual[n] as i16 - expected[n] as i16).abs() < 12,
                "{actual:?} != {expected:?}"
            );
        }
    }

    #[test]
    fn effects_defaults_are_passthrough_and_local_paths_do_not_persist() {
        let mut options = EffectsOptions::default();
        let graph = build_graph(
            &options,
            Some(VideoBase {
                label: "0:V:0",
                width: 320,
                height: 240,
            }),
            Some("0:a:0"),
            1,
            1.0,
        )
        .unwrap();
        assert!(graph.filters.is_empty());
        assert_eq!(graph.video_label.as_deref(), Some("0:V:0"));
        assert_eq!(graph.audio_label.as_deref(), Some("0:a:0"));
        options.watermark.path = Some(PathBuf::from("private.png"));
        options.watermark.enabled = true;
        options.watermark.opacity = 0.5;
        options.rotation = Rotation::Clockwise90;
        options.composition.layout = CompositionLayout::Horizontal;
        options.composition.files.push(PathBuf::from("private.mp4"));
        options
            .mix
            .tracks
            .push(MixTrack::new(PathBuf::from("private.wav")));
        options.mix.enabled = true;
        let json = serde_json::to_string(&options).unwrap();
        assert!(!json.contains("private"));
        let restored: EffectsOptions = serde_json::from_str(&json).unwrap();
        assert!(restored.watermark.path.is_none());
        assert!(restored.composition.files.is_empty());
        assert!(restored.mix.tracks.is_empty());
        assert!(!restored.watermark.enabled);
        assert_eq!(restored.composition.layout, CompositionLayout::None);
        assert!(!restored.mix.enabled);
        assert_eq!(restored.watermark.opacity, 0.5);
        assert_eq!(restored.rotation, Rotation::Clockwise90);
        assert!(validate(&restored).is_ok());
    }

    #[test]
    fn effects_reject_missing_files_invalid_values_and_injected_labels() {
        let mut options = EffectsOptions::default();
        options.watermark.enabled = true;
        assert!(validate(&options).is_err());
        options.watermark.enabled = false;
        options.mix.enabled = true;
        options.mix.source_gain = f64::NAN;
        assert!(validate(&options).is_err());
        options.mix.source_gain = 1.0;
        assert!(build_graph(&options, None, None, 1, 1.0).is_err());
        options.mix.enabled = false;
        assert!(build_graph(&options, None, Some("0:a];anull[evil"), 1, 1.0).is_err());
    }

    #[test]
    fn live_ffmpeg_rotation_flip_watermark_pixels_when_installed() {
        let Some(ffmpeg) = ffmpeg() else {
            return;
        };
        let fixture = Fixture::new();
        let source = fixture.image("quadrants.ppm", None);
        for (rotation, horizontal, vertical, width, expected) in [
            (Rotation::Clockwise90, false, false, 240, [0, 0, 255]),
            (Rotation::CounterClockwise90, false, false, 240, [0, 255, 0]),
            (Rotation::HalfTurn, false, false, 320, [255, 255, 0]),
            (Rotation::None, true, false, 320, [0, 255, 0]),
            (Rotation::None, false, true, 320, [0, 0, 255]),
        ] {
            let options = EffectsOptions {
                rotation,
                flip_horizontal: horizontal,
                flip_vertical: vertical,
                ..Default::default()
            };
            let pixels = video_pixels(&ffmpeg, &source, &options);
            assert_eq!(pixels.len(), 320 * 240 * 3);
            close_color(pixel(&pixels, width, 20, 20), expected);
        }
        let mut options = EffectsOptions::default();
        options.watermark.enabled = true;
        options.watermark.path = Some(fixture.image("水印 ' [x].ppm", Some([0, 0, 255])));
        options.watermark.position = OverlayPosition::TopLeft;
        options.watermark.margin = 0;
        options.watermark.width_percent = 25;
        options.watermark.opacity = 0.5;
        let pixels = video_pixels(&ffmpeg, &source, &options);
        close_color(pixel(&pixels, 320, 20, 20), [128, 0, 128]);
        close_color(pixel(&pixels, 320, 120, 20), [255, 0, 0]);
    }

    #[test]
    fn live_ffmpeg_multi_input_composition_pixels_when_installed() {
        let Some(ffmpeg) = ffmpeg() else {
            return;
        };
        let fixture = Fixture::new();
        let source = fixture.image("main.ppm", Some([255, 0, 0]));
        let mut options = EffectsOptions::default();
        options.composition.files = vec![
            fixture.image("second.ppm", Some([0, 255, 0])),
            fixture.image("third.ppm", Some([0, 0, 255])),
        ];
        for layout in [
            CompositionLayout::Horizontal,
            CompositionLayout::Vertical,
            CompositionLayout::PictureInPicture,
        ] {
            options.composition.layout = layout;
            let pixels = video_pixels(&ffmpeg, &source, &options);
            match layout {
                CompositionLayout::Horizontal => {
                    assert_eq!(pixels.len(), 960 * 240 * 3);
                    close_color(pixel(&pixels, 960, 20, 20), [255, 0, 0]);
                    close_color(pixel(&pixels, 960, 340, 20), [0, 255, 0]);
                    close_color(pixel(&pixels, 960, 660, 20), [0, 0, 255]);
                }
                CompositionLayout::Vertical => {
                    assert_eq!(pixels.len(), 320 * 720 * 3);
                    close_color(pixel(&pixels, 320, 20, 20), [255, 0, 0]);
                    close_color(pixel(&pixels, 320, 20, 260), [0, 255, 0]);
                    close_color(pixel(&pixels, 320, 20, 500), [0, 0, 255]);
                }
                CompositionLayout::PictureInPicture => {
                    assert_eq!(pixels.len(), 320 * 240 * 3);
                    close_color(pixel(&pixels, 320, 20, 20), [255, 0, 0]);
                    close_color(pixel(&pixels, 320, 240, 180), [0, 255, 0]);
                    close_color(pixel(&pixels, 320, 240, 40), [0, 0, 255]);
                }
                CompositionLayout::None => unreachable!(),
            }
        }
    }

    #[test]
    fn live_ffmpeg_mix_gain_offsets_duration_and_silent_source_when_installed() {
        let Some(ffmpeg) = ffmpeg() else {
            return;
        };
        let fixture = Fixture::new();
        let source = fixture.audio("main.wav", 3277);
        let mut options = EffectsOptions::default();
        options.mix.enabled = true;
        options.mix.source_gain = 0.5;
        options.mix.tracks = vec![
            MixTrack {
                path: fixture.audio("mix1.wav", 6554),
                gain: 1.0,
                offset_seconds: 0.25,
                duration_seconds: 0.25,
            },
            MixTrack {
                path: fixture.audio("mix2.wav", 3277),
                gain: 2.0,
                offset_seconds: 0.5,
                duration_seconds: 0.25,
            },
        ];
        for base_audio in [Some("0:a:0"), None] {
            let graph = build_graph(&options, None, base_audio, 1, 1.0).unwrap();
            let mut args = vec![OsString::from("-i"), source.as_os_str().to_owned()];
            for input in inputs(&options) {
                append_input_args(&input, &mut args);
            }
            args.extend([
                OsString::from("-filter_complex"),
                graph.filters.join(";").into(),
                "-map".into(),
                "[fx_mix]".into(),
            ]);
            args.extend(
                [
                    "-t",
                    "1",
                    "-ar",
                    "8000",
                    "-ac",
                    "1",
                    "-c:a",
                    "pcm_f32le",
                    "-f",
                    "f32le",
                    "pipe:1",
                ]
                .map(OsString::from),
            );
            let raw = execute(&ffmpeg, &args).stdout;
            let samples: Vec<f32> = raw
                .as_chunks::<4>()
                .0
                .iter()
                .copied()
                .map(f32::from_le_bytes)
                .collect();
            assert_eq!(samples.len(), 8000);
            let base = if base_audio.is_some() { 0.05 } else { 0.0 };
            for (index, expected) in [
                (800, base),
                (2800, base + 0.2),
                (4800, base + 0.2),
                (7200, base),
            ] {
                assert!(
                    (samples[index] - expected).abs() < 0.002,
                    "sample {index}: {} != {expected}",
                    samples[index]
                );
            }
        }
        options.mix.tracks[0].offset_seconds = 1.0;
        assert!(build_graph(&options, None, Some("0:a:0"), 1, 1.0).is_err());
    }

    #[test]
    fn live_ffmpeg_plan_two_pass_effects_mix_and_external_subtitle_when_installed() {
        let Some(ffmpeg) = ffmpeg() else {
            return;
        };
        let fixture = Fixture::new();
        let main = fixture.image("main.ppm", None);
        let audio = fixture.audio("source.wav", 3277);
        let source = fixture.directory.join("source.mkv");
        execute(
            &ffmpeg,
            &[
                "-loop".into(),
                "1".into(),
                "-i".into(),
                main.into_os_string(),
                "-i".into(),
                audio.into_os_string(),
                "-t".into(),
                "1".into(),
                "-c:v".into(),
                "libx264".into(),
                "-pix_fmt".into(),
                "yuv420p".into(),
                "-c:a".into(),
                "pcm_s16le".into(),
                source.as_os_str().to_owned(),
            ],
        );
        let tool_dir = if ffmpeg.is_absolute() {
            ffmpeg.parent().map(Path::to_path_buf)
        } else {
            None
        };
        let tools = crate::media::discover(tool_dir.as_deref()).unwrap();
        let media = crate::media::probe(&tools, &source).unwrap();
        let subtitle = fixture.directory.join("caption.srt");
        fs::write(
            &subtitle,
            "1\n00:00:00,000 --> 00:00:00,400\nFilter integration\n",
        )
        .unwrap();
        let mut settings = crate::media::ExportSettings {
            operation: crate::media::Operation::Trim,
            format: "mkv".into(),
            start_seconds: 0.2,
            end_seconds: 0.8,
            resolution: 160,
            subtitle_files: vec![subtitle],
            ..Default::default()
        };
        settings.encoding.video_rate_mode = crate::encoding::VideoRateMode::Bitrate;
        settings.encoding.video_bitrate = 500;
        settings.encoding.two_pass = true;
        settings.effects.rotation = Rotation::Clockwise90;
        settings.effects.flip_horizontal = true;
        settings.effects.watermark.enabled = true;
        settings.effects.watermark.path = Some(fixture.image("watermark.ppm", Some([0, 0, 255])));
        settings.effects.composition.layout = CompositionLayout::Horizontal;
        settings.effects.composition.files = vec![fixture.image("second.ppm", Some([0, 255, 0]))];
        settings.effects.mix.enabled = true;
        settings.effects.mix.tracks = vec![MixTrack {
            path: fixture.audio("extra.wav", 3277),
            gain: 0.5,
            offset_seconds: 0.1,
            duration_seconds: 0.2,
        }];
        let plan = crate::media::plan(&tools, &media, &settings).unwrap();
        assert!(plan.first_pass.is_some());
        let output = crate::media::run(
            plan,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            |_| {},
        )
        .unwrap();
        let info = crate::media::probe(&tools, &output).unwrap();
        assert_eq!((info.width, info.height), (240, 160));
        assert_eq!(info.video_codec.as_deref(), Some("h264"));
        assert_eq!(info.audio_codec.as_deref(), Some("aac"));
        assert!((info.duration - 0.6).abs() < 0.2, "{info:?}");
        let mut probe = Command::new(&tools.ffprobe);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            probe.creation_flags(0x08000000);
        }
        let packets = probe
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_packets",
                "-of",
                "json",
            ])
            .arg(&output)
            .output()
            .unwrap();
        assert!(packets.status.success());
        let data: serde_json::Value = serde_json::from_slice(&packets.stdout).unwrap();
        let packets = data["packets"].as_array().unwrap();
        assert_eq!(packets.len(), 15);
        let first = packets
            .iter()
            .map(|packet| packet["pts_time"].as_str().unwrap().parse::<f64>().unwrap())
            .fold(f64::INFINITY, f64::min);
        let end = packets
            .iter()
            .map(|packet| {
                packet["pts_time"].as_str().unwrap().parse::<f64>().unwrap()
                    + packet["duration_time"]
                        .as_str()
                        .unwrap()
                        .parse::<f64>()
                        .unwrap()
            })
            .fold(0.0, f64::max);
        assert!(
            first.abs() < 0.001,
            "video starts at {first}, subtitle seek must not shift it"
        );
        assert!((end - 0.6).abs() < 0.001, "video ends at {end}");
        assert!(
            info.tracks
                .iter()
                .any(|track| track.kind == crate::tracks::TrackKind::Subtitle)
        );
        assert!(!fs::read_dir(&fixture.directory).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("passlog")
        }));
        // Switching workflows must retain visual effects, drop audio-only graph branches,
        // and reindex inputs after removing the external subtitle input.
        for (operation, format, codec) in [
            (crate::media::Operation::Gif, "gif", "gif"),
            (crate::media::Operation::Snapshot, "png", "png"),
        ] {
            settings.operation = operation;
            settings.format = format.into();
            settings.fps = 10;
            let plan = crate::media::plan(&tools, &media, &settings).unwrap();
            assert!(plan.first_pass.is_none());
            let output = crate::media::run(
                plan,
                std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                |_| {},
            )
            .unwrap();
            let info = crate::media::probe(&tools, &output).unwrap();
            assert_eq!((info.width, info.height), (240, 160));
            assert_eq!(info.video_codec.as_deref(), Some(codec));
            assert!(info.audio_codec.is_none());
        }
    }
}
