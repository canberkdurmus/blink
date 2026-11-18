//! Golden-file helpers. Regenerate goldens with `BLINK_UPDATE_SNAPSHOTS=1` and commit them with
//! a `golden-update:` trailer.

use std::path::Path;

/// Compare `got` with the golden file at `path` (CRLF-insensitive). With `BLINK_UPDATE_SNAPSHOTS`
/// set, write `got` first.
pub fn check(path: &Path, got: &str) {
    if std::env::var_os("BLINK_UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(path.parent().expect("golden path has a parent")).unwrap();
        std::fs::write(path, got).unwrap();
    }
    let want = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .replace("\r\n", "\n");
    assert_eq!(got, want, "golden {} differs", path.display());
}

/// Replace `<number> ms` timings with `<t> ms` so goldens are stable.
pub fn mask_timings(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for line in s.lines() {
        let mut masked = String::new();
        let toks: Vec<&str> = line.split(' ').collect();
        for (i, t) in toks.iter().enumerate() {
            let next_ms = toks.get(i + 1).is_some_and(|n| n.starts_with("ms"));
            let trimmed = t.trim_start_matches('(');
            let is_num =
                !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_digit() || c == '.');
            if i > 0 {
                masked.push(' ');
            }
            if next_ms && is_num {
                masked.push_str(&t[..t.len() - trimmed.len()]);
                masked.push_str("<t>");
            } else {
                masked.push_str(t);
            }
        }
        out.push_str(&masked);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timings_are_masked() {
        assert_eq!(
            mask_timings("timing: read 12.4 ms, analyze 356.4 ms\nok 8 × 5 (448.3 ms)"),
            "timing: read <t> ms, analyze <t> ms\nok 8 × 5 (<t> ms)\n"
        );
        assert_eq!(mask_timings("PERF-4 ms budget"), "PERF-4 ms budget\n");
    }
}
