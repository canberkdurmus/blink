//! Loader behaviour with a memory file system and synthetic files (VIEW-1, VIEW-4).

use blink_core::formats::Formats;
use blink_core::item::ScannedFile;
use blink_core::preview::PreviewSource;
use blink_core::state::{Action, Effect, Level, Nav, State, reduce};
use blink_core::view::Zoom;
use blink_decode::image::DecodeError;
use blink_decode::raw::{RawBackend, RawProbe};
use blink_decode::{Preview, RgbImage};
use blink_runtime::fs::MemFs;
use blink_runtime::loader::Loader;
use blink_testkit::fixtures::{jpeg, quadrants, tiff_with_jpegs};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// A backend that knows a 400×300 sensor and "develops" it to a flat grey image.
struct FakeRaw;

impl RawBackend for FakeRaw {
    fn name(&self) -> &'static str {
        "fake"
    }
    fn extensions(&self) -> Vec<String> {
        vec!["arw".into()]
    }
    fn probe(&self, _: &[u8]) -> Result<RawProbe, DecodeError> {
        Ok(RawProbe {
            make: "Fake".into(),
            model: "F".into(),
            sensor_w: 400,
            sensor_h: 300,
        })
    }
    fn develop(&self, _: &[u8]) -> Result<RgbImage, DecodeError> {
        Ok(RgbImage::new(400, 300, vec![77; 400 * 300 * 3]).unwrap())
    }
}

/// A RAW stand-in with a 160×120 embedded preview (LOW-RES against the 400 px sensor).
fn low_res_raw() -> Vec<u8> {
    tiff_with_jpegs(&[jpeg(
        &quadrants(160, 120, [[200, 0, 0], [0, 200, 0], [0, 0, 200], [9, 9, 9]]),
        160,
        120,
        95,
        None,
    )])
}

struct Session {
    state: State,
    loader: Loader<(u32, u32)>,
}

impl Session {
    fn new(fs: MemFs, names: &[&str]) -> Self {
        let files: Vec<ScannedFile> = names
            .iter()
            .map(|n| ScannedFile {
                path: Path::new("/f").join(n),
                len: 1,
            })
            .collect();
        let loader = Loader::new(
            Arc::new(fs),
            Arc::new(FakeRaw),
            2,
            1 << 30,
            |p: &Preview| (p.image.width, p.image.height),
            || {},
        );
        let mut s = Self {
            state: State::new(Formats::new(["arw"])),
            loader,
        };
        s.dispatch(Action::OpenFolder("/f".into()));
        s.dispatch(Action::FolderScanned {
            folder: "/f".into(),
            recursive: false,
            files,
        });
        s
    }

    fn dispatch(&mut self, a: Action) -> Vec<Effect> {
        let (st, fx) = reduce(&self.state, a);
        self.state = st;
        for e in &fx {
            if let Some(a) = self.loader.execute(e) {
                self.dispatch(a);
            }
        }
        fx
    }

    /// Feed loader results until `done` holds.
    fn until(&mut self, done: impl Fn(&State) -> bool) {
        let t = std::time::Instant::now();
        while !done(&self.state) {
            assert!(
                t.elapsed() < Duration::from_secs(20),
                "timeout: {:?}",
                self.state.loupe
            );
            if let Some(a) = self.loader.wait(Duration::from_millis(50)) {
                self.dispatch(a);
            }
        }
    }
}

fn source(s: &State) -> Option<PreviewSource> {
    s.loupe.shown.as_ref().map(|x| x.source)
}

#[test]
fn req_view_1_first_item_shows_its_embedded_preview_and_neighbours_are_prefetched() {
    let fs = MemFs::new();
    for n in ["a", "b", "c"] {
        fs.add_file(format!("/f/{n}.ARW"), low_res_raw());
    }
    let mut s = Session::new(fs, &["a.ARW", "b.ARW", "c.ARW"]);
    s.until(|st| st.loupe.shown.is_some());
    let shown = s.state.loupe.shown.clone().unwrap();
    assert_eq!(shown.source, PreviewSource::EmbeddedLowRes);
    assert_eq!((shown.size, shown.full_size), ((160, 120), (400, 300)));
    let l = s.loupe_loaded("/f/a.ARW");
    assert_eq!(l.prepared, (160, 120));
    assert_eq!(l.histogram.source, PreviewSource::EmbeddedLowRes);
    // Prefetched: moving on is answered from the cache in the same dispatch.
    let t = std::time::Instant::now();
    while !(s
        .loader
        .get(Path::new("/f/b.ARW"), Level::Preview)
        .is_some()
        && s.loader
            .get(Path::new("/f/c.ARW"), Level::Preview)
            .is_some())
    {
        assert!(t.elapsed() < Duration::from_secs(20));
        std::thread::sleep(Duration::from_millis(5));
    }
    s.dispatch(Action::Navigate(Nav::Next));
    assert_eq!(source(&s.state), Some(PreviewSource::EmbeddedLowRes));
    assert_eq!(
        s.state.loupe.shown.as_ref().unwrap().path,
        Path::new("/f/b.ARW")
    );
}

impl Session {
    fn loupe_loaded(&self, p: &str) -> Arc<blink_runtime::loader::Loaded<(u32, u32)>> {
        self.loader.best(Path::new(p)).expect("cached")
    }
}

#[test]
fn req_view_4_zoom_100_on_low_res_raw_ends_with_raw_decode_at_sensor_size() {
    let fs = MemFs::new();
    fs.add_file("/f/a.ARW", low_res_raw());
    let mut s = Session::new(fs, &["a.ARW"]);
    s.until(|st| st.loupe.shown.is_some());
    s.dispatch(Action::ToggleZoom100 {
        cursor: Some([10.0, 10.0]),
    });
    assert!(s.state.loupe.pending_full, "spinner while decoding");
    assert_eq!(source(&s.state), Some(PreviewSource::EmbeddedLowRes));
    s.until(|st| !st.loupe.pending_full);
    let shown = s.state.loupe.shown.clone().unwrap();
    assert_eq!(shown.source, PreviewSource::RawDecode);
    assert_eq!(shown.size, (400, 300));
    assert_eq!(s.loupe_loaded("/f/a.ARW").level, Level::Full);
}

#[test]
fn req_view_4_zoom_on_low_res_raw_with_a_paired_jpeg_uses_the_jpeg() {
    let fs = MemFs::new();
    fs.add_file("/f/a.ARW", low_res_raw());
    fs.add_file(
        "/f/a.JPG",
        jpeg(&vec![128; 400 * 300 * 3], 400, 300, 90, None),
    );
    let mut s = Session::new(fs, &["a.ARW", "a.JPG"]);
    s.until(|st| st.loupe.shown.is_some());
    s.dispatch(Action::SetZoom {
        zoom: Zoom::P100,
        cursor: None,
    });
    s.until(|st| !st.loupe.pending_full);
    let shown = s.state.loupe.shown.clone().unwrap();
    assert_eq!(
        (shown.source, shown.size),
        (PreviewSource::PairedJpeg, (400, 300))
    );
}

#[test]
fn req_view_1_heif_and_unreadable_files_show_no_preview() {
    let fs = MemFs::new();
    fs.add_file("/f/a.HEIC", vec![0u8; 16]);
    let mut s = Session::new(fs, &["a.HEIC", "b.ARW"]);
    s.until(|st| st.loupe.shown.is_some());
    assert_eq!(source(&s.state), Some(PreviewSource::Unavailable));
    s.dispatch(Action::Navigate(Nav::Next)); // b.ARW is missing from the file system
    s.until(|st| st.loupe.shown.is_some());
    assert_eq!(source(&s.state), Some(PreviewSource::Unavailable));
}

#[test]
fn loader_cache_evicts_least_recently_used_beyond_its_cap() {
    let fs = MemFs::new();
    for n in ["a", "b", "c", "d"] {
        fs.add_file(format!("/f/{n}.ARW"), low_res_raw());
    }
    // Room for about two 160×120 previews.
    let loader: Loader<()> = Loader::new(
        Arc::new(fs),
        Arc::new(FakeRaw),
        1,
        160 * 120 * 7 * 2 + 10,
        |_| (),
        || {},
    );
    let item = |n: &str| blink_core::state::ItemRef {
        path: format!("/f/{n}.ARW").into(),
        kind: blink_core::formats::FileKind::Raw,
        paired_jpeg: None,
    };
    for n in ["a", "b", "c", "d"] {
        loader.show(&item(n), &[]);
        loader.wait(Duration::from_secs(20)).expect("loaded");
    }
    assert!(loader.cache_bytes() <= 160 * 120 * 7 * 2 + 10);
    assert!(loader.get(Path::new("/f/d.ARW"), Level::Preview).is_some());
    assert!(loader.get(Path::new("/f/a.ARW"), Level::Preview).is_none());
}

/// An Fs whose reads of one path block until released.
struct GateFs {
    inner: MemFs,
    gated: std::path::PathBuf,
    open: std::sync::Mutex<bool>,
    cv: std::sync::Condvar,
    entered: std::sync::Mutex<bool>,
}

impl blink_runtime::fs::Fs for GateFs {
    fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        if path == self.gated {
            *self.entered.lock().unwrap() = true;
            let mut open = self.open.lock().unwrap();
            while !*open {
                open = self.cv.wait(open).unwrap();
            }
        }
        self.inner.read(path)
    }
    fn metadata(&self, path: &Path) -> std::io::Result<blink_runtime::fs::Meta> {
        self.inner.metadata(path)
    }
    fn read_dir(&self, path: &Path) -> std::io::Result<Vec<blink_runtime::fs::DirEntry>> {
        self.inner.read_dir(path)
    }
}

#[test]
fn req_view_1_a_running_prefetch_still_wanted_by_a_newer_show_is_not_dropped() {
    let inner = MemFs::new();
    for n in ["a", "b", "c"] {
        inner.add_file(format!("/f/{n}.ARW"), low_res_raw());
    }
    let fs = Arc::new(GateFs {
        inner,
        gated: "/f/c.ARW".into(),
        open: std::sync::Mutex::new(false),
        cv: std::sync::Condvar::new(),
        entered: std::sync::Mutex::new(false),
    });
    let loader: Loader<()> = Loader::new(fs.clone(), Arc::new(FakeRaw), 2, 1 << 30, |_| (), || {});
    let item = |n: &str| blink_core::state::ItemRef {
        path: format!("/f/{n}.ARW").into(),
        kind: blink_core::formats::FileKind::Raw,
        paired_jpeg: None,
    };
    loader.show(&item("a"), &[item("c")]);
    let t = std::time::Instant::now();
    while !*fs.entered.lock().unwrap() {
        assert!(t.elapsed() < Duration::from_secs(20));
        std::thread::sleep(Duration::from_millis(1));
    }
    // c is mid-read; the next Show still wants it.
    loader.show(&item("b"), &[item("c")]);
    *fs.open.lock().unwrap() = true;
    fs.cv.notify_all();
    let t = std::time::Instant::now();
    while loader.get(Path::new("/f/c.ARW"), Level::Preview).is_none() {
        assert!(t.elapsed() < Duration::from_secs(20), "c was dropped");
        std::thread::sleep(Duration::from_millis(5));
    }
}
