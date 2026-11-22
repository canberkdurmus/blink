//! Safety log summary (M4 gate, SPEC §13): `verify --full` runs the suite with
//! `BLINK_SAFETY_LOG` set; both file systems log every write call (`blink_runtime::safety`).
//! This sums the logs up, writes `target/safety-log.md`, and fails on any source-file write.

use crate::{Result, root};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub fn dir() -> PathBuf {
    root().join("target").join("safety-log")
}

pub fn summary_path() -> PathBuf {
    root().join("target").join("safety-log.md")
}

/// Same rule as `blink_runtime::safety::is_violation`.
fn is_violation(op: &str, class: &str, ok: bool) -> bool {
    ok && class == "source" && matches!(op, "rename" | "remove_file")
}

#[derive(Debug, Default, PartialEq)]
pub struct Summary {
    /// (op, class, ok) → count.
    pub counts: BTreeMap<(String, String, bool), usize>,
    pub violations: Vec<String>,
    pub files: usize,
}

pub fn parse(logs: &[String]) -> Summary {
    let mut s = Summary {
        files: logs.len(),
        ..Summary::default()
    };
    for line in logs.iter().flat_map(|l| l.lines()) {
        let mut f = line.splitn(4, '\t');
        let (Some(op), Some(class), Some(res), Some(path)) =
            (f.next(), f.next(), f.next(), f.next())
        else {
            continue;
        };
        let ok = res == "ok";
        *s.counts
            .entry((op.to_string(), class.to_string(), ok))
            .or_default() += 1;
        if is_violation(op, class, ok) {
            s.violations.push(format!("{op} {path}"));
        }
    }
    s
}

pub fn markdown(s: &Summary) -> String {
    let mut md = String::new();
    let total: usize = s.counts.values().sum();
    let _ = writeln!(
        md,
        "Safety log: {total} write calls from {} test processes; **{} source-file writes**.\n",
        s.files,
        s.violations.len()
    );
    let _ = writeln!(md, "| Call | Target | Result | Count |\n|---|---|---|---|");
    for ((op, class, ok), n) in &s.counts {
        let res = if *ok { "ok" } else { "refused/failed" };
        let _ = writeln!(md, "| `{op}` | {class} | {res} | {n} |");
    }
    for v in &s.violations {
        let _ = writeln!(md, "\n- VIOLATION: {v}");
    }
    md
}

fn read_logs(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if let Ok(t) = std::fs::read_to_string(e.path()) {
                out.push(t);
            }
        }
    }
    out
}

/// Clear the log directory before a run.
pub fn reset() -> Result {
    let d = dir();
    if d.exists() {
        std::fs::remove_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    std::fs::create_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()))
}

/// Summarise the run's logs; error on any source-file write.
pub fn check() -> Result {
    let s = parse(&read_logs(&dir()));
    let md = markdown(&s);
    std::fs::write(summary_path(), &md).map_err(|e| e.to_string())?;
    eprint!("{md}");
    if s.files == 0 {
        return Err("safety log: no test process logged anything".into());
    }
    if s.violations.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "safety log: {} source-file writes",
            s.violations.len()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safety_log_counts_and_flags_source_writes() {
        let a = "rename\tsidecar\tok\t/f/a.xmp\nmove_file\tsource\tok\t/f/a.ARW\n\
                 remove_file\tsource\terr\t/f/a.ARW\n"
            .to_string();
        let s = parse(std::slice::from_ref(&a));
        assert!(s.violations.is_empty());
        assert_eq!(s.counts[&("rename".into(), "sidecar".into(), true)], 1);
        let b = "rename\tsource\tok\t/f/a.ARW\nwrite_new\tsource\tok\t/f/new\n".to_string();
        let s = parse(&[a, b]);
        assert_eq!(s.violations, ["rename /f/a.ARW"]);
        assert_eq!(s.files, 2);
        assert!(markdown(&s).contains("**1 source-file writes**"));
    }
}
