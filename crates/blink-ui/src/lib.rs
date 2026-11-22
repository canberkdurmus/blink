//! Blink UI: egui views, theming and input mapping. Talks to files only through the runtime.

pub mod app;
pub mod bench;
pub mod tiles;

pub use app::{BlinkApp, Displayed, Options};
use std::path::PathBuf;

/// Options from the user's config file (RATE-2, D-3); problems with it become a toast.
pub fn configured_options() -> Options {
    let fs = blink_runtime::fs::RealFs;
    let Some(paths) = blink_runtime::paths::AppPaths::for_current_exe(&fs) else {
        return Options::default();
    };
    let (c, errs) = blink_runtime::config::Config::load(&fs, &paths.config_dir);
    Options {
        sidecar_style: c.sidecar_style,
        external_apps: c.external_apps(),
        marks: c.marks,
        notice: (!errs.is_empty()).then(|| errs.join("; ")),
        ..Options::default()
    }
}

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
                configured_options(),
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
