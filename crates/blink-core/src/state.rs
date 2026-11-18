//! State, actions, effects and the pure reducer (SPEC §8.1).
//!
//! `reduce(&State, Action) -> (State, Vec<Effect>)`. The runtime executes effects (scan a folder,
//! write a sidecar…) and feeds results back as actions.

use crate::formats::Formats;
use crate::item::{Item, ScannedFile, build_items};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Values of the XMP fields Blink owns (SPEC §6), for one file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Marks {
    /// −1 rejected, 0 unrated, 1–5 stars. `None`: no rating recorded.
    pub rating: Option<i8>,
    /// Colour label name, e.g. `Red`.
    pub label: Option<String>,
}

/// Application state.
#[derive(Debug, Clone)]
pub struct State {
    pub formats: Formats,
    pub folder: Option<PathBuf>,
    /// Include sub-folders when scanning (OPEN-1 toggle). Off by default.
    pub recursive: bool,
    pub items: Vec<Item>,
    pub current: Option<usize>,
    /// Marks per item, keyed by the item's primary path.
    pub marks: BTreeMap<PathBuf, Marks>,
}

impl State {
    pub fn new(formats: Formats) -> Self {
        Self {
            formats,
            folder: None,
            recursive: false,
            items: Vec::new(),
            current: None,
            marks: BTreeMap::new(),
        }
    }

    pub fn current_item(&self) -> Option<&Item> {
        self.current.and_then(|i| self.items.get(i))
    }
}

/// Everything that can happen.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Open a folder (CLI argument, drag-and-drop or dialog — OPEN-1).
    OpenFolder(PathBuf),
    /// Toggle recursive scanning; rescans the open folder.
    SetRecursive(bool),
    /// Result of [`Effect::ScanFolder`].
    FolderScanned {
        folder: PathBuf,
        recursive: bool,
        files: Vec<ScannedFile>,
    },
    /// Marks read from sidecars or embedded XMP (RATE-4), keyed by primary path.
    MarksLoaded(Vec<(PathBuf, Marks)>),
    /// Set the star rating (−1…5) of an item; applies to every member (OPEN-2).
    SetRating { item: usize, rating: i8 },
    /// Set (`Some`) or clear (`None`) the colour label of an item; applies to every member.
    SetLabel { item: usize, label: Option<String> },
}

/// Work for the runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    ScanFolder {
        folder: PathBuf,
        recursive: bool,
    },
    /// Apply `change` to the sidecar of `source` (sidecar naming: FILE-4). Only the changed
    /// field is carried, so a writer never clears a field it wasn't asked to touch.
    WriteSidecar {
        source: PathBuf,
        change: MarkChange,
    },
}

/// One field change for a sidecar write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkChange {
    /// Set `xmp:Rating` (−1…5).
    Rating(i8),
    /// Set `xmp:Label` (`None` removes it).
    Label(Option<String>),
}

/// The pure reducer.
pub fn reduce(state: &State, action: Action) -> (State, Vec<Effect>) {
    let mut s = state.clone();
    let mut fx = Vec::new();
    match action {
        Action::OpenFolder(folder) => {
            s.folder = Some(folder.clone());
            s.items.clear();
            s.marks.clear();
            s.current = None;
            fx.push(Effect::ScanFolder {
                folder,
                recursive: s.recursive,
            });
        }
        Action::SetRecursive(on) => {
            s.recursive = on;
            if let Some(folder) = s.folder.clone() {
                fx.push(Effect::ScanFolder {
                    folder,
                    recursive: on,
                });
            }
        }
        Action::FolderScanned {
            folder,
            recursive,
            files,
        } => {
            // Drop stale results (another folder or mode was requested meanwhile).
            if s.folder.as_ref() == Some(&folder) && s.recursive == recursive {
                let keep = s.current_item().map(|i| i.primary.path.clone());
                s.items = build_items(&files, &s.formats);
                s.current = keep
                    .and_then(|p| s.items.iter().position(|i| i.primary.path == p))
                    .or((!s.items.is_empty()).then_some(0));
            }
        }
        Action::MarksLoaded(list) => {
            // Loading runs off-thread: a field the user already set wins over the file's value.
            for (path, m) in list {
                let e = s.marks.entry(path).or_default();
                e.rating = e.rating.or(m.rating);
                e.label = e.label.take().or(m.label);
            }
        }
        Action::SetRating { item, rating } => {
            if let Some(it) = s.items.get(item)
                && (-1..=5).contains(&rating)
            {
                let key = it.primary.path.clone();
                let mut m = s.marks.get(&key).cloned().unwrap_or_default();
                m.rating = Some(rating);
                for member in it.members() {
                    fx.push(Effect::WriteSidecar {
                        source: member.path.clone(),
                        change: MarkChange::Rating(rating),
                    });
                }
                s.marks.insert(key, m);
            }
        }
        Action::SetLabel { item, label } => {
            if let Some(it) = s.items.get(item) {
                let key = it.primary.path.clone();
                for member in it.members() {
                    fx.push(Effect::WriteSidecar {
                        source: member.path.clone(),
                        change: MarkChange::Label(label.clone()),
                    });
                }
                s.marks.entry(key).or_default().label = label;
            }
        }
    }
    (s, fx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn scanned(dir: &str, names: &[&str]) -> Vec<ScannedFile> {
        names
            .iter()
            .map(|n| ScannedFile {
                path: Path::new(dir).join(n),
                len: 1,
            })
            .collect()
    }

    fn opened(names: &[&str]) -> State {
        let s = State::new(Formats::new(["arw"]));
        let (s, fx) = reduce(&s, Action::OpenFolder("/f".into()));
        assert_eq!(
            fx,
            [Effect::ScanFolder {
                folder: "/f".into(),
                recursive: false
            }]
        );
        reduce(
            &s,
            Action::FolderScanned {
                folder: "/f".into(),
                recursive: false,
                files: scanned("/f", names),
            },
        )
        .0
    }

    #[test]
    fn req_open_1_open_folder_is_non_recursive_by_default_and_toggle_rescans() {
        let s = opened(&["a.ARW", "b.JPG", "notes.txt"]);
        assert_eq!(s.items.len(), 2);
        assert_eq!(s.current, Some(0));
        let (s2, fx) = reduce(&s, Action::SetRecursive(true));
        assert!(s2.recursive);
        assert_eq!(
            fx,
            [Effect::ScanFolder {
                folder: "/f".into(),
                recursive: true
            }]
        );
        // A stale non-recursive result arriving after the toggle is ignored.
        let (s3, _) = reduce(
            &s2,
            Action::FolderScanned {
                folder: "/f".into(),
                recursive: false,
                files: vec![],
            },
        );
        assert_eq!(s3.items.len(), 2);
    }

    #[test]
    fn req_open_2_rating_applies_to_every_pair_member() {
        let s = opened(&["a.ARW", "a.JPG", "a.HEIC", "b.JPG"]);
        let (s, fx) = reduce(&s, Action::SetRating { item: 0, rating: 3 });
        let want = Marks {
            rating: Some(3),
            label: None,
        };
        assert_eq!(
            fx,
            ["/f/a.ARW", "/f/a.HEIC", "/f/a.JPG"]
                .iter()
                .map(|p| Effect::WriteSidecar {
                    source: p.into(),
                    change: MarkChange::Rating(3)
                })
                .collect::<Vec<_>>()
        );
        assert_eq!(s.marks.get(Path::new("/f/a.ARW")), Some(&want));
        // Out-of-range ratings and items do nothing.
        assert!(
            reduce(&s, Action::SetRating { item: 0, rating: 6 })
                .1
                .is_empty()
        );
        assert!(
            reduce(&s, Action::SetRating { item: 9, rating: 1 })
                .1
                .is_empty()
        );
    }

    #[test]
    fn req_rate_4_late_marks_load_does_not_override_user_rating() {
        let s = opened(&["a.ARW", "b.ARW"]);
        let (s, _) = reduce(&s, Action::SetRating { item: 0, rating: 3 });
        let disk = |r: i8, l: &str| Marks {
            rating: Some(r),
            label: Some(l.into()),
        };
        let (s, _) = reduce(
            &s,
            Action::MarksLoaded(vec![
                ("/f/a.ARW".into(), disk(1, "Red")),
                ("/f/b.ARW".into(), disk(4, "Blue")),
            ]),
        );
        // The user's 3 stays; the file's label still shows; untouched items load as on disk.
        assert_eq!(s.marks.get(Path::new("/f/a.ARW")), Some(&disk(3, "Red")));
        assert_eq!(s.marks.get(Path::new("/f/b.ARW")), Some(&disk(4, "Blue")));
    }

    #[test]
    fn req_open_2_label_applies_to_every_pair_member_and_only_the_label_changes() {
        let s = opened(&["a.ARW", "a.JPG"]);
        let (s, _) = reduce(&s, Action::SetRating { item: 0, rating: 2 });
        let (s, fx) = reduce(
            &s,
            Action::SetLabel {
                item: 0,
                label: Some("Blue".into()),
            },
        );
        assert_eq!(
            fx,
            ["/f/a.ARW", "/f/a.JPG"]
                .iter()
                .map(|p| Effect::WriteSidecar {
                    source: p.into(),
                    change: MarkChange::Label(Some("Blue".into()))
                })
                .collect::<Vec<_>>()
        );
        let want = Marks {
            rating: Some(2),
            label: Some("Blue".into()),
        };
        assert_eq!(s.marks.get(Path::new("/f/a.ARW")), Some(&want));
        let (s, _) = reduce(
            &s,
            Action::SetLabel {
                item: 0,
                label: None,
            },
        );
        assert_eq!(s.marks.get(Path::new("/f/a.ARW")).unwrap().label, None);
    }
}
