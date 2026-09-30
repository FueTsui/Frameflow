use crate::{
    icons::{self, Icon},
    media::{
        self, ExportSettings, MediaInfo, Operation, ProgressEvent, Toolchain, VideoCodec,
        VideoEncoder,
    },
    software_update::SoftwareUpdate,
    theme::{self, Palette},
    timecode::{self, TimeFormat},
    updater,
};

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
use eframe::egui::{
    self, Align, Align2, Color32, FontId, Frame, Layout, Margin, Rect, RichText, Sense, Stroke, Ui,
    pos2, vec2,
};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::Duration,
};

#[derive(Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}
impl Appearance {
    fn label(self) -> &'static str {
        match self {
            Self::System => "跟随系统",
            Self::Light => "浅色",
            Self::Dark => "深色",
        }
    }
    fn preference(self) -> egui::ThemePreference {
        match self {
            Self::System => egui::ThemePreference::System,
            Self::Light => egui::ThemePreference::Light,
            Self::Dark => egui::ThemePreference::Dark,
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Preferences {
    appearance: Appearance,
    tools_dir: Option<PathBuf>,
    export: ExportSettings,
    auto_check_updates: bool,
    auto_check_app_updates: bool,
    sidebar_collapsed: bool,
    workflow_settings: Vec<ExportSettings>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            appearance: Appearance::System,
            tools_dir: None,
            export: ExportSettings::default(),
            auto_check_updates: true,
            auto_check_app_updates: true,
            sidebar_collapsed: false,
            workflow_settings: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum UpdateStatus {
    Idle,
    Checking,
    Available,
    Current,
    Installing,
    Installed,
    Failed,
}

#[derive(Clone, Copy, PartialEq)]
enum Status {
    Analyzing,
    Ready,
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}
impl Status {
    fn label(self) -> &'static str {
        match self {
            Self::Analyzing => "读取中",
            Self::Ready => "待处理",
            Self::Queued => "排队中",
            Self::Running => "处理中",
            Self::Done => "已完成",
            Self::Failed => "失败",
            Self::Cancelled => "已取消",
        }
    }
}
struct Entry {
    id: u64,
    path: PathBuf,
    media: Option<MediaInfo>,
    status: Status,
    progress: f32,
    speed: String,
    error: String,
    output: Option<PathBuf>,
    settings: Option<ExportSettings>,
}
enum Message {
    Tools(Result<Toolchain, String>),
    Files(Vec<PathBuf>),
    Subtitles(Vec<PathBuf>),
    AudioFiles(Vec<PathBuf>),
    AdvancedFiles(u8, Vec<PathBuf>),
    Imported(u64, Result<MediaInfo, String>),
    OutputDirectory(PathBuf),
    ToolsDirectory(PathBuf),
    DialogClosed,
    Progress(u64, ProgressEvent),
    Finished(u64, Result<PathBuf, String>),
    UpdateChecked {
        release: Result<updater::Release, String>,
        current_version: Option<String>,
    },
    UpdateProgress(String),
    UpdateInstalled(Result<PathBuf, String>),
}

#[derive(Clone, Copy, PartialEq)]
enum WorkspacePage {
    Files,
    Export,
    Preferences,
}

#[derive(Clone, Copy)]
enum Navigation {
    Page(WorkspacePage),
    Operation(Operation),
}

#[derive(Clone, Default)]
struct TimeDraft {
    text: String,
    committed: f64,
    invalid: bool,
}

impl TimeDraft {
    fn sync(&mut self, seconds: &mut f64, mode: TimeFormat) {
        if self.text.is_empty() && !self.invalid || *seconds != self.committed {
            self.text = timecode::format(*seconds, mode);
            self.committed = timecode::parse(&self.text, mode).unwrap_or(0.);
            *seconds = self.committed;
            self.invalid = false;
        }
    }
}

#[derive(Clone, Default)]
struct TimeInputs {
    start: TimeDraft,
    end: TimeDraft,
}

struct TimeEditRouting {
    mode: TimeFormat,
    owner: Option<egui::Id>,
    next_focus: Option<egui::Id>,
}

impl TimeEditRouting {
    fn new(ui: &Ui, mode: TimeFormat) -> Self {
        let editing = ui.input(|input| input.events.iter().any(time_edit_event));
        let owner = ui.memory(|memory| memory.focused()).filter(|id| {
            editing
                && ["开始", "结束", "位置"]
                    .iter()
                    .any(|label| *id == time_input_id(label))
        });
        Self {
            mode,
            owner,
            next_focus: None,
        }
    }

    fn finish(self, ui: &Ui) {
        if let Some(id) = self.next_focus {
            ui.memory_mut(|memory| memory.request_focus(id));
        }
    }
}

pub struct Frameflow {
    prefs: Preferences,
    tools: Option<Arc<Toolchain>>,
    tool_error: String,
    checking: bool,
    entries: Vec<Entry>,
    next_id: u64,
    selected: Option<u64>,
    tx: Sender<Message>,
    rx: Receiver<Message>,
    active: Option<(u64, Arc<AtomicBool>)>,
    worker: Option<thread::JoinHandle<()>>,
    batch: bool,
    command_open: bool,
    page: WorkspacePage,
    notice: Option<String>,
    logs: Vec<String>,
    dialog_open: bool,
    frames: u32,
    screenshot: Option<PathBuf>,
    screenshot_requested: bool,
    pending_files: Vec<PathBuf>,
    update_status: UpdateStatus,
    update_release: Option<updater::Release>,
    update_message: String,
    update_current_version: Option<String>,
    installed_update_dir: Option<PathBuf>,
    auto_update_checked: bool,
    software_update: SoftwareUpdate,
    time_inputs: TimeInputs,
    saved_time_inputs: Vec<(Operation, TimeInputs)>,
    pending_navigation: Option<Navigation>,
}

impl Frameflow {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut prefs: Preferences = cc
            .storage
            .and_then(|s| eframe::get_value(s, "preferences"))
            .unwrap_or_default();
        if std::env::var_os("FRAMEFLOW_SCREENSHOT").is_some() {
            prefs = Preferences::default();
        }
        if let Ok(value) = std::env::var("FRAMEFLOW_THEME") {
            prefs.appearance = match value.as_str() {
                "dark" => Appearance::Dark,
                "light" => Appearance::Light,
                _ => Appearance::System,
            };
        }
        cc.egui_ctx.set_theme(prefs.appearance.preference());
        theme::configure(&cc.egui_ctx);
        let (tx, rx) = mpsc::channel();
        let mut app = Self {
            prefs,
            tools: None,
            tool_error: String::new(),
            checking: false,
            entries: vec![],
            next_id: 1,
            selected: None,
            tx,
            rx,
            active: None,
            worker: None,
            batch: false,
            command_open: false,
            page: WorkspacePage::Files,
            notice: None,
            logs: vec![],
            dialog_open: false,
            frames: 0,
            screenshot: std::env::var_os("FRAMEFLOW_SCREENSHOT").map(PathBuf::from),
            screenshot_requested: false,
            pending_files: std::env::args_os().skip(1).map(PathBuf::from).collect(),
            update_status: UpdateStatus::Idle,
            update_release: None,
            update_message: String::new(),
            update_current_version: None,
            installed_update_dir: None,
            auto_update_checked: false,
            software_update: SoftwareUpdate::default(),
            time_inputs: TimeInputs::default(),
            saved_time_inputs: Vec::new(),
            pending_navigation: None,
        };
        if app.screenshot.is_some() {
            app.prefs.sidebar_collapsed =
                std::env::var("FRAMEFLOW_CAPTURE_COLLAPSED").as_deref() == Ok("1");
            if std::env::var("FRAMEFLOW_CAPTURE_GPU").as_deref() == Ok("1") {
                app.prefs.export.video_encoder = VideoEncoder::Nvidia;
                app.prefs.export.video_codec = VideoCodec::H265;
            }
            app.page = match std::env::var("FRAMEFLOW_CAPTURE_PAGE").as_deref() {
                Ok("settings") => WorkspacePage::Export,
                Ok("preferences") => WorkspacePage::Preferences,
                _ => WorkspacePage::Files,
            };
            if let Ok(value) = std::env::var("FRAMEFLOW_CAPTURE_OPERATION") {
                let operation = match value.as_str() {
                    "compress" => Operation::Compress,
                    "trim" => Operation::Trim,
                    "audio" => Operation::Audio,
                    "gif" => Operation::Gif,
                    "snapshot" => Operation::Snapshot,
                    "remux" => Operation::Remux,
                    "subtitle" => Operation::Subtitle,
                    _ => Operation::Convert,
                };
                app.change_operation(operation);
            }
            for (name, value) in [
                (
                    "FRAMEFLOW_CAPTURE_START",
                    &mut app.prefs.export.start_seconds,
                ),
                ("FRAMEFLOW_CAPTURE_END", &mut app.prefs.export.end_seconds),
            ] {
                if let Some(milliseconds) = std::env::var(name)
                    .ok()
                    .and_then(|value| value.parse::<u64>().ok())
                    .filter(|value| *value <= 864_000_000)
                {
                    *value = milliseconds as f64 / 1000.;
                }
            }
            match std::env::var("FRAMEFLOW_CAPTURE_ADVANCED").as_deref() {
                Ok("encoding") => {
                    app.prefs.export.encoding.video_rate_mode =
                        crate::encoding::VideoRateMode::TargetSize;
                    app.prefs.export.encoding.two_pass = true;
                }
                Ok("effects") => {
                    app.prefs.export.effects.rotation = crate::effects::Rotation::Clockwise90;
                    app.prefs.export.effects.watermark.enabled = true;
                    app.prefs.export.effects.composition.layout =
                        crate::effects::CompositionLayout::Horizontal;
                    app.prefs.export.effects.mix.enabled = true;
                }
                Ok("lossless") => app.prefs.export.lossless_trim = true,
                _ => {}
            }
            match std::env::var("FRAMEFLOW_CAPTURE_UPDATE").as_deref() {
                Ok("error") => {
                    app.update_status = UpdateStatus::Failed;
                    app.update_message = "暂时无法连接更新服务，请稍后重试。".into();
                }
                Ok("checking") => app.update_status = UpdateStatus::Checking,
                _ => {}
            }
        }
        app.sync_time_inputs();
        app.check_tools(&cc.egui_ctx);
        app
    }

    fn check_tools(&mut self, ctx: &egui::Context) {
        if self.checking || self.engine_change_locked() {
            return;
        }
        self.checking = true;
        if cfg!(test) {
            return;
        }
        let (tx, ctx, dir) = (self.tx.clone(), ctx.clone(), self.prefs.tools_dir.clone());
        thread::spawn(move || {
            let result = media::discover(dir.as_deref()).map(|mut tools| {
                tools.gpu_encoders = media::detect_gpu_encoders(&tools);
                tools
            });
            let _ = tx.send(Message::Tools(result));
            ctx.request_repaint();
        });
    }

    fn dialog(&mut self, ctx: &egui::Context, kind: u8) {
        if self.dialog_open
            || (kind >= 3 && self.batch)
            || (kind == 2 && (self.engine_change_locked() || self.checking))
        {
            return;
        }
        self.dialog_open = true;
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        thread::spawn(move || {
            let result = match kind {
                1 => rfd::FileDialog::new()
                    .set_title("选择输出文件夹")
                    .pick_folder()
                    .map(Message::OutputDirectory),
                2 => rfd::FileDialog::new()
                    .set_title("选择包含 ffmpeg 和 ffprobe 的文件夹")
                    .pick_folder()
                    .map(Message::ToolsDirectory),
                3 => rfd::FileDialog::new()
                    .set_title("添加字幕文件")
                    .add_filter("文本字幕", &["srt", "ass", "ssa", "vtt"])
                    .pick_files()
                    .map(Message::Subtitles),
                4 => rfd::FileDialog::new()
                    .set_title("添加音频文件")
                    .add_filter(
                        "音频文件",
                        &[
                            "mp3", "wav", "flac", "m4a", "aac", "ogg", "opus", "ac3", "eac3",
                            "mka", "aiff", "wma",
                        ],
                    )
                    .add_filter("所有文件", &["*"])
                    .pick_files()
                    .map(Message::AudioFiles),
                5 => rfd::FileDialog::new()
                    .set_title("选择水印图片")
                    .add_filter("图片", &["png", "jpg", "webp"])
                    .pick_file()
                    .map(|p| Message::AdvancedFiles(kind, vec![p])),
                6 => rfd::FileDialog::new()
                    .set_title("添加合成视频")
                    .pick_files()
                    .map(|p| Message::AdvancedFiles(kind, p)),
                7 => rfd::FileDialog::new()
                    .set_title("添加混音素材")
                    .pick_files()
                    .map(|p| Message::AdvancedFiles(kind, p)),
                8 => rfd::FileDialog::new()
                    .set_title("选择烧录字幕")
                    .add_filter("文本字幕", &["srt", "ass", "ssa", "vtt"])
                    .pick_file()
                    .map(|p| Message::AdvancedFiles(kind, vec![p])),
                9 => rfd::FileDialog::new()
                    .set_title("选择字幕字体文件夹")
                    .pick_folder()
                    .map(|p| Message::AdvancedFiles(kind, vec![p])),
                10 => rfd::FileDialog::new()
                    .set_title("添加字体附件")
                    .add_filter("字体", &["ttf", "otf", "ttc"])
                    .pick_files()
                    .map(|p| Message::AdvancedFiles(kind, p)),
                _ => rfd::FileDialog::new()
                    .set_title("添加媒体文件")
                    .add_filter(
                        "媒体文件",
                        &[
                            "mp4", "mkv", "mov", "avi", "webm", "m4v", "ts", "mts", "mp3", "wav",
                            "flac", "m4a", "ogg", "aac", "ac3", "tta", "gif", "png", "jpg", "webp",
                        ],
                    )
                    .add_filter("所有文件", &["*"])
                    .pick_files()
                    .map(Message::Files),
            };
            if let Some(m) = result {
                let _ = tx.send(m);
            }
            let _ = tx.send(Message::DialogClosed);
            ctx.request_repaint();
        });
    }

    fn import(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        if self.update_status == UpdateStatus::Installing || self.software_update.busy() {
            self.pending_files.extend(paths);
            self.notice = Some("更新期间暂不添加文件，请稍后再试。".into());
            return;
        }
        let Some(tools) = self.tools.clone() else {
            self.notice = Some("请先在设置中配置 FFmpeg，再添加媒体文件。".into());
            self.page = WorkspacePage::Preferences;
            return;
        };
        let mut jobs = vec![];
        for path in paths {
            let path = path.canonicalize().unwrap_or(path);
            if self.entries.iter().any(|e| e.path == path) {
                continue;
            }
            if !path.is_file() {
                self.notice = Some("请添加媒体文件，暂不支持导入整个文件夹。".into());
                continue;
            }
            let id = self.next_id;
            self.next_id += 1;
            self.entries.push(Entry {
                id,
                path: path.clone(),
                media: None,
                status: Status::Analyzing,
                progress: 0.,
                speed: String::new(),
                error: String::new(),
                output: None,
                settings: None,
            });
            self.selected = Some(id);
            jobs.push((id, path));
        }
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        thread::spawn(move || {
            for (id, path) in jobs {
                let _ = tx.send(Message::Imported(id, media::probe(&tools, &path)));
                ctx.request_repaint();
            }
        });
    }

    fn drain(&mut self, ctx: &egui::Context) {
        while let Ok(message) = self.rx.try_recv() {
            match message {
                Message::Tools(result) => {
                    self.checking = false;
                    match result {
                        Ok(t) => {
                            self.tool_error.clear();
                            self.tools = Some(Arc::new(t));
                            let files = std::mem::take(&mut self.pending_files);
                            if !files.is_empty() {
                                self.import(files, ctx);
                            }
                        }
                        Err(e) => {
                            self.tools = None;
                            self.tool_error = e;
                        }
                    }
                    if self.prefs.tools_dir.is_none()
                        && matches!(
                            self.update_status,
                            UpdateStatus::Available | UpdateStatus::Current
                        )
                    {
                        self.update_current_version =
                            self.tools.as_ref().map(|tools| tools.version.clone());
                        self.refresh_update_availability();
                    }
                }
                Message::UpdateChecked {
                    release,
                    current_version,
                } => {
                    if self.update_status == UpdateStatus::Checking {
                        self.update_current_version =
                            if self.prefs.tools_dir.is_none() && !self.checking {
                                self.tools
                                    .as_ref()
                                    .map(|tools| tools.version.clone())
                                    .or(current_version)
                            } else {
                                current_version
                            };
                        match release {
                            Ok(release) => {
                                self.update_release = Some(release);
                                self.refresh_update_availability();
                                self.update_message.clear();
                            }
                            Err(error) => {
                                self.update_status = UpdateStatus::Failed;
                                self.update_message = error;
                            }
                        }
                    }
                }
                Message::UpdateProgress(progress) => {
                    if self.update_status == UpdateStatus::Installing {
                        self.update_message = progress;
                    }
                }
                Message::UpdateInstalled(result) => {
                    if self.update_status == UpdateStatus::Installing {
                        match result {
                            Ok(directory) => {
                                self.installed_update_dir = Some(directory);
                                self.update_current_version = self
                                    .update_release
                                    .as_ref()
                                    .map(|release| release.version.clone());
                                self.update_status = UpdateStatus::Installed;
                                self.update_message.clear();
                            }
                            Err(error) => {
                                self.update_status = UpdateStatus::Failed;
                                self.update_message = error;
                            }
                        }
                        // Explicit custom engines stay selected; default discovery finds the managed update.
                        self.check_tools(ctx);
                    }
                }
                Message::Files(paths) => self.import(paths, ctx),
                Message::Subtitles(paths) => {
                    if !self.batch {
                        for path in paths {
                            if !self.prefs.export.subtitle_files.contains(&path) {
                                self.prefs.export.subtitle_files.push(path);
                            }
                        }
                    }
                }
                Message::AudioFiles(paths) => {
                    if !self.batch {
                        for path in paths {
                            if !self.prefs.export.audio_files.contains(&path) {
                                self.prefs.export.audio_files.push(path);
                            }
                        }
                    }
                }
                Message::AdvancedFiles(kind, paths) => {
                    if !self.batch {
                        match kind {
                            5 => {
                                self.prefs.export.effects.watermark.path = paths.into_iter().next()
                            }
                            6 => {
                                let remaining = 3usize.saturating_sub(
                                    self.prefs.export.effects.composition.files.len(),
                                );
                                if paths.len() > remaining {
                                    self.notice = Some("最多添加 3 个合成视频。".into());
                                }
                                self.prefs
                                    .export
                                    .effects
                                    .composition
                                    .files
                                    .extend(paths.into_iter().take(remaining));
                            }
                            7 => {
                                let remaining = 8usize
                                    .saturating_sub(self.prefs.export.effects.mix.tracks.len());
                                if paths.len() > remaining {
                                    self.notice = Some("最多添加 8 个混音文件。".into());
                                }
                                self.prefs.export.effects.mix.tracks.extend(
                                    paths
                                        .into_iter()
                                        .take(remaining)
                                        .map(crate::effects::MixTrack::new),
                                );
                            }
                            8 => self.prefs.export.tracks.burn_external = paths.into_iter().next(),
                            9 => self.prefs.export.tracks.fonts_dir = paths.into_iter().next(),
                            10 => self.prefs.export.tracks.font_attachments.extend(paths),
                            _ => {}
                        }
                    }
                }
                Message::Imported(id, result) => {
                    if let Some(e) = self.entries.iter_mut().find(|e| e.id == id) {
                        match result {
                            Ok(m) => {
                                if self.screenshot.is_some()
                                    && std::env::var("FRAMEFLOW_CAPTURE_ADVANCED").as_deref()
                                        == Ok("effects")
                                {
                                    self.prefs.export.effects.watermark.path =
                                        Some(m.path.with_extension("png"));
                                    self.prefs.export.effects.composition.files =
                                        vec![m.path.clone()];
                                    self.prefs.export.effects.mix.tracks =
                                        vec![crate::effects::MixTrack::new(m.path.clone())];
                                }
                                if self.screenshot.is_some()
                                    && std::env::var("FRAMEFLOW_CAPTURE_ADVANCED").as_deref()
                                        == Ok("tracks")
                                {
                                    self.prefs.export.tracks.audio = Some(
                                        m.tracks
                                            .iter()
                                            .filter(|t| t.kind == crate::tracks::TrackKind::Audio)
                                            .map(crate::tracks::TrackSelection::from)
                                            .collect(),
                                    );
                                    self.prefs.export.tracks.subtitles = m
                                        .tracks
                                        .iter()
                                        .filter(|t| t.kind == crate::tracks::TrackKind::Subtitle)
                                        .take(1)
                                        .map(crate::tracks::TrackSelection::from)
                                        .collect();
                                }
                                e.media = Some(m);
                                e.status = Status::Ready;
                            }
                            Err(err) => {
                                e.error = err;
                                e.status = Status::Failed;
                            }
                        }
                    }
                }
                Message::OutputDirectory(path) => self.prefs.export.output_dir = Some(path),
                Message::ToolsDirectory(path) => {
                    if !self.engine_change_locked() && !self.checking {
                        self.prefs.tools_dir = Some(path);
                        self.check_tools(ctx);
                    }
                }
                Message::DialogClosed => self.dialog_open = false,
                Message::Progress(id, event) => match event {
                    ProgressEvent::Progress { fraction, speed } => {
                        if let Some(e) = self.entries.iter_mut().find(|e| e.id == id) {
                            e.progress = fraction.clamp(0., 1.);
                            e.speed = speed;
                        }
                    }
                    ProgressEvent::Log(line) => {
                        self.logs.push(line);
                        if self.logs.len() > 240 {
                            self.logs.drain(..80);
                        }
                    }
                },
                Message::Finished(id, result) => {
                    let cancelled = self
                        .active
                        .as_ref()
                        .is_some_and(|(i, c)| *i == id && c.load(Ordering::Relaxed));
                    if let Some(e) = self.entries.iter_mut().find(|e| e.id == id) {
                        match result {
                            Ok(path) => {
                                e.output = Some(path);
                                e.progress = 1.;
                                e.status = Status::Done;
                            }
                            Err(err) => {
                                e.error = err;
                                e.status = if cancelled {
                                    Status::Cancelled
                                } else {
                                    Status::Failed
                                };
                            }
                        }
                    }
                    self.active = None;
                    if let Some(worker) = self.worker.take() {
                        let _ = worker.join();
                    }
                }
            }
        }
        if !self.checking && !self.auto_update_checked {
            self.auto_update_checked = true;
            if self.prefs.auto_check_updates && self.background_updates_allowed() {
                self.check_engine_updates(ctx);
            }
        }
        self.software_update.poll(
            ctx,
            self.prefs.auto_check_app_updates,
            self.background_updates_allowed(),
        );
        if !self.software_update.busy()
            && self.update_status != UpdateStatus::Installing
            && !self.checking
            && self.tools.is_some()
            && !self.pending_files.is_empty()
        {
            let paths = std::mem::take(&mut self.pending_files);
            self.import(paths, ctx);
        }
        if self.batch && self.active.is_none() {
            self.run_next(ctx);
        }
    }

    fn background_updates_allowed(&self) -> bool {
        updater::supported() && !cfg!(test) && self.screenshot.is_none()
    }

    fn engine_change_locked(&self) -> bool {
        self.active.is_some()
            || self.batch
            || self.update_status == UpdateStatus::Installing
            || self.software_update.busy()
            || self.entries.iter().any(|entry| {
                matches!(
                    entry.status,
                    Status::Analyzing | Status::Queued | Status::Running
                )
            })
    }

    fn check_engine_updates(&mut self, ctx: &egui::Context) {
        if !updater::supported()
            || matches!(
                self.update_status,
                UpdateStatus::Checking | UpdateStatus::Installing
            )
        {
            return;
        }
        self.auto_update_checked = true;
        self.update_status = UpdateStatus::Checking;
        self.update_release = None;
        self.update_message.clear();
        if !self.background_updates_allowed() {
            return;
        }
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        thread::spawn(move || {
            let release = updater::latest_release();
            let current_version = media::discover(None).ok().map(|tools| tools.version);
            let _ = tx.send(Message::UpdateChecked {
                release,
                current_version,
            });
            ctx.request_repaint();
        });
    }

    fn refresh_update_availability(&mut self) {
        if let Some(release) = &self.update_release {
            self.update_status = if self
                .update_current_version
                .as_ref()
                .is_none_or(|current| updater::is_newer(&release.version, current))
            {
                UpdateStatus::Available
            } else {
                UpdateStatus::Current
            };
        }
    }

    fn install_engine_update(&mut self, ctx: &egui::Context) {
        if !updater::supported()
            || self.engine_change_locked()
            || self.checking
            || self.dialog_open
            || !matches!(
                self.update_status,
                UpdateStatus::Available | UpdateStatus::Failed
            )
        {
            return;
        }
        let Some(release) = self.update_release.clone() else {
            return;
        };
        self.update_status = UpdateStatus::Installing;
        self.update_message = "正在准备下载…".into();
        if !self.background_updates_allowed() {
            return;
        }
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        thread::spawn(move || {
            let result = updater::install_release(&release, |message| {
                let _ = tx.send(Message::UpdateProgress(message));
                ctx.request_repaint();
            });
            let _ = tx.send(Message::UpdateInstalled(result));
            ctx.request_repaint();
        });
    }

    fn restore_default_engine(&mut self, ctx: &egui::Context) {
        if self.engine_change_locked() || self.checking || self.dialog_open {
            return;
        }
        self.prefs.tools_dir = None;
        self.check_tools(ctx);
    }

    fn start_batch(&mut self, ctx: &egui::Context) {
        self.normalize_workflow();
        self.sync_time_inputs();
        if let Some(error) = self.time_input_error() {
            self.notice = Some(error.into());
            self.page = WorkspacePage::Export;
            return;
        }
        if self.active.is_some()
            || self.tools.is_none()
            || self.dialog_open
            || self.checking
            || self.update_status == UpdateStatus::Installing
            || self.software_update.busy()
        {
            return;
        }
        if let Some(error) = self
            .tools
            .as_ref()
            .and_then(|tools| media::video_settings_error(tools, &self.prefs.export))
        {
            self.notice = Some(error);
            self.page = WorkspacePage::Export;
            return;
        }
        self.logs.clear();
        for e in &mut self.entries {
            if e.media.is_some()
                && matches!(e.status, Status::Ready | Status::Failed | Status::Cancelled)
            {
                e.status = Status::Queued;
                e.settings = Some(self.prefs.export.clone());
                e.error.clear();
                e.progress = 0.;
                e.speed.clear();
                e.output = None;
            }
        }
        if self
            .entries
            .iter()
            .any(|entry| entry.status == Status::Queued)
        {
            self.page = WorkspacePage::Files;
        }
        self.batch = true;
        self.run_next(ctx);
    }

    fn run_next(&mut self, ctx: &egui::Context) {
        let Some(tools) = self.tools.clone() else {
            self.batch = false;
            return;
        };
        while let Some(e) = self.entries.iter_mut().find(|e| e.status == Status::Queued) {
            let result = media::plan(
                &tools,
                e.media.as_ref().unwrap(),
                e.settings.as_ref().unwrap(),
            );
            match result {
                Err(error) => {
                    e.status = Status::Failed;
                    e.error = error;
                    continue;
                }
                Ok(plan) => {
                    let id = e.id;
                    e.status = Status::Running;
                    let cancel = Arc::new(AtomicBool::new(false));
                    self.active = Some((id, cancel.clone()));
                    let (tx, ctx) = (self.tx.clone(), ctx.clone());
                    self.worker = Some(thread::spawn(move || {
                        let (progress_tx, progress_ctx) = (tx.clone(), ctx.clone());
                        let result = media::run(plan, cancel, move |event| {
                            let _ = progress_tx.send(Message::Progress(id, event));
                            progress_ctx.request_repaint();
                        });
                        let _ = tx.send(Message::Finished(id, result));
                        ctx.request_repaint();
                    }));
                    return;
                }
            }
        }
        self.batch = false;
    }

    fn cancel(&mut self) {
        self.batch = false;
        if let Some((_, cancel)) = &self.active {
            cancel.store(true, Ordering::Relaxed);
        }
        for e in &mut self.entries {
            if e.status == Status::Queued {
                e.status = Status::Ready;
            }
        }
    }

    fn change_operation(&mut self, op: Operation) {
        if self.prefs.export.operation == op {
            return;
        }
        self.sync_time_inputs();
        let previous = self.prefs.export.operation;
        if time_mode(previous).is_some() {
            self.saved_time_inputs
                .retain(|(operation, _)| *operation != previous);
            self.saved_time_inputs
                .push((previous, self.time_inputs.clone()));
        }
        let output_dir = self.prefs.export.output_dir.clone();
        self.prefs
            .workflow_settings
            .retain(|s| s.operation != previous);
        self.prefs.workflow_settings.push(self.prefs.export.clone());
        self.prefs.export = self
            .prefs
            .workflow_settings
            .iter()
            .find(|s| s.operation == op)
            .cloned()
            .unwrap_or_else(|| workflow_defaults(op));
        self.prefs.export.output_dir = output_dir;
        self.time_inputs = self
            .saved_time_inputs
            .iter()
            .find(|(operation, _)| *operation == op)
            .map(|(_, inputs)| inputs.clone())
            .unwrap_or_default();
        if !self.time_inputs.start.text.is_empty() || self.time_inputs.start.invalid {
            self.prefs.export.start_seconds = self.time_inputs.start.committed;
            self.prefs.export.end_seconds = self.time_inputs.end.committed;
        }
        self.sync_time_inputs();
    }

    fn sync_time_inputs(&mut self) {
        if let Some(mode) = time_mode(self.prefs.export.operation) {
            self.time_inputs
                .start
                .sync(&mut self.prefs.export.start_seconds, mode);
            self.time_inputs
                .end
                .sync(&mut self.prefs.export.end_seconds, mode);
        }
    }

    fn time_input_error(&self) -> Option<&'static str> {
        let op = self.prefs.export.operation;
        time_mode(op)?;
        if self.time_inputs.start.invalid
            || (op != Operation::Snapshot && self.time_inputs.end.invalid)
        {
            Some("时间输入无效，请修正后再开始处理。")
        } else if op != Operation::Snapshot
            && self.prefs.export.end_seconds > 0.
            && self.prefs.export.end_seconds <= self.prefs.export.start_seconds
        {
            Some("结束时间须晚于开始时间，或全零表示到结尾。")
        } else {
            None
        }
    }

    fn apply_navigation(&mut self) {
        match self.pending_navigation.take() {
            Some(Navigation::Page(page)) => self.page = page,
            Some(Navigation::Operation(op)) => {
                if !self.batch {
                    self.change_operation(op);
                }
                if self.page == WorkspacePage::Preferences {
                    self.page = WorkspacePage::Files;
                }
            }
            None => {}
        }
    }

    fn sidebar(&mut self, ctx: &egui::Context, p: Palette) {
        egui::SidePanel::left("navigation")
            .exact_width(if self.prefs.sidebar_collapsed {
                72.
            } else {
                204.
            })
            .resizable(false)
            .frame(
                Frame::new()
                    .fill(p.panel)
                    .inner_margin(Margin::symmetric(12, 18)),
            )
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 4.;

                let toggle = if self.prefs.sidebar_collapsed {
                    "展开菜单"
                } else {
                    "收起菜单"
                };
                if navigation_item(ui, p, Icon::Menu, toggle, false).clicked() {
                    self.prefs.sidebar_collapsed = !self.prefs.sidebar_collapsed;
                }
                for op in Operation::ALL {
                    let enabled = !self.batch || self.prefs.export.operation == op;
                    let selected = self.page != WorkspacePage::Preferences
                        && self.prefs.export.operation == op;
                    let response = ui
                        .add_enabled_ui(enabled, |ui| {
                            navigation_item(ui, p, operation_icon(op), op.label(), selected)
                        })
                        .inner;
                    if response.clicked() {
                        self.pending_navigation = Some(Navigation::Operation(op));
                    }
                }
                ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
                    if navigation_item(
                        ui,
                        p,
                        Icon::Settings,
                        "设置",
                        self.page == WorkspacePage::Preferences,
                    )
                    .clicked()
                    {
                        self.pending_navigation =
                            Some(Navigation::Page(WorkspacePage::Preferences));
                    }
                });
            });
    }

    fn export_panel(&mut self, ctx: &egui::Context, p: Palette) -> bool {
        if self.page == WorkspacePage::Preferences {
            return false;
        }
        let mut start_requested = false;
        egui::TopBottomPanel::bottom("export-actions")
            .exact_height(104.)
            .frame(
                Frame::new()
                    .fill(p.card)
                    .inner_margin(Margin::symmetric(24, 12)),
            )
            .show(ctx, |ui| {
                let waiting = self
                    .entries
                    .iter()
                    .filter(|e| {
                        e.media.is_some()
                            && matches!(
                                e.status,
                                Status::Ready | Status::Failed | Status::Cancelled
                            )
                    })
                    .count();
                let summary = if let Some((id, _)) = &self.active {
                    self.entries
                        .iter()
                        .find(|entry| entry.id == *id)
                        .map(|entry| {
                            format!(
                                "正在处理 · {:.0}% · {}",
                                entry.progress * 100.,
                                entry.path.file_name().unwrap_or_default().to_string_lossy()
                            )
                        })
                        .unwrap_or_else(|| "正在处理…".into())
                } else {
                    format!(
                        "{} · {} 个待处理文件",
                        self.prefs.export.format.to_uppercase(),
                        waiting
                    )
                };
                ui.horizontal(|ui| {
                    let summary_width = ui.available_width();
                    ui.allocate_ui_with_layout(
                        vec2(summary_width, 18.),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            ui.add(
                                egui::Label::new(RichText::new(&summary).size(12.).color(p.text))
                                    .truncate()
                                    .show_tooltip_when_elided(false),
                            );
                        },
                    );
                });
                ui.add_space(9.);
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(!self.batch && !self.dialog_open, |ui| {
                        if icons::button(
                            ui,
                            Icon::Folder,
                            "选择文件夹",
                            "选择输出文件夹",
                            144.,
                            false,
                            p,
                        )
                        .clicked()
                        {
                            self.dialog(ctx, 1);
                        }
                    });
                    if self.prefs.export.output_dir.is_some()
                        && !self.batch
                        && icons::button(ui, Icon::Refresh, "", "恢复源文件夹", 36., false, p)
                            .clicked()
                    {
                        self.prefs.export.output_dir = None;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if self.active.is_some() {
                            if icons::button(
                                ui,
                                Icon::Stop,
                                "取消处理",
                                "取消当前任务及等待队列",
                                144.,
                                false,
                                p,
                            )
                            .clicked()
                            {
                                self.cancel();
                            }
                        } else {
                            ui.add_enabled_ui(
                                waiting > 0
                                    && self.tools.is_some()
                                    && !self.dialog_open
                                    && !self.checking
                                    && self.time_input_error().is_none()
                                    && self.update_status != UpdateStatus::Installing
                                    && !self.software_update.busy(),
                                |ui| {
                                    if icons::button(
                                        ui,
                                        Icon::Play,
                                        "开始处理",
                                        "处理所有待处理文件 · Ctrl+Enter",
                                        144.,
                                        true,
                                        p,
                                    )
                                    .clicked()
                                    {
                                        start_requested = true;
                                    }
                                },
                            );
                        }
                        if icons::button(
                            ui,
                            Icon::Code,
                            "命令与日志",
                            "查看所选文件的命令和处理日志",
                            144.,
                            false,
                            p,
                        )
                        .clicked()
                        {
                            self.command_open = true;
                        }
                    });
                });
            });
        start_requested
    }

    fn export_settings(&mut self, ui: &mut Ui, ctx: &egui::Context, p: Palette) {
        self.normalize_workflow();
        let op = self.prefs.export.operation;
        egui::ScrollArea::vertical()
            .id_salt(("export-settings", op.label()))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(12., 12.);
                ui.add_enabled_ui(!self.batch, |ui| match op {
                    Operation::Convert => {
                        self.video_output_settings(ui, p, true);
                        self.video_details(ui, p, false);
                        self.audio_workflow(ui, ctx, p, true, false);
                        self.subtitle_workflow(ui, ctx, p, false, false);
                        self.visual_effects(ui, ctx, p);
                    }
                    Operation::Compress => {
                        settings_group(ui, p, "压缩目标", |ui| {
                            crate::encoding::video_rate_ui(ui, &mut self.prefs.export);
                        });
                        self.video_output_settings(ui, p, true);
                        settings_group(ui, p, "尺寸与帧率", |ui| {
                            self.picture_controls(ui, p, true)
                        });
                        self.audio_workflow(ui, ctx, p, true, false);
                        self.subtitle_workflow(ui, ctx, p, false, false);
                        self.visual_effects(ui, ctx, p);
                    }
                    Operation::Trim => {
                        self.time_settings(ui, p);
                        settings_card(
                            ui,
                            p,
                            "裁剪方式",
                            "无损裁剪受关键帧限制。",
                            |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    ui.selectable_value(
                                        &mut self.prefs.export.lossless_trim,
                                        false,
                                        "精确裁剪",
                                    );
                                    ui.selectable_value(
                                        &mut self.prefs.export.lossless_trim,
                                        true,
                                        "无损裁剪",
                                    );
                                });
                            },
                        );
                        let encode = !self.prefs.export.lossless_trim;
                        self.video_output_settings(ui, p, encode);
                        if encode {
                            self.video_details(ui, p, false);
                        }
                        self.audio_workflow(ui, ctx, p, encode, false);
                        self.subtitle_workflow(ui, ctx, p, false, false);
                        if encode {
                            self.visual_effects(ui, ctx, p);
                        }
                    }
                    Operation::Audio => {
                        self.time_settings(ui, p);
                        settings_group(ui, p, "输出音频", |ui| {
                            self.format_control(ui);
                            ui.separator();
                            crate::encoding::audio_ui(ui, &mut self.prefs.export);
                        });
                        self.audio_workflow(ui, ctx, p, false, true);
                    }
                    Operation::Gif => {
                        self.time_settings(ui, p);
                        settings_group(ui, p, "动画参数", |ui| {
                            self.picture_controls(ui, p, true)
                        });
                        self.visual_effects(ui, ctx, p);
                    }
                    Operation::Snapshot => {
                        self.time_settings(ui, p);
                        settings_group(ui, p, "输出图片", |ui| {
                            self.format_control(ui);
                            self.picture_controls(ui, p, false);
                        });
                        self.visual_effects(ui, ctx, p);
                    }
                    Operation::Remux => {
                        self.video_output_settings(ui, p, false);
                        self.audio_workflow(ui, ctx, p, false, true);
                        self.subtitle_workflow(ui, ctx, p, true, false);
                    }
                    Operation::Subtitle => {
                        self.subtitle_workflow(ui, ctx, p, true, true);
                        settings_group(ui, p, "字幕格式", |ui| {
                            self.format_control(ui);
                            ui.small("文本字幕：SRT / ASS / VTT；图形字幕：MKS。");
                        });
                    }
                });
            });
    }

    // Normalize only options made inapplicable by an explicit workflow/format choice.
    fn normalize_workflow(&mut self) {
        let s = &mut self.prefs.export;
        if s.muted && s.operation != Operation::Audio {
            s.tracks.audio = Some(Vec::new());
        }
        s.muted = false;
        if s.format != "mkv" {
            s.tracks.font_attachments.clear();
            s.tracks.preserve_attachments = false;
        }
        let burn = matches!(s.operation, Operation::Convert | Operation::Compress)
            || (s.operation == Operation::Trim && !s.lossless_trim);
        if !burn && s.tracks.mode == crate::tracks::SubtitleMode::Burn {
            s.tracks.mode = crate::tracks::SubtitleMode::Keep;
        }
        if (s.operation == Operation::Audio || (burn && s.effects.mix.enabled))
            && let Some(tracks) = &mut s.tracks.audio
        {
            tracks.truncate(1);
        }
        crate::encoding::normalize_ui_options(s);
    }

    fn capture_group(&self, group: &str) -> bool {
        self.screenshot.is_some()
            && std::env::var("FRAMEFLOW_CAPTURE_ADVANCED").as_deref() == Ok(group)
    }

    fn format_control(&mut self, ui: &mut Ui) {
        setting_row(ui, "文件格式", |ui| {
            egui::ComboBox::from_id_salt("format")
                .icon(crate::theme::combo_chevron)
                .width(220.)
                .selected_text(self.prefs.export.format.to_uppercase())
                .show_ui(ui, |ui| {
                    for format in self.prefs.export.operation.formats() {
                        ui.selectable_value(
                            &mut self.prefs.export.format,
                            (*format).into(),
                            format.to_uppercase(),
                        );
                    }
                });
        });
        self.normalize_workflow();
    }

    fn video_output_settings(&mut self, ui: &mut Ui, p: Palette, encode: bool) {
        let capture = self.capture_group("encoding");
        let title = if self.prefs.export.operation == Operation::Remux {
            "封装格式"
        } else {
            "输出视频"
        };
        settings_group(ui, p, title, |ui| {
            self.format_control(ui);
            if !encode {
                ui.small("复制原始编码，容器须兼容源文件。");
                return;
            }
            if self.prefs.export.format == "webm" {
                self.prefs.export.video_encoder = VideoEncoder::Cpu;
                setting_row(ui, "视频编码", |ui| {
                    ui.label("VP9 · CPU");
                });
            } else {
                setting_row(ui, "视频编码", |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.selectable_value(
                            &mut self.prefs.export.video_codec,
                            VideoCodec::H264,
                            "H.264",
                        );
                        ui.selectable_value(
                            &mut self.prefs.export.video_codec,
                            VideoCodec::H265,
                            "H.265",
                        );
                    });
                });
                setting_row(ui, "执行设备", |ui| {
                    egui::ComboBox::from_id_salt("video-encoder")
                        .icon(crate::theme::combo_chevron)
                        .width(220.)
                        .selected_text(self.prefs.export.video_encoder.label())
                        .show_ui(ui, |ui| {
                            for encoder in VideoEncoder::ALL {
                                let available = encoder == VideoEncoder::Cpu
                                    || self.tools.as_ref().is_some_and(|tools| {
                                        tools
                                            .gpu_encoders
                                            .contains(&(encoder, self.prefs.export.video_codec))
                                    });
                                ui.add_enabled_ui(available && !self.checking, |ui| {
                                    ui.selectable_value(
                                        &mut self.prefs.export.video_encoder,
                                        encoder,
                                        if available {
                                            encoder.label().into()
                                        } else {
                                            format!("{}（不可用）", encoder.label())
                                        },
                                    );
                                });
                            }
                        });
                    if self.checking {
                        ui.small("正在检测 GPU…");
                    }
                });
            }
            if let Some(error) = self
                .tools
                .as_ref()
                .and_then(|tools| media::video_settings_error(tools, &self.prefs.export))
            {
                ui.colored_label(p.danger, error);
            }
            egui::CollapsingHeader::new("硬件选项")
                .icon(crate::theme::collapse_chevron)
                .default_open(capture)
                .show(ui, |ui| {
                    crate::encoding::hardware_ui(ui, &mut self.prefs.export);
                });
        });
    }

    fn video_details(&mut self, ui: &mut Ui, p: Palette, open: bool) {
        let open = open || self.capture_group("encoding");
        settings_expander(ui, p, "画质与尺寸", open, |ui| {
            crate::encoding::video_rate_ui(ui, &mut self.prefs.export);
            ui.separator();
            self.picture_controls(ui, p, true);
        });
    }

    fn picture_controls(&mut self, ui: &mut Ui, _p: Palette, fps: bool) {
        setting_row(ui, "画面尺寸", |ui| {
            egui::ComboBox::from_id_salt("resolution")
                .icon(crate::theme::combo_chevron)
                .width(220.)
                .selected_text(if self.prefs.export.resolution == 0 {
                    "保持原始".into()
                } else {
                    format!("{}p", self.prefs.export.resolution)
                })
                .show_ui(ui, |ui| {
                    for (value, label) in [
                        (0, "保持原始"),
                        (2160, "2160p · 4K"),
                        (1080, "1080p · 全高清"),
                        (720, "720p · 高清"),
                        (480, "480p"),
                        (360, "360p"),
                    ] {
                        ui.selectable_value(&mut self.prefs.export.resolution, value, label);
                    }
                });
        });
        if fps {
            setting_row(ui, "帧率", |ui| {
                egui::ComboBox::from_id_salt("fps")
                    .icon(crate::theme::combo_chevron)
                    .width(220.)
                    .selected_text(if self.prefs.export.fps == 0 {
                        "保持原始".into()
                    } else {
                        format!("{} fps", self.prefs.export.fps)
                    })
                    .show_ui(ui, |ui| {
                        for fps in [0, 10, 12, 15, 24, 25, 30, 60] {
                            ui.selectable_value(
                                &mut self.prefs.export.fps,
                                fps,
                                if fps == 0 {
                                    "保持原始".into()
                                } else {
                                    format!("{fps} fps")
                                },
                            );
                        }
                    });
            });
        }
    }

    fn time_settings(&mut self, ui: &mut Ui, p: Palette) {
        let op = self.prefs.export.operation;
        settings_card(
            ui,
            p,
            if op == Operation::Snapshot {
                "截取位置"
            } else {
                "时间范围"
            },
            if op == Operation::Snapshot {
                "时:分:秒:毫秒"
            } else {
                "结束全零表示到结尾。"
            },
            |ui| {
                let mut routing = TimeEditRouting::new(ui, time_mode(op).unwrap());
                time_input_row(
                    ui,
                    p,
                    if op == Operation::Snapshot {
                        "位置"
                    } else {
                        "开始"
                    },
                    &mut self.time_inputs.start,
                    &mut self.prefs.export.start_seconds,
                    &mut routing,
                );
                if op != Operation::Snapshot {
                    time_input_row(
                        ui,
                        p,
                        "结束",
                        &mut self.time_inputs.end,
                        &mut self.prefs.export.end_seconds,
                        &mut routing,
                    );
                }
                routing.finish(ui);
                if let Some(error) = self.time_input_error() {
                    ui.colored_label(p.danger, error);
                }
            },
        );
    }

    fn selected_tracks(&self) -> Vec<crate::tracks::TrackInfo> {
        self.selected
            .and_then(|id| self.entries.iter().find(|e| e.id == id))
            .or_else(|| self.entries.first())
            .and_then(|e| e.media.as_ref())
            .map(|m| m.tracks.clone())
            .unwrap_or_default()
    }

    fn audio_workflow(
        &mut self,
        ui: &mut Ui,
        ctx: &egui::Context,
        p: Palette,
        encode: bool,
        open: bool,
    ) {
        let op = self.prefs.export.operation;
        let tracks = self.selected_tracks();
        let active_mix =
            (encode || op == Operation::Audio) && self.prefs.export.effects.mix.enabled;
        let single = op == Operation::Audio || active_mix;
        let capture = self.capture_group("tracks")
            || self.capture_group("effects")
            || self.capture_group("encoding");
        let mut action = None;
        let title = if op == Operation::Audio {
            "音轨与混音"
        } else {
            "音频"
        };
        settings_expander(ui, p, title, open || capture, |ui| {
            crate::tracks::ui(
                ui,
                &mut self.prefs.export.tracks,
                &tracks,
                crate::tracks::TrackUiOptions {
                    audio: true,
                    subtitles: false,
                    extract: false,
                    burn: false,
                    single_audio: single,
                    attachments: false,
                },
            );
            let has_audio = !self
                .prefs
                .export
                .tracks
                .audio
                .as_ref()
                .is_some_and(Vec::is_empty)
                || active_mix;
            if encode && has_audio {
                ui.separator();
                crate::encoding::audio_ui(ui, &mut self.prefs.export);
            }
            if op == Operation::Remux {
                ui.separator();
                ui.label("外部音轨");
                self.audio_settings(ui, ctx, p);
            } else if encode || op == Operation::Audio {
                ui.separator();
                action = crate::effects::edit_ui(ui, &mut self.prefs.export.effects, false, true);
            }
        });
        self.effect_dialog(ctx, action);
    }

    fn subtitle_workflow(
        &mut self,
        ui: &mut Ui,
        ctx: &egui::Context,
        p: Palette,
        open: bool,
        extract: bool,
    ) {
        let tracks = self.selected_tracks();
        let burn = matches!(
            self.prefs.export.operation,
            Operation::Convert | Operation::Compress
        ) || (self.prefs.export.operation == Operation::Trim
            && !self.prefs.export.lossless_trim);
        let attachments = self.prefs.export.format == "mkv" && !extract;
        let capture = self.capture_group("tracks");
        let mut action = None;
        settings_expander(
            ui,
            p,
            if extract { "选择字幕" } else { "字幕" },
            open || capture,
            |ui| {
                action = crate::tracks::ui(
                    ui,
                    &mut self.prefs.export.tracks,
                    &tracks,
                    crate::tracks::TrackUiOptions {
                        audio: false,
                        subtitles: true,
                        extract,
                        burn,
                        single_audio: false,
                        attachments,
                    },
                );
                if !extract && self.prefs.export.tracks.mode == crate::tracks::SubtitleMode::Keep {
                    ui.separator();
                    self.subtitle_settings(ui, ctx, p);
                }
            },
        );
        if let Some(action) = action {
            self.dialog(
                ctx,
                match action {
                    crate::tracks::Picker::BurnSubtitle => 8,
                    crate::tracks::Picker::FontsDirectory => 9,
                    crate::tracks::Picker::FontAttachments => 10,
                },
            );
        }
    }

    fn visual_effects(&mut self, ui: &mut Ui, ctx: &egui::Context, p: Palette) {
        let mut action = None;
        if self.capture_group("effects") {
            ui.scroll_to_cursor(Some(Align::TOP));
        }
        settings_group(ui, p, "画面编辑", |ui| {
            action = crate::effects::edit_ui(ui, &mut self.prefs.export.effects, true, false);
        });
        self.effect_dialog(ctx, action);
    }

    fn effect_dialog(&mut self, ctx: &egui::Context, action: Option<crate::effects::EffectAction>) {
        if let Some(action) = action {
            self.dialog(
                ctx,
                match action {
                    crate::effects::EffectAction::ChooseWatermark => 5,
                    crate::effects::EffectAction::AddVideos => 6,
                    crate::effects::EffectAction::AddMixAudio => 7,
                },
            );
        }
    }

    fn subtitle_settings(&mut self, ui: &mut Ui, ctx: &egui::Context, p: Palette) {
        ui.add_enabled_ui(!self.dialog_open, |ui| {
            if icons::button(
                ui,
                Icon::Plus,
                "添加字幕",
                "选择 SRT、ASS、SSA 或 VTT 字幕文件",
                146.,
                false,
                p,
            )
            .clicked()
            {
                self.dialog(ctx, 3);
            }
        });
        if !self.prefs.export.subtitle_files.is_empty() {
            let mut remove = None;
            egui::ScrollArea::vertical()
                .id_salt("subtitle-files")
                .max_height(176.)
                .show(ui, |ui| {
                    for (index, path) in self.prefs.export.subtitle_files.iter().enumerate() {
                        ui.push_id(("subtitle", index), |ui| {
                            ui.horizontal(|ui| {
                                let width =
                                    (ui.available_width() - 30. - ui.spacing().item_spacing.x)
                                        .max(40.);
                                ui.allocate_ui_with_layout(
                                    vec2(width, 36.),
                                    Layout::left_to_right(Align::Center),
                                    |ui| {
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(
                                                    path.file_name()
                                                        .unwrap_or_default()
                                                        .to_string_lossy(),
                                                )
                                                .size(12.),
                                            )
                                            .truncate()
                                            .show_tooltip_when_elided(false),
                                        );
                                    },
                                );
                                if icons::button(ui, Icon::Close, "", "移除字幕", 30., false, p)
                                    .clicked()
                                {
                                    remove = Some(index);
                                }
                            });
                        });
                    }
                });
            if let Some(index) = remove {
                self.prefs.export.subtitle_files.remove(index);
            }
        }
        ui.label(
            RichText::new("添加可切换字幕，应用于本批文件。")
                .size(11.)
                .color(p.muted),
        );
        if self.prefs.export.format != "mkv" {
            ui.label(
                RichText::new("保留 ASS / SSA 样式请选择 MKV。")
                    .size(11.)
                    .color(p.muted),
            );
        }
    }

    fn audio_settings(&mut self, ui: &mut Ui, ctx: &egui::Context, p: Palette) {
        ui.add_enabled_ui(!self.dialog_open, |ui| {
            if icons::button(
                ui,
                Icon::Plus,
                "添加音频",
                "选择 MP3、M4A、FLAC、WAV 等 音频文件",
                146.,
                false,
                p,
            )
            .clicked()
            {
                self.dialog(ctx, 4);
            }
        });
        if !self.prefs.export.audio_files.is_empty() {
            let mut remove = None;
            egui::ScrollArea::vertical()
                .id_salt("audio-files")
                .max_height(176.)
                .show(ui, |ui| {
                    for (index, path) in self.prefs.export.audio_files.iter().enumerate() {
                        ui.push_id(("audio", index), |ui| {
                            ui.horizontal(|ui| {
                                let width =
                                    (ui.available_width() - 30. - ui.spacing().item_spacing.x)
                                        .max(40.);
                                ui.allocate_ui_with_layout(
                                    vec2(width, 36.),
                                    Layout::left_to_right(Align::Center),
                                    |ui| {
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(
                                                    path.file_name()
                                                        .unwrap_or_default()
                                                        .to_string_lossy(),
                                                )
                                                .size(12.),
                                            )
                                            .truncate()
                                            .show_tooltip_when_elided(false),
                                        );
                                    },
                                );
                                if icons::button(ui, Icon::Close, "", "移除音频", 30., false, p)
                                    .clicked()
                                {
                                    remove = Some(index);
                                }
                            });
                        });
                    }
                });
            if let Some(index) = remove {
                self.prefs.export.audio_files.remove(index);
            }
        }
        ui.label(
            RichText::new("合并为独立音轨，不混音、不裁剪。")
                .size(11.)
                .color(p.muted),
        );
    }

    fn workspace(&mut self, ctx: &egui::Context, p: Palette) {
        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(p.bg)
                    .inner_margin(Margin::symmetric(24, 16)),
            )
            .show(ctx, |ui| {
                if self.page == WorkspacePage::Preferences {
                    self.preferences_page(ui, ctx, p);
                    return;
                }
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 18.;
                    for (page, label) in [
                        (WorkspacePage::Files, "文件与任务"),
                        (
                            WorkspacePage::Export,
                            workflow_tab(self.prefs.export.operation),
                        ),
                    ] {
                        let (rect, response) =
                            ui.allocate_exact_size(vec2(112., 36.), Sense::click());
                        if response.hovered() {
                            ui.painter().rect_filled(rect, 4., p.hover);
                        }
                        ui.painter().text(
                            rect.center(),
                            Align2::CENTER_CENTER,
                            label,
                            FontId::proportional(14.),
                            if self.page == page { p.text } else { p.muted },
                        );
                        if self.page == page {
                            ui.painter().rect_filled(
                                Rect::from_center_size(
                                    pos2(rect.center().x, rect.bottom() - 2.),
                                    vec2(34., 3.),
                                ),
                                2.,
                                p.accent,
                            );
                        }
                        if response.has_focus() {
                            ui.painter().rect_stroke(
                                rect,
                                4.,
                                Stroke::new(1_f32, p.accent),
                                egui::StrokeKind::Inside,
                            );
                        }
                        response.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::SelectableLabel,
                                true,
                                label,
                            )
                        });
                        if response.clicked() {
                            self.pending_navigation = Some(Navigation::Page(page));
                        }
                    }
                });
                ui.add_space(12.);
                if let Some(message) = self.notice.clone() {
                    Frame::new()
                        .fill(p.tint)
                        .corner_radius(6)
                        .inner_margin(12)
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(message).size(12.));
                                if ui.small_button("关闭").clicked() {
                                    self.notice = None;
                                }
                            });
                        });
                    ui.add_space(8.);
                }
                if self.page == WorkspacePage::Export {
                    self.export_settings(ui, ctx, p);
                } else {
                    Frame::new()
                        .fill(p.card)
                        .stroke(Stroke::new(1_f32, p.line))
                        .corner_radius(8)
                        .inner_margin(16)
                        .show(ui, |ui| {
                            ui.set_min_height((ui.available_height() - 2.).max(180.));
                            ui.set_min_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(format!(
                                        "处理队列 · {} 个文件",
                                        self.entries.len()
                                    ))
                                    .size(15.)
                                    .strong(),
                                );
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if icons::button(
                                        ui,
                                        Icon::Plus,
                                        "添加文件",
                                        "添加媒体文件 · Ctrl+O",
                                        116.,
                                        false,
                                        p,
                                    )
                                    .clicked()
                                    {
                                        self.dialog(ctx, 0);
                                    }
                                    if !self.entries.is_empty()
                                        && ui
                                            .add(
                                                egui::Button::new(
                                                    RichText::new("清除已完成")
                                                        .size(12.)
                                                        .color(p.muted),
                                                )
                                                .frame(false),
                                            )
                                            .clicked()
                                    {
                                        self.entries.retain(|e| e.status != Status::Done);
                                        if self.selected.is_some_and(|id| {
                                            !self.entries.iter().any(|e| e.id == id)
                                        }) {
                                            self.selected = self.entries.first().map(|e| e.id);
                                        }
                                    }
                                });
                            });
                            ui.add_space(4.);
                            ui.separator();
                            ui.add_space(8.);
                            if self.entries.is_empty() {
                                self.empty_state(ui, ctx, p);
                            } else {
                                self.file_list(ui, ctx, p);
                            }
                        });
                }
            });
    }

    fn empty_state(&mut self, ui: &mut Ui, ctx: &egui::Context, p: Palette) {
        let roomy = ui.available_height() > 310.;
        ui.vertical_centered(|ui| {
            ui.add_space(if roomy { 42. } else { 8. });
            let size = if roomy { 68. } else { 48. };
            let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
            ui.painter().rect_filled(rect, 12., p.tint);
            icons::paint(
                ui,
                rect.shrink(if roomy { 17. } else { 12. }),
                Icon::Folder,
                p.accent,
            );
            ui.add_space(if roomy { 14. } else { 7. });
            ui.label(RichText::new("添加媒体文件").size(21.).strong());
            ui.label(
                RichText::new("将文件拖到此处，或从设备中选择")
                    .size(12.)
                    .color(p.muted),
            );
            ui.add_space(8.);
            if icons::button(
                ui,
                Icon::Plus,
                "选择文件",
                "选择视频、音频或图像",
                136.,
                true,
                p,
            )
            .clicked()
            {
                self.dialog(ctx, 0);
            }
            if roomy {
                ui.add_space(12.);
                ui.label(
                    RichText::new("支持 MP4、MOV、MKV、MP3 等格式")
                        .size(11.)
                        .color(p.muted),
                );
                ui.add_space(32.);
                ui.label(
                    RichText::new("添加文件  →  调整导出设置  →  开始处理")
                        .size(12.)
                        .color(p.muted),
                );
            }
        });
    }

    fn file_list(&mut self, ui: &mut Ui, _ctx: &egui::Context, p: Palette) {
        let mut remove = None;
        let mut folder = None;
        let mut retry = None;
        egui::ScrollArea::vertical()
            .id_salt("files")
            .max_height((ui.available_height() - 86.).max(126.))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for e in &self.entries {
                    ui.push_id(e.id, |ui| {
                        let frame = Frame::new()
                            .fill(if self.selected == Some(e.id) {
                                p.tint
                            } else {
                                p.card
                            })
                            .stroke(Stroke::new(1_f32, p.line))
                            .corner_radius(6)
                            .inner_margin(12);
                        let response = frame
                            .show(ui, |ui| {
                                ui.set_min_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    let (r, _) =
                                        ui.allocate_exact_size(vec2(36., 44.), Sense::hover());
                                    ui.painter().rect_filled(r, 7., p.panel);
                                    icons::paint(
                                        ui,
                                        r.shrink2(vec2(8., 12.)),
                                        if e.media.as_ref().is_some_and(|m| m.video_codec.is_none())
                                        {
                                            Icon::Audio
                                        } else {
                                            Icon::File
                                        },
                                        p.accent,
                                    );
                                    ui.vertical(|ui| {
                                        ui.set_width((ui.available_width() - 112.).max(84.));
                                        let name = e
                                            .path
                                            .file_name()
                                            .unwrap_or_default()
                                            .to_string_lossy();
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(name).size(13.).strong(),
                                            )
                                            .truncate()
                                            .show_tooltip_when_elided(false),
                                        );
                                        let info = e
                                            .media
                                            .as_ref()
                                            .map(|m| {
                                                format!(
                                                    "{}  ·  {}  ·  {}",
                                                    duration(m.duration),
                                                    if m.width > 0 {
                                                        format!("{} × {}", m.width, m.height)
                                                    } else {
                                                        "音频".into()
                                                    },
                                                    size(m.size)
                                                )
                                            })
                                            .unwrap_or_else(|| "读取媒体信息…".into());
                                        ui.label(RichText::new(info).size(11.).color(p.muted));
                                    });
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if !matches!(
                                            e.status,
                                            Status::Running | Status::Queued | Status::Analyzing
                                        ) && icons::button(
                                            ui,
                                            Icon::Close,
                                            "",
                                            "移除文件",
                                            30.,
                                            false,
                                            p,
                                        )
                                        .clicked()
                                        {
                                            remove = Some(e.id);
                                        }
                                        if e.status == Status::Done {
                                            if icons::button(
                                                ui,
                                                Icon::Folder,
                                                "",
                                                "打开输出文件夹",
                                                30.,
                                                false,
                                                p,
                                            )
                                            .clicked()
                                            {
                                                folder = e.output.as_ref().and_then(|o| {
                                                    o.parent().map(|p| p.to_path_buf())
                                                });
                                            }
                                        } else {
                                            ui.label(
                                                RichText::new(e.status.label()).size(11.).color(
                                                    if e.status == Status::Failed {
                                                        p.danger
                                                    } else {
                                                        p.accent
                                                    },
                                                ),
                                            );
                                        }
                                    });
                                });
                                if e.status == Status::Running {
                                    ui.add_space(4.);
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(format!("{:.0}%", e.progress * 100.))
                                                .size(11.)
                                                .strong()
                                                .color(p.text),
                                        );
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
                                                ui.add(
                                                    egui::Label::new(
                                                        RichText::new(&e.speed)
                                                            .size(11.)
                                                            .color(p.muted),
                                                    )
                                                    .truncate()
                                                    .show_tooltip_when_elided(false),
                                                );
                                            },
                                        );
                                    });
                                    ui.add(
                                        egui::ProgressBar::new(e.progress)
                                            .fill(p.accent)
                                            .desired_width(ui.available_width())
                                            .desired_height(5.)
                                            .animate(false),
                                    );
                                }
                                if !e.error.is_empty() {
                                    ui.add_space(2.);
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&e.error).size(11.).color(p.danger),
                                        )
                                        .wrap(),
                                    );
                                }
                                if e.status == Status::Done {
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new("已保存").size(10.).color(p.accent));
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
                                                if ui.small_button("重新处理").clicked() {
                                                    retry = Some(e.id);
                                                }
                                                if let Some(path) = &e.output {
                                                    ui.add(
                                                        egui::Label::new(
                                                            RichText::new(
                                                                path.file_name()
                                                                    .unwrap_or_default()
                                                                    .to_string_lossy(),
                                                            )
                                                            .size(10.)
                                                            .color(p.muted),
                                                        )
                                                        .truncate()
                                                        .show_tooltip_when_elided(false),
                                                    );
                                                }
                                            },
                                        );
                                    });
                                }
                            })
                            .response;
                        if ui.rect_contains_pointer(response.rect)
                            && ui.input(|i| i.pointer.primary_clicked())
                        {
                            self.selected = Some(e.id);
                        }
                    });
                    ui.add_space(7.);
                }
            });
        if let Some(id) = remove {
            self.entries.retain(|e| e.id != id);
            if self.selected == Some(id) {
                self.selected = self.entries.first().map(|e| e.id);
            }
        }
        if let Some(id) = retry
            && let Some(e) = self.entries.iter_mut().find(|e| e.id == id)
        {
            e.status = Status::Ready;
        }
        if let Some(path) = folder {
            self.open_path(path);
        }
        ui.add_space(12.);
        if let Some(e) = self.entries.iter().find(|e| Some(e.id) == self.selected)
            && let Some(m) = &e.media
        {
            Frame::new()
                .fill(p.panel)
                .corner_radius(6)
                .inner_margin(12)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new("素材信息").size(11.).color(p.muted));
                    ui.horizontal_wrapped(|ui| {
                        for (label, value) in [
                            ("时长", duration(m.duration)),
                            ("视频", m.video_codec.clone().unwrap_or_else(|| "无".into())),
                            ("音频", m.audio_codec.clone().unwrap_or_else(|| "无".into())),
                        ] {
                            ui.label(RichText::new(format!("{label}  {value}")).size(12.));
                            ui.add_space(10.);
                        }
                    });
                });
        }
    }

    fn open_path(&mut self, path: impl AsRef<std::ffi::OsStr>) {
        if let Err(err) = open::that_detached(path) {
            self.notice = Some(format!("无法打开：{err}"));
        }
    }

    fn preferences_page(&mut self, ui: &mut Ui, ctx: &egui::Context, p: Palette) {
        egui::ScrollArea::vertical()
            .id_salt("preferences-page")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(12., 14.);
                if let Some(message) = self.notice.clone() {
                    Frame::new()
                        .fill(p.tint)
                        .corner_radius(6)
                        .inner_margin(12)
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(message).size(12.));
                                if ui.small_button("关闭").clicked() {
                                    self.notice = None;
                                }
                            });
                        });
                }
                if self.active.is_some() {
                    Frame::new()
                        .fill(p.tint)
                        .corner_radius(6)
                        .inner_margin(12)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("媒体任务正在后台处理。").size(12.));
                                if ui.button("返回任务").clicked() {
                                    self.page = WorkspacePage::Files;
                                }
                            });
                        });
                }
                settings_frame(p).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("应用主题").size(15.).strong());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let previous = self.prefs.appearance;
                            egui::ComboBox::from_id_salt("appearance")
                                .icon(theme::combo_chevron)
                                .width(160.)
                                .selected_text(self.prefs.appearance.label())
                                .show_ui(ui, |ui| {
                                    for mode in
                                        [Appearance::System, Appearance::Light, Appearance::Dark]
                                    {
                                        ui.selectable_value(
                                            &mut self.prefs.appearance,
                                            mode,
                                            mode.label(),
                                        );
                                    }
                                });
                            if self.prefs.appearance != previous {
                                ctx.set_theme(self.prefs.appearance.preference());
                            }
                        });
                    });
                });
                settings_group(ui, p, "媒体引擎", |ui| {
                    if self.checking {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("正在检测 FFmpeg…");
                        });
                    } else if let Some(tools) = &self.tools {
                        ui.label(RichText::new("引擎已就绪").size(13.).color(p.accent));
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!(
                                    "FFmpeg {}",
                                    updater::short_version(&tools.version)
                                ))
                                .size(12.),
                            )
                            .truncate()
                            .show_tooltip_when_elided(false),
                        );
                        let engine_path = if self.screenshot.is_some()
                            && std::env::var("FRAMEFLOW_CAPTURE_REDACT_PATHS").as_deref() == Ok("1")
                        {
                            std::borrow::Cow::Borrowed("<安装目录>\\tools\\ffmpeg.exe")
                        } else {
                            tools.ffmpeg.to_string_lossy()
                        };
                        ui.add(
                            egui::Label::new(
                                RichText::new(engine_path.as_ref()).size(12.).color(p.muted),
                            )
                            .truncate()
                            .show_tooltip_when_elided(false),
                        );
                    } else {
                        ui.label(
                            RichText::new("需要配置 ffmpeg 和 ffprobe")
                                .size(13.)
                                .color(p.danger),
                        );
                        if !self.tool_error.is_empty() {
                            ui.add(
                                egui::Label::new(RichText::new(&self.tool_error).size(12.)).wrap(),
                            );
                        }
                    }
                    ui.add_space(4.);
                    ui.add_enabled_ui(
                        !self.engine_change_locked() && !self.checking && !self.dialog_open,
                        |ui| {
                            ui.horizontal_wrapped(|ui| {
                                if icons::button(
                                    ui,
                                    Icon::Folder,
                                    "选择引擎文件夹",
                                    "选择同时包含 ffmpeg 和 ffprobe 的文件夹",
                                    185.,
                                    false,
                                    p,
                                )
                                .clicked()
                                {
                                    self.dialog(ctx, 2);
                                }
                                if icons::button(
                                    ui,
                                    Icon::Refresh,
                                    "重新检测",
                                    "重新检测 FFmpeg",
                                    128.,
                                    false,
                                    p,
                                )
                                .clicked()
                                {
                                    self.check_tools(ctx);
                                }
                                if self.prefs.tools_dir.is_some()
                                    && ui.button("恢复自动检测").clicked()
                                {
                                    self.restore_default_engine(ctx);
                                }
                            });
                        },
                    );
                    ui.label(
                        RichText::new(if cfg!(target_os = "windows") {
                            "优先使用更新版本，可选择其他引擎。"
                        } else {
                            "自动检测系统 PATH、程序旁的 tools / bin，以及 Homebrew 安装目录。"
                        })
                        .size(12.)
                        .color(p.muted),
                    );
                });
                if ui.available_width() >= 760. {
                    ui.columns(2, |columns| {
                        let mut engine_frame = settings_frame(p).begin(&mut columns[0]);
                        engine_frame.content_ui.push_id("engine-update", |ui| {
                            self.engine_update_settings(ui, ctx, p)
                        });
                        let mut software_frame = settings_frame(p).begin(&mut columns[1]);
                        software_frame.content_ui.push_id("software-update", |ui| {
                            self.software_update_settings(ui, ctx, p)
                        });
                        // Measure natural content once in this frame. Do not reuse padded
                        // heights from earlier frames or run interactive contents twice.
                        let content_bottom = engine_frame
                            .content_ui
                            .min_rect()
                            .bottom()
                            .max(software_frame.content_ui.min_rect().bottom());
                        // set_min_height is relative to the current cursor in egui,
                        // so extend the measured absolute bottom instead of adding space.
                        engine_frame.content_ui.expand_to_include_y(content_bottom);
                        software_frame
                            .content_ui
                            .expand_to_include_y(content_bottom);
                        engine_frame.end(&mut columns[0]);
                        software_frame.end(&mut columns[1]);
                    });
                } else {
                    settings_frame(p).show(ui, |ui| {
                        ui.push_id("engine-update", |ui| {
                            self.engine_update_settings(ui, ctx, p)
                        });
                    });
                    settings_frame(p).show(ui, |ui| {
                        ui.push_id("software-update", |ui| {
                            self.software_update_settings(ui, ctx, p)
                        });
                    });
                }
            });
    }

    fn software_update_settings(&mut self, ui: &mut Ui, ctx: &egui::Context, p: Palette) {
        let blocked = self.active.is_some()
            || self.batch
            || self.checking
            || self.dialog_open
            || self.update_status == UpdateStatus::Installing
            || self.entries.iter().any(|entry| {
                matches!(
                    entry.status,
                    Status::Analyzing | Status::Queued | Status::Running
                )
            });
        let network = self.background_updates_allowed();
        ui.set_min_width(ui.available_width());
        settings_header(ui, "软件更新", |ui| {
            SoftwareUpdate::header_ui(ui, p);
        });
        self.software_update.ui(
            ui,
            ctx,
            &mut self.prefs.auto_check_app_updates,
            blocked,
            network,
            p,
        );
    }

    fn engine_update_settings(&mut self, ui: &mut Ui, ctx: &egui::Context, p: Palette) {
        ui.set_min_width(ui.available_width());
        settings_header(ui, "FFmpeg 更新", |ui| {
            if ui.link("FFmpeg 下载页面").clicked() {
                self.open_path("https://ffmpeg.org/download.html");
            }
            if ui.link("使用文档").clicked() {
                self.open_path("https://github.com/FueTsui/Frameflow#readme");
            }
        });
        let automatic = ui.checkbox(&mut self.prefs.auto_check_updates, "自动检查更新");
        if automatic.changed() && self.prefs.auto_check_updates && self.background_updates_allowed()
        {
            self.check_engine_updates(ctx);
        }
        let release_version = self
            .update_release
            .as_ref()
            .map(|release| release.version.as_str())
            .unwrap_or("");
        let status = match self.update_status {
            UpdateStatus::Idle => "可检查并更新默认媒体引擎。".into(),
            UpdateStatus::Checking => "正在检查更新…".into(),
            UpdateStatus::Available => format!("发现新版本 FFmpeg {release_version}"),
            UpdateStatus::Current => {
                let current = self
                    .update_current_version
                    .as_ref()
                    .map(|version| updater::short_version(version))
                    .unwrap_or_else(|| release_version.into());
                if current == release_version {
                    format!("默认引擎已是最新版本（{current}）。")
                } else {
                    format!("当前默认引擎：{current}；最新稳定版本：{release_version}。")
                }
            }
            UpdateStatus::Installing => self.update_message.clone(),
            UpdateStatus::Installed => format!("默认引擎已更新至 FFmpeg {release_version}。"),
            UpdateStatus::Failed => format!("更新失败：{}", self.update_message),
        };
        ui.horizontal_wrapped(|ui| {
            if matches!(
                self.update_status,
                UpdateStatus::Checking | UpdateStatus::Installing
            ) {
                ui.spinner();
            }
            ui.add(
                egui::Label::new(RichText::new(status).size(13.).color(
                    if self.update_status == UpdateStatus::Failed {
                        p.danger
                    } else {
                        p.text
                    },
                ))
                .wrap(),
            );
        });
        if !updater::supported() {
            ui.label(
                RichText::new("此平台暂不支持内置更新，请手动安装 FFmpeg。")
                    .size(12.)
                    .color(p.muted),
            );
            return;
        }
        ui.horizontal_wrapped(|ui| {
            let can_check = !matches!(
                self.update_status,
                UpdateStatus::Checking | UpdateStatus::Installing
            );
            ui.add_enabled_ui(can_check, |ui| {
                let label = if self.update_status == UpdateStatus::Failed
                    && self.update_release.is_none()
                {
                    "重试检查"
                } else {
                    "检查更新"
                };
                if icons::button(
                    ui,
                    Icon::Refresh,
                    label,
                    "检查默认媒体引擎的新版本",
                    128.,
                    false,
                    p,
                )
                .clicked()
                {
                    self.check_engine_updates(ctx);
                }
            });
            if matches!(
                self.update_status,
                UpdateStatus::Available | UpdateStatus::Failed
            ) && self.update_release.is_some()
            {
                ui.add_enabled_ui(
                    !self.engine_change_locked() && !self.checking && !self.dialog_open,
                    |ui| {
                        let label = if self.update_status == UpdateStatus::Failed {
                            "重试更新"
                        } else {
                            "下载并更新"
                        };
                        if icons::button(
                            ui,
                            Icon::Play,
                            label,
                            "下载并安装到应用管理的引擎目录",
                            160.,
                            true,
                            p,
                        )
                        .clicked()
                        {
                            self.install_engine_update(ctx);
                        }
                    },
                );
            }
        });
        if self.update_status == UpdateStatus::Installing {
            ui.label(
                RichText::new("更新完成后可继续处理媒体文件。")
                    .size(12.)
                    .color(p.muted),
            );
        } else if self.engine_change_locked() && self.update_release.is_some() {
            ui.label(
                RichText::new("媒体任务完成后可安装更新。")
                    .size(12.)
                    .color(p.muted),
            );
        }
        if self.prefs.tools_dir.is_some() {
            ui.label(
                RichText::new("当前使用自定义引擎。更新仅安装默认引擎，不会覆盖自定义文件。")
                    .size(12.)
                    .color(p.muted),
            );
            if self.installed_update_dir.is_some() || updater::managed_engine_dir().is_some() {
                ui.add_enabled_ui(
                    !self.engine_change_locked() && !self.checking && !self.dialog_open,
                    |ui| {
                        if ui.button("使用更新后的默认引擎").clicked() {
                            self.restore_default_engine(ctx);
                        }
                    },
                );
            }
        }
    }

    fn render_ui(&mut self, ctx: &egui::Context, p: Palette) {
        self.sync_time_inputs();
        let shortcut_start =
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter));
        self.sidebar(ctx, p);
        let button_start = self.export_panel(ctx, p);
        self.workspace(ctx, p);
        if shortcut_start || button_start {
            // Text events must be applied before using the settings, including
            // a paste and Ctrl+Enter delivered in the same native frame.
            self.pending_navigation = None;
            self.start_batch(ctx);
        } else {
            self.apply_navigation();
        }
        if self.command_open {
            self.commands(ctx, p);
        }
    }

    fn commands(&mut self, ctx: &egui::Context, p: Palette) {
        self.normalize_workflow();
        let mut show = self.command_open;
        egui::Window::new("命令与日志")
            .open(&mut show)
            .default_width(670.)
            .default_height(380.)
            .collapsible(false)
            .show(ctx, |ui| {
                let selected = self
                    .entries
                    .iter()
                    .find(|e| Some(e.id) == self.selected)
                    .or_else(|| self.entries.first());
                if let (Some(tools), Some(e)) = (&self.tools, selected) {
                    if let Some(info) = &e.media {
                        let settings = if matches!(e.status, Status::Running | Status::Queued) {
                            e.settings.as_ref().unwrap_or(&self.prefs.export)
                        } else {
                            &self.prefs.export
                        };
                        let plan = if !matches!(e.status, Status::Running | Status::Queued)
                            && let Some(error) = self.time_input_error()
                        {
                            Err(error.to_owned())
                        } else {
                            media::plan(tools, info, settings)
                        };
                        match plan {
                            Ok(plan) => {
                                let mut preview = media::command_preview(&plan);
                                ui.label(RichText::new("命令预览").strong());
                                ui.label(
                                    RichText::new(
                                        "以下展示参数；运行时会先写入独立临时目录，再保存成品。",
                                    )
                                    .size(11.)
                                    .color(p.muted),
                                );
                                if ui.button("复制命令").clicked() {
                                    ctx.copy_text(preview.clone());
                                }
                                ui.add(
                                    egui::TextEdit::multiline(&mut preview)
                                        .font(egui::TextStyle::Monospace)
                                        .desired_rows(4)
                                        .desired_width(f32::INFINITY)
                                        .interactive(false),
                                );
                            }
                            Err(err) => {
                                ui.colored_label(p.danger, err);
                            }
                        }
                    }
                } else {
                    ui.label("添加媒体文件后，可以在这里查看命令。");
                }
                ui.add_space(8.);
                ui.separator();
                ui.label(RichText::new("处理日志").strong());
                egui::ScrollArea::vertical()
                    .id_salt("logs")
                    .max_height(260.)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        if self.logs.is_empty() {
                            ui.label(RichText::new("运行任务后显示 FFmpeg 日志。").color(p.muted));
                        } else {
                            for line in &self.logs {
                                ui.add(
                                    egui::Label::new(RichText::new(line).monospace().size(11.))
                                        .wrap(),
                                );
                            }
                        }
                    });
            });
        self.command_open = show;
    }

    fn capture(&mut self, ctx: &egui::Context) {
        if let Some(path) = &self.screenshot {
            self.frames += 1;
            if self.frames > 8
                && !self.checking
                && !self.entries.iter().any(|e| e.status == Status::Analyzing)
                && !self.screenshot_requested
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
                self.screenshot_requested = true;
            }
            let screenshot = ctx.input(|i| {
                i.events.iter().find_map(|event| {
                    if let egui::Event::Screenshot { image, .. } = event {
                        Some(image.clone())
                    } else {
                        None
                    }
                })
            });
            if let Some(image) = screenshot {
                let bytes: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
                let _ = image::save_buffer(
                    path,
                    &bytes,
                    image.size[0] as u32,
                    image.size[1] as u32,
                    image::ColorType::Rgba8,
                );
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ctx.request_repaint_after(Duration::from_millis(80));
        }
    }
}

impl eframe::App for Frameflow {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain(ctx);
        self.sync_time_inputs();
        let p = theme::apply(ctx);
        let files: Vec<_> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if !files.is_empty() {
            self.import(files, ctx);
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::O)) {
            self.dialog(ctx, 0);
        }
        self.render_ui(ctx, p);
        if !ctx.input(|i| i.raw.hovered_files.is_empty()) {
            let layer = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("drop-overlay"),
            ));
            let r = ctx.content_rect();
            layer.rect_filled(r, 0., Color32::from_black_alpha(165));
            layer.text(
                r.center(),
                Align2::CENTER_CENTER,
                "松开鼠标，添加到工作台",
                FontId::proportional(25.),
                Color32::WHITE,
            );
        }
        if self.active.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        self.capture(ctx);
    }
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if self.screenshot.is_none() {
            eframe::set_value(storage, "preferences", &self.prefs);
        }
    }
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn navigation_item(
    ui: &mut Ui,
    p: Palette,
    icon: Icon,
    label: &str,
    selected: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 40.), Sense::click());
    if selected || response.hovered() {
        ui.painter()
            .rect_filled(rect, 5., if selected { p.card } else { p.hover });
    }
    if selected {
        ui.painter().rect_filled(
            Rect::from_center_size(pos2(rect.left() + 2., rect.center().y), vec2(3., 18.)),
            2.,
            p.accent,
        );
    }
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            5.,
            Stroke::new(1_f32, p.accent),
            egui::StrokeKind::Inside,
        );
    }
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    let foreground = if ui.is_enabled() { p.text } else { p.muted };
    icons::paint(
        ui,
        Rect::from_center_size(pos2(rect.left() + 24., rect.center().y), vec2(19., 19.)),
        icon,
        if selected { p.accent } else { foreground },
    );
    if rect.width() > 80. && !matches!(icon, Icon::Menu) {
        ui.painter().text(
            pos2(rect.left() + 46., rect.center().y),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(14.),
            foreground,
        );
    }
    response
}

fn time_mode(op: Operation) -> Option<TimeFormat> {
    match op {
        Operation::Gif => Some(TimeFormat::Seconds),
        Operation::Trim | Operation::Audio | Operation::Snapshot => Some(TimeFormat::Clock),
        _ => None,
    }
}

fn time_input_row(
    ui: &mut Ui,
    p: Palette,
    label: &str,
    draft: &mut TimeDraft,
    seconds: &mut f64,
    routing: &mut TimeEditRouting,
) {
    let mode = routing.mode;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.;
        ui.spacing_mut().button_padding = vec2(4., 4.);
        ui.add_sized(
            [32., 32.],
            egui::Label::new(RichText::new(label).size(12.).color(p.muted)),
        );
        let input_width = (ui.available_width() - 64.).max(152.);
        let input_id = time_input_id(label);
        // Finish text delivered alongside an outside click before releasing
        // focus; otherwise egui can discard that frame's paste or keystroke.
        let finish_edit = routing.owner == Some(input_id);
        let can_edit = routing.owner.is_none() || finish_edit;
        let focus_policy = ui.memory(|memory| memory.options.input_options.surrender_focus_on);
        if finish_edit {
            ui.memory_mut(|memory| {
                memory.options.input_options.surrender_focus_on = egui::SurrenderFocusOn::Never
            });
        }
        let input = egui::TextEdit::singleline(&mut draft.text)
            .id(input_id)
            .interactive(can_edit)
            .font(egui::TextStyle::Monospace)
            .text_color(if draft.invalid { p.danger } else { p.text })
            .vertical_align(Align::Center)
            .margin(vec2(4., 4.))
            .desired_width(input_width - 8.);
        let response = ui.add_sized([input_width, 32.], input);
        if !can_edit
            && ui.is_enabled()
            && response.contains_pointer()
            && ui.input(|input| {
                input.pointer.any_click()
                    && input.pointer.interact_pos().is_some_and(|position| {
                        response.rect.contains(position) && ui.clip_rect().contains(position)
                    })
            })
        {
            routing.next_focus = Some(input_id);
        }
        if finish_edit {
            ui.memory_mut(|memory| memory.options.input_options.surrender_focus_on = focus_policy);
            if response.clicked_elsewhere() {
                response.surrender_focus();
                // TextEdit reads cloned events. A second field gaining focus
                // below must not apply the edit a second time.
                ui.input_mut(|input| input.events.retain(|event| !time_edit_event(event)));
            }
        }
        if response.changed() {
            if let Some(value) = timecode::parse(&draft.text, mode) {
                *seconds = value;
                draft.committed = value;
                draft.invalid = false;
            } else {
                draft.invalid = true;
            }
        }
        if response.lost_focus() && !draft.invalid {
            draft.text = timecode::format(*seconds, mode);
        }
        let decrement = ui
            .add_enabled(
                !draft.invalid,
                egui::Button::new("−").min_size(vec2(28., 32.)),
            )
            .clicked();
        let increment = ui
            .add_enabled(
                !draft.invalid,
                egui::Button::new("+").min_size(vec2(28., 32.)),
            )
            .clicked();
        if decrement || increment {
            let milliseconds = (*seconds * 1000.).round() + if increment { 1. } else { -1. };
            *seconds = milliseconds.clamp(0., timecode::MAX_SECONDS * 1000.) / 1000.;
            draft.committed = *seconds;
            draft.text = timecode::format(*seconds, mode);
        }
    });
    if draft.invalid {
        ui.label(
            RichText::new(match mode {
                TimeFormat::Clock => "请输入时:分:秒:毫秒，如 00:00:01:005。",
                TimeFormat::Seconds => "请输入总秒:毫秒，如 65:005。",
            })
            .size(12.)
            .color(p.danger),
        );
    }
}

fn time_input_id(label: &str) -> egui::Id {
    egui::Id::new(("time-input", label))
}

fn time_edit_event(event: &egui::Event) -> bool {
    match event {
        egui::Event::Text(_) | egui::Event::Paste(_) | egui::Event::Cut | egui::Event::Ime(_) => {
            true
        }
        egui::Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } => {
            matches!(key, egui::Key::Backspace | egui::Key::Delete)
                || (modifiers.command && matches!(key, egui::Key::A | egui::Key::Z | egui::Key::Y))
                || (modifiers.ctrl
                    && matches!(
                        key,
                        egui::Key::H | egui::Key::K | egui::Key::U | egui::Key::W
                    ))
        }
        _ => false,
    }
}

fn workflow_tab(op: Operation) -> &'static str {
    match op {
        Operation::Convert => "转换设置",
        Operation::Compress => "压缩设置",
        Operation::Trim => "裁剪设置",
        Operation::Audio => "音频设置",
        Operation::Gif => "GIF 设置",
        Operation::Snapshot => "截图设置",
        Operation::Remux => "封装设置",
        Operation::Subtitle => "字幕设置",
    }
}

fn workflow_defaults(op: Operation) -> ExportSettings {
    ExportSettings {
        operation: op,
        format: match op {
            Operation::Audio => "mp3",
            Operation::Gif => "gif",
            Operation::Snapshot => "png",
            Operation::Subtitle => "srt",
            _ => "mp4",
        }
        .into(),
        quality: if op == Operation::Compress { 28 } else { 23 },
        resolution: if op == Operation::Gif { 480 } else { 0 },
        fps: if op == Operation::Gif { 12 } else { 0 },
        end_seconds: if op == Operation::Gif { 5. } else { 0. },
        ..Default::default()
    }
}

fn setting_row(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    if ui.available_width() < 400. {
        ui.label(label);
        add(ui);
    } else {
        ui.horizontal_top(|ui| {
            ui.add_sized([120., 32.], egui::Label::new(label).halign(Align::LEFT));
            ui.allocate_ui_with_layout(
                vec2(ui.available_width(), 0.),
                Layout::top_down(Align::LEFT),
                add,
            );
        });
    }
}

fn settings_expander(ui: &mut Ui, p: Palette, title: &str, open: bool, add: impl FnOnce(&mut Ui)) {
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1_f32, p.line))
        .corner_radius(8)
        .inner_margin(16)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            egui::CollapsingHeader::new(RichText::new(title).size(16.).strong())
                .icon(crate::theme::collapse_chevron)
                .id_salt(("workflow-group", title))
                .default_open(open)
                .show(ui, add);
        });
}

fn settings_card(ui: &mut Ui, p: Palette, title: &str, help: &str, add: impl FnOnce(&mut Ui)) {
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1_f32, p.line))
        .corner_radius(8)
        .inner_margin(16)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            if ui.available_width() >= 580. {
                ui.set_min_height(40.);
                ui.horizontal_top(|ui| {
                    let description_width = (ui.available_width() * 0.3).clamp(170., 250.);
                    ui.allocate_ui_with_layout(
                        vec2(description_width, 0.),
                        Layout::top_down(Align::LEFT),
                        |ui| {
                            ui.label(RichText::new(title).size(15.).strong());
                            if !help.is_empty() {
                                ui.add(
                                    egui::Label::new(RichText::new(help).size(12.).color(p.muted))
                                        .wrap(),
                                );
                            }
                        },
                    );
                    ui.allocate_ui_with_layout(
                        vec2(ui.available_width(), 0.),
                        Layout::top_down(Align::LEFT),
                        |ui| {
                            ui.set_min_width(ui.available_width());
                            add(ui);
                        },
                    );
                });
            } else {
                ui.label(RichText::new(title).size(15.).strong());
                if !help.is_empty() {
                    ui.add(egui::Label::new(RichText::new(help).size(12.).color(p.muted)).wrap());
                }
                ui.add_space(4.);
                add(ui);
            }
        });
}

fn settings_frame(p: Palette) -> Frame {
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1_f32, p.line))
        .corner_radius(8)
        .inner_margin(16)
}

fn settings_header(ui: &mut Ui, title: &str, trailing: impl FnOnce(&mut Ui)) {
    ui.horizontal_top(|ui| {
        ui.label(RichText::new(title).size(16.).strong());
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), 0.),
            Layout::right_to_left(Align::Center).with_main_wrap(true),
            trailing,
        );
    });
    ui.add_space(8.);
}

fn settings_group(ui: &mut Ui, p: Palette, title: &str, add: impl FnOnce(&mut Ui)) {
    settings_frame(p).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.label(RichText::new(title).size(16.).strong());
        ui.add_space(8.);
        add(ui);
    });
}

fn operation_icon(op: Operation) -> Icon {
    match op {
        Operation::Convert => Icon::Convert,
        Operation::Compress => Icon::Compress,
        Operation::Trim => Icon::Trim,
        Operation::Audio => Icon::Audio,
        Operation::Gif => Icon::Gif,
        Operation::Snapshot => Icon::Snapshot,
        Operation::Remux => Icon::Remux,
        Operation::Subtitle => Icon::File,
    }
}
fn duration(seconds: f64) -> String {
    timecode::format(seconds, TimeFormat::Clock)
}
fn size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.)
    } else {
        format!("{:.1} MB", bytes as f64 / 1_048_576.)
    }
}
