#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod app_updater;
mod effects;
mod encoding;
mod icons;
mod media;
mod software_update;
mod theme;
mod timecode;
mod tracks;
mod updater;

fn main() -> eframe::Result {
    let capture_mode = std::env::var_os("FRAMEFLOW_SCREENSHOT").is_some();
    // Snapshot runs must not load saved window geometry or alter user preferences.
    let capture_storage = capture_mode.then(|| {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "frameflow-capture-{}-{nonce}.ron",
            std::process::id()
        ))
    });
    let capture_size = capture_mode
        .then(|| std::env::var("FRAMEFLOW_SCREENSHOT_SIZE").ok())
        .flatten()
        .and_then(|value| {
            let (width, height) = value.split_once('x')?;
            let (width, height) = (width.parse::<u32>().ok()?, height.parse::<u32>().ok()?);
            ((960..=3840).contains(&width) && (660..=2160).contains(&height))
                .then_some([width as f32, height as f32])
        });
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("帧流 Frameflow")
            .with_app_id("app.frameflow.desktop")
            .with_inner_size(capture_size.unwrap_or([1240.0, 820.0]))
            .with_min_inner_size([960.0, 660.0])
            .with_decorations(true)
            .with_resizable(true)
            .with_icon(icons::app_icon()),
        centered: true,
        persist_window: !capture_mode,
        persistence_path: capture_storage.clone(),
        ..Default::default()
    };
    let result = eframe::run_native(
        "Frameflow",
        options,
        Box::new(|cc| Ok(Box::new(app::Frameflow::new(cc)))),
    );
    if let Some(path) = capture_storage {
        let _ = std::fs::remove_file(path);
    }
    result
}
