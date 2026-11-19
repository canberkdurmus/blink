//! PERF-4: embedded preview extract + decode, per file (SPEC §7). The file is handled by one
//! worker; the decode of a large baseline JPEG fans out over the CPU's cores (ADR 0011).
//!
//! The test checks the work is done correctly and records the median of 11 runs to
//! `$BLINK_PERF_DIR/PERF-4.json` (default `target/perf/`). `cargo xtask bench` runs it in release
//! mode and compares the median with `perf/baseline-<target>.json` (> 10 % slower fails) and
//! reports it against the absolute budget.

use blink_decode::{embedded, image};
use blink_testkit::fixtures::{perf_jpeg_6mb, tiff_with_jpegs};
use std::time::Instant;

const BUDGET_MS: f64 = 25.0;

#[test]
fn req_perf_4_embedded_preview_extract_and_decode_6mb_jpeg() {
    let big = perf_jpeg_6mb();
    assert!((5_500_000..6_500_000).contains(&big.len()), "{}", big.len());
    let thumb = blink_testkit::fixtures::jpeg(&[128; 160 * 120 * 3], 160, 120, 80, None);
    let file = tiff_with_jpegs(&[thumb, big.clone()]);
    // The timed path is the parallel decoder, not the zune-jpeg fallback.
    assert!(image::decode_jpeg_parallel(&big, 2).is_some());

    let mut runs = Vec::new();
    for _ in 0..11 {
        let t = Instant::now();
        let found = embedded::find(&file);
        let chosen = embedded::largest(&found).expect("preview found");
        let bytes = chosen.bytes(&file).expect("in bounds");
        let d = image::decode_jpeg(bytes).expect("decodes");
        runs.push(t.elapsed().as_secs_f64() * 1000.0);
        assert_eq!((d.image.width, d.image.height), (6000, 4000));
    }
    runs.sort_by(f64::total_cmp);
    let median = runs[runs.len() / 2];
    let dir = std::env::var_os("BLINK_PERF_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            blink_testkit::corpus::workspace_root()
                .join("target")
                .join("perf")
        });
    std::fs::create_dir_all(&dir).unwrap();
    let json = format!(
        "{{\"id\":\"PERF-4\",\"metric\":\"embedded preview extract + decode, 6 MB JPEG, median of 11\",\"median_ms\":{median:.3},\"budget_ms\":{BUDGET_MS},\"runs_ms\":{runs:?},\"optimized\":{}}}\n",
        !cfg!(debug_assertions)
    );
    std::fs::write(dir.join("PERF-4.json"), json).unwrap();
    eprintln!("PERF-4 median {median:.1} ms (budget {BUDGET_MS} ms)");
}
