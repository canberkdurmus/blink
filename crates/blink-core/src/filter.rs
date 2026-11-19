//! Item filters. M1 ships the subset `blink-cli list --filter` needs (CLI-1); FILT-1 (M4) adds
//! body, lens, ranges and dates, and the UI chips.
//!
//! Syntax: comma-separated conditions, combined with AND:
//! `rating>=N`, `rating=N`, `rejected`, `!rejected`, `picked`, `label=NAME`, `kind=raw|dng|jpeg|heif`.

use crate::formats::FileKind;
use crate::item::Item;
use crate::state::Marks;

/// One condition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cond {
    RatingAtLeast(i8),
    RatingEq(i8),
    Rejected,
    NotRejected,
    Picked,
    Label(String),
    Kind(FileKind),
}

/// All conditions must hold.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter(pub Vec<Cond>);

impl Filter {
    pub fn parse(expr: &str) -> Result<Self, String> {
        let mut out = Vec::new();
        for part in expr.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let lower = part.to_ascii_lowercase();
            let num = |s: &str| -> Result<i8, String> {
                let n: i8 = s
                    .trim()
                    .parse()
                    .map_err(|_| format!("bad rating in `{part}`"))?;
                if (-1..=5).contains(&n) {
                    Ok(n)
                } else {
                    Err(format!("rating out of range in `{part}`"))
                }
            };
            let c = if let Some(v) = lower.strip_prefix("rating>=") {
                Cond::RatingAtLeast(num(v)?)
            } else if let Some(v) = lower.strip_prefix("rating=") {
                Cond::RatingEq(num(v)?)
            } else if lower == "rejected" {
                Cond::Rejected
            } else if lower == "!rejected" {
                Cond::NotRejected
            } else if lower == "picked" {
                Cond::Picked
            } else if let Some(v) = lower.strip_prefix("label=") {
                Cond::Label(v.trim().to_string())
            } else if let Some(v) = lower.strip_prefix("kind=") {
                Cond::Kind(match v.trim() {
                    "raw" => FileKind::Raw,
                    "dng" => FileKind::Dng,
                    "jpeg" | "jpg" => FileKind::Jpeg,
                    "heif" | "heic" => FileKind::Heif,
                    k => return Err(format!("unknown kind `{k}`")),
                })
            } else {
                return Err(format!("unknown filter `{part}`"));
            };
            out.push(c);
        }
        Ok(Self(out))
    }

    /// Whether `item` with `marks` passes. Pick = rating equal to `pick_value` (D-2).
    pub fn matches(&self, item: &Item, marks: Option<&Marks>, pick_value: i8) -> bool {
        let rating = marks.and_then(|m| m.rating).unwrap_or(0);
        let label = marks.and_then(|m| m.label.as_deref());
        self.0.iter().all(|c| match c {
            Cond::RatingAtLeast(n) => rating >= *n,
            Cond::RatingEq(n) => rating == *n,
            Cond::Rejected => rating == -1,
            Cond::NotRejected => rating != -1,
            Cond::Picked => rating == pick_value,
            Cond::Label(l) => label.is_some_and(|x| x.eq_ignore_ascii_case(l)),
            Cond::Kind(k) => item.primary.kind == *k,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::Member;

    fn item(kind: FileKind) -> Item {
        Item {
            primary: Member {
                path: "/a".into(),
                kind,
            },
            companions: vec![],
        }
    }

    #[test]
    fn req_cli_1_filter_expressions() {
        let f = Filter::parse("rating>=3, label=red, kind=raw").unwrap();
        let m = Marks {
            rating: Some(4),
            label: Some("Red".into()),
        };
        assert!(f.matches(&item(FileKind::Raw), Some(&m), 1));
        assert!(!f.matches(&item(FileKind::Jpeg), Some(&m), 1));
        assert!(!f.matches(&item(FileKind::Raw), None, 1));
        let rej = Marks {
            rating: Some(-1),
            label: None,
        };
        assert!(
            Filter::parse("rejected")
                .unwrap()
                .matches(&item(FileKind::Raw), Some(&rej), 1)
        );
        assert!(
            !Filter::parse("!rejected")
                .unwrap()
                .matches(&item(FileKind::Raw), Some(&rej), 1)
        );
        let one = Marks {
            rating: Some(1),
            label: None,
        };
        assert!(
            Filter::parse("picked")
                .unwrap()
                .matches(&item(FileKind::Dng), Some(&one), 1)
        );
        assert!(
            Filter::parse("")
                .unwrap()
                .matches(&item(FileKind::Heif), None, 1)
        );
        assert!(Filter::parse("rating>=9").is_err());
        assert!(Filter::parse("lens=x").is_err());
    }
}
