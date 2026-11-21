//! `cargo xtask bench`: run the perf tests (`req_perf_*`) in release mode, collect the JSON
//! results they write to `target/perf/`, compare medians with `perf/baseline-<target>.json`
//! (a regression of more than 10 % fails, SPEC §7) and print them against the absolute budgets.
//! GUI bench scripts (`perf/scripts/*.json`) run through the `req_perf_*` tests of `blink-ui`
//! (also available as `blink --bench-script <script> <folder>`). Needs `cargo xtask corpus fetch`.
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
    pub runs_ms: Vec<f64>,
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
/// Medians are compared against at least this many ms: below it, scheduler jitter dominates
/// (PERF-2, and the longest-frame metric, a maximum over hundreds of frames).
pub const NOISE_FLOOR_MS: f64 = 2.0;

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
                // Below the 2 ms floor, 10 % is timer and scheduler noise (ADR 0014).
                let floor = b.max(NOISE_FLOOR_MS);
                let st = if r.median_ms > floor * 1.1 {
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
            // Every perf test, including full-tier ones (GUI scripts; PERF-5 needs the
            // corpus, so run `cargo xtask corpus fetch` first).
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
                runs_ms: json_array(&text, "runs_ms"),
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
    let md_dir = root().join("target").join("bench");
    std::fs::create_dir_all(&md_dir).map_err(|e| e.to_string())?;
    std::fs::write(
        md_dir.join("results.md"),
        results_md(&key, &results, &lines),
    )
    .map_err(|e| e.to_string())?;
    if fails.is_empty() {
        Ok(())
    } else {
        Err(format!("perf regressions:\n  {}", fails.join("\n  ")))
    }
}

/// Numbers of a flat JSON array field, e.g. `"runs_ms":[1.0, 2.5]`.
fn json_array(text: &str, key: &str) -> Vec<f64> {
    let pat = format!("\"{key}\":");
    let Some(i) = text.find(&pat) else {
        return Vec::new();
    };
    let rest = text[i + pat.len()..].trim_start();
    let Some(body) = rest.strip_prefix('[').and_then(|r| r.split(']').next()) else {
        return Vec::new();
    };
    body.split(',')
        .filter_map(|n| n.trim().parse().ok())
        .collect()
}

/// Markdown for the gate report (`cargo xtask report` reads `target/bench/results.md`).
fn results_md(key: &str, results: &[Result1], lines: &[String]) -> String {
    let mut md = format!(
        "Target `{key}`, `cargo xtask bench`, release build.\n\n\
         | ID | Median ms | Budget ms | Raw runs ms |\n|---|---|---|---|\n"
    );
    for r in results {
        let runs = r
            .runs_ms
            .iter()
            .map(|v| format!("{v:.1}"))
            .collect::<Vec<_>>()
            .join(", ");
        md.push_str(&format!(
            "| {} | {:.1} | {:.1} | {runs} |\n",
            r.id, r.median_ms, r.budget_ms
        ));
    }
    md.push_str("\nVs CI baseline for this target:\n\n```\n");
    for l in lines {
        md.push_str(l);
        md.push('\n');
    }
    md.push_str("```\n");
    md
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sub_millisecond_medians_compare_against_a_2_ms_floor() {
        let r = |id: &str, m: f64| Result1 {
            id: id.into(),
            median_ms: m,
            budget_ms: 16.0,
            runs_ms: vec![m],
        };
        let base: BTreeMap<String, f64> =
            [("PERF-2".to_string(), 0.5), ("PERF-1".to_string(), 160.0)].into();
        let (_, fails) = compare(&[r("PERF-2", 2.15), r("PERF-1", 175.0)], &base);
        assert!(fails.is_empty(), "{fails:?}");
        let (_, fails) = compare(&[r("PERF-2", 2.3), r("PERF-1", 177.0)], &base);
        assert_eq!(fails.len(), 2, "{fails:?}");
    }

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
                runs_ms: vec![m],
            }]
        };
        assert!(compare(&r(54.9), &b).1.is_empty());
        assert_eq!(compare(&r(55.1), &b).1.len(), 1);
        assert_eq!(compare(&r(5.0), &BTreeMap::new()).1.len(), 1);
        assert_eq!(
            json_num("{\"median_ms\":75.5,\"x\":1}", "median_ms"),
            Some(75.5)
        );
        assert_eq!(
            json_array("{\"runs_ms\":[1.5, 2],\"x\":1}", "runs_ms"),
            vec![1.5, 2.0]
        );
        let md = results_md("k", &r(20.0), &[]);
        assert!(md.contains("| PERF-4 | 20.0 | 25.0 | 20.0 |"), "{md}");
    }
}
