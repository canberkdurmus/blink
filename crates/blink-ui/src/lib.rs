//! Blink UI: egui views, theming and input mapping. Talks to files only through the runtime.

use std::path::PathBuf;

/// The top-level egui application.
#[derive(Debug, Default)]
pub struct BlinkApp {
    folder: Option<PathBuf>,
}

impl BlinkApp {
    /// Create the app, optionally with a folder passed on the command line.
    pub fn new(folder: Option<PathBuf>) -> Self {
        Self { folder }
    }

    /// Draw one frame into the root UI.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        match &self.folder {
            Some(f) => ui.label(format!("{}", f.display())),
            None => ui.label("No folder open"),
        };
    }
}

impl eframe::App for BlinkApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }
}

/// Run the GUI event loop.
pub fn run(folder: Option<PathBuf>) -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(blink_core::APP_NAME)
            .with_inner_size([1280.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        blink_core::APP_NAME,
        options,
        Box::new(move |_cc| Ok(Box::new(BlinkApp::new(folder)))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_window_renders_headless() {
        let ctx = egui::Context::default();
        let mut app = BlinkApp::new(None);
        let out = ctx.run_ui(egui::RawInput::default(), |ui| app.show(ui));
        assert!(!out.shapes.is_empty());
        out.drop_without_applying_deltas();
    }
}
