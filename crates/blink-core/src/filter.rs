//! Item filters (FILT-1, CLI-1). Pure.
//!
//! Syntax (CLI `--filter`, the GUI's filter field): comma-separated conditions, combined with AND:
//! `rating>=N`, `rating=N`, `rejected`, `!rejected`, `picked`, `label=NAME`, `body=NAME`,
//! `lens=NAME`, `focal=MIN..MAX` (mm), `iso=MIN..MAX`, `date=FROM..TO` (`YYYY-MM-DD`, inclusive),
//! `kind=raw|dng|jpeg|heif`. A range may leave out either end (`iso=..800`, `focal=200..`), and a
//! single value is a one-value range (`focal=50`, `date=2024-06-01`). Text values compare ignoring
//! ASCII case. An item whose file doesn't carry a field fails every condition on that field.

use crate::formats::FileKind;
use crate::item::Item;
use crate::state::Marks;

/// The capture metadata filters and the capture-time sort use, read from an item's primary file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ItemMeta {
    /// Camera body as the info panel shows it ([`crate::info::body`]).
    pub body: Option<String>,
    pub lens: Option<String>,
    pub focal_mm: Option<f64>,
    pub iso: Option<u32>,
    /// EXIF `DateTimeOriginal`, `YYYY:MM:DD HH:MM:SS` (camera time). Sorts as text.
    pub captured: Option<String>,
}

impl ItemMeta {
    /// The capture date as `YYYY-MM-DD`.
    pub fn date(&self) -> Option<String> {
        let c = self.captured.as_deref()?.trim();
        let d = c.get(..10)?;
        let b = d.as_bytes();
        let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
        (digits(0..4) && digits(5..7) && digits(8..10) && matches!(b[4], b':' | b'-'))
            .then(|| format!("{}-{}-{}", &d[0..4], &d[5..7], &d[8..10]))
    }
}

/// An inclusive range; `None` ends are open.
#[derive(Debug, Clone, PartialEq)]
pub struct Range<T> {
    pub min: Option<T>,
    pub max: Option<T>,
}

impl<T: PartialOrd> Range<T> {
    pub fn contains(&self, v: &T) -> bool {
        self.min.as_ref().is_none_or(|m| v >= m) && self.max.as_ref().is_none_or(|m| v <= m)
    }
}

impl<T: std::fmt::Display + PartialEq> Range<T> {
    fn expr(&self) -> String {
        match (&self.min, &self.max) {
            (Some(a), Some(b)) if a == b => a.to_string(),
            (a, b) => format!(
                "{}..{}",
                a.as_ref().map(T::to_string).unwrap_or_default(),
                b.as_ref().map(T::to_string).unwrap_or_default()
            ),
        }
    }

    fn label(&self, unit: &str) -> String {
        match (&self.min, &self.max) {
            (Some(a), Some(b)) if a == b => format!("{a}{unit}"),
            (Some(a), Some(b)) => format!("{a}–{b}{unit}"),
            (Some(a), None) => format!("≥ {a}{unit}"),
            (None, Some(b)) => format!("≤ {b}{unit}"),
            (None, None) => "any".into(),
        }
    }
}

/// One condition.
#[derive(Debug, Clone, PartialEq)]
pub enum Cond {
    RatingAtLeast(i8),
    RatingEq(i8),
    Rejected,
    NotRejected,
    Picked,
    Label(String),
    Body(String),
    Lens(String),
    Focal(Range<f64>),
    Iso(Range<u32>),
    /// Capture date, `YYYY-MM-DD` bounds.
    Date(Range<String>),
    Kind(FileKind),
}

impl Cond {
    /// Conditions of the same slot replace each other when added from the UI (one rating
    /// condition, one flag, one label…).
    pub fn slot(&self) -> u8 {
        match self {
            Self::RatingAtLeast(_) | Self::RatingEq(_) => 0,
            Self::Rejected | Self::NotRejected => 1,
            Self::Picked => 2,
            Self::Label(_) => 3,
            Self::Body(_) => 4,
            Self::Lens(_) => 5,
            Self::Focal(_) => 6,
            Self::Iso(_) => 7,
            Self::Date(_) => 8,
            Self::Kind(_) => 9,
        }
    }

    /// The condition in filter syntax; [`Filter::parse`] reads it back.
    pub fn expr(&self) -> String {
        match self {
            Self::RatingAtLeast(n) => format!("rating>={n}"),
            Self::RatingEq(n) => format!("rating={n}"),
            Self::Rejected => "rejected".into(),
            Self::NotRejected => "!rejected".into(),
            Self::Picked => "picked".into(),
            Self::Label(l) => format!("label={l}"),
            Self::Body(b) => format!("body={b}"),
            Self::Lens(l) => format!("lens={l}"),
            Self::Focal(r) => format!("focal={}", r.expr()),
            Self::Iso(r) => format!("iso={}", r.expr()),
            Self::Date(r) => format!("date={}", r.expr()),
            Self::Kind(k) => format!("kind={}", k.label().to_ascii_lowercase()),
        }
    }

    /// Text of the condition's chip in the top bar.
    pub fn label(&self) -> String {
        match self {
            Self::RatingAtLeast(n) => format!("Rating ≥ {n}"),
            Self::RatingEq(n) => format!("Rating = {n}"),
            Self::Rejected => "Rejected".into(),
            Self::NotRejected => "Not rejected".into(),
            Self::Picked => "Picked".into(),
            Self::Label(l) => format!("Label: {l}"),
            Self::Body(b) => format!("Body: {b}"),
            Self::Lens(l) => format!("Lens: {l}"),
            Self::Focal(r) => format!("Focal {}", r.label(" mm")),
            Self::Iso(r) => format!("ISO {}", r.label("")),
            Self::Date(r) => format!("Date {}", r.label("")),
            Self::Kind(k) => format!("Type: {}", k.label()),
        }
    }

    /// Whether the condition reads capture metadata (so a caller must load it).
    pub fn needs_meta(&self) -> bool {
        matches!(
            self,
            Self::Body(_) | Self::Lens(_) | Self::Focal(_) | Self::Iso(_) | Self::Date(_)
        )
    }

    /// Whether the condition reads ratings or labels.
    pub fn needs_marks(&self) -> bool {
        self.slot() <= 3
    }

    fn matches(
        &self,
        item: &Item,
        marks: Option<&Marks>,
        meta: Option<&ItemMeta>,
        pick: i8,
    ) -> bool {
        let rating = marks.and_then(|m| m.rating).unwrap_or(0);
        let text_eq = |want: &str, got: Option<&String>| {
            got.is_some_and(|g| g.trim().eq_ignore_ascii_case(want.trim()))
        };
        match self {
            Self::RatingAtLeast(n) => rating >= *n,
            Self::RatingEq(n) => rating == *n,
            Self::Rejected => rating == -1,
            Self::NotRejected => rating != -1,
            Self::Picked => rating == pick,
            Self::Label(l) => text_eq(l, marks.and_then(|m| m.label.as_ref())),
            Self::Body(b) => text_eq(b, meta.and_then(|m| m.body.as_ref())),
            Self::Lens(l) => text_eq(l, meta.and_then(|m| m.lens.as_ref())),
            Self::Focal(r) => meta
                .and_then(|m| m.focal_mm)
                .is_some_and(|f| r.contains(&f)),
            Self::Iso(r) => meta.and_then(|m| m.iso).is_some_and(|i| r.contains(&i)),
            Self::Date(r) => meta
                .and_then(ItemMeta::date)
                .is_some_and(|d| r.contains(&d)),
            Self::Kind(k) => item.primary.kind == *k,
        }
    }
}

/// All conditions must hold.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Filter(pub Vec<Cond>);

fn range<T: std::str::FromStr + PartialOrd>(
    v: &str,
    part: &str,
    check: impl Fn(&T) -> bool,
) -> Result<Range<T>, String> {
    let bad = || format!("bad range in `{part}`");
    let one = |s: &str| -> Result<Option<T>, String> {
        let s = s.trim();
        if s.is_empty() {
            return Ok(None);
        }
        let x: T = s.parse().map_err(|_| bad())?;
        if check(&x) { Ok(Some(x)) } else { Err(bad()) }
    };
    let r = match v.split_once("..") {
        Some((a, b)) => Range {
            min: one(a)?,
            max: one(b)?,
        },
        None => {
            let x = one(v)?.ok_or_else(bad)?;
            Range {
                min: Some(x),
                max: one(v)?,
            }
        }
    };
    if r.min.is_none() && r.max.is_none() {
        return Err(bad());
    }
    if let (Some(a), Some(b)) = (&r.min, &r.max)
        && a > b
    {
        return Err(format!("empty range in `{part}`"));
    }
    Ok(r)
}

/// `YYYY-MM-DD` with a plausible month and day.
fn is_date(s: &String) -> bool {
    let b = s.as_bytes();
    let num = |r: std::ops::Range<usize>| -> Option<u32> {
        b.get(r.clone())
            .filter(|d| d.iter().all(u8::is_ascii_digit))
            .and_then(|_| s[r].parse().ok())
    };
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && num(0..4).is_some()
        && num(5..7).is_some_and(|m| (1..=12).contains(&m))
        && num(8..10).is_some_and(|d| (1..=31).contains(&d))
}

impl Filter {
    pub fn parse(expr: &str) -> Result<Self, String> {
        let mut out = Vec::new();
        for part in expr.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let lower = part.to_ascii_lowercase();
            // Text values keep their case (it is shown on the chip); keys don't.
            let value = |key: &str| part[key.len()..].trim().to_string();
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
            } else if lower.starts_with("label=") {
                Cond::Label(value("label="))
            } else if lower.starts_with("body=") {
                Cond::Body(value("body="))
            } else if lower.starts_with("lens=") {
                Cond::Lens(value("lens="))
            } else if let Some(v) = lower.strip_prefix("focal=") {
                Cond::Focal(range(v, part, |f: &f64| f.is_finite() && *f >= 0.0)?)
            } else if let Some(v) = lower.strip_prefix("iso=") {
                Cond::Iso(range(v, part, |_: &u32| true)?)
            } else if let Some(v) = lower.strip_prefix("date=") {
                Cond::Date(range(v, part, is_date)?)
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

    /// The filter in [`Filter::parse`] syntax.
    pub fn expr(&self) -> String {
        self.0.iter().map(Cond::expr).collect::<Vec<_>>().join(",")
    }

    /// Whether `item` with `marks` and `meta` passes. Pick = rating equal to `pick_value` (D-2).
    pub fn matches_meta(
        &self,
        item: &Item,
        marks: Option<&Marks>,
        meta: Option<&ItemMeta>,
        pick_value: i8,
    ) -> bool {
        self.0
            .iter()
            .all(|c| c.matches(item, marks, meta, pick_value))
    }

    /// [`Filter::matches_meta`] without capture metadata (metadata conditions fail).
    pub fn matches(&self, item: &Item, marks: Option<&Marks>, pick_value: i8) -> bool {
        self.matches_meta(item, marks, None, pick_value)
    }

    pub fn needs_meta(&self) -> bool {
        self.0.iter().any(Cond::needs_meta)
    }

    pub fn needs_marks(&self) -> bool {
        self.0.iter().any(Cond::needs_marks)
    }

    /// Add `c`, replacing any condition of the same slot.
    pub fn with(&self, c: Cond) -> Self {
        let mut v: Vec<Cond> = self
            .0
            .iter()
            .filter(|x| x.slot() != c.slot())
            .cloned()
            .collect();
        v.push(c);
        Self(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::Member;
    use proptest::prelude::*;

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
        assert!(Filter::parse("shutter=x").is_err());
    }

    fn meta() -> ItemMeta {
        ItemMeta {
            body: Some("SONY ILCE-1M2".into()),
            lens: Some("FE 70-200mm F2.8 GM OSS II".into()),
            focal_mm: Some(133.0),
            iso: Some(100),
            captured: Some("2025:05:09 09:35:49".into()),
        }
    }

    #[test]
    fn req_filt_1_each_metadata_condition_and_missing_values() {
        let it = item(FileKind::Raw);
        let pass = |e: &str| {
            Filter::parse(e)
                .unwrap()
                .matches_meta(&it, None, Some(&meta()), 1)
        };
        for yes in [
            "body=sony ilce-1m2",
            "lens= FE 70-200mm F2.8 GM OSS II ",
            "focal=70..200",
            "focal=133",
            "focal=..133",
            "focal=133..",
            "iso=100",
            "iso=..400",
            "date=2025-05-09",
            "date=2025-01-01..2025-12-31",
            "date=..2025-05-09",
            "kind=raw",
            "body=SONY ILCE-1M2,iso=100..200,focal=100..150,date=2025-05-01..",
        ] {
            assert!(pass(yes), "{yes}");
        }
        for no in [
            "body=SONY",
            "lens=FE 70-200mm",
            "focal=134..",
            "focal=..132.5",
            "iso=200..",
            "date=2025-05-10..",
            "date=..2025-05-08",
            "kind=jpeg",
            "body=SONY ILCE-1M2,iso=200",
        ] {
            assert!(!pass(no), "{no}");
        }
        // A file without the field fails the condition; one without metadata fails them all.
        let none = ItemMeta::default();
        for e in [
            "body=x",
            "lens=x",
            "focal=0..",
            "iso=0..",
            "date=..9999-12-31",
        ] {
            let f = Filter::parse(e).unwrap();
            assert!(!f.matches_meta(&it, None, Some(&none), 1), "{e}");
            assert!(!f.matches_meta(&it, None, None, 1), "{e}");
        }
        for bad in [
            "focal=a..b",
            "focal=..",
            "focal=",
            "iso=-5",
            "iso=800..100",
            "date=2025-13-01",
            "date=2025/01/01",
            "date=20250101",
        ] {
            assert!(Filter::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn req_filt_1_chips_have_labels_and_adding_replaces_the_same_slot() {
        let f = Filter::default()
            .with(Cond::RatingAtLeast(3))
            .with(Cond::Body("SONY NEX-5N".into()))
            .with(Cond::RatingEq(2));
        assert_eq!(
            f.0.iter().map(Cond::label).collect::<Vec<_>>(),
            ["Body: SONY NEX-5N", "Rating = 2"]
        );
        let all = Filter::parse(
            "rating>=3,rejected,picked,label=Red,body=B,lens=L,focal=24..70,iso=..800,date=2024-01-01..,kind=dng",
        )
        .unwrap();
        assert_eq!(
            all.0.iter().map(Cond::label).collect::<Vec<_>>(),
            [
                "Rating ≥ 3",
                "Rejected",
                "Picked",
                "Label: Red",
                "Body: B",
                "Lens: L",
                "Focal 24–70 mm",
                "ISO ≤ 800",
                "Date ≥ 2024-01-01",
                "Type: DNG",
            ]
        );
        assert_eq!(Filter::parse(&all.expr()).unwrap(), all);
        assert!(all.needs_meta() && all.needs_marks());
        assert!(!Filter::parse("kind=raw").unwrap().needs_meta());
    }

    fn arb_cond() -> impl Strategy<Value = Cond> {
        let r = || proptest::option::of(0u32..3000);
        prop_oneof![
            (-1i8..=5).prop_map(Cond::RatingAtLeast),
            (-1i8..=5).prop_map(Cond::RatingEq),
            Just(Cond::Rejected),
            Just(Cond::NotRejected),
            Just(Cond::Picked),
            prop::sample::select(vec!["Red", "Green"]).prop_map(|s| Cond::Label(s.into())),
            prop::sample::select(vec!["A", "B"]).prop_map(|s| Cond::Body(s.into())),
            prop::sample::select(vec!["L1", "L2"]).prop_map(|s| Cond::Lens(s.into())),
            (r(), r()).prop_filter_map("empty", |(a, b)| {
                ((a.is_some() || b.is_some()) && a.zip(b).is_none_or(|(a, b)| a <= b)).then_some(
                    Cond::Focal(Range {
                        min: a.map(f64::from),
                        max: b.map(f64::from),
                    }),
                )
            }),
            (r(), r()).prop_filter_map("empty", |(a, b)| {
                ((a.is_some() || b.is_some()) && a.zip(b).is_none_or(|(a, b)| a <= b))
                    .then_some(Cond::Iso(Range { min: a, max: b }))
            }),
            (1u32..=28, 1u32..=28).prop_map(|(a, b)| Cond::Date(Range {
                min: Some(format!("2024-01-{:02}", a.min(b))),
                max: Some(format!("2024-01-{:02}", a.max(b))),
            })),
            prop::sample::select(vec![FileKind::Raw, FileKind::Jpeg]).prop_map(Cond::Kind),
        ]
    }

    fn arb_case() -> impl Strategy<Value = (Item, Option<Marks>, Option<ItemMeta>)> {
        (
            prop::sample::select(vec![FileKind::Raw, FileKind::Jpeg, FileKind::Dng]),
            proptest::option::of((
                proptest::option::of(-1i8..=5),
                proptest::option::of(prop::sample::select(vec!["Red", "green"])),
            )),
            proptest::option::of((
                proptest::option::of(prop::sample::select(vec!["a", "B"])),
                proptest::option::of(prop::sample::select(vec!["l1", "L2"])),
                proptest::option::of(0u32..3000),
                proptest::option::of(0u32..3000),
                proptest::option::of(1u32..=28),
            )),
        )
            .prop_map(|(k, m, x)| {
                (
                    item(k),
                    m.map(|(rating, label)| Marks {
                        rating,
                        label: label.map(String::from),
                    }),
                    x.map(|(body, lens, f, iso, d)| ItemMeta {
                        body: body.map(String::from),
                        lens: lens.map(String::from),
                        focal_mm: f.map(f64::from),
                        iso,
                        captured: d.map(|d| format!("2024:01:{d:02} 12:00:00")),
                    }),
                )
            })
    }

    proptest! {
        /// Filter algebra (SPEC §11.1): AND of conditions, order-independent, the empty filter
        /// passes everything, and the expression syntax round-trips.
        #[test]
        fn req_filt_1_filter_algebra(
            a in prop::collection::vec(arb_cond(), 0..4),
            b in prop::collection::vec(arb_cond(), 0..4),
            (it, marks, meta) in arb_case(),
            pick in 1i8..=5,
        ) {
            let m = |f: &Filter| f.matches_meta(&it, marks.as_ref(), meta.as_ref(), pick);
            let fa = Filter(a.clone());
            let fb = Filter(b.clone());
            let ab = Filter([a.clone(), b.clone()].concat());
            let ba = Filter([b, a.clone()].concat());
            prop_assert_eq!(m(&ab), m(&fa) && m(&fb));
            prop_assert_eq!(m(&ab), m(&ba));
            prop_assert!(m(&Filter::default()));
            prop_assert_eq!(m(&fa), a.iter().all(|c| m(&Filter(vec![c.clone()]))));
            prop_assert_eq!(&Filter::parse(&fa.expr()).unwrap(), &fa);
        }
    }
}
