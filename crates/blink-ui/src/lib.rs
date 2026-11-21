//! Blink UI: egui views, theming and input mapping. Talks to files only through the runtime.

pub mod app;
pub mod bench;
pub mod tiles;

pub use app::{BlinkApp, Displayed, Options};
use std::path::PathBuf;

/// Run the GUI event loop.
pub fn run(folder: Option<PathBuf>) -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(blink_core::APP_NAME)
            .with_inner_size([1280.0, 800.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        blink_core::APP_NAME,
        options,
        Box::new(move |cc| {
            Ok(Box::new(BlinkApp::new(
                &cc.egui_ctx,
                folder,
                options_from_config(),
            )))
        }),
    )
}

/// Default options with the user's `config.toml` applied (RATE-2, D-3, PLAT-2 locations).
pub fn options_from_config() -> Options {
    let mut o = Options::default();
    if let Some(paths) = blink_runtime::paths::AppPaths::for_current_exe(o.fs.as_ref()) {
        let (c, msg) = blink_runtime::config::Config::load(o.fs.as_ref(), &paths.config_dir);
        o.settings = c.settings;
        o.sidecar_style = c.sidecar_style;
        o.config_message = msg;
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_window_renders_headless() {
        let ctx = egui::Context::default();
        let mut app = BlinkApp::new(&ctx, None, Options::default());
        let out = ctx.run_ui(egui::RawInput::default(), |ui| app.show(ui));
        assert!(!out.shapes.is_empty());
        out.drop_without_applying_deltas();
    }
}
