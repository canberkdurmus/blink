//! Sort order of the visible items (FILT-2). Pure.
//!
//! Capture time (default) is oldest first, items without a capture time last; rating is highest
//! first (unrated counts as 0, rejected last); filename is ascending. The sort is stable and every
//! tie is broken by the file name, then the full path, so the order never depends on how the
//! items were listed (ADR 0020).

use crate::filter::ItemMeta;
use crate::item::Item;
use crate::state::Marks;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// What the items are sorted by.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum SortKey {
    #[default]
    CaptureTime,
    FileName,
    Rating,
}

impl SortKey {
    pub const ALL: [SortKey; 3] = [Self::CaptureTime, Self::FileName, Self::Rating];

    pub fn label(self) -> &'static str {
        match self {
            Self::CaptureTime => "Capture time",
            Self::FileName => "Filename",
            Self::Rating => "Rating",
        }
    }

    /// Whether the order depends on marks or metadata (a change of those re-sorts).
    pub fn needs_marks(self) -> bool {
        self == Self::Rating
    }
}

fn by_name(a: &Item, b: &Item) -> Ordering {
    a.primary
        .path
        .file_name()
        .cmp(&b.primary.path.file_name())
        .then_with(|| a.primary.path.cmp(&b.primary.path))
}

/// Indices of `items` for which `keep` holds, in `key` order.
pub fn order(
    items: &[Item],
    key: SortKey,
    marks: &BTreeMap<PathBuf, Marks>,
    meta: &BTreeMap<PathBuf, ItemMeta>,
    keep: impl Fn(usize) -> bool,
) -> Vec<usize> {
    let mut v: Vec<usize> = (0..items.len()).filter(|&i| keep(i)).collect();
    let time = |i: usize| {
        meta.get(&items[i].primary.path)
            .and_then(|m| m.captured.as_deref())
    };
    let rating = |i: usize| {
        marks
            .get(&items[i].primary.path)
            .and_then(|m| m.rating)
            .unwrap_or(0)
    };
    v.sort_by(|&a, &b| {
        let first = match key {
            // `None` sorts after every time.
            SortKey::CaptureTime => match (time(a), time(b)) {
                (Some(x), Some(y)) => x.cmp(y),
                (x, y) => x.is_none().cmp(&y.is_none()),
            },
            SortKey::FileName => Ordering::Equal,
            SortKey::Rating => rating(b).cmp(&rating(a)),
        };
        first.then_with(|| by_name(&items[a], &items[b]))
    });
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::FileKind;
    use crate::item::Member;

    fn items(paths: &[&str]) -> Vec<Item> {
        paths
            .iter()
            .map(|p| Item {
                primary: Member {
                    path: p.into(),
                    kind: FileKind::Raw,
                },
                companions: vec![],
            })
            .collect()
    }

    fn names(items: &[Item], o: &[usize]) -> Vec<String> {
        o.iter()
            .map(|&i| items[i].primary.path.display().to_string())
            .collect()
    }

    #[test]
    fn req_filt_2_capture_time_filename_and_rating_orders_with_filename_ties() {
        let it = items(&[
            "/f/d.ARW",
            "/f/b.ARW",
            "/f/a.ARW",
            "/f/c.ARW",
            "/f/sub/a.ARW",
        ]);
        let t = |p: &str, c: Option<&str>| {
            (
                PathBuf::from(p),
                ItemMeta {
                    captured: c.map(String::from),
                    ..Default::default()
                },
            )
        };
        let meta: BTreeMap<_, _> = [
            t("/f/d.ARW", Some("2024:01:01 10:00:00")),
            t("/f/b.ARW", Some("2024:01:01 09:00:00")),
            t("/f/a.ARW", None),
            t("/f/c.ARW", Some("2024:01:01 10:00:00")),
        ]
        .into();
        let r = |p: &str, n: i8| {
            (
                PathBuf::from(p),
                Marks {
                    rating: Some(n),
                    label: None,
                },
            )
        };
        let marks: BTreeMap<_, _> = [r("/f/d.ARW", 3), r("/f/b.ARW", -1), r("/f/c.ARW", 3)].into();
        let all = |_| true;
        assert_eq!(
            names(&it, &order(&it, SortKey::CaptureTime, &marks, &meta, all)),
            [
                "/f/b.ARW",
                "/f/c.ARW",
                "/f/d.ARW",
                "/f/a.ARW",
                "/f/sub/a.ARW"
            ]
        );
        assert_eq!(
            names(&it, &order(&it, SortKey::FileName, &marks, &meta, all)),
            [
                "/f/a.ARW",
                "/f/sub/a.ARW",
                "/f/b.ARW",
                "/f/c.ARW",
                "/f/d.ARW"
            ]
        );
        assert_eq!(
            names(&it, &order(&it, SortKey::Rating, &marks, &meta, all)),
            [
                "/f/c.ARW",
                "/f/d.ARW",
                "/f/a.ARW",
                "/f/sub/a.ARW",
                "/f/b.ARW"
            ]
        );
        // `keep` filters before sorting.
        assert_eq!(
            names(
                &it,
                &order(&it, SortKey::Rating, &marks, &meta, |i| i % 2 == 0)
            ),
            ["/f/d.ARW", "/f/a.ARW", "/f/sub/a.ARW"]
        );
        assert_eq!(SortKey::default(), SortKey::CaptureTime);
    }

    proptest::proptest! {
        /// Stable and total: the result never depends on the input order of the items.
        #[test]
        fn req_filt_2_order_is_independent_of_the_listing_order(
            spec in proptest::collection::vec(
                (0u8..4, proptest::option::of(0u8..3), proptest::option::of(-1i8..=5)), 1..12),
            key in proptest::sample::select(SortKey::ALL.to_vec()),
            seed in proptest::prelude::any::<u64>(),
        ) {
            let mut it = Vec::new();
            let mut meta = BTreeMap::new();
            let mut marks = BTreeMap::new();
            for (n, (name, t, r)) in spec.iter().enumerate() {
                let p = PathBuf::from(format!("/f/{n}/{name}.ARW"));
                if let Some(t) = t {
                    meta.insert(p.clone(), ItemMeta { captured: Some(format!("2024:01:01 0{t}:00:00")), ..Default::default() });
                }
                marks.insert(p.clone(), Marks { rating: *r, label: None });
                it.push(Item { primary: Member { path: p, kind: FileKind::Raw }, companions: vec![] });
            }
            let want: Vec<PathBuf> = order(&it, key, &marks, &meta, |_| true)
                .into_iter().map(|i| it[i].primary.path.clone()).collect();
            let mut shuffled = it.clone();
            let mut s = seed;
            for i in (1..shuffled.len()).rev() {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                shuffled.swap(i, (s >> 33) as usize % (i + 1));
            }
            let got: Vec<PathBuf> = order(&shuffled, key, &marks, &meta, |_| true)
                .into_iter().map(|i| shuffled[i].primary.path.clone()).collect();
            proptest::prop_assert_eq!(got, want);
        }
    }
}
