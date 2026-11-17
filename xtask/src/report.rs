//! `cargo xtask report Mn`: generate `docs/reports/Mn.md` (SPEC §13) from trace coverage,
//! CI runs, perf results, BLOCKED.md, ADRs and the pre-release. The agent then reviews it.

use crate::{Result, capture, root, spec, trace};
use std::fmt::Write as _;

pub fn run(milestone: &str) -> Result {
    let spec = spec::load_spec()?;
    let ids = spec
        .milestones
        .get(milestone)
        .ok_or_else(|| format!("unknown milestone {milestone}"))?;
    let cov = trace::coverage(Some(milestone))?;
    let mut md = String::new();
    let w = &mut md;
    let _ = writeln!(w, "# Gate report {milestone}\n");
    let date = capture("git", &["log", "-1", "--format=%cs"]).unwrap_or_default();
    let commit = capture("git", &["rev-parse", "--short", "HEAD"]).unwrap_or_default();
    let _ = writeln!(w, "Date: {} · Commit: `{}`\n", date.trim(), commit.trim());

    let _ = writeln!(w, "## IDs closed\n");
    if ids.is_empty() {
        let _ = writeln!(w, "None: {milestone} is infrastructure only.\n");
    } else {
        let _ = writeln!(w, "| ID | Passing tests (last full run) |\n|---|---|");
        for id in ids {
            let (src, pass, _) = cov.per_id.get(id).cloned().unwrap_or_default();
            let names = if pass.is_empty() { src } else { pass };
            let list = if names.is_empty() {
                "**MISSING**".to_string()
            } else {
                names
                    .iter()
                    .map(|n| format!("`{n}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let _ = writeln!(w, "| {id} | {list} |");
        }
        let _ = writeln!(w);
    }

    let _ = writeln!(w, "## CI runs\n");
    match capture(
        "gh",
        &[
            "run",
            "list",
            "--branch",
            "main",
            "--limit",
            "8",
            "--json",
            "workflowName,conclusion,url,headSha,createdAt",
            "--template",
            "{{range .}}| {{.workflowName}} | {{.conclusion}} | {{.headSha}} | {{.url}} |\n{{end}}",
        ],
    ) {
        Ok(rows) => {
            let _ = writeln!(
                w,
                "| Workflow | Result | Commit | Link |\n|---|---|---|---|\n{rows}"
            );
        }
        Err(e) => {
            let _ = writeln!(w, "(could not query runs: {e})\n");
        }
    }

    let _ = writeln!(w, "## Performance\n");
    let perf_ids: Vec<_> = ids.iter().filter(|i| i.starts_with("PERF-")).collect();
    let results = root().join("target").join("bench").join("results.md");
    if let Ok(t) = std::fs::read_to_string(&results) {
        let _ = writeln!(w, "{t}\n");
    } else if perf_ids.is_empty() {
        let _ = writeln!(w, "No PERF IDs in {milestone}.\n");
    } else {
        let _ = writeln!(w, "**No bench results found** for {perf_ids:?}.\n");
    }

    let _ = writeln!(w, "## Screenshots\n");
    let _ = writeln!(
        w,
        "(UI screenshot goldens are listed here once UI tests exist.)\n"
    );

    let _ = writeln!(w, "## Open BLOCKED.md items\n");
    let blocked = std::fs::read_to_string(root().join("BLOCKED.md")).unwrap_or_default();
    let items: Vec<_> = blocked.lines().filter(|l| l.starts_with("## ")).collect();
    if items.is_empty() {
        let _ = writeln!(w, "None.\n");
    } else {
        for i in items {
            let _ = writeln!(w, "- {}", i.trim_start_matches("## "));
        }
        let _ = writeln!(w);
    }

    let _ = writeln!(w, "## ADRs\n");
    let mut adrs: Vec<_> = std::fs::read_dir(root().join("docs").join("adr"))
        .map(|r| {
            r.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    adrs.sort();
    for a in adrs {
        let _ = writeln!(w, "- [{a}](../adr/{a})");
    }
    let _ = writeln!(w);

    let _ = writeln!(w, "## Pre-release\n");
    let tag = if milestone == "M0" {
        "v0.0.1-m0".to_string()
    } else {
        format!("v1.0.0-{}", milestone.to_ascii_lowercase())
    };
    match capture(
        "gh",
        &[
            "release",
            "view",
            &tag,
            "--json",
            "url,assets",
            "--template",
            "{{.url}}\n{{range .assets}}- [{{.name}}]({{.url}})\n{{end}}",
        ],
    ) {
        Ok(t) => {
            let _ = writeln!(w, "{t}");
        }
        Err(_) => {
            let _ = writeln!(w, "Release `{tag}` not found yet.\n");
        }
    }

    let dir = root().join("docs").join("reports");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{milestone}.md"));
    std::fs::write(&path, md).map_err(|e| e.to_string())?;
    println!("wrote {}", path.display());
    Ok(())
}
