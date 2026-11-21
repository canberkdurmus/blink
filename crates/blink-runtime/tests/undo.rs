//! RATE-3: any sequence of mark keys, navigation, undo and redo, with the sidecar writer
//! flushing at arbitrary points, followed by undo-all restores every sidecar byte for byte
//! (including absence).

use blink_core::formats::Formats;
use blink_core::state::{Action, Effect, LabelColor, MarkKey, Nav, State, reduce};
use blink_runtime::fs::MemFs;
use blink_runtime::sidecar::{Clock, DEBOUNCE, SidecarStyle, SidecarWriter};
use blink_testkit::sidecars;
use proptest::prelude::*;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

/// A clock that moves on a second per read, so every write has a new `xmp:MetadataDate`.
struct Ticking(AtomicI64);

impl Clock for Ticking {
    fn unix_secs(&self) -> i64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
}

/// A folder with every sidecar situation: Lightroom, darktable, both styles, none, a pair whose
/// members hold different values, a standalone JPEG, unpaired same-stem RAWs and a sidecar with
/// an out-of-range rating.
fn folder() -> MemFs {
    let fs = MemFs::new();
    fs.add_dir("/f");
    for raw in [
        "a.ARW", "b.ARW", "c.ARW", "d.ARW", "e.ARW", "g.ARW", "g.NEF", "h.ARW",
    ] {
        fs.add_file(format!("/f/{raw}"), b"raw bytes".to_vec());
    }
    fs.add_file("/f/c.JPG", b"\xff\xd8jpeg".to_vec());
    fs.add_file("/f/f.JPG", b"\xff\xd8jpeg".to_vec());
    fs.add_file("/f/a.xmp", sidecars::lightroom(2, Some("Blue")));
    fs.add_file("/f/b.ARW.xmp", sidecars::darktable(3));
    fs.add_file("/f/c.xmp", sidecars::bridge(4, None));
    fs.add_file("/f/c.JPG.xmp", sidecars::digikam(1, Some("Red")));
    fs.add_file("/f/e.xmp", sidecars::xap_prefix(5));
    fs.add_file("/f/e.ARW.xmp", sidecars::darktable(-1));
    fs.add_file(
        "/f/h.xmp",
        sidecars::lightroom(2, None).replace("=\"2\"", "=\"9\""),
    );
    // A subfolder, seen only while recursive scanning is on: a rescan drops its items.
    fs.add_file("/f/sub/s.ARW", b"raw bytes".to_vec());
    fs.add_file("/f/sub/s.xmp", sidecars::lightroom(4, Some("Green")));
    fs.add_file("/f/sub/t.ARW", b"raw bytes".to_vec());
    fs
}

fn snapshot(fs: &MemFs) -> BTreeMap<PathBuf, Vec<u8>> {
    fs.file_paths()
        .into_iter()
        .map(|p| {
            let b = fs.peek(&p).unwrap();
            (p, b)
        })
        .collect()
}

#[derive(Debug, Clone)]
enum Step {
    Do(Action),
    /// Let `ms` milliseconds pass: the writer flushes what is due.
    Wait(u64),
}

fn step() -> impl Strategy<Value = Step> {
    let key = prop_oneof![
        (0u8..=5).prop_map(MarkKey::Stars),
        Just(MarkKey::Pick),
        Just(MarkKey::Reject),
        Just(MarkKey::Unflag),
        (0usize..5).prop_map(|i| MarkKey::Label(LabelColor::ALL[i])),
    ];
    prop_oneof![
        4 => key.prop_map(|k| Step::Do(Action::Mark(k))),
        2 => prop_oneof![Just(Nav::Next), Just(Nav::Prev), Just(Nav::First), Just(Nav::Last)]
            .prop_map(|n| Step::Do(Action::Navigate(n))),
        2 => Just(Step::Do(Action::Undo)),
        1 => Just(Step::Do(Action::Redo)),
        1 => Just(Step::Do(Action::ToggleAutoAdvance)),
        1 => any::<bool>().prop_map(|on| Step::Do(Action::SetRecursive(on))),
        2 => (0u64..300).prop_map(Step::Wait),
    ]
}

struct Session<'a> {
    fs: &'a MemFs,
    state: State,
    writer: SidecarWriter<'a>,
    now: Instant,
}

impl Session<'_> {
    fn dispatch(&mut self, a: Action) {
        let (s, fx) = reduce(&self.state, a);
        self.state = s;
        for e in fx {
            match e {
                Effect::WriteSidecar {
                    source,
                    change,
                    owns_adobe_name,
                } => self.writer.queue(source, change, owns_adobe_name, self.now),
                // Scan and load marks the way the GUI does (the load sees what's on disk now).
                Effect::ScanFolder { folder, recursive } => {
                    let files =
                        blink_runtime::scan::scan_folder(self.fs, &folder, recursive).unwrap();
                    self.dispatch(Action::FolderScanned {
                        folder,
                        recursive,
                        files,
                    });
                    let marks = blink_runtime::marks::load_marks(self.fs, &self.state.items);
                    self.dispatch(Action::MarksLoaded(marks));
                }
                _ => {}
            }
        }
    }

    fn wait(&mut self, ms: u64) {
        self.now += Duration::from_millis(ms);
        for r in self.writer.flush_due(self.now) {
            r.result.unwrap();
        }
    }
}

fn open<'a>(fs: &'a MemFs, clock: &'a Ticking, style: SidecarStyle, pick: i8) -> Session<'a> {
    let mut state = State::new(Formats::new(["arw", "nef"]));
    state.prefs.pick_value = pick;
    let mut s = Session {
        fs,
        state,
        writer: SidecarWriter::new(fs, clock, style),
        now: Instant::now(),
    };
    s.dispatch(Action::OpenFolder("/f".into()));
    s
}

fn undo_all_restores_every_sidecar(steps: &[Step], style: SidecarStyle, pick: i8) {
    let fs = folder();
    let before = snapshot(&fs);
    let clock = Ticking(AtomicI64::new(1_795_251_900));
    let mut s = open(&fs, &clock, style, pick);
    assert_eq!(s.state.items.len(), 9);
    for st in steps {
        match st {
            Step::Do(a) => s.dispatch(a.clone()),
            Step::Wait(ms) => s.wait(*ms),
        }
    }
    s.wait(DEBOUNCE.as_millis() as u64);
    while !s.state.undo.is_empty() {
        s.dispatch(Action::Undo);
        s.wait(7);
    }
    for r in s.writer.flush_all() {
        r.result.unwrap();
    }
    let after = snapshot(&fs);
    let keys: std::collections::BTreeSet<_> = before.keys().chain(after.keys()).collect();
    for k in keys {
        let text = |m: &BTreeMap<PathBuf, Vec<u8>>| {
            m.get(k).map(|b| String::from_utf8_lossy(b).into_owned())
        };
        assert_eq!(text(&after), text(&before), "{} differs", k.display());
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn req_rate_3_any_actions_then_undo_all_restore_every_sidecar_byte_for_byte(
        steps in proptest::collection::vec(step(), 0..60),
        darktable in any::<bool>(),
        pick in 1i8..=5,
    ) {
        let style = if darktable { SidecarStyle::Darktable } else { SidecarStyle::Adobe };
        undo_all_restores_every_sidecar(&steps, style, pick);
    }
}

#[test]
fn req_rate_3_undo_all_after_a_full_pass_with_writes_flushed_each_step() {
    // Deterministic: mark every item with every key, flushing after each, then undo all.
    let mut steps = Vec::new();
    for _ in 0..9 {
        for k in [
            MarkKey::Stars(3),
            MarkKey::Label(LabelColor::Green),
            MarkKey::Pick,
            MarkKey::Unflag,
            MarkKey::Reject,
            MarkKey::Label(LabelColor::Green),
        ] {
            steps.push(Step::Do(Action::Mark(k)));
            steps.push(Step::Wait(150));
        }
        steps.push(Step::Do(Action::Navigate(Nav::Next)));
    }
    for style in [SidecarStyle::Adobe, SidecarStyle::Darktable] {
        undo_all_restores_every_sidecar(&steps, style, 1);
    }
}
