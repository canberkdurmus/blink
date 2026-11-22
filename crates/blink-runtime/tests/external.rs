//! FILE-8: sidecars changed on disk by other apps are reloaded; when Blink also had unsaved
//! changes, the on-disk version wins and the change is reported as a conflict.

use blink_core::state::MarkChange;
use blink_runtime::fs::{Fs, MemFs, RealFs};
use blink_runtime::sidecar::{
    FixedClock, SidecarEvent, SidecarService, SidecarStyle, SidecarWriter,
};
use blink_testkit::sidecars;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const NOW: i64 = 1_795_251_900;

fn rating(fs: &dyn Fs, p: &str) -> Option<i8> {
    blink_xmp::read_owned(&fs.read(Path::new(p)).unwrap())
        .unwrap()
        .rating
}

#[test]
fn req_file_8_own_writes_are_not_external_changes() {
    let fs = MemFs::new();
    fs.add_file("/f/a.ARW", b"raw".to_vec());
    let clock = FixedClock(NOW);
    let mut w = SidecarWriter::new(&fs, &clock, SidecarStyle::Adobe);
    w.queue(
        "/f/a.ARW".into(),
        MarkChange::Rating(Some(3)),
        true,
        Instant::now(),
    );
    w.flush_all();
    assert_eq!(w.external_change(Path::new("/f/a.xmp")), None);
    assert_eq!(w.external_change(Path::new("/f/a.ARW")), None);
}

#[test]
fn req_file_8_external_change_with_unsaved_changes_keeps_the_disk_version() {
    let fs = MemFs::new();
    fs.add_file("/f/a.ARW", b"raw".to_vec());
    fs.add_file("/f/a.JPG", vec![0xFF, 0xD8, 0xFF, 0xD9]);
    fs.add_file("/f/a.xmp", sidecars::lightroom(1, None));
    let clock = FixedClock(NOW);
    let mut w = SidecarWriter::new(&fs, &clock, SidecarStyle::Adobe);
    let t0 = Instant::now();
    w.queue("/f/a.ARW".into(), MarkChange::Rating(Some(5)), true, t0);
    w.queue("/f/a.JPG".into(), MarkChange::Rating(Some(5)), false, t0);
    // Lightroom saves a.xmp before the debounce fires.
    fs.add_file("/f/a.xmp", sidecars::lightroom(2, Some("Red")));
    let c = w.external_change(Path::new("/f/a.xmp")).unwrap();
    assert!(c.conflict());
    assert_eq!(c.dropped, [PathBuf::from("/f/a.ARW")]);
    w.flush_all();
    assert_eq!(rating(&fs, "/f/a.xmp"), Some(2));
    // The companion's own change wasn't affected.
    assert_eq!(rating(&fs, "/f/a.JPG.xmp"), Some(5));
    // Without unsaved changes it's a plain reload.
    fs.add_file("/f/a.xmp", sidecars::lightroom(3, None));
    let c = w.external_change(Path::new("/f/a.xmp")).unwrap();
    assert!(!c.conflict());
    // The new disk version is the reference: a later restore brings it back, not the
    // session-start bytes.
    w.queue(
        "/f/a.ARW".into(),
        MarkChange::Rating(Some(4)),
        true,
        Instant::now(),
    );
    w.flush_all();
    w.queue(
        "/f/a.ARW".into(),
        MarkChange::Rating(Some(3)),
        true,
        Instant::now(),
    );
    w.flush_all();
    assert_eq!(
        fs.peek("/f/a.xmp").unwrap(),
        sidecars::lightroom(3, None).as_bytes()
    );
}

#[test]
fn req_file_8_a_change_on_disk_before_the_debounced_write_is_never_overwritten() {
    // The other app saves between Blink's key press and its write, and the watcher event
    // hasn't arrived yet: the write itself notices and keeps the disk version.
    let fs = MemFs::new();
    fs.add_file("/f/a.ARW", b"raw".to_vec());
    fs.add_file("/f/b.ARW", b"raw".to_vec());
    fs.add_file("/f/a.xmp", sidecars::lightroom(1, None));
    let clock = FixedClock(NOW);
    let mut w = SidecarWriter::new(&fs, &clock, SidecarStyle::Adobe);
    let t0 = Instant::now();
    w.queue("/f/a.ARW".into(), MarkChange::Rating(Some(5)), true, t0);
    w.queue("/f/b.ARW".into(), MarkChange::Rating(Some(4)), true, t0);
    fs.add_file("/f/a.xmp", sidecars::lightroom(2, Some("Red")));
    // darktable creates b's darktable-style sidecar meanwhile: also a change on disk.
    fs.add_file("/f/b.ARW.xmp", sidecars::darktable(1));
    for r in w.flush_all() {
        r.result.unwrap();
    }
    assert_eq!(
        fs.peek("/f/a.xmp").unwrap(),
        sidecars::lightroom(2, Some("Red")).as_bytes()
    );
    assert_eq!(rating(&fs, "/f/b.ARW.xmp"), Some(1));
    assert_eq!(fs.peek("/f/b.xmp"), None);
    let ext = w.take_external();
    assert_eq!(
        ext.iter()
            .map(|c| (c.sidecar.clone(), c.dropped.clone()))
            .collect::<Vec<_>>(),
        [
            (PathBuf::from("/f/a.xmp"), vec![PathBuf::from("/f/a.ARW")]),
            (
                PathBuf::from("/f/b.ARW.xmp"),
                vec![PathBuf::from("/f/b.ARW")]
            ),
        ]
    );
    assert!(ext.iter().all(|c| c.conflict()));
    // The later watcher event for the same change is not reported twice.
    assert_eq!(w.external_change(Path::new("/f/a.xmp")), None);
    // Nothing changed meanwhile: writes go through as usual.
    w.queue(
        "/f/a.ARW".into(),
        MarkChange::Rating(Some(3)),
        true,
        Instant::now(),
    );
    w.flush_all();
    assert_eq!(rating(&fs, "/f/a.xmp"), Some(3));
    assert!(w.take_external().is_empty());
}

#[test]
fn req_file_8_a_write_whose_directory_fsync_failed_is_still_blinks_own() {
    // The rename happened, then the directory fsync failed: the bytes on disk are Blink's. The
    // retry must write, not report another app's change.
    let fs = MemFs::new();
    fs.add_file("/f/a.ARW", b"raw".to_vec());
    let clock = FixedClock(NOW);
    let mut w = SidecarWriter::new(&fs, &clock, SidecarStyle::Adobe);
    w.queue(
        "/f/a.ARW".into(),
        MarkChange::Rating(Some(3)),
        true,
        Instant::now(),
    );
    // write_new, sync_file, rename, sync_dir: fail the fourth.
    fs.fail_at_write(3);
    let reps = w.flush_all();
    assert!(reps[0].result.is_err(), "{reps:?}");
    assert_eq!(rating(&fs, "/f/a.xmp"), Some(3));
    fs.clear_fault();
    w.queue(
        "/f/a.ARW".into(),
        MarkChange::Rating(Some(4)),
        true,
        Instant::now(),
    );
    for r in w.flush_all() {
        r.result.unwrap();
    }
    assert_eq!(rating(&fs, "/f/a.xmp"), Some(4));
    assert!(w.take_external().is_empty());
    assert_eq!(w.external_change(Path::new("/f/a.xmp")), None);
    // The same holds for a watcher event naming the sidecar in another case.
    assert_eq!(w.external_change(Path::new("/f/A.XMP")), None);
}

mod full_watch {
    use super::*;

    #[test]
    fn req_file_8_the_folder_watcher_reports_a_sidecar_another_app_writes() {
        let dir = std::env::temp_dir().join(format!("blink-file8-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.ARW"), b"raw").unwrap();
        let events: Arc<Mutex<Vec<SidecarEvent>>> = Arc::default();
        let ev = events.clone();
        let svc = SidecarService::spawn(
            Arc::new(RealFs),
            Arc::new(FixedClock(NOW)),
            SidecarStyle::Adobe,
            move |e| ev.lock().unwrap().push(e),
        );
        svc.watch(dir.clone(), false);
        svc.flush();
        // Blink's own write is not reported as external.
        svc.queue(dir.join("a.ARW"), MarkChange::Rating(Some(2)), true);
        svc.flush();
        std::thread::sleep(Duration::from_millis(300));
        svc.flush();
        let external = |events: &Arc<Mutex<Vec<SidecarEvent>>>| {
            events
                .lock()
                .unwrap()
                .iter()
                .filter_map(|e| match e {
                    SidecarEvent::External(c) => Some(c.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert!(external(&events).is_empty(), "{:?}", events.lock().unwrap());
        // Another app rewrites the sidecar (atomically, like Blink and darktable do).
        let tmp = dir.join("a.xmp.other-app");
        std::fs::write(&tmp, sidecars::lightroom(5, Some("Blue"))).unwrap();
        std::fs::rename(&tmp, dir.join("a.xmp")).unwrap();
        let t = Instant::now();
        while external(&events).is_empty() {
            assert!(
                t.elapsed() < Duration::from_secs(10),
                "no external change reported"
            );
            std::thread::sleep(Duration::from_millis(20));
            svc.flush();
        }
        let c = &external(&events)[0];
        // The full path as opened, also where the OS reports real paths (macOS `/private/var`).
        assert_eq!(c.sidecar, dir.join("a.xmp"));
        assert!(!c.conflict());
        drop(svc);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
