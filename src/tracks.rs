//! Explicit source track selection and subtitle handling.
//! All process arguments remain separate; filter paths are escaped twice for libavfilter.

use eframe::egui::{self, Ui};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashSet,
    ffi::OsString,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackKind {
    Audio,
    Subtitle,
    Attachment,
}

#[derive(Clone, Debug)]
pub struct TrackInfo {
    /// Absolute stream index, not an index relative to its media type.
    pub index: usize,
    pub kind: TrackKind,
    pub codec: String,
    pub language: String,
    pub title: String,
    pub default: bool,
}

pub fn parse_tracks(data: &Value) -> Vec<TrackInfo> {
    data["streams"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|stream| {
            let kind = match stream["codec_type"].as_str()? {
                "audio" => TrackKind::Audio,
                "subtitle" => TrackKind::Subtitle,
                "attachment" => TrackKind::Attachment,
                _ => return None,
            };
            Some(TrackInfo {
                index: stream["index"].as_u64()? as usize,
                kind,
                codec: stream["codec_name"].as_str().unwrap_or("unknown").into(),
                language: stream["tags"]["language"].as_str().unwrap_or("und").into(),
                title: stream["tags"]["title"].as_str().unwrap_or("").into(),
                default: stream["disposition"]["default"].as_u64() == Some(1),
            })
        })
        .collect()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrackSelection {
    pub index: usize,
    pub language: String,
    pub title: String,
    pub default: bool,
}
impl From<&TrackInfo> for TrackSelection {
    fn from(track: &TrackInfo) -> Self {
        Self {
            index: track.index,
            language: track.language.clone(),
            title: track.title.clone(),
            default: track.default,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubtitleMode {
    None,
    #[default]
    Keep,
    Burn,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackOptions {
    /// None selects the first audio stream; Some(empty) deliberately removes all audio.
    pub audio: Option<Vec<TrackSelection>>,
    pub subtitles: Vec<TrackSelection>,
    pub mode: SubtitleMode,
    #[serde(skip)]
    pub burn_external: Option<PathBuf>,
    #[serde(skip)]
    pub fonts_dir: Option<PathBuf>,
    #[serde(skip)]
    pub font_attachments: Vec<PathBuf>,
    pub preserve_attachments: bool,
}

fn push(args: &mut Vec<OsString>, values: &[&str]) {
    args.extend(values.iter().map(OsString::from));
}

fn selections<'a>(
    tracks: &'a [TrackInfo],
    chosen: &[TrackSelection],
    kind: TrackKind,
) -> Result<Vec<&'a TrackInfo>, String> {
    let mut seen = HashSet::new();
    if chosen.iter().filter(|s| s.default).count() > 1 {
        return Err("同类轨道只能设置一条默认轨。".into());
    }
    chosen
        .iter()
        .map(|selection| {
            if !seen.insert(selection.index) {
                return Err("所选轨道重复。".into());
            }
            if !selection.language.is_empty()
                && (selection.language.len() > 3
                    || selection.language.len() < 2
                    || !selection.language.bytes().all(|c| c.is_ascii_alphabetic()))
            {
                return Err("轨道语言请使用 2–3 位代码，如 zho、eng。".into());
            }
            tracks
                .iter()
                .find(|t| t.index == selection.index && t.kind == kind)
                .ok_or_else(|| {
                    format!(
                        "源文件中没有所选的 {} 号轨道，请重新选择。",
                        selection.index
                    )
                })
        })
        .collect()
}

fn metadata(args: &mut Vec<OsString>, kind: &str, output_index: usize, selection: &TrackSelection) {
    push(
        args,
        &[
            &format!("-metadata:s:{kind}:{output_index}"),
            &format!(
                "language={}",
                if selection.language.is_empty() {
                    "und"
                } else {
                    &selection.language
                }
            ),
        ],
    );
    push(
        args,
        &[
            &format!("-metadata:s:{kind}:{output_index}"),
            &format!("title={}", selection.title),
        ],
    );
    push(
        args,
        &[
            &format!("-metadata:s:{kind}:{output_index}"),
            &format!("handler_name={}", selection.title),
        ],
    );
    push(
        args,
        &[
            &format!("-disposition:{kind}:{output_index}"),
            if selection.default { "default" } else { "0" },
        ],
    );
}

pub fn append_audio(
    args: &mut Vec<OsString>,
    tracks: &[TrackInfo],
    options: &TrackOptions,
    muted: bool,
    single: bool,
) -> Result<usize, String> {
    // Other explicit maps prevent automatic audio selection. Do not emit -an here:
    // remux may still append external audio after excluding the source tracks.
    if muted {
        return Ok(0);
    }
    if let Some(chosen) = &options.audio {
        selections(tracks, chosen, TrackKind::Audio)?;
        if single && chosen.len() != 1 {
            return Err("音频处理请选择一条源音轨。".into());
        }
        for (i, selection) in chosen.iter().enumerate() {
            push(args, &["-map", &format!("0:{}", selection.index)]);
            metadata(args, "a", i, selection);
        }
        Ok(chosen.len())
    } else {
        push(args, &["-map", if single { "0:a:0" } else { "0:a:0?" }]);
        Ok(usize::from(
            tracks.iter().any(|t| t.kind == TrackKind::Audio),
        ))
    }
}

fn text_subtitle(codec: &str) -> bool {
    matches!(
        codec,
        "subrip"
            | "srt"
            | "ass"
            | "ssa"
            | "webvtt"
            | "mov_text"
            | "text"
            | "microdvd"
            | "mpl2"
            | "jacosub"
            | "sami"
            | "realtext"
            | "subviewer"
            | "subviewer1"
            | "vplayer"
            | "pjs"
            | "stl"
            | "ttml"
    )
}

fn subtitle_codec(codec: &str, format: &str) -> Result<&'static str, String> {
    if format == "mkv" || format == "mks" {
        return Ok("copy");
    }
    if !text_subtitle(codec) {
        return Err("图形字幕只能保留到 MKV / MKS；不能转换为文本字幕。".into());
    }
    match format {
        "mp4" | "mov" => Ok("mov_text"),
        "webm" | "vtt" => Ok("webvtt"),
        "srt" => Ok("srt"),
        "ass" => Ok("ass"),
        _ => Err("此输出格式不支持字幕。".into()),
    }
}

/// External subtitle inputs must already be appended at input indices 1..=N.
pub fn append_subtitles(
    args: &mut Vec<OsString>,
    tracks: &[TrackInfo],
    options: &TrackOptions,
    format: &str,
    external_files: &[PathBuf],
) -> Result<usize, String> {
    if options.mode != SubtitleMode::Keep {
        args.push("-sn".into());
        return Ok(0);
    }
    let chosen = selections(tracks, &options.subtitles, TrackKind::Subtitle)?;
    for (i, (track, selection)) in chosen.iter().zip(&options.subtitles).enumerate() {
        push(
            args,
            &[
                "-map",
                &format!("0:{}", track.index),
                &format!("-c:s:{i}"),
                subtitle_codec(&track.codec, format)?,
            ],
        );
        metadata(args, "s", i, selection);
    }
    for (i, path) in external_files.iter().enumerate() {
        let output_index = chosen.len() + i;
        let codec = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !path.is_file() || !matches!(codec.as_str(), "srt" | "ass" | "ssa" | "vtt") {
            return Err("请选择 SRT、ASS、SSA 或 VTT 字幕。".into());
        }
        push(
            args,
            &[
                "-map",
                &format!("{}:s:0", i + 1),
                &format!("-c:s:{output_index}"),
                subtitle_codec(if codec == "vtt" { "webvtt" } else { &codec }, format)?,
            ],
        );
        metadata(
            args,
            "s",
            output_index,
            &TrackSelection {
                index: 0,
                language: "und".into(),
                title: path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                default: false,
            },
        );
    }
    let count = chosen.len() + external_files.len();
    if count == 0 {
        args.push("-sn".into());
    }
    Ok(count)
}

pub fn extraction_args(
    args: &mut Vec<OsString>,
    tracks: &[TrackInfo],
    options: &TrackOptions,
    format: &str,
) -> Result<(), String> {
    if options.subtitles.len() != 1 {
        return Err("字幕提取请选择一条字幕轨。".into());
    }
    let chosen = selections(tracks, &options.subtitles, TrackKind::Subtitle)?;
    push(
        args,
        &[
            "-map",
            &format!("0:{}", chosen[0].index),
            "-vn",
            "-an",
            "-dn",
            "-c:s",
            subtitle_codec(&chosen[0].codec, format)?,
        ],
    );
    metadata(args, "s", 0, &options.subtitles[0]);
    if format == "mks" {
        push(args, &["-f", "matroska"]);
    }
    Ok(())
}

pub fn append_attachments(
    args: &mut Vec<OsString>,
    options: &TrackOptions,
    format: &str,
) -> Result<(), String> {
    if !options.preserve_attachments && options.font_attachments.is_empty() {
        return Ok(());
    }
    if format != "mkv" {
        return Err("字体附件与源附件保留需要 MKV 格式。".into());
    }
    if options.preserve_attachments {
        push(args, &["-map", "0:t?", "-c:t", "copy"]);
    }
    // Attachment metadata is set separately after counting preserved source attachments.
    for font in &options.font_attachments {
        font_mimetype(font)?;
        args.push("-attach".into());
        args.push(font.as_os_str().to_owned());
    }
    Ok(())
}

fn font_mimetype(path: &Path) -> Result<&'static str, String> {
    if !path.is_file() {
        return Err("字体附件不存在，请重新选择。".into());
    }
    match path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "ttf" | "ttc" => Ok("application/x-truetype-font"),
        "otf" => Ok("application/vnd.ms-opentype"),
        _ => Err("字体附件仅支持 TTF、TTC 或 OTF。".into()),
    }
}

/// Called after append_attachments; counts preserved attachments to target each new font.
pub fn append_attachment_metadata(
    args: &mut Vec<OsString>,
    tracks: &[TrackInfo],
    options: &TrackOptions,
) -> Result<(), String> {
    let offset = if options.preserve_attachments {
        tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Attachment)
            .count()
    } else {
        0
    };
    for (i, font) in options.font_attachments.iter().enumerate() {
        push(
            args,
            &[
                &format!("-metadata:s:t:{}", offset + i),
                &format!("mimetype={}", font_mimetype(font)?),
            ],
        );
        let mut filename = OsString::from("filename=");
        filename.push(font.file_name().unwrap_or_default());
        args.push(format!("-metadata:s:t:{}", offset + i).into());
        args.push(filename);
    }
    Ok(())
}

/// Two parser layers: AVOption values, then the surrounding filtergraph.
fn filter_path(path: &Path) -> Result<String, String> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    let raw = path
        .to_str()
        .ok_or("字幕路径必须是有效 Unicode。")?
        .replace('\\', "/");
    let mut option = String::new();
    for c in raw.chars() {
        if matches!(c, '\\' | '\'' | ':' | ' ') {
            option.push('\\');
        }
        option.push(c);
    }
    let mut graph = String::new();
    for c in option.chars() {
        if matches!(c, '\\' | '\'' | ',' | ';' | '[' | ']') {
            graph.push('\\');
        }
        graph.push(c);
    }
    Ok(graph)
}

pub fn burn_filter(
    source: &Path,
    tracks: &[TrackInfo],
    options: &TrackOptions,
) -> Result<Option<String>, String> {
    if options.mode != SubtitleMode::Burn {
        return Ok(None);
    }
    let mut filter = if let Some(external) = &options.burn_external {
        if !external.is_file()
            || !matches!(
                external
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .as_str(),
                "srt" | "ass" | "ssa" | "vtt"
            )
        {
            return Err("烧录请选择 SRT、ASS、SSA 或 VTT 字幕。".into());
        }
        format!("subtitles=filename={}", filter_path(external)?)
    } else {
        if options.subtitles.len() != 1 {
            return Err("烧录请选择一条文本字幕，或选择外部字幕。".into());
        }
        let chosen = selections(tracks, &options.subtitles, TrackKind::Subtitle)?;
        if !text_subtitle(&chosen[0].codec) {
            return Err("烧录暂支持文本字幕；图形字幕请保留到 MKV。".into());
        }
        let relative = tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Subtitle)
            .position(|t| t.index == chosen[0].index)
            .ok_or("字幕轨不可用。")?;
        format!("subtitles=filename={}:si={relative}", filter_path(source)?)
    };
    if let Some(fonts) = &options.fonts_dir {
        if !fonts.is_dir() {
            return Err("字体文件夹不存在。".into());
        }
        filter.push_str(&format!(":fontsdir={}", filter_path(fonts)?));
    }
    Ok(Some(filter))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Picker {
    BurnSubtitle,
    FontsDirectory,
    FontAttachments,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TrackUiOptions {
    pub audio: bool,
    pub subtitles: bool,
    pub extract: bool,
    pub burn: bool,
    pub single_audio: bool,
    pub attachments: bool,
}

/// The app supplies only the capabilities relevant to the current export tool.
pub fn ui(
    ui: &mut Ui,
    options: &mut TrackOptions,
    tracks: &[TrackInfo],
    capabilities: TrackUiOptions,
) -> Option<Picker> {
    let mut picker = None;
    if capabilities.audio {
        let mut mode = match &options.audio {
            None => 0,
            Some(chosen) if chosen.is_empty() => 2,
            Some(_) => 1,
        };
        let previous = mode;
        let first_audio = tracks.iter().find(|track| track.kind == TrackKind::Audio);
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut mode, 0, "首条音轨");
            ui.add_enabled_ui(first_audio.is_some(), |ui| {
                ui.selectable_value(&mut mode, 1, "选择音轨");
            });
            ui.selectable_value(&mut mode, 2, "不保留");
        });
        if mode != previous {
            options.audio = match mode {
                0 => None,
                1 => Some(first_audio.map(TrackSelection::from).into_iter().collect()),
                _ => Some(Vec::new()),
            };
        }
        if mode == 1
            && let Some(chosen) = &mut options.audio
        {
            track_editor(
                ui,
                tracks,
                chosen,
                TrackKind::Audio,
                capabilities.single_audio,
                true,
            );
        }
    }
    if capabilities.subtitles {
        if !capabilities.extract {
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut options.mode, SubtitleMode::None, "无字幕");
                ui.selectable_value(&mut options.mode, SubtitleMode::Keep, "保留字幕");
                if capabilities.burn {
                    ui.selectable_value(&mut options.mode, SubtitleMode::Burn, "烧录字幕");
                }
            });
            if options.mode == SubtitleMode::Burn && !capabilities.burn {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    "当前导出不支持烧录，请选择保留或无字幕。",
                );
            }
        }
        if capabilities.extract || options.mode != SubtitleMode::None {
            track_editor(
                ui,
                tracks,
                &mut options.subtitles,
                TrackKind::Subtitle,
                capabilities.extract || options.mode == SubtitleMode::Burn,
                !capabilities.extract && options.mode == SubtitleMode::Keep,
            );
        }
        if !capabilities.extract && capabilities.burn && options.mode == SubtitleMode::Burn {
            ui.horizontal_wrapped(|ui| {
                if ui.button("外部字幕…").clicked() {
                    picker = Some(Picker::BurnSubtitle);
                }
                if ui.button("字体文件夹…").clicked() {
                    picker = Some(Picker::FontsDirectory);
                }
            });
            optional_path(ui, &mut options.burn_external, "外部字幕");
            optional_path(ui, &mut options.fonts_dir, "字体文件夹");
            if options.burn_external.is_some() {
                ui.small("使用外部字幕，源字幕不参与烧录。");
            }
        }
    }
    if capabilities.attachments && !capabilities.extract {
        egui::CollapsingHeader::new("字体附件")
            .icon(crate::theme::collapse_chevron)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(&mut options.preserve_attachments, "保留源附件");
                    if ui.button("添加字体…").clicked() {
                        picker = Some(Picker::FontAttachments);
                    }
                });
                let mut remove = None;
                for (i, font) in options.font_attachments.iter().enumerate() {
                    ui.push_id(i, |ui| {
                        if removable_path(ui, font, "", "移除") {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    options.font_attachments.remove(i);
                }
            });
    }
    picker
}

fn optional_path(ui: &mut Ui, path: &mut Option<PathBuf>, label: &str) {
    if let Some(current) = path.as_ref()
        && removable_path(ui, current, label, "清除")
    {
        *path = None;
    }
}

fn removable_path(ui: &mut Ui, path: &Path, label: &str, remove_label: &str) -> bool {
    let mut remove = false;
    let details_id = ui.make_persistent_id(("track-file-details", label, path));
    let mut details = ui.data(|data| data.get_temp::<bool>(details_id).unwrap_or(false));
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            remove = ui.small_button(remove_label).clicked();
            if ui.small_button("路径").clicked() {
                details = !details;
            }
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            let caption = if label.is_empty() {
                name.into_owned()
            } else {
                format!("{label}：{name}")
            };
            ui.add(
                egui::Label::new(caption)
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
    ui.data_mut(|data| data.insert_temp(details_id, details));
    remove
}

fn track_editor(
    ui: &mut Ui,
    tracks: &[TrackInfo],
    chosen: &mut Vec<TrackSelection>,
    kind: TrackKind,
    single: bool,
    edit_metadata: bool,
) {
    if single {
        chosen.truncate(1);
    }
    let available: Vec<_> = tracks.iter().filter(|t| t.kind == kind).collect();
    if available.is_empty() {
        ui.small(if kind == TrackKind::Audio {
            "所选文件没有音轨。"
        } else {
            "所选文件暂无字幕轨。"
        });
    }
    for track in available {
        ui.push_id((kind as u8, track.index), |ui| {
            let mut selected = chosen.iter().any(|s| s.index == track.index);
            let caption = format!(
                "#{} · {} · {}{}",
                track.index,
                track.codec.to_uppercase(),
                track.language,
                if track.title.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", track.title)
                },
            );
            let changed = if single {
                let response = ui.radio(selected, caption);
                if response.clicked() && !selected {
                    selected = true;
                    true
                } else {
                    false
                }
            } else {
                ui.checkbox(&mut selected, caption).changed()
            };
            if changed {
                if selected {
                    if single {
                        chosen.clear();
                    }
                    chosen.push(TrackSelection::from(track));
                } else {
                    chosen.retain(|s| s.index != track.index);
                }
            }
            if edit_metadata && let Some(i) = chosen.iter().position(|s| s.index == track.index) {
                let mut make_default = false;
                egui::CollapsingHeader::new("轨道信息")
                    .icon(crate::theme::collapse_chevron)
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label("语言");
                            ui.add(
                                egui::TextEdit::singleline(&mut chosen[i].language)
                                    .desired_width(56.)
                                    .hint_text("zho"),
                            );
                            ui.label("标题");
                            ui.add(
                                egui::TextEdit::singleline(&mut chosen[i].title)
                                    .desired_width(160.),
                            );
                            make_default = ui.checkbox(&mut chosen[i].default, "默认轨").changed()
                                && chosen[i].default;
                        });
                        if !chosen[i].language.is_empty()
                            && (chosen[i].language.len() < 2
                                || chosen[i].language.len() > 3
                                || !chosen[i].language.bytes().all(|c| c.is_ascii_alphabetic()))
                        {
                            ui.colored_label(
                                ui.visuals().error_fg_color,
                                "语言使用 2–3 位代码，如 zho、eng。",
                            );
                        }
                    });
                if make_default {
                    for (j, s) in chosen.iter_mut().enumerate() {
                        if j != i {
                            s.default = false;
                        }
                    }
                }
            }
        });
    }
    if chosen
        .iter()
        .any(|s| !tracks.iter().any(|t| t.kind == kind && t.index == s.index))
    {
        ui.colored_label(ui.visuals().error_fg_color, "选择已失效，请重选轨道。");
        if ui.small_button("清除失效选择").clicked() {
            chosen.retain(|s| tracks.iter().any(|t| t.kind == kind && t.index == s.index));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        process::{Command, Stdio},
        time::{SystemTime, UNIX_EPOCH},
    };

    fn fixture() -> Vec<TrackInfo> {
        parse_tracks(&serde_json::json!({"streams":[
            {"index":0,"codec_type":"video","codec_name":"h264"},
            {"index":2,"codec_type":"audio","codec_name":"aac","tags":{"language":"zho","title":"原声"},"disposition":{"default":1}},
            {"index":4,"codec_type":"audio","codec_name":"aac","tags":{"language":"eng"}},
            {"index":6,"codec_type":"subtitle","codec_name":"ass","tags":{"language":"eng","title":"字幕"}},
            {"index":8,"codec_type":"subtitle","codec_name":"hdmv_pgs_subtitle"},
            {"index":9,"codec_type":"attachment","codec_name":"ttf"}
        ]}))
    }
    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }
    fn pair(args: &[OsString], key: &str, value: &str) -> bool {
        strings(args)
            .windows(2)
            .any(|p| p[0] == key && p[1] == value)
    }

    #[test]
    fn global_track_indices_and_metadata_are_preserved() {
        let tracks = fixture();
        assert_eq!(tracks.len(), 5);
        assert_eq!(tracks[0].index, 2);
        assert_eq!(tracks[0].language, "zho");
        assert!(tracks[0].default);
        let mut second = TrackSelection::from(&tracks[1]);
        second.default = true;
        second.title = "English".into();
        let options = TrackOptions {
            audio: Some(vec![second]),
            ..Default::default()
        };
        let mut args = Vec::new();
        assert_eq!(
            append_audio(&mut args, &tracks, &options, false, true).unwrap(),
            1
        );
        assert!(pair(&args, "-map", "0:4"));
        assert!(pair(&args, "-metadata:s:a:0", "language=eng"));
        assert!(pair(&args, "-disposition:a:0", "default"));
        assert!(!pair(&args, "-map", "0:a:0"));
    }

    #[test]
    fn invalid_duplicate_or_multiple_default_tracks_are_rejected() {
        let tracks = fixture();
        let mut options = TrackOptions {
            audio: Some(vec![
                TrackSelection::from(&tracks[0]),
                TrackSelection::from(&tracks[1]),
            ]),
            ..Default::default()
        };
        assert!(append_audio(&mut Vec::new(), &tracks, &options, false, true).is_err());
        options.audio.as_mut().unwrap()[1].default = true;
        assert!(append_audio(&mut Vec::new(), &tracks, &options, false, false).is_err());
        options.audio.as_mut().unwrap()[1].default = false;
        options.audio.as_mut().unwrap()[1].index = 33;
        assert!(append_audio(&mut Vec::new(), &tracks, &options, false, false).is_err());
        options.audio.as_mut().unwrap()[1].index = 2;
        assert!(append_audio(&mut Vec::new(), &tracks, &options, false, false).is_err());
    }

    #[test]
    fn subtitle_copy_conversion_and_bitmap_boundaries() {
        let tracks = fixture();
        let mut options = TrackOptions {
            subtitles: vec![TrackSelection::from(&tracks[2])],
            ..Default::default()
        };
        let mut args = Vec::new();
        append_subtitles(&mut args, &tracks, &options, "mp4", &[]).unwrap();
        assert!(pair(&args, "-map", "0:6"));
        assert!(pair(&args, "-c:s:0", "mov_text"));
        options.subtitles = vec![TrackSelection::from(&tracks[3])];
        assert!(append_subtitles(&mut Vec::new(), &tracks, &options, "mp4", &[]).is_err());
        assert!(extraction_args(&mut Vec::new(), &tracks, &options, "srt").is_err());
        args.clear();
        extraction_args(&mut args, &tracks, &options, "mks").unwrap();
        assert!(pair(&args, "-c:s", "copy"));
        assert!(pair(&args, "-f", "matroska"));
        options.mode = SubtitleMode::Burn;
        assert!(burn_filter(Path::new("input.mkv"), &tracks, &options).is_err());
    }

    #[test]
    fn paths_do_not_persist_and_default_audio_remains_compatible() {
        let options = TrackOptions {
            burn_external: Some("private.srt".into()),
            fonts_dir: Some("private-fonts".into()),
            font_attachments: vec!["private.ttf".into()],
            ..Default::default()
        };
        let stored = serde_json::to_string(&options).unwrap();
        assert!(!stored.contains("private"));
        let decoded: TrackOptions = serde_json::from_str("{}").unwrap();
        let mut args = Vec::new();
        append_audio(&mut args, &fixture(), &decoded, false, false).unwrap();
        assert!(pair(&args, "-map", "0:a:0?"));
        assert_eq!(decoded.mode, SubtitleMode::Keep);
        assert!(decoded.subtitles.is_empty());
    }

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "frameflow-tracks-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn command(program: &str) -> Command {
        let mut command = Command::new(program);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        command.stdin(Stdio::null());
        command
    }
    fn ffmpeg(args: &[OsString]) {
        let output = command("ffmpeg")
            .args(["-hide_banner", "-v", "error", "-nostdin", "-y"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fn probe(path: &Path) -> Value {
        let output = command("ffprobe")
            .args(["-v", "error", "-show_streams", "-of", "json"])
            .arg(path)
            .output()
            .unwrap();
        assert!(output.status.success());
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn execute_plan(
        tools: &crate::media::Toolchain,
        media: &crate::media::MediaInfo,
        settings: &crate::media::ExportSettings,
    ) -> PathBuf {
        let job = crate::media::plan(tools, media, settings).unwrap();
        let preview = crate::media::command_preview(&job);
        crate::media::run(
            job,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            |_| {},
        )
        .unwrap_or_else(|error| panic!("{error}\n{preview}"))
    }

    fn frame_has_subtitle(path: &Path, time: &str, output: &Path) -> bool {
        let mut args = Vec::new();
        push(&mut args, &["-ss", time, "-i"]);
        args.push(path.as_os_str().to_owned());
        push(&mut args, &["-map", "0:v:0", "-frames:v", "1", "-an"]);
        args.push(output.as_os_str().to_owned());
        ffmpeg(&args);
        image::open(output)
            .unwrap()
            .into_rgb8()
            .pixels()
            .any(|p| p.0.iter().any(|c| *c > 150))
    }

    #[test]
    fn live_integrated_track_jobs_and_trimmed_subtitles_when_installed() {
        if !command("ffmpeg")
            .arg("-version")
            .output()
            .is_ok_and(|o| o.status.success())
        {
            return;
        }
        use crate::media::{ExportSettings, Operation, Toolchain};
        let temp = Temp::new();
        let subtitle = temp.0.join("cue's [中文],测试;.srt");
        fs::write(
            &subtitle,
            "1\n00:00:01,000 --> 00:00:01,500\nVisible subtitle\n",
        )
        .unwrap();
        let source = temp.0.join("source.mkv");
        let mut args = Vec::new();
        push(
            &mut args,
            &[
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=320x240:r=20:d=3",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=3",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=880:duration=3",
                "-i",
            ],
        );
        args.push(subtitle.as_os_str().to_owned());
        push(
            &mut args,
            &[
                "-map",
                "0:v",
                "-map",
                "1:a",
                "-map",
                "2:a",
                "-map",
                "3:s",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-c:a",
                "aac",
                "-c:s",
                "srt",
                "-t",
                "3",
            ],
        );
        args.push(source.as_os_str().to_owned());
        ffmpeg(&args);
        let tools = Toolchain {
            ffmpeg: "ffmpeg".into(),
            ffprobe: "ffprobe".into(),
            version: "test".into(),
            gpu_encoders: Vec::new(),
        };
        let media = crate::media::probe(&tools, &source).unwrap();
        let mut audio = TrackSelection::from(media.tracks.iter().find(|t| t.index == 2).unwrap());
        audio.language = "eng".into();
        audio.title = "Selected English".into();
        audio.default = true;
        let mut subtitle_track = TrackSelection::from(
            media
                .tracks
                .iter()
                .find(|t| t.kind == TrackKind::Subtitle)
                .unwrap(),
        );
        subtitle_track.language = "zho".into();
        subtitle_track.default = true;
        let options = TrackOptions {
            audio: Some(vec![audio]),
            subtitles: vec![subtitle_track],
            ..Default::default()
        };

        // The public planner/run boundary must retain selected source metadata during encoding.
        let settings = ExportSettings {
            format: "mkv".into(),
            tracks: options.clone(),
            ..Default::default()
        };
        let converted = execute_plan(&tools, &media, &settings);
        let converted_tracks = parse_tracks(&probe(&converted));
        assert_eq!(
            converted_tracks
                .iter()
                .filter(|t| t.kind == TrackKind::Audio)
                .count(),
            1
        );
        let selected = converted_tracks
            .iter()
            .find(|t| t.kind == TrackKind::Audio)
            .unwrap();
        assert_eq!(selected.language, "eng");
        assert_eq!(selected.title, "Selected English");
        assert!(selected.default);
        let selected_subtitle = converted_tracks
            .iter()
            .find(|t| t.kind == TrackKind::Subtitle)
            .unwrap();
        assert_eq!(selected_subtitle.language, "zho");
        assert!(selected_subtitle.default);

        // Mixed output still carries the selected source track's edited metadata.
        let mut mixed = settings.clone();
        mixed.effects.mix.enabled = true;
        mixed
            .effects
            .mix
            .tracks
            .push(crate::effects::MixTrack::new(source.clone()));
        let mixed_output = execute_plan(&tools, &media, &mixed);
        let mixed_tracks = parse_tracks(&probe(&mixed_output));
        let mixed_audio = mixed_tracks
            .iter()
            .find(|t| t.kind == TrackKind::Audio)
            .unwrap();
        assert_eq!(mixed_audio.language, "eng");
        assert_eq!(mixed_audio.title, "Selected English");
        assert!(mixed_audio.default);

        let extracted = execute_plan(
            &tools,
            &media,
            &ExportSettings {
                operation: Operation::Subtitle,
                format: "srt".into(),
                tracks: options.clone(),
                ..Default::default()
            },
        );
        let extracted_text = fs::read_to_string(extracted).unwrap();
        assert!(extracted_text.contains("Visible subtitle"));
        assert!(extracted_text.contains("00:00:01,"));

        let font = temp.0.join("attached.ttf");
        fs::write(&font, b"font fixture").unwrap();
        let mut remux = ExportSettings {
            operation: Operation::Remux,
            format: "mkv".into(),
            tracks: options.clone(),
            ..Default::default()
        };
        remux.tracks.font_attachments.push(font.clone());
        let remuxed = execute_plan(&tools, &media, &remux);
        let remuxed_info = crate::media::probe(&tools, &remuxed).unwrap();
        assert_eq!(
            remuxed_info
                .tracks
                .iter()
                .filter(|t| t.kind == TrackKind::Attachment)
                .count(),
            1
        );
        assert_eq!(
            remuxed_info
                .tracks
                .iter()
                .filter(|t| t.kind == TrackKind::Subtitle)
                .count(),
            1
        );
        assert!(
            remuxed_info
                .tracks
                .iter()
                .any(|t| t.kind == TrackKind::Audio && t.language == "eng" && t.default)
        );
        let mut preserve = ExportSettings {
            operation: Operation::Remux,
            format: "mkv".into(),
            ..Default::default()
        };
        preserve.tracks.subtitles = remuxed_info
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Subtitle)
            .map(TrackSelection::from)
            .collect();
        preserve.tracks.preserve_attachments = true;
        preserve.tracks.font_attachments.push(font);
        let preserved = execute_plan(&tools, &remuxed_info, &preserve);
        assert_eq!(
            parse_tracks(&probe(&preserved))
                .iter()
                .filter(|t| t.kind == TrackKind::Attachment)
                .count(),
            2
        );

        for external in [false, true] {
            let mut trim = ExportSettings {
                operation: Operation::Trim,
                format: "mkv".into(),
                start_seconds: 1.0,
                end_seconds: 2.5,
                tracks: options.clone(),
                ..Default::default()
            };
            trim.tracks.mode = SubtitleMode::Burn;
            trim.tracks.burn_external = external.then(|| subtitle.clone());
            let output = execute_plan(&tools, &media, &trim);
            assert!(
                frame_has_subtitle(&output, "0.15", &temp.0.join("active.png")),
                "Subtitle should be visible soon after the cropped start; external={external}"
            );
            assert!(
                !frame_has_subtitle(&output, "1.00", &temp.0.join("inactive.png")),
                "Subtitle must expire at its original timeline end; external={external}"
            );
        }

        let soft = execute_plan(
            &tools,
            &media,
            &ExportSettings {
                operation: Operation::Trim,
                format: "mkv".into(),
                start_seconds: 1.0,
                end_seconds: 2.5,
                subtitle_files: vec![subtitle],
                ..Default::default()
            },
        );
        let packets = command("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "s",
                "-show_packets",
                "-of",
                "json",
            ])
            .arg(soft)
            .output()
            .unwrap();
        assert!(packets.status.success());
        let data: Value = serde_json::from_slice(&packets.stdout).unwrap();
        let packets = data["packets"].as_array().unwrap();
        assert_eq!(
            packets.len(),
            1,
            "External subtitles should remain in the trimmed output"
        );
        let pts: f64 = packets[0]["pts_time"].as_str().unwrap().parse().unwrap();
        assert!(
            pts.abs() < 0.08,
            "Cue must move to the cropped start, got {pts}"
        );
    }

    #[test]
    fn live_ffmpeg_selected_tracks_extract_burn_and_fonts_when_installed() {
        if !command("ffmpeg")
            .arg("-version")
            .output()
            .is_ok_and(|o| o.status.success())
        {
            return;
        }
        let temp = Temp::new();
        // Exercise both escaping layers with Unicode, spaces, apostrophe and graph punctuation.
        let assets = temp.0.join("字体's [目录],测试;");
        fs::create_dir(&assets).unwrap();
        let subtitle = assets.join("字幕's [A],一;.srt");
        fs::write(
            &subtitle,
            "1\n00:00:00,000 --> 00:00:01,000\nFrameflow subtitle\n",
        )
        .unwrap();
        let source = assets.join("源's [B],二;.mkv");
        let mut args = Vec::new();
        push(
            &mut args,
            &[
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=320x240:r=10:d=1",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=1",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=880:duration=1",
                "-i",
            ],
        );
        args.push(subtitle.clone().into_os_string());
        push(&mut args, &["-i"]);
        args.push(subtitle.clone().into_os_string());
        push(
            &mut args,
            &[
                "-map", "0:v", "-map", "1:a", "-map", "2:a", "-map", "3:s", "-map", "4:s", "-c:v",
                "libx264", "-c:a", "aac", "-c:s", "srt", "-t", "1",
            ],
        );
        args.push(source.clone().into_os_string());
        ffmpeg(&args);
        let tracks = parse_tracks(&probe(&source));
        let mut audio = TrackSelection::from(tracks.iter().find(|t| t.index == 2).unwrap());
        audio.language = "eng".into();
        audio.default = true;
        audio.title = "English".into();
        let mut sub = TrackSelection::from(tracks.iter().find(|t| t.index == 4).unwrap());
        sub.language = "zho".into();
        sub.default = true;
        let options = TrackOptions {
            audio: Some(vec![audio]),
            subtitles: vec![sub],
            ..Default::default()
        };
        let preserved = temp.0.join("selected.mkv");
        args.clear();
        push(&mut args, &["-i"]);
        args.push(source.clone().into_os_string());
        push(&mut args, &["-map", "0:v:0", "-c", "copy"]);
        append_audio(&mut args, &tracks, &options, false, false).unwrap();
        append_subtitles(&mut args, &tracks, &options, "mkv", &[]).unwrap();
        args.push(preserved.clone().into_os_string());
        ffmpeg(&args);
        let result = parse_tracks(&probe(&preserved));
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].language, "eng");
        assert!(result[0].default);
        assert_eq!(result[1].language, "zho");
        assert!(result[1].default);
        for format in ["srt", "ass", "vtt", "mks"] {
            let output = temp.0.join(format!("subtitle.{format}"));
            args.clear();
            push(&mut args, &["-i"]);
            args.push(source.clone().into_os_string());
            extraction_args(&mut args, &tracks, &options, format).unwrap();
            args.push(output.clone().into_os_string());
            ffmpeg(&args);
            assert!(fs::metadata(output).unwrap().len() > 20);
        }
        let mut burn = options.clone();
        burn.mode = SubtitleMode::Burn;
        burn.fonts_dir = Some(assets.clone());
        for external in [false, true] {
            burn.burn_external = external.then(|| subtitle.clone());
            let filter = burn_filter(&source, &tracks, &burn).unwrap().unwrap();
            let output = temp.0.join(if external {
                "burn-external.png"
            } else {
                "burn-embedded.png"
            });
            args.clear();
            push(&mut args, &["-i"]);
            args.push(source.clone().into_os_string());
            push(
                &mut args,
                &["-map", "0:v:0", "-vf", &filter, "-frames:v", "1", "-an"],
            );
            args.push(output.clone().into_os_string());
            ffmpeg(&args);
            let image = image::open(output).unwrap().into_rgb8();
            assert!(
                image.pixels().any(|p| p.0.iter().any(|c| *c > 150)),
                "Burned subtitle must visibly alter the black input"
            );
        }
        let font = assets.join("font.ttf");
        fs::write(&font, b"font attachment fixture").unwrap();
        let mut fonts = options.clone();
        fonts.font_attachments.push(font);
        let attached = temp.0.join("attached.mkv");
        args.clear();
        push(&mut args, &["-i"]);
        args.push(source.clone().into_os_string());
        push(&mut args, &["-map", "0:v:0", "-c", "copy"]);
        append_attachments(&mut args, &fonts, "mkv").unwrap();
        append_attachment_metadata(&mut args, &tracks, &fonts).unwrap();
        args.push(attached.clone().into_os_string());
        ffmpeg(&args);
        let result = probe(&attached);
        let attachment = result["streams"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["codec_type"] == "attachment")
            .unwrap();
        assert_eq!(
            attachment["tags"]["mimetype"],
            "application/x-truetype-font"
        );
        fonts.font_attachments.clear();
        fonts.preserve_attachments = true;
        let second_font = assets.join("second.otf");
        fs::write(&second_font, b"second font attachment fixture").unwrap();
        fonts.font_attachments.push(second_font);
        let copied = temp.0.join("attachments-preserved.mkv");
        args.clear();
        push(&mut args, &["-i"]);
        args.push(attached.clone().into_os_string());
        push(&mut args, &["-map", "0:v:0", "-c", "copy"]);
        append_attachments(&mut args, &fonts, "mkv").unwrap();
        append_attachment_metadata(&mut args, &parse_tracks(&result), &fonts).unwrap();
        args.push(copied.clone().into_os_string());
        ffmpeg(&args);
        let preserved = probe(&copied);
        let attachments: Vec<_> = preserved["streams"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|s| s["codec_type"] == "attachment")
            .collect();
        assert_eq!(attachments.len(), 2);
        assert_eq!(
            attachments[0]["tags"]["mimetype"],
            "application/x-truetype-font"
        );
        assert_eq!(
            attachments[1]["tags"]["mimetype"],
            "application/vnd.ms-opentype"
        );
    }
}
