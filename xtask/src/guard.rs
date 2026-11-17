//! `cargo xtask guard --base REF`: CI guard checks (SPEC §12).
//!
//! - Changes under `corpus/manifest.toml`, `**/snapshots/**` or `perf/baseline-*` need a commit
//!   in `REF..HEAD` with a `golden-update:` trailer.
//! - Removed tests (net loss of `#[test]` attributes) or added `#[ignore` need a `test-fix:` trailer.

use crate::{Result, capture};

pub fn is_golden(path: &str) -> bool {
    path == "corpus/manifest.toml"
        || path.contains("/snapshots/")
        || path.starts_with("snapshots/")
        || path.starts_with("perf/baseline-")
}

/// (removed `#[test]` count, added `#[test]` count, added `#[ignore` lines) in a unified diff.
pub fn test_changes(diff: &str) -> (usize, usize, usize) {
    let (mut removed, mut added, mut ignored) = (0, 0, 0);
    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        let is_test = |l: &str| {
            let t = l.trim();
            t.starts_with("#[test]") || t.starts_with("#[tokio::test") || t.contains("#[rstest")
        };
        if let Some(l) = line.strip_prefix('-') {
            if is_test(l) {
                removed += 1;
            }
        } else if let Some(l) = line.strip_prefix('+') {
            if is_test(l) {
                added += 1;
            }
            if l.trim_start().starts_with("#[ignore")
                || l.contains("#[cfg_attr(") && l.contains("ignore")
            {
                ignored += 1;
            }
        }
    }
    (removed, added, ignored)
}

pub fn run(base: &str) -> Result {
    let range = format!("{base}..HEAD");
    let messages = capture("git", &["log", "--format=%B", &range])?;
    let has_trailer = |t: &str| messages.lines().any(|l| l.trim_start().starts_with(t));
    let files = capture("git", &["diff", "--name-only", &format!("{base}...HEAD")])?;
    let diff = capture("git", &["diff", &format!("{base}...HEAD"), "--", "*.rs"])?;
    let mut errors = Vec::new();

    let goldens: Vec<&str> = files.lines().filter(|f| is_golden(f)).collect();
    if !goldens.is_empty() && !has_trailer("golden-update:") {
        errors.push(format!(
            "goldens/baselines changed without a `golden-update:` trailer: {goldens:?}"
        ));
    }
    let (removed, added, ignored) = test_changes(&diff);
    if (removed > added || ignored > 0) && !has_trailer("test-fix:") {
        errors.push(format!(
            "tests removed ({removed} removed vs {added} added) or ignored ({ignored}) without a `test-fix:` trailer"
        ));
    }
    if errors.is_empty() {
        println!(
            "guard: ok ({} goldens changed, tests -{removed}/+{added}, {ignored} ignores)",
            goldens.len()
        );
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golden_paths() {
        assert!(is_golden("corpus/manifest.toml"));
        assert!(is_golden("crates/blink-ui/tests/snapshots/loupe.png"));
        assert!(is_golden("perf/baseline-linux-x86_64.json"));
        assert!(!is_golden("crates/blink-core/src/lib.rs"));
    }

    #[test]
    fn counts_test_changes() {
        let diff =
            "--- a/x.rs\n+++ b/x.rs\n-    #[test]\n-    fn a() {}\n+    #[test]\n+    #[ignore]\n";
        assert_eq!(test_changes(diff), (1, 1, 1));
    }
}
