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
    // Telling a file from a folder reads the file system off the UI thread (PERF-9).
    step_until(&mut h, "the folder opens", |a| a.state().folder.is_some());
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

#[test]
fn req_file_2_copy_move_and_export_of_the_filtered_items_through_the_app() {
    use blink_core::filter::Cond;
    use blink_core::state::Action;
    let fs = Arc::new(folder(3));
    fs.add_dir("/out");
    fs.add_file(
        "/f/IMG_0001.xmp",
        blink_testkit::sidecars::lightroom(4, None),
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
    step_until(&mut h, "marks loaded", |a| !a.state().marks.is_empty());
    settle(&mut h);
    h.state_mut()
        .dispatch(Action::AddFilter(Cond::RatingAtLeast(4)));
    h.state_mut().dispatch(Action::ExportList {
        path: "/out/list.txt".into(),
        csv: false,
    });
    step_until(&mut h, "exported", |_| fs.peek("/out/list.txt").is_some());
    assert_eq!(fs.peek("/out/list.txt").unwrap(), b"/f/IMG_0001.ARW\n");
    h.state_mut().dispatch(Action::Transfer {
        dest: "/out".into(),
        copy: true,
    });
    step_until(&mut h, "copied", |a| a.state().copies_running == 0);
    assert_eq!(fs.peek("/out/IMG_0001.ARW"), fs.peek("/f/IMG_0001.ARW"));
    assert_eq!(fs.peek("/out/IMG_0001.xmp"), fs.peek("/f/IMG_0001.xmp"));
    assert!(fs.peek("/out/IMG_0000.ARW").is_none(), "filtered out");
    h.state_mut().dispatch(Action::Transfer {
        dest: "/out".into(),
        copy: false,
    });
    step_until(&mut h, "moved", |a| a.state().items.len() == 2);
    assert!(fs.peek("/f/IMG_0001.ARW").is_none());
    assert!(fs.peek("/out/IMG_0001-1.ARW").is_some());
}

/// Records launches instead of starting processes; fails for programs named `missing`.
#[derive(Default)]
struct RecordingLauncher(std::sync::Mutex<Vec<blink_runtime::external::Launch>>);

impl blink_runtime::external::Launcher for RecordingLauncher {
    fn launch(&self, cmd: &blink_runtime::external::Launch) -> std::io::Result<()> {
        self.0.lock().unwrap().push(cmd.clone());
        if cmd.program == Path::new("missing") {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no such program",
            ));
        }
        Ok(())
    }
}

#[test]
fn req_file_3_ctrl_shift_e_opens_the_current_item_in_the_first_external_app() {
    use blink_core::external::ExternalApp;
    use egui::{Key, Modifiers};
    let launcher = Arc::new(RecordingLauncher::default());
    let app = |name: &str, prog: &str, args: &[&str]| ExternalApp {
        name: name.into(),
        programs: vec![prog.into()],
        args: args.iter().map(|s| s.to_string()).collect(),
    };
    let fs = Arc::new(folder(3));
    let mut h = Harness::builder().with_size([800.0, 500.0]).build_ui_state(
        |ui, app: &mut BlinkApp| app.show(ui),
        BlinkApp::new(
            &egui::Context::default(),
            Some("/f".into()),
            Options {
                fs: fs.clone(),
                workers: 2,
                external_apps: vec![
                    app("Editor", "editor", &["--open", "{file}"]),
                    app("Broken", "missing", &[]),
                ],
                launcher: launcher.clone(),
                ..Options::default()
            },
        ),
    );
    settle(&mut h);
    h.key_press(Key::ArrowRight);
    settle(&mut h);
    let current = h
        .state()
        .state()
        .current_item()
        .unwrap()
        .primary
        .path
        .clone();
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::E);
    step_until(&mut h, "launched", |a| {
        a.state().toast.as_deref() == Some("Opened in Editor")
    });
    assert_eq!(
        *launcher.0.lock().unwrap(),
        [blink_runtime::external::Launch {
            program: "editor".into(),
            args: vec!["--open".into(), current.clone().into_os_string()],
        }]
    );
    h.state_mut().open_external(1);
    step_until(&mut h, "failed", |a| {
        a.state()
            .toast
            .as_deref()
            .is_some_and(|t| t.starts_with("Could not open in Broken"))
    });
    assert_eq!(launcher.0.lock().unwrap().len(), 2);
}

fn item_names(app: &BlinkApp) -> Vec<String> {
    app.state()
        .items
        .iter()
        .map(|i| {
            i.primary
                .path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

/// Step until `want` holds; OPEN-3 allows 1 s from the change.
fn within_1s(h: &mut Harness<'static, BlinkApp>, what: &str, want: impl Fn(&BlinkApp) -> bool) {
    let t = Instant::now();
    while !want(h.state()) {
        assert!(
            t.elapsed() < Duration::from_secs(1),
            "{what} not shown within 1 s: {:?}",
            item_names(h.state())
        );
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn req_open_3_added_removed_and_renamed_files_show_within_1_s() {
    let fs = Arc::new(folder(3));
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
    h.key_press(egui::Key::ArrowRight);
    settle(&mut h);
    fs.add_file("/f/IMG_0009.ARW", raw(90));
    h.state().folder_changed();
    within_1s(&mut h, "added file", |a| a.state().items.len() == 4);
    let cur = |a: &BlinkApp| {
        a.state().current_item().map(|i| {
            i.primary
                .path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
    };
    assert_eq!(cur(h.state()).as_deref(), Some("IMG_0001.ARW"));
    // Removing the current item moves on to the next one.
    fs.remove_externally("/f/IMG_0001.ARW");
    h.state().folder_changed();
    within_1s(&mut h, "removed file", |a| a.state().items.len() == 3);
    assert_eq!(cur(h.state()).as_deref(), Some("IMG_0002.ARW"));
    let bytes = fs.remove_externally("/f/IMG_0002.ARW").unwrap();
    fs.add_file("/f/renamed.ARW", bytes);
    h.state().folder_changed();
    within_1s(&mut h, "renamed file", |a| {
        item_names(a) == ["IMG_0000.ARW", "IMG_0009.ARW", "renamed.ARW"]
    });
    settle(&mut h);
    // A long copy reports events closer together than the debounce: the first file still
    // shows within 1 s, not only when the events stop.
    fs.add_file("/f/IMG_0010.ARW", raw(10));
    let t = Instant::now();
    let mut n = 11;
    while h.state().state().items.len() < 4 {
        assert!(
            t.elapsed() < Duration::from_secs(1),
            "a steady stream of events starved the rescan"
        );
        fs.add_file(format!("/f/IMG_{n:04}.ARW"), raw(10));
        n += 1;
        h.state().folder_changed();
        for _ in 0..5 {
            h.step();
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    settle(&mut h);
}

/// OPEN-3 with the real watcher and real files (full tier: it touches the disk).
mod full_folder_watch {
    use super::*;

    #[test]
    fn req_open_3_the_real_watcher_shows_added_removed_and_renamed_files_within_1_s() {
        let dir = std::env::temp_dir().join(format!("blink-open3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..2 {
            std::fs::write(dir.join(format!("IMG_{i:04}.ARW")), raw(i * 50)).unwrap();
        }
        let mut h = Harness::builder().with_size([800.0, 500.0]).build_ui_state(
            |ui, app: &mut BlinkApp| app.show(ui),
            BlinkApp::new(
                &egui::Context::default(),
                Some(dir.clone()),
                Options {
                    workers: 2,
                    ..Options::default()
                },
            ),
        );
        settle(&mut h);
        // Let the watch start before changing anything.
        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(dir.join("IMG_0002.ARW"), raw(120)).unwrap();
        within_1s(&mut h, "added file", |a| a.state().items.len() == 3);
        std::fs::remove_file(dir.join("IMG_0000.ARW")).unwrap();
        within_1s(&mut h, "removed file", |a| a.state().items.len() == 2);
        std::fs::rename(dir.join("IMG_0001.ARW"), dir.join("renamed.ARW")).unwrap();
        within_1s(&mut h, "renamed file", |a| {
            item_names(a) == ["IMG_0002.ARW", "renamed.ARW"]
        });
        settle(&mut h);
        drop(h);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Random actions for `secs` with a decode cache far smaller than the folder; the cache must
/// never hold more than its cap (PERF-8). Seeded and printed, so a failure can be replayed.
fn soak(secs: u64, seed: u64) {
    use blink_core::filter::Cond;
    use blink_core::state::{Action, MarkKey, Nav};
    eprintln!("soak: {secs} s, seed {seed}");
    let fs = Arc::new(folder(40));
    // One 600×400 preview is 1.68 MB in the cache's accounting: room for about five.
    let cap = 9 << 20;
    let mut h = Harness::builder().with_size([800.0, 500.0]).build_ui_state(
        |ui, app: &mut BlinkApp| app.show(ui),
        BlinkApp::new(
            &egui::Context::default(),
            Some("/f".into()),
            Options {
                fs: fs.clone(),
                workers: 2,
                cache_bytes: cap,
                ..Options::default()
            },
        ),
    );
    let mut x = seed | 1;
    let mut rnd = |n: u64| {
        // xorshift64
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x % n
    };
    let (t, mut steps, mut peak) = (Instant::now(), 0u64, 0usize);
    while t.elapsed() < Duration::from_secs(secs) {
        let a = match rnd(12) {
            0..=3 => Action::Navigate(Nav::Next),
            4 => Action::Navigate(Nav::Prev),
            5 => Action::Navigate(if rnd(2) == 0 { Nav::First } else { Nav::Last }),
            6 => Action::Mark(MarkKey::Stars(rnd(6) as u8)),
            7 => Action::ToggleZoom100 { cursor: None },
            8 => Action::AddFilter(Cond::RatingAtLeast(rnd(4) as i8)),
            9 => Action::RemoveFilter(0),
            10 => Action::Undo,
            _ => Action::ToggleClipping,
        };
        h.state_mut().dispatch(a);
        for _ in 0..=rnd(3) {
            h.step();
            let (used, cap) = h.state().cache_usage();
            peak = peak.max(used);
            assert!(
                used <= cap,
                "cache {used} > cap {cap} after {steps} steps (seed {seed})"
            );
        }
        steps += 1;
    }
    eprintln!("soak: {steps} actions, peak cache {peak} of {cap} bytes");
    assert!(steps > 0);
    assert!(peak > 0, "nothing was ever cached");
}

#[test]
fn req_perf_8_random_actions_never_exceed_the_decode_cache_cap() {
    soak(2, 0x5eed_0008);
}

/// The long soak: `cargo xtask soak --minutes N` (nightly) sets `BLINK_SOAK_SECS`.
mod full_soak {
    #[test]
    fn req_perf_8_soak_never_exceeds_the_decode_cache_cap() {
        let secs = std::env::var("BLINK_SOAK_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10);
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| d.as_nanos() as u64);
        super::soak(secs.max(1), seed);
    }
}

/// PERF-9: on slow storage (every call takes 30 ms) no file-system call is ever made from the
/// UI thread, through opening, navigation, marks, filters, undo, zoom and a rescan.
#[test]
fn req_perf_9_the_ui_thread_never_touches_slow_storage() {
    use blink_core::filter::Cond;
    use blink_core::state::{Action, MarkKey, Nav};
    use blink_runtime::fs::SlowFs;
    let slow = Arc::new(SlowFs::new(Arc::new(folder(12)), Duration::from_millis(30)));
    let ui_thread = std::thread::current().id();
    let mut h = Harness::builder().with_size([800.0, 500.0]).build_ui_state(
        |ui, app: &mut BlinkApp| app.show(ui),
        BlinkApp::new(
            &egui::Context::default(),
            Some("/f".into()),
            Options {
                fs: slow.clone(),
                workers: 2,
                ..Options::default()
            },
        ),
    );
    let actions = [
        Action::Navigate(Nav::Next),
        Action::Mark(MarkKey::Stars(3)),
        Action::Navigate(Nav::Next),
        Action::Navigate(Nav::Last),
        Action::ToggleZoom100 { cursor: None },
        Action::Navigate(Nav::Prev),
        Action::AddFilter(Cond::RatingAtLeast(1)),
        Action::RemoveFilter(0),
        Action::Undo,
        Action::ToggleClipping,
        Action::Navigate(Nav::First),
    ];
    let t = Instant::now();
    for a in actions {
        h.state_mut().dispatch(a);
        for _ in 0..5 {
            h.step();
        }
    }
    h.state().folder_changed();
    while t.elapsed() < Duration::from_secs(3) {
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    settle(&mut h);
    assert!(!slow.calls().is_empty(), "nothing was read at all");
    assert_eq!(
        slow.calls_from(ui_thread),
        [],
        "file-system calls on the UI thread"
    );
}
