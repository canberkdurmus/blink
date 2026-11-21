//! FILE-4 (naming), FILE-6 (atomic writes, debounce), FILE-7 (sources untouched), FILE-9, OPEN-2
//! (a sidecar per pair member) through the runtime's sidecar writer on the recording `MemFs`.

use blink_core::formats::Formats;
use blink_core::state::{Action, Effect, MarkChange, State, reduce};
use blink_runtime::fs::{Fs, FsCall, MemFs};
use blink_runtime::sidecar::{DEBOUNCE, FixedClock, SidecarStyle, SidecarWriter, sidecar_targets};
use blink_testkit::sidecars;
use blink_xmp::{OwnedFields, read_owned};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const NOW: i64 = 1_795_251_900; // 2026-11-21T09:05:00Z

fn owned(fs: &MemFs, p: &str) -> OwnedFields {
    read_owned(&fs.peek(p).unwrap_or_else(|| panic!("{p} missing"))).unwrap()
}

fn r(v: i8) -> OwnedFields {
    OwnedFields {
        rating: Some(v),
        label: None,
    }
}

fn xmps(fs: &MemFs) -> Vec<String> {
    fs.file_paths()
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("xmp")))
        .map(|p| p.display().to_string())
        .collect()
}

fn write(fs: &MemFs, style: SidecarStyle, source: &str, owns: bool, rating: i8) {
    let clock = FixedClock(NOW);
    let mut w = SidecarWriter::new(fs, &clock, style);
    w.queue(
        source.into(),
        MarkChange::Rating(Some(rating)),
        owns,
        Instant::now(),
    );
    for rep in w.flush_all() {
        rep.result.unwrap();
    }
}

#[test]
fn req_file_4_fixture_matrix() {
    // (existing sidecars, style, owns the Adobe name) → sidecars that hold the new rating.
    let lr = sidecars::lightroom(1, None);
    let dt = sidecars::darktable(1);
    type Case<'a> = (&'a [&'a str], SidecarStyle, bool, &'a [&'a str]);
    let cases: &[Case] = &[
        (&[], SidecarStyle::Adobe, true, &["/f/a.xmp"]),
        (&[], SidecarStyle::Darktable, true, &["/f/a.ARW.xmp"]),
        (&["/f/a.xmp"], SidecarStyle::Adobe, true, &["/f/a.xmp"]),
        (&["/f/a.xmp"], SidecarStyle::Darktable, true, &["/f/a.xmp"]),
        (
            &["/f/a.ARW.xmp"],
            SidecarStyle::Adobe,
            true,
            &["/f/a.ARW.xmp"],
        ),
        (
            &["/f/a.ARW.xmp"],
            SidecarStyle::Darktable,
            true,
            &["/f/a.ARW.xmp"],
        ),
        (
            &["/f/a.xmp", "/f/a.ARW.xmp"],
            SidecarStyle::Adobe,
            true,
            &["/f/a.xmp", "/f/a.ARW.xmp"],
        ),
        (
            &["/f/a.xmp", "/f/a.ARW.xmp"],
            SidecarStyle::Darktable,
            true,
            &["/f/a.xmp", "/f/a.ARW.xmp"],
        ),
        (&["/f/a.XMP"], SidecarStyle::Adobe, true, &["/f/a.XMP"]),
        (
            &["/f/a.ARW.XMP"],
            SidecarStyle::Adobe,
            true,
            &["/f/a.ARW.XMP"],
        ),
        // Doesn't own `a.xmp` (pair companion): never touches it, uses its own darktable name.
        (&[], SidecarStyle::Adobe, false, &["/f/a.ARW.xmp"]),
        (&["/f/a.xmp"], SidecarStyle::Adobe, false, &["/f/a.ARW.xmp"]),
        (
            &["/f/a.xmp", "/f/a.ARW.xmp"],
            SidecarStyle::Adobe,
            false,
            &["/f/a.ARW.xmp"],
        ),
    ];
    for (existing, style, owns, want) in cases {
        let fs = MemFs::new();
        fs.add_file("/f/a.ARW", b"raw".to_vec());
        for e in *existing {
            let body = if e.contains(".ARW.") { &dt } else { &lr };
            fs.add_file(*e, body.as_bytes().to_vec());
        }
        let t = sidecar_targets(&fs, Path::new("/f/a.ARW"), *style, *owns);
        assert_eq!(
            t,
            want.iter().map(PathBuf::from).collect::<Vec<_>>(),
            "{existing:?} {style:?} {owns}"
        );
        write(&fs, *style, "/f/a.ARW", *owns, 4);
        for w in *want {
            assert_eq!(
                owned(&fs, w).rating,
                Some(4),
                "{w} in {existing:?} {style:?}"
            );
        }
        // Nothing else was created; other existing sidecars are untouched.
        let mut all: Vec<String> = existing
            .iter()
            .chain(want.iter())
            .map(|s| s.to_string())
            .collect();
        all.sort();
        all.dedup();
        assert_eq!(xmps(&fs), all);
        for e in *existing {
            if !want.contains(e) {
                assert_eq!(owned(&fs, e).rating, Some(1));
            }
        }
    }
}

fn opened(fs: &MemFs, dir: &str) -> State {
    let files = blink_runtime::scan::scan_folder(fs, Path::new(dir), false).unwrap();
    let s = State::new(Formats::new(["arw", "nef"]));
    let (s, _) = reduce(&s, Action::OpenFolder(dir.into()));
    reduce(
        &s,
        Action::FolderScanned {
            folder: dir.into(),
            recursive: false,
            files,
        },
    )
    .0
}

/// Run the reducer's sidecar effects through a writer and flush.
fn run(fs: &MemFs, style: SidecarStyle, s: &State, a: Action) -> State {
    let clock = FixedClock(NOW);
    let mut w = SidecarWriter::new(fs, &clock, style);
    let (s, fx) = reduce(s, a);
    for e in fx {
        if let Effect::WriteSidecar {
            source,
            change,
            owns_adobe_name,
        } = e
        {
            w.queue(source, change, owns_adobe_name, Instant::now());
        }
    }
    for rep in w.flush_all() {
        rep.result.unwrap();
    }
    s
}

#[test]
fn req_open_2_req_file_4_every_pair_member_gets_its_own_sidecar_with_the_same_values() {
    for style in [SidecarStyle::Adobe, SidecarStyle::Darktable] {
        let fs = MemFs::new();
        fs.add_file("/p/a.ARW", b"raw".to_vec());
        fs.add_file("/p/a.JPG", vec![0xFF, 0xD8, 0xFF, 0xD9]);
        fs.add_file("/p/a.HEIC", b"heif".to_vec());
        let s = opened(&fs, "/p");
        assert_eq!(s.items.len(), 1);
        let s = run(&fs, style, &s, Action::SetRating { item: 0, rating: 3 });
        run(
            &fs,
            style,
            &s,
            Action::SetLabel {
                item: 0,
                label: Some("Red".into()),
            },
        );
        let raw = match style {
            SidecarStyle::Adobe => "/p/a.xmp",
            SidecarStyle::Darktable => "/p/a.ARW.xmp",
        };
        let want = OwnedFields {
            rating: Some(3),
            label: Some("Red".into()),
        };
        let mut paths = vec![raw, "/p/a.JPG.xmp", "/p/a.HEIC.xmp"];
        for p in &paths {
            assert_eq!(owned(&fs, p), want, "{p} ({style:?})");
        }
        paths.sort();
        assert_eq!(xmps(&fs), paths);
    }
}

#[test]
fn req_file_4_unpaired_same_stem_files_never_share_a_sidecar() {
    // Two RAWs with one stem don't pair (ambiguous): each gets its own darktable-style sidecar.
    let fs = MemFs::new();
    fs.add_file("/u/a.ARW", b"raw".to_vec());
    fs.add_file("/u/a.NEF", b"raw".to_vec());
    let s = opened(&fs, "/u");
    assert_eq!(s.items.len(), 2);
    let s = run(
        &fs,
        SidecarStyle::Adobe,
        &s,
        Action::SetRating { item: 0, rating: 2 },
    );
    run(
        &fs,
        SidecarStyle::Adobe,
        &s,
        Action::SetRating { item: 1, rating: 5 },
    );
    assert_eq!(xmps(&fs), ["/u/a.ARW.xmp", "/u/a.NEF.xmp"]);
    assert_eq!(owned(&fs, "/u/a.ARW.xmp"), r(2));
    assert_eq!(owned(&fs, "/u/a.NEF.xmp"), r(5));
}

#[test]
fn req_file_9_every_write_sets_metadata_date() {
    let fs = MemFs::new();
    fs.add_file("/f/a.ARW", b"raw".to_vec());
    fs.add_file("/f/a.xmp", sidecars::bridge(1, None));
    write(&fs, SidecarStyle::Adobe, "/f/a.ARW", true, 2);
    let t = String::from_utf8(fs.peek("/f/a.xmp").unwrap()).unwrap();
    assert!(
        t.contains("<xmp:MetadataDate>2026-11-21T09:05:00+00:00</xmp:MetadataDate>"),
        "{t}"
    );
    fs.add_file("/f/b.ARW", b"raw".to_vec());
    write(&fs, SidecarStyle::Adobe, "/f/b.ARW", true, 2);
    let t = String::from_utf8(fs.peek("/f/b.xmp").unwrap()).unwrap();
    assert!(
        t.contains("xmp:MetadataDate=\"2026-11-21T09:05:00+00:00\""),
        "{t}"
    );
}

#[test]
fn req_file_6_rating_writes_are_debounced_and_coalesced_within_250_ms() {
    const { assert!(DEBOUNCE.as_millis() <= 250) };
    let fs = MemFs::new();
    fs.add_file("/f/a.ARW", b"raw".to_vec());
    let clock = FixedClock(NOW);
    let mut w = SidecarWriter::new(&fs, &clock, SidecarStyle::Adobe);
    let t0 = Instant::now();
    w.queue("/f/a.ARW".into(), MarkChange::Rating(Some(1)), true, t0);
    w.queue(
        "/f/a.ARW".into(),
        MarkChange::Rating(Some(2)),
        true,
        t0 + Duration::from_millis(30),
    );
    w.queue(
        "/f/a.ARW".into(),
        MarkChange::Label(Some("Red".into())),
        true,
        t0 + Duration::from_millis(60),
    );
    // Not due yet: nothing written.
    assert!(
        w.flush_due(t0 + DEBOUNCE - Duration::from_millis(1))
            .is_empty()
    );
    assert!(!fs.calls().iter().any(FsCall::is_write));
    // Due at first change + DEBOUNCE, even though later changes arrived (bounded latency).
    assert_eq!(w.next_due(), Some(t0 + DEBOUNCE));
    let reps = w.flush_due(t0 + DEBOUNCE);
    assert_eq!(reps.len(), 1);
    assert_eq!(
        owned(&fs, "/f/a.xmp"),
        OwnedFields {
            rating: Some(2),
            label: Some("Red".into())
        }
    );
    // One atomic write for three changes.
    let renames = fs
        .calls()
        .iter()
        .filter(|c| matches!(c, FsCall::Rename(..)))
        .count();
    assert_eq!(renames, 1);
    assert!(!w.has_pending());
}

#[test]
fn req_file_6_write_is_temp_fsync_rename_fsync_dir_in_the_same_directory() {
    let fs = MemFs::new();
    fs.add_file("/f/a.ARW", b"raw".to_vec());
    write(&fs, SidecarStyle::Adobe, "/f/a.ARW", true, 3);
    let w: Vec<FsCall> = fs.calls().into_iter().filter(FsCall::is_write).collect();
    let [
        FsCall::WriteNew(tmp),
        FsCall::SyncFile(t2),
        FsCall::Rename(t3, to),
        FsCall::SyncDir(d),
    ] = w.as_slice()
    else {
        panic!("{w:?}")
    };
    assert_eq!(tmp.parent(), Some(Path::new("/f")));
    assert!(tmp.file_name().unwrap().to_string_lossy().starts_with('.'));
    assert!(tmp == t2 && tmp == t3);
    assert_eq!(to, Path::new("/f/a.xmp"));
    assert_eq!(d, Path::new("/f"));
}

#[test]
fn req_file_7_writes_only_ever_touch_sidecars() {
    let fs = MemFs::new();
    fs.add_file("/f/a.ARW", b"raw".to_vec());
    fs.add_file("/f/a.JPG", vec![0xFF, 0xD8, 0xFF, 0xD9]);
    fs.add_file("/f/b.JPG", vec![0xFF, 0xD8, 0xFF, 0xD9]);
    fs.add_file("/f/b.xmp", sidecars::lightroom(2, None));
    let before: Vec<_> = fs
        .file_paths()
        .into_iter()
        .map(|p| (fs.peek(&p), p))
        .collect();
    let s = opened(&fs, "/f");
    let s = run(
        &fs,
        SidecarStyle::Adobe,
        &s,
        Action::SetRating { item: 0, rating: 5 },
    );
    let s = run(
        &fs,
        SidecarStyle::Darktable,
        &s,
        Action::SetLabel {
            item: 1,
            label: Some("Red".into()),
        },
    );
    run(
        &fs,
        SidecarStyle::Adobe,
        &s,
        Action::SetRating {
            item: 1,
            rating: -1,
        },
    );
    for c in fs.calls().iter().filter(|c| c.is_write()) {
        for p in c.changed_paths() {
            let n = p.file_name().unwrap().to_string_lossy();
            assert!(
                n.ends_with(".xmp") || n.ends_with(".tmp"),
                "write touched {p:?}"
            );
        }
        assert!(!matches!(c, FsCall::RemoveFile(p) if !p.to_string_lossy().ends_with(".tmp")));
    }
    for (bytes, p) in before {
        if !p.to_string_lossy().ends_with(".xmp") {
            assert_eq!(fs.peek(&p), bytes, "{p:?} changed");
        }
    }
    // The atomic writer refuses non-sidecar targets outright.
    assert!(blink_runtime::atomic::write_atomic(&fs, Path::new("/f/a.ARW"), b"x").is_err());
    assert!(blink_runtime::atomic::remove_sidecar(&fs, Path::new("/f/a.JPG")).is_err());
    assert_eq!(fs.peek("/f/a.ARW").unwrap(), b"raw");
}

#[test]
fn req_file_5_unparseable_sidecar_is_left_alone_and_the_change_stays_queued() {
    let fs = MemFs::new();
    fs.add_file("/f/a.ARW", b"raw".to_vec());
    fs.add_file("/f/a.xmp", b"<x:xmpmeta><rdf:RDF>".to_vec());
    let clock = FixedClock(NOW);
    let mut w = SidecarWriter::new(&fs, &clock, SidecarStyle::Adobe);
    w.queue(
        "/f/a.ARW".into(),
        MarkChange::Rating(Some(1)),
        true,
        Instant::now(),
    );
    let reps = w.flush_all();
    assert!(reps[0].result.is_err());
    assert_eq!(fs.peek("/f/a.xmp").unwrap(), b"<x:xmpmeta><rdf:RDF>");
    assert!(w.has_pending());
}

mod full_crash {
    use super::*;

    /// Every way a write can end: fail at step k (the process dies there), then lose power.
    /// The sidecar must hold the old bytes or the new bytes, never anything else, and must not
    /// be missing if it existed.
    fn crash_matrix(old: Option<&str>, owns: bool, style: SidecarStyle, target: &str) {
        // Count the steps of a clean run first.
        let fresh = || {
            let fs = MemFs::new();
            fs.add_file("/f/a.ARW", b"raw".to_vec());
            if let Some(o) = old {
                fs.add_file(target, o.as_bytes().to_vec());
            }
            fs
        };
        let clean = fresh();
        write(&clean, style, "/f/a.ARW", owns, 4);
        let new = clean.peek(target).unwrap();
        let steps = clean.write_count();
        assert!(steps >= 4, "{steps}");
        for k in 0..=steps {
            for power_loss in [false, true] {
                let fs = fresh();
                fs.fail_at_write(k);
                let clock = FixedClock(NOW);
                let mut w = SidecarWriter::new(&fs, &clock, style);
                w.queue(
                    "/f/a.ARW".into(),
                    MarkChange::Rating(Some(4)),
                    owns,
                    Instant::now(),
                );
                let rep = w.flush_all();
                if k < steps {
                    assert!(rep[0].result.is_err(), "step {k} should fail");
                }
                if power_loss {
                    fs.power_loss();
                }
                let got = fs.peek(target);
                let ok =
                    got.as_deref() == Some(&new[..]) || got.as_deref() == old.map(str::as_bytes);
                assert!(
                    ok,
                    "fail at step {k}, power loss {power_loss}: {target} = {:?}",
                    got.map(|b| String::from_utf8_lossy(&b).into_owned())
                );
                assert_eq!(fs.peek("/f/a.ARW").unwrap(), b"raw");
                // After a "reboot", a retry completes the write.
                if k < steps {
                    let mut w = SidecarWriter::new(&fs, &clock, style);
                    w.queue(
                        "/f/a.ARW".into(),
                        MarkChange::Rating(Some(4)),
                        owns,
                        Instant::now(),
                    );
                    assert!(w.flush_all()[0].result.is_ok() || !power_loss);
                    if power_loss {
                        assert_eq!(owned(&fs, target).rating, Some(4));
                    }
                }
            }
        }
    }

    #[test]
    fn req_file_6_crash_at_every_step_leaves_the_old_or_the_new_sidecar() {
        crash_matrix(
            Some(&sidecars::lightroom(1, Some("Red"))),
            true,
            SidecarStyle::Adobe,
            "/f/a.xmp",
        );
        crash_matrix(
            Some(&sidecars::bridge(2, None)),
            true,
            SidecarStyle::Adobe,
            "/f/a.xmp",
        );
        crash_matrix(
            Some(&sidecars::darktable(3)),
            true,
            SidecarStyle::Adobe,
            "/f/a.ARW.xmp",
        );
        crash_matrix(None, true, SidecarStyle::Adobe, "/f/a.xmp");
        crash_matrix(None, false, SidecarStyle::Adobe, "/f/a.ARW.xmp");
    }

    #[test]
    fn req_file_6_crash_while_updating_both_styles_leaves_each_old_or_new() {
        let lr = sidecars::lightroom(1, None);
        let dt = sidecars::darktable(1);
        let fresh = || {
            let fs = MemFs::new();
            fs.add_file("/f/a.ARW", b"raw".to_vec());
            fs.add_file("/f/a.xmp", lr.as_bytes().to_vec());
            fs.add_file("/f/a.ARW.xmp", dt.as_bytes().to_vec());
            fs
        };
        let clean = fresh();
        write(&clean, SidecarStyle::Adobe, "/f/a.ARW", true, 5);
        let steps = clean.write_count();
        for k in 0..steps {
            let fs = fresh();
            fs.fail_at_write(k);
            write_ignoring_errors(&fs);
            fs.power_loss();
            for (p, old) in [("/f/a.xmp", &lr), ("/f/a.ARW.xmp", &dt)] {
                let got = fs.peek(p).unwrap();
                assert!(
                    got == old.as_bytes() || got == clean.peek(p).unwrap(),
                    "step {k}: {p}"
                );
            }
        }
    }

    fn write_ignoring_errors(fs: &MemFs) {
        let clock = FixedClock(NOW);
        let mut w = SidecarWriter::new(fs, &clock, SidecarStyle::Adobe);
        w.queue(
            "/f/a.ARW".into(),
            MarkChange::Rating(Some(5)),
            true,
            Instant::now(),
        );
        let _ = w.flush_all();
    }

    #[test]
    fn req_file_6_the_memfs_model_catches_a_writer_that_skips_fsync() {
        // Guard for the test itself: renaming unsynced data and losing power must corrupt.
        let fs = MemFs::new();
        fs.add_file("/f/a.xmp", b"old".to_vec());
        fs.write_new(Path::new("/f/.t"), b"new").unwrap();
        fs.rename(Path::new("/f/.t"), Path::new("/f/a.xmp"))
            .unwrap();
        fs.sync_dir(Path::new("/f")).unwrap();
        fs.power_loss();
        assert_eq!(fs.peek("/f/a.xmp").unwrap(), b"");
        // And a rename that was never made durable is rolled back.
        let fs = MemFs::new();
        fs.add_file("/f/a.xmp", b"old".to_vec());
        fs.write_new(Path::new("/f/.t"), b"new").unwrap();
        fs.sync_file(Path::new("/f/.t")).unwrap();
        fs.rename(Path::new("/f/.t"), Path::new("/f/a.xmp"))
            .unwrap();
        fs.power_loss();
        assert_eq!(fs.peek("/f/a.xmp").unwrap(), b"old");
    }
}

/// Oracle (SPEC §11.1): exiftool reads what Blink wrote, and sees the same foreign XMP as before.
#[cfg(target_os = "linux")]
mod full_oracle {
    use super::*;
    use blink_runtime::fs::RealFs;
    use std::process::Command;

    /// `-args` lines of every XMP tag exiftool finds, e.g. `-XMP-dc:Subject=beach`.
    fn exif_args(p: &Path) -> Vec<String> {
        let out = Command::new("exiftool")
            .args(["-args", "-n", "-G1", "-XMP:all"])
            .arg(p)
            .output()
            .expect("exiftool is installed (oracle tier)");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn owned_line(l: &str) -> bool {
        [
            "-XMP-xmp:Rating=",
            "-XMP-xmp:Label=",
            "-XMP-xmp:MetadataDate=",
        ]
        .iter()
        .any(|p| l.starts_with(p))
    }

    #[test]
    fn req_file_5_req_file_9_req_rate_1_exiftool_reads_written_sidecars() {
        let dir = std::env::temp_dir().join(format!("blink-oracle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fixtures = [
            ("lr", Some(sidecars::lightroom(4, Some("Red")))),
            ("br", Some(sidecars::bridge(5, Some("Green")))),
            ("dt", Some(sidecars::darktable(2))),
            ("dk", Some(sidecars::digikam(3, Some("Purple")))),
            ("xap", Some(sidecars::xap_prefix(1))),
            ("new", None),
        ];
        let changes: [(&str, Vec<MarkChange>, &str, &str); 3] = [
            ("a", vec![MarkChange::Rating(Some(-1))], "-1", ""),
            (
                "b",
                vec![
                    MarkChange::Rating(Some(5)),
                    MarkChange::Label(Some("Blue".into())),
                ],
                "5",
                "Blue",
            ),
            (
                "c",
                vec![MarkChange::Label(None), MarkChange::Rating(Some(0))],
                "0",
                "",
            ),
        ];
        let clock = FixedClock(NOW);
        for (name, body) in &fixtures {
            for (tag, list, rating, label) in &changes {
                let src = dir.join(format!("{name}_{tag}.ARW"));
                std::fs::write(&src, b"raw").unwrap();
                let side = src.with_extension("xmp");
                let before = body.as_ref().map(|b| {
                    std::fs::write(&side, b).unwrap();
                    exif_args(&side)
                });
                let mut w = SidecarWriter::new(&RealFs, &clock, SidecarStyle::Adobe);
                for c in list {
                    w.queue(src.clone(), c.clone(), true, Instant::now());
                }
                for r in w.flush_all() {
                    r.result.unwrap();
                }
                let after = exif_args(&side);
                let ctx = format!("{name}_{tag}: {after:?}");
                assert!(
                    after.contains(&format!("-XMP-xmp:Rating={rating}")),
                    "{ctx}"
                );
                let label_line =
                    |v: &[String]| v.iter().find(|l| l.starts_with("-XMP-xmp:Label=")).cloned();
                let touches_label = list.iter().any(|c| matches!(c, MarkChange::Label(_)));
                if !touches_label {
                    let was = before.as_deref().and_then(label_line);
                    assert_eq!(label_line(&after), was, "{ctx}");
                } else if label.is_empty() {
                    assert_eq!(label_line(&after), None, "{ctx}");
                } else {
                    assert!(after.contains(&format!("-XMP-xmp:Label={label}")), "{ctx}");
                }
                assert!(
                    after.contains(&"-XMP-xmp:MetadataDate=2026:11:21 09:05:00+00:00".to_string()),
                    "{ctx}"
                );
                if let Some(before) = before {
                    let keep = |v: &[String]| -> Vec<String> {
                        v.iter().filter(|l| !owned_line(l)).cloned().collect()
                    };
                    assert_eq!(
                        keep(&before),
                        keep(&after),
                        "{name}_{tag}: foreign XMP changed"
                    );
                }
                assert_eq!(std::fs::read(&src).unwrap(), b"raw");
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn req_file_6_real_fs_atomic_write_replaces_and_leaves_no_temp_file() {
        let dir = std::env::temp_dir().join(format!("blink-atomic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let t = dir.join("a.xmp");
        std::fs::write(&t, b"old").unwrap();
        blink_runtime::atomic::write_atomic(&RealFs, &t, b"new").unwrap();
        assert_eq!(std::fs::read(&t).unwrap(), b"new");
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["a.xmp"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[test]
fn req_file_5_another_apps_later_changes_are_never_restored_over() {
    let clock = FixedClock(NOW);
    // A: Lightroom sidecar rated 2; Blink rates 4; another app adds a keyword; Blink rates 2.
    let fs = MemFs::new();
    fs.add_file("/f/a.ARW", b"raw".to_vec());
    fs.add_file("/f/a.xmp", sidecars::lightroom(2, None));
    let mut w = SidecarWriter::new(&fs, &clock, SidecarStyle::Adobe);
    let rate = |w: &mut SidecarWriter, r| {
        w.queue(
            "/f/a.ARW".into(),
            MarkChange::Rating(r),
            true,
            Instant::now(),
        );
        for rep in w.flush_all() {
            rep.result.unwrap();
        }
    };
    rate(&mut w, Some(4));
    let edited = String::from_utf8(fs.peek("/f/a.xmp").unwrap())
        .unwrap()
        .replace(
            "<rdf:li>beach</rdf:li>",
            "<rdf:li>beach</rdf:li><rdf:li>sunset</rdf:li>",
        );
    fs.add_file("/f/a.xmp", edited);
    rate(&mut w, Some(2));
    let t = String::from_utf8(fs.peek("/f/a.xmp").unwrap()).unwrap();
    assert!(t.contains("sunset"), "{t}");
    assert_eq!(owned(&fs, "/f/a.xmp"), r(2));
    // B: Blink creates a sidecar; another app adds to it; an undo back to "no rating" must not
    // delete it.
    let fs = MemFs::new();
    fs.add_file("/f/b.ARW", b"raw".to_vec());
    let mut w = SidecarWriter::new(&fs, &clock, SidecarStyle::Adobe);
    w.queue(
        "/f/b.ARW".into(),
        MarkChange::Rating(Some(3)),
        true,
        Instant::now(),
    );
    w.flush_all();
    let added = String::from_utf8(fs.peek("/f/b.xmp").unwrap())
        .unwrap()
        .replace(
            "xmlns:xmp=",
            "xmlns:dc=\"http://purl.org/dc/elements/1.1/\" dc:format=\"x\" xmlns:xmp=",
        );
    fs.add_file("/f/b.xmp", added);
    w.queue(
        "/f/b.ARW".into(),
        MarkChange::Rating(None),
        true,
        Instant::now(),
    );
    w.flush_all();
    let t = String::from_utf8(fs.peek("/f/b.xmp").expect("kept")).unwrap();
    assert!(t.contains("dc:format=\"x\""), "{t}");
    assert_eq!(read_owned(t.as_bytes()).unwrap().rating, None);
}

#[test]
fn req_file_6_failed_writes_back_off() {
    let fs = MemFs::new();
    fs.add_file("/f/a.ARW", b"raw".to_vec());
    fs.add_file("/f/a.xmp", b"<broken".to_vec());
    let clock = FixedClock(NOW);
    let mut w = SidecarWriter::new(&fs, &clock, SidecarStyle::Adobe);
    let t0 = Instant::now();
    w.queue("/f/a.ARW".into(), MarkChange::Rating(Some(1)), true, t0);
    let mut at = t0 + DEBOUNCE;
    let mut gaps = Vec::new();
    for _ in 0..5 {
        assert!(w.flush_due(at)[0].result.is_err());
        let next = w.next_due().unwrap();
        gaps.push(next - at);
        at = next;
    }
    assert!(gaps.windows(2).all(|g| g[1] > g[0]), "{gaps:?}");
    assert!(w.flush_due(at - Duration::from_millis(1)).is_empty());
}
