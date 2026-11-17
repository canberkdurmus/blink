//! Items and pairing (OPEN-1, OPEN-2, D-4, D-5). Pure: takes a directory listing, returns items.

use crate::formats::{FileKind, Formats};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One file of a folder listing, as the runtime saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedFile {
    pub path: PathBuf,
    pub len: u64,
}

/// One file that belongs to an item.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Member {
    pub path: PathBuf,
    pub kind: FileKind,
}

/// One shot as the user sees it (SPEC §3): a RAW/DNG with optional same-stem JPEG/HEIF, or a
/// standalone JPEG/DNG/HEIF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// The file shown and identified: the RAW/DNG of a pair, else the only file.
    pub primary: Member,
    /// Paired JPEG/HEIF files (empty for standalone items).
    pub companions: Vec<Member>,
}

impl Item {
    /// All member files, primary first. Ratings and labels apply to every one (OPEN-2).
    pub fn members(&self) -> impl Iterator<Item = &Member> {
        std::iter::once(&self.primary).chain(&self.companions)
    }

    /// The paired JPEG, if any (display fallback, D-4).
    pub fn paired_jpeg(&self) -> Option<&Path> {
        self.companions
            .iter()
            .find(|m| m.kind == FileKind::Jpeg)
            .map(|m| m.path.as_path())
    }

    pub fn file_name(&self) -> String {
        file_name(&self.primary.path)
    }
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Group key: same folder, same stem, compared case-insensitively (cameras and card readers
/// disagree on case; `IMG_1.CR3` + `img_1.jpg` are one shot).
fn stem_key(p: &Path) -> (PathBuf, String) {
    let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    (dir, stem)
}

/// Build items from a listing. Unsupported files are ignored. A JPEG/HEIF pairs with a RAW/DNG
/// of the same stem in the same folder when exactly one RAW/DNG has that stem; otherwise every
/// file stays its own item (an ambiguous pair is never guessed). Items are sorted by path.
pub fn build_items(files: &[ScannedFile], formats: &Formats) -> Vec<Item> {
    let mut groups: BTreeMap<(PathBuf, String), Vec<Member>> = BTreeMap::new();
    for f in files {
        let Some(kind) = formats.classify(&file_name(&f.path)) else {
            continue;
        };
        groups.entry(stem_key(&f.path)).or_default().push(Member {
            path: f.path.clone(),
            kind,
        });
    }
    let mut items = Vec::new();
    for (_, mut members) in groups {
        members.sort();
        let raws: Vec<_> = members.iter().filter(|m| m.kind.is_raw_like()).collect();
        if raws.len() == 1 {
            let primary = raws[0].clone();
            let companions = members.into_iter().filter(|m| *m != primary).collect();
            items.push(Item {
                primary,
                companions,
            });
        } else {
            items.extend(members.into_iter().map(|primary| Item {
                primary,
                companions: Vec::new(),
            }));
        }
    }
    items.sort_by(|a, b| a.primary.path.cmp(&b.primary.path));
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn formats() -> Formats {
        Formats::new(["arw", "cr3", "nef"])
    }

    fn files(names: &[&str]) -> Vec<ScannedFile> {
        names
            .iter()
            .map(|n| ScannedFile {
                path: PathBuf::from("/shoot").join(n),
                len: 1,
            })
            .collect()
    }

    #[test]
    fn req_open_2_n_pairs_show_as_n_items() {
        let n = 25;
        let mut names = Vec::new();
        for i in 0..n {
            names.push(format!("DSC{i:05}.ARW"));
            names.push(format!("DSC{i:05}.JPG"));
            if i % 2 == 0 {
                names.push(format!("DSC{i:05}.HEIC"));
            }
        }
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let items = build_items(&files(&refs), &formats());
        assert_eq!(items.len(), n);
        for (i, it) in items.iter().enumerate() {
            assert_eq!(it.primary.kind, FileKind::Raw);
            assert_eq!(it.companions.len(), if i % 2 == 0 { 2 } else { 1 });
            assert!(it.paired_jpeg().is_some());
            assert_eq!(it.members().count(), it.companions.len() + 1);
        }
    }

    #[test]
    fn req_open_2_pairing_rules() {
        let items = build_items(
            &files(&[
                "a.ARW", "A.jpg", // pair, case-insensitive stem
                "b.JPG", // standalone JPEG
                "c.DNG", "c.JPG",  // DNG + JPEG pair
                "d.HEIC", // standalone HEIF
                "e.ARW", "e.CR3", "e.JPG", // ambiguous: three separate items
                "f.xmp", "g.txt", // ignored
                "h.NEF",
            ]),
            &formats(),
        );
        let shape: Vec<(String, usize)> = items
            .iter()
            .map(|i| (i.file_name(), i.companions.len()))
            .collect();
        assert_eq!(
            shape,
            [
                ("a.ARW".into(), 1),
                ("b.JPG".into(), 0),
                ("c.DNG".into(), 1),
                ("d.HEIC".into(), 0),
                ("e.ARW".into(), 0),
                ("e.CR3".into(), 0),
                ("e.JPG".into(), 0),
                ("h.NEF".into(), 0),
            ]
        );
    }

    #[test]
    fn req_open_2_pairs_only_within_the_same_folder() {
        let fs = vec![
            ScannedFile {
                path: PathBuf::from("/s/x/1.ARW"),
                len: 1,
            },
            ScannedFile {
                path: PathBuf::from("/s/y/1.JPG"),
                len: 1,
            },
        ];
        assert_eq!(build_items(&fs, &formats()).len(), 2);
    }
}
