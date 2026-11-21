//! UI tests (SPEC §11.1): scripted input against the real app, with a memory file system and
//! synthetic RAW stand-ins. Pixel goldens are Linux-only and in the full tier.

use blink_core::preview::PreviewSource;
use blink_core::view::Zoom;
use blink_runtime::fs::MemFs;
use blink_testkit::fixtures::{jpeg, quadrants, tiff_with_jpegs};
use blink_ui::{BlinkApp, Options};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A RAW stand-in (TIFF container, `.ARW` name) whose only preview is a 600×400 JPEG with four
/// coloured quadrants; `shade` varies the colours per file.
fn raw(shade: u8) -> Vec<u8> {
    let q = [
        [200, 40, shade],
        [40, 200, shade],
        [shade, 40, 200],
        [250, 250, 250],
    ];
    tiff_with_jpegs(&[jpeg(&quadrants(600, 400, q), 600, 400, 95, None)])
}

fn folder(n: usize) -> MemFs {
    let fs = MemFs::new();
    fs.add_dir("/f");
    for i in 0..n {
        fs.add_file(format!("/f/IMG_{i:04}.ARW"), raw((i * 40 % 250) as u8));
    }
    fs
}

fn opts(fs: MemFs) -> Options {
    Options {
        fs: Arc::new(fs),
        workers: 2,
        ..Options::default()
    }
}

fn harness(fs: MemFs, open: bool) -> Harness<'static, BlinkApp> {
    Harness::builder().with_size([800.0, 500.0]).build_ui_state(
        |ui, app: &mut BlinkApp| app.show(ui),
        BlinkApp::new(
            &egui::Context::default(),
            open.then(|| "/f".into()),
            opts(fs),
        ),
    )
}

/// Step frames until the current item is drawn and the loader is idle.
fn settle(h: &mut Harness<'static, BlinkApp>) {
    let t = Instant::now();
    loop {
        h.step();
        let s = h.state();
        let cur = s.state().current_item().map(|i| i.primary.path.clone());
        if s.displayed().map(|d| Some(&d.path) == cur.as_ref()) == Some(true) && s.loader_idle() {
            h.step();
            return;
        }
        assert!(t.elapsed() < Duration::from_secs(30), "never settled");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn req_view_1_command_line_folder_opens_in_the_loupe_fit_with_the_embedded_preview() {
    let mut h = harness(folder(3), true);
    settle(&mut h);
    let app = h.state();
    let d = app.displayed().unwrap();
    assert_eq!(d.path, Path::new("/f/IMG_0000.ARW"));
    assert_eq!(d.source, PreviewSource::Embedded);
    assert_eq!(d.size, (600, 400));
    let l = &app.state().loupe;
    assert_eq!(l.zoom, Zoom::Fit);
    let r = l.image_rect().unwrap();
    assert!(r.w <= l.viewport[0] + 0.5 && r.h <= l.viewport[1] + 0.5);
    assert!(
        (r.w - l.viewport[0]).abs() < 0.5 || (r.h - l.viewport[1]).abs() < 0.5,
        "fit touches two edges: {r:?} in {:?}",
        l.viewport
    );
    h.get_by_label("EMBEDDED");
    h.get_by_label("1 / 3");
}

#[test]
fn req_view_1_arrow_keys_move_and_no_frame_shows_a_stale_item() {
    let mut h = harness(folder(5), true);
    settle(&mut h);
    for _ in 0..4 {
        h.key_press(egui::Key::ArrowRight);
        // Every frame: whatever is drawn belongs to the current item.
        let t = Instant::now();
        loop {
            h.step();
            let app = h.state();
            let cur = app.state().current_item().unwrap().primary.path.clone();
            match app.displayed() {
                Some(d) if d.path == cur => break,
                Some(d) => panic!("stale frame: {:?} drawn for {cur:?}", d.path),
                None => {}
            }
            assert!(t.elapsed() < Duration::from_secs(30));
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    assert_eq!(h.state().state().current, Some(4));
    h.key_press(egui::Key::Home);
    h.step();
    assert_eq!(h.state().state().current, Some(0));
    h.key_press(egui::Key::End);
    h.step();
    assert_eq!(h.state().state().current, Some(4));
}

#[derive(Debug)]
struct Dropped(std::path::PathBuf);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &Path {
        &self.0
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        Err("not read".into())
    }
}

#[test]
fn req_open_1_dropping_a_folder_or_file_opens_its_folder() {
    let mut h = harness(folder(2), false);
    h.step();
    h.get_by_label("No folder open");
    h.state_mut().open_path(Path::new("/f/IMG_0001.ARW")); // a dropped file → its folder
    settle(&mut h);
    assert_eq!(h.state().state().folder.as_deref(), Some(Path::new("/f")));
    // Real drop events go through the same path.
    h.input_mut()
        .dropped_files
        .push(Arc::new(Dropped("/f".into())));
    h.step();
    settle(&mut h);
    assert_eq!(h.state().state().items.len(), 2);
}

#[test]
fn req_view_6_j_toggles_the_clipping_overlay_and_i_shows_the_labelled_histogram() {
    let mut h = harness(folder(1), true);
    settle(&mut h);
    h.key_press(egui::Key::J);
    h.step();
    assert!(h.state().state().loupe.clipping);
    h.key_press(egui::Key::I);
    // The histogram is computed off the UI thread; wait for it (bounded).
    let start = std::time::Instant::now();
    while h.query_by_label("Histogram · EMBEDDED").is_none() {
        assert!(
            start.elapsed() < std::time::Duration::from_secs(30),
            "histogram never shown"
        );
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.get_by_label("Histogram · EMBEDDED");
    // The quadrant at 250,250,250 is not clipped; the counts come from the displayed image.
    let hist = h.state().loaded().unwrap().histogram().clone();
    assert_eq!(hist.source, PreviewSource::Embedded);
    assert_eq!(hist.red.iter().sum::<u32>(), 600 * 400);
}

/// Step frames until `done` holds (bounded).
fn step_until(h: &mut Harness<'static, BlinkApp>, what: &str, done: impl Fn(&BlinkApp) -> bool) {
    let t = Instant::now();
    while !done(h.state()) {
        assert!(t.elapsed() < Duration::from_secs(30), "{what}");
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn req_view_4_z_zooms_to_100_percent_with_every_pixel_and_req_view_9_keeps_it_on_the_next_item() {
    // Previews larger than the screen are drawn from a screen-resolution copy at Fit, and from
    // their real pixels once zoomed to 100 %.
    let big = |s: u8| {
        let q = [[200, 40, s], [40, 200, s], [s, 40, 200], [250, 250, 250]];
        tiff_with_jpegs(&[jpeg(&quadrants(3000, 2000, q), 3000, 2000, 90, None)])
    };
    let fs = MemFs::new();
    fs.add_dir("/f");
    fs.add_file("/f/A.ARW", big(10));
    fs.add_file("/f/B.ARW", big(90));
    fs.add_file("/f/C.ARW", big(170));
    let mut h = harness(fs, true);
    settle(&mut h);
    let d = h.state().displayed().unwrap().clone();
    assert_eq!(d.size, (3000, 2000));
    assert!(d.pixels.0 <= blink_ui::tiles::SCREEN_LONG, "fit: {d:?}");

    h.key_press(egui::Key::Z);
    step_until(&mut h, "100 % never drew every pixel", |a| {
        a.displayed().is_some_and(|d| d.pixels == (3000, 2000))
    });
    assert_eq!(h.state().state().loupe.zoom, Zoom::P100);

    // Keep zoom (default on): the next item opens at 100 % too, at full resolution.
    h.key_press(egui::Key::ArrowRight);
    step_until(&mut h, "next item never drawn at 100 %", |a| {
        a.displayed()
            .is_some_and(|d| d.path == Path::new("/f/B.ARW") && d.pixels == (3000, 2000))
    });
    assert_eq!(h.state().state().loupe.zoom, Zoom::P100);

    // Toggled off: the next item opens fit again.
    h.state_mut()
        .dispatch(blink_core::state::Action::SetKeepZoom(false));
    h.key_press(egui::Key::ArrowRight);
    step_until(&mut h, "next item never drawn fit", |a| {
        a.displayed()
            .is_some_and(|d| d.path == Path::new("/f/C.ARW"))
    });
    assert_eq!(h.state().state().loupe.zoom, Zoom::Fit);
    assert!(h.state().displayed().unwrap().pixels.0 <= blink_ui::tiles::SCREEN_LONG);
}

#[cfg(target_os = "linux")]
mod full_ui {
    use super::*;

    /// VIEW-1 acceptance: Linux screenshot golden of the loupe (fit, embedded preview, badge).
    #[test]
    fn req_view_1_loupe_screenshot_golden() {
        let mut h = Harness::builder()
            .with_size([800.0, 500.0])
            .wgpu()
            .build_ui_state(
                |ui, app: &mut BlinkApp| app.show(ui),
                BlinkApp::new(
                    &egui::Context::default(),
                    Some("/f".into()),
                    opts(folder(3)),
                ),
            );
        settle(&mut h);
        assert_eq!(
            h.state().displayed().unwrap().source,
            PreviewSource::Embedded
        );
        h.snapshot("view_1_loupe_fit");
    }
}
