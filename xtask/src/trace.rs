//! Requirement traceability (SPEC §11.2).
//!
//! Required IDs = every ID of milestones before the current one, every ID ticked in
//! PROGRESS.md, and (with `--gate Mn`) every ID of milestone `Mn`.
//!
//! - Fast mode: each required ID needs a tagged test in the source tree, and no tagged
//!   test that ran (per the JUnit report) may have failed.
//! - Full mode: additionally each required ID needs at least one *passing* tagged test
//!   in the JUnit report of the last nextest run.

use crate::spec::{self, ids_in_test_name};
use crate::{Result, root};
use quick_xml::Reader;
use quick_xml::events::Event;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Fast,
    Full,
}

#[derive(Debug, Default, Clone)]
pub struct TestResult {
    pub name: String,
    pub passed: bool,
}

/// Parse nextest JUnit XML into (full test name, passed) pairs.
pub fn parse_junit(xml: &str) -> Result<Vec<TestResult>> {
    let mut reader = Reader::from_str(xml);
    let mut out = Vec::new();
    let mut current: Option<TestResult> = None;
    loop {
        match reader.read_event().map_err(|e| format!("junit: {e}"))? {
            Event::Start(e) | Event::Empty(e) if e.name().as_ref() == "testcase" => {
                if let Some(t) = current.take() {
                    out.push(t);
                }
                let mut name = String::new();
                let mut class = String::new();
                for a in e.attributes().flatten() {
                    let v = a
                        .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .map_err(|e| e.to_string())?
                        .into_owned();
                    match a.key.as_ref() {
                        "name" => name = v,
                        "classname" => class = v,
                        _ => {}
                    }
                }
                current = Some(TestResult {
                    name: format!("{class}::{name}"),
                    passed: true,
                });
            }
            Event::Start(e) | Event::Empty(e)
                if matches!(e.name().as_ref(), "failure" | "error" | "skipped") =>
            {
                if let Some(t) = current.as_mut() {
                    t.passed = false;
                }
            }
            Event::End(e) if e.name().as_ref() == "testcase" => {
                if let Some(t) = current.take() {
                    out.push(t);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if let Some(t) = current.take() {
        out.push(t);
    }
    Ok(out)
}

/// Latest JUnit report written by nextest (any profile), if one exists.
fn latest_junit() -> Option<PathBuf> {
    let dir = root().join("target").join("nextest");
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path().join("junit.xml"))
        .filter(|p| p.exists())
        .max_by_key(|p| p.metadata().and_then(|m| m.modified()).ok())
}

/// Every `fn req_*` test function name in Rust sources under `dir`.
fn source_test_names(dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name();
        if p.is_dir() {
            if name != "target" && !name.to_string_lossy().starts_with('.') {
                source_test_names(&p, out);
            }
        } else if p.extension().is_some_and(|x| x == "rs")
            && let Ok(text) = std::fs::read_to_string(&p)
        {
            for line in text.lines() {
                if let Some(pos) = line.find("fn req_") {
                    let n: String = line[pos + 3..]
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                        .collect();
                    out.push(n);
                }
            }
        }
    }
}

/// Per ID: (tagged test names in source, passing names in JUnit, failing names).
pub type IdTests = (Vec<String>, Vec<String>, Vec<String>);

pub struct Coverage {
    /// ID → (tagged test names in source, passing test names in JUnit, failing names)
    pub per_id: BTreeMap<String, IdTests>,
    pub required: BTreeSet<String>,
}

pub fn coverage(gate: Option<&str>) -> Result<Coverage> {
    let spec = spec::load_spec()?;
    let progress = spec::load_progress()?;
    let cur = spec::ordinal(&progress.current);
    let mut required: BTreeSet<String> = progress.ticked.iter().cloned().collect();
    for (m, ids) in &spec.milestones {
        if spec::ordinal(m) < cur || Some(m.as_str()) == gate {
            required.extend(ids.iter().cloned());
        }
    }
    if let Some(g) = gate
        && !spec.milestones.contains_key(g)
    {
        return Err(format!("unknown milestone {g}"));
    }

    let mut per_id: BTreeMap<String, IdTests> = spec
        .ids
        .iter()
        .map(|id| (id.clone(), Default::default()))
        .collect();

    let mut names = Vec::new();
    for d in ["crates", "xtask", "tests"] {
        source_test_names(&root().join(d), &mut names);
    }
    for n in names {
        for id in ids_in_test_name(&n) {
            if let Some(e) = per_id.get_mut(&id) {
                e.0.push(n.clone());
            }
        }
    }
    if let Some(j) = latest_junit() {
        let xml = std::fs::read_to_string(&j).map_err(|e| e.to_string())?;
        for t in parse_junit(&xml)? {
            for id in ids_in_test_name(&t.name) {
                if let Some(e) = per_id.get_mut(&id) {
                    if t.passed {
                        e.1.push(t.name.clone());
                    } else {
                        e.2.push(t.name.clone());
                    }
                }
            }
        }
    }
    Ok(Coverage { per_id, required })
}

pub fn run(mode: Mode, gate: Option<&str>) -> Result {
    let cov = coverage(gate)?;
    let mut problems = Vec::new();
    let mut covered = 0;
    for (id, (src, pass, fail)) in &cov.per_id {
        let req = cov.required.contains(id);
        let ok = !pass.is_empty() && fail.is_empty();
        if ok {
            covered += 1;
        }
        let status = if ok {
            "PASS"
        } else if !fail.is_empty() {
            "FAIL"
        } else if !src.is_empty() {
            "NOT RUN"
        } else {
            "—"
        };
        println!(
            "{}{id:<8} {status:<8} {}",
            if req { "*" } else { " " },
            pass.first()
                .or(src.first())
                .map(String::as_str)
                .unwrap_or("")
        );
        if !fail.is_empty() {
            problems.push(format!("{id}: failing tests {fail:?}"));
        }
        if req && src.is_empty() {
            problems.push(format!(
                "{id}: required but no `fn {}*` test",
                spec::tag(id)
            ));
        }
        if req && mode == Mode::Full && pass.is_empty() && !src.is_empty() {
            problems.push(format!(
                "{id}: required but no passing test in the last run"
            ));
        }
    }
    println!(
        "trace: {covered}/{} IDs with passing tests; {} required (* = required)",
        cov.per_id.len(),
        cov.required.len()
    );
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!("trace failed:\n  {}", problems.join("\n  ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn junit_parsing() {
        let xml = r#"<testsuites><testsuite name="x">
            <testcase name="tests::req_open_1_a" classname="blink-core"/>
            <testcase name="tests::req_open_2_b" classname="blink-core"><failure message="x"/></testcase>
            <testcase name="tests::other" classname="blink-core"></testcase>
        </testsuite></testsuites>"#;
        let r = parse_junit(xml).unwrap();
        assert_eq!(r.len(), 3);
        assert!(r[0].passed && r[0].name == "blink-core::tests::req_open_1_a");
        assert!(!r[1].passed);
        assert!(r[2].passed);
    }
}
