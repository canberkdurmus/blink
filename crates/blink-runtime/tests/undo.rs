//! RATE-3 acceptance: any action sequence followed by undo-all restores every sidecar byte for
//! byte, including absence. The reducer's effects run through the real sidecar writer on the
//! recording `MemFs`, with flushes (debounce expiries) at arbitrary points.

use blink_core::formats::Formats;
use blink_core::state::{Action, Effect, Nav, State, reduce};
use blink_runtime::fs::MemFs;
use blink_runtime::sidecar::{FixedClock, SidecarStyle, SidecarWriter};
use blink_testkit::sidecars;
use proptest::prelude::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn folder() -> MemFs {
    let fs = MemFs::new();
    // A pair whose sidecars disagree, a darktable sidecar, no sidecar, both styles, a JPEG.
    fs.add_file("/f/a.ARW", b"raw-a".to_vec());
    fs.add_file("/f/a.JPG", vec![0xFF, 0xD8, 0xFF, 0xD9]);
    fs.add_file("/f/a.xmp", sidecars::lightroom(2, Some("Red")));
    fs.add_file("/f/a.JPG.xmp", sidecars::digikam(1, None));
    fs.add_file("/f/b.ARW", b"raw-b".to_vec());
    fs.add_file("/f/b.ARW.xmp", sidecars::darktable(-1));
    fs.add_file("/f/c.ARW", b"raw-c".to_vec());
    fs.add_file("/f/d.ARW", b"raw-d".to_vec());
    fs.add_file("/f/d.xmp", sidecars::bridge(5, Some("Green")));
    fs.add_file("/f/d.ARW.xmp", sidecars::darktable(3));
    fs.add_file("/f/e.JPG", vec![0xFF, 0xD8, 0xFF, 0xD9]);
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
    Flush,
    /// Toggle recursive: rescan and reload marks (they now include Blink's writes).
    Rescan,
    /// Open the same folder again: a new undo session.
    Reopen,
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        3 => (0..=5i8).prop_map(|n| Step::Do(Action::Rate(n))),
        1 => Just(Step::Do(Action::Pick)),
        1 => Just(Step::Do(Action::Reject)),
        1 => Just(Step::Do(Action::Unflag)),
        2 => (0..5usize).prop_map(|i| Step::Do(Action::ToggleLabel(i))),
        2 => Just(Step::Do(Action::Navigate(Nav::Next))),
        1 => Just(Step::Do(Action::Navigate(Nav::Prev))),
        2 => Just(Step::Do(Action::Undo)),
        1 => Just(Step::Do(Action::Redo)),
        1 => Just(Step::Do(Action::ToggleAutoAdvance)),
        2 => Just(Step::Flush),
        1 => Just(Step::Rescan),
        1 => Just(Step::Reopen),
    ]
}

/// Drives the reducer like the GUI does: marks load right after the scan (the GUI ignores mark
/// edits until then), and opening a folder resets the writer.
/// Returns the snapshot taken when the last undo session began.
fn run(fs: &MemFs, style: SidecarStyle, steps: &[Step]) -> BTreeMap<PathBuf, Vec<u8>> {
    let clock = FixedClock(1_795_251_900);
    let mut w = SidecarWriter::new(fs, &clock, style);
    let mut held: Vec<Effect> = Vec::new();
    let loaded = true;
    let mut start = snapshot(fs);
    let mut s = State::new(Formats::new(["arw"]));
    let queue = |w: &mut SidecarWriter, e: Effect| {
        if let Effect::WriteSidecar {
            source,
            change,
            owns_adobe_name,
        } = e
        {
            w.queue(source, change, owns_adobe_name, Instant::now());
        }
    };
    let exec = |s: &State,
                a: Action,
                w: &mut SidecarWriter,
                held: &mut Vec<Effect>,
                loaded: bool|
     -> State {
        let (t, fx) = reduce(s, a);
        for e in fx {
            if loaded { queue(w, e) } else { held.push(e) }
        }
        t
    };
    let scan = |s: &State, recursive: bool| -> State {
        let files = blink_runtime::scan::scan_folder(fs, Path::new("/f"), recursive).unwrap();
        reduce(
            s,
            Action::FolderScanned {
                folder: "/f".into(),
                recursive,
                files,
            },
        )
        .0
    };
    let load = |s: &State| -> State {
        let marks = blink_runtime::marks::load_marks(fs, &s.items);
        reduce(s, Action::MarksLoaded(marks)).0
    };
    // Open.
    s = reduce(&s, Action::OpenFolder("/f".into())).0;
    s = scan(&s, false);
    s = load(&s);
    for st in steps {
        match st {
            Step::Do(a) => s = exec(&s, a.clone(), &mut w, &mut held, loaded),
            Step::Flush => {
                for r in w.flush_all() {
                    r.result.unwrap();
                }
            }
            Step::Rescan => {
                let rec = !s.recursive;
                s = reduce(&s, Action::SetRecursive(rec)).0;
                s = scan(&s, rec);
                s = load(&s);
            }
            Step::Reopen => {
                for r in w.reset() {
                    r.result.unwrap();
                }
                start = snapshot(fs);
                s = reduce(&s, Action::OpenFolder("/f".into())).0;
                let rec = s.recursive;
                s = scan(&s, rec);
                s = load(&s);
            }
        }
    }
    while !s.undo.is_empty() {
        s = exec(&s, Action::Undo, &mut w, &mut held, loaded);
    }
    for r in w.flush_all() {
        r.result.unwrap();
    }
    start
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn req_rate_3_undo_all_restores_every_sidecar_byte_for_byte(
        steps in prop::collection::vec(step(), 0..40),
        darktable in any::<bool>(),
    ) {
        let fs = folder();
        let style = if darktable { SidecarStyle::Darktable } else { SidecarStyle::Adobe };
        let before = run(&fs, style, &steps);
        let after = snapshot(&fs);
        prop_assert_eq!(
            before.keys().collect::<Vec<_>>(),
            after.keys().collect::<Vec<_>>(),
            "sidecars appeared or vanished"
        );
        for (p, b) in &before {
            prop_assert!(after.get(p) == Some(b), "{} differs", p.display());
        }
    }
}

#[test]
fn req_rate_3_rating_twice_then_undo_twice_restores_the_original() {
    let fs = folder();
    let before = snapshot(&fs);
    let start = run(
        &fs,
        SidecarStyle::Adobe,
        &[
            Step::Do(Action::Rate(4)),
            Step::Flush,
            Step::Do(Action::ToggleLabel(1)),
            Step::Do(Action::Navigate(Nav::Next)),
            Step::Do(Action::Navigate(Nav::Next)),
            Step::Do(Action::Rate(3)),
            Step::Flush,
        ],
    );
    assert_eq!(start, before);
    assert_eq!(snapshot(&fs), before);
}

#[test]
fn req_rate_3_rescan_and_reopen_keep_undo_exact() {
    // The reviewer's scenarios: rate, flush, rescan (reload reads Blink's write), rate, undo all;
    // and rate, flush, reopen, rate, undo all.
    for steps in [
        vec![
            Step::Do(Action::Rate(4)),
            Step::Flush,
            Step::Rescan,
            Step::Do(Action::Rate(5)),
            Step::Flush,
        ],
        vec![
            Step::Do(Action::Rate(4)),
            Step::Flush,
            Step::Reopen,
            Step::Do(Action::Rate(3)),
            Step::Flush,
            Step::Do(Action::Rate(4)),
        ],
    ] {
        let fs = folder();
        let start = run(&fs, SidecarStyle::Adobe, &steps);
        assert_eq!(snapshot(&fs), start, "{steps:?}");
    }
}
