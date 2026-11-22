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
fn req_open_1_a_relative_file_argument_opens_the_current_folder() {
    // `blink IMG.ARW`: the parent of a bare file name is "", which must mean ".".
    let mut h = harness(folder(1), false);
    h.state_mut().open_path(Path::new("IMG_0001.ARW"));
    h.step();
    assert_eq!(h.state().state().folder.as_deref(), Some(Path::new(".")));
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

#[test]
fn req_rate_4_ratings_and_labels_from_sidecars_show_in_the_overlay() {
    // Regression: the default fonts have no star glyph; a rated folder must not crash the UI.
    let fs = folder(3);
    fs.add_file(
        "/f/IMG_0000.xmp",
        blink_testkit::sidecars::lightroom(3, Some("Red")),
    );
    fs.add_file("/f/IMG_0001.xmp", blink_testkit::sidecars::darktable(-1));
    let mut h = harness(fs, true);
    let t = Instant::now();
    while h.state().state().marks.is_empty() {
        assert!(t.elapsed() < Duration::from_secs(10));
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    settle(&mut h);
    assert!(h.query_by_label("3 stars").is_some());
    assert!(h.query_by_label("Red").is_some());
    h.state_mut().dispatch(blink_core::state::Action::Navigate(
        blink_core::state::Nav::Next,
    ));
    settle(&mut h);
    assert!(h.query_by_label("REJECTED").is_some());
    assert!(h.query_by_label("3 stars").is_none());
}

#[test]
fn req_rate_1_req_rate_3_mark_keys_write_sidecars_and_ctrl_z_takes_them_back() {
    use egui::{Key, Modifiers};
    let fs = Arc::new(folder(3));
    fs.add_file(
        "/f/IMG_0001.xmp",
        blink_testkit::sidecars::lightroom(2, None),
    );
    let before = fs.peek("/f/IMG_0001.xmp");
    let mut h = Harness::builder().with_size([800.0, 500.0]).build_ui_state(
        |ui, app: &mut BlinkApp| app.show(ui),
        BlinkApp::new(
            &egui::Context::default(),
            Some("/f".into()),
            Options {
                fs: fs.clone(),
                workers: 2,
                ..Options::default()
            },
        ),
    );
    let t = Instant::now();
    while h.state().state().marks.is_empty() {
        assert!(t.elapsed() < Duration::from_secs(10));
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    settle(&mut h);
    let owned = |p: &str| fs.peek(p).map(|b| blink_xmp::read_owned(&b).unwrap());
    h.key_press(Key::Num3);
    h.key_press(Key::Num7);
    h.step();
    assert!(h.query_by_label("3 stars").is_some());
    assert!(h.query_by_label("Yellow").is_some());
    h.state().flush_sidecars();
    let o = owned("/f/IMG_0000.xmp").expect("created");
    assert_eq!((o.rating, o.label.as_deref()), (Some(3), Some("Yellow")));
    // Shift+9 is purple; P picks (1 star by default) and the overlay says so.
    h.key_press_modifiers(Modifiers::SHIFT, Key::Num9);
    h.key_press(Key::P);
    h.step();
    assert!(h.query_by_label("PICK").is_some());
    assert!(h.query_by_label("Purple").is_some());
    // Shift+A: auto-advance; X rejects the item and moves on.
    h.key_press_modifiers(Modifiers::SHIFT, Key::A);
    h.key_press(Key::X);
    h.step();
    assert_eq!(h.state().state().current, Some(1));
    h.key_press(Key::Num5);
    h.step();
    assert_eq!(h.state().state().current, Some(2));
    h.state().flush_sidecars();
    assert_eq!(owned("/f/IMG_0000.xmp").unwrap().rating, Some(-1));
    assert_eq!(owned("/f/IMG_0001.xmp").unwrap().rating, Some(5));
    // Undo everything: both sidecars are back byte for byte (the new one is gone).
    for _ in 0..6 {
        h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
        h.step();
    }
    assert_eq!(h.state().state().current, Some(0));
    h.state().flush_sidecars();
    assert_eq!(fs.peek("/f/IMG_0000.xmp"), None);
    assert_eq!(fs.peek("/f/IMG_0001.xmp"), before);
    // Redo brings the first change back.
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    h.step();
    h.state().flush_sidecars();
    assert_eq!(owned("/f/IMG_0000.xmp").unwrap().rating, Some(3));
}

#[test]
fn req_file_8_external_change_reloads_and_a_conflict_keeps_disk_and_shows_a_marker() {
    let fs = Arc::new(folder(2));
    fs.add_file(
        "/f/IMG_0001.xmp",
        blink_testkit::sidecars::lightroom(1, None),
    );
    let mut h = Harness::builder().with_size([800.0, 500.0]).build_ui_state(
        |ui, app: &mut BlinkApp| app.show(ui),
        BlinkApp::new(
            &egui::Context::default(),
            Some("/f".into()),
            Options {
                fs: fs.clone(),
                workers: 2,
                ..Options::default()
            },
        ),
    );
    settle(&mut h);
    let wait = |h: &mut Harness<'static, BlinkApp>, f: &dyn Fn(&BlinkApp) -> bool| {
        let t = Instant::now();
        while !f(h.state()) {
            assert!(t.elapsed() < Duration::from_secs(10));
            h.step();
            std::thread::sleep(Duration::from_millis(5));
        }
        h.step();
    };
    // No unsaved changes: an outside edit is simply reloaded.
    fs.add_file(
        "/f/IMG_0000.xmp",
        blink_testkit::sidecars::lightroom(4, Some("Green")),
    );
    h.state().sidecars_changed(vec!["/f/IMG_0000.xmp".into()]);
    wait(&mut h, &|a| {
        a.state()
            .marks
            .get(Path::new("/f/IMG_0000.ARW"))
            .and_then(|m| m.rating)
            == Some(4)
    });
    assert!(h.query_by_label("4 stars").is_some());
    assert!(h.query_by_label("CONFLICT").is_none());
    // Unsaved change (still in the debounce) + an outside edit: the disk version wins.
    h.key_press(egui::Key::ArrowRight);
    settle(&mut h);
    h.key_press(egui::Key::Num5);
    h.step();
    fs.add_file(
        "/f/IMG_0001.xmp",
        blink_testkit::sidecars::lightroom(2, Some("Red")),
    );
    h.state().sidecars_changed(vec!["/f/IMG_0001.xmp".into()]);
    wait(&mut h, &|a| {
        a.state().conflicts.contains(Path::new("/f/IMG_0001.ARW"))
    });
    let m = h.state().state().marks[Path::new("/f/IMG_0001.ARW")].clone();
    assert_eq!((m.rating, m.label.as_deref()), (Some(2), Some("Red")));
    assert!(h.query_by_label("CONFLICT").is_some());
    assert!(
        h.state()
            .state()
            .log
            .iter()
            .any(|l| l.contains("kept the on-disk version"))
    );
    h.state().flush_sidecars();
    assert_eq!(
        fs.peek("/f/IMG_0001.xmp").unwrap(),
        blink_testkit::sidecars::lightroom(2, Some("Red")).as_bytes()
    );
    // Rating again clears the marker and writes over the reloaded file.
    h.key_press(egui::Key::Num3);
    h.step();
    assert!(
        !h.state()
            .state()
            .conflicts
            .contains(Path::new("/f/IMG_0001.ARW"))
    );
}

#[test]
fn req_filt_1_req_filt_2_alt_digit_adds_a_removable_chip_and_the_sort_selector_reorders() {
    use egui::{Key, Modifiers};
    use egui_kittest::kittest::NodeT;
    let fs = folder(4);
    fs.add_file(
        "/f/IMG_0001.xmp",
        blink_testkit::sidecars::lightroom(3, None),
    );
    fs.add_file(
        "/f/IMG_0003.xmp",
        blink_testkit::sidecars::lightroom(5, None),
    );
    let mut h = harness(fs, true);
    step_until(&mut h, "marks loaded", |a| a.state().marks.len() == 2);
    settle(&mut h);
    h.get_by_label("1 / 4");
    // Alt+3: rating ≥ 3. The chip shows, the counter counts the visible items.
    h.key_press_modifiers(Modifiers::ALT, Key::Num3);
    h.step();
    settle(&mut h);
    h.get_by_label("Rating ≥ 3 ×");
    h.get_by_label("1 / 2");
    h.get_by_label("of 4");
    assert_eq!(
        h.state().displayed().unwrap().path,
        Path::new("/f/IMG_0001.ARW")
    );
    h.key_press(Key::ArrowRight);
    h.step();
    settle(&mut h);
    assert_eq!(
        h.state().displayed().unwrap().path,
        Path::new("/f/IMG_0003.ARW")
    );
    // Sort by rating: the 5-star item comes first.
    let sort = |h: &Harness<'static, BlinkApp>| {
        h.get_by_role_and_label(egui::accesskit::Role::ComboBox, "Sort")
            .accesskit_node()
            .value()
    };
    assert_eq!(sort(&h).as_deref(), Some("Capture time"));
    h.get_by_role_and_label(egui::accesskit::Role::ComboBox, "Sort")
        .click();
    h.run();
    h.get_by_label("Rating").click();
    h.run();
    assert_eq!(sort(&h).as_deref(), Some("Rating"));
    assert_eq!(h.state().state().sort, blink_core::sort::SortKey::Rating);
    assert_eq!(h.state().state().pos(), Some(0));
    assert_eq!(
        h.state().state().current_item().unwrap().primary.path,
        Path::new("/f/IMG_0003.ARW")
    );
    // The chip's × removes the filter.
    h.get_by_label("Rating ≥ 3 ×").click();
    h.run();
    assert!(h.query_by_label("Rating ≥ 3 ×").is_none());
    h.get_by_label("1 / 4");
    assert!(h.query_by_label("of 4").is_none());
    h.get_by_label("+ Filter");
}

#[test]
fn req_file_1_ctrl_shift_delete_confirms_moves_rejects_to_rejected_and_ctrl_z_brings_them_back() {
    use blink_runtime::fs::FsCall;
    use egui::{Key, Modifiers};
    let fs = Arc::new(folder(3));
    fs.add_file("/f/IMG_0001.JPG", b"jpeg".to_vec());
    let before: Vec<_> = fs.file_paths();
    let mut h = Harness::builder().with_size([800.0, 500.0]).build_ui_state(
        |ui, app: &mut BlinkApp| app.show(ui),
        BlinkApp::new(
            &egui::Context::default(),
            Some("/f".into()),
            Options {
                fs: fs.clone(),
                workers: 2,
                ..Options::default()
            },
        ),
    );
    settle(&mut h);
    h.key_press(Key::ArrowRight);
    h.step();
    h.key_press(Key::X);
    h.step();
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Delete);
    h.run();
    h.get_by_label("Move 1 rejected item to _rejected/?");
    // Escape cancels; nothing moved.
    h.key_press(Key::Escape);
    h.run();
    assert!(
        h.query_by_label("Move 1 rejected item to _rejected/?")
            .is_none()
    );
    assert_eq!(h.state().state().items.len(), 3);
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Delete);
    h.run();
    h.get_by_label("Move").click();
    step_until(&mut h, "rejects moved", |a| a.state().items.len() == 2);
    h.state().flush_sidecars();
    assert!(fs.peek("/f/_rejected/IMG_0001.ARW").is_some());
    assert!(fs.peek("/f/_rejected/IMG_0001.JPG").is_some());
    assert!(
        fs.peek("/f/_rejected/IMG_0001.xmp").is_some(),
        "the new sidecar moved too"
    );
    assert!(fs.peek("/f/IMG_0001.ARW").is_none());
    // The item after the moved one is current.
    h.run();
    h.get_by_label("2 / 2");
    assert_eq!(
        h.state().state().current_item().unwrap().primary.path,
        Path::new("/f/IMG_0002.ARW")
    );
    // Undo: the files come back and the item is listed again.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    step_until(&mut h, "rejects back", |a| a.state().items.len() == 3);
    assert!(fs.peek("/f/IMG_0001.ARW").is_some());
    assert!(fs.peek("/f/IMG_0001.JPG").is_some());
    let mut now = fs.file_paths();
    now.retain(|p| !p.extension().is_some_and(|e| e == "xmp"));
    assert_eq!(now, before);
    assert!(!fs.calls().iter().any(|c| matches!(c, FsCall::RemoveFile(p) if !p.to_string_lossy().ends_with(".xmp") && !p.to_string_lossy().ends_with(".tmp"))));
}
