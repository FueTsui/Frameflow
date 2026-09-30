//! FFmpeg execution boundary. Arguments are passed directly to the process, never a shell.
//! See https://ffmpeg.org/ffmpeg.html for option scope and progress protocol.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::VecDeque,
    env,
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operation {
    Convert,
    Compress,
    Trim,
    Audio,
    Gif,
    Snapshot,
    Remux,
    Subtitle,
}

impl Operation {
    pub const ALL: [Self; 8] = [
        Self::Convert,
        Self::Compress,
        Self::Trim,
        Self::Audio,
        Self::Gif,
        Self::Snapshot,
        Self::Remux,
        Self::Subtitle,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Convert => "格式转换",
            Self::Compress => "视频压缩",
            Self::Trim => "片段裁剪",
            Self::Audio => "音频处理",
            Self::Gif => "制作 GIF",
            Self::Snapshot => "截取画面",
            Self::Remux => "无损封装",
            Self::Subtitle => "字幕提取",
        }
    }

    pub fn formats(self) -> &'static [&'static str] {
        match self {
            Self::Convert | Self::Compress | Self::Trim | Self::Remux => {
                &["mp4", "mkv", "webm", "mov"]
            }
            Self::Audio => &[
                "mp3", "m4a", "aac", "ac3", "flac", "alac", "tta", "wav", "ogg",
            ],
            Self::Gif => &["gif"],
            Self::Snapshot => &["png", "jpg", "webp"],
            Self::Subtitle => &["srt", "ass", "vtt", "mks"],
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            Self::Convert => "converted",
            Self::Compress => "compressed",
            Self::Trim => "trimmed",
            Self::Audio => "audio",
            Self::Gif => "animation",
            Self::Snapshot => "frame",
            Self::Remux => "remuxed",
            Self::Subtitle => "subtitle",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoCodec {
    #[default]
    H264,
    H265,
}
impl VideoCodec {
    pub fn label(self) -> &'static str {
        match self {
            Self::H264 => "H.264",
            Self::H265 => "H.265",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoEncoder {
    #[default]
    Cpu,
    Nvidia,
    Intel,
    Amd,
}
impl VideoEncoder {
    pub const ALL: [Self; 4] = [Self::Cpu, Self::Nvidia, Self::Intel, Self::Amd];
    pub fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Nvidia => "GPU · NVIDIA",
            Self::Intel => "GPU · Intel",
            Self::Amd => "GPU · AMD",
        }
    }
    fn codec_name(self, codec: VideoCodec) -> &'static str {
        match (self, codec) {
            (Self::Cpu, VideoCodec::H264) => "libx264",
            (Self::Cpu, VideoCodec::H265) => "libx265",
            (Self::Nvidia, VideoCodec::H264) => "h264_nvenc",
            (Self::Nvidia, VideoCodec::H265) => "hevc_nvenc",
            (Self::Intel, VideoCodec::H264) => "h264_qsv",
            (Self::Intel, VideoCodec::H265) => "hevc_qsv",
            (Self::Amd, VideoCodec::H264) => "h264_amf",
            (Self::Amd, VideoCodec::H265) => "hevc_amf",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportSettings {
    pub operation: Operation,
    pub format: String,
    pub quality: u8,
    pub video_codec: VideoCodec,
    pub video_encoder: VideoEncoder,
    pub audio_bitrate: u32,
    pub encoding: crate::encoding::EncodingOptions,
    pub tracks: crate::tracks::TrackOptions,
    pub effects: crate::effects::EffectsOptions,
    pub lossless_trim: bool,
    pub resolution: u32,
    pub fps: u32,
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub muted: bool,
    pub output_dir: Option<PathBuf>,
    /// Explicit external subtitle tracks for Remux only; never persist local file selections.
    #[serde(skip)]
    pub subtitle_files: Vec<PathBuf>,
    #[serde(skip)]
    pub audio_files: Vec<PathBuf>,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            operation: Operation::Convert,
            format: "mp4".into(),
            quality: 23,
            video_codec: VideoCodec::H264,
            video_encoder: VideoEncoder::Cpu,
            audio_bitrate: 192,
            encoding: Default::default(),
            tracks: Default::default(),
            effects: Default::default(),
            lossless_trim: false,
            resolution: 0,
            fps: 0,
            start_seconds: 0.0,
            end_seconds: 0.0,
            muted: false,
            output_dir: None,
            subtitle_files: Vec::new(),
            audio_files: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Toolchain {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub version: String,
    pub gpu_encoders: Vec<(VideoEncoder, VideoCodec)>,
}

fn command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.stdin(Stdio::null());
    cmd
}

/// Test real encoder initialization, not merely the compiled encoder list.
/// Run only on the engine discovery worker. Each child is bounded and reaped.
pub fn detect_gpu_encoders(tools: &Toolchain) -> Vec<(VideoEncoder, VideoCodec)> {
    let mut available = Vec::new();
    for encoder in [VideoEncoder::Nvidia, VideoEncoder::Intel, VideoEncoder::Amd] {
        for codec in [VideoCodec::H264, VideoCodec::H265] {
            let settings = ExportSettings {
                video_encoder: encoder,
                video_codec: codec,
                ..Default::default()
            };
            let mut args = Vec::new();
            push(
                &mut args,
                &[
                    "-hide_banner",
                    "-nostdin",
                    "-loglevel",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "color=size=256x256:rate=30",
                    "-frames:v",
                    "3",
                ],
            );
            video_encoding(&mut args, "mkv", &settings);
            push(&mut args, &["-f", "null", "-"]);
            let Ok(mut child) = command(&tools.ffmpeg)
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            else {
                continue;
            };
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        if status.success() {
                            available.push((encoder, codec));
                        }
                        break;
                    }
                    Ok(None) if std::time::Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(25))
                    }
                    _ => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                }
            }
        }
    }
    available
}

pub fn video_settings_error(tools: &Toolchain, settings: &ExportSettings) -> Option<String> {
    if settings.operation == Operation::Trim && settings.lossless_trim {
        return None;
    }
    if !matches!(
        settings.operation,
        Operation::Convert | Operation::Compress | Operation::Trim
    ) {
        return None;
    }
    if settings.format.trim().eq_ignore_ascii_case("webm") {
        return (settings.video_encoder != VideoEncoder::Cpu)
            .then(|| "WebM 使用 CPU / VP9，请切换为 CPU 或 MP4、MKV、MOV。".into());
    }
    if settings.video_encoder != VideoEncoder::Cpu
        && !tools
            .gpu_encoders
            .contains(&(settings.video_encoder, settings.video_codec))
    {
        return Some(format!(
            "{} 的 {} 不可用，请选择 CPU 或重新检测引擎。",
            settings.video_encoder.label(),
            settings.video_codec.label()
        ));
    }
    None
}

/// Find both executables together; an explicitly selected folder takes priority.
pub fn discover(directory: Option<&Path>) -> Result<Toolchain, String> {
    let mut directories = Vec::new();
    if let Some(path) = directory {
        directories.push(path.to_path_buf());
        directories.push(path.join("bin"));
    }
    if let Some(path) = crate::updater::managed_engine_dir() {
        directories.push(path);
    }
    if let Ok(exe) = env::current_exe()
        && let Some(parent) = exe.parent()
    {
        directories.extend([
            parent.to_path_buf(),
            parent.join("tools"),
            parent.join("bin"),
            parent.join("tools/bin"),
            parent.join("../Resources/bin"),
            parent.join("../Resources/tools"),
        ]);
    }
    if let Ok(cwd) = env::current_dir() {
        directories.extend([cwd.join("tools/bin"), cwd.join("tools"), cwd.join("bin")]);
    }
    if let Some(paths) = env::var_os("PATH") {
        directories.extend(env::split_paths(&paths));
    }
    #[cfg(target_os = "macos")]
    directories.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ]);

    let mut seen = Vec::new();
    let mut errors = Vec::new();
    for directory in directories {
        if seen.contains(&directory) {
            continue;
        }
        seen.push(directory.clone());
        let ffmpeg = directory.join(if cfg!(windows) {
            "ffmpeg.exe"
        } else {
            "ffmpeg"
        });
        let ffprobe = directory.join(if cfg!(windows) {
            "ffprobe.exe"
        } else {
            "ffprobe"
        });
        if !ffmpeg.is_file() || !ffprobe.is_file() {
            continue;
        }
        let pair = (tool_version(&ffmpeg), tool_version(&ffprobe));
        match pair {
            (Ok(version), Ok(_)) => {
                return Ok(Toolchain {
                    ffmpeg: absolute_path(&ffmpeg)?,
                    ffprobe: absolute_path(&ffprobe)?,
                    version,
                    gpu_encoders: Vec::new(),
                });
            }
            (Err(error), _) | (_, Err(error)) => errors.push(error),
        }
    }
    let detail = errors.first().map(|s| format!("\n{s}")).unwrap_or_default();
    Err(format!(
        "未找到可用的 FFmpeg 和 FFprobe。请选择同时包含这两个程序的文件夹，或将它们加入 PATH。{detail}"
    ))
}

fn tool_version(path: &Path) -> Result<String, String> {
    crate::updater::executable_version(path)
}

#[derive(Clone, Debug)]
pub struct MediaInfo {
    pub path: PathBuf,
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub size: u64,
    pub tracks: Vec<crate::tracks::TrackInfo>,
}

pub fn probe(tools: &Toolchain, path: &Path) -> Result<MediaInfo, String> {
    let path = absolute_path(path)?;
    let metadata = fs::metadata(&path).map_err(|e| format!("无法读取文件：{e}"))?;
    if !metadata.is_file() {
        return Err("请选择媒体文件，不能使用文件夹。".into());
    }
    let output = command(&tools.ffprobe)
        .args([
            "-v",
            "error",
            "-show_format",
            "-show_streams",
            "-of",
            "json",
        ])
        .arg(&path)
        .output()
        .map_err(|e| format!("无法启动 FFprobe：{e}"))?;
    if !output.status.success() {
        return Err(format!("无法解析媒体：{}", tail_text(&output.stderr, 1600)));
    }
    parse_probe(&output.stdout, path, metadata.len())
}

fn parse_probe(bytes: &[u8], path: PathBuf, size: u64) -> Result<MediaInfo, String> {
    let data: Value =
        serde_json::from_slice(bytes).map_err(|e| format!("媒体信息格式错误：{e}"))?;
    let streams = data["streams"].as_array().ok_or("媒体中没有可识别的流。")?;
    let video = streams.iter().find(|s| {
        s["codec_type"] == "video" && s["disposition"]["attached_pic"].as_u64().unwrap_or(0) == 0
    });
    let audio = streams.iter().find(|s| s["codec_type"] == "audio");
    if video.is_none() && audio.is_none() && !streams.iter().any(|s| s["codec_type"] == "subtitle")
    {
        return Err("文件中没有可用的视频或音频。".into());
    }
    let duration = number(&data["format"]["duration"])
        .filter(|value| *value > 0.0)
        .unwrap_or_else(|| {
            streams
                .iter()
                .filter_map(|s| number(&s["duration"]))
                .fold(0.0, f64::max)
        });
    let (mut width, mut height) = video
        .map(|s| {
            (
                s["width"].as_u64().unwrap_or(0) as u32,
                s["height"].as_u64().unwrap_or(0) as u32,
            )
        })
        .unwrap_or((0, 0));
    if let Some(video) = video {
        let rotation = video["side_data_list"]
            .as_array()
            .and_then(|list| list.iter().find_map(|s| number(&s["rotation"])))
            .or_else(|| number(&video["tags"]["rotate"]))
            .unwrap_or(0.0);
        if ((rotation.abs() % 180.0) - 90.0).abs() < 1.0 {
            std::mem::swap(&mut width, &mut height);
        }
    }
    Ok(MediaInfo {
        path,
        duration: duration.max(0.0),
        width,
        height,
        video_codec: video.map(|s| s["codec_name"].as_str().unwrap_or("unknown").into()),
        audio_codec: audio.map(|s| s["codec_name"].as_str().unwrap_or("unknown").into()),
        size,
        tracks: crate::tracks::parse_tracks(&data),
    })
}

fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|n| n.is_finite())
}

#[derive(Clone, Debug)]
pub struct JobPlan {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub output: PathBuf,
    pub duration: f64,
    pub first_pass: Option<Vec<OsString>>,
}

/// Build a reviewable command without creating folders or modifying files.
pub fn plan(
    tools: &Toolchain,
    media: &MediaInfo,
    settings: &ExportSettings,
) -> Result<JobPlan, String> {
    let source = absolute_path(&media.path)?;
    if !source.is_file() {
        return Err("源文件已移动或不存在，请重新添加。".into());
    }
    let operation = settings.operation;
    let format = settings.format.trim().to_ascii_lowercase();
    if !operation.formats().contains(&format.as_str())
        && !(operation == Operation::Snapshot && format == "jpeg")
    {
        return Err(format!("{}不支持 .{format} 格式。", operation.label()));
    }
    if let Some(error) = video_settings_error(tools, settings) {
        return Err(error);
    }
    if settings.encoding.audio_rate_mode == crate::encoding::AudioRateMode::Bitrate
        && !(32..=320).contains(&settings.audio_bitrate)
    {
        return Err("音频码率需在 32–320 kbps 之间。".into());
    }
    if operation == Operation::Audio
        && format == "ogg"
        && settings.audio_bitrate < 64
        && settings.encoding.audio_rate_mode == crate::encoding::AudioRateMode::Bitrate
    {
        return Err("OGG 码率至少为 64 kbps。".into());
    }
    let video_output = matches!(
        operation,
        Operation::Convert | Operation::Compress | Operation::Trim
    );
    let copy_video =
        operation == Operation::Remux || (operation == Operation::Trim && settings.lossless_trim);
    let subtitles = if (video_output || operation == Operation::Remux)
        && settings.tracks.mode == crate::tracks::SubtitleMode::Keep
    {
        validate_subtitles(&settings.subtitle_files)?
    } else {
        Vec::new()
    };
    let audio_files = if operation == Operation::Remux {
        validate_audio_files(&settings.audio_files)?
    } else {
        Vec::new()
    };
    if operation == Operation::Audio && media.audio_codec.is_none() && !settings.effects.mix.enabled
    {
        return Err("此文件没有音轨，无法提取音频。".into());
    }
    if !matches!(
        operation,
        Operation::Audio | Operation::Remux | Operation::Subtitle
    ) && media.video_codec.is_none()
    {
        return Err("此操作需要视频画面。纯音频文件请使用“音频处理”。".into());
    }
    if operation == Operation::Remux
        && media.video_codec.is_none()
        && (media.audio_codec.is_none() || settings.muted)
        && audio_files.is_empty()
    {
        return Err("没有可输出的媒体流。".into());
    }
    if settings.quality > 51
        || settings.resolution > 8640
        || settings.resolution == 1
        || settings.fps > 240
    {
        return Err("画质需在 0–51 之间，分辨率不超过 8640，帧率不超过 240。".into());
    }
    let (start, end) = if matches!(operation, Operation::Remux | Operation::Subtitle) {
        (0.0, 0.0)
    } else {
        (
            settings.start_seconds,
            if operation == Operation::Snapshot {
                0.0
            } else {
                settings.end_seconds
            },
        )
    };
    if !start.is_finite() || !end.is_finite() || start < 0.0 || end < 0.0 {
        return Err("时间必须为有效的非负秒数。".into());
    }
    if media.duration > 0.0 && start >= media.duration {
        return Err("开始时间必须小于媒体总时长。".into());
    }
    if end > 0.0 && end <= start {
        return Err("结束时间必须大于开始时间；填 0 表示直到末尾。".into());
    }
    let stop = if end > 0.0 {
        if media.duration > 0.0 {
            end.min(media.duration)
        } else {
            end
        }
    } else {
        media.duration.max(0.0)
    };
    let duration = if operation == Operation::Snapshot {
        0.0
    } else {
        (stop - start).max(0.0)
    };
    let output_dir = settings
        .output_dir
        .as_deref()
        .unwrap_or_else(|| source.parent().unwrap_or(Path::new(".")));
    let output_dir = absolute_path(output_dir)?;
    if output_dir.exists() && !output_dir.is_dir() {
        return Err("输出位置不是文件夹。".into());
    }
    let mut stem = source
        .file_stem()
        .unwrap_or(OsStr::new("media"))
        .to_os_string();
    stem.push(format!("-{}", operation.suffix()));
    let extension = if format == "alac" { "m4a" } else { &format };
    let output = available_output(&output_dir, &stem, extension)?;
    if output == source {
        return Err("输出文件不能覆盖源文件。".into());
    }
    let mut args = Vec::new();
    push(
        &mut args,
        &[
            "-hide_banner",
            "-nostdin",
            "-n",
            "-nostats",
            "-loglevel",
            "warning",
            "-progress",
            "pipe:1",
            "-stats_period",
            "0.25",
        ],
    );
    if video_output && !copy_video {
        args.extend(crate::encoding::input_args(settings)?);
    }
    if start > 0.0 {
        push(&mut args, &["-ss", &seconds(start)]);
    }
    args.push("-i".into());
    args.push(source.as_os_str().to_owned());
    for (index, subtitle) in subtitles.iter().chain(audio_files.iter()).enumerate() {
        if index < subtitles.len() && start > 0.0 {
            push(&mut args, &["-ss", &seconds(start)]);
        }
        args.push("-i".into());
        args.push(subtitle.as_os_str().to_owned());
    }
    let source_tracks = effective_tracks(media);
    let mut effects = settings.effects.clone();
    if !video_output && !matches!(operation, Operation::Gif | Operation::Snapshot) || copy_video {
        effects.rotation = Default::default();
        effects.flip_horizontal = false;
        effects.flip_vertical = false;
        effects.watermark = Default::default();
        effects.composition = Default::default();
    }
    if !matches!(
        operation,
        Operation::Convert | Operation::Compress | Operation::Trim | Operation::Audio
    ) || copy_video
        || (settings.muted && operation != Operation::Audio)
    {
        effects.mix = Default::default();
    }
    let effect_inputs = crate::effects::inputs(&effects);
    let first_extra_input = 1 + subtitles.len() + audio_files.len();
    for input in &effect_inputs {
        crate::effects::append_input_args(input, &mut args);
    }
    if end > 0.0 && operation != Operation::Snapshot || !effect_inputs.is_empty() && duration > 0.0
    {
        push(&mut args, &["-t", &seconds(duration)]);
    }
    let mut audio_maps = Vec::new();
    let audio_count = if !(effects.mix.enabled
        && (!effects.mix.include_source
            || settings.tracks.audio.as_ref().is_some_and(Vec::is_empty)))
        && matches!(
            operation,
            Operation::Convert
                | Operation::Compress
                | Operation::Trim
                | Operation::Remux
                | Operation::Audio
        ) {
        crate::tracks::append_audio(
            &mut audio_maps,
            &source_tracks,
            &settings.tracks,
            settings.muted && operation != Operation::Audio,
            operation == Operation::Audio || effects.mix.enabled,
        )?
    } else {
        0
    };
    if operation == Operation::Audio && audio_count == 0 && !effects.mix.enabled {
        return Err("请选择至少一条音轨。".into());
    }
    let base_audio = if audio_count > 0 {
        Some(
            settings
                .tracks
                .audio
                .as_ref()
                .and_then(|s| s.first())
                .map(|s| format!("0:{}", s.index))
                .unwrap_or_else(|| "0:a:0".into()),
        )
    } else {
        None
    };
    let has_video =
        !copy_video && (video_output || matches!(operation, Operation::Gif | Operation::Snapshot));
    let effect_duration = if operation == Operation::Snapshot {
        (media.duration - start).max(0.0)
    } else {
        duration
    };
    let mut graph = crate::effects::build_graph(
        &effects,
        has_video.then_some(crate::effects::VideoBase {
            label: "0:V:0",
            width: media.width,
            height: media.height,
        }),
        base_audio.as_deref(),
        first_extra_input,
        effect_duration,
    )?;
    let mut first_pass = None;
    match operation {
        Operation::Convert | Operation::Compress | Operation::Trim if !copy_video => {
            let output_audio_count = if effects.mix.enabled { 1 } else { audio_count };
            crate::encoding::validate(settings, duration, output_audio_count)?;
            let mut filters = Vec::new();
            if let Some(burn) =
                crate::tracks::burn_filter(&source, &source_tracks, &settings.tracks)?
            {
                if start > 0.0 {
                    filters.push(format!("setpts=PTS+{start:.6}/TB"));
                }
                filters.push(burn);
                if start > 0.0 {
                    filters.push("setpts=PTS-STARTPTS".into());
                }
            }
            let mut spatial = settings.clone();
            if settings.encoding.gpu_scale {
                spatial.resolution = 0;
            }
            filters.extend(spatial_filters(&spatial, true));
            if let Some(filter) = crate::encoding::gpu_scale_filter(settings)? {
                filters.push(filter);
            }
            if let Some(filter) = crate::encoding::encoder_upload_filter(settings)? {
                filters.push(filter);
            }
            let video_label = graph.video_label.clone().unwrap_or_else(|| "0:V:0".into());
            graph
                .filters
                .push(format!("[{video_label}]{}[ff_video]", filters.join(",")));
            let common = args.clone();
            let video_args = crate::encoding::video_args(settings, duration, output_audio_count)?;
            if crate::encoding::two_pass(settings) {
                let mut first = common;
                let mut first_graph = graph.filters.clone();
                if effects.mix.enabled
                    && let Some(label) = &graph.audio_label
                {
                    first_graph.push(format!("[{label}]anullsink"));
                }
                push(
                    &mut first,
                    &[
                        "-filter_complex",
                        &first_graph.join(";"),
                        "-map",
                        "[ff_video]",
                    ],
                );
                first.extend(video_args.clone());
                first.extend(crate::encoding::pass_args(
                    1,
                    Path::new("__FRAMEFLOW_PASSLOG__"),
                    settings,
                ));
                push(
                    &mut first,
                    &[
                        "-an",
                        "-sn",
                        "-dn",
                        "-f",
                        "null",
                        if cfg!(windows) { "NUL" } else { "/dev/null" },
                    ],
                );
                first_pass = Some(first);
            }
            push(
                &mut args,
                &[
                    "-filter_complex",
                    &graph.filters.join(";"),
                    "-map",
                    "[ff_video]",
                ],
            );
            args.extend(video_args);
            if crate::encoding::two_pass(settings) {
                args.extend(crate::encoding::pass_args(
                    2,
                    Path::new("__FRAMEFLOW_PASSLOG__"),
                    settings,
                ));
            }
            append_encoded_audio(
                &mut args,
                &audio_maps,
                &graph,
                effects.mix.enabled,
                output_audio_count,
                settings,
                if format == "webm" {
                    "video-opus"
                } else {
                    "video-aac"
                },
            )?;
            crate::tracks::append_subtitles(
                &mut args,
                &source_tracks,
                &settings.tracks,
                &format,
                &subtitles,
            )?;
            crate::tracks::append_attachments(&mut args, &settings.tracks, &format)?;
            crate::tracks::append_attachment_metadata(&mut args, &source_tracks, &settings.tracks)?;
            args.push("-dn".into());
            // A subtitle crossing the cut start can have a negative PTS. Do not
            // let the muxer shift the re-encoded audio/video timeline to match it.
            push(&mut args, &["-avoid_negative_ts", "disabled"]);
        }
        Operation::Remux | Operation::Trim => {
            if settings.tracks.mode == crate::tracks::SubtitleMode::Burn {
                return Err("烧录字幕需要重新编码，请关闭无损裁剪或选择格式转换。".into());
            }
            if media.video_codec.is_some() {
                push(&mut args, &["-map", "0:V:0"]);
            }
            args.extend(audio_maps);
            append_external_audio(&mut args, &audio_files, subtitles.len() + 1, audio_count);
            if media.video_codec.is_none() && audio_count == 0 && audio_files.is_empty() {
                return Err("没有可输出的媒体流。".into());
            }
            push(&mut args, &["-c", "copy", "-dn"]);
            crate::tracks::append_subtitles(
                &mut args,
                &source_tracks,
                &settings.tracks,
                &format,
                &subtitles,
            )?;
            crate::tracks::append_attachments(&mut args, &settings.tracks, &format)?;
            crate::tracks::append_attachment_metadata(&mut args, &source_tracks, &settings.tracks)?;
            if operation == Operation::Trim {
                push(&mut args, &["-avoid_negative_ts", "make_zero"]);
            }
            if matches!(format.as_str(), "mp4" | "mov") {
                push(&mut args, &["-movflags", "+faststart"]);
            }
        }
        Operation::Audio => {
            crate::encoding::validate(settings, duration, 1)?;
            if !graph.filters.is_empty() {
                push(&mut args, &["-filter_complex", &graph.filters.join(";")]);
            }
            append_encoded_audio(
                &mut args,
                &audio_maps,
                &graph,
                effects.mix.enabled,
                if effects.mix.enabled { 1 } else { audio_count },
                settings,
                &format,
            )?;
            push(&mut args, &["-vn", "-sn", "-dn"]);
        }
        Operation::Gif | Operation::Snapshot => {
            let mut spatial = settings.clone();
            if operation == Operation::Snapshot {
                spatial.fps = 0;
            }
            let mut filters = spatial_filters(&spatial, false);
            if filters.is_empty() {
                filters.push("null".into());
            }
            if operation == Operation::Gif {
                filters.push(
                    "split[a][b];[a]palettegen=stats_mode=single[p];[b][p]paletteuse=new=1".into(),
                );
            }
            let label = graph.video_label.clone().unwrap_or_else(|| "0:V:0".into());
            graph
                .filters
                .push(format!("[{label}]{}[ff_video]", filters.join(",")));
            push(
                &mut args,
                &[
                    "-filter_complex",
                    &graph.filters.join(";"),
                    "-map",
                    "[ff_video]",
                    "-an",
                ],
            );
            if operation == Operation::Gif {
                push(&mut args, &["-loop", "0"]);
            } else {
                push(&mut args, &["-frames:v", "1", "-update", "1"]);
                match format.as_str() {
                    "png" => push(&mut args, &["-c:v", "png"]),
                    "jpg" | "jpeg" => push(&mut args, &["-c:v", "mjpeg", "-q:v", "2"]),
                    "webp" => push(&mut args, &["-c:v", "libwebp", "-lossless", "1"]),
                    _ => unreachable!(),
                }
            }
        }
        Operation::Subtitle => {
            crate::tracks::extraction_args(&mut args, &source_tracks, &settings.tracks, &format)?
        }
        Operation::Convert | Operation::Compress => unreachable!(),
    }
    args.push(output.as_os_str().to_owned());
    Ok(JobPlan {
        program: tools.ffmpeg.clone(),
        args,
        output,
        duration,
        first_pass,
    })
}

fn effective_tracks(media: &MediaInfo) -> Vec<crate::tracks::TrackInfo> {
    if !media.tracks.is_empty() {
        return media.tracks.clone();
    }
    media
        .audio_codec
        .as_ref()
        .map(|codec| crate::tracks::TrackInfo {
            index: usize::from(media.video_codec.is_some()),
            kind: crate::tracks::TrackKind::Audio,
            codec: codec.clone(),
            language: String::new(),
            title: String::new(),
            default: true,
        })
        .into_iter()
        .collect()
}

fn append_encoded_audio(
    args: &mut Vec<OsString>,
    maps: &[OsString],
    graph: &crate::effects::EffectGraph,
    mixed: bool,
    count: usize,
    settings: &ExportSettings,
    format: &str,
) -> Result<(), String> {
    if count == 0 {
        args.push("-an".into());
        return Ok(());
    }
    if mixed {
        let label = graph.audio_label.as_ref().ok_or("混音未产生音轨。")?;
        push(args, &["-map", &format!("[{label}]")]);
        for pair in maps.as_chunks::<2>().0 {
            let flag = pair[0].to_string_lossy();
            if flag.starts_with("-metadata:s:a:") || flag.starts_with("-disposition:a:") {
                args.extend_from_slice(pair);
            }
        }
    } else {
        args.extend_from_slice(maps);
    }
    args.extend(crate::encoding::audio_args(settings, format)?);
    Ok(())
}

fn append_external_audio(
    args: &mut Vec<OsString>,
    files: &[PathBuf],
    first_input: usize,
    first_output: usize,
) {
    for (index, audio) in files.iter().enumerate() {
        let track = first_output + index;
        push(args, &["-map", &format!("{}:a:0", first_input + index)]);
        let mut title = OsString::from("title=");
        title.push(audio.file_stem().unwrap_or(OsStr::new("音频")));
        args.push(format!("-metadata:s:a:{track}").into());
        args.push(title);
        let mut handler = OsString::from("handler_name=");
        handler.push(audio.file_stem().unwrap_or(OsStr::new("音频")));
        args.push(format!("-metadata:s:a:{track}").into());
        args.push(handler);
        push(
            args,
            &[
                &format!("-disposition:a:{track}"),
                if track == 0 { "default" } else { "0" },
            ],
        );
    }
}

fn validate_audio_files(files: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    files
        .iter()
        .map(|file| {
            let path = absolute_path(file)?;
            let metadata = fs::metadata(&path)
                .map_err(|error| format!("无法读取音频文件 {}：{error}", path.display()))?;
            if !metadata.is_file() || metadata.len() == 0 {
                return Err(format!("音频路径必须是非空文件：{}", path.display()));
            }
            File::open(&path)
                .map_err(|error| format!("无法打开音频文件 {}：{error}", path.display()))?;
            Ok(path)
        })
        .collect()
}

fn validate_subtitles(files: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    files
        .iter()
        .map(|file| {
            let path = absolute_path(file)?;
            let format = path
                .extension()
                .and_then(OsStr::to_str)
                .unwrap_or_default()
                .to_ascii_lowercase();
            if !["srt", "ass", "ssa", "vtt"].contains(&format.as_str()) {
                return Err(format!(
                    "不支持的字幕格式：{}。请选择 SRT、ASS、SSA 或 VTT 文件。",
                    path.display()
                ));
            }
            let metadata = fs::metadata(&path)
                .map_err(|error| format!("无法读取字幕文件 {}：{error}", path.display()))?;
            if !metadata.is_file() {
                return Err(format!("字幕路径不是文件：{}", path.display()));
            }
            if metadata.len() == 0 {
                return Err(format!("字幕文件为空：{}", path.display()));
            }
            File::open(&path)
                .map_err(|error| format!("无法打开字幕文件 {}：{error}", path.display()))?;
            Ok(path)
        })
        .collect()
}

fn push(args: &mut Vec<OsString>, values: &[&str]) {
    args.extend(values.iter().map(OsString::from));
}

fn seconds(value: f64) -> String {
    format!("{value:.6}")
}

fn spatial_filters(settings: &ExportSettings, even_dimensions: bool) -> Vec<String> {
    let mut filters = Vec::new();
    if settings.fps > 0 {
        filters.push(format!("fps={}", settings.fps));
    }
    if settings.resolution > 0 {
        let height = if even_dimensions {
            settings.resolution.div_ceil(2) * 2
        } else {
            settings.resolution
        };
        filters.push(format!("scale=-2:{height}:flags=lanczos"));
    } else if even_dimensions {
        filters.push("pad=ceil(iw/2)*2:ceil(ih/2)*2".into());
    }
    filters
}

fn video_encoding(args: &mut Vec<OsString>, format: &str, settings: &ExportSettings) {
    push(args, &["-vf", &spatial_filters(settings, true).join(",")]);
    if format == "webm" {
        push(
            args,
            &[
                "-c:v",
                "libvpx-vp9",
                "-crf",
                &settings.quality.to_string(),
                "-b:v",
                "0",
                "-deadline",
                "good",
                "-cpu-used",
                "4",
            ],
        );
    } else {
        push(
            args,
            &[
                "-c:v",
                settings.video_encoder.codec_name(settings.video_codec),
            ],
        );
        let quality = settings.quality.to_string();
        match settings.video_encoder {
            VideoEncoder::Cpu => push(args, &["-preset", "medium", "-crf", &quality]),
            VideoEncoder::Nvidia => push(
                args,
                &["-preset", "p4", "-rc", "vbr", "-cq", &quality, "-b:v", "0"],
            ),
            VideoEncoder::Intel => push(args, &["-preset", "medium", "-global_quality", &quality]),
            VideoEncoder::Amd => push(
                args,
                &[
                    "-quality", "balanced", "-rc", "cqp", "-qp_i", &quality, "-qp_p", &quality,
                ],
            ),
        }
        if settings.video_codec == VideoCodec::H265 && matches!(format, "mp4" | "mov") {
            push(args, &["-tag:v", "hvc1"]);
        }
    }
    push(args, &["-pix_fmt", "yuv420p"]);
    if format == "mp4" || format == "mov" {
        push(args, &["-movflags", "+faststart"]);
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|e| format!("无法解析文件路径：{e}"))
    }
}

fn output_candidate(directory: &Path, stem: &OsStr, extension: &str, index: u32) -> PathBuf {
    let mut name = stem.to_os_string();
    if index > 0 {
        name.push(format!("-{index}"));
    }
    name.push(format!(".{extension}"));
    directory.join(name)
}

fn available_output(directory: &Path, stem: &OsStr, extension: &str) -> Result<PathBuf, String> {
    for index in 0..10_000 {
        let path = output_candidate(directory, stem, extension, index);
        if !path
            .try_exists()
            .map_err(|e| format!("无法检查输出位置：{e}"))?
        {
            return Ok(path);
        }
    }
    Err("同名输出文件过多，请选择另一个输出文件夹。".into())
}

/// A display/copy convenience only. Execution always uses the original OsString arguments.
pub fn command_preview(plan: &JobPlan) -> String {
    let final_command = preview_args(&plan.program, &plan.args);
    if let Some(first) = &plan.first_pass {
        format!(
            "# 第 1 遍分析；运行时使用独立临时日志\n{}\n\n# 第 2 遍输出\n{}",
            preview_args(&plan.program, first),
            final_command
        )
    } else {
        final_command
    }
}

fn preview_args(program: &Path, args: &[OsString]) -> String {
    let preview = std::iter::once(program.as_os_str())
        .chain(args.iter().map(OsString::as_os_str))
        .map(|argument| {
            let value = argument.to_string_lossy();
            if cfg!(windows) {
                // PowerShell single-quoted literals cannot interpolate $, backticks, or subexpressions.
                format!("'{}'", value.replace('\'', "''"))
            } else {
                format!("'{}'", value.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    if cfg!(windows) {
        format!("& {preview}")
    } else {
        preview
    }
}

#[derive(Clone, Debug)]
pub enum ProgressEvent {
    Progress { fraction: f32, speed: String },
    Log(String),
}

enum PipeLine {
    Progress(String),
    Error(String),
}

fn read_pipe(reader: impl Read, sender: mpsc::SyncSender<PipeLine>, progress: bool) {
    let mut reader = BufReader::new(reader);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let line: String = String::from_utf8_lossy(&buffer)
                    .trim()
                    .chars()
                    .take(2000)
                    .collect();
                if line.is_empty() {
                    continue;
                }
                let message = if progress {
                    PipeLine::Progress(line)
                } else {
                    PipeLine::Error(line)
                };
                if sender.send(message).is_err() {
                    break;
                }
            }
        }
    }
}

/// Encode into a privately created folder, then exclusively create the destination.
/// This prevents failed/cancelled tasks from removing or overwriting unrelated files.
pub fn run(
    plan: JobPlan,
    cancel: Arc<AtomicBool>,
    mut on_event: impl FnMut(ProgressEvent) + Send + 'static,
) -> Result<PathBuf, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("已取消".into());
    }
    if plan.args.last().map(OsString::as_os_str) != Some(plan.output.as_os_str()) {
        return Err("输出计划无效。".into());
    }
    let directory = plan.output.parent().ok_or("输出路径无效。")?;
    fs::create_dir_all(directory).map_err(|e| format!("无法创建输出文件夹：{e}"))?;
    let stage = Stage::new(
        directory,
        plan.output.extension().unwrap_or(OsStr::new("tmp")),
    )?;
    let mut args = plan.args.clone();
    *args.last_mut().ok_or("输出计划为空。")? = stage.output.as_os_str().to_owned();
    let log_path = stage.directory.join("two-pass");
    if let Some(first) = &plan.first_pass {
        let mut first = resolve_pass_log(first, &log_path);
        if let Some(no_overwrite) = first.iter_mut().find(|arg| *arg == "-n") {
            *no_overwrite = "-y".into();
        }
        on_event(ProgressEvent::Log("第 1 遍：分析视频。".into()));
        execute(
            &plan.program,
            &first,
            plan.duration,
            &cancel,
            &mut on_event,
            0.0,
            0.5,
        )?;
        if cancel.load(Ordering::Relaxed) {
            return Err("已取消".into());
        }
        on_event(ProgressEvent::Log("第 2 遍：生成输出。".into()));
    }
    args = resolve_pass_log(&args, &log_path);
    let (offset, weight) = if plan.first_pass.is_some() {
        (0.5, 0.5)
    } else {
        (0.0, 1.0)
    };
    execute(
        &plan.program,
        &args,
        plan.duration,
        &cancel,
        &mut on_event,
        offset,
        weight,
    )?;
    if cancel.load(Ordering::Relaxed) {
        return Err("已取消".into());
    }
    if fs::metadata(&stage.output).map(|m| m.len()).unwrap_or(0) == 0 {
        return Err("FFmpeg 未生成有效文件，请检查时间范围和媒体流。".into());
    }
    let output = publish(&stage.output, &plan.output, &cancel)?;
    on_event(ProgressEvent::Progress {
        fraction: 1.0,
        speed: String::new(),
    });
    Ok(output)
}

fn resolve_pass_log(args: &[OsString], path: &Path) -> Vec<OsString> {
    args.iter()
        .map(|arg| {
            let value = arg.to_string_lossy();
            if value.contains("__FRAMEFLOW_PASSLOG__") {
                let replacement = if value.contains("stats=__FRAMEFLOW_PASSLOG__") {
                    crate::encoding::escape_x265_path(path)
                } else {
                    path.to_string_lossy().into_owned()
                };
                value.replace("__FRAMEFLOW_PASSLOG__", &replacement).into()
            } else {
                arg.clone()
            }
        })
        .collect()
}

fn execute(
    program: &Path,
    args: &[OsString],
    duration: f64,
    cancel: &Arc<AtomicBool>,
    on_event: &mut impl FnMut(ProgressEvent),
    offset: f32,
    weight: f32,
) -> Result<(), String> {
    let mut child = command(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("无法启动 FFmpeg：{e}"))?;
    on_event(ProgressEvent::Log(format!(
        "FFmpeg 已启动（进程 {}）。",
        child.id()
    )));
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");
    let (sender, receiver) = mpsc::sync_channel(128);
    let progress_sender = sender.clone();
    let progress_reader = thread::spawn(move || read_pipe(stdout, progress_sender, true));
    let error_reader = thread::spawn(move || read_pipe(stderr, sender, false));
    let mut errors = VecDeque::new();
    let mut fraction = 0.0;
    let mut speed = String::new();
    let outcome = {
        let mut consume = |line: PipeLine| match line {
            PipeLine::Error(line) => {
                if errors.len() == 24 {
                    errors.pop_front();
                }
                let line: String = line.chars().take(2000).collect();
                errors.push_back(line.clone());
                on_event(ProgressEvent::Log(line));
            }
            PipeLine::Progress(line) => {
                if let Some((key, value)) = line.split_once('=') {
                    match key {
                        "out_time_us" | "out_time_ms" => {
                            // Both FFmpeg fields are microseconds; out_time_ms is a legacy spelling.
                            if let Ok(microseconds) = value.parse::<f64>()
                                && duration > 0.0
                                && microseconds.is_finite()
                            {
                                fraction =
                                    (microseconds / 1_000_000.0 / duration).clamp(0.0, 0.99) as f32;
                            }
                        }
                        "speed" => speed = value.trim().to_owned(),
                        "progress" => on_event(ProgressEvent::Progress {
                            fraction: offset + fraction * weight,
                            speed: speed.clone(),
                        }),
                        _ => {}
                    }
                }
            }
        };

        let outcome = loop {
            if cancel.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                break Err("已取消".to_string());
            }
            match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(line) => consume(line),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    thread::sleep(Duration::from_millis(10))
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) => {}
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(format!("无法读取 FFmpeg 状态：{error}"));
                }
            }
        };
        // Drain before joining: a reader may be waiting for space in the bounded queue.
        for line in receiver {
            consume(line);
        }
        let _ = progress_reader.join();
        let _ = error_reader.join();
        outcome
    };
    let status = outcome?;
    if !status.success() {
        let detail = errors.into_iter().collect::<Vec<_>>().join("\n");
        let gpu = args.windows(2).any(|pair| {
            pair[0] == "-c:v"
                && [
                    "h264_nvenc",
                    "hevc_nvenc",
                    "h264_qsv",
                    "hevc_qsv",
                    "h264_amf",
                    "hevc_amf",
                ]
                .iter()
                .any(|codec| pair[1] == *codec)
        });
        if gpu {
            return Err(format!(
                "GPU 编码失败，请检查驱动和输出尺寸，或选择 CPU。\n{detail}"
            ));
        }
        return Err(if detail.is_empty() {
            format!("FFmpeg 执行失败（{status}）。")
        } else {
            format!("FFmpeg 执行失败：\n{detail}")
        });
    }
    Ok(())
}

static NEXT_STAGE: AtomicU64 = AtomicU64::new(0);

struct Stage {
    directory: PathBuf,
    output: PathBuf,
}

impl Stage {
    fn new(parent: &Path, extension: &OsStr) -> Result<Self, String> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for _ in 0..100 {
            let sequence = NEXT_STAGE.fetch_add(1, Ordering::Relaxed);
            let directory = parent.join(format!(
                ".ffmpeg-studio-{}-{stamp}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&directory) {
                Ok(()) => {
                    let mut filename = OsString::from("output.");
                    filename.push(extension);
                    return Ok(Self {
                        output: directory.join(filename),
                        directory,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(format!("无法创建导出临时文件夹：{error}")),
            }
        }
        Err("无法分配导出临时文件夹。".into())
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.output);
        if let Ok(files) = fs::read_dir(&self.directory) {
            for entry in files.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if (name == "two-pass"
                    || name.starts_with("two-pass.")
                    || name.starts_with("two-pass-"))
                    && entry.file_type().is_ok_and(|kind| kind.is_file())
                {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
        // Deliberately non-recursive: never remove any unexpected files.
        let _ = fs::remove_dir(&self.directory);
    }
}

fn publish(stage: &Path, desired: &Path, cancel: &AtomicBool) -> Result<PathBuf, String> {
    let parent = desired.parent().ok_or("输出路径无效。")?;
    let stem = desired.file_stem().ok_or("输出文件名无效。")?;
    let extension = desired
        .extension()
        .and_then(OsStr::to_str)
        .ok_or("输出格式无效。")?;
    for index in 0..10_000 {
        if cancel.load(Ordering::Relaxed) {
            return Err("已取消".into());
        }
        let output = output_candidate(parent, stem, extension, index);
        // A same-filesystem hard link publishes atomically without replacing an existing file.
        match fs::hard_link(stage, &output) {
            Ok(()) => return Ok(output),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(_) => {}
        }
        // FAT/exFAT/network volumes may not support hard links. create_new still prevents overwrite.
        let mut destination = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("无法保存导出文件：{error}")),
        };
        let result = (|| {
            let mut source = File::open(stage)?;
            let mut buffer = vec![0_u8; 1024 * 1024];
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "已取消"));
                }
                let count = source.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                destination.write_all(&buffer[..count])?;
            }
            destination.sync_all()
        })();
        drop(destination);
        if let Err(error) = result {
            let _ = fs::remove_file(&output); // This handle was exclusively created by this task.
            return Err(format!("保存导出文件失败：{error}"));
        }
        return Ok(output);
    }
    Err("同名输出文件过多，请更改输出文件夹。".into())
}

fn tail_text(bytes: &[u8], max_chars: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    let chars: Vec<char> = text.trim().chars().collect();
    chars[chars.len().saturating_sub(max_chars)..]
        .iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        directory: PathBuf,
        media: MediaInfo,
    }

    impl Fixture {
        fn new() -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let directory = env::temp_dir().join(format!(
                "ffmpeg-studio-test-{}-{stamp}-{}",
                std::process::id(),
                NEXT_STAGE.fetch_add(1, Ordering::Relaxed)
            ));
            // Exclusive creation makes this test the sole owner of the cleanup directory.
            fs::create_dir(&directory).unwrap();
            let path = directory.join("中文 source ' clip.mp4");
            fs::write(&path, "fixture").unwrap();
            Self {
                directory,
                media: MediaInfo {
                    path,
                    duration: 120.0,
                    width: 1920,
                    height: 1080,
                    video_codec: Some("h264".into()),
                    audio_codec: Some("aac".into()),
                    size: 7,
                    tracks: Vec::new(),
                },
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            // Test-owned unique directory only.
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    fn tools() -> Toolchain {
        Toolchain {
            ffmpeg: "ffmpeg".into(),
            ffprobe: "ffprobe".into(),
            version: "test".into(),
            gpu_encoders: Vec::new(),
        }
    }

    fn strings(plan: &JobPlan) -> Vec<String> {
        plan.args
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn trim_uses_input_seek_and_output_duration_and_preserves_path() {
        let fixture = Fixture::new();
        let settings = ExportSettings {
            operation: Operation::Trim,
            start_seconds: 15.25,
            end_seconds: 30.5,
            ..Default::default()
        };
        let plan = plan(&tools(), &fixture.media, &settings).unwrap();
        let args = strings(&plan);
        let seek = args.iter().position(|s| s == "-ss").unwrap();
        let input = args.iter().position(|s| s == "-i").unwrap();
        let duration = args.iter().position(|s| s == "-t").unwrap();
        assert!(seek < input && input < duration);
        assert_eq!(args[seek + 1], "15.250000");
        assert_eq!(args[duration + 1], "15.250000");
        assert_eq!(plan.args[input + 1], fixture.media.path.as_os_str());
        assert_eq!(plan.duration, 15.25);
        assert!(args.contains(&"-n".to_string()));
        assert!(!args.contains(&"-y".to_string()));
    }

    #[test]
    fn timed_operations_preserve_milliseconds_in_ffmpeg_arguments() {
        let mut fixture = Fixture::new();
        fixture.media.duration = 4000.0;
        for (start, end, seek, duration) in [
            (0.001, 0.002, "0.001000", "0.001000"),
            (61.001, 62.002, "61.001000", "1.001000"),
            (3661.999, 3662.0, "3661.999000", "0.001000"),
        ] {
            for (operation, format) in [
                (Operation::Trim, "mp4"),
                (Operation::Audio, "wav"),
                (Operation::Snapshot, "png"),
                (Operation::Gif, "gif"),
            ] {
                let settings = ExportSettings {
                    operation,
                    format: format.into(),
                    start_seconds: start,
                    end_seconds: end,
                    ..Default::default()
                };
                let job = plan(&tools(), &fixture.media, &settings).unwrap();
                let args = strings(&job);
                let seek_option = args.iter().position(|arg| arg == "-ss").unwrap();
                assert_eq!(args[seek_option + 1], seek, "{operation:?}");
                if operation == Operation::Snapshot {
                    assert!(!args.iter().any(|arg| arg == "-t"));
                    assert!(args.windows(2).any(|pair| pair == ["-frames:v", "1"]));
                } else {
                    let duration_option = args.iter().position(|arg| arg == "-t").unwrap();
                    assert_eq!(args[duration_option + 1], duration, "{operation:?}");
                }
            }
        }
    }

    #[test]
    fn millisecond_input_keeps_end_sentinel_and_submillisecond_source_boundary() {
        let mut fixture = Fixture::new();
        fixture.media.duration = 0.0;
        let mut settings = ExportSettings {
            operation: Operation::Audio,
            format: "wav".into(),
            start_seconds: 0.001,
            end_seconds: 0.0,
            ..Default::default()
        };
        let args = strings(&plan(&tools(), &fixture.media, &settings).unwrap());
        assert!(args.windows(2).any(|pair| pair == ["-ss", "0.001000"]));
        assert!(!args.iter().any(|arg| arg == "-t"));
        fixture.media.duration = 0.0015;
        settings.end_seconds = 0.002;
        let args = strings(&plan(&tools(), &fixture.media, &settings).unwrap());
        assert!(args.windows(2).any(|pair| pair == ["-t", "0.000500"]));
    }

    #[test]
    fn validates_media_and_time_ranges() {
        let mut fixture = Fixture::new();
        let mut settings = ExportSettings {
            operation: Operation::Trim,
            start_seconds: 20.0,
            end_seconds: 10.0,
            ..Default::default()
        };
        assert!(plan(&tools(), &fixture.media, &settings).is_err());
        settings.end_seconds = 0.0;
        settings.start_seconds = f64::NAN;
        assert!(plan(&tools(), &fixture.media, &settings).is_err());
        settings.start_seconds = 120.0;
        assert!(plan(&tools(), &fixture.media, &settings).is_err());
        settings.start_seconds = 0.0;
        fixture.media.video_codec = None;
        assert!(plan(&tools(), &fixture.media, &settings).is_err());
        settings.operation = Operation::Audio;
        settings.format = "mp3".into();
        assert!(plan(&tools(), &fixture.media, &settings).is_ok());
        fixture.media.audio_codec = None;
        assert!(plan(&tools(), &fixture.media, &settings).is_err());
    }

    #[test]
    fn remux_never_adds_encoding_filters_or_trim() {
        let fixture = Fixture::new();
        let settings = ExportSettings {
            operation: Operation::Remux,
            start_seconds: 30.0,
            end_seconds: 60.0,
            resolution: 720,
            fps: 24,
            ..Default::default()
        };
        let job = plan(&tools(), &fixture.media, &settings).unwrap();
        let args = strings(&job);
        assert!(args.windows(2).any(|p| p == ["-c", "copy"]));
        assert!(
            !args
                .iter()
                .any(|s| ["-vf", "-ss", "-t", "-crf"].contains(&s.as_str()))
        );
        assert_eq!(job.duration, 120.0);
    }

    #[test]
    fn remux_audio_validation_and_selection_scope() {
        let fixture = Fixture::new();
        let missing = fixture.media.path.with_file_name("missing.m4a");
        let mut settings = ExportSettings {
            operation: Operation::Remux,
            audio_files: vec![missing],
            ..Default::default()
        };
        assert!(
            plan(&tools(), &fixture.media, &settings)
                .unwrap_err()
                .contains("无法读取音频文件")
        );
        settings.operation = Operation::Convert;
        assert!(plan(&tools(), &fixture.media, &settings).is_ok());
        let value = serde_json::to_value(&settings).unwrap();
        assert!(value.get("audio_files").is_none());
        assert!(
            serde_json::from_value::<ExportSettings>(value)
                .unwrap()
                .audio_files
                .is_empty()
        );
    }

    #[test]
    fn live_ffmpeg_remux_external_audio_with_subtitles_when_installed() {
        let Ok(toolchain) = discover(None) else {
            return;
        };
        let mut fixture = Fixture::new();
        let input = fixture.media.path.with_file_name("source.mp4");
        let result = command(&toolchain.ffmpeg)
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=size=64x64:duration=1",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=1",
                "-c:v",
                "libx264",
                "-c:a",
                "aac",
            ])
            .arg(&input)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        fixture.media = probe(&toolchain, &input).unwrap();
        let mut audio_files = Vec::new();
        for (name, frequency) in [("中文 音频.m4a", "660"), ("English.m4a", "880")] {
            let path = input.with_file_name(name);
            let result = command(&toolchain.ffmpeg)
                .args([
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("sine=frequency={frequency}:duration=1.5"),
                    "-c:a",
                    "aac",
                ])
                .arg(&path)
                .output()
                .unwrap();
            assert!(result.status.success());
            audio_files.push(path);
        }
        let subtitle = input.with_file_name("captions.srt");
        fs::write(&subtitle, SRT_SUBTITLE).unwrap();
        for format in ["mkv", "mp4", "mov"] {
            for muted in [false, true] {
                let settings = ExportSettings {
                    operation: Operation::Remux,
                    format: format.into(),
                    muted,
                    audio_files: audio_files.clone(),
                    subtitle_files: vec![subtitle.clone()],
                    ..Default::default()
                };
                let output = run(
                    plan(&toolchain, &fixture.media, &settings).unwrap(),
                    Arc::new(AtomicBool::new(false)),
                    |_| {},
                )
                .unwrap();
                let streams = stream_info(&toolchain, &output);
                assert_eq!(
                    streams
                        .iter()
                        .filter(|s| s["codec_type"] == "audio")
                        .count(),
                    if muted { 2 } else { 3 }
                );
                assert_eq!(
                    streams
                        .iter()
                        .filter(|s| s["codec_type"] == "subtitle")
                        .count(),
                    1
                );
                assert_eq!(
                    encoded_stream_hash(&toolchain, &input, "0:v:0"),
                    encoded_stream_hash(&toolchain, &output, "0:v:0")
                );
                if !muted {
                    assert_eq!(
                        encoded_stream_hash(&toolchain, &input, "0:a:0"),
                        encoded_stream_hash(&toolchain, &output, "0:a:0")
                    );
                }
                for (index, path) in audio_files.iter().enumerate() {
                    let track = index + usize::from(!muted);
                    assert_eq!(
                        encoded_stream_hash(&toolchain, path, "0:a:0"),
                        encoded_stream_hash(&toolchain, &output, &format!("0:a:{track}"))
                    );
                }
            }
        }
        let invalid_audio = input.with_file_name("invalid.wav");
        fs::write(&invalid_audio, b"not audio").unwrap();
        let settings = ExportSettings {
            operation: Operation::Remux,
            audio_files: vec![invalid_audio],
            ..Default::default()
        };
        let job = plan(&toolchain, &fixture.media, &settings).unwrap();
        let output = job.output.clone();
        assert!(run(job, Arc::new(AtomicBool::new(false)), |_| {}).is_err());
        assert!(!output.exists());
    }

    const SRT_SUBTITLE: &str = "1\n00:00:00,100 --> 00:00:01,100\n你好，Frameflow!\n\n";
    const ASS_SUBTITLE: &str = r"[Script Info]
ScriptType: v4.00+
PlayResX: 256
PlayResY: 144
[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: FrameflowAccent,Arial,28,&H0000FF00,&H000000FF,&H00000000,&H00000000,-1,0,0,0,100,100,0,0,1,2,0,2,10,10,10,1
[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: 0,0:00:00.10,0:00:01.10,FrameflowAccent,,0,0,0,,{\b1}样式字幕
";
    const SSA_SUBTITLE: &str = r"[Script Info]
ScriptType: v4.00
[V4 Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, TertiaryColour, BackColour, Bold, Italic, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, AlphaLevel, Encoding
Style: Default,Arial,24,16777215,255,0,0,0,0,1,1,0,2,10,10,10,0,1
[Events]
Format: Marked, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: Marked=0,0:00:00.10,0:00:01.10,Default,,0,0,0,,传统字幕
";

    fn subtitle_fixture(fixture: &Fixture) -> Vec<PathBuf> {
        [
            ("中文 ' 字幕.SRT", SRT_SUBTITLE),
            ("Styled track.ass", ASS_SUBTITLE),
            ("Legacy track.ssa", SSA_SUBTITLE),
            (
                "Web track.vtt",
                "WEBVTT\n\n00:00:00.100 --> 00:00:01.100\nWeb subtitle\n\n",
            ),
        ]
        .into_iter()
        .map(|(name, content)| {
            let path = fixture.directory.join(name);
            fs::write(&path, content).unwrap();
            path
        })
        .collect()
    }

    #[test]
    fn remux_subtitles_map_separate_inputs_and_only_encode_subtitle_streams() {
        let fixture = Fixture::new();
        let subtitles = subtitle_fixture(&fixture);
        for (format, codec) in [
            ("mkv", "copy"),
            ("mp4", "mov_text"),
            ("mov", "mov_text"),
            ("webm", "webvtt"),
        ] {
            let settings = ExportSettings {
                operation: Operation::Remux,
                format: format.into(),
                subtitle_files: subtitles.clone(),
                ..Default::default()
            };
            let job = plan(&tools(), &fixture.media, &settings).unwrap();
            let args = strings(&job);
            let first_map = args.iter().position(|s| s == "-map").unwrap();
            let inputs: Vec<_> = job
                .args
                .windows(2)
                .enumerate()
                .filter(|(_, pair)| pair[0] == "-i")
                .collect();
            assert_eq!(inputs.len(), subtitles.len() + 1);
            for (index, subtitle) in subtitles.iter().enumerate() {
                assert!(inputs[index + 1].0 < first_map);
                assert_eq!(inputs[index + 1].1[1], subtitle.as_os_str());
                assert!(
                    args.windows(2)
                        .any(|pair| pair[0] == "-map" && pair[1] == format!("{}:s:0", index + 1))
                );
            }
            assert!(args.windows(2).any(|pair| pair == ["-c", "copy"]));
            for index in 0..subtitles.len() {
                assert!(
                    args.windows(2)
                        .any(|pair| pair[0] == format!("-c:s:{index}") && pair[1] == codec)
                );
            }
            assert!(
                !args
                    .iter()
                    .any(|s| ["-sn", "-c:a", "-c:v", "0:s", "0:s:0", "-vf"].contains(&s.as_str()))
            );
        }
    }

    #[test]
    fn remux_subtitles_validate_files_and_other_operations_ignore_selection() {
        let fixture = Fixture::new();
        let missing = fixture.directory.join("missing.srt");
        let mut settings = ExportSettings {
            operation: Operation::Remux,
            subtitle_files: vec![missing.clone()],
            ..Default::default()
        };
        assert!(
            plan(&tools(), &fixture.media, &settings)
                .unwrap_err()
                .contains("无法读取字幕文件")
        );
        fs::write(&missing, b"").unwrap();
        assert!(
            plan(&tools(), &fixture.media, &settings)
                .unwrap_err()
                .contains("字幕文件为空")
        );
        let unsupported = fixture.directory.join("bitmap.sup");
        fs::write(&unsupported, b"binary subtitle").unwrap();
        settings.subtitle_files = vec![unsupported];
        assert!(
            plan(&tools(), &fixture.media, &settings)
                .unwrap_err()
                .contains("不支持的字幕格式")
        );
        let folder = fixture.directory.join("folder.ass");
        fs::create_dir(&folder).unwrap();
        settings.subtitle_files = vec![folder];
        assert!(
            plan(&tools(), &fixture.media, &settings)
                .unwrap_err()
                .contains("不是文件")
        );

        for operation in Operation::ALL
            .into_iter()
            .filter(|op| matches!(op, Operation::Audio | Operation::Gif | Operation::Snapshot))
        {
            settings.operation = operation;
            settings.format = operation.formats()[0].into();
            settings.subtitle_files = vec![fixture.directory.join("missing.srt")];
            let with_selection = plan(&tools(), &fixture.media, &settings).unwrap();
            settings.subtitle_files.clear();
            let without_selection = plan(&tools(), &fixture.media, &settings).unwrap();
            assert_eq!(with_selection.args, without_selection.args);
        }
    }

    #[test]
    fn subtitle_selection_does_not_survive_settings_serialization() {
        let settings = ExportSettings {
            operation: Operation::Remux,
            subtitle_files: vec![PathBuf::from("private-subtitle.srt")],
            ..Default::default()
        };
        let value = serde_json::to_value(&settings).unwrap();
        assert!(value.get("subtitle_files").is_none());
        let restored: ExportSettings = serde_json::from_value(value).unwrap();
        assert!(restored.subtitle_files.is_empty());
        let supplied: ExportSettings =
            serde_json::from_value(serde_json::json!({"subtitle_files": ["old.srt"]})).unwrap();
        assert!(supplied.subtitle_files.is_empty());
    }

    fn stream_info(toolchain: &Toolchain, path: &Path) -> Vec<Value> {
        let result = command(&toolchain.ffprobe)
            .args(["-v", "error", "-show_streams", "-of", "json"])
            .arg(path)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        serde_json::from_slice::<Value>(&result.stdout).unwrap()["streams"]
            .as_array()
            .unwrap()
            .clone()
    }

    fn encoded_stream_hash(toolchain: &Toolchain, path: &Path, selector: &str) -> Vec<u8> {
        let result = command(&toolchain.ffmpeg)
            .args(["-v", "error", "-nostdin", "-i"])
            .arg(path)
            .args([
                "-map", selector, "-c", "copy", "-f", "hash", "-hash", "sha256", "pipe:1",
            ])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(result.stdout.starts_with(b"SHA256="));
        result.stdout
    }

    #[test]
    fn live_ffmpeg_remux_subtitles_preserve_av_and_mkv_styles_when_installed() {
        let Ok(toolchain) = discover(None) else {
            return;
        };
        let mut fixture = Fixture::new();
        let subtitles = subtitle_fixture(&fixture);
        for source_format in ["mp4", "webm"] {
            let input = fixture
                .directory
                .join(format!("with-embedded.{source_format}"));
            let mut cmd = command(&toolchain.ffmpeg);
            cmd.args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-n",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=256x144:rate=12:duration=1.5",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=1.5",
                "-i",
            ])
            .arg(&subtitles[0])
            .args(["-map", "0:v:0", "-map", "1:a:0", "-map", "2:s:0"]);
            if source_format == "webm" {
                cmd.args([
                    "-c:v",
                    "libvpx-vp9",
                    "-deadline",
                    "realtime",
                    "-cpu-used",
                    "8",
                    "-c:a",
                    "libopus",
                    "-c:s",
                    "webvtt",
                ]);
            } else {
                cmd.args([
                    "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-c:s", "mov_text",
                ]);
            }
            let result = cmd.arg(&input).output().unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            fixture.media = probe(&toolchain, &input).unwrap();
            let original_video = encoded_stream_hash(&toolchain, &input, "0:v:0");
            let original_audio = encoded_stream_hash(&toolchain, &input, "0:a:0");
            let formats: &[&str] = if source_format == "webm" {
                &["webm"]
            } else {
                &["mkv", "mp4", "mov"]
            };
            for format in formats {
                let settings = ExportSettings {
                    operation: Operation::Remux,
                    format: (*format).into(),
                    subtitle_files: subtitles.clone(),
                    ..Default::default()
                };
                let job = plan(&toolchain, &fixture.media, &settings).unwrap();
                let output = run(job, Arc::new(AtomicBool::new(false)), |_| {})
                    .unwrap_or_else(|error| panic!("{format}: {error}"));
                let streams = stream_info(&toolchain, &output);
                let tracks: Vec<_> = streams
                    .iter()
                    .filter(|s| s["codec_type"] == "subtitle")
                    .collect();
                // The pre-existing embedded subtitle is excluded, leaving only the four selections.
                assert_eq!(tracks.len(), subtitles.len(), "{format}");
                for (index, track) in tracks.iter().enumerate() {
                    let expected = match *format {
                        "mkv" => {
                            stream_info(&toolchain, &subtitles[index])[0]["codec_name"].clone()
                        }
                        "webm" => Value::String("webvtt".into()),
                        _ => Value::String("mov_text".into()),
                    };
                    assert_eq!(track["codec_name"], expected, "{format}: {index}");
                    assert_eq!(track["disposition"]["forced"], 0);
                }
                assert_eq!(
                    encoded_stream_hash(&toolchain, &output, "0:v:0"),
                    original_video,
                    "video was changed in {format}"
                );
                assert_eq!(
                    encoded_stream_hash(&toolchain, &output, "0:a:0"),
                    original_audio,
                    "audio was changed in {format}"
                );
                if *format == "mkv" {
                    assert_eq!(tracks[0]["tags"]["title"], "中文 ' 字幕");
                    let extracted = command(&toolchain.ffmpeg)
                        .args(["-v", "error", "-nostdin", "-i"])
                        .arg(&output)
                        .args(["-map", "0:s:1", "-c:s", "copy", "-f", "ass", "pipe:1"])
                        .output()
                        .unwrap();
                    assert!(extracted.status.success());
                    let text = String::from_utf8_lossy(&extracted.stdout);
                    assert!(
                        text.contains("Style: FrameflowAccent,Arial,28,&H0000FF00"),
                        "ASS style was lost: {text}"
                    );
                    assert!(
                        text.contains("{\\b1}样式字幕"),
                        "ASS override was lost: {text}"
                    );
                }
            }
        }
    }

    #[test]
    fn probe_excludes_album_art_and_recognizes_rotation() {
        let json = br#"{"streams":[{"codec_type":"video","codec_name":"png","disposition":{"attached_pic":1}},{"codec_type":"audio","codec_name":"aac"},{"codec_type":"video","codec_name":"h264","width":1920,"height":1080,"side_data_list":[{"rotation":-90}]}],"format":{"duration":"12.5"}}"#;
        let media = parse_probe(json, "movie.mp4".into(), 42).unwrap();
        assert_eq!(media.video_codec.as_deref(), Some("h264"));
        assert_eq!((media.width, media.height), (1080, 1920));
        assert_eq!(media.duration, 12.5);
    }

    #[test]
    fn publishing_and_planning_never_replace_existing_files() {
        let fixture = Fixture::new();
        let settings = ExportSettings::default();
        let first = plan(&tools(), &fixture.media, &settings).unwrap();
        fs::write(&first.output, b"existing file").unwrap();
        let second = plan(&tools(), &fixture.media, &settings).unwrap();
        assert_ne!(first.output, second.output);
        let staged = fixture.directory.join("stage.mp4");
        fs::write(&staged, b"new file").unwrap();
        let actual = publish(&staged, &first.output, &AtomicBool::new(false)).unwrap();
        assert_ne!(actual, first.output);
        assert_eq!(fs::read(&first.output).unwrap(), b"existing file");
        assert_eq!(fs::read(actual).unwrap(), b"new file");
        assert_eq!(fs::read(&fixture.media.path).unwrap(), b"fixture");
    }

    #[test]
    fn cancellation_before_start_creates_no_output() {
        let fixture = Fixture::new();
        let job = plan(&tools(), &fixture.media, &ExportSettings::default()).unwrap();
        let output = job.output.clone();
        assert!(
            run(job, Arc::new(AtomicBool::new(true)), |_| {})
                .unwrap_err()
                .contains("取消")
        );
        assert!(!output.exists());
    }

    #[test]
    fn live_ffmpeg_conversion_and_cancellation_when_installed() {
        let Ok(toolchain) = discover(None) else {
            return;
        };
        let mut fixture = Fixture::new();
        let input = fixture.directory.join("generated.wav");
        let result = command(&toolchain.ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-n",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=1",
                "-c:a",
                "pcm_s16le",
            ])
            .arg(&input)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        fixture.media = probe(&toolchain, &input).unwrap();
        let settings = ExportSettings {
            operation: Operation::Audio,
            format: "flac".into(),
            ..Default::default()
        };
        let job = plan(&toolchain, &fixture.media, &settings).unwrap();
        let exported = run(job, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        assert!(probe(&toolchain, &exported).unwrap().duration > 0.9);

        let mut job = plan(&toolchain, &fixture.media, &settings).unwrap();
        let input_option = job.args.iter().position(|s| s == "-i").unwrap();
        job.args.insert(input_option, "-re".into());
        let output = job.output.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let signal = Arc::clone(&cancel);
        let (started, ready) = mpsc::channel();
        let worker = thread::spawn(move || {
            run(job, cancel, move |_| {
                let _ = started.send(());
            })
        });
        ready
            .recv_timeout(Duration::from_secs(5))
            .expect("FFmpeg must start before cancellation");
        signal.store(true, Ordering::Relaxed);
        assert!(worker.join().unwrap().unwrap_err().contains("取消"));
        assert!(!output.exists());
        assert!(fixture.directory.read_dir().unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".ffmpeg-studio-")
        }));
    }

    #[test]
    fn live_ffmpeg_wav_millisecond_cut_preserves_exact_samples_when_installed() {
        let Ok(toolchain) = discover(None) else {
            return;
        };
        let mut fixture = Fixture::new();
        // 20 ms of deterministic mono PCM at 48 kHz. Every sample identifies its position.
        let original: Vec<u8> = (0_i16..960).flat_map(i16::to_le_bytes).collect();
        let raw = fixture.directory.join("sample-ramp.s16le");
        let input = fixture.directory.join("sample-ramp.wav");
        fs::write(&raw, &original).unwrap();
        let generated = command(&toolchain.ffmpeg)
            .args([
                "-v", "error", "-nostdin", "-n", "-f", "s16le", "-ar", "48000", "-ac", "1", "-i",
            ])
            .arg(&raw)
            .args(["-c:a", "pcm_s16le"])
            .arg(&input)
            .output()
            .unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        fixture.media = probe(&toolchain, &input).unwrap();
        let settings = ExportSettings {
            operation: Operation::Audio,
            format: "wav".into(),
            start_seconds: 0.001,
            end_seconds: 0.004,
            ..Default::default()
        };
        let job = plan(&toolchain, &fixture.media, &settings).unwrap();
        let exported = run(job, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        let decoded = command(&toolchain.ffmpeg)
            .args(["-v", "error", "-nostdin", "-i"])
            .arg(&exported)
            .args([
                "-map",
                "0:a:0",
                "-c:a",
                "pcm_s16le",
                "-f",
                "s16le",
                "pipe:1",
            ])
            .output()
            .unwrap();
        assert!(
            decoded.status.success(),
            "{}",
            String::from_utf8_lossy(&decoded.stderr)
        );
        // A 1 ms seek skips exactly 48 samples; a 3 ms interval contains exactly 144 samples.
        // This PCM guarantee is intentionally not asserted for frame-based video/GIF output.
        assert_eq!(decoded.stdout.len(), 144 * 2);
        assert_eq!(decoded.stdout, original[48 * 2..192 * 2]);
    }

    #[test]
    fn live_ffmpeg_all_operations_when_installed() {
        let Ok(toolchain) = discover(None) else {
            return;
        };
        let mut fixture = Fixture::new();
        let input = fixture.directory.join("generated.mp4");
        let result = command(&toolchain.ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-n",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=256x144:rate=24:duration=2",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=2",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-shortest",
            ])
            .arg(&input)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        fixture.media = probe(&toolchain, &input).unwrap();
        for operation in Operation::ALL
            .into_iter()
            .filter(|op| *op != Operation::Subtitle)
        {
            let settings = ExportSettings {
                operation,
                format: operation.formats()[0].into(),
                start_seconds: 0.25,
                end_seconds: 1.25,
                resolution: 120,
                fps: 12,
                ..Default::default()
            };
            let job = plan(&toolchain, &fixture.media, &settings).unwrap();
            let output = run(job, Arc::new(AtomicBool::new(false)), |_| {})
                .unwrap_or_else(|error| panic!("{operation:?}: {error}"));
            let exported = probe(&toolchain, &output).unwrap();
            assert!(exported.size > 0, "{operation:?}");
            match operation {
                Operation::Remux => {
                    assert_eq!(exported.width, 256);
                    assert!(exported.duration > 1.9);
                }
                Operation::Audio => {
                    assert!(exported.video_codec.is_none());
                    assert!(exported.audio_codec.is_some());
                    assert!((exported.duration - 1.0).abs() < 0.2);
                }
                Operation::Snapshot => assert_eq!(exported.height, 120),
                _ => {
                    assert_eq!(exported.height, 120);
                    assert!(
                        (exported.duration - 1.0).abs() < 0.2,
                        "{operation:?}: {}",
                        exported.duration
                    );
                }
            }
        }
    }
    #[test]
    fn encoding_defaults_and_unavailable_gpu_are_safe() {
        let settings: ExportSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings.video_encoder, VideoEncoder::Cpu);
        assert_eq!(settings.video_codec, VideoCodec::H264);
        assert_eq!(settings.audio_bitrate, 192);
        let fixture = Fixture::new();
        let mut settings = ExportSettings {
            video_encoder: VideoEncoder::Nvidia,
            ..Default::default()
        };
        assert!(
            plan(&tools(), &fixture.media, &settings)
                .unwrap_err()
                .contains("不可用")
        );
        settings.format = "webm".into();
        assert!(
            plan(&tools(), &fixture.media, &settings)
                .unwrap_err()
                .contains("WebM")
        );
        settings.operation = Operation::Audio;
        settings.format = "mp3".into();
        let args = strings(&plan(&tools(), &fixture.media, &settings).unwrap());
        assert!(!args.iter().any(|s| s == "-c:v"));
        settings.format = "ogg".into();
        settings.audio_bitrate = 32;
        assert!(
            plan(&tools(), &fixture.media, &settings)
                .unwrap_err()
                .contains("64 kbps")
        );
    }

    #[test]
    fn gpu_quality_flags_match_each_encoder_and_codec() {
        let fixture = Fixture::new();
        for encoder in VideoEncoder::ALL {
            for codec in [VideoCodec::H264, VideoCodec::H265] {
                let mut tools = tools();
                tools.gpu_encoders.push((encoder, codec));
                let settings = ExportSettings {
                    video_encoder: encoder,
                    video_codec: codec,
                    ..Default::default()
                };
                let args = strings(&plan(&tools, &fixture.media, &settings).unwrap());
                assert!(
                    args.windows(2)
                        .any(|pair| pair == ["-c:v", encoder.codec_name(codec)])
                );
                let flag = match encoder {
                    VideoEncoder::Cpu => "-crf",
                    VideoEncoder::Nvidia => "-cq",
                    VideoEncoder::Intel => "-global_quality",
                    VideoEncoder::Amd => "-qp_p",
                };
                assert!(args.windows(2).any(|pair| pair == [flag, "23"]));
                if codec == VideoCodec::H265 {
                    assert!(args.windows(2).any(|pair| pair == ["-tag:v", "hvc1"]));
                }
            }
        }
    }

    #[test]
    fn live_ffmpeg_cpu_gpu_and_audio_compression_when_installed() {
        let Ok(mut tools) = discover(None) else {
            assert!(
                env::var_os("FRAMEFLOW_REQUIRE_GPU").is_none(),
                "Release QA requires FFmpeg"
            );
            return;
        };
        tools.gpu_encoders = detect_gpu_encoders(&tools);
        if env::var_os("FRAMEFLOW_REQUIRE_GPU").is_some() {
            assert!(
                tools
                    .gpu_encoders
                    .contains(&(VideoEncoder::Nvidia, VideoCodec::H264))
            );
            assert!(
                tools
                    .gpu_encoders
                    .contains(&(VideoEncoder::Nvidia, VideoCodec::H265))
            );
        }
        eprintln!("Live GPU encoders: {:?}", tools.gpu_encoders);
        let fixture = Fixture::new();
        let source = fixture.directory.join("gpu audio sample.mkv");
        let result = command(&tools.ffmpeg)
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=512x512:rate=30:duration=1.5",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000:duration=1.5",
                "-c:v",
                "libx264",
                "-c:a",
                "pcm_s16le",
            ])
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let media = probe(&tools, &source).unwrap();
        let mut encoders = vec![
            (VideoEncoder::Cpu, VideoCodec::H264),
            (VideoEncoder::Cpu, VideoCodec::H265),
        ];
        encoders.extend(tools.gpu_encoders.iter().copied());
        for (encoder, codec) in encoders {
            for operation in [Operation::Convert, Operation::Compress, Operation::Trim] {
                let settings = ExportSettings {
                    operation,
                    video_encoder: encoder,
                    video_codec: codec,
                    start_seconds: 0.101,
                    end_seconds: 1.101,
                    resolution: 256,
                    fps: 24,
                    audio_bitrate: 64,
                    ..Default::default()
                };
                let output = run(
                    plan(&tools, &media, &settings).unwrap(),
                    Arc::new(AtomicBool::new(false)),
                    |_| {},
                )
                .unwrap();
                let info = probe(&tools, &output).unwrap();
                assert_eq!(
                    info.video_codec.as_deref(),
                    Some(if codec == VideoCodec::H264 {
                        "h264"
                    } else {
                        "hevc"
                    })
                );
                assert_eq!(info.audio_codec.as_deref(), Some("aac"));
                assert_eq!((info.width, info.height), (256, 256));
                assert!((info.duration - 1.0).abs() < 0.15);
                eprintln!("Passed {operation:?} {encoder:?} {codec:?}");
            }
        }
        let mut pure_audio = None;
        for format in Operation::Audio.formats() {
            let settings = ExportSettings {
                operation: Operation::Audio,
                format: (*format).into(),
                audio_bitrate: 64,
                ..Default::default()
            };
            let output = run(
                plan(&tools, &media, &settings).unwrap(),
                Arc::new(AtomicBool::new(false)),
                |_| {},
            )
            .unwrap();
            let info = probe(&tools, &output).unwrap();
            assert!(info.video_codec.is_none());
            assert!(info.audio_codec.is_some());
            assert!((info.duration - 1.5).abs() < 0.25);
            if *format == "wav" {
                pure_audio = Some(info);
            }
            eprintln!("Passed audio {format}");
        }
        let media = pure_audio.unwrap();
        let mut sizes = Vec::new();
        for bitrate in [64, 192] {
            let settings = ExportSettings {
                operation: Operation::Audio,
                format: "mp3".into(),
                audio_bitrate: bitrate,
                ..Default::default()
            };
            let output = run(
                plan(&tools, &media, &settings).unwrap(),
                Arc::new(AtomicBool::new(false)),
                |_| {},
            )
            .unwrap();
            sizes.push(fs::metadata(output).unwrap().len());
        }
        assert!(sizes[0] < sizes[1]);
    }
}
