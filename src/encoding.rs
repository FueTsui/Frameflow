//! Advanced encoding options. Every option is passed as an argument, never a shell string.
use crate::media::{ExportSettings, Operation, VideoCodec, VideoEncoder};
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::{ffi::OsString, path::Path};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioRateMode {
    #[default]
    Bitrate,
    Quality,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoRateMode {
    #[default]
    Quality,
    Bitrate,
    TargetSize,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum HardwareDecode {
    #[default]
    None,
    Cuda,
    D3d11va,
    Qsv,
}
impl HardwareDecode {
    fn label(self) -> &'static str {
        match self {
            Self::None => "CPU",
            Self::Cuda => "NVIDIA CUDA",
            Self::D3d11va => "Direct3D 11",
            Self::Qsv => "Intel QSV",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct EncodingOptions {
    /// Zero preserves the source rate/channel count; zero bit depth uses codec default.
    pub audio_sample_rate: u32,
    pub audio_channels: u32,
    pub audio_bit_depth: u8,
    pub audio_rate_mode: AudioRateMode,
    /// Higher is better; converted to each encoder's native quality scale.
    pub audio_quality: u8,
    pub video_rate_mode: VideoRateMode,
    pub video_bitrate: u32,
    /// Decimal MB. Reserve 2% for the container, then subtract every output audio track.
    pub target_size_mb: f64,
    pub two_pass: bool,
    pub hardware_decode: HardwareDecode,
    /// Empty = automatic. NVIDIA uses CUDA/NVENC indices; Intel/AMD use DXGI indices.
    pub gpu_device: String,
    pub gpu_scale: bool,
}
impl Default for EncodingOptions {
    fn default() -> Self {
        Self {
            audio_sample_rate: 0,
            audio_channels: 0,
            audio_bit_depth: 0,
            audio_rate_mode: AudioRateMode::Bitrate,
            audio_quality: 5,
            video_rate_mode: VideoRateMode::Quality,
            video_bitrate: 4000,
            target_size_mb: 100.0,
            two_pass: false,
            hardware_decode: HardwareDecode::None,
            gpu_device: String::new(),
            gpu_scale: false,
        }
    }
}

fn video_operation(settings: &ExportSettings) -> bool {
    matches!(
        settings.operation,
        Operation::Convert | Operation::Compress | Operation::Trim
    )
}
fn push(args: &mut Vec<OsString>, values: &[&str]) {
    args.extend(values.iter().map(OsString::from));
}
fn device(settings: &ExportSettings) -> Result<Option<u32>, String> {
    let value = settings.encoding.gpu_device.trim();
    if value.is_empty() {
        Ok(None)
    } else {
        value
            .parse::<u32>()
            .ok()
            .filter(|n| *n <= 63)
            .map(Some)
            .ok_or_else(|| "显卡编号应为 0–63，留空自动选择。".into())
    }
}
fn audio_lossless(format: &str) -> bool {
    matches!(format, "wav" | "flac" | "alac" | "tta")
}
fn effective_audio_format(settings: &ExportSettings) -> &str {
    if settings.operation == Operation::Audio {
        &settings.format
    } else if settings.format == "webm" {
        "video-opus"
    } else {
        "video-aac"
    }
}

pub fn validate(
    settings: &ExportSettings,
    duration: f64,
    audio_track_count: usize,
) -> Result<(), String> {
    let options = &settings.encoding;
    if settings.operation == Operation::Audio || (video_operation(settings) && !settings.muted) {
        audio_args(settings, effective_audio_format(settings))?;
    }
    if !video_operation(settings) {
        return Ok(());
    }
    if options.video_rate_mode != VideoRateMode::Quality {
        video_bitrate(settings, duration, audio_track_count)?;
    }
    if options.two_pass {
        if settings.video_encoder != VideoEncoder::Cpu {
            return Err("两遍编码需要选择 CPU。".into());
        }
        if options.video_rate_mode == VideoRateMode::Quality {
            return Err("两遍编码需要目标码率或目标体积模式。".into());
        }
    }
    if settings.format == "webm" && settings.video_encoder != VideoEncoder::Cpu {
        return Err("WebM 使用 CPU / VP9 编码。".into());
    }
    let selected = device(settings)?;
    if selected.is_some()
        && settings.video_encoder == VideoEncoder::Cpu
        && options.hardware_decode == HardwareDecode::None
    {
        return Err("指定显卡前，请启用 GPU 编码或硬件解码。".into());
    }
    match (settings.video_encoder, options.hardware_decode) {
        (VideoEncoder::Nvidia, HardwareDecode::Qsv)
        | (VideoEncoder::Intel, HardwareDecode::Cuda)
        | (VideoEncoder::Amd, HardwareDecode::Cuda | HardwareDecode::Qsv) => {
            return Err("硬件解码与所选编码器厂商不兼容。".into());
        }
        (VideoEncoder::Nvidia, HardwareDecode::D3d11va) if selected.is_some() => {
            return Err("NVIDIA 指定显卡时请选 CUDA 解码，或关闭硬件解码。".into());
        }
        _ => {}
    }
    if options.gpu_scale {
        if settings.video_encoder != VideoEncoder::Nvidia || settings.format == "webm" {
            return Err("GPU 缩放需要 NVIDIA 编码。".into());
        }
        if settings.resolution == 0 {
            return Err("GPU 缩放需要指定输出高度。".into());
        }
    }
    Ok(())
}

pub fn video_bitrate(
    settings: &ExportSettings,
    duration: f64,
    audio_track_count: usize,
) -> Result<u32, String> {
    let options = &settings.encoding;
    if options.video_rate_mode != VideoRateMode::TargetSize {
        return (32..=1_000_000)
            .contains(&options.video_bitrate)
            .then_some(options.video_bitrate)
            .ok_or_else(|| "视频码率应为 32–1000000 kbps。".into());
    }
    if !duration.is_finite() || duration <= 0.0 {
        return Err("目标体积需要有效的媒体时长。".into());
    }
    if !options.target_size_mb.is_finite()
        || options.target_size_mb <= 0.0
        || options.target_size_mb > 1_000_000.0
    {
        return Err("目标体积应大于 0 且不超过 1000000 MB。".into());
    }
    if !settings.muted && audio_track_count > 0 && options.audio_rate_mode == AudioRateMode::Quality
    {
        return Err("目标体积模式需要将音频设为码率模式。".into());
    }
    let audio = if settings.muted {
        0.0
    } else {
        f64::from(settings.audio_bitrate) * audio_track_count as f64
    };
    let bitrate = options.target_size_mb * 8000.0 * 0.98 / duration - audio;
    if !(32.0..=1_000_000.0).contains(&bitrate) {
        return Err("目标体积过小或过大；请调整体积、时长或音频码率。".into());
    }
    Ok(bitrate.floor() as u32)
}

pub fn video_args(
    settings: &ExportSettings,
    duration: f64,
    audio_track_count: usize,
) -> Result<Vec<OsString>, String> {
    validate(settings, duration, audio_track_count)?;
    let mut args = Vec::new();
    let webm = settings.format == "webm";
    let codec = if webm {
        "libvpx-vp9"
    } else {
        match (settings.video_encoder, settings.video_codec) {
            (VideoEncoder::Cpu, VideoCodec::H264) => "libx264",
            (VideoEncoder::Cpu, VideoCodec::H265) => "libx265",
            (VideoEncoder::Nvidia, VideoCodec::H264) => "h264_nvenc",
            (VideoEncoder::Nvidia, VideoCodec::H265) => "hevc_nvenc",
            (VideoEncoder::Intel, VideoCodec::H264) => "h264_qsv",
            (VideoEncoder::Intel, VideoCodec::H265) => "hevc_qsv",
            (VideoEncoder::Amd, VideoCodec::H264) => "h264_amf",
            (VideoEncoder::Amd, VideoCodec::H265) => "hevc_amf",
        }
    };
    push(&mut args, &["-c:v", codec]);
    if webm {
        push(&mut args, &["-deadline", "good", "-cpu-used", "4"]);
    } else {
        match settings.video_encoder {
            VideoEncoder::Cpu | VideoEncoder::Intel => push(&mut args, &["-preset", "medium"]),
            VideoEncoder::Nvidia => push(&mut args, &["-preset", "p4"]),
            VideoEncoder::Amd => push(&mut args, &["-quality", "balanced"]),
        }
    }
    if settings.encoding.video_rate_mode == VideoRateMode::Quality {
        let quality = settings.quality.to_string();
        if webm {
            push(&mut args, &["-crf", &quality, "-b:v", "0"]);
        } else {
            match settings.video_encoder {
                VideoEncoder::Cpu => push(&mut args, &["-crf", &quality]),
                VideoEncoder::Nvidia => {
                    push(&mut args, &["-rc", "vbr", "-cq", &quality, "-b:v", "0"])
                }
                VideoEncoder::Intel => push(&mut args, &["-global_quality", &quality]),
                VideoEncoder::Amd => push(
                    &mut args,
                    &["-rc", "cqp", "-qp_i", &quality, "-qp_p", &quality],
                ),
            }
        }
    } else {
        let rate = video_bitrate(settings, duration, audio_track_count)?;
        push(&mut args, &["-b:v", &format!("{rate}k")]);
        if !webm {
            match settings.video_encoder {
                VideoEncoder::Nvidia => push(&mut args, &["-rc", "vbr"]),
                VideoEncoder::Amd => push(
                    &mut args,
                    &[
                        "-rc",
                        "vbr_peak",
                        "-maxrate",
                        &format!("{rate}k"),
                        "-bufsize",
                        &format!("{}k", rate * 2),
                    ],
                ),
                _ => {}
            }
        }
    }
    if settings.video_encoder == VideoEncoder::Nvidia
        && let Some(index) = device(settings)?
    {
        push(&mut args, &["-gpu", &index.to_string()]);
    }
    let pixel_format = if encoder_upload_filter(settings)?.is_some() {
        if settings.video_encoder == VideoEncoder::Intel {
            "qsv"
        } else {
            "d3d11"
        }
    } else {
        "yuv420p"
    };
    push(&mut args, &["-pix_fmt", pixel_format]);
    if matches!(settings.format.as_str(), "mp4" | "mov") {
        push(&mut args, &["-movflags", "+faststart"]);
        if settings.video_codec == VideoCodec::H265 {
            push(&mut args, &["-tag:v", "hvc1"]);
        }
    }
    Ok(args)
}

pub fn audio_args(settings: &ExportSettings, format: &str) -> Result<Vec<OsString>, String> {
    let options = &settings.encoding;
    if ![
        0, 8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000, 88200, 96000, 192000,
    ]
    .contains(&options.audio_sample_rate)
        || ![0, 1, 2, 6, 8].contains(&options.audio_channels)
        || ![0, 16, 24, 32].contains(&options.audio_bit_depth)
    {
        return Err("音频采样率、声道或位深无效。".into());
    }
    if options.audio_quality > 10 {
        return Err("音频质量应为 0–10。".into());
    }
    let lossless = audio_lossless(format);
    if !lossless && !(32..=320).contains(&settings.audio_bitrate) {
        return Err("音频码率应为 32–320 kbps。".into());
    }
    if format == "mp3" && (options.audio_channels > 2 || options.audio_sample_rate > 48000) {
        return Err("MP3 最多支持双声道与 48 kHz。".into());
    }
    if format == "ac3"
        && (options.audio_channels > 6
            || ![0, 32000, 44100, 48000].contains(&options.audio_sample_rate))
    {
        return Err("AC3 支持最多 6 声道及 32 / 44.1 / 48 kHz。".into());
    }
    if format == "video-opus"
        && ![0, 8000, 12000, 16000, 24000, 48000].contains(&options.audio_sample_rate)
    {
        return Err("Opus 采样率请选择 8 / 12 / 16 / 24 / 48 kHz。".into());
    }
    if matches!(format, "aac" | "m4a" | "video-aac") && options.audio_sample_rate > 96000 {
        return Err("AAC 最高支持 96 kHz。".into());
    }
    if matches!(format, "alac" | "tta") && options.audio_bit_depth == 32 {
        return Err("ALAC / TTA 位深请选择 16 或 24 位。".into());
    }
    if format == "ac3" && options.audio_rate_mode == AudioRateMode::Quality {
        return Err("AC3 不支持质量模式，请选择码率。".into());
    }
    let codec = match format {
        "mp3" => "libmp3lame",
        "wav" => match options.audio_bit_depth {
            24 => "pcm_s24le",
            32 => "pcm_s32le",
            _ => "pcm_s16le",
        },
        "flac" => "flac",
        "alac" => "alac",
        "tta" => "tta",
        "ogg" => "libvorbis",
        "aac" | "m4a" | "video-aac" => "aac",
        "video-opus" => "libopus",
        "ac3" => "ac3",
        _ => return Err("不支持的音频编码格式。".into()),
    };
    let mut args = Vec::new();
    push(&mut args, &["-c:a", codec]);
    if !lossless {
        match options.audio_rate_mode {
            AudioRateMode::Bitrate => {
                push(
                    &mut args,
                    &["-b:a", &format!("{}k", settings.audio_bitrate)],
                );
                if format == "video-opus" {
                    push(&mut args, &["-vbr", "off"]);
                }
            }
            AudioRateMode::Quality => {
                let quality = f64::from(options.audio_quality);
                match format {
                    "mp3" => push(
                        &mut args,
                        &["-q:a", &(9.0 - quality * 0.9).round().to_string()],
                    ),
                    "ogg" => push(&mut args, &["-q:a", &quality.to_string()]),
                    "video-opus" => push(
                        &mut args,
                        &[
                            "-b:a",
                            &format!("{}k", 24 + u32::from(options.audio_quality) * 24),
                            "-vbr",
                            "on",
                        ],
                    ),
                    _ => push(&mut args, &["-q:a", &format!("{:.1}", 0.1 + quality * 0.4)]),
                }
            }
        }
    }
    if options.audio_sample_rate != 0 {
        push(&mut args, &["-ar", &options.audio_sample_rate.to_string()]);
    }
    if options.audio_channels != 0 {
        push(&mut args, &["-ac", &options.audio_channels.to_string()]);
    }
    if lossless && format != "wav" && options.audio_bit_depth != 0 {
        let sample_format = match (format, options.audio_bit_depth) {
            ("alac", 16) => "s16p",
            ("alac", _) => "s32p",
            (_, 16) => "s16",
            _ => "s32",
        };
        push(
            &mut args,
            &[
                "-sample_fmt",
                sample_format,
                "-bits_per_raw_sample",
                &options.audio_bit_depth.to_string(),
            ],
        );
    }
    if matches!(format, "m4a" | "alac") {
        push(&mut args, &["-movflags", "+faststart"]);
    }
    Ok(args)
}

/// Must precede the main input's `-i`. Downloaded NV12 frames keep CPU editing available.
pub fn input_args(settings: &ExportSettings) -> Result<Vec<OsString>, String> {
    let mut args = Vec::new();
    if !video_operation(settings) {
        return Ok(args);
    }
    let options = &settings.encoding;
    let selected = device(settings)?;
    let explicit_upload = selected.is_some()
        && matches!(
            settings.video_encoder,
            VideoEncoder::Intel | VideoEncoder::Amd
        );
    if explicit_upload || options.hardware_decode == HardwareDecode::Qsv {
        let kind = if settings.video_encoder == VideoEncoder::Amd {
            "d3d11va=ff_device"
        } else {
            "qsv=ff_device:hw"
        };
        let init = if kind.starts_with("qsv") {
            if let Some(index) = selected {
                format!("{kind},child_device={index},child_device_type=d3d11va")
            } else {
                format!("{kind},child_device_type=d3d11va")
            }
        } else {
            format!("{kind}:{}", selected.unwrap_or(0))
        };
        push(
            &mut args,
            &["-init_hw_device", &init, "-filter_hw_device", "ff_device"],
        );
    }
    if options.hardware_decode != HardwareDecode::None {
        let method = match options.hardware_decode {
            HardwareDecode::Cuda => "cuda",
            HardwareDecode::Qsv => "qsv",
            _ => "d3d11va",
        };
        push(
            &mut args,
            &["-hwaccel", method, "-hwaccel_output_format", "nv12"],
        );
        if options.hardware_decode == HardwareDecode::Qsv
            || (explicit_upload && settings.video_encoder == VideoEncoder::Amd)
        {
            push(&mut args, &["-hwaccel_device", "ff_device"]);
        } else if let Some(index) = selected {
            push(&mut args, &["-hwaccel_device", &index.to_string()]);
        }
    }
    Ok(args)
}

/// Replace CPU scaling with this stage; do all other software filters before it.
pub fn gpu_scale_filter(settings: &ExportSettings) -> Result<Option<String>, String> {
    if !video_operation(settings) || !settings.encoding.gpu_scale {
        return Ok(None);
    }
    if settings.video_encoder != VideoEncoder::Nvidia || settings.resolution == 0 {
        return Err("GPU 缩放需要 NVIDIA 编码及指定输出高度。".into());
    }
    let height = settings.resolution.div_ceil(2) * 2;
    let index = device(settings)?.unwrap_or(0);
    Ok(Some(format!(
        "format=nv12,hwupload_cuda=device={index},scale_cuda=w=-2:h={height}:format=nv12,hwdownload,format=nv12"
    )))
}

/// Append last, after all software filters. This binds AMF/QSV to the selected device.
pub fn encoder_upload_filter(settings: &ExportSettings) -> Result<Option<String>, String> {
    if video_operation(settings)
        && device(settings)?.is_some()
        && matches!(
            settings.video_encoder,
            VideoEncoder::Intel | VideoEncoder::Amd
        )
    {
        Ok(Some("format=nv12,hwupload=extra_hw_frames=64".into()))
    } else {
        Ok(None)
    }
}

pub fn two_pass(settings: &ExportSettings) -> bool {
    video_operation(settings)
        && settings.video_encoder == VideoEncoder::Cpu
        && settings.encoding.two_pass
        && settings.encoding.video_rate_mode != VideoRateMode::Quality
}

pub fn pass_log_value(settings: &ExportSettings, log: &Path) -> String {
    if settings.video_codec == VideoCodec::H265 && settings.format != "webm" {
        escape_x265_path(log)
    } else {
        log.to_string_lossy().into_owned()
    }
}

pub fn escape_x265_path(log: &Path) -> String {
    log.to_string_lossy()
        .replace('\\', "/")
        .replace(':', "\\:")
        .replace('\'', "\\'")
}

pub fn pass_args(pass: u8, log: &Path, settings: &ExportSettings) -> Vec<OsString> {
    let mut args = Vec::new();
    if settings.video_codec == VideoCodec::H265 && settings.format != "webm" {
        push(
            &mut args,
            &[
                "-x265-params",
                &format!("pass={pass}:stats={}", pass_log_value(settings, log)),
            ],
        );
    } else {
        push(&mut args, &["-pass", &pass.to_string(), "-passlogfile"]);
        args.push(log.as_os_str().to_owned());
    }
    args
}

fn setting_row(ui: &mut egui::Ui, label: &str, controls: impl FnOnce(&mut egui::Ui)) {
    if ui.available_width() < 400.0 {
        ui.label(label);
        controls(ui);
    } else {
        ui.horizontal_top(|ui| {
            ui.add_sized(
                [120.0, 32.0],
                egui::Label::new(label).halign(egui::Align::LEFT),
            );
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), 0.0),
                egui::Layout::top_down(egui::Align::LEFT),
                controls,
            );
        });
    }
}

pub fn video_rate_ui(ui: &mut egui::Ui, settings: &mut ExportSettings) {
    normalize_ui_options(settings);
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        let options = &mut settings.encoding;
        setting_row(ui, "控制方式", |ui| {
            egui::ComboBox::from_id_salt("advanced_video_rate")
                .icon(crate::theme::combo_chevron)
                .width(ui.available_width().min(220.0))
                .selected_text(match options.video_rate_mode {
                    VideoRateMode::Quality => "质量",
                    VideoRateMode::Bitrate => "目标码率",
                    VideoRateMode::TargetSize => "目标体积",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut options.video_rate_mode,
                        VideoRateMode::Quality,
                        "质量",
                    );
                    ui.selectable_value(
                        &mut options.video_rate_mode,
                        VideoRateMode::Bitrate,
                        "目标码率",
                    );
                    ui.selectable_value(
                        &mut options.video_rate_mode,
                        VideoRateMode::TargetSize,
                        "目标体积",
                    );
                });
        });
        match options.video_rate_mode {
            VideoRateMode::Quality => {
                setting_row(ui, "画质", |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.selectable_value(&mut settings.quality, 18, "高清");
                        ui.selectable_value(&mut settings.quality, 23, "均衡");
                        ui.selectable_value(&mut settings.quality, 28, "小体积");
                    });
                    ui.spacing_mut().slider_width =
                        (ui.available_width() - 68.0).clamp(80.0, 240.0);
                    ui.add(
                        egui::Slider::new(&mut settings.quality, 16..=35)
                            .trailing_fill(true)
                            .handle_shape(egui::style::HandleShape::Circle),
                    );
                });
            }
            VideoRateMode::Bitrate => {
                setting_row(ui, "目标码率", |ui| {
                    ui.add(
                        egui::DragValue::new(&mut options.video_bitrate)
                            .range(32..=1_000_000)
                            .suffix(" kbps"),
                    );
                });
            }
            VideoRateMode::TargetSize => {
                setting_row(ui, "目标体积", |ui| {
                    ui.add(
                        egui::DragValue::new(&mut options.target_size_mb)
                            .range(0.01..=1_000_000.0)
                            .speed(0.5)
                            .suffix(" MB"),
                    );
                });
            }
        }
        if options.video_rate_mode != VideoRateMode::Quality
            && settings.video_encoder == VideoEncoder::Cpu
        {
            setting_row(ui, "两遍编码", |ui| {
                ui.checkbox(&mut options.two_pass, "启用");
            });
        }
    });
    normalize_ui_options(settings);
}

fn decode_supported(encoder: VideoEncoder, decode: HardwareDecode, explicit_device: bool) -> bool {
    match decode {
        HardwareDecode::None => true,
        HardwareDecode::Cuda => matches!(encoder, VideoEncoder::Cpu | VideoEncoder::Nvidia),
        HardwareDecode::D3d11va => encoder != VideoEncoder::Nvidia || !explicit_device,
        HardwareDecode::Qsv => matches!(encoder, VideoEncoder::Cpu | VideoEncoder::Intel),
    }
}

pub fn hardware_ui(ui: &mut egui::Ui, settings: &mut ExportSettings) {
    normalize_ui_options(settings);
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        let options = &mut settings.encoding;
        setting_row(ui, "硬件解码", |ui| {
            egui::ComboBox::from_id_salt("hardware_decode")
                .icon(crate::theme::combo_chevron)
                .width(ui.available_width().min(220.0))
                .selected_text(options.hardware_decode.label())
                .show_ui(ui, |ui| {
                    for method in [
                        HardwareDecode::None,
                        HardwareDecode::Cuda,
                        HardwareDecode::D3d11va,
                        HardwareDecode::Qsv,
                    ] {
                        if decode_supported(
                            settings.video_encoder,
                            method,
                            !options.gpu_device.trim().is_empty(),
                        ) {
                            ui.selectable_value(
                                &mut options.hardware_decode,
                                method,
                                method.label(),
                            );
                        }
                    }
                });
        });
        if settings.video_encoder != VideoEncoder::Cpu
            || options.hardware_decode != HardwareDecode::None
        {
            setting_row(ui, "显卡编号", |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut options.gpu_device)
                        .desired_width(108.0)
                        .hint_text("自动"),
                );
            });
        }
        if settings.video_encoder == VideoEncoder::Nvidia {
            setting_row(ui, "GPU 缩放", |ui| {
                ui.add_enabled(
                    settings.resolution != 0,
                    egui::Checkbox::new(&mut options.gpu_scale, "启用"),
                );
            });
        }
    });
    normalize_ui_options(settings);
}

fn audio_sample_rates(format: &str) -> &'static [u32] {
    match format {
        "mp3" => &[
            0, 8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000,
        ],
        "ac3" => &[0, 32000, 44100, 48000],
        "video-opus" => &[0, 8000, 12000, 16000, 24000, 48000],
        "aac" | "m4a" | "video-aac" => &[
            0, 8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000, 88200, 96000,
        ],
        _ => &[
            0, 8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000, 88200, 96000, 192000,
        ],
    }
}

fn normalize_audio_ui(settings: &mut ExportSettings, format: &str) {
    let is_video = video_operation(settings);
    let options = &mut settings.encoding;
    if !audio_sample_rates(format).contains(&options.audio_sample_rate) {
        options.audio_sample_rate = 0;
    }
    let max_channels = match format {
        "mp3" => 2,
        "ac3" => 6,
        _ => 8,
    };
    if options.audio_channels > max_channels || ![0, 1, 2, 6, 8].contains(&options.audio_channels) {
        options.audio_channels = max_channels;
    }
    if matches!(format, "alac" | "tta") && options.audio_bit_depth == 32 {
        options.audio_bit_depth = 24;
    }
    if ![0, 16, 24, 32].contains(&options.audio_bit_depth) {
        options.audio_bit_depth = 0;
    }
    let fixed_bitrate =
        format == "ac3" || (is_video && options.video_rate_mode == VideoRateMode::TargetSize);
    if fixed_bitrate {
        options.audio_rate_mode = AudioRateMode::Bitrate;
    }
    options.audio_quality = options.audio_quality.min(10);
    settings.audio_bitrate = settings
        .audio_bitrate
        .clamp(if format == "ogg" { 64 } else { 32 }, 320);
    if format == "ac3" {
        settings.audio_bitrate = [
            32_u32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
        ]
        .into_iter()
        .min_by_key(|rate| rate.abs_diff(settings.audio_bitrate))
        .unwrap_or(192);
    }
}

/// Normalize the active export profile independently of whether its controls are expanded.
pub fn normalize_ui_options(settings: &mut ExportSettings) {
    let reencode_video = video_operation(settings)
        && !(settings.operation == Operation::Trim && settings.lossless_trim);
    if !reencode_video && settings.operation != Operation::Audio {
        return;
    }
    if reencode_video {
        if settings.format == "webm" {
            settings.video_encoder = VideoEncoder::Cpu;
        }
        let options = &mut settings.encoding;
        if settings.video_encoder != VideoEncoder::Cpu
            || options.video_rate_mode == VideoRateMode::Quality
        {
            options.two_pass = false;
        }
        if !decode_supported(
            settings.video_encoder,
            options.hardware_decode,
            !options.gpu_device.trim().is_empty(),
        ) {
            options.hardware_decode = HardwareDecode::None;
        }
        if settings.video_encoder == VideoEncoder::Cpu
            && options.hardware_decode == HardwareDecode::None
        {
            options.gpu_device.clear();
        }
        if settings.video_encoder != VideoEncoder::Nvidia || settings.resolution == 0 {
            options.gpu_scale = false;
        }
    }
    let format = effective_audio_format(settings).to_owned();
    normalize_audio_ui(settings, &format);
}

pub fn audio_ui(ui: &mut egui::Ui, settings: &mut ExportSettings) {
    normalize_ui_options(settings);
    let format = effective_audio_format(settings).to_owned();
    let is_video = video_operation(settings);
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        let options = &mut settings.encoding;
        if !audio_lossless(&format) {
            let fixed_bitrate = format == "ac3"
                || (is_video && options.video_rate_mode == VideoRateMode::TargetSize);
            if !fixed_bitrate {
                setting_row(ui, "控制方式", |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.selectable_value(
                            &mut options.audio_rate_mode,
                            AudioRateMode::Bitrate,
                            "码率",
                        );
                        ui.selectable_value(
                            &mut options.audio_rate_mode,
                            AudioRateMode::Quality,
                            "质量 / VBR",
                        );
                    });
                });
            }
            if options.audio_rate_mode == AudioRateMode::Quality {
                setting_row(ui, "音频质量", |ui| {
                    ui.spacing_mut().slider_width =
                        (ui.available_width() - 68.0).clamp(80.0, 240.0);
                    ui.add(egui::Slider::new(&mut options.audio_quality, 0..=10));
                });
            } else {
                setting_row(ui, "音频码率", |ui| {
                    egui::ComboBox::from_id_salt("audio-bitrate")
                        .icon(crate::theme::combo_chevron)
                        .width(ui.available_width().min(220.0))
                        .selected_text(format!("{} kbps", settings.audio_bitrate))
                        .show_ui(ui, |ui| {
                            let rates: &[u32] = if format == "ac3" {
                                &[
                                    32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
                                ]
                            } else {
                                &[32, 64, 96, 128, 160, 192, 256, 320]
                            };
                            for &rate in rates {
                                if format != "ogg" || rate >= 64 {
                                    ui.selectable_value(
                                        &mut settings.audio_bitrate,
                                        rate,
                                        format!("{rate} kbps"),
                                    );
                                }
                            }
                        });
                });
            }
        }
        setting_row(ui, "采样率", |ui| {
            egui::ComboBox::from_id_salt("audio_sample_rate")
                .icon(crate::theme::combo_chevron)
                .width(ui.available_width().min(220.0))
                .selected_text(if options.audio_sample_rate == 0 {
                    "源采样率".into()
                } else {
                    format!("{} Hz", options.audio_sample_rate)
                })
                .show_ui(ui, |ui| {
                    for &rate in audio_sample_rates(&format) {
                        ui.selectable_value(
                            &mut options.audio_sample_rate,
                            rate,
                            if rate == 0 {
                                "源采样率".into()
                            } else {
                                format!("{rate} Hz")
                            },
                        );
                    }
                });
        });
        setting_row(ui, "声道", |ui| {
            egui::ComboBox::from_id_salt("audio_channels")
                .icon(crate::theme::combo_chevron)
                .width(ui.available_width().min(220.0))
                .selected_text(match options.audio_channels {
                    0 => "源声道",
                    1 => "单声道",
                    2 => "双声道",
                    6 => "5.1",
                    _ => "7.1",
                })
                .show_ui(ui, |ui| {
                    for (value, label) in [
                        (0, "源声道"),
                        (1, "单声道"),
                        (2, "双声道"),
                        (6, "5.1"),
                        (8, "7.1"),
                    ] {
                        if (format != "mp3" || value <= 2) && (format != "ac3" || value <= 6) {
                            ui.selectable_value(&mut options.audio_channels, value, label);
                        }
                    }
                });
        });
        if audio_lossless(&format) {
            setting_row(ui, "音频位深", |ui| {
                egui::ComboBox::from_id_salt("audio_depth")
                    .icon(crate::theme::combo_chevron)
                    .width(ui.available_width().min(220.0))
                    .selected_text(if options.audio_bit_depth == 0 {
                        "自动".into()
                    } else {
                        format!("{} 位", options.audio_bit_depth)
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut options.audio_bit_depth, 0, "自动");
                        for depth in [16, 24, 32] {
                            if depth == 32 && matches!(format.as_str(), "alac" | "tta") {
                                continue;
                            }
                            ui.selectable_value(
                                &mut options.audio_bit_depth,
                                depth,
                                format!("{depth} 位"),
                            );
                        }
                    });
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        process::{Command, Stdio},
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
    };
    fn strings(args: Vec<OsString>) -> Vec<String> {
        args.into_iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect()
    }
    fn has(args: &[String], key: &str, value: &str) -> bool {
        args.windows(2).any(|pair| pair == [key, value])
    }
    #[test]
    fn audio_ui_format_changes_keep_exportable_parameters() {
        for format in [
            "mp3",
            "ac3",
            "aac",
            "m4a",
            "video-aac",
            "video-opus",
            "alac",
            "tta",
            "wav",
            "flac",
            "ogg",
        ] {
            let mut settings = ExportSettings {
                operation: Operation::Audio,
                format: format.into(),
                audio_bitrate: 33,
                ..Default::default()
            };
            settings.encoding.audio_sample_rate = 192000;
            settings.encoding.audio_channels = 8;
            settings.encoding.audio_bit_depth = 32;
            settings.encoding.audio_rate_mode = AudioRateMode::Quality;
            normalize_ui_options(&mut settings);
            assert!(audio_args(&settings, format).is_ok(), "{format}");
            if format == "ac3" {
                assert_eq!(settings.encoding.audio_rate_mode, AudioRateMode::Bitrate);
                assert_eq!(settings.audio_bitrate, 32);
                assert_eq!(settings.encoding.audio_channels, 6);
            }
            if format == "mp3" {
                assert_eq!(settings.encoding.audio_channels, 2);
            }
            if matches!(format, "alac" | "tta") {
                assert_eq!(settings.encoding.audio_bit_depth, 24);
            }
            if format == "ogg" {
                assert_eq!(settings.audio_bitrate, 64);
            }
        }
    }
    #[test]
    fn audio_ui_target_size_limits_video_audio_without_changing_audio_exports() {
        let mut settings = ExportSettings::default();
        settings.encoding.video_rate_mode = VideoRateMode::TargetSize;
        settings.encoding.audio_rate_mode = AudioRateMode::Quality;
        normalize_ui_options(&mut settings);
        assert_eq!(settings.encoding.audio_rate_mode, AudioRateMode::Bitrate);
        assert!(validate(&settings, 60.0, 1).is_ok());

        settings.operation = Operation::Audio;
        settings.format = "mp3".into();
        settings.encoding.audio_rate_mode = AudioRateMode::Quality;
        normalize_ui_options(&mut settings);
        assert_eq!(settings.encoding.audio_rate_mode, AudioRateMode::Quality);
        assert!(audio_args(&settings, "mp3").is_ok());
    }
    #[test]
    fn closed_hardware_controls_normalize_processor_and_resolution_changes() {
        let mut settings = ExportSettings {
            video_encoder: VideoEncoder::Nvidia,
            resolution: 1080,
            ..Default::default()
        };
        settings.encoding.hardware_decode = HardwareDecode::Cuda;
        settings.encoding.gpu_device = "0".into();
        settings.encoding.gpu_scale = true;
        settings.encoding.two_pass = true;
        normalize_ui_options(&mut settings);
        assert!(settings.encoding.gpu_scale);
        assert_eq!(settings.encoding.hardware_decode, HardwareDecode::Cuda);
        assert_eq!(settings.encoding.gpu_device, "0");
        assert!(!settings.encoding.two_pass);
        assert!(validate(&settings, 60.0, 1).is_ok());

        settings.video_encoder = VideoEncoder::Cpu;
        normalize_ui_options(&mut settings);
        assert!(!settings.encoding.gpu_scale);
        assert_eq!(settings.encoding.gpu_device, "0");
        assert_eq!(settings.encoding.hardware_decode, HardwareDecode::Cuda);
        settings.encoding.hardware_decode = HardwareDecode::None;
        normalize_ui_options(&mut settings);
        assert!(settings.encoding.gpu_device.is_empty());
        assert!(validate(&settings, 60.0, 1).is_ok());

        settings.video_encoder = VideoEncoder::Intel;
        settings.encoding.hardware_decode = HardwareDecode::Cuda;
        settings.encoding.gpu_device = "1".into();
        settings.encoding.gpu_scale = true;
        settings.encoding.video_rate_mode = VideoRateMode::Bitrate;
        settings.encoding.two_pass = true;
        normalize_ui_options(&mut settings);
        assert_eq!(settings.encoding.hardware_decode, HardwareDecode::None);
        assert_eq!(settings.encoding.gpu_device, "1");
        assert!(!settings.encoding.gpu_scale);
        assert!(!settings.encoding.two_pass);
        assert!(validate(&settings, 60.0, 1).is_ok());

        settings.video_encoder = VideoEncoder::Nvidia;
        settings.resolution = 0;
        settings.encoding.gpu_scale = true;
        normalize_ui_options(&mut settings);
        assert!(!settings.encoding.gpu_scale);
    }
    #[test]
    fn closed_video_controls_normalize_quality_and_webm_transitions() {
        let mut settings = ExportSettings::default();
        settings.encoding.two_pass = true;
        normalize_ui_options(&mut settings);
        assert!(!settings.encoding.two_pass);
        settings.encoding.video_rate_mode = VideoRateMode::Bitrate;
        settings.encoding.two_pass = true;
        normalize_ui_options(&mut settings);
        assert!(settings.encoding.two_pass);
        settings.format = "webm".into();
        settings.video_encoder = VideoEncoder::Nvidia;
        settings.encoding.gpu_device = "0".into();
        settings.encoding.gpu_scale = true;
        settings.encoding.audio_sample_rate = 44100;
        normalize_ui_options(&mut settings);
        assert_eq!(settings.video_encoder, VideoEncoder::Cpu);
        assert!(settings.encoding.gpu_device.is_empty());
        assert!(!settings.encoding.gpu_scale);
        assert_eq!(settings.encoding.audio_sample_rate, 0);
        assert!(validate(&settings, 60.0, 1).is_ok());
    }
    #[test]
    fn normalization_preserves_parameters_unused_by_the_active_operation() {
        for operation in [
            Operation::Remux,
            Operation::Gif,
            Operation::Snapshot,
            Operation::Subtitle,
            Operation::Trim,
        ] {
            let mut settings = ExportSettings {
                operation,
                lossless_trim: true,
                video_encoder: VideoEncoder::Intel,
                format: "mp4".into(),
                audio_bitrate: 7,
                ..Default::default()
            };
            settings.encoding.hardware_decode = HardwareDecode::Cuda;
            settings.encoding.gpu_device = "2".into();
            settings.encoding.gpu_scale = true;
            settings.encoding.two_pass = true;
            settings.encoding.audio_sample_rate = 192000;
            settings.encoding.audio_rate_mode = AudioRateMode::Quality;
            settings.encoding.video_rate_mode = VideoRateMode::TargetSize;
            let before = serde_json::to_value(&settings).unwrap();
            normalize_ui_options(&mut settings);
            assert_eq!(
                serde_json::to_value(&settings).unwrap(),
                before,
                "{operation:?}"
            );
        }

        let mut settings = ExportSettings {
            operation: Operation::Audio,
            format: "mp3".into(),
            video_encoder: VideoEncoder::Intel,
            ..Default::default()
        };
        settings.encoding.hardware_decode = HardwareDecode::Cuda;
        settings.encoding.gpu_device = "2".into();
        settings.encoding.gpu_scale = true;
        settings.encoding.two_pass = true;
        settings.encoding.audio_sample_rate = 192000;
        normalize_ui_options(&mut settings);
        assert_eq!(settings.encoding.audio_sample_rate, 0);
        assert_eq!(settings.encoding.hardware_decode, HardwareDecode::Cuda);
        assert_eq!(settings.encoding.gpu_device, "2");
        assert!(settings.encoding.gpu_scale);
        assert!(settings.encoding.two_pass);
        assert_eq!(settings.video_encoder, VideoEncoder::Intel);
    }
    #[test]
    fn target_size_subtracts_all_audio_and_rejects_unbounded_vbr() {
        let mut settings = ExportSettings::default();
        settings.encoding.video_rate_mode = VideoRateMode::TargetSize;
        settings.encoding.target_size_mb = 10.0;
        assert_eq!(video_bitrate(&settings, 100.0, 2).unwrap(), 400);
        settings.muted = true;
        assert_eq!(video_bitrate(&settings, 100.0, 2).unwrap(), 784);
        settings.muted = false;
        settings.encoding.audio_rate_mode = AudioRateMode::Quality;
        assert!(validate(&settings, 100.0, 2).is_err());
        settings.encoding.target_size_mb = f64::NAN;
        assert!(video_bitrate(&settings, 100.0, 0).is_err());
    }
    #[test]
    fn bitrate_and_quality_flags_are_exclusive_and_hardware_pass_is_rejected() {
        let mut settings = ExportSettings::default();
        let args = strings(video_args(&settings, 10.0, 1).unwrap());
        assert!(has(&args, "-crf", "23"));
        settings.encoding.video_rate_mode = VideoRateMode::Bitrate;
        settings.encoding.two_pass = true;
        let args = strings(video_args(&settings, 10.0, 1).unwrap());
        assert!(has(&args, "-b:v", "4000k"));
        assert!(!args.contains(&"-crf".into()));
        assert!(two_pass(&settings));
        settings.video_encoder = VideoEncoder::Nvidia;
        assert!(validate(&settings, 10.0, 1).is_err());
    }
    #[test]
    fn audio_depth_vbr_and_unsupported_format_constraints() {
        let mut settings = ExportSettings::default();
        settings.encoding.audio_bit_depth = 24;
        assert!(has(
            &strings(audio_args(&settings, "wav").unwrap()),
            "-c:a",
            "pcm_s24le"
        ));
        let args = strings(audio_args(&settings, "flac").unwrap());
        assert!(has(&args, "-bits_per_raw_sample", "24"));
        settings.encoding.audio_rate_mode = AudioRateMode::Quality;
        settings.encoding.audio_quality = 10;
        let args = strings(audio_args(&settings, "mp3").unwrap());
        assert!(has(&args, "-q:a", "0"));
        assert!(!args.contains(&"-b:a".into()));
        assert!(audio_args(&settings, "ac3").is_err());
        settings.encoding.audio_sample_rate = 96000;
        assert!(audio_args(&settings, "mp3").is_err());
    }
    #[test]
    fn explicit_devices_are_bound_to_encoder_and_decoder() {
        let mut settings = ExportSettings {
            video_encoder: VideoEncoder::Nvidia,
            resolution: 720,
            ..Default::default()
        };
        settings.encoding.gpu_device = "1".into();
        settings.encoding.hardware_decode = HardwareDecode::Cuda;
        settings.encoding.gpu_scale = true;
        assert!(has(
            &strings(video_args(&settings, 5.0, 0).unwrap()),
            "-gpu",
            "1"
        ));
        assert!(has(
            &strings(input_args(&settings).unwrap()),
            "-hwaccel_device",
            "1"
        ));
        assert!(
            gpu_scale_filter(&settings)
                .unwrap()
                .unwrap()
                .contains("hwupload_cuda=device=1")
        );
        settings.encoding.gpu_device = "0,extra=bad".into();
        assert!(input_args(&settings).is_err());
        settings.encoding.gpu_device = "1".into();
        settings.encoding.gpu_scale = false;
        settings.encoding.hardware_decode = HardwareDecode::None;
        settings.video_encoder = VideoEncoder::Intel;
        assert!(
            strings(input_args(&settings).unwrap())
                .iter()
                .any(|s| s.contains("child_device=1"))
        );
        assert!(encoder_upload_filter(&settings).unwrap().is_some());
        settings.video_encoder = VideoEncoder::Amd;
        assert!(has(
            &strings(input_args(&settings).unwrap()),
            "-init_hw_device",
            "d3d11va=ff_device:1"
        ));
    }

    #[test]
    fn qsv_automatic_device_does_not_force_dxgi_adapter_zero() {
        let mut settings = ExportSettings {
            video_encoder: VideoEncoder::Intel,
            ..Default::default()
        };
        settings.encoding.hardware_decode = HardwareDecode::Qsv;
        let args = strings(input_args(&settings).unwrap());
        assert!(has(
            &args,
            "-init_hw_device",
            "qsv=ff_device:hw,child_device_type=d3d11va"
        ));
        assert!(!args.iter().any(|arg| arg.contains("child_device=")));
        settings.encoding.gpu_device = "2".into();
        assert!(has(
            &strings(input_args(&settings).unwrap()),
            "-init_hw_device",
            "qsv=ff_device:hw,child_device=2,child_device_type=d3d11va"
        ));
    }
    #[test]
    fn old_preferences_receive_safe_defaults() {
        let settings: ExportSettings = serde_json::from_str("{\"format\":\"mp4\"}").unwrap();
        assert_eq!(settings.encoding.video_rate_mode, VideoRateMode::Quality);
        assert_eq!(settings.encoding.hardware_decode, HardwareDecode::None);
        assert!(!two_pass(&settings));
    }
    fn ffmpeg() -> Option<std::path::PathBuf> {
        crate::media::discover(None).ok().map(|tools| tools.ffmpeg)
    }
    fn temp_directory() -> std::path::PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "frameflow-encoding-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
    fn command(program: &Path) -> Command {
        let mut cmd = Command::new(program);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        cmd.stdin(Stdio::null());
        cmd
    }
    #[test]
    fn live_true_two_pass_h264_h265_when_ffmpeg_installed() {
        let Some(ffmpeg) = ffmpeg() else {
            return;
        };
        let directory = temp_directory();
        for codec in [VideoCodec::H264, VideoCodec::H265] {
            let mut settings = ExportSettings {
                video_codec: codec,
                muted: true,
                ..Default::default()
            };
            settings.encoding.video_rate_mode = VideoRateMode::Bitrate;
            settings.encoding.video_bitrate = 300;
            settings.encoding.two_pass = true;
            let log = directory.join(format!("{}-stats.log", codec.label()));
            for pass in [1, 2] {
                let mut cmd = command(&ffmpeg);
                cmd.args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc2=size=320x180:rate=25",
                    "-t",
                    "0.6",
                ]);
                cmd.args(video_args(&settings, 0.6, 0).unwrap());
                cmd.args(pass_args(pass, &log, &settings))
                    .args(["-an", "-f", "null", "-"]);
                let output = cmd.output().unwrap();
                assert!(
                    output.status.success(),
                    "{} pass {pass}: {}",
                    codec.label(),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn live_audio_24bit_mono_44100_when_ffmpeg_installed() {
        let Some(ffmpeg) = ffmpeg() else {
            return;
        };
        let directory = temp_directory();
        let target = directory.join("depth.wav");
        let mut settings = ExportSettings::default();
        settings.encoding.audio_bit_depth = 24;
        settings.encoding.audio_channels = 1;
        settings.encoding.audio_sample_rate = 44100;
        let output = command(&ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=1000:sample_rate=48000",
                "-t",
                "0.2",
            ])
            .args(audio_args(&settings, "wav").unwrap())
            .arg(&target)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let tools = crate::media::discover(None).unwrap();
        let probe = command(&tools.ffprobe)
            .args(["-v", "error", "-show_streams", "-of", "json"])
            .arg(target)
            .output()
            .unwrap();
        let data: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
        let stream = &data["streams"][0];
        assert_eq!(stream["bits_per_sample"], 24);
        assert_eq!(stream["sample_rate"], "44100");
        assert_eq!(stream["channels"], 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn live_cuda_decode_scaling_and_explicit_device_when_available() {
        let Some(ffmpeg) = ffmpeg() else {
            return;
        };
        // This capability probe distinguishes machines without NVENC from pipeline regressions.
        let probe = command(&ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=size=320x180:rate=25",
                "-frames:v",
                "1",
                "-c:v",
                "h264_nvenc",
                "-gpu",
                "0",
                "-f",
                "null",
                "-",
            ])
            .output()
            .unwrap();
        if !probe.status.success() {
            return;
        }
        let directory = temp_directory();
        let source = directory.join("input.mp4");
        let target = directory.join("gpu.mp4");
        let generated = command(&ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=640x360:rate=25",
                "-t",
                "0.6",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
            ])
            .arg(&source)
            .output()
            .unwrap();
        assert!(generated.status.success());
        let mut settings = ExportSettings {
            video_encoder: VideoEncoder::Nvidia,
            resolution: 240,
            muted: true,
            ..Default::default()
        };
        settings.encoding.hardware_decode = HardwareDecode::Cuda;
        settings.encoding.gpu_device = "0".into();
        settings.encoding.gpu_scale = true;
        let output = command(&ffmpeg)
            .args(["-hide_banner", "-loglevel", "error"])
            .args(input_args(&settings).unwrap())
            .arg("-i")
            .arg(&source)
            .arg("-vf")
            .arg(gpu_scale_filter(&settings).unwrap().unwrap())
            .args(video_args(&settings, 0.6, 0).unwrap())
            .arg(&target)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let tools = crate::media::discover(None).unwrap();
        let probe = command(&tools.ffprobe)
            .args(["-v", "error", "-show_streams", "-of", "json"])
            .arg(target)
            .output()
            .unwrap();
        let data: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
        assert_eq!(data["streams"][0]["codec_name"], "h264");
        assert_eq!(data["streams"][0]["height"], 240);
        fs::remove_dir_all(directory).unwrap();
    }

    struct IntegratedFixture {
        tools: crate::media::Toolchain,
        directory: std::path::PathBuf,
        media: crate::media::MediaInfo,
    }
    impl IntegratedFixture {
        fn new() -> Option<Self> {
            let tools = crate::media::discover(None).ok()?;
            let directory = temp_directory();
            let source = directory.join("source.mp4");
            let output = command(&tools.ffmpeg)
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc2=size=640x360:rate=30:duration=4",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:sample_rate=48000:duration=4",
                    "-c:v",
                    "libx264",
                    "-preset",
                    "ultrafast",
                    "-g",
                    "30",
                    "-keyint_min",
                    "30",
                    "-sc_threshold",
                    "0",
                    "-bf",
                    "0",
                    "-pix_fmt",
                    "yuv420p",
                    "-c:a",
                    "aac",
                    "-b:a",
                    "64k",
                    "-shortest",
                ])
                .arg(&source)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let media = crate::media::probe(&tools, &source).unwrap();
            Some(Self {
                tools,
                directory,
                media,
            })
        }
        fn assert_no_private_files(&self) {
            let leftovers: Vec<_> = fs::read_dir(&self.directory)
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .filter(|name| name.to_string_lossy().starts_with(".ffmpeg-studio-"))
                .collect();
            assert!(
                leftovers.is_empty(),
                "staging/log files left behind: {leftovers:?}"
            );
        }
    }
    impl Drop for IntegratedFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    #[test]
    fn live_plan_run_target_size_two_pass_h264_and_h265_when_installed() {
        let Some(fixture) = IntegratedFixture::new() else {
            return;
        };
        for codec in [VideoCodec::H264, VideoCodec::H265] {
            let mut settings = ExportSettings {
                video_codec: codec,
                audio_bitrate: 64,
                resolution: 180,
                output_dir: Some(fixture.directory.clone()),
                ..Default::default()
            };
            settings.encoding.video_rate_mode = VideoRateMode::TargetSize;
            settings.encoding.target_size_mb = 0.2;
            settings.encoding.two_pass = true;
            let plan = crate::media::plan(&fixture.tools, &fixture.media, &settings).unwrap();
            assert!(plan.first_pass.is_some());
            assert!(
                strings(plan.args.clone())
                    .iter()
                    .any(|arg| arg.contains("__FRAMEFLOW_PASSLOG__"))
            );
            let seen_second = Arc::new(AtomicBool::new(false));
            let second = seen_second.clone();
            let target = crate::media::run(plan, Arc::new(AtomicBool::new(false)), move |event| {
                if let crate::media::ProgressEvent::Log(message) = event
                    && message.starts_with("第 2 遍")
                {
                    second.store(true, Ordering::Relaxed);
                }
            })
            .unwrap_or_else(|error| panic!("{}: {error}", codec.label()));
            assert!(seen_second.load(Ordering::Relaxed));
            let exported = crate::media::probe(&fixture.tools, &target).unwrap();
            assert_eq!(
                exported.video_codec.as_deref(),
                Some(if codec == VideoCodec::H264 {
                    "h264"
                } else {
                    "hevc"
                })
            );
            assert_eq!(exported.audio_codec.as_deref(), Some("aac"));
            assert_eq!(exported.height, 180);
            assert!((exported.duration - 4.0).abs() < 0.2);
            let ratio = exported.size as f64 / 200_000.0;
            // Short clips and codec/container overhead prevent an exact byte guarantee.
            assert!(
                (0.6..=1.4).contains(&ratio),
                "{} target estimate: {} bytes / 200000",
                codec.label(),
                exported.size
            );
            fixture.assert_no_private_files();
        }
    }

    #[test]
    fn live_plan_run_cancel_after_first_pass_cleans_logs_and_preserves_files() {
        let Some(fixture) = IntegratedFixture::new() else {
            return;
        };
        let sentinel = fixture.directory.join("keep.txt");
        fs::write(&sentinel, b"unrelated data").unwrap();
        let mut settings = ExportSettings {
            video_codec: VideoCodec::H265,
            muted: true,
            resolution: 180,
            output_dir: Some(fixture.directory.clone()),
            ..Default::default()
        };
        settings.encoding.video_rate_mode = VideoRateMode::Bitrate;
        settings.encoding.video_bitrate = 300;
        settings.encoding.two_pass = true;
        let plan = crate::media::plan(&fixture.tools, &fixture.media, &settings).unwrap();
        let desired = plan.output.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let trigger = cancel.clone();
        let result = crate::media::run(plan, cancel.clone(), move |event| {
            if let crate::media::ProgressEvent::Log(message) = event
                && message.starts_with("第 2 遍")
            {
                trigger.store(true, Ordering::Relaxed);
            }
        });
        assert!(
            cancel.load(Ordering::Relaxed),
            "first pass must finish before cancellation"
        );
        assert!(result.unwrap_err().contains("取消"));
        assert!(!desired.exists());
        assert_eq!(fs::read(sentinel).unwrap(), b"unrelated data");
        assert!(fixture.media.path.is_file());
        fixture.assert_no_private_files();
    }

    #[test]
    fn live_plan_run_cuda_decode_scale_with_software_filter_when_available() {
        let Some(mut fixture) = IntegratedFixture::new() else {
            return;
        };
        let hardware = command(&fixture.tools.ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=size=320x180:rate=25",
                "-frames:v",
                "1",
                "-c:v",
                "h264_nvenc",
                "-gpu",
                "0",
                "-f",
                "null",
                "-",
            ])
            .output()
            .unwrap();
        if !hardware.status.success() {
            return;
        }
        fixture
            .tools
            .gpu_encoders
            .push((VideoEncoder::Nvidia, VideoCodec::H264));
        let mut settings = ExportSettings {
            video_encoder: VideoEncoder::Nvidia,
            resolution: 240,
            fps: 15,
            output_dir: Some(fixture.directory.clone()),
            ..Default::default()
        };
        settings.encoding.hardware_decode = HardwareDecode::Cuda;
        settings.encoding.gpu_device = "0".into();
        settings.encoding.gpu_scale = true;
        settings.effects.flip_horizontal = true;
        let plan = crate::media::plan(&fixture.tools, &fixture.media, &settings).unwrap();
        let args = strings(plan.args.clone());
        let graph = args
            .windows(2)
            .find(|pair| pair[0] == "-filter_complex")
            .unwrap()[1]
            .clone();
        assert!(
            graph.contains("hflip") && graph.contains("scale_cuda") && graph.contains("fps=15")
        );
        let target = crate::media::run(plan, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        let exported = crate::media::probe(&fixture.tools, &target).unwrap();
        assert_eq!(exported.height, 240);
        assert_eq!(exported.width, 426);
        assert_eq!(exported.video_codec.as_deref(), Some("h264"));
        assert_eq!(exported.audio_codec.as_deref(), Some("aac"));
        assert!((exported.duration - 4.0).abs() < 0.2);
        fixture.assert_no_private_files();
    }

    fn packet_hashes(tools: &crate::media::Toolchain, path: &Path) -> Vec<String> {
        let output = command(&tools.ffprobe)
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_packets",
                "-show_entries",
                "packet=data_hash",
                "-show_data_hash",
                "sha256",
                "-of",
                "json",
            ])
            .arg(path)
            .output()
            .unwrap();
        assert!(output.status.success());
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        value["packets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|packet| packet["data_hash"].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn live_lossless_trim_preserves_contiguous_encoded_packets() {
        let Some(fixture) = IntegratedFixture::new() else {
            return;
        };
        let settings = ExportSettings {
            operation: Operation::Trim,
            lossless_trim: true,
            start_seconds: 1.2,
            end_seconds: 2.4,
            muted: true,
            output_dir: Some(fixture.directory.clone()),
            ..Default::default()
        };
        let plan = crate::media::plan(&fixture.tools, &fixture.media, &settings).unwrap();
        let args = strings(plan.args.clone());
        assert!(has(&args, "-c:v", "copy") || has(&args, "-c", "copy"));
        assert!(plan.first_pass.is_none());
        let target = crate::media::run(plan, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        let source_packets = packet_hashes(&fixture.tools, &fixture.media.path);
        let output_packets = packet_hashes(&fixture.tools, &target);
        assert!(!output_packets.is_empty() && output_packets.len() < source_packets.len());
        assert!(
            source_packets
                .windows(output_packets.len())
                .any(|window| window == output_packets),
            "stream-copy trim changed encoded video packets"
        );
        let exported = crate::media::probe(&fixture.tools, &target).unwrap();
        assert_eq!(exported.width, fixture.media.width);
        assert_eq!(exported.height, fixture.media.height);
        assert!(exported.audio_codec.is_none());
        fixture.assert_no_private_files();
    }

    #[test]
    fn live_ogg_quality_and_silent_source_external_mix_reach_output() {
        let Some(fixture) = IntegratedFixture::new() else {
            return;
        };
        let silent = fixture.directory.join("silent.mp4");
        let copied = command(&fixture.tools.ffmpeg)
            .args(["-v", "error", "-i"])
            .arg(&fixture.media.path)
            .args(["-map", "0:v:0", "-c:v", "copy", "-an"])
            .arg(&silent)
            .output()
            .unwrap();
        assert!(copied.status.success());
        let source = crate::media::probe(&fixture.tools, &silent).unwrap();
        assert!(source.audio_codec.is_none());
        let mut settings = ExportSettings {
            operation: Operation::Audio,
            format: "ogg".into(),
            audio_bitrate: 32,
            output_dir: Some(fixture.directory.clone()),
            ..Default::default()
        };
        settings.encoding.audio_rate_mode = AudioRateMode::Quality;
        settings.tracks.audio = Some(Vec::new());
        settings.effects.mix.enabled = true;
        settings.effects.mix.tracks.push(crate::effects::MixTrack {
            path: fixture.media.path.clone(),
            gain: 0.5,
            offset_seconds: 0.0,
            duration_seconds: 0.0,
        });
        let plan = crate::media::plan(&fixture.tools, &source, &settings).unwrap();
        let args = strings(plan.args.clone());
        assert!(has(&args, "-q:a", "5"));
        assert!(!args.contains(&"-b:a".into()));
        let target = crate::media::run(plan, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        let output = crate::media::probe(&fixture.tools, &target).unwrap();
        assert_eq!(output.audio_codec.as_deref(), Some("vorbis"));
        assert!(output.video_codec.is_none());
        assert!((output.duration - 4.0).abs() < 0.2);
        fixture.assert_no_private_files();
    }
}
