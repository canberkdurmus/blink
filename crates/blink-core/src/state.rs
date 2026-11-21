//! State, actions, effects and the pure reducer (SPEC §8.1).
//!
//! `reduce(&State, Action) -> (State, Vec<Effect>)`. The runtime executes effects (scan a folder,
//! write a sidecar…) and feeds results back as actions.

use crate::formats::{FileKind, Formats};
use crate::item::{Item, ScannedFile, build_items};
use crate::preview::PreviewSource;
use crate::view::{DECODE_AHEAD, Direction, Loupe, Shown, Zoom, prefetch_window};
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

/// The colour labels, in key order (`6`–`9`, `Shift+9`; SPEC §5.2).
pub const LABELS: [&str; 5] = ["Red", "Yellow", "Green", "Blue", "Purple"];

/// User settings the reducer needs (loaded from the config file by the runtime).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Star value a pick maps to (D-2, RATE-2): 1–5, default 1.
    pub pick_value: i8,
    /// Move to the next item after a rating, pick, reject or unflag (RATE-1).
    pub auto_advance: bool,
    /// The `xmp:Label` value written for each colour of [`LABELS`] (SPEC §6: names configurable
    /// for apps that differ).
    pub label_names: [String; 5],
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            pick_value: 1,
            auto_advance: false,
            label_names: LABELS.map(String::from),
        }
    }
}

/// One undoable change of an item's marks (RATE-3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    /// The item's primary path.
    pub key: PathBuf,
    pub before: Marks,
    pub after: Marks,
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
    /// Marks as read from disk on open (RATE-4). When an item's marks return to these, its
    /// sidecars are restored byte for byte (RATE-3).
    pub disk_marks: BTreeMap<PathBuf, Marks>,
    pub settings: Settings,
    /// Undo and redo stacks (RATE-3), newest last. Cleared when another folder opens.
    pub undo: Vec<Edit>,
    pub redo: Vec<Edit>,
    /// Loupe view (VIEW-1, VIEW-4, VIEW-9).
    pub loupe: Loupe,
    /// Direction of the last move, for the prefetch window.
    pub direction: Direction,
    /// Last non-blocking message for the toast area (SPEC §5.1).
    pub toast: Option<String>,
}

/// What the runtime needs to load an item's pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemRef {
    pub path: PathBuf,
    pub kind: FileKind,
    /// The paired JPEG: the full-resolution fallback of a LOW-RES RAW (VIEW-4).
    pub paired_jpeg: Option<PathBuf>,
}

impl ItemRef {
    pub fn of(item: &Item) -> Self {
        Self {
            path: item.primary.path.clone(),
            kind: item.primary.kind,
            paired_jpeg: item.paired_jpeg().map(PathBuf::from),
        }
    }
}

impl Action {
    /// Changes marks (or their history). The GUI holds these back until the folder's marks are
    /// loaded: before that, an edit can't know what it changes (RATE-3, RATE-4).
    pub fn is_mark_edit(&self) -> bool {
        matches!(
            self,
            Self::SetRating { .. }
                | Self::SetLabel { .. }
                | Self::Rate(_)
                | Self::Pick
                | Self::Reject
                | Self::Unflag
                | Self::ToggleLabel(_)
                | Self::Undo
                | Self::Redo
        )
    }
}

/// Moves between items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    Next,
    Prev,
    First,
    Last,
}

/// Which image of an item a load result is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Level {
    /// The default preview (embedded preview, or the JPEG itself).
    Preview,
    /// The full-resolution image for zoom (paired JPEG or RAW decode, VIEW-4).
    Full,
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
            disk_marks: BTreeMap::new(),
            settings: Settings::default(),
            undo: Vec::new(),
            redo: Vec::new(),
            loupe: Loupe::default(),
            direction: Direction::Forward,
            toast: None,
        }
    }

    /// The effect that shows the current item and prefetches its neighbours.
    fn show_effect(&self) -> Option<Effect> {
        let cur = self.current?;
        let window = prefetch_window(cur, self.items.len(), self.direction);
        let ahead = (1..=DECODE_AHEAD)
            .map_while(|n| match self.direction {
                Direction::Forward => cur.checked_add(n).filter(|&i| i < self.items.len()),
                Direction::Backward => cur.checked_sub(n),
            })
            .map(|i| ItemRef::of(&self.items[i]))
            .collect();
        Some(Effect::Show {
            current: ItemRef::of(self.items.get(cur)?),
            prefetch: window
                .into_iter()
                .map(|i| ItemRef::of(&self.items[i]))
                .collect(),
            ahead,
        })
    }

    /// Request the full-resolution image when the zoom needs it (VIEW-4).
    fn full_effect(&mut self) -> Option<Effect> {
        if self.loupe.pending_full || !self.loupe.needs_full() {
            return None;
        }
        let item = ItemRef::of(self.current_item()?);
        self.loupe.pending_full = true;
        Some(Effect::LoadFull { item })
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
    SetRating {
        item: usize,
        rating: i8,
    },
    /// Set (`Some`) or clear (`None`) the colour label of an item; applies to every member.
    SetLabel {
        item: usize,
        label: Option<String>,
    },
    /// Keys `0`–`5`: set the current item's stars (RATE-1).
    Rate(i8),
    /// `P`: rating = the pick value (RATE-2).
    Pick,
    /// `X`: rating = −1.
    Reject,
    /// `U`: clear a pick or a reject. A pick is cleared only if the rating equals the pick
    /// value (RATE-2); other star ratings stay.
    Unflag,
    /// `6`–`9`, `Shift+9`: toggle colour label `LABELS[i]` on the current item.
    ToggleLabel(usize),
    /// `Shift+A`.
    ToggleAutoAdvance,
    /// `Ctrl/Cmd+Z`, `Ctrl/Cmd+Shift+Z` (RATE-3).
    Undo,
    Redo,
    /// Apply settings from the config file.
    Configure(Settings),
    /// Move to another item (VIEW-1).
    Navigate(Nav),
    /// Set the zoom; with a cursor position (viewport pixels) zoom happens at the cursor.
    SetZoom {
        zoom: Zoom,
        cursor: Option<[f32; 2]>,
    },
    /// Decode the current item's RAW data now, whatever the preview size (bench scripts, PERF-5).
    DecodeRaw,
    /// `Z` / click: Fit ↔ 100 % at the cursor (VIEW-4).
    ToggleZoom100 {
        cursor: Option<[f32; 2]>,
    },
    /// Drag by viewport pixels (VIEW-9).
    Pan([f32; 2]),
    /// The loupe area changed size (physical pixels).
    Resize([f32; 2]),
    /// Keep zoom and position when moving to another item (VIEW-9).
    SetKeepZoom(bool),
    ToggleClipping,
    ToggleInfoPanel,
    /// Pixels for `path` are ready in the runtime's cache (result of [`Effect::Show`] or
    /// [`Effect::LoadFull`]).
    ImageReady {
        path: PathBuf,
        level: Level,
        source: PreviewSource,
        size: (u32, u32),
        full_size: (u32, u32),
    },
    /// Writing the sidecar of `source` failed (FILE-6); the change stays queued for a retry.
    SidecarWriteFailed {
        source: PathBuf,
        error: String,
    },
    /// Loading failed.
    ImageFailed {
        path: PathBuf,
        level: Level,
        error: String,
    },
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
    /// `owns_adobe_name`: the Adobe-style name `stem.xmp` belongs to this file alone. It doesn't
    /// for the JPEG/HEIF companions of a pair (the RAW owns it) or for same-stem files that
    /// didn't pair; those use the darktable-style `name.ext.xmp` (ADR 0015).
    WriteSidecar {
        source: PathBuf,
        change: MarkChange,
        owns_adobe_name: bool,
    },
    /// Display `current` as soon as possible and prefetch `prefetch` in order. Jobs for any
    /// other item are stale and may be cancelled. `ahead` lists up to
    /// [`DECODE_AHEAD`] items in the travel direction, nearest first (the first ones are the
    /// prefetch window's), so the loader can decode ahead of a held key (PERF-3).
    Show {
        current: ItemRef,
        prefetch: Vec<ItemRef>,
        ahead: Vec<ItemRef>,
    },
    /// Load the full-resolution image of `item`: its paired JPEG if any, else a RAW decode.
    LoadFull {
        item: ItemRef,
    },
}

/// One field change for a sidecar write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkChange {
    /// Set `xmp:Rating` (−1…5); `None` removes it (undo back to "no rating").
    Rating(Option<i8>),
    /// Set `xmp:Label` (`None` removes it).
    Label(Option<String>),
    /// The item's marks are back at their on-disk values (RATE-3): put each sidecar back exactly
    /// as it was before Blink first wrote it this session (or remove one it created). A sidecar
    /// another app changed since gets these values instead. Field changes queued after it apply
    /// on top.
    Restore {
        rating: Option<i8>,
        label: Option<String>,
    },
}

/// Folder and lower-cased stem: files that share it share the Adobe-style sidecar name.
fn stem_of(p: &std::path::Path) -> (Option<&std::path::Path>, String) {
    (
        p.parent(),
        p.file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default(),
    )
}

/// One [`Effect::WriteSidecar`] per member of `items[item]` (OPEN-2).
fn sidecar_writes(items: &[Item], item: usize, change: &MarkChange) -> Vec<Effect> {
    let Some(it) = items.get(item) else {
        return Vec::new();
    };
    let stem = stem_of(&it.primary.path);
    // Another item with the same stem (an unpaired same-stem file) shares `stem.xmp`.
    let shared = items
        .iter()
        .enumerate()
        .any(|(i, o)| i != item && o.members().any(|m| stem_of(&m.path) == stem));
    it.members()
        .enumerate()
        .map(|(k, m)| Effect::WriteSidecar {
            source: m.path.clone(),
            change: change.clone(),
            owns_adobe_name: k == 0 && !shared,
        })
        .collect()
}

fn navigate(s: &mut State, nav: Nav, fx: &mut Vec<Effect>) {
    let len = s.items.len();
    if let Some(cur) = s.current {
        let (next, dir) = match nav {
            Nav::Next => ((cur + 1).min(len - 1), Direction::Forward),
            Nav::Prev => (cur.saturating_sub(1), Direction::Backward),
            Nav::First => (0, Direction::Backward),
            Nav::Last => (len - 1, Direction::Forward),
        };
        s.direction = dir;
        if next != cur {
            s.current = Some(next);
            // Never show the previous item's pixels for the new item (PERF-3).
            s.loupe.shown = None;
            s.loupe.pending_full = false;
            if !s.loupe.keep_zoom {
                s.loupe.zoom = Zoom::Fit;
                s.loupe.center = [0.5, 0.5];
            }
            fx.extend(s.show_effect());
        }
    }
}

/// Set the marks of `items[item]`: record the edit for undo (when `undo` is set), update the
/// state and emit the sidecar writes for the fields that changed (OPEN-2).
fn set_marks(s: &mut State, item: usize, new: Marks, fx: &mut Vec<Effect>, undo: bool) {
    let Some(it) = s.items.get(item) else {
        return;
    };
    let key = it.primary.path.clone();
    let old = s.marks.get(&key).cloned().unwrap_or_default();
    if old == new {
        return;
    }
    if undo {
        s.undo.push(Edit {
            key: key.clone(),
            before: old.clone(),
            after: new.clone(),
        });
        s.redo.clear();
    }
    let disk = s.disk_marks.get(&key).cloned().unwrap_or_default();
    if new == disk {
        let restore = MarkChange::Restore {
            rating: new.rating,
            label: new.label.clone(),
        };
        fx.extend(sidecar_writes(&s.items, item, &restore));
    } else {
        if new.rating != old.rating {
            fx.extend(sidecar_writes(
                &s.items,
                item,
                &MarkChange::Rating(new.rating),
            ));
        }
        if new.label != old.label {
            fx.extend(sidecar_writes(
                &s.items,
                item,
                &MarkChange::Label(new.label.clone()),
            ));
        }
    }
    s.marks.insert(key, new);
}

/// Change the current item's rating; auto-advance if on (RATE-1).
fn rate_current(s: &mut State, rating: Option<i8>, fx: &mut Vec<Effect>) {
    let Some(cur) = s.current else { return };
    let Some(key) = s.current_item().map(|i| i.primary.path.clone()) else {
        return;
    };
    let mut m = s.marks.get(&key).cloned().unwrap_or_default();
    if let Some(r) = rating {
        m.rating = Some(r);
    }
    set_marks(s, cur, m, fx, true);
    if s.settings.auto_advance {
        navigate(s, Nav::Next, fx);
    }
}

/// Undo (`redo == false`) or redo the newest edit.
fn undo_redo(s: &mut State, redo: bool, fx: &mut Vec<Effect>) {
    let popped = if redo { s.redo.pop() } else { s.undo.pop() };
    let Some(e) = popped else { return };
    let target = if redo {
        e.after.clone()
    } else {
        e.before.clone()
    };
    if let Some(i) = s.items.iter().position(|it| it.primary.path == e.key) {
        set_marks(s, i, target, fx, false);
    }
    if redo {
        s.undo.push(e);
    } else {
        s.redo.push(e);
    }
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
            s.disk_marks.clear();
            s.undo.clear();
            s.redo.clear();
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
                if s.loupe.shown.as_ref().map(|x| &x.path)
                    != s.current_item().map(|i| &i.primary.path)
                {
                    s.loupe.shown = None;
                    s.loupe.pending_full = false;
                }
                fx.extend(s.show_effect());
            }
        }
        Action::Navigate(nav) => navigate(&mut s, nav, &mut fx),
        Action::SetZoom { zoom, cursor } => {
            s.loupe.set_zoom(zoom, cursor);
            fx.extend(s.full_effect());
        }
        Action::ToggleZoom100 { cursor } => {
            let zoom = if s.loupe.zoom == Zoom::Fit {
                Zoom::P100
            } else {
                Zoom::Fit
            };
            s.loupe.set_zoom(zoom, cursor);
            fx.extend(s.full_effect());
        }
        Action::DecodeRaw => {
            if let Some(it) = s.current_item()
                && it.primary.kind.is_raw_like()
                && !s.loupe.pending_full
            {
                let item = ItemRef {
                    paired_jpeg: None,
                    ..ItemRef::of(it)
                };
                s.loupe.pending_full = true;
                fx.push(Effect::LoadFull { item });
            }
        }
        Action::Pan(d) => s.loupe.pan(d),
        Action::Resize(v) => {
            s.loupe.viewport = [v[0].max(1.0), v[1].max(1.0)];
            s.loupe.clamp_center();
        }
        Action::SetKeepZoom(on) => s.loupe.keep_zoom = on,
        Action::ToggleClipping => s.loupe.clipping = !s.loupe.clipping,
        Action::ToggleInfoPanel => s.loupe.info_panel = !s.loupe.info_panel,
        Action::ImageReady {
            path,
            level,
            source,
            size,
            full_size,
        } => {
            let current = s.current_item().map(|i| i.primary.path.clone());
            // A full image always replaces a preview; a late preview never replaces a full one.
            let replaces = match (&s.loupe.shown, level) {
                (_, Level::Full) => s.loupe.pending_full,
                (None, Level::Preview) => true,
                (Some(x), Level::Preview) => x.path != path,
            };
            if current.as_ref() == Some(&path) && replaces {
                if level == Level::Full {
                    s.loupe.pending_full = false;
                }
                s.loupe.shown = Some(Shown {
                    path,
                    source,
                    size,
                    full_size,
                });
                s.loupe.clamp_center();
                // Kept zoom on a new item: fetch its full image too (VIEW-9 + VIEW-4).
                fx.extend(s.full_effect());
            }
        }
        Action::ImageFailed { path, level, error } => {
            if s.current_item().map(|i| &i.primary.path) == Some(&path) {
                match level {
                    Level::Preview => {
                        s.loupe.shown = Some(Shown {
                            path,
                            source: PreviewSource::Unavailable,
                            size: (0, 0),
                            full_size: (0, 0),
                        });
                    }
                    // Only a failed zoom decode is news to the user; "no preview" is the badge.
                    Level::Full => {
                        s.loupe.pending_full = false;
                        s.toast = Some(error);
                    }
                }
            }
        }
        Action::SidecarWriteFailed { source, error } => {
            let name = source
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            s.toast = Some(format!("Could not write the sidecar of {name}: {error}"));
        }
        Action::MarksLoaded(list) => {
            // Loading runs off-thread: a field the user already set wins over the file's value.
            for (path, m) in list {
                // The first load after open is what's on disk. A reload after a rescan reads
                // Blink's own writes back and must move neither the restore point nor the undo
                // history, nor the marks (RATE-3).
                if s.disk_marks.contains_key(&path) {
                    continue;
                }
                s.disk_marks.insert(path.clone(), m.clone());
                let e = s.marks.entry(path).or_default();
                e.rating = e.rating.or(m.rating);
                e.label = e.label.take().or(m.label);
            }
            // Items with nothing on disk: their restore point is "no marks", even after Blink
            // has written a sidecar for them and a rescan reads it.
            for it in &s.items {
                s.disk_marks.entry(it.primary.path.clone()).or_default();
            }
        }
        Action::SetRating { item, rating } => {
            if (-1..=5).contains(&rating)
                && let Some(it) = s.items.get(item)
            {
                let mut m = s.marks.get(&it.primary.path).cloned().unwrap_or_default();
                m.rating = Some(rating);
                set_marks(&mut s, item, m, &mut fx, true);
            }
        }
        Action::SetLabel { item, label } => {
            if let Some(it) = s.items.get(item) {
                let mut m = s.marks.get(&it.primary.path).cloned().unwrap_or_default();
                m.label = label;
                set_marks(&mut s, item, m, &mut fx, true);
            }
        }
        Action::Rate(n) => {
            if (0..=5).contains(&n) {
                rate_current(&mut s, Some(n), &mut fx);
            }
        }
        Action::Pick => {
            let v = s.settings.pick_value;
            rate_current(&mut s, Some(v), &mut fx);
        }
        Action::Reject => rate_current(&mut s, Some(-1), &mut fx),
        Action::Unflag => {
            let r = s
                .current_item()
                .and_then(|i| s.marks.get(&i.primary.path))
                .and_then(|m| m.rating);
            let flagged = r == Some(-1) || r == Some(s.settings.pick_value);
            rate_current(&mut s, flagged.then_some(0), &mut fx);
        }
        Action::ToggleLabel(i) => {
            if let (Some(cur), Some(name)) = (s.current, s.settings.label_names.get(i).cloned()) {
                let key = s.items[cur].primary.path.clone();
                let mut m = s.marks.get(&key).cloned().unwrap_or_default();
                m.label = if m.label.as_deref() == Some(name.as_str()) {
                    None
                } else {
                    Some(name)
                };
                set_marks(&mut s, cur, m, &mut fx, true);
            }
        }
        Action::ToggleAutoAdvance => {
            s.settings.auto_advance = !s.settings.auto_advance;
            s.toast = Some(format!(
                "Auto-advance {}",
                if s.settings.auto_advance { "on" } else { "off" }
            ));
        }
        Action::Undo => undo_redo(&mut s, false, &mut fx),
        Action::Redo => undo_redo(&mut s, true, &mut fx),
        Action::Configure(mut settings) => {
            settings.pick_value = settings.pick_value.clamp(1, 5);
            s.settings = settings;
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
                    change: MarkChange::Rating(Some(3)),
                    owns_adobe_name: *p == "/f/a.ARW",
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
                    change: MarkChange::Label(Some("Blue".into())),
                    owns_adobe_name: *p == "/f/a.ARW",
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

    fn ready(path: &str, level: Level, source: PreviewSource, size: (u32, u32)) -> Action {
        Action::ImageReady {
            path: path.into(),
            level,
            source,
            size,
            full_size: (4912, 3264),
        }
    }

    fn shows(fx: &[Effect]) -> Vec<PathBuf> {
        fx.iter()
            .filter_map(|e| match e {
                Effect::Show { current, .. } => Some(current.path.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn req_view_1_opening_shows_the_first_item_fit_with_its_embedded_preview() {
        let s = State::new(Formats::new(["arw"]));
        let (s, _) = reduce(&s, Action::OpenFolder("/f".into()));
        let (s, fx) = reduce(
            &s,
            Action::FolderScanned {
                folder: "/f".into(),
                recursive: false,
                files: scanned("/f", &["a.ARW", "b.ARW", "c.ARW"]),
            },
        );
        let Some(Effect::Show {
            current, prefetch, ..
        }) = fx.first()
        else {
            panic!("{fx:?}")
        };
        assert_eq!(current.path, Path::new("/f/a.ARW"));
        assert_eq!(
            prefetch.iter().map(|r| r.path.clone()).collect::<Vec<_>>(),
            [PathBuf::from("/f/b.ARW"), "/f/c.ARW".into()]
        );
        assert_eq!(s.loupe.shown, None);
        let (s, fx) = reduce(
            &s,
            ready(
                "/f/a.ARW",
                Level::Preview,
                PreviewSource::Embedded,
                (4912, 3264),
            ),
        );
        assert!(fx.is_empty(), "fit never needs the full image: {fx:?}");
        // State snapshot of the loupe after opening.
        assert_eq!(s.loupe.zoom, Zoom::Fit);
        assert_eq!(s.loupe.center, [0.5, 0.5]);
        assert!(!s.loupe.pending_full);
        let shown = s.loupe.shown.as_ref().unwrap();
        assert_eq!(shown.source, PreviewSource::Embedded);
        assert_eq!(shown.path, Path::new("/f/a.ARW"));
        let r = s.loupe.image_rect().unwrap();
        assert!(r.w <= s.loupe.viewport[0] + 0.01 && r.h <= s.loupe.viewport[1] + 0.01);
    }

    #[test]
    fn req_view_1_navigation_moves_the_current_item_and_never_shows_stale_pixels() {
        let s = opened(&["a.ARW", "b.ARW", "c.ARW"]);
        let (s, _) = reduce(
            &s,
            ready(
                "/f/a.ARW",
                Level::Preview,
                PreviewSource::Embedded,
                (10, 10),
            ),
        );
        let (s, fx) = reduce(&s, Action::Navigate(Nav::Next));
        assert_eq!(s.current, Some(1));
        assert_eq!(s.loupe.shown, None, "previous item's pixels must not stay");
        assert_eq!(shows(&fx), [PathBuf::from("/f/b.ARW")]);
        // A late result for another item is ignored.
        let (s, _) = reduce(
            &s,
            ready(
                "/f/a.ARW",
                Level::Preview,
                PreviewSource::Embedded,
                (10, 10),
            ),
        );
        assert_eq!(s.loupe.shown, None);
        let (s, fx) = reduce(&s, Action::Navigate(Nav::Last));
        assert_eq!((s.current, shows(&fx).len()), (Some(2), 1));
        let (s2, fx) = reduce(&s, Action::Navigate(Nav::Next));
        assert_eq!(s2.current, Some(2));
        assert!(fx.is_empty(), "no move at the end");
        let (s, fx) = reduce(&s, Action::Navigate(Nav::Prev));
        assert_eq!((s.current, s.direction), (Some(1), Direction::Backward));
        let Some(Effect::Show { prefetch, .. }) = fx.first() else {
            panic!()
        };
        assert_eq!(
            prefetch[0].path,
            Path::new("/f/a.ARW"),
            "travel direction first"
        );
        let (s, _) = reduce(&s, Action::Navigate(Nav::First));
        assert_eq!(s.current, Some(0));
    }

    #[test]
    fn req_view_4_zoom_on_low_res_raw_loads_the_raw_decode_and_swaps_it_in() {
        let s = opened(&["a.ARW", "b.ARW"]);
        let (s, _) = reduce(
            &s,
            ready(
                "/f/a.ARW",
                Level::Preview,
                PreviewSource::EmbeddedLowRes,
                (1616, 1080),
            ),
        );
        let (s, fx) = reduce(
            &s,
            Action::ToggleZoom100 {
                cursor: Some([100.0, 100.0]),
            },
        );
        assert_eq!(s.loupe.zoom, Zoom::P100);
        assert_eq!(
            fx,
            [Effect::LoadFull {
                item: ItemRef {
                    path: "/f/a.ARW".into(),
                    kind: FileKind::Raw,
                    paired_jpeg: None
                }
            }]
        );
        // Meanwhile the upscaled preview stays with a spinner.
        assert!(s.loupe.pending_full);
        assert_eq!(
            s.loupe.shown.as_ref().unwrap().source,
            PreviewSource::EmbeddedLowRes
        );
        // Zooming again while pending doesn't queue a second decode.
        let (s, fx) = reduce(
            &s,
            Action::SetZoom {
                zoom: Zoom::P200,
                cursor: None,
            },
        );
        assert!(fx.is_empty());
        let (s, _) = reduce(
            &s,
            ready(
                "/f/a.ARW",
                Level::Full,
                PreviewSource::RawDecode,
                (4912, 3264),
            ),
        );
        assert!(!s.loupe.pending_full);
        let shown = s.loupe.shown.as_ref().unwrap();
        assert_eq!(
            (shown.source, shown.size),
            (PreviewSource::RawDecode, (4912, 3264))
        );
        // A late preview never replaces the full image.
        let (s, _) = reduce(
            &s,
            ready(
                "/f/a.ARW",
                Level::Preview,
                PreviewSource::EmbeddedLowRes,
                (1616, 1080),
            ),
        );
        assert_eq!(s.loupe.shown.unwrap().source, PreviewSource::RawDecode);
    }

    #[test]
    fn req_view_4_paired_jpeg_is_named_in_the_full_request_and_failure_clears_the_spinner() {
        let s = opened(&["a.ARW", "a.JPG"]);
        let (s, _) = reduce(
            &s,
            ready(
                "/f/a.ARW",
                Level::Preview,
                PreviewSource::EmbeddedLowRes,
                (1616, 1080),
            ),
        );
        let (s, fx) = reduce(&s, Action::ToggleZoom100 { cursor: None });
        let Some(Effect::LoadFull { item }) = fx.first() else {
            panic!("{fx:?}")
        };
        assert_eq!(item.paired_jpeg.as_deref(), Some(Path::new("/f/a.JPG")));
        let (s, _) = reduce(
            &s,
            Action::ImageFailed {
                path: "/f/a.ARW".into(),
                level: Level::Full,
                error: "raw: unsupported".into(),
            },
        );
        assert!(!s.loupe.pending_full);
        assert_eq!(s.toast.as_deref(), Some("raw: unsupported"));
        assert_eq!(
            s.loupe.shown.unwrap().source,
            PreviewSource::EmbeddedLowRes,
            "the preview stays"
        );
    }

    #[test]
    fn req_perf_3_show_lists_the_items_ahead_in_the_travel_direction_nearest_first() {
        let names: Vec<String> = (0..20).map(|i| format!("{i:02}.ARW")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let s = opened(&names);
        let ahead = |fx: &[Effect]| match fx.first() {
            Some(Effect::Show {
                ahead, prefetch, ..
            }) => {
                // The first items ahead are the prefetch window's; decode-ahead reaches further.
                let window: Vec<_> = ahead.iter().filter(|a| prefetch.contains(a)).collect();
                assert!(ahead.iter().zip(&window).all(|(a, w)| a == *w));
                ahead
                    .iter()
                    .map(|r| r.path.file_name().unwrap().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
            }
            other => panic!("{other:?}"),
        };
        let (s, fx) = reduce(&s, Action::Navigate(Nav::Next));
        let (s, fx) = (s, ahead(&fx));
        assert_eq!(
            fx[..8],
            [
                "02.ARW", "03.ARW", "04.ARW", "05.ARW", "06.ARW", "07.ARW", "08.ARW", "09.ARW"
            ]
        );
        let (s, fx) = reduce(&s, Action::Navigate(Nav::Last));
        assert!(ahead(&fx).is_empty(), "nothing ahead of the last item");
        let (_, fx) = reduce(&s, Action::Navigate(Nav::Prev));
        assert_eq!(ahead(&fx)[..3], ["17.ARW", "16.ARW", "15.ARW"]);
    }

    #[test]
    fn req_perf_3_ahead_reaches_past_the_prefetch_window_for_slow_decodes() {
        // A decode that takes 12 key repeats must start 12 items ahead, beyond the 8 prefetched.
        let names: Vec<String> = (0..100).map(|i| format!("{i:03}.ARW")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let (s, fx) = reduce(&opened(&names), Action::Navigate(Nav::Next));
        let Some(Effect::Show {
            ahead, prefetch, ..
        }) = fx.first()
        else {
            panic!("{fx:?}")
        };
        assert_eq!(ahead.len(), crate::view::DECODE_AHEAD);
        assert_eq!(ahead[0].path.file_name().unwrap(), "002.ARW");
        assert_eq!(ahead[31].path.file_name().unwrap(), "033.ARW");
        assert_eq!(
            prefetch.len(),
            9,
            "the prefetch window stays 8 ahead (and 1 behind item 1)"
        );
        let (_, fx) = reduce(&s, Action::Navigate(Nav::Last));
        let Some(Effect::Show { ahead, .. }) = fx.first() else {
            panic!("{fx:?}")
        };
        assert!(ahead.is_empty());
    }

    #[test]
    fn req_view_9_zoom_position_is_kept_on_the_next_item_and_toggleable() {
        let s = opened(&["a.ARW", "b.ARW", "c.ARW"]);
        let (s, _) = reduce(&s, Action::Resize([1000.0, 500.0]));
        let full = |p: &str| ready(p, Level::Preview, PreviewSource::Embedded, (4912, 3264));
        let (s, _) = reduce(&s, full("/f/a.ARW"));
        let (s, _) = reduce(
            &s,
            Action::SetZoom {
                zoom: Zoom::P100,
                cursor: Some([900.0, 100.0]),
            },
        );
        let (zoom, center) = (s.loupe.zoom, s.loupe.center);
        assert_ne!(center, [0.5, 0.5]);
        let (s, _) = reduce(&s, Action::Navigate(Nav::Next));
        // A resize while the next item loads must not lose the position.
        let (s, _) = reduce(&s, Action::Resize([1000.0, 500.0]));
        let (s, _) = reduce(&s, full("/f/b.ARW"));
        assert_eq!((s.loupe.zoom, s.loupe.center), (zoom, center), "same eye");
        // Kept 100 % on a LOW-RES item requests its full image right away.
        let (s, _) = reduce(&s, Action::Navigate(Nav::Next));
        let (s, fx) = reduce(
            &s,
            ready(
                "/f/c.ARW",
                Level::Preview,
                PreviewSource::EmbeddedLowRes,
                (1616, 1080),
            ),
        );
        assert!(matches!(fx.as_slice(), [Effect::LoadFull { .. }]), "{fx:?}");
        // Toggle off: moving resets to Fit.
        let (s, _) = reduce(&s, Action::SetKeepZoom(false));
        let (s, _) = reduce(&s, Action::Navigate(Nav::Prev));
        assert_eq!((s.loupe.zoom, s.loupe.center), (Zoom::Fit, [0.5, 0.5]));
    }

    #[test]
    fn req_view_6_clipping_and_info_panel_toggle() {
        let s = opened(&["a.ARW"]);
        assert!(!s.loupe.clipping && !s.loupe.info_panel);
        let (s, _) = reduce(&s, Action::ToggleClipping);
        let (s, _) = reduce(&s, Action::ToggleInfoPanel);
        assert!(s.loupe.clipping && s.loupe.info_panel);
        let (s, _) = reduce(&s, Action::ToggleClipping);
        assert!(!s.loupe.clipping);
    }

    #[test]
    fn req_perf_5_decode_raw_requests_a_raw_decode_even_with_a_full_size_preview_or_jpeg() {
        let s = opened(&["a.ARW", "a.JPG", "b.JPG"]);
        let (s, _) = reduce(
            &s,
            ready(
                "/f/a.ARW",
                Level::Preview,
                PreviewSource::Embedded,
                (4912, 3264),
            ),
        );
        let (s, fx) = reduce(&s, Action::DecodeRaw);
        assert_eq!(
            fx,
            [Effect::LoadFull {
                item: ItemRef {
                    path: "/f/a.ARW".into(),
                    kind: FileKind::Raw,
                    paired_jpeg: None
                }
            }]
        );
        assert!(s.loupe.pending_full);
        let (s, _) = reduce(
            &s,
            ready(
                "/f/a.ARW",
                Level::Full,
                PreviewSource::RawDecode,
                (4912, 3264),
            ),
        );
        assert_eq!(s.loupe.shown.unwrap().source, PreviewSource::RawDecode);
        // A standalone JPEG has no RAW data.
        let s = opened(&["b.JPG"]);
        assert!(reduce(&s, Action::DecodeRaw).1.is_empty());
    }
}

#[cfg(test)]
mod rate_tests {
    use super::*;
    use std::path::Path;

    fn opened(names: &[&str]) -> State {
        let s = State::new(Formats::new(["arw"]));
        let (s, _) = reduce(&s, Action::OpenFolder("/f".into()));
        let files = names
            .iter()
            .map(|n| ScannedFile {
                path: Path::new("/f").join(n),
                len: 1,
            })
            .collect();
        reduce(
            &s,
            Action::FolderScanned {
                folder: "/f".into(),
                recursive: false,
                files,
            },
        )
        .0
    }

    fn marks(s: &State, p: &str) -> Marks {
        s.marks.get(Path::new(p)).cloned().unwrap_or_default()
    }

    fn m(r: Option<i8>, l: Option<&str>) -> Marks {
        Marks {
            rating: r,
            label: l.map(str::to_string),
        }
    }

    fn changes(fx: &[Effect]) -> Vec<(String, MarkChange)> {
        fx.iter()
            .filter_map(|e| match e {
                Effect::WriteSidecar { source, change, .. } => {
                    Some((source.display().to_string(), change.clone()))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn req_rate_1_each_key_sets_stars_reject_and_toggles_labels() {
        let mut s = opened(&["a.ARW", "a.JPG", "b.ARW"]);
        for n in 0..=5 {
            let (t, fx) = reduce(&s, Action::Rate(n));
            assert_eq!(marks(&t, "/f/a.ARW").rating, Some(n));
            if n > 0 {
                assert_eq!(
                    changes(&fx),
                    [
                        ("/f/a.ARW".into(), MarkChange::Rating(Some(n))),
                        ("/f/a.JPG".into(), MarkChange::Rating(Some(n)))
                    ]
                );
            }
            s = t;
        }
        assert!(reduce(&s, Action::Rate(6)).1.is_empty());
        let (s, _) = reduce(&s, Action::Reject);
        assert_eq!(marks(&s, "/f/a.ARW").rating, Some(-1));
        for (i, name) in LABELS.iter().enumerate() {
            let (t, fx) = reduce(&s, Action::ToggleLabel(i));
            assert_eq!(marks(&t, "/f/a.ARW"), m(Some(-1), Some(name)));
            // Only the label is written.
            assert!(
                changes(&fx)
                    .iter()
                    .all(|(_, c)| *c == MarkChange::Label(Some(name.to_string())))
            );
            // Same key again removes it.
            let (u, _) = reduce(&t, Action::ToggleLabel(i));
            assert_eq!(marks(&u, "/f/a.ARW").label, None);
        }
        // Another label replaces the current one.
        let (s, _) = reduce(&s, Action::ToggleLabel(0));
        let (s, _) = reduce(&s, Action::ToggleLabel(3));
        assert_eq!(marks(&s, "/f/a.ARW").label.as_deref(), Some("Blue"));
        assert!(reduce(&s, Action::ToggleLabel(5)).1.is_empty());
    }

    #[test]
    fn req_rate_1_auto_advance_moves_on_after_rating_and_toggles() {
        let s = opened(&["a.ARW", "b.ARW", "c.ARW"]);
        let (s, _) = reduce(&s, Action::Rate(3));
        assert_eq!(s.current, Some(0));
        let (s, _) = reduce(&s, Action::ToggleAutoAdvance);
        assert!(s.settings.auto_advance);
        assert_eq!(s.toast.as_deref(), Some("Auto-advance on"));
        let (s, fx) = reduce(&s, Action::Rate(4));
        assert_eq!(s.current, Some(1));
        assert!(fx.iter().any(|e| matches!(e, Effect::Show { .. })));
        let (s, _) = reduce(&s, Action::Pick);
        let (s, _) = reduce(&s, Action::Reject);
        assert_eq!(s.current, Some(2));
        assert_eq!(marks(&s, "/f/b.ARW").rating, Some(1));
        assert_eq!(marks(&s, "/f/c.ARW").rating, Some(-1));
        // Labels don't advance.
        let (s, _) = reduce(&s, Action::Navigate(Nav::First));
        let (s, _) = reduce(&s, Action::ToggleLabel(0));
        assert_eq!(s.current, Some(0));
        let (s, _) = reduce(&s, Action::ToggleAutoAdvance);
        let (s, _) = reduce(&s, Action::Rate(1));
        assert_eq!(s.current, Some(0));
    }

    #[test]
    fn req_rate_2_pick_uses_the_configured_value_and_unpick_only_clears_it() {
        for pick in 1..=5i8 {
            let s = opened(&["a.ARW"]);
            let (s, _) = reduce(
                &s,
                Action::Configure(Settings {
                    pick_value: pick,
                    ..Settings::default()
                }),
            );
            let (s, _) = reduce(&s, Action::Pick);
            assert_eq!(marks(&s, "/f/a.ARW").rating, Some(pick));
            let (u, _) = reduce(&s, Action::Unflag);
            assert_eq!(marks(&u, "/f/a.ARW").rating, Some(0));
            // A rating that isn't the pick value stays.
            let other = if pick == 5 { 4 } else { pick + 1 };
            let (t, _) = reduce(&s, Action::Rate(other));
            let (t, fx) = reduce(&t, Action::Unflag);
            assert_eq!(marks(&t, "/f/a.ARW").rating, Some(other));
            assert!(changes(&fx).is_empty());
            // Unflag also clears a reject.
            let (r, _) = reduce(&s, Action::Reject);
            let (r, _) = reduce(&r, Action::Unflag);
            assert_eq!(marks(&r, "/f/a.ARW").rating, Some(0));
        }
        // Out-of-range config values are clamped.
        let s = opened(&["a.ARW"]);
        let (s, _) = reduce(
            &s,
            Action::Configure(Settings {
                pick_value: 9,
                ..Settings::default()
            }),
        );
        assert_eq!(s.settings.pick_value, 5);
    }

    #[test]
    fn req_rate_2_label_names_come_from_the_config() {
        let s = opened(&["a.ARW"]);
        let mut cfg = Settings::default();
        cfg.label_names[4] = "Violet".into();
        let (s, _) = reduce(&s, Action::Configure(cfg));
        let (s, fx) = reduce(&s, Action::ToggleLabel(4));
        assert_eq!(marks(&s, "/f/a.ARW").label.as_deref(), Some("Violet"));
        assert_eq!(changes(&fx)[0].1, MarkChange::Label(Some("Violet".into())));
    }

    #[test]
    fn req_rate_3_undo_redo_walks_the_history_and_restores_at_disk_values() {
        let s = opened(&["a.ARW", "b.ARW"]);
        let (s, _) = reduce(
            &s,
            Action::MarksLoaded(vec![("/f/a.ARW".into(), m(Some(2), Some("Red")))]),
        );
        let (s, _) = reduce(&s, Action::Rate(4));
        let (s, _) = reduce(&s, Action::ToggleLabel(2));
        let (s, _) = reduce(&s, Action::Navigate(Nav::Next));
        let (s, _) = reduce(&s, Action::Reject);
        assert_eq!(s.undo.len(), 3);
        let (s, fx) = reduce(&s, Action::Undo);
        assert_eq!(marks(&s, "/f/b.ARW"), m(None, None));
        assert_eq!(
            changes(&fx),
            [(
                "/f/b.ARW".into(),
                MarkChange::Restore {
                    rating: None,
                    label: None
                }
            )]
        );
        let (s, _) = reduce(&s, Action::Undo);
        assert_eq!(marks(&s, "/f/a.ARW"), m(Some(4), Some("Red")));
        let (s, fx) = reduce(&s, Action::Undo);
        assert_eq!(marks(&s, "/f/a.ARW"), m(Some(2), Some("Red")));
        assert_eq!(
            changes(&fx),
            [(
                "/f/a.ARW".into(),
                MarkChange::Restore {
                    rating: Some(2),
                    label: Some("Red".into())
                }
            )]
        );
        assert!(reduce(&s, Action::Undo).1.is_empty());
        // Redo replays in order; a new edit clears the redo stack.
        let (s, _) = reduce(&s, Action::Redo);
        assert_eq!(marks(&s, "/f/a.ARW"), m(Some(4), Some("Red")));
        let (s, _) = reduce(&s, Action::Redo);
        assert_eq!(marks(&s, "/f/a.ARW"), m(Some(4), Some("Green")));
        let (s, _) = reduce(&s, Action::Rate(1));
        assert!(s.redo.is_empty());
        assert!(reduce(&s, Action::Redo).1.is_empty());
        // Setting the same value is not an edit.
        let n = s.undo.len();
        assert_eq!(reduce(&s, Action::Rate(1)).0.undo.len(), n);
        // Opening another folder starts a new history.
        let (s, _) = reduce(&s, Action::OpenFolder("/g".into()));
        assert!(s.undo.is_empty() && s.redo.is_empty());
    }
}
