//! Parsing of SPEC.md (requirement IDs, milestones) and PROGRESS.md (ticks, current milestone).

use crate::{Result, root};
use std::collections::BTreeMap;

/// A requirement ID like `OPEN-1` or `A11Y-1`.
pub fn is_id(s: &str) -> bool {
    let Some((prefix, num)) = s.split_once('-') else {
        return false;
    };
    !prefix.is_empty()
        && prefix.starts_with(|c: char| c.is_ascii_uppercase())
        && prefix
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        && !num.is_empty()
        && num.chars().all(|c| c.is_ascii_digit())
}

/// Find every requirement ID token in `text`.
pub fn ids_in(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .filter(|t| is_id(t))
        .map(str::to_string)
        .collect()
}

/// The test-name tag for an ID: `OPEN-1` → `req_open_1`.
pub fn tag(id: &str) -> String {
    format!("req_{}", id.to_ascii_lowercase().replace('-', "_"))
}

/// All IDs that appear in a test name (`req_view_2_req_view_3_foo` → VIEW-2, VIEW-3).
pub fn ids_in_test_name(name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = name;
    while let Some(pos) = rest.find("req_") {
        rest = &rest[pos + 4..];
        let mut parts = rest.splitn(3, '_');
        let (Some(prefix), Some(num)) = (parts.next(), parts.next()) else {
            break;
        };
        let num: String = num.chars().take_while(char::is_ascii_digit).collect();
        let id = format!("{}-{}", prefix.to_ascii_uppercase(), num);
        if is_id(&id) {
            out.push(id);
        }
    }
    out
}

pub struct Spec {
    /// Every requirement ID in §4 and §7, in document order.
    pub ids: Vec<String>,
    /// Milestone name (`M1`) → IDs.
    pub milestones: BTreeMap<String, Vec<String>>,
}

pub fn parse_spec(text: &str) -> Spec {
    let mut ids = Vec::new();
    let mut milestones = BTreeMap::new();
    let mut section = String::new();
    for line in text.lines() {
        if let Some(h) = line.strip_prefix("## ") {
            section = h.split('.').next().unwrap_or("").trim().to_string();
            continue;
        }
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        if cells.len() < 3 {
            continue;
        }
        match section.as_str() {
            "4" | "7" if is_id(cells[1]) => ids.push(cells[1].to_string()),
            "13" => {
                if let Some(m) = cells[1].strip_prefix("**M")
                    && let Some(n) = m.split_whitespace().next()
                {
                    milestones.insert(format!("M{n}"), ids_in(cells[2]));
                }
            }
            _ => {}
        }
    }
    Spec { ids, milestones }
}

pub fn load_spec() -> Result<Spec> {
    let text = std::fs::read_to_string(root().join("SPEC.md")).map_err(|e| e.to_string())?;
    Ok(parse_spec(&text))
}

pub struct Progress {
    pub current: String,
    pub ticked: Vec<String>,
}

pub fn parse_progress(text: &str) -> Progress {
    let mut current = String::from("M0");
    let mut ticked = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Current milestone:") {
            current = rest
                .trim()
                .trim_matches('*')
                .split_whitespace()
                .next()
                .unwrap_or("M0")
                .to_string();
        }
        if let Some(rest) = line.trim_start().strip_prefix("- [x] ")
            && let Some(id) = ids_in(rest).into_iter().next()
        {
            ticked.push(id);
        }
    }
    Progress { current, ticked }
}

pub fn load_progress() -> Result<Progress> {
    let text = std::fs::read_to_string(root().join("PROGRESS.md")).unwrap_or_default();
    Ok(parse_progress(&text))
}

/// Milestone ordinal: `M3` → 3.
pub fn ordinal(m: &str) -> u32 {
    m.trim_start_matches('M').parse().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_tags() {
        assert!(is_id("A11Y-1"));
        assert!(is_id("OPEN-12"));
        assert!(!is_id("M1"));
        assert!(!is_id("open-1"));
        assert_eq!(tag("A11Y-1"), "req_a11y_1");
        assert_eq!(
            ids_in_test_name("tests::req_view_2_req_a11y_1_largest"),
            vec!["VIEW-2", "A11Y-1"]
        );
        assert_eq!(ids_in_test_name("req_perf_10x"), vec!["PERF-10"]);
        assert!(ids_in_test_name("no_tag_here").is_empty());
    }

    #[test]
    fn parses_the_real_spec() {
        let spec = load_spec().unwrap();
        assert!(spec.ids.contains(&"OPEN-1".to_string()));
        assert!(spec.ids.contains(&"PERF-9".to_string()));
        assert!(spec.ids.contains(&"A11Y-1".to_string()));
        assert_eq!(spec.milestones.len(), 6);
        assert!(spec.milestones["M0"].is_empty());
        assert!(spec.milestones["M1"].contains(&"PERF-4".to_string()));
        assert!(spec.milestones["M5"].contains(&"KEY-1".to_string()));
        // Every §4/§7 ID belongs to exactly one milestone.
        for id in &spec.ids {
            let n = spec.milestones.values().filter(|v| v.contains(id)).count();
            assert_eq!(n, 1, "{id} in {n} milestones");
        }
    }

    #[test]
    fn parses_progress() {
        let p = parse_progress(
            "Current milestone: **M2**\n- [x] OPEN-1 — `req_open_1_x`\n- [ ] OPEN-2\n",
        );
        assert_eq!(p.current, "M2");
        assert_eq!(p.ticked, vec!["OPEN-1"]);
    }
}
