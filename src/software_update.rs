//! Application release UI and asynchronous state; the engine updater remains independent.
use crate::{
    app_updater,
    icons::{self, Icon},
    theme::Palette,
};
use eframe::egui::{self, RichText, Ui};
use std::{
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

#[derive(Default, Clone, Copy, Debug, PartialEq)]
enum State {
    #[default]
    Idle,
    Checking,
    Available,
    Current,
    Downloading,
    Ready,
    Installing,
    Failed,
}

enum Message {
    Checked(Result<app_updater::Release, String>),
    Progress(String),
    Downloaded(Result<app_updater::DownloadedInstaller, String>),
}

pub struct SoftwareUpdate {
    state: State,
    release: Option<app_updater::Release>,
    installer: Option<app_updater::DownloadedInstaller>,
    message: String,
    checked: bool,
    tx: Sender<Message>,
    rx: Receiver<Message>,
}

impl Default for SoftwareUpdate {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            state: State::Idle,
            release: None,
            installer: None,
            message: String::new(),
            checked: false,
            tx,
            rx,
        }
    }
}

impl SoftwareUpdate {
    pub fn busy(&self) -> bool {
        matches!(self.state, State::Downloading | State::Installing)
    }

    pub fn poll(&mut self, ctx: &egui::Context, automatic: bool, network: bool) {
        while let Ok(message) = self.rx.try_recv() {
            match message {
                Message::Checked(result) if self.state == State::Checking => match result {
                    Ok(release) => {
                        self.state =
                            if app_updater::is_newer(&release.version, env!("CARGO_PKG_VERSION")) {
                                State::Available
                            } else {
                                State::Current
                            };
                        self.release = Some(release);
                        self.message.clear();
                    }
                    Err(error) => {
                        self.state = State::Failed;
                        self.message = error;
                    }
                },
                Message::Progress(message) if self.state == State::Downloading => {
                    self.message = message
                }
                Message::Downloaded(result) if self.state == State::Downloading => match result {
                    Ok(installer) => {
                        self.installer = Some(installer);
                        self.state = State::Ready;
                        self.message.clear();
                    }
                    Err(error) => {
                        self.state = State::Failed;
                        self.message = error;
                    }
                },
                _ => {}
            }
        }
        if !self.checked {
            self.checked = true;
            if automatic && network && app_updater::supported() {
                self.check(ctx, network);
            }
        }
    }

    fn check(&mut self, ctx: &egui::Context, network: bool) {
        if !app_updater::supported() || self.busy() || self.state == State::Checking {
            return;
        }
        self.checked = true;
        self.state = State::Checking;
        self.release = None;
        self.installer = None;
        self.message.clear();
        if !network {
            return;
        }
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        thread::spawn(move || {
            let _ = tx.send(Message::Checked(app_updater::latest_release()));
            ctx.request_repaint();
        });
    }

    fn download(&mut self, ctx: &egui::Context, blocked: bool, network: bool) {
        if blocked
            || !app_updater::supported()
            || !matches!(self.state, State::Available | State::Failed)
        {
            return;
        }
        let Some(release) = self
            .release
            .clone()
            .filter(|r| app_updater::is_newer(&r.version, env!("CARGO_PKG_VERSION")))
        else {
            return;
        };
        self.state = State::Downloading;
        self.message = "正在准备下载…".into();
        self.installer = None;
        if !network {
            return;
        }
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        thread::spawn(move || {
            let result = app_updater::download_release(&release, |message| {
                let _ = tx.send(Message::Progress(message));
                ctx.request_repaint();
            });
            let _ = tx.send(Message::Downloaded(result));
            ctx.request_repaint();
        });
    }

    fn install(&mut self, ctx: &egui::Context, blocked: bool, execute: bool) {
        if blocked || self.state != State::Ready || self.installer.is_none() {
            return;
        }
        if !execute {
            return;
        }
        let result = std::env::current_exe()
            .map_err(|e| format!("无法读取程序路径：{e}"))
            .and_then(|path| {
                let directory = path.parent().ok_or("当前程序目录无效。")?;
                app_updater::launch_installer(self.installer.as_ref().unwrap(), directory)
            });
        match result {
            Ok(()) => {
                self.state = State::Installing;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(error) => {
                // Keep the verified installer available for an explicit retry.
                self.message = error;
            }
        }
    }

    pub fn ui(
        &mut self,
        ui: &mut Ui,
        ctx: &egui::Context,
        automatic: &mut bool,
        blocked: bool,
        network: bool,
        p: Palette,
    ) {
        let automatic_response = ui.checkbox(automatic, "自动检查更新");
        if automatic_response.changed() && *automatic && network {
            self.check(ctx, network);
        }
        ui.label(
            RichText::new(concat!("当前版本 ", env!("CARGO_PKG_VERSION")))
                .size(12.)
                .color(p.muted),
        );
        let status = match self.state {
            State::Idle => "从 GitHub 获取软件更新。".into(),
            State::Checking => "正在检查更新…".into(),
            State::Available => format!(
                "发现新版本 {}",
                self.release
                    .as_ref()
                    .map(|r| r.version.as_str())
                    .unwrap_or("")
            ),
            State::Current => "当前已是最新版本。".into(),
            State::Downloading => self.message.clone(),
            State::Ready => "下载已校验，安装将退出软件。".into(),
            State::Installing => "正在打开安装向导…".into(),
            State::Failed => format!("更新失败：{}", self.message),
        };
        ui.add(egui::Label::new(status).wrap());
        if matches!(self.state, State::Checking | State::Downloading) {
            ui.spinner();
        }
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(
                app_updater::supported() && !self.busy() && self.state != State::Checking,
                |ui| {
                    if icons::button(
                        ui,
                        Icon::Refresh,
                        "检查更新",
                        "检查软件更新",
                        128.,
                        false,
                        p,
                    )
                    .clicked()
                    {
                        self.check(ctx, network);
                    }
                },
            );
            if matches!(self.state, State::Available | State::Failed) && self.release.is_some() {
                ui.add_enabled_ui(!blocked, |ui| {
                    if ui
                        .button(if self.state == State::Failed {
                            "重试下载"
                        } else {
                            "下载更新"
                        })
                        .clicked()
                    {
                        self.download(ctx, blocked, network);
                    }
                });
            }
            if self.state == State::Ready {
                ui.add_enabled_ui(!blocked, |ui| {
                    if ui.button("安装并退出").clicked() {
                        self.install(ctx, blocked, network);
                    }
                });
            }
        });
        if self.state == State::Ready {
            ui.small("安装到当前目录，任务列表不保留。");
            if !self.message.is_empty() {
                ui.colored_label(p.danger, &self.message);
            }
        }
        if blocked && matches!(self.state, State::Available | State::Ready) {
            ui.small("当前操作完成后可更新软件。");
        }
        if !app_updater::supported() {
            ui.small("自动更新目前支持 Windows x64。");
        }
        ui.horizontal_wrapped(|ui| {
            ui.hyperlink_to("版本发布页", app_updater::RELEASES_URL);
            ui.hyperlink_to("GitHub 项目", app_updater::REPOSITORY_URL);
        });
    }
}

#[cfg(all(test, target_os = "windows", target_arch = "x86_64"))]
mod tests {
    use super::*;

    #[test]
    fn completed_download_waits_for_explicit_install_and_does_not_close_the_app() {
        let ctx = egui::Context::default();
        let mut update = SoftwareUpdate::default();
        update.check(&ctx, false);
        update
            .tx
            .send(Message::Checked(Ok(app_updater::test_release("0.2.2"))))
            .unwrap();
        update.poll(&ctx, false, false);
        update.download(&ctx, false, false);
        update
            .tx
            .send(Message::Downloaded(Ok(
                app_updater::test_downloaded_installer("fixture.exe".into()),
            )))
            .unwrap();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            update.poll(ctx, false, false);
            update.install(ctx, true, true);
        });
        assert_eq!(update.state, State::Ready);
        assert!(update.installer.is_some());
        assert!(!update.busy());
        assert!(
            output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .iter()
                .all(|command| !matches!(command, egui::ViewportCommand::Close))
        );
    }

    #[test]
    fn release_checks_reject_downgrades_and_only_offer_newer_versions() {
        let ctx = egui::Context::default();
        let mut update = SoftwareUpdate::default();
        for (version, expected) in [
            ("0.2.0", State::Current),
            ("0.2.1", State::Current),
            ("0.2.2", State::Available),
        ] {
            update.check(&ctx, false);
            update
                .tx
                .send(Message::Checked(Ok(app_updater::test_release(version))))
                .unwrap();
            update.poll(&ctx, false, false);
            assert_eq!(update.state, expected);
        }
        update.download(&ctx, true, false);
        assert_eq!(update.state, State::Available);
        update.download(&ctx, false, false);
        assert!(update.busy());
        update.check(&ctx, false);
        assert_eq!(update.state, State::Downloading);
        update
            .tx
            .send(Message::Downloaded(Err("校验失败".into())))
            .unwrap();
        update.poll(&ctx, false, false);
        assert!(!update.busy());
        assert_eq!(update.state, State::Failed);
        assert!(update.release.is_some());
        update.download(&ctx, false, false);
        assert_eq!(update.state, State::Downloading);
    }

    #[test]
    fn no_background_request_when_disabled_and_stale_messages_cannot_replace_state() {
        let ctx = egui::Context::default();
        let mut update = SoftwareUpdate::default();
        update.poll(&ctx, false, true);
        assert_eq!(update.state, State::Idle);
        update
            .tx
            .send(Message::Checked(Ok(app_updater::test_release("9.0.0"))))
            .unwrap();
        update
            .tx
            .send(Message::Progress("unexpected".into()))
            .unwrap();
        update
            .tx
            .send(Message::Downloaded(Err("unexpected".into())))
            .unwrap();
        update.poll(&ctx, false, false);
        assert_eq!(update.state, State::Idle);
        assert!(update.release.is_none());
        assert!(update.message.is_empty());
        update.check(&ctx, false);
        update
            .tx
            .send(Message::Checked(Err("无法连接".into())))
            .unwrap();
        update.poll(&ctx, false, false);
        assert_eq!(update.state, State::Failed);
        update.check(&ctx, false);
        assert_eq!(update.state, State::Checking);
        assert!(update.message.is_empty());
    }
}
