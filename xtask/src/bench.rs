//! `cargo xtask bench`: run the perf tests (`req_perf_*`) in release mode, collect the JSON
//! results they write to `target/perf/`, compare medians with `perf/baseline-<target>.json`
//! (a regression of more than 10 % fails, SPEC §7) and print them against the absolute budgets.
//! GUI bench scripts (`perf/scripts/*.json`, `blink --bench-script`) join with the M2 PERF IDs.
//!
//! Baseline format: `{ "PERF-4": { "median_ms": 75.0 }, ... }`.

use crate::{Result, root};
use std::collections::BTreeMap;
use std::process::Command;

/// Target key used for baseline file names, e.g. `linux-x86_64`.
pub fn target_key() -> String {
    let os = std::env::consts::OS;
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    };
    format!("{os}-{arch}")
}

#[derive(Debug, Clone, PartialEq)]
pub struct Result1 {
    pub id: String,
    pub median_ms: f64,
    pub budget_ms: f64,
}

/// Pull a number out of a flat JSON object without a JSON dependency.
fn json_num(text: &str, key: &str) -> Option<f64> {
    let pat = format!("\"{key}\":");
    let at = text.find(&pat)? + pat.len();
    let rest = text[at..].trim_start();
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == 'e'))
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn json_str(text: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\":\"");
    let at = text.find(&pat)? + pat.len();
    Some(text[at..].split('"').next()?.to_string())
}

/// Parse the baseline: `{ "ID": { "median_ms": N }, ... }`.
pub fn parse_baseline(text: &str) -> BTreeMap<String, f64> {
    let mut out = BTreeMap::new();
    for part in text.split("\"PERF-").skip(1) {
        let id_end = part.find('"').unwrap_or(0);
        let id = format!("PERF-{}", &part[..id_end]);
        if let Some(v) = json_num(part, "median_ms") {
            out.insert(id, v);
        }
    }
    out
}

/// Compare results with baselines. Returns (table lines, failures).
pub fn compare(
    results: &[Result1],
    baseline: &BTreeMap<String, f64>,
) -> (Vec<String>, Vec<String>) {
    let mut lines = vec![format!(
        "{:<8} {:>10} {:>10} {:>10} {:>8}  {}",
        "ID", "median ms", "budget ms", "baseline", "delta", "status"
    )];
    let mut fails = Vec::new();
    for r in results {
        let (base, delta, status) = match baseline.get(&r.id) {
            Some(&b) => {
                let d = (r.median_ms - b) / b * 100.0;
                let st = if d > 10.0 {
                    fails.push(format!(
                        "{}: {:.1} ms is {d:+.1}% vs baseline {b:.1} ms",
                        r.id, r.median_ms
                    ));
                    "REGRESSION"
                } else {
                    "ok"
                };
                (format!("{b:.1}"), format!("{d:+.1}%"), st)
            }
            None => {
                fails.push(format!(
                    "{}: no baseline for this target; record one with a golden-update commit",
                    r.id
                ));
                ("—".into(), "—".into(), "NO BASELINE")
            }
        };
        let budget = if r.median_ms <= r.budget_ms {
            "within budget"
        } else {
            "OVER BUDGET"
        };
        lines.push(format!(
            "{:<8} {:>10.1} {:>10.1} {:>10} {:>8}  {status}, {budget}",
            r.id, r.median_ms, r.budget_ms, base, delta
        ));
    }
    (lines, fails)
}

pub fn run(allow_missing_baseline: bool) -> Result {
    let out = root().join("target").join("perf");
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(&cargo)
        .args([
            "nextest",
            "run",
            "--release",
            "--workspace",
            "--locked",
            "--no-fail-fast",
            // One test at a time: timings must not compete with other tests for cores.
            "--test-threads",
            "1",
            "-E",
            "test(/req_perf_/)",
        ])
        .env("BLINK_PERF_DIR", &out)
        .current_dir(root())
        .status()
        .map_err(|e| format!("failed to start cargo nextest: {e}"))?;
    if !status.success() {
        return Err("perf tests failed".into());
    }
    let mut results = Vec::new();
    for e in std::fs::read_dir(&out)
        .map_err(|e| e.to_string())?
        .flatten()
    {
        let text = std::fs::read_to_string(e.path()).map_err(|e| e.to_string())?;
        if let (Some(id), Some(m), Some(b)) = (
            json_str(&text, "id"),
            json_num(&text, "median_ms"),
            json_num(&text, "budget_ms"),
        ) {
            results.push(Result1 {
                id,
                median_ms: m,
                budget_ms: b,
            });
        }
    }
    results.sort_by(|a, b| a.id.cmp(&b.id));
    let key = target_key();
    let base_path = root().join("perf").join(format!("baseline-{key}.json"));
    let baseline = std::fs::read_to_string(&base_path)
        .map(|t| parse_baseline(&t))
        .unwrap_or_default();
    let (lines, mut fails) = compare(&results, &baseline);
    if allow_missing_baseline {
        fails.retain(|f| !f.contains("no baseline"));
    }
    println!("bench ({key}, baseline {}):", base_path.display());
    for l in &lines {
        println!("  {l}");
    }
    let mut summary = String::from("{\n");
    for (i, r) in results.iter().enumerate() {
        let comma = if i + 1 < results.len() { "," } else { "" };
        summary.push_str(&format!(
            "  \"{}\": {{ \"median_ms\": {:.3} }}{comma}\n",
            r.id, r.median_ms
        ));
    }
    summary.push_str("}\n");
    std::fs::write(out.join(format!("results-{key}.json")), &summary).map_err(|e| e.to_string())?;
    if fails.is_empty() {
        Ok(())
    } else {
        Err(format!("perf regressions:\n  {}", fails.join("\n  ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_parse_and_compare() {
        let b = parse_baseline(
            "{\n  \"PERF-4\": { \"median_ms\": 50.0 },\n  \"PERF-2\": { \"median_ms\": 10 }\n}",
        );
        assert_eq!(b.get("PERF-4"), Some(&50.0));
        assert_eq!(b.get("PERF-2"), Some(&10.0));
        let r = |m| {
            vec![Result1 {
                id: "PERF-4".into(),
                median_ms: m,
                budget_ms: 25.0,
            }]
        };
        assert!(compare(&r(54.9), &b).1.is_empty());
        assert_eq!(compare(&r(55.1), &b).1.len(), 1);
        assert_eq!(compare(&r(5.0), &BTreeMap::new()).1.len(), 1);
        assert_eq!(
            json_num("{\"median_ms\":75.5,\"x\":1}", "median_ms"),
            Some(75.5)
        );
    }
}
