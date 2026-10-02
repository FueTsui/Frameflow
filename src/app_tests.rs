//! Pointer-driven UI regression tests. The media entries are fixtures; no FFmpeg
//! process, native window, or file dialog is launched by these tests.

use super::*;
use egui::{Event, FullOutput, Modifiers, PointerButton, Pos2, RawInput, Theme};

fn has_viewport_command(output: &FullOutput, expected: egui::ViewportCommand) -> bool {
    output.viewport_output[&egui::ViewportId::ROOT]
        .commands
        .contains(&expected)
}

#[test]
fn close_idle_window_does_not_require_confirmation() {
    let mut ui = Harness::new();
    let output = ui.close_frame(true);
    assert_eq!(ui.app.close_state, CloseState::Open);
    assert!(!has_viewport_command(
        &output,
        egui::ViewportCommand::CancelClose
    ));
}

#[test]
fn close_running_task_requires_confirmation_and_waits_for_worker_cleanup() {
    let mut ui = Harness::new();
    let running = ui.add_file("running.mp4");
    ui.add_file("queued.mp4");
    ui.app.entries[0].status = Status::Running;
    ui.app.entries[1].status = Status::Queued;
    ui.app.batch = true;
    let cancellation = Arc::new(AtomicBool::new(false));
    ui.app.active = Some((running, cancellation.clone()));
    let (release, wait) = mpsc::channel();
    let (done, cleaned) = mpsc::channel();
    let tx = ui.app.tx.clone();
    ui.app.worker = Some(thread::spawn(move || {
        wait.recv().unwrap();
        tx.send(Message::Finished(running, Err("已取消".into())))
            .unwrap();
        done.send(()).unwrap();
    }));

    let output = ui.close_frame(true);
    assert!(has_viewport_command(
        &output,
        egui::ViewportCommand::CancelClose
    ));
    assert_eq!(ui.app.close_state, CloseState::Confirming);
    assert!(!cancellation.load(Ordering::Relaxed));

    let output = ui.settle();
    ui.click(painted_text_bounds(&output, "继续使用").0.center());
    assert_eq!(ui.app.close_state, CloseState::Open);
    assert!(!cancellation.load(Ordering::Relaxed));

    ui.close_frame(true);
    let output = ui.settle();
    ui.click(painted_text_bounds(&output, "取消任务并退出").0.center());
    assert_eq!(ui.app.close_state, CloseState::Waiting);
    assert!(cancellation.load(Ordering::Relaxed));
    assert!(!ui.app.batch);
    assert!(ui.app.entries[1].status == Status::Ready);
    let output = ui.close_frame(true);
    assert!(has_viewport_command(
        &output,
        egui::ViewportCommand::CancelClose
    ));
    assert!(!has_viewport_command(&output, egui::ViewportCommand::Close));

    release.send(()).unwrap();
    cleaned.recv_timeout(Duration::from_secs(2)).unwrap();
    let output = ui.close_frame(false);
    assert_eq!(ui.app.close_state, CloseState::Ready);
    assert!(ui.app.active.is_none());
    assert!(ui.app.worker.is_none());
    assert!(ui.app.entries[0].status == Status::Cancelled);
    assert!(has_viewport_command(&output, egui::ViewportCommand::Close));
    assert!(!has_viewport_command(
        &ui.close_frame(true),
        egui::ViewportCommand::CancelClose
    ));
}

#[test]
fn close_engine_installation_waits_for_result_before_exiting() {
    for result in [Ok(PathBuf::from("managed-engine")), Err("下载失败".into())] {
        let mut ui = Harness::new();
        ui.app.update_status = UpdateStatus::Installing;
        let output = ui.close_frame(true);
        assert!(has_viewport_command(
            &output,
            egui::ViewportCommand::CancelClose
        ));
        let output = ui.settle();
        ui.click(painted_text_bounds(&output, "完成更新后退出").0.center());
        assert_eq!(ui.app.close_state, CloseState::Waiting);
        assert!(!has_viewport_command(
            &ui.close_frame(false),
            egui::ViewportCommand::Close
        ));
        ui.app.tx.send(Message::UpdateInstalled(result)).unwrap();
        let output = ui.close_frame(false);
        assert_eq!(ui.app.close_state, CloseState::Ready);
        assert!(has_viewport_command(&output, egui::ViewportCommand::Close));
    }
}

#[test]
fn close_confirmation_defers_new_work_and_refreshes_engine_when_dismissed() {
    let mut ui = Harness::new();
    ui.app.close_state = CloseState::Confirming;
    ui.app.update_status = UpdateStatus::Installing;
    ui.app.import(vec!["later.mp4".into()], &ui.context);
    ui.app.dialog(&ui.context, 0);
    ui.app.start_batch(&ui.context);
    assert_eq!(ui.app.pending_files, [PathBuf::from("later.mp4")]);
    assert!(!ui.app.dialog_open);
    assert!(!ui.app.batch);
    ui.app
        .tx
        .send(Message::UpdateInstalled(Ok("engine".into())))
        .unwrap();
    ui.close_frame(false);
    assert!(!ui.app.checking);
    let output = ui.settle();
    ui.click(painted_text_bounds(&output, "继续使用").0.center());
    assert_eq!(ui.app.close_state, CloseState::Open);
    assert!(ui.app.checking);
}

struct Harness {
    app: Frameflow,
    context: egui::Context,
    time: f64,
    system_theme: Theme,
    overlay: Option<Rect>,
    viewport: egui::Vec2,
}

impl Harness {
    fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let context = egui::Context::default();
        context.set_theme(egui::ThemePreference::System);
        theme::configure(&context);
        Self {
            app: Frameflow {
                prefs: Preferences::default(),
                tools: None,
                tool_error: String::new(),
                checking: false,
                entries: Vec::new(),
                next_id: 1,
                selected: None,
                tx,
                rx,
                active: None,
                worker: None,
                close_state: CloseState::Open,
                batch: false,
                command_open: false,
                page: WorkspacePage::Files,
                notice: None,
                logs: Vec::new(),
                dialog_open: false,
                frames: 0,
                screenshot: None,
                screenshot_requested: false,
                pending_files: Vec::new(),
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
            },
            context,
            time: 0.0,
            system_theme: Theme::Light,
            overlay: None,
            viewport: vec2(1180.0, 780.0),
        }
    }

    fn add_file(&mut self, name: &str) -> u64 {
        let id = self.app.next_id;
        self.app.next_id += 1;
        let path = PathBuf::from(name);
        self.app.entries.push(Entry {
            id,
            path: path.clone(),
            media: Some(MediaInfo {
                path,
                duration: 12.0,
                width: 1920,
                height: 1080,
                video_codec: Some("h264".into()),
                audio_codec: Some("aac".into()),
                size: 1_048_576,
                tracks: Vec::new(),
            }),
            status: Status::Ready,
            progress: 0.0,
            speed: String::new(),
            error: String::new(),
            output: None,
            settings: None,
        });
        id
    }

    fn frame(&mut self, events: Vec<Event>) -> (FullOutput, bool) {
        self.time += 1.0 / 30.0;
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.viewport)),
            time: Some(self.time),
            focused: true,
            system_theme: Some(self.system_theme),
            events,
            ..Default::default()
        };
        let app = &mut self.app;
        let overlay = self.overlay;
        let mut overlay_clicked = false;
        let output = self.context.run(input, |ctx| {
            let palette = theme::apply(ctx);
            app.render_ui(ctx, palette);
            if let Some(rect) = overlay {
                egui::Area::new(egui::Id::new("test-foreground-window"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(rect.min)
                    .show(ctx, |ui| {
                        let (_, response) = ui.allocate_exact_size(rect.size(), Sense::click());
                        overlay_clicked |= response.clicked();
                    });
            }
        });
        (output, overlay_clicked)
    }

    fn close_frame(&mut self, requested: bool) -> FullOutput {
        self.time += 1.0 / 30.0;
        let mut input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.viewport)),
            time: Some(self.time),
            focused: true,
            system_theme: Some(self.system_theme),
            ..Default::default()
        };
        if requested {
            input
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .events
                .push(egui::ViewportEvent::Close);
        }
        let app = &mut self.app;
        self.context.run(input, |ctx| {
            app.handle_close_request(ctx);
            app.drain(ctx);
            app.render_ui(ctx, theme::apply(ctx));
        })
    }

    fn settle(&mut self) -> FullOutput {
        self.frame(Vec::new());
        self.frame(Vec::new()).0
    }

    fn click(&mut self, point: Pos2) -> bool {
        self.frame(vec![Event::PointerMoved(point)]);
        self.frame(vec![Event::PointerButton {
            pos: point,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        let (_, overlay_clicked) = self.frame(vec![Event::PointerButton {
            pos: point,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
        overlay_clicked
    }

    fn scroll_export_to(&mut self, label: &str) -> FullOutput {
        self.show_export();
        self.scroll_content_to(label)
    }

    fn scroll_content_to(&mut self, label: &str) -> FullOutput {
        // egui may cull an offscreen widget completely. If the text is absent,
        // begin at the top rather than assuming the target is below us.
        if painted_text_bounds_if_present(&self.settle(), label).is_none() {
            self.frame(vec![
                Event::PointerMoved(pos2(self.viewport.x - 38., 280.)),
                Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: vec2(0., 100_000.),
                    modifiers: Modifiers::NONE,
                },
            ]);
            for _ in 0..15 {
                self.frame(Vec::new());
            }
        }
        for _ in 0..30 {
            let output = self.settle();
            let direction = match painted_text_bounds_if_present(&output, label) {
                Some((bounds, clip)) if clip.expand(0.5).contains_rect(bounds) => return output,
                Some((bounds, clip)) if bounds.bottom() <= clip.bottom() => 70.,
                _ => -70.,
            };
            self.frame(vec![
                Event::PointerMoved(pos2(self.viewport.x - 38., 280.)),
                Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: vec2(0., direction),
                    modifiers: Modifiers::NONE,
                },
            ]);
            for _ in 0..15 {
                self.frame(Vec::new());
            }
        }
        panic!("Export control {label:?} should be reachable by scrolling");
    }

    fn scroll_card_control_to(&mut self, card: &str, label: &str) -> Pos2 {
        // Start from the top on every lookup. Matching a label at the current
        // scroll position could otherwise select a later card with the same action.
        self.frame(vec![
            Event::PointerMoved(pos2(self.viewport.x - 38., 280.)),
            Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: vec2(0., 100_000.),
                modifiers: Modifiers::NONE,
            },
        ]);
        for _ in 0..15 {
            self.frame(Vec::new());
        }
        for _ in 0..30 {
            let output = self.settle();
            let mut direction = -70.;
            if let Some((heading, _)) = painted_text_bounds_if_present(&output, card) {
                let palette = Palette::get(&self.context);
                let mut cards = Vec::new();
                for clipped in &output.shapes {
                    visit_shapes(&clipped.shape, &mut |shape| {
                        if let egui::Shape::Rect(rect) = shape
                            && rect.fill == palette.card
                            && rect.rect.contains_rect(heading)
                        {
                            cards.push(rect.rect);
                        }
                    });
                }
                let bounds = cards
                    .into_iter()
                    .min_by(|a, b| a.area().total_cmp(&b.area()));
                if let Some(card_bounds) = bounds {
                    let matches: Vec<_> = painted_text_matches(&output, label)
                        .into_iter()
                        .filter(|(text, _)| card_bounds.contains_rect(*text))
                        .collect();
                    assert!(
                        matches.len() <= 1,
                        "Expected at most one {label:?} control inside {card:?}"
                    );
                    if let Some((text, clip)) = matches.first() {
                        if clip.expand(0.5).contains_rect(*text) {
                            return text.center();
                        }
                        if text.top() < clip.top() {
                            direction = 70.;
                        }
                    }
                }
            }
            self.frame(vec![
                Event::PointerMoved(pos2(self.viewport.x - 38., 280.)),
                Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: vec2(0., direction),
                    modifiers: Modifiers::NONE,
                },
            ]);
            for _ in 0..15 {
                self.frame(Vec::new());
            }
        }
        panic!("Control {label:?} inside {card:?} should be reachable by scrolling");
    }

    fn show_export(&mut self) {
        if self.app.page != WorkspacePage::Export {
            let output = self.settle();
            self.click(text_center(
                &output,
                workflow_tab(self.app.prefs.export.operation),
            ));
            assert!(self.app.page == WorkspacePage::Export);
        }
    }

    fn focus_time(&mut self, label: &str) {
        let output = self.scroll_export_to(label);
        self.click(text_center(&output, label) + vec2(90., 0.));
        assert!(
            self.context.wants_keyboard_input(),
            "The time input must receive pointer focus"
        );
    }

    fn select_time_text(&mut self, label: &str) {
        self.focus_time(label);
        self.frame(vec![command_key(egui::Key::A)]);
    }

    fn enter_time(&mut self, label: &str, text: &str, paste: bool) -> FullOutput {
        self.select_time_text(label);
        self.frame(vec![if paste {
            Event::Paste(text.into())
        } else {
            Event::Text(text.into())
        }]);
        self.settle()
    }

    fn hover_without_tooltip(&mut self, point: Pos2) {
        self.frame(vec![Event::PointerMoved(point)]);
        self.time += 2.0;
        for _ in 0..3 {
            self.frame(Vec::new());
            let tooltip_visible = self.context.memory(|memory| {
                memory
                    .areas()
                    .visible_layer_ids()
                    .iter()
                    .any(|layer| layer.order == egui::Order::Tooltip)
            });
            assert!(
                !tooltip_visible,
                "Hovering {point:?} must never create a tooltip layer"
            );
        }
    }

    fn source_tracks(&mut self) {
        self.app.entries[0].media.as_mut().unwrap().tracks = crate::tracks::parse_tracks(
            &serde_json::json!({"streams": [
                {"index": 1, "codec_type": "audio", "codec_name": "aac", "tags": {"language": "zho", "title": "原声"}, "disposition": {"default": 1}},
                {"index": 3, "codec_type": "audio", "codec_name": "aac", "tags": {"language": "eng", "title": "配音"}},
                {"index": 2, "codec_type": "subtitle", "codec_name": "subrip", "tags": {"language": "zho", "title": "中文"}},
                {"index": 4, "codec_type": "subtitle", "codec_name": "ass", "tags": {"language": "eng", "title": "English"}}
            ]}),
        );
    }

    fn export_labels(&mut self) -> std::collections::BTreeMap<String, usize> {
        self.show_export();
        let mut labels = std::collections::BTreeMap::<String, usize>::new();
        // Visit the full scroll range; offscreen controls cannot satisfy a
        // layout assertion simply because egui omitted their paint shapes.
        for delta in std::iter::once(100_000.0).chain(std::iter::repeat_n(-220.0, 16)) {
            self.frame(vec![
                Event::PointerMoved(pos2(self.viewport.x - 38., 280.)),
                Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: vec2(0., delta),
                    modifiers: Modifiers::NONE,
                },
            ]);
            for _ in 0..15 {
                self.frame(Vec::new());
            }
            let output = self.settle();
            let mut frame_labels = std::collections::BTreeMap::<String, usize>::new();
            for clipped in &output.shapes {
                visit_shapes(&clipped.shape, &mut |shape| {
                    if let egui::Shape::Text(text) = shape
                        && clipped.clip_rect.intersects(text.visual_bounding_rect())
                    {
                        *frame_labels
                            .entry(text.galley.job.text.clone())
                            .or_default() += 1;
                    }
                });
            }
            for (label, count) in frame_labels {
                labels
                    .entry(label)
                    .and_modify(|n| *n = (*n).max(count))
                    .or_insert(count);
            }
        }
        labels
    }
}

fn command_key(key: egui::Key) -> Event {
    Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::COMMAND,
    }
}

struct TimePlanFile(PathBuf);

impl TimePlanFile {
    fn new() -> Self {
        static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "frameflow-time-ui-{}-{serial}.mp4",
            std::process::id()
        ));
        std::fs::write(&path, b"Plan-only fixture; no media decoding is performed").unwrap();
        Self(path)
    }
}

impl Drop for TimePlanFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn assert_plan_time(ui: &Harness, seek: &str, length: Option<&str>) -> String {
    let plan = media::plan(
        ui.app.tools.as_ref().unwrap(),
        ui.app.entries[0].media.as_ref().unwrap(),
        &ui.app.prefs.export,
    )
    .unwrap();
    let arguments = plan
        .args
        .iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let seek_index = arguments.iter().position(|value| value == "-ss").unwrap();
    assert_eq!(arguments[seek_index + 1], seek);
    if let Some(length) = length {
        let duration_index = arguments.iter().position(|value| value == "-t").unwrap();
        assert_eq!(arguments[duration_index + 1], length);
    } else {
        assert!(!arguments.iter().any(|value| value == "-t"));
    }
    media::command_preview(&plan)
}

#[test]
fn timecodes_entered_by_pointer_keyboard_and_paste_reach_plans_and_preview() {
    let fixture = TimePlanFile::new();
    for system_theme in [Theme::Light, Theme::Dark] {
        for viewport in [vec2(1240., 820.), vec2(960., 660.)] {
            for op in [
                Operation::Trim,
                Operation::Audio,
                Operation::Snapshot,
                Operation::Gif,
            ] {
                let mut ui = Harness::new();
                ui.viewport = viewport;
                ui.system_theme = system_theme;
                ui.app.tools = Some(Arc::new(fixture_tools("9.0.1")));
                ui.add_file(fixture.0.to_str().unwrap());
                ui.app.entries[0].media.as_mut().unwrap().duration = 864000.;
                let output = ui.settle();
                ui.click(text_center(&output, op.label()));
                let label = if op == Operation::Snapshot {
                    "位置"
                } else {
                    "开始"
                };
                let (start, end, start_seconds, end_seconds) = if op == Operation::Gif {
                    ("65:001", "70:002", 65.001, 70.002)
                } else {
                    ("100:00:01:001", "100:00:06:002", 360001.001, 360006.002)
                };
                ui.enter_time(label, start, false);
                assert_eq!(
                    ui.app.prefs.export.start_seconds, start_seconds,
                    "{op:?} typed start"
                );
                if op != Operation::Snapshot {
                    ui.enter_time("结束", end, true);
                    assert_eq!(
                        ui.app.prefs.export.end_seconds, end_seconds,
                        "{op:?} pasted end"
                    );
                }
                assert!(ui.app.time_input_error().is_none());
                let seek = format!("{start_seconds:.6}");
                let preview = assert_plan_time(
                    &ui,
                    &seek,
                    if op == Operation::Snapshot {
                        None
                    } else {
                        Some("5.001000")
                    },
                );
                assert!(preview.contains(&seek));
                let output = ui.scroll_export_to(label);
                let (bounds, clip) = painted_text_bounds(&output, start);
                assert!(
                    clip.expand(0.5).contains_rect(bounds),
                    "Complete timecode clipped: {op:?} {viewport:?} {bounds:?} / {clip:?}"
                );
                let screen = Rect::from_min_size(Pos2::ZERO, viewport);
                assert!(screen.contains_rect(bounds));
                ui.click(text_center(&output, "命令与日志"));
                let output = ui.settle();
                assert!(painted_text_bounds_if_present(&output, "复制命令").is_some());
                ui.click(text_center(&output, "复制命令"));
                let output = ui.frame(Vec::new()).0;
                // Copy output is emitted on the click frame; the visible preview
                // itself must contain exactly the same validated command.
                let mut preview_painted = false;
                for clipped in &output.shapes {
                    visit_shapes(&clipped.shape, &mut |shape| {
                        if let egui::Shape::Text(text) = shape {
                            preview_painted |= text.galley.job.text == preview;
                        }
                    });
                }
                assert!(
                    preview_painted,
                    "The command window must use the millisecond settings"
                );
            }
        }
    }
}

#[test]
fn one_millisecond_end_sentinel_and_step_buttons_keep_precision() {
    let fixture = TimePlanFile::new();
    for op in [
        Operation::Trim,
        Operation::Audio,
        Operation::Snapshot,
        Operation::Gif,
    ] {
        let mut ui = Harness::new();
        ui.viewport = vec2(960., 660.);
        ui.app.tools = Some(Arc::new(fixture_tools("9.0.1")));
        ui.add_file(fixture.0.to_str().unwrap());
        let output = ui.settle();
        ui.click(text_center(&output, op.label()));
        let label = if op == Operation::Snapshot {
            "位置"
        } else {
            "开始"
        };
        let one_ms = if op == Operation::Gif {
            "00:001"
        } else {
            "00:00:00:001"
        };
        ui.enter_time(label, one_ms, true);
        if op != Operation::Snapshot {
            let zero = if op == Operation::Gif {
                "00:000"
            } else {
                "00:00:00:000"
            };
            ui.enter_time("结束", zero, true);
            assert_eq!(ui.app.prefs.export.end_seconds, 0.);
        }
        assert_eq!(ui.app.prefs.export.start_seconds, 0.001);
        assert_plan_time(&ui, "0.001000", None);
        let output = ui.scroll_export_to(label);
        let row_y = text_center(&output, label).y;
        let mut increment = None;
        for clipped in &output.shapes {
            visit_shapes(&clipped.shape, &mut |shape| {
                if let egui::Shape::Text(text) = shape
                    && text.galley.job.text == "+"
                {
                    let bounds = Rect::from_min_size(text.pos, text.galley.size());
                    if (bounds.center().y - row_y).abs() < 4. {
                        increment = Some(bounds.center());
                    }
                }
            });
        }
        ui.click(increment.expect("A visible +1ms action should be beside the time input"));
        assert_eq!(ui.app.prefs.export.start_seconds, 0.002);
        assert_eq!(duration(0.001), "00:00:00:001");
    }
}

#[test]
fn invalid_time_draft_blocks_start_preview_and_survives_navigation() {
    let mut ui = Harness::new();
    ui.app.tools = Some(Arc::new(fixture_tools("9.0.1")));
    ui.add_file("not-an-actual-source.mp4");
    let output = ui.settle();
    ui.click(text_center(&output, Operation::Trim.label()));
    let output = ui.enter_time("开始", "00:61:00:001", true);
    assert!(ui.app.time_input_error().is_some());
    assert_eq!(ui.app.prefs.export.start_seconds, 0.);
    assert!(
        painted_text_bounds_if_present(&output, "时间输入无效，请修正后再开始处理。").is_some()
    );
    ui.click(text_center(&output, "开始处理"));
    assert!(ui.app.entries[0].status == Status::Ready && ui.app.entries[0].settings.is_none());
    // An existing source ensures preview rejection is caused by the invalid
    // draft, not by an unrelated missing-file error from the media planner.
    let fixture = TimePlanFile::new();
    ui.app.entries[0].media.as_mut().unwrap().path = fixture.0.clone();
    let output = ui.settle();
    ui.click(text_center(&output, "命令与日志"));
    let output = ui.settle();
    assert!(painted_text_bounds_if_present(&output, "复制命令").is_none());
    ui.app.command_open = false;
    let output = ui.settle();
    ui.click(text_center(&output, "文件与任务"));
    ui.frame(vec![command_key(egui::Key::Enter)]);
    assert!(ui.app.page == WorkspacePage::Export);
    assert!(ui.app.entries[0].settings.is_none());
    let output = ui.settle();
    ui.click(text_center(&output, "设置"));
    let output = ui.settle();
    ui.click(text_center(&output, Operation::Audio.label()));
    let output = ui.settle();
    ui.click(text_center(&output, Operation::Trim.label()));
    assert!(ui.app.time_input_error().is_some());
    assert_eq!(ui.app.time_inputs.start.text, "00:61:00:001");
    ui.enter_time("开始", "00:00:00:001", true);
    assert!(ui.app.time_input_error().is_none());
    assert_eq!(ui.app.prefs.export.start_seconds, 0.001);
}

#[test]
fn same_frame_time_paste_is_validated_before_shortcut_or_pointer_start() {
    for pointer_start in [false, true] {
        for (text, valid) in [("00:00:00:001", true), ("not-a-time", false)] {
            let mut ui = Harness::new();
            ui.app.tools = Some(Arc::new(fixture_tools("9.0.1")));
            ui.add_file("missing-source-prevents-test-process.mp4");
            let output = ui.settle();
            ui.click(text_center(&output, Operation::Audio.label()));
            ui.select_time_text("开始");
            let output = ui.settle();
            let mut events = vec![Event::Paste(text.into())];
            if pointer_start {
                let point = text_center(&output, "开始处理");
                events.extend([
                    Event::PointerMoved(point),
                    Event::PointerButton {
                        pos: point,
                        button: PointerButton::Primary,
                        pressed: true,
                        modifiers: Modifiers::NONE,
                    },
                    Event::PointerButton {
                        pos: point,
                        button: PointerButton::Primary,
                        pressed: false,
                        modifiers: Modifiers::NONE,
                    },
                ]);
            } else {
                events.push(command_key(egui::Key::Enter));
            }
            ui.frame(events);
            assert!(ui.app.active.is_none() && ui.app.worker.is_none());
            if valid {
                let settings = ui.app.entries[0]
                    .settings
                    .as_ref()
                    .expect("Valid same-frame input should reach the attempted plan");
                assert_eq!(
                    settings.start_seconds, 0.001,
                    "Start must use this frame's pasted value"
                );
                assert!(
                    ui.app.entries[0].status == Status::Failed,
                    "Only the missing fixture file should reject this plan"
                );
            } else {
                assert!(
                    ui.app.entries[0].status == Status::Ready
                        && ui.app.entries[0].settings.is_none(),
                    "Invalid same-frame paste must prevent submitting old settings"
                );
                assert!(ui.app.time_input_error().is_some());
            }
        }
    }
}

#[test]
fn same_frame_invalid_paste_and_navigation_preserve_the_draft() {
    let mut ui = Harness::new();
    let output = ui.settle();
    ui.click(text_center(&output, Operation::Trim.label()));
    ui.select_time_text("开始");
    let output = ui.settle();
    let point = text_center(&output, "文件与任务");
    ui.frame(vec![
        Event::Paste("00:00:00:1000".into()),
        Event::PointerMoved(point),
        Event::PointerButton {
            pos: point,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
        Event::PointerButton {
            pos: point,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        },
    ]);
    assert!(ui.app.page == WorkspacePage::Files);
    assert!(ui.app.time_input_error().is_some());
    assert_eq!(ui.app.time_inputs.start.text, "00:00:00:1000");
    let output = ui.scroll_export_to("开始");
    assert!(painted_text_bounds_if_present(&output, "00:00:00:1000").is_some());
}

#[test]
fn same_frame_paste_and_other_time_field_click_edit_only_the_original_field() {
    for (original, target) in [("开始", "结束"), ("结束", "开始")] {
        let mut ui = Harness::new();
        let output = ui.settle();
        ui.click(text_center(&output, Operation::Trim.label()));
        ui.select_time_text(original);
        let output = ui.settle();
        let point = text_center(&output, target) + vec2(90., 0.);
        ui.frame(vec![
            Event::Paste("00:00:00:001".into()),
            Event::PointerMoved(point),
            Event::PointerButton {
                pos: point,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
            Event::PointerButton {
                pos: point,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            },
        ]);
        let (changed, untouched) = if original == "开始" {
            (&ui.app.time_inputs.start, &ui.app.time_inputs.end)
        } else {
            (&ui.app.time_inputs.end, &ui.app.time_inputs.start)
        };
        assert_eq!(changed.committed, 0.001, "Paste belongs to {original}");
        assert_eq!(
            untouched.committed, 0.,
            "Paste must not also modify {target}"
        );
        assert_eq!(untouched.text, "00:00:00:000");
        assert!(
            ui.app.time_input_error().is_none(),
            "The newly focused field must not receive the same paste again"
        );
        assert!(
            ui.context
                .memory(|memory| memory.has_focus(time_input_id(target))),
            "The pointer must still focus {target} after the pending edit"
        );
    }
}

fn visit_shapes(shape: &egui::Shape, visitor: &mut impl FnMut(&egui::Shape)) {
    visitor(shape);
    if let egui::Shape::Vec(shapes) = shape {
        for child in shapes {
            visit_shapes(child, visitor);
        }
    }
}

fn text_center(output: &FullOutput, label: &str) -> Pos2 {
    let mut found = None;
    for clipped in &output.shapes {
        visit_shapes(&clipped.shape, &mut |shape| {
            if let egui::Shape::Text(text) = shape
                && text.galley.job.text == label
            {
                found = Some(Rect::from_min_size(text.pos, text.galley.size()).center());
            }
        });
    }
    found.unwrap_or_else(|| panic!("No rendered control labelled {label:?}"))
}

fn text_centers(output: &FullOutput, label: &str) -> Vec<Pos2> {
    let mut found = Vec::new();
    for clipped in &output.shapes {
        visit_shapes(&clipped.shape, &mut |shape| {
            if let egui::Shape::Text(text) = shape
                && text.galley.job.text == label
            {
                found.push(text.visual_bounding_rect().center());
            }
        });
    }
    found.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)));
    found
}

// Read the actual painted bounds instead of assuming row/header coordinates.
// Remove buttons are 30 x 32; other icon-only actions are 36 x 32.
fn button_bounds(output: &FullOutput, width: f32) -> Vec<Rect> {
    let mut bounds = Vec::new();
    for clipped in &output.shapes {
        visit_shapes(&clipped.shape, &mut |shape| {
            if let egui::Shape::Rect(rect) = shape
                && (rect.rect.width() - width).abs() < 0.1
                && (rect.rect.height() - 32.0).abs() < 0.1
            {
                bounds.push(rect.rect);
            }
        });
    }
    bounds
}

fn painted_text_matches(output: &FullOutput, label: &str) -> Vec<(Rect, Rect)> {
    let mut matches = Vec::new();
    for clipped in &output.shapes {
        visit_shapes(&clipped.shape, &mut |shape| {
            if let egui::Shape::Text(text) = shape
                && text.galley.job.text == label
            {
                matches.push((text.visual_bounding_rect(), clipped.clip_rect));
            }
        });
    }
    matches
}

fn painted_text_bounds_if_present(output: &FullOutput, label: &str) -> Option<(Rect, Rect)> {
    let matches = painted_text_matches(output, label);
    assert!(
        matches.len() <= 1,
        "Expected at most one painted label {label:?}"
    );
    matches.first().copied()
}

fn painted_text_bounds(output: &FullOutput, label: &str) -> (Rect, Rect) {
    painted_text_bounds_if_present(output, label)
        .unwrap_or_else(|| panic!("Expected one painted label {label:?}"))
}

#[test]
fn running_progress_labels_are_visible_and_separate_from_the_thin_bar() {
    for system_theme in [Theme::Light, Theme::Dark] {
        for viewport in [vec2(1180.0, 780.0), vec2(960.0, 660.0)] {
            let mut ui = Harness::new();
            ui.system_theme = system_theme;
            ui.viewport = viewport;
            let id = ui.add_file("running.mp4");
            ui.app.entries[0].status = Status::Running;
            ui.app
                .tx
                .send(Message::Progress(
                    id,
                    ProgressEvent::Progress {
                        fraction: 0.48,
                        speed: "0.888x".into(),
                    },
                ))
                .unwrap();
            ui.app.drain(&ui.context);
            let output = ui.settle();
            let palette = Palette::get(&ui.context);
            let mut bars = Vec::new();
            for clipped in &output.shapes {
                visit_shapes(&clipped.shape, &mut |shape| {
                    if let egui::Shape::Rect(rect) = shape
                        && rect.fill == palette.accent
                        && (rect.rect.height() - 5.0).abs() < 0.1
                        && rect.rect.width() > 40.0
                    {
                        assert!(
                            clipped.clip_rect.contains_rect(rect.rect),
                            "Progress bar clipped in {system_theme:?} at {viewport:?}: {:?} / {:?}",
                            rect.rect,
                            clipped.clip_rect
                        );
                        bars.push(rect.rect);
                    }
                });
            }
            assert_eq!(bars.len(), 1, "Expected one visible 5 px progress fill");
            let bar = bars[0];
            for label in ["48%", "0.888x"] {
                let (bounds, clip) = painted_text_bounds(&output, label);
                assert!(
                    bounds.height() > bar.height(),
                    "{label} needs its own readable text height at {viewport:?}"
                );
                assert!(
                    clip.expand(0.5).contains_rect(bounds),
                    "{label} is clipped in {system_theme:?} at {viewport:?}: {bounds:?} vs {clip:?}"
                );
                assert!(
                    !bounds.intersects(bar),
                    "{label} must be painted outside the 5 px progress bar"
                );
            }
            for clipped in &output.shapes {
                visit_shapes(&clipped.shape, &mut |shape| {
                    if let egui::Shape::Text(text) = shape {
                        assert!(
                            !text.visual_bounding_rect().intersects(bar),
                            "The thin bar must not contain painted text: {:?}",
                            text.galley.job.text
                        );
                    }
                });
            }
        }
    }
}

#[test]
fn remux_subtitle_selection_deduplicates_paths_and_remove_buttons_receive_clicks() {
    for system_theme in [Theme::Light, Theme::Dark] {
        for viewport in [vec2(1180.0, 780.0), vec2(960.0, 660.0)] {
            let mut ui = Harness::new();
            ui.system_theme = system_theme;
            ui.viewport = viewport;
            let media_id = ui.add_file("movie.mp4");
            ui.app.selected = Some(media_id);
            let output = ui.settle();
            ui.click(text_center(&output, Operation::Remux.label()));
            assert_eq!(
                ui.app.prefs.export.operation,
                Operation::Remux,
                "Remux navigation should work at {viewport:?} in {system_theme:?}"
            );

            let first = PathBuf::from("subtitles/中文.srt");
            let second = PathBuf::from("subtitles/English.ass");
            ui.app
                .tx
                .send(Message::Subtitles(vec![first.clone(), first.clone()]))
                .unwrap();
            ui.app
                .tx
                .send(Message::Subtitles(vec![second.clone(), first.clone()]))
                .unwrap();
            ui.app.drain(&ui.context);
            assert_eq!(
                ui.app.prefs.export.subtitle_files,
                [first.clone(), second.clone()],
                "Repeated selections should show each subtitle only once"
            );

            for (label, remaining) in [("中文.srt", vec![second.clone()]), ("English.ass", vec![])]
            {
                let output = ui.scroll_export_to(label);
                let (name_bounds, name_clip) = painted_text_bounds(&output, label);
                assert!(
                    name_clip.expand(0.5).contains_rect(name_bounds),
                    "Subtitle name should be visible at {viewport:?} in {system_theme:?}"
                );
                let buttons = button_bounds(&output, 30.0);
                let row_buttons = buttons
                    .into_iter()
                    .filter(|button| {
                        button.left() > name_bounds.right()
                            && button.top() <= name_bounds.center().y
                            && button.bottom() >= name_bounds.center().y
                    })
                    .collect::<Vec<_>>();
                assert_eq!(
                    row_buttons.len(),
                    1,
                    "Find the remove control beside {label}"
                );
                ui.click(row_buttons[0].center());
                assert_eq!(
                    ui.app.prefs.export.subtitle_files, remaining,
                    "Clicking the subtitle row's icon should remove only that subtitle"
                );
                assert_eq!(ui.app.entries.len(), 1);
                assert_eq!(ui.app.entries[0].id, media_id);
                assert_eq!(ui.app.selected, Some(media_id));
            }
        }
    }
}

#[test]
fn remux_audio_selection_deduplicates_paths_and_remove_buttons_receive_clicks() {
    for system_theme in [Theme::Light, Theme::Dark] {
        for viewport in [vec2(1180.0, 780.0), vec2(960.0, 660.0)] {
            let mut ui = Harness::new();
            ui.system_theme = system_theme;
            ui.viewport = viewport;
            let media_id = ui.add_file("movie.mp4");
            ui.app.selected = Some(media_id);
            let output = ui.settle();
            ui.click(text_center(&output, Operation::Remux.label()));
            assert_eq!(
                ui.app.prefs.export.operation,
                Operation::Remux,
                "Remux navigation should work at {viewport:?} in {system_theme:?}"
            );

            let first = PathBuf::from("audios/中文.m4a");
            let second = PathBuf::from("audios/English.flac");
            ui.app
                .tx
                .send(Message::AudioFiles(vec![first.clone(), first.clone()]))
                .unwrap();
            ui.app
                .tx
                .send(Message::AudioFiles(vec![second.clone(), first.clone()]))
                .unwrap();
            ui.app.drain(&ui.context);
            assert_eq!(
                ui.app.prefs.export.audio_files,
                [first.clone(), second.clone()],
                "Repeated selections should show each audio only once"
            );

            for (label, remaining) in [("中文.m4a", vec![second.clone()]), ("English.flac", vec![])]
            {
                // Reach the actual control after the preceding settings groups.
                let output = ui.scroll_export_to(label);
                let (name_bounds, name_clip) = painted_text_bounds(&output, label);
                assert!(
                    name_clip.expand(0.5).contains_rect(name_bounds),
                    "Audio name should be visible at {viewport:?} in {system_theme:?}"
                );
                let buttons = button_bounds(&output, 30.0);
                let row_buttons = buttons
                    .into_iter()
                    .filter(|button| {
                        button.left() > name_bounds.right()
                            && button.top() <= name_bounds.center().y
                            && button.bottom() >= name_bounds.center().y
                    })
                    .collect::<Vec<_>>();
                assert_eq!(
                    row_buttons.len(),
                    1,
                    "Find the remove control beside {label}"
                );
                ui.click(row_buttons[0].center());
                assert_eq!(
                    ui.app.prefs.export.audio_files, remaining,
                    "Clicking the audio row's icon should remove only that audio"
                );
                assert_eq!(ui.app.entries.len(), 1);
                assert_eq!(ui.app.entries[0].id, media_id);
                assert_eq!(ui.app.selected, Some(media_id));
            }
        }
    }
}

#[test]
fn remove_button_receives_pointer_click_inside_selectable_file_row() {
    let mut ui = Harness::new();
    let first = ui.add_file("first.mp4");
    let second = ui.add_file("second.mp4");
    ui.app.selected = Some(first);
    let output = ui.settle();
    let buttons = button_bounds(&output, 30.0);
    assert_eq!(
        buttons.len(),
        2,
        "Each ready file needs its own remove button"
    );

    ui.click(buttons[0].center());

    assert_eq!(
        ui.app.entries.len(),
        1,
        "Row selection must not swallow Remove"
    );
    assert_eq!(ui.app.entries[0].id, second);
    assert_eq!(ui.app.selected, Some(second));
}

#[test]
fn foreground_window_click_does_not_select_or_remove_the_file_behind_it() {
    let mut ui = Harness::new();
    let first = ui.add_file("first.mp4");
    ui.add_file("second.mp4");
    ui.app.selected = Some(first);
    let output = ui.settle();
    let buttons = button_bounds(&output, 30.0);
    let covered_button = buttons[1];
    ui.overlay = Some(covered_button.expand(4.0));
    ui.settle();

    assert!(
        ui.click(covered_button.center()),
        "The foreground control must receive the click"
    );
    assert_eq!(
        ui.app.entries.len(),
        2,
        "A window must block controls below it"
    );
    assert_eq!(
        ui.app.selected,
        Some(first),
        "Window clicks must not select background rows"
    );
}

#[test]
fn navigation_clicks_switch_to_usable_gif_and_audio_settings() {
    let mut ui = Harness::new();
    ui.app.prefs.export.start_seconds = 9.0;
    ui.app.prefs.export.end_seconds = 11.0;
    let output = ui.settle();
    ui.click(text_center(&output, "制作 GIF"));

    let gif = &ui.app.prefs.export;
    assert_eq!(gif.operation, Operation::Gif);
    assert_eq!(gif.format, "gif");
    assert!(gif.fps > 0 && gif.resolution > 0);
    assert!(gif.end_seconds > gif.start_seconds);
    let output = ui.settle();
    ui.click(text_center(&output, "音频处理"));

    let audio = &ui.app.prefs.export;
    assert_eq!(audio.operation, Operation::Audio);
    assert_eq!(audio.format, "mp3");
    assert_eq!(audio.start_seconds, 0.0);
    assert_eq!(
        audio.end_seconds, 0.0,
        "GIF's short range must not truncate audio export"
    );
}

#[test]
fn appearance_settings_track_system_theme_changes() {
    let mut ui = Harness::new();
    ui.app.page = WorkspacePage::Preferences;
    for (label, mode, dark) in [
        ("浅色", Appearance::Light, false),
        ("深色", Appearance::Dark, true),
        ("跟随系统", Appearance::System, false),
    ] {
        let output = ui.settle();
        let current = ui.app.prefs.appearance.label();
        ui.click(text_center(&output, current));
        let output = ui.settle();
        ui.click(text_center(&output, label));
        ui.settle();
        assert!(ui.app.prefs.appearance == mode);
        assert_eq!(ui.context.style().visuals.dark_mode, dark);
    }
    ui.system_theme = Theme::Dark;
    ui.settle();
    assert!(ui.context.style().visuals.dark_mode);
    ui.system_theme = Theme::Light;
    ui.settle();
    assert!(!ui.context.style().visuals.dark_mode);
}

#[test]
fn every_workflow_keeps_primary_controls_visible_in_both_themes_and_window_sizes() {
    for system_theme in [Theme::Light, Theme::Dark] {
        for viewport in [vec2(1240., 820.), vec2(1180., 780.), vec2(960., 660.)] {
            let mut ui = Harness::new();
            ui.system_theme = system_theme;
            ui.viewport = viewport;
            for op in Operation::ALL {
                let output = ui.settle();
                ui.click(text_center(&output, "文件与任务"));
                let output = ui.settle();
                ui.click(text_center(&output, op.label()));
                assert_eq!(ui.app.prefs.export.operation, op);
                let output = ui.settle();
                let screen = Rect::from_min_size(Pos2::ZERO, viewport);
                for label in [
                    workflow_tab(op),
                    "添加文件",
                    "选择文件",
                    "开始处理",
                    "命令与日志",
                ] {
                    let (bounds, clip) = painted_text_bounds(&output, label);
                    assert!(
                        clip.expand(0.5).contains_rect(bounds),
                        "{label} clipped for {op:?} in {system_theme:?} at {viewport:?}: {bounds:?} / {clip:?}"
                    );
                    assert!(
                        screen.contains_rect(bounds),
                        "{label} outside window for {op:?} at {viewport:?}"
                    );
                }
                ui.show_export();
                let anchor = match op {
                    Operation::Convert => "输出视频",
                    Operation::Compress => "压缩目标",
                    Operation::Trim | Operation::Audio | Operation::Gif => "时间范围",
                    Operation::Snapshot => "截取位置",
                    Operation::Remux => "封装格式",
                    Operation::Subtitle => "选择字幕",
                };
                let output = ui.scroll_content_to(anchor);
                let (format_title, clip) = painted_text_bounds(&output, anchor);
                assert!(clip.expand(0.5).contains_rect(format_title));
                let palette = Palette::get(&ui.context);
                let mut cards = Vec::new();
                for clipped in &output.shapes {
                    visit_shapes(&clipped.shape, &mut |shape| {
                        if let egui::Shape::Rect(rect) = shape
                            && rect.fill == palette.card
                            && rect.rect.width() > 300.
                            && rect.rect.height() >= 70.
                            && rect.rect.top() < viewport.y - 134.
                        {
                            assert!(
                                rect.rect.left() >= 227. && rect.rect.right() <= viewport.x - 23.,
                                "Setting card overflows horizontal workspace for {op:?} in {system_theme:?} at {viewport:?}: {:?}",
                                rect.rect
                            );
                        }
                        if let egui::Shape::Rect(rect) = shape
                            && rect.fill == palette.card
                            && rect.rect.contains_rect(format_title)
                            && rect.rect.height() >= 70.
                        {
                            cards.push(rect.rect);
                        }
                    });
                }
                assert_eq!(
                    cards.len(),
                    1,
                    "The workflow's leading task needs its own roomy card"
                );
                assert!(cards[0].width() >= if viewport.x < 1100. { 650. } else { 420. });
                let (folder, clip) = painted_text_bounds(&output, "选择文件夹");
                assert!(clip.expand(0.5).contains_rect(folder));
                // The scrollable settings must never push the export action off screen.
                let (start, clip) = painted_text_bounds(&output, "开始处理");
                assert!(clip.expand(0.5).contains_rect(start) && screen.contains_rect(start));
                ui.app.command_open = false;
                ui.click(text_center(&output, "命令与日志"));
                assert!(
                    ui.app.command_open,
                    "Command dialog must remain usable for {op:?}"
                );
                ui.app.command_open = false;
            }
        }
    }
}

#[test]
fn completed_file_with_long_output_name_keeps_retry_visible_and_clickable() {
    for system_theme in [Theme::Light, Theme::Dark] {
        for viewport in [vec2(1240., 820.), vec2(960., 660.)] {
            let mut ui = Harness::new();
            ui.system_theme = system_theme;
            ui.viewport = viewport;
            let id = ui.add_file("finished.mp4");
            ui.app.selected = Some(id);
            ui.app.entries[0].status = Status::Done;
            ui.app.entries[0].output = Some(PathBuf::from(format!(
                "{}_converted.mp4",
                "output".repeat(30)
            )));
            let output = ui.settle();
            let (bounds, clip) = painted_text_bounds(&output, "重新处理");
            assert!(
                clip.expand(0.5).contains_rect(bounds),
                "Retry clipped at {viewport:?}: {bounds:?} / {clip:?}"
            );
            ui.click(bounds.center());
            assert!(ui.app.entries[0].status == Status::Ready);
        }
    }
}

#[test]
fn sidebar_settings_opens_roomy_page_with_theme_controls_and_returns_to_workflow() {
    for system_theme in [Theme::Light, Theme::Dark] {
        for viewport in [vec2(1240., 820.), vec2(960., 660.)] {
            let mut ui = Harness::new();
            ui.system_theme = system_theme;
            ui.viewport = viewport;
            let id = ui.add_file("keep-this-file.mp4");
            ui.app.selected = Some(id);
            ui.app.prefs.export.quality = 27;
            let output = ui.settle();
            let header_controls: Vec<_> = button_bounds(&output, 36.)
                .into_iter()
                .filter(|rect| rect.bottom() <= 56.)
                .collect();
            assert_eq!(
                header_controls.len(),
                0,
                "The redundant in-client header must be gone"
            );
            let settings_nav = text_center(&output, "设置");
            assert!(settings_nav.x < 204. && settings_nav.y > 400.);
            ui.click(settings_nav);
            assert!(ui.app.page == WorkspacePage::Preferences);
            let output = ui.settle();
            for hidden_label in [
                "文件与任务",
                workflow_tab(Operation::Convert),
                "开始处理",
                "命令与日志",
                "选择文件夹",
            ] {
                assert!(
                    painted_text_bounds_if_present(&output, hidden_label).is_none(),
                    "{hidden_label} should not appear on application settings"
                );
            }
            for label in [
                "应用主题",
                "媒体引擎",
                "跟随系统",
                "选择引擎文件夹",
                "重新检测",
            ] {
                let (bounds, clip) = painted_text_bounds(&output, label);
                assert!(
                    clip.expand(0.5).contains_rect(bounds),
                    "{label} clipped at {viewport:?}: {bounds:?} / {clip:?}"
                );
                assert!(
                    bounds.left() >= 228. && bounds.right() <= viewport.x - 24.,
                    "{label} should remain inside the full-width central page"
                );
            }
            for (label, mode, dark) in [
                ("深色", Appearance::Dark, true),
                ("浅色", Appearance::Light, false),
                ("跟随系统", Appearance::System, system_theme == Theme::Dark),
            ] {
                let output = ui.settle();
                let current = ui.app.prefs.appearance.label();
                ui.click(text_center(&output, current));
                let output = ui.settle();
                ui.click(text_center(&output, label));
                ui.settle();
                assert!(ui.app.prefs.appearance == mode);
                assert_eq!(ui.context.style().visuals.dark_mode, dark);
                assert!(ui.app.page == WorkspacePage::Preferences);
            }
            let output = ui.scroll_content_to("使用文档");
            let (bounds, clip) = painted_text_bounds(&output, "使用文档");
            assert!(clip.expand(0.5).contains_rect(bounds));
            ui.click(text_center(&output, Operation::Convert.label()));
            assert!(ui.app.page == WorkspacePage::Files);
            assert_eq!(ui.app.entries.len(), 1);
            assert_eq!(ui.app.entries[0].id, id);
            assert_eq!(
                ui.app.prefs.export.quality, 27,
                "Returning to current workflow must preserve export settings"
            );
            let output = ui.settle();
            assert!(painted_text_bounds_if_present(&output, "开始处理").is_some());
        }
    }
}

#[test]
fn missing_engine_import_routes_to_preferences_page() {
    let mut ui = Harness::new();
    ui.app
        .import(vec![PathBuf::from("missing-engine.mp4")], &ui.context);
    assert!(ui.app.page == WorkspacePage::Preferences);
    assert!(ui.app.notice.is_some());
    assert_eq!(ui.app.pending_files, [PathBuf::from("missing-engine.mp4")]);
    let output = ui.settle();
    assert!(painted_text_bounds_if_present(&output, "选择引擎文件夹").is_some());
    assert!(painted_text_bounds_if_present(&output, "开始处理").is_none());
}

#[test]
fn secondary_launch_preserves_files_while_engine_detection_or_updates_are_pending() {
    let mut ui = Harness::new();
    ui.app.launch_handler()(Vec::new());
    ui.app.drain(&ui.context);
    assert!(ui.app.notice.is_none());
    assert!(ui.app.page == WorkspacePage::Files);
    ui.app.checking = true;
    let launch = ui.app.launch_handler();
    launch(vec![PathBuf::from("startup.mp4")]);
    ui.app.drain(&ui.context);
    assert_eq!(ui.app.pending_files, [PathBuf::from("startup.mp4")]);
    assert!(ui.app.notice.is_none());
    assert!(ui.app.entries.is_empty());

    // A failed initial engine check must retain the request for a later retry.
    ui.app
        .tx
        .send(Message::Tools(Err("引擎暂不可用".into())))
        .unwrap();
    ui.app.drain(&ui.context);
    launch(vec![PathBuf::from("after-failure.mp4")]);
    ui.app.drain(&ui.context);
    assert_eq!(ui.app.pending_files.len(), 2);
    assert!(ui.app.notice.is_some());

    ui.app.update_status = UpdateStatus::Installing;
    launch(vec![PathBuf::from("during-update.mp4")]);
    ui.app.drain(&ui.context);
    assert_eq!(ui.app.pending_files.len(), 3);
}

#[test]
fn secondary_launch_uses_the_normal_import_path_when_the_engine_is_ready() {
    let mut ui = Harness::new();
    ui.app.tools = Some(Arc::new(fixture_tools("9.0.1")));
    ui.app.launch_handler()(vec![PathBuf::from("not-an-existing-media-file.mp4")]);
    ui.app.drain(&ui.context);
    assert!(ui.app.pending_files.is_empty());
    assert_eq!(
        ui.app.notice.as_deref(),
        Some("请添加媒体文件，暂不支持导入整个文件夹。")
    );
}

#[test]
fn visiting_settings_during_processing_keeps_job_and_current_tool_accessible() {
    let mut ui = Harness::new();
    ui.viewport = vec2(960., 660.);
    let id = ui.add_file("running.mp4");
    let cancellation = Arc::new(AtomicBool::new(false));
    ui.app.entries[0].status = Status::Running;
    ui.app.active = Some((id, cancellation.clone()));
    ui.app.batch = true;
    let output = ui.settle();
    ui.click(text_center(&output, "设置"));
    assert!(ui.app.page == WorkspacePage::Preferences);
    let output = ui.settle();
    let (bounds, clip) = painted_text_bounds(&output, "返回任务");
    assert!(clip.expand(0.5).contains_rect(bounds));
    assert!(!cancellation.load(Ordering::Relaxed));
    ui.click(text_center(&output, Operation::Convert.label()));
    assert!(ui.app.page == WorkspacePage::Files);
    assert!(ui.app.active.is_some() && ui.app.batch);
    assert!(ui.app.entries[0].status == Status::Running);
    assert!(!cancellation.load(Ordering::Relaxed));
}

fn fixture_tools(version: &str) -> Toolchain {
    Toolchain {
        gpu_encoders: Vec::new(),
        ffmpeg: PathBuf::from("fixture-engine/ffmpeg.exe"),
        ffprobe: PathBuf::from("fixture-engine/ffprobe.exe"),
        version: format!(
            "ffmpeg version {version}-essentials_build-www.gyan.dev Copyright fixture"
        ),
    }
}

#[test]
fn automatic_update_preference_defaults_on_for_old_settings_and_persists_opt_out() {
    let defaults: Preferences = serde_json::from_str("{}").unwrap();
    assert!(defaults.auto_check_updates);
    let opted_out = Preferences {
        auto_check_updates: false,
        ..Preferences::default()
    };
    let saved = serde_json::to_string(&opted_out).unwrap();
    let restored: Preferences = serde_json::from_str(&saved).unwrap();
    assert!(!restored.auto_check_updates);
    let mut ui = Harness::new();
    ui.app.prefs.auto_check_updates = false;
    ui.app
        .tx
        .send(Message::Tools(Ok(fixture_tools("9.0.1"))))
        .unwrap();
    ui.app.drain(&ui.context);
    assert!(ui.app.auto_update_checked);
    assert_eq!(ui.app.update_status, UpdateStatus::Idle);
    assert!(
        !ui.app.background_updates_allowed(),
        "Unit tests must never contact the updater"
    );
}

#[test]
fn redundant_titles_and_vendor_version_text_are_not_painted() {
    let mut ui = Harness::new();
    ui.app.tools = Some(Arc::new(fixture_tools("9.0.1")));
    let output = ui.settle();
    for removed in [
        "帧流 Frameflow",
        "本地媒体处理",
        "  媒体工具",
        "帮助与使用文档  ↗",
        "Ctrl+O 添加文件     Ctrl+Enter 开始处理",
        "⌘ O 添加文件     ⌘ Enter 开始处理",
    ] {
        assert!(painted_text_bounds_if_present(&output, removed).is_none());
    }
    assert!(painted_text_bounds_if_present(&output, "FFmpeg 9.0.1 · 已就绪").is_none());
    for op in Operation::ALL {
        let (bounds, _) = painted_text_bounds(&output, op.label());
        assert!(
            bounds.right() < 204.,
            "Tool titles belong only in navigation"
        );
    }
    ui.click(text_center(&output, "设置"));
    let output = ui.settle();
    let (theme, _) = painted_text_bounds(&output, "应用主题");
    assert!(
        theme.top() < 60.,
        "Settings cards should start immediately without redundant headings"
    );
    assert!(painted_text_bounds_if_present(&output, "调整应用外观与媒体处理引擎。").is_none());
    assert!(painted_text_bounds_if_present(&output, "FFmpeg 9.0.1").is_some());
    for clipped in &output.shapes {
        visit_shapes(&clipped.shape, &mut |shape| {
            if let egui::Shape::Text(text) = shape {
                assert!(
                    !text.galley.job.text.contains("gyan.dev")
                        && !text.galley.job.text.contains("Copyright")
                );
            }
        });
    }
    let output = ui.scroll_content_to("使用文档");
    let (help, _) = painted_text_bounds(&output, "使用文档");
    assert!(help.left() > 204.);
}

#[test]
fn update_card_supports_check_download_failure_retry_success_and_current_states_without_network() {
    if !updater::supported() {
        return;
    }
    let mut ui = Harness::new();
    ui.viewport = vec2(960., 660.);
    ui.app.page = WorkspacePage::Preferences;
    ui.app.tools = Some(Arc::new(fixture_tools("9.0.1")));
    let point = ui.scroll_card_control_to("FFmpeg 更新", "检查更新");
    ui.click(point);
    assert_eq!(ui.app.update_status, UpdateStatus::Checking);
    ui.app
        .tx
        .send(Message::UpdateChecked {
            release: Err("连接失败".into()),
            current_version: Some("9.0.1".into()),
        })
        .unwrap();
    ui.app.drain(&ui.context);
    assert_eq!(ui.app.update_status, UpdateStatus::Failed);
    let output = ui.scroll_content_to("重试检查");
    ui.click(text_center(&output, "重试检查"));
    ui.app
        .tx
        .send(Message::UpdateChecked {
            release: Ok(updater::test_release("9.0.2")),
            current_version: Some("9.0.1".into()),
        })
        .unwrap();
    ui.app.drain(&ui.context);
    assert_eq!(ui.app.update_status, UpdateStatus::Available);
    let output = ui.scroll_content_to("下载并更新");
    ui.click(text_center(&output, "下载并更新"));
    assert_eq!(ui.app.update_status, UpdateStatus::Installing);
    ui.app
        .tx
        .send(Message::UpdateProgress("正在下载：50%".into()))
        .unwrap();
    ui.app
        .tx
        .send(Message::UpdateChecked {
            release: Err("迟到的检查消息".into()),
            current_version: None,
        })
        .unwrap();
    ui.app.drain(&ui.context);
    assert_eq!(ui.app.update_status, UpdateStatus::Installing);
    assert_eq!(ui.app.update_message, "正在下载：50%");
    ui.app
        .tx
        .send(Message::UpdateInstalled(Err("校验失败，保留原引擎".into())))
        .unwrap();
    ui.app.drain(&ui.context);
    assert_eq!(ui.app.update_status, UpdateStatus::Failed);
    ui.app
        .tx
        .send(Message::Tools(Ok(fixture_tools("9.0.1"))))
        .unwrap();
    ui.app.drain(&ui.context);
    let output = ui.scroll_content_to("重试更新");
    ui.click(text_center(&output, "重试更新"));
    assert_eq!(ui.app.update_status, UpdateStatus::Installing);
    ui.app
        .tx
        .send(Message::UpdateInstalled(Ok(PathBuf::from(
            "managed/9.0.2/bin",
        ))))
        .unwrap();
    ui.app.drain(&ui.context);
    ui.app
        .tx
        .send(Message::Tools(Ok(fixture_tools("9.0.2"))))
        .unwrap();
    ui.app.drain(&ui.context);
    assert_eq!(ui.app.update_status, UpdateStatus::Installed);
    assert_eq!(
        ui.app.installed_update_dir,
        Some(PathBuf::from("managed/9.0.2/bin"))
    );
    let point = ui.scroll_card_control_to("FFmpeg 更新", "检查更新");
    ui.click(point);
    ui.app
        .tx
        .send(Message::UpdateChecked {
            release: Ok(updater::test_release("9.0.2")),
            current_version: Some("9.0.2".into()),
        })
        .unwrap();
    ui.app.drain(&ui.context);
    assert_eq!(ui.app.update_status, UpdateStatus::Current);
    let output = ui.scroll_content_to("默认引擎已是最新版本（9.0.2）。");
    assert!(painted_text_bounds_if_present(&output, "下载并更新").is_none());
}

#[test]
fn update_and_engine_changes_are_serialized_with_media_jobs_dialogs_and_detection() {
    if !updater::supported() {
        return;
    }
    let mut ui = Harness::new();
    ui.app.tools = Some(Arc::new(fixture_tools("9.0.1")));
    ui.app.update_status = UpdateStatus::Available;
    ui.app.update_release = Some(updater::test_release("9.0.2"));
    ui.app.prefs.tools_dir = Some(PathBuf::from("custom-engine"));
    ui.add_file("queued.mp4");
    ui.app.entries[0].status = Status::Queued;
    ui.app.install_engine_update(&ui.context);
    assert_eq!(ui.app.update_status, UpdateStatus::Available);
    ui.app.restore_default_engine(&ui.context);
    assert_eq!(ui.app.prefs.tools_dir, Some(PathBuf::from("custom-engine")));
    ui.app
        .tx
        .send(Message::ToolsDirectory(PathBuf::from("late-dialog")))
        .unwrap();
    ui.app.drain(&ui.context);
    assert_eq!(ui.app.prefs.tools_dir, Some(PathBuf::from("custom-engine")));
    // Checking metadata is allowed while media is queued.
    ui.app.check_engine_updates(&ui.context);
    assert_eq!(ui.app.update_status, UpdateStatus::Checking);
    ui.app.entries[0].status = Status::Ready;
    for (checking, dialog) in [(true, false), (false, true)] {
        ui.app.checking = checking;
        ui.app.dialog_open = dialog;
        ui.app.update_status = UpdateStatus::Available;
        ui.app.update_release = Some(updater::test_release("9.0.2"));
        ui.app.install_engine_update(&ui.context);
        assert_eq!(ui.app.update_status, UpdateStatus::Available);
        ui.app.start_batch(&ui.context);
        assert!(ui.app.entries[0].status == Status::Ready && ui.app.active.is_none());
    }
    ui.app.checking = false;
    ui.app.dialog_open = false;
    ui.app.install_engine_update(&ui.context);
    assert_eq!(ui.app.update_status, UpdateStatus::Installing);
    ui.app.start_batch(&ui.context);
    assert!(ui.app.entries[0].status == Status::Ready && ui.app.active.is_none());
    ui.app.dialog(&ui.context, 2);
    assert!(!ui.app.dialog_open);
    ui.app
        .import(vec![PathBuf::from("added-during-update.mp4")], &ui.context);
    assert_eq!(
        ui.app.entries.len(),
        1,
        "An installer must not overlap new probes"
    );
    assert_eq!(
        ui.app.pending_files,
        [PathBuf::from("added-during-update.mp4")]
    );
    ui.app
        .tx
        .send(Message::UpdateInstalled(Ok(PathBuf::from(
            "managed/9.0.2/bin",
        ))))
        .unwrap();
    ui.app.drain(&ui.context);
    assert_eq!(
        ui.app.prefs.tools_dir,
        Some(PathBuf::from("custom-engine")),
        "Installing a default update must preserve the chosen custom engine"
    );
    assert!(
        ui.app.checking,
        "After activation the default/custom selection is rediscovered"
    );
}

#[test]
fn update_comparison_uses_current_default_engine_and_rechecks_after_tool_detection() {
    let mut ui = Harness::new();
    ui.app.tools = Some(Arc::new(fixture_tools("9.0.2")));
    ui.app.update_status = UpdateStatus::Checking;
    ui.app
        .tx
        .send(Message::UpdateChecked {
            release: Ok(updater::test_release("9.0.2")),
            current_version: Some("9.0.1".into()),
        })
        .unwrap();
    ui.app.drain(&ui.context);
    assert_eq!(
        ui.app.update_status,
        UpdateStatus::Current,
        "Late update metadata must not use a stale current version"
    );
    ui.app.update_status = UpdateStatus::Available;
    ui.app
        .tx
        .send(Message::Tools(Ok(fixture_tools("9.0.3"))))
        .unwrap();
    ui.app.drain(&ui.context);
    assert_eq!(ui.app.update_status, UpdateStatus::Current);
    ui.app.prefs.tools_dir = Some(PathBuf::from("custom-engine"));
    ui.app.tools = Some(Arc::new(fixture_tools("10.0.0")));
    ui.app.update_status = UpdateStatus::Checking;
    ui.app
        .tx
        .send(Message::UpdateChecked {
            release: Ok(updater::test_release("9.0.2")),
            current_version: Some("9.0.1".into()),
        })
        .unwrap();
    ui.app.drain(&ui.context);
    assert_eq!(
        ui.app.update_status,
        UpdateStatus::Available,
        "A custom version is independent of updates to the managed default engine"
    );
}

#[test]
fn sidebar_collapses_navigates_and_persists() {
    let mut ui = Harness::new();
    ui.settle();
    ui.click(pos2(36., 38.));
    assert!(ui.app.prefs.sidebar_collapsed);
    let output = ui.settle();
    assert!(painted_text_bounds_if_present(&output, "格式转换").is_none());
    ui.click(pos2(36., 38. + 4. * 44.));
    assert_eq!(ui.app.prefs.export.operation, Operation::Audio);
    ui.click(pos2(36., 38.));
    assert!(!ui.app.prefs.sidebar_collapsed);
    assert!(painted_text_bounds_if_present(&ui.settle(), "音频处理").is_some());
    ui.app.prefs.sidebar_collapsed = true;
    let restored: Preferences =
        serde_json::from_str(&serde_json::to_string(&ui.app.prefs).unwrap()).unwrap();
    assert!(restored.sidebar_collapsed);
}

#[test]
fn unavailable_gpu_blocks_batch_and_encoding_choices_persist() {
    let mut ui = Harness::new();
    ui.add_file("sample.mp4");
    ui.app.tools = Some(Arc::new(fixture_tools("9.0.2")));
    ui.app.prefs.export.video_encoder = VideoEncoder::Nvidia;
    ui.app.prefs.export.video_codec = VideoCodec::H265;
    ui.app.prefs.export.audio_bitrate = 64;
    ui.app.start_batch(&ui.context);
    assert!(!ui.app.batch);
    assert!(ui.app.notice.as_ref().unwrap().contains("不可用"));
    let restored: Preferences =
        serde_json::from_str(&serde_json::to_string(&ui.app.prefs).unwrap()).unwrap();
    assert_eq!(restored.export.video_encoder, VideoEncoder::Nvidia);
    assert_eq!(restored.export.video_codec, VideoCodec::H265);
    assert_eq!(restored.export.audio_bitrate, 64);
}

#[test]
fn encoding_and_audio_controls_accept_pointer_selection() {
    let mut ui = Harness::new();
    let mut tools = fixture_tools("9.0.2");
    tools
        .gpu_encoders
        .push((VideoEncoder::Nvidia, VideoCodec::H265));
    ui.app.tools = Some(Arc::new(tools));
    let output = ui.scroll_export_to("H.265");
    ui.click(text_center(&output, "H.265"));
    assert_eq!(ui.app.prefs.export.video_codec, VideoCodec::H265);
    let output = ui.scroll_content_to("CPU");
    ui.click(text_center(&output, "CPU"));
    let output = ui.settle();
    ui.click(text_center(&output, "GPU · NVIDIA"));
    assert_eq!(ui.app.prefs.export.video_encoder, VideoEncoder::Nvidia);
    open_advanced_group(&mut ui, "音频");
    let output = ui.scroll_content_to("192 kbps");
    ui.click(text_center(&output, "192 kbps"));
    let output = ui.settle();
    ui.click(text_center(&output, "64 kbps"));
    assert_eq!(ui.app.prefs.export.audio_bitrate, 64);
    ui.app.change_operation(Operation::Audio);
    ui.app.prefs.export.format = "flac".into();
    let output = ui.scroll_content_to("音频位深");
    assert!(painted_text_bounds_if_present(&output, "192 kbps").is_none());
    assert!(painted_text_bounds_if_present(&output, "音频码率").is_none());
}

fn open_advanced_group(ui: &mut Harness, label: &str) {
    let output = ui.scroll_export_to(label);
    ui.click(text_center(&output, label));
    ui.settle();
}

#[test]
fn advanced_rate_audio_and_device_controls_are_pointer_accessible() {
    use crate::encoding::{AudioRateMode, HardwareDecode, VideoRateMode};
    for system_theme in [Theme::Light, Theme::Dark] {
        let mut ui = Harness::new();
        ui.system_theme = system_theme;
        ui.viewport = vec2(960., 660.);
        open_advanced_group(&mut ui, "画质与尺寸");
        let output = ui.scroll_content_to("质量");
        ui.click(text_center(&output, "质量"));
        let output = ui.settle();
        ui.click(text_center(&output, "目标体积"));
        assert_eq!(
            ui.app.prefs.export.encoding.video_rate_mode,
            VideoRateMode::TargetSize
        );
        let output = ui.scroll_content_to("两遍编码");
        ui.click(text_center(&output, "启用"));
        assert!(ui.app.prefs.export.encoding.two_pass);
        // VBR is intentionally unavailable for a fixed target size. Switch to
        // target bitrate before exercising quality-based audio encoding.
        let output = ui.scroll_content_to("控制方式");
        let control = text_centers(&output, "目标体积")[0];
        ui.click(control);
        let output = ui.settle();
        ui.click(text_center(&output, "目标码率"));
        assert_eq!(
            ui.app.prefs.export.encoding.video_rate_mode,
            VideoRateMode::Bitrate
        );
        open_advanced_group(&mut ui, "音频");
        let output = ui.scroll_content_to("源采样率");
        ui.click(text_center(&output, "源采样率"));
        let output = ui.settle();
        ui.click(text_center(&output, "16000 Hz"));
        assert_eq!(ui.app.prefs.export.encoding.audio_sample_rate, 16000);
        let output = ui.scroll_content_to("源声道");
        ui.click(text_center(&output, "源声道"));
        let output = ui.settle();
        ui.click(text_center(&output, "单声道"));
        assert_eq!(ui.app.prefs.export.encoding.audio_channels, 1);
        let output = ui.scroll_content_to("质量 / VBR");
        ui.click(text_center(&output, "质量 / VBR"));
        assert_eq!(
            ui.app.prefs.export.encoding.audio_rate_mode,
            AudioRateMode::Quality
        );

        // Switching an already-configured job to GPU removes incompatible CPU-only state.
        ui.app.prefs.export.video_encoder = VideoEncoder::Nvidia;
        ui.app.prefs.export.resolution = 240;
        open_advanced_group(&mut ui, "硬件选项");
        let output = ui.scroll_content_to("硬件解码");
        ui.click(text_center(&output, "CPU"));
        let output = ui.settle();
        ui.click(text_center(&output, "NVIDIA CUDA"));
        assert_eq!(
            ui.app.prefs.export.encoding.hardware_decode,
            HardwareDecode::Cuda
        );
        assert!(!ui.app.prefs.export.encoding.two_pass);
        let output = ui.scroll_content_to("显卡编号");
        let label = painted_text_bounds(&output, "显卡编号").0;
        ui.click(pos2(label.left() + 145., label.center().y));
        assert!(ui.context.wants_keyboard_input());
        ui.frame(vec![command_key(egui::Key::A)]);
        ui.frame(vec![Event::Text("0".into())]);
        assert_eq!(ui.app.prefs.export.encoding.gpu_device, "0");
        let output = ui.scroll_content_to("GPU 缩放");
        ui.click(text_center(&output, "启用"));
        assert!(ui.app.prefs.export.encoding.gpu_scale);
        let (start, clip) = painted_text_bounds(&ui.settle(), "开始处理");
        assert!(clip.expand(0.5).contains_rect(start));
    }
}

#[test]
fn lossless_audio_depth_and_effect_controls_accept_pointer_changes() {
    let mut ui = Harness::new();
    ui.viewport = vec2(960., 660.);
    ui.app.change_operation(Operation::Audio);
    ui.app.prefs.export.format = "flac".into();
    let output = ui.scroll_export_to("音频位深");
    ui.click(text_center(&output, "自动"));
    let output = ui.settle();
    ui.click(text_center(&output, "24 位"));
    assert_eq!(ui.app.prefs.export.encoding.audio_bit_depth, 24);

    ui.app.change_operation(Operation::Convert);
    let output = ui.scroll_export_to("旋转与翻转");
    ui.click(text_center(&output, "旋转与翻转"));
    let output = ui.scroll_content_to("不旋转");
    ui.click(text_center(&output, "不旋转"));
    let output = ui.settle();
    ui.click(text_center(&output, "顺时针 90°"));
    assert_eq!(
        ui.app.prefs.export.effects.rotation,
        crate::effects::Rotation::Clockwise90
    );
    let output = ui.scroll_content_to("水平翻转");
    ui.click(text_center(&output, "水平翻转"));
    assert!(ui.app.prefs.export.effects.flip_horizontal);
    open_advanced_group(&mut ui, "音频");
    let output = ui.scroll_content_to("音频混音");
    ui.click(text_center(&output, "音频混音"));
    let output = ui.scroll_content_to("启用混音");
    ui.click(text_center(&output, "启用混音"));
    assert!(ui.app.prefs.export.effects.mix.enabled);
    let output = ui.scroll_content_to("混入所选音轨");
    ui.click(text_center(&output, "混入所选音轨"));
    assert!(!ui.app.prefs.export.effects.mix.include_source);
}

#[test]
fn subtitle_navigation_selects_real_source_track_and_builds_extract_plan() {
    use crate::tracks::{TrackInfo, TrackKind};
    let fixture = TimePlanFile::new();
    let mut ui = Harness::new();
    ui.viewport = vec2(960., 660.);
    ui.app.tools = Some(Arc::new(fixture_tools("9.0.2")));
    let id = ui.add_file(fixture.0.to_str().unwrap());
    ui.app.selected = Some(id);
    ui.app.entries[0].media.as_mut().unwrap().tracks = vec![TrackInfo {
        index: 2,
        kind: TrackKind::Subtitle,
        codec: "subrip".into(),
        language: "zho".into(),
        title: "中文字幕".into(),
        default: true,
    }];
    let output = ui.settle();
    ui.click(text_center(&output, "字幕提取"));
    assert_eq!(ui.app.prefs.export.operation, Operation::Subtitle);
    assert_eq!(ui.app.prefs.export.format, "srt");
    let output = ui.scroll_export_to("#2 · SUBRIP · zho · 中文字幕");
    ui.click(text_center(&output, "#2 · SUBRIP · zho · 中文字幕"));
    assert_eq!(ui.app.prefs.export.tracks.subtitles.len(), 1);
    assert_eq!(ui.app.prefs.export.tracks.subtitles[0].index, 2);
    let output = ui.settle();
    assert!(painted_text_bounds_if_present(&output, "编码参数").is_none());
    let plan = media::plan(
        ui.app.tools.as_ref().unwrap(),
        ui.app.entries[0].media.as_ref().unwrap(),
        &ui.app.prefs.export,
    )
    .unwrap();
    let args: Vec<_> = plan
        .args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(args.windows(2).any(|pair| pair == ["-map", "0:2"]));
    assert!(args.windows(2).any(|pair| pair == ["-c:s", "srt"]));
    assert!(plan.first_pass.is_none());
    let output = ui.settle();
    ui.click(text_center(&output, "格式转换"));
    assert_eq!(ui.app.prefs.export.operation, Operation::Convert);
    assert_eq!(ui.app.entries[0].id, id);
}

#[test]
fn batch_captures_independent_advanced_settings_for_every_entry() {
    use crate::encoding::VideoRateMode;
    let mut ui = Harness::new();
    ui.app.tools = Some(Arc::new(fixture_tools("9.0.2")));
    // Missing sources intentionally stop planning before any process can be launched.
    ui.add_file("frameflow-capture-test-intentionally-missing-1.mp4");
    ui.add_file("frameflow-capture-test-intentionally-missing-2.mp4");
    for entry in &ui.app.entries {
        assert!(!entry.path.exists());
    }
    ui.app.prefs.export.encoding.video_rate_mode = VideoRateMode::TargetSize;
    ui.app.prefs.export.encoding.target_size_mb = 8.0;
    ui.app.prefs.export.encoding.two_pass = true;
    ui.app.prefs.export.encoding.audio_sample_rate = 44100;
    ui.app.prefs.export.encoding.audio_channels = 1;
    ui.app.prefs.export.effects.flip_horizontal = true;
    ui.app.prefs.export.tracks.audio = Some(vec![crate::tracks::TrackSelection {
        index: 1,
        language: "jpn".into(),
        title: "日本語".into(),
        default: true,
    }]);
    ui.app.start_batch(&ui.context);
    assert!(ui.app.worker.is_none());
    ui.app.prefs.export.encoding.target_size_mb = 99.0;
    ui.app.prefs.export.encoding.audio_channels = 2;
    ui.app.prefs.export.effects.flip_horizontal = false;
    ui.app.prefs.export.tracks.audio.as_mut().unwrap()[0].language = "eng".into();
    for entry in &ui.app.entries {
        let captured = entry
            .settings
            .as_ref()
            .expect("Every entry captures export settings before planning");
        assert_eq!(captured.encoding.video_rate_mode, VideoRateMode::TargetSize);
        assert_eq!(captured.encoding.target_size_mb, 8.0);
        assert!(captured.encoding.two_pass);
        assert_eq!(captured.encoding.audio_sample_rate, 44100);
        assert_eq!(captured.encoding.audio_channels, 1);
        assert!(captured.effects.flip_horizontal);
        assert_eq!(captured.tracks.audio.as_ref().unwrap()[0].language, "jpn");
        assert!(entry.status == Status::Failed);
    }
}

#[test]
fn long_hover_over_elided_filenames_icons_links_and_values_never_opens_tooltips() {
    for system_theme in [Theme::Light, Theme::Dark] {
        let mut ui = Harness::new();
        ui.viewport = vec2(960., 660.);
        ui.system_theme = system_theme;
        let filename = format!("{}.mp4", "很长的本地媒体文件名称".repeat(20));
        ui.add_file(&filename);
        let output = ui.settle();
        let (bounds, clip) = painted_text_bounds(&output, &filename);
        ui.hover_without_tooltip(bounds.intersect(clip).center());
        let output = ui.settle();
        ui.hover_without_tooltip(button_bounds(&output, 30.)[0].center());

        ui.settle();
        ui.click(pos2(36., 38.));
        ui.hover_without_tooltip(pos2(36., 82.));
        ui.click(pos2(36., 38.));
        let output = ui.settle();
        ui.click(text_center(&output, "设置"));
        let output = ui.scroll_content_to("FFmpeg 下载页面");
        ui.hover_without_tooltip(text_center(&output, "FFmpeg 下载页面"));
        let output = ui.scroll_content_to("使用文档");
        ui.hover_without_tooltip(text_center(&output, "使用文档"));

        let output = ui.settle();
        ui.click(text_center(&output, Operation::Audio.label()));
        let output = ui.scroll_export_to("开始");
        ui.hover_without_tooltip(text_center(&output, "开始") + vec2(90., 0.));
        assert!(!ui.context.style().explanation_tooltips);
        assert!(!ui.context.style().url_in_tooltip);
    }
}

#[test]
fn each_workflow_exposes_its_own_export_sections_without_duplicate_generic_panels() {
    for viewport in [vec2(1240., 820.), vec2(960., 660.)] {
        for op in Operation::ALL {
            let mut ui = Harness::new();
            ui.viewport = viewport;
            ui.add_file("source.mp4");
            ui.source_tracks();
            let output = ui.settle();
            ui.click(text_center(&output, op.label()));
            let labels = ui.export_labels();
            let (required, forbidden): (&[&str], &[&str]) = match op {
                Operation::Convert => (
                    &[
                        "输出视频",
                        "文件格式",
                        "视频编码",
                        "执行设备",
                        "画质与尺寸",
                        "音频",
                        "字幕",
                        "画面编辑",
                    ],
                    &["时间范围", "压缩目标", "输出音频", "字幕格式"],
                ),
                Operation::Compress => (
                    &[
                        "压缩目标",
                        "输出视频",
                        "视频编码",
                        "执行设备",
                        "尺寸与帧率",
                        "音频",
                        "字幕",
                        "画面编辑",
                    ],
                    &["画质与尺寸", "时间范围", "输出音频", "字幕格式"],
                ),
                Operation::Trim => (
                    &[
                        "时间范围",
                        "裁剪方式",
                        "精确裁剪",
                        "无损裁剪",
                        "输出视频",
                        "画质与尺寸",
                        "音频",
                        "字幕",
                    ],
                    &["压缩目标", "输出音频", "字幕格式"],
                ),
                Operation::Audio => (
                    &[
                        "时间范围",
                        "输出音频",
                        "音频码率",
                        "采样率",
                        "声道",
                        "音轨与混音",
                        "音频混音",
                    ],
                    &["视频编码", "执行设备", "画面编辑", "字幕", "画面尺寸"],
                ),
                Operation::Gif => (
                    &["时间范围", "动画参数", "画面尺寸", "帧率", "画面编辑"],
                    &[
                        "文件格式",
                        "视频编码",
                        "执行设备",
                        "音频",
                        "字幕",
                        "音频混音",
                    ],
                ),
                Operation::Snapshot => (
                    &["截取位置", "输出图片", "文件格式", "画面尺寸", "画面编辑"],
                    &[
                        "结束",
                        "帧率",
                        "视频编码",
                        "执行设备",
                        "音频",
                        "字幕",
                        "音频混音",
                    ],
                ),
                Operation::Remux => (
                    &[
                        "封装格式",
                        "文件格式",
                        "音频",
                        "首条音轨",
                        "外部音轨",
                        "添加音频",
                        "字幕",
                        "添加字幕",
                    ],
                    &[
                        "视频编码",
                        "执行设备",
                        "硬件选项",
                        "画面编辑",
                        "采样率",
                        "音频码率",
                        "烧录字幕",
                    ],
                ),
                Operation::Subtitle => (
                    &[
                        "选择字幕",
                        "字幕格式",
                        "文件格式",
                        "#2 · SUBRIP · zho · 中文",
                        "#4 · ASS · eng · English",
                    ],
                    &[
                        "轨道信息",
                        "字体附件",
                        "音频",
                        "执行设备",
                        "画面编辑",
                        "添加字幕",
                        "烧录字幕",
                    ],
                ),
            };
            for label in required {
                assert_eq!(
                    labels.get(*label),
                    Some(&1),
                    "{op:?} needs one {label:?} at {viewport:?}"
                );
            }
            for label in forbidden.iter().copied().chain([
                "导出设置",
                "编码参数",
                "轨道与字幕",
                "画面与声音",
                "码率、音频格式与硬件加速",
                "旋转、水印、合成与混音",
                "移除音频",
                "移除源音轨，仅保留添加的音频",
            ]) {
                assert!(
                    !labels.contains_key(label),
                    "{op:?} includes unrelated or obsolete {label:?}"
                );
            }
            assert_eq!(labels.get(workflow_tab(op)), Some(&1));
            assert_eq!(labels.get("选择文件夹"), Some(&1));
            assert_eq!(labels.get("开始处理"), Some(&1));
        }
    }
}

#[test]
fn track_controls_offer_three_audio_modes_and_single_selection_for_audio_and_extraction() {
    let mut ui = Harness::new();
    ui.viewport = vec2(960., 660.);
    ui.add_file("tracks.mp4");
    ui.source_tracks();
    let output = ui.settle();
    ui.click(text_center(&output, Operation::Audio.label()));
    let output = ui.scroll_export_to("选择音轨");
    ui.click(text_center(&output, "选择音轨"));
    assert_eq!(
        ui.app.prefs.export.tracks.audio.as_ref().unwrap()[0].index,
        1
    );
    let output = ui.scroll_content_to("#3 · AAC · eng · 配音");
    ui.click(text_center(&output, "#3 · AAC · eng · 配音"));
    let chosen = ui.app.prefs.export.tracks.audio.as_ref().unwrap();
    assert_eq!(chosen.len(), 1);
    assert_eq!(chosen[0].index, 3);
    let output = ui.scroll_content_to("不保留");
    ui.click(text_center(&output, "不保留"));
    assert!(
        ui.app
            .prefs
            .export
            .tracks
            .audio
            .as_ref()
            .unwrap()
            .is_empty()
    );
    assert!(!ui.app.prefs.export.muted);
    let output = ui.scroll_content_to("首条音轨");
    ui.click(text_center(&output, "首条音轨"));
    assert!(ui.app.prefs.export.tracks.audio.is_none());

    let output = ui.settle();
    ui.click(text_center(&output, Operation::Subtitle.label()));
    for (label, index) in [
        ("#2 · SUBRIP · zho · 中文", 2),
        ("#4 · ASS · eng · English", 4),
    ] {
        let output = ui.scroll_content_to(label);
        ui.click(text_center(&output, label));
        assert_eq!(ui.app.prefs.export.tracks.subtitles.len(), 1);
        assert_eq!(ui.app.prefs.export.tracks.subtitles[0].index, index);
        let output = ui.settle();
        for omitted in ["轨道信息", "默认轨", "外部字幕…", "字体附件"] {
            assert!(painted_text_bounds_if_present(&output, omitted).is_none());
        }
    }
}

#[test]
fn copy_workflows_preserve_multiple_audio_tracks_despite_dormant_mix_settings() {
    use crate::tracks::{TrackKind, TrackSelection};
    for op in [Operation::Trim, Operation::Remux] {
        let mut ui = Harness::new();
        ui.viewport = vec2(960., 660.);
        ui.add_file("multi-audio.mp4");
        ui.source_tracks();
        ui.app.change_operation(op);
        ui.app.prefs.export.lossless_trim = op == Operation::Trim;
        ui.app.prefs.export.effects.mix.enabled = true;
        ui.app.prefs.export.tracks.audio = Some(
            ui.app.entries[0]
                .media
                .as_ref()
                .unwrap()
                .tracks
                .iter()
                .filter(|track| track.kind == TrackKind::Audio)
                .map(TrackSelection::from)
                .collect(),
        );
        if op == Operation::Trim {
            open_advanced_group(&mut ui, "音频");
        } else {
            ui.show_export();
        }
        let output = ui.scroll_content_to("#1 · AAC · zho · 原声");
        assert_eq!(
            ui.app.prefs.export.tracks.audio.as_ref().unwrap().len(),
            2,
            "{op:?} copies audio; rendering must not apply the dormant mix single-track restriction",
        );
        // A checked copy-workflow row remains a checkbox: deselecting and
        // reselecting one track must retain the other selected source track.
        ui.click(text_center(&output, "#1 · AAC · zho · 原声"));
        let chosen = ui.app.prefs.export.tracks.audio.as_ref().unwrap();
        assert_eq!(chosen.len(), 1);
        assert_eq!(chosen[0].index, 3);
        let output = ui.scroll_content_to("#1 · AAC · zho · 原声");
        ui.click(text_center(&output, "#1 · AAC · zho · 原声"));
        let mut selected: Vec<_> = ui
            .app
            .prefs
            .export
            .tracks
            .audio
            .as_ref()
            .unwrap()
            .iter()
            .map(|track| track.index)
            .collect();
        selected.sort_unstable();
        assert_eq!(selected, [1, 3]);
        assert!(
            ui.app.prefs.export.effects.mix.enabled,
            "Copy mode should leave the dormant effect preference available when returning to encoding"
        );

        if op == Operation::Trim {
            let output = ui.scroll_content_to("精确裁剪");
            ui.click(text_center(&output, "精确裁剪"));
            ui.settle();
            assert!(!ui.app.prefs.export.lossless_trim);
            assert_eq!(
                ui.app.prefs.export.tracks.audio.as_ref().unwrap().len(),
                1,
                "Mixing during precise trim requires one source audio track"
            );
            for (label, index) in [("#1 · AAC · zho · 原声", 1), ("#3 · AAC · eng · 配音", 3)]
            {
                let output = ui.scroll_content_to(label);
                ui.click(text_center(&output, label));
                let chosen = ui.app.prefs.export.tracks.audio.as_ref().unwrap();
                assert_eq!(chosen.len(), 1);
                assert_eq!(chosen[0].index, index);
            }
        }
    }
}

#[test]
fn workflow_options_are_isolated_restored_and_persisted_without_reactivating_local_assets() {
    use crate::{
        effects::MixTrack,
        encoding::{AudioRateMode, VideoRateMode},
        tracks::{SubtitleMode, TrackSelection},
    };
    let mut ui = Harness::new();
    ui.add_file("source.mp4");
    ui.source_tracks();
    ui.app.prefs.export.encoding.video_rate_mode = VideoRateMode::TargetSize;
    ui.app.prefs.export.encoding.target_size_mb = 19.5;
    ui.app.prefs.export.effects.watermark.enabled = true;
    ui.app.prefs.export.effects.watermark.path = Some(PathBuf::from("watermark.png"));
    ui.app.prefs.export.effects.watermark.width_percent = 37;
    ui.app.prefs.export.effects.mix.enabled = true;
    ui.app.prefs.export.effects.mix.source_gain = 0.7;
    ui.app
        .prefs
        .export
        .effects
        .mix
        .tracks
        .push(MixTrack::new(PathBuf::from("music.wav")));
    ui.app.prefs.export.tracks.mode = SubtitleMode::Burn;
    ui.app.prefs.export.tracks.subtitles.push(TrackSelection {
        index: 2,
        language: "zho".into(),
        title: "中文".into(),
        default: true,
    });
    ui.app
        .prefs
        .export
        .subtitle_files
        .push(PathBuf::from("external.srt"));
    let output = ui.settle();
    ui.click(text_center(&output, Operation::Audio.label()));
    assert_eq!(
        ui.app.prefs.export.encoding.video_rate_mode,
        VideoRateMode::Quality
    );
    assert!(!ui.app.prefs.export.effects.watermark.enabled);
    assert!(!ui.app.prefs.export.effects.mix.enabled);
    assert!(ui.app.prefs.export.tracks.subtitles.is_empty());
    assert!(ui.app.prefs.export.subtitle_files.is_empty());
    ui.app.prefs.export.encoding.audio_rate_mode = AudioRateMode::Quality;
    ui.app.prefs.export.encoding.audio_quality = 7;
    ui.app.prefs.export.encoding.audio_sample_rate = 44100;
    ui.app.prefs.export.output_dir = Some(PathBuf::from("chosen-export-folder"));
    for op in [
        Operation::Compress,
        Operation::Trim,
        Operation::Gif,
        Operation::Snapshot,
        Operation::Remux,
        Operation::Subtitle,
    ] {
        let output = ui.settle();
        ui.click(text_center(&output, op.label()));
        assert_eq!(ui.app.prefs.export.operation, op);
        assert_eq!(
            ui.app.prefs.export.encoding.video_rate_mode,
            VideoRateMode::Quality
        );
        assert_eq!(ui.app.prefs.export.encoding.audio_sample_rate, 0);
        assert!(!ui.app.prefs.export.effects.watermark.enabled);
        assert!(ui.app.prefs.export.effects.watermark.path.is_none());
        assert!(!ui.app.prefs.export.effects.mix.enabled);
        assert!(ui.app.prefs.export.effects.mix.tracks.is_empty());
        assert!(ui.app.prefs.export.tracks.subtitles.is_empty());
        assert!(ui.app.prefs.export.subtitle_files.is_empty());
        assert_eq!(
            ui.app.prefs.export.output_dir,
            Some(PathBuf::from("chosen-export-folder"))
        );
    }
    let output = ui.settle();
    ui.click(text_center(&output, Operation::Convert.label()));
    let restored = &ui.app.prefs.export;
    assert_eq!(restored.encoding.video_rate_mode, VideoRateMode::TargetSize);
    assert_eq!(restored.encoding.target_size_mb, 19.5);
    assert!(restored.effects.watermark.enabled && restored.effects.mix.enabled);
    assert_eq!(
        restored.effects.watermark.path,
        Some(PathBuf::from("watermark.png"))
    );
    assert_eq!(
        restored.effects.mix.tracks[0].path,
        PathBuf::from("music.wav")
    );
    assert_eq!(restored.tracks.mode, SubtitleMode::Burn);
    assert_eq!(restored.tracks.subtitles[0].index, 2);
    assert_eq!(restored.subtitle_files, [PathBuf::from("external.srt")]);

    let serialized = serde_json::to_string(&ui.app.prefs).unwrap();
    let mut reopened = Harness::new();
    reopened.app.prefs = serde_json::from_str(&serialized).unwrap();
    reopened.app.change_operation(Operation::Audio);
    assert_eq!(
        reopened.app.prefs.export.encoding.audio_rate_mode,
        AudioRateMode::Quality
    );
    assert_eq!(reopened.app.prefs.export.encoding.audio_quality, 7);
    assert_eq!(reopened.app.prefs.export.encoding.audio_sample_rate, 44100);
    reopened.app.change_operation(Operation::Convert);
    let restored = &reopened.app.prefs.export;
    assert_eq!(restored.encoding.target_size_mb, 19.5);
    assert_eq!(restored.effects.watermark.width_percent, 37);
    assert_eq!(restored.effects.mix.source_gain, 0.7);
    assert_eq!(restored.tracks.subtitles[0].index, 2);
    assert!(!restored.effects.watermark.enabled && restored.effects.watermark.path.is_none());
    assert!(!restored.effects.mix.enabled && restored.effects.mix.tracks.is_empty());
    assert!(restored.subtitle_files.is_empty());
    let legacy: Preferences = serde_json::from_str("{}").unwrap();
    assert!(legacy.workflow_settings.is_empty());
}

#[test]
fn lossless_and_container_changes_clear_only_incompatible_controls() {
    use crate::tracks::SubtitleMode;
    let mut ui = Harness::new();
    ui.app.change_operation(Operation::Trim);
    ui.app.prefs.export.tracks.mode = SubtitleMode::Burn;
    ui.app.prefs.export.muted = true;
    ui.app.prefs.export.format = "mkv".into();
    ui.app.prefs.export.tracks.preserve_attachments = true;
    ui.app
        .prefs
        .export
        .tracks
        .font_attachments
        .push(PathBuf::from("font.ttf"));
    let output = ui.scroll_export_to("无损裁剪");
    ui.click(text_center(&output, "无损裁剪"));
    ui.settle();
    assert!(ui.app.prefs.export.lossless_trim);
    assert_eq!(ui.app.prefs.export.tracks.mode, SubtitleMode::Keep);
    assert!(!ui.app.prefs.export.muted);
    assert!(
        ui.app
            .prefs
            .export
            .tracks
            .audio
            .as_ref()
            .unwrap()
            .is_empty()
    );
    assert!(ui.app.prefs.export.tracks.preserve_attachments);
    let labels = ui.export_labels();
    for omitted in ["视频编码", "执行设备", "画质与尺寸", "画面编辑", "硬件选项"]
    {
        assert!(!labels.contains_key(omitted));
    }
    let output = ui.scroll_content_to("MKV");
    ui.click(text_center(&output, "MKV"));
    let output = ui.settle();
    ui.click(text_center(&output, "MP4"));
    ui.settle();
    assert!(ui.app.prefs.export.tracks.font_attachments.is_empty());
    assert!(!ui.app.prefs.export.tracks.preserve_attachments);
    let output = ui.scroll_content_to("精确裁剪");
    ui.click(text_center(&output, "精确裁剪"));
    let output = ui.scroll_content_to("视频编码");
    assert!(painted_text_bounds_if_present(&output, "H.264").is_some());
}

#[test]
fn settings_layout_removes_footer_and_places_software_update_and_help_correctly() {
    let mut ui = Harness::new();
    ui.viewport = vec2(1240., 900.);
    ui.app.page = WorkspacePage::Preferences;
    ui.app.tools = Some(Arc::new(fixture_tools("9.0.2")));
    let output = ui.settle();
    for removed in [
        "选择应用外观。",
        "收起菜单",
        "展开菜单",
        "FFmpeg 9.0.2 · 已就绪",
        "帧流 Frameflow",
        "关于与帮助",
    ] {
        assert!(
            painted_text_bounds_if_present(&output, removed).is_none(),
            "Unexpected caption: {removed}"
        );
    }
    let (engine, _) = painted_text_bounds(&output, "FFmpeg 更新");
    let (software, _) = painted_text_bounds(&output, "软件更新");
    assert!(software.left() > engine.right());
    assert!((software.top() - engine.top()).abs() < 2.);
    let (theme_label, _) = painted_text_bounds(&output, "应用主题");
    let (theme_control, _) = painted_text_bounds(&output, "跟随系统");
    assert!(theme_control.left() > theme_label.right() + 400.);
    assert!((theme_control.center().y - theme_label.center().y).abs() < 4.);
    assert!(painted_text_bounds_if_present(&output, "浅色").is_none());
    assert!(painted_text_bounds_if_present(&output, "深色").is_none());
    for label in [
        format!("当前版本 {}", env!("CARGO_PKG_VERSION")),
        "版本发布页".into(),
        "GitHub 项目".into(),
    ] {
        let (bounds, clip) = painted_text_bounds(&output, &label);
        assert!(
            bounds.left() > software.right(),
            "{label} belongs beside its heading"
        );
        assert!(
            (bounds.center().y - software.center().y).abs() < 4.,
            "{label}"
        );
        assert!(clip.contains_rect(bounds));
    }
    let card_bounds = |output: &FullOutput, heading: Rect, palette: Palette| {
        let mut cards = Vec::new();
        for clipped in &output.shapes {
            visit_shapes(&clipped.shape, &mut |shape| {
                if let egui::Shape::Rect(rect) = shape
                    && rect.fill == palette.card
                    && rect.rect.contains_rect(heading)
                {
                    cards.push(rect.rect);
                }
            });
        }
        cards
            .into_iter()
            .min_by(|a, b| a.area().total_cmp(&b.area()))
            .unwrap()
    };
    let engine_card = card_bounds(&output, engine, Palette::get(&ui.context));
    let software_card = card_bounds(&output, software, Palette::get(&ui.context));
    assert!(
        (engine_card.bottom() - software_card.bottom()).abs() < 1.,
        "Engine card {engine_card:?}, software card {software_card:?}"
    );
    // Content expansion must grow both cards this frame, then shrink again.
    ui.app.update_status = UpdateStatus::Failed;
    ui.app.update_message = "可重试的网络错误。".repeat(60);
    let expanded = ui.settle();
    let left = card_bounds(
        &expanded,
        painted_text_bounds(&expanded, "FFmpeg 更新").0,
        Palette::get(&ui.context),
    );
    let right = card_bounds(
        &expanded,
        painted_text_bounds(&expanded, "软件更新").0,
        Palette::get(&ui.context),
    );
    assert!(left.height() > engine_card.height());
    assert!((left.height() - right.height()).abs() < 1.);
    ui.app.update_status = UpdateStatus::Idle;
    ui.app.update_message.clear();
    let output = ui.settle();
    let restored = card_bounds(
        &output,
        painted_text_bounds(&output, "FFmpeg 更新").0,
        Palette::get(&ui.context),
    );
    assert!(
        (restored.height() - engine_card.height()).abs() < 1.,
        "Card size must not accumulate across frames"
    );
    let mut automatic = Vec::new();
    for clipped in &output.shapes {
        visit_shapes(&clipped.shape, &mut |shape| {
            if let egui::Shape::Text(text) = shape
                && text.galley.job.text == "自动检查更新"
            {
                automatic.push(Rect::from_min_size(text.pos, text.galley.size()));
            }
        });
    }
    automatic.sort_by(|a, b| a.left().total_cmp(&b.left()));
    assert_eq!(automatic.len(), 2);
    ui.click(automatic[1].center());
    assert!(!ui.app.prefs.auto_check_app_updates);
    assert!(ui.app.prefs.auto_check_updates);
    let persisted: Preferences =
        serde_json::from_str(&serde_json::to_string(&ui.app.prefs).unwrap()).unwrap();
    assert!(!persisted.auto_check_app_updates);
    assert!(persisted.auto_check_updates);
    let output = ui.scroll_content_to("使用文档");
    let (name, _) = painted_text_bounds(&output, "FFmpeg 更新");
    let (help, _) = painted_text_bounds(&output, "使用文档");
    assert!(help.left() > name.right());
    assert!((help.center().y - name.center().y).abs() < 4.);
    let (download, _) = painted_text_bounds(&output, "FFmpeg 下载页面");
    assert!(download.left() > name.right());
    assert!((download.center().y - name.center().y).abs() < 4.);
    let output = ui.settle();
    ui.click(pos2(36., 38.));
    assert!(ui.app.prefs.sidebar_collapsed);
    ui.click(pos2(36., 38.));
    assert!(!ui.app.prefs.sidebar_collapsed);
    drop(output);

    ui.viewport = vec2(960., 660.);
    let output = ui.scroll_content_to("软件更新");
    let (software, clip) = painted_text_bounds(&output, "软件更新");
    assert!(clip.contains_rect(software));
    let output = ui.scroll_content_to("FFmpeg 更新");
    let (engine, _) = painted_text_bounds(&output, "FFmpeg 更新");
    assert!(engine.left() < 350.);
}
