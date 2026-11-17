//! `cargo xtask bench`: run the standard bench scripts in `perf/scripts/` with
//! `blink --bench-script` and compare medians with `perf/baseline-<target>.json`.
//! A regression of more than 10% (median of 5 runs) fails (SPEC §7).

use crate::{Result, root};

/// Target key used for baseline file names, e.g. `linux-x86_64`.
pub fn target_key() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

pub fn run() -> Result {
    let dir = root().join("perf").join("scripts");
    let scripts: Vec<_> = std::fs::read_dir(&dir)
        .map(|r| {
            r.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "json"))
                .collect()
        })
        .unwrap_or_default();
    let baseline = root()
        .join("perf")
        .join(format!("baseline-{}.json", target_key()));
    if scripts.is_empty() {
        println!(
            "bench: no scripts in {} yet (bench scripts arrive with the PERF-* IDs); baseline {} {}",
            dir.display(),
            baseline.display(),
            if baseline.exists() {
                "exists"
            } else {
                "absent"
            }
        );
        return Ok(());
    }
    Err(format!(
        "bench: {} scripts found but the bench runner is not implemented yet",
        scripts.len()
    ))
}
