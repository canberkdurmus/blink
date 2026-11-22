//! GUI performance budgets (SPEC §7) via the bench scripts in `perf/scripts/`. Each test runs
//! its script with the real app headless and writes `$BLINK_PERF_DIR/<ID>.json` (default
//! `target/perf/`); `cargo xtask bench` runs them in release mode and compares with
//! `perf/baseline-<target>.json`. Correctness conditions (no stale frame) are asserted here;
//! timings are compared by the bench. Full tier (`full_perf`): they take seconds and PERF-5
//! needs the corpus.

use blink_testkit::corpus::workspace_root;
use blink_ui::Options;
use blink_ui::bench::{Report, Script, run, write};
use std::path::{Path, PathBuf};

fn script(name: &str) -> Script {
    let p = workspace_root().join("perf").join("scripts").join(name);
    Script::parse(&std::fs::read_to_string(&p).unwrap()).unwrap()
}

fn out_dir() -> PathBuf {
    std::env::var_os("BLINK_PERF_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root().join("target").join("perf"))
}

fn bench(name: &str, folder: &Path) -> Report {
    let s = script(name);
    let r = run(&s, folder, Options::default).unwrap();
    write(&r, &out_dir()).unwrap();
    for m in &r.metrics {
        eprintln!(
            "{} median {:.1} ms (budget {} ms) runs {:?}",
            m.id,
            m.median_ms(),
            m.budget_ms,
            m.runs_ms
        );
    }
    r
}

/// Full tier: slow (seconds of held keys) and PERF-5 needs the corpus.
mod full_perf {
    use super::*;

    #[test]
    fn req_perf_1_launch_to_first_image_with_2000_raws() {
        let r = bench(
            "perf-1-launch.json",
            &blink_testkit::fixtures::perf_folder(2000),
        );
        assert_eq!(r.metrics[0].runs_ms.len(), 5);
    }

    #[test]
    fn req_perf_2_next_item_prefetched_is_drawn_at_once() {
        let r = bench(
            "perf-2-next.json",
            &blink_testkit::fixtures::perf_folder(2000),
        );
        assert_eq!(r.metrics[0].runs_ms.len(), 21);
    }

    #[test]
    fn req_perf_3_held_arrow_key_never_shows_a_stale_item() {
        let r = bench(
            "perf-3-held-key.json",
            &blink_testkit::fixtures::perf_folder(2000),
        );
        assert_eq!(r.stale_frames, 0, "frames drew another item's pixels");
    }

    /// PERF-7 rates items in a writable folder of its own, so no sidecar lands in the shared
    /// PERF-1/2/3 folder.
    #[test]
    fn req_perf_7_rating_key_to_durable_sidecar() {
        let dir = workspace_root()
            .join("target")
            .join("perf-fixtures")
            .join("rate-21");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = blink_testkit::fixtures::perf_folder(2000).join("IMG_0000.ARW");
        // Saved now: the RAWs are hard links to `src`, so reading `src` afterwards would see any
        // write too.
        let raw_bytes = std::fs::read(&src).unwrap();
        for i in 0..21 {
            let dst = dir.join(format!("IMG_{i:04}.ARW"));
            if std::fs::hard_link(&src, &dst).is_err() {
                std::fs::copy(&src, &dst).unwrap();
            }
        }
        let r = bench("perf-7-rate-durable.json", &dir);
        assert_eq!(r.metrics[0].runs_ms.len(), 21);
        // Every run's sidecar is on disk with its rating; the RAWs are untouched (FILE-7).
        for i in 0..21 {
            let x = std::fs::read(dir.join(format!("IMG_{i:04}.xmp"))).unwrap();
            let want = (i % 5 + 1) as i8;
            assert_eq!(blink_xmp::read_owned(&x).unwrap().rating, Some(want));
        }
        for i in 0..21 {
            let raw = std::fs::read(dir.join(format!("IMG_{i:04}.ARW"))).unwrap();
            assert!(raw == raw_bytes, "IMG_{i:04}.ARW changed (FILE-7)");
        }
    }

    /// PERF-5 uses the 50 MP Sony ILCE-1M2 file from the corpus (`cargo xtask corpus fetch`).
    #[test]
    fn req_perf_5_full_raw_decode_45mp_for_100_percent_zoom() {
        let e = blink_testkit::corpus::manifest()
            .files
            .into_iter()
            .find(|e| e.path == "sony/ILCE-1M2.ARW")
            .unwrap();
        assert!(e.expected.sensor_w * e.expected.sensor_h >= 45_000_000);
        // A folder with just this file: it is the first item.
        let dir = workspace_root()
            .join("target")
            .join("perf-fixtures")
            .join("raw-45mp");
        std::fs::create_dir_all(&dir).unwrap();
        let dst = dir.join("ILCE-1M2.ARW");
        if !dst.exists() && std::fs::hard_link(e.file(), &dst).is_err() {
            std::fs::copy(e.file(), &dst).unwrap();
        }
        let r = bench("perf-5-raw-decode.json", &dir);
        assert_eq!(r.metrics.len(), 2);
    }
}
