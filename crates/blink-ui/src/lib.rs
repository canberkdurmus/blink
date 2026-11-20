//! Blink UI: egui views, theming and input mapping. Talks to files only through the runtime.

pub mod app;
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
                Options::default(),
            )))
        }),
    )
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
