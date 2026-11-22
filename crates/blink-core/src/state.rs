//! State, actions, effects and the pure reducer (SPEC §8.1).
//!
//! `reduce(&State, Action) -> (State, Vec<Effect>)`. The runtime executes effects (scan a folder,
//! write a sidecar…) and feeds results back as actions.

use crate::filter::{Cond, Filter, ItemMeta};
use crate::formats::{FileKind, Formats};
use crate::item::{Item, ScannedFile, build_items};
use crate::preview::PreviewSource;
use crate::sort::SortKey;
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

/// The five colour labels (RATE-1), in key order `6`…`Shift+9`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LabelColor {
    Red,
    Yellow,
    Green,
    Blue,
    Purple,
}

impl LabelColor {
    pub const ALL: [LabelColor; 5] = [
        LabelColor::Red,
        LabelColor::Yellow,
        LabelColor::Green,
        LabelColor::Blue,
        LabelColor::Purple,
    ];

    /// Index into [`MarkPrefs::label_names`].
    pub fn index(self) -> usize {
        self as usize
    }
}

/// User preferences for marking (RATE-1, RATE-2; loaded from the config file by the runtime).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkPrefs {
    /// Star value a pick maps to (D-2): 1–5, default 1.
    pub pick_value: i8,
    /// Move to the next item after a mark key (RATE-1, `Shift+A`).
    pub auto_advance: bool,
    /// `xmp:Label` value written for each colour (SPEC §6: configurable).
    pub label_names: [String; 5],
}

impl Default for MarkPrefs {
    fn default() -> Self {
        Self {
            pick_value: 1,
            auto_advance: false,
            label_names: ["Red", "Yellow", "Green", "Blue", "Purple"].map(String::from),
        }
    }
}

impl MarkPrefs {
    /// The `xmp:Label` value for `c`.
    pub fn label_name(&self, c: LabelColor) -> &str {
        &self.label_names[c.index()]
    }

    /// Which colour a stored label is, if any (case-insensitive).
    pub fn color_of(&self, label: &str) -> Option<LabelColor> {
        LabelColor::ALL
            .into_iter()
            .find(|c| self.label_name(*c).eq_ignore_ascii_case(label))
    }
}

/// A mark key on the current item (RATE-1, RATE-2; SPEC §5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkKey {
    /// `0`–`5`.
    Stars(u8),
    /// `P`: rating = the pick value (D-2).
    Pick,
    /// `X`: rating −1.
    Reject,
    /// `U`: clears a pick (only if the rating equals the pick value, RATE-2) or a reject.
    Unflag,
    /// `6`–`Shift+9`: sets the label, or clears it when the item already has it.
    Label(LabelColor),
}

/// One undoable change of one field of one item's marks (RATE-3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkEdit {
    /// Primary path of the item.
    pub path: PathBuf,
    /// `true`: the edit changed the rating; `false`: the label. (`before` and `after` may
    /// become equal when marks load after the edit, so this isn't derived from them.)
    pub rating: bool,
    pub before: Marks,
    pub after: Marks,
    /// `before` is the value the item had when the session started (no earlier edit touched the
    /// field): undo then reverts every member's sidecar to its own original bytes.
    pub before_original: bool,
    /// The sidecar writes of the edit (each member's source and `owns_adobe_name`), kept so
    /// undo/redo reach the same sidecars even when a rescan dropped the item.
    pub targets: Vec<(PathBuf, bool)>,
}

impl MarkEdit {
    fn rating_changed(&self) -> bool {
        self.rating
    }
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
    /// Loupe view (VIEW-1, VIEW-4, VIEW-9).
    pub loupe: Loupe,
    /// Direction of the last move, for the prefetch window.
    pub direction: Direction,
    /// Last non-blocking message for the toast area (SPEC §5.1).
    pub toast: Option<String>,
    /// Pick value, auto-advance, label names (RATE-1, RATE-2).
    pub prefs: MarkPrefs,
    /// Applied mark edits, oldest first (RATE-3).
    pub undo: Vec<MarkEdit>,
    /// Undone edits, most recently undone last.
    pub redo: Vec<MarkEdit>,
    /// Items whose sidecar changed on disk while Blink had unsaved changes for it: the on-disk
    /// version was kept (FILE-8). Shown as a conflict marker until the item is marked again.
    pub conflicts: std::collections::BTreeSet<PathBuf>,
    /// Session log (FILE-8 conflicts and external reloads), oldest first.
    pub log: Vec<String>,
    /// Mark changes per item (edits, undo, redo): a reload read before the latest one is stale.
    pub mark_gen: BTreeMap<PathBuf, u64>,
    /// Active filter (FILT-1); its conditions are the top bar's chips.
    pub filter: Filter,
    /// Sort order (FILT-2).
    pub sort: SortKey,
    /// Capture metadata per item, keyed by primary path, loaded in the background (FILT-1/2).
    pub meta: BTreeMap<PathBuf, ItemMeta>,
    /// Indices into `items` that pass the filter, in sort order: what the loupe steps through
    /// and the counter counts. `current` is always one of them (or `None` when it is empty).
    pub visible: Vec<usize>,
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
            loupe: Loupe::default(),
            direction: Direction::Forward,
            toast: None,
            prefs: MarkPrefs::default(),
            undo: Vec::new(),
            redo: Vec::new(),
            conflicts: Default::default(),
            log: Vec::new(),
            mark_gen: BTreeMap::new(),
            filter: Filter::default(),
            sort: SortKey::default(),
            meta: BTreeMap::new(),
            visible: Vec::new(),
        }
    }

    /// Position of the current item in [`State::visible`].
    pub fn pos(&self) -> Option<usize> {
        let c = self.current?;
        self.visible.iter().position(|&i| i == c)
    }

    /// The visible items, in order.
    pub fn visible_items(&self) -> impl Iterator<Item = &Item> {
        self.visible.iter().map(|&i| &self.items[i])
    }

    fn compute_visible(&self) -> Vec<usize> {
        let pick = self.prefs.pick_value;
        crate::sort::order(&self.items, self.sort, &self.marks, &self.meta, |i| {
            let p = &self.items[i].primary.path;
            self.filter
                .matches_meta(&self.items[i], self.marks.get(p), self.meta.get(p), pick)
        })
    }

    /// Recompute [`State::visible`] after the filter, sort, items, marks or metadata changed.
    /// A current item that no longer passes the filter gives way to the item that followed it
    /// (else the one before it); with nothing visible there is no current item (ADR 0020).
    fn refresh_view(&mut self, fx: &mut Vec<Effect>) {
        let new = self.compute_visible();
        if new == self.visible && self.current.is_none_or(|c| new.contains(&c)) {
            return;
        }
        let old_pos = self.pos();
        let old = std::mem::replace(&mut self.visible, new);
        let mut shown = vec![false; self.items.len()];
        for &i in &self.visible {
            shown[i] = true;
        }
        match self.current {
            Some(c) if shown.get(c).copied().unwrap_or(false) => {
                // Same item, new neighbours: prefetch them.
                fx.extend(self.show_effect());
            }
            _ => {
                let p = old_pos.unwrap_or(0).min(old.len());
                let next = old[p..]
                    .iter()
                    .chain(old[..p].iter().rev())
                    .copied()
                    .find(|&i| shown.get(i).copied().unwrap_or(false))
                    .or(self.visible.first().copied());
                self.current = None;
                self.loupe.shown = None;
                self.loupe.pending_full = false;
                if let Some(n) = next {
                    self.go_to(n, fx);
                }
            }
        }
    }

    /// [`State::refresh_view`] when marks changed and the filter or sort reads them.
    fn refresh_marks_view(&mut self, fx: &mut Vec<Effect>) {
        if self.filter.needs_marks() || self.sort.needs_marks() {
            self.refresh_view(fx);
        }
    }

    /// The visible item after (`forward`) or before the current one, clamped to the ends.
    fn neighbour(&self, forward: bool) -> Option<usize> {
        let p = self.pos()?;
        let q = if forward {
            (p + 1).min(self.visible.len() - 1)
        } else {
            p.saturating_sub(1)
        };
        self.visible.get(q).copied()
    }

    /// Move to item `next` (no-op if it is already current).
    fn go_to(&mut self, next: usize, fx: &mut Vec<Effect>) {
        if self.current == Some(next) || next >= self.items.len() {
            return;
        }
        self.current = Some(next);
        // Never show the previous item's pixels for the new item (PERF-3).
        self.loupe.shown = None;
        self.loupe.pending_full = false;
        if !self.loupe.keep_zoom {
            self.loupe.zoom = Zoom::Fit;
            self.loupe.center = [0.5, 0.5];
        }
        fx.extend(self.show_effect());
    }

    /// The marks `key` gives the current item, `None` if there is no current item.
    fn marks_for_key(&self, key: MarkKey) -> Option<(usize, Marks)> {
        let idx = self.current?;
        let it = self.items.get(idx)?;
        let mut m = self
            .marks
            .get(&it.primary.path)
            .cloned()
            .unwrap_or_default();
        let pick = self.prefs.pick_value;
        match key {
            MarkKey::Stars(n) => m.rating = Some(n.min(5) as i8),
            MarkKey::Pick => m.rating = Some(pick),
            MarkKey::Reject => m.rating = Some(-1),
            // RATE-2: unpick restores 0 only if the rating equals the pick value.
            MarkKey::Unflag => {
                if m.rating == Some(pick) || m.rating == Some(-1) {
                    m.rating = Some(0);
                }
            }
            MarkKey::Label(c) => {
                let name = self.prefs.label_name(c);
                m.label = if m.label.as_deref() == Some(name) {
                    None
                } else {
                    Some(name.to_string())
                };
            }
        }
        Some((idx, m))
    }

    /// Whether an applied edit already changed this field of `path` (so its current value is
    /// Blink's, not the session's original).
    fn touched(&self, path: &std::path::Path, rating: bool) -> bool {
        self.undo
            .iter()
            .any(|e| e.path == path && e.rating_changed() == rating)
    }

    /// Give item `item` the marks `new` (one field changes), record it for undo and emit the
    /// sidecar writes. A change that leaves the marks as they are does nothing.
    fn set_marks(&mut self, item: usize, new: Marks, fx: &mut Vec<Effect>) {
        let Some(it) = self.items.get(item) else {
            return;
        };
        let path = it.primary.path.clone();
        let old = self.marks.get(&path).cloned().unwrap_or_default();
        // "0" on an unrated item changes nothing (no rating means 0, XMP spec).
        let same_rating = old.rating.unwrap_or(0) == new.rating.unwrap_or(0);
        let edits = [
            (!same_rating).then(|| {
                (
                    true,
                    Marks {
                        rating: new.rating,
                        label: old.label.clone(),
                    },
                )
            }),
            (old.label != new.label).then(|| {
                (
                    false,
                    Marks {
                        rating: old.rating,
                        label: new.label.clone(),
                    },
                )
            }),
        ];
        self.conflicts.remove(&path);
        *self.mark_gen.entry(path.clone()).or_default() += 1;
        let mut before = old;
        for (rating, after) in edits.into_iter().flatten() {
            let change = if rating {
                MarkChange::Rating(after.rating)
            } else {
                MarkChange::Label(after.label.clone())
            };
            let writes = sidecar_writes(&self.items, item, &change);
            let targets = writes
                .iter()
                .filter_map(|e| match e {
                    Effect::WriteSidecar {
                        source,
                        owns_adobe_name,
                        ..
                    } => Some((source.clone(), *owns_adobe_name)),
                    _ => None,
                })
                .collect();
            fx.extend(writes);
            let before_original = !self.touched(&path, rating);
            self.undo.push(MarkEdit {
                path: path.clone(),
                rating,
                before: before.clone(),
                after: after.clone(),
                before_original,
                targets,
            });
            self.redo.clear();
            self.marks.insert(path.clone(), after.clone());
            before = after;
        }
    }

    /// Undo (`forward == false`) or redo the latest edit: puts the marks back, writes the
    /// sidecars and moves to the item.
    fn step_history(&mut self, forward: bool, fx: &mut Vec<Effect>) {
        let edit = if forward {
            self.redo.pop()
        } else {
            self.undo.pop()
        };
        let Some(edit) = edit else {
            self.toast = Some(
                if forward {
                    "Nothing to redo"
                } else {
                    "Nothing to undo"
                }
                .into(),
            );
            return;
        };
        let rating = edit.rating_changed();
        let target = if forward { &edit.after } else { &edit.before };
        let change = match (rating, !forward && edit.before_original) {
            (true, true) => MarkChange::RevertRating,
            (false, true) => MarkChange::RevertLabel,
            (true, false) => MarkChange::Rating(target.rating),
            (false, false) => MarkChange::Label(target.label.clone()),
        };
        fx.extend(
            edit.targets
                .iter()
                .map(|(source, owns_adobe_name)| Effect::WriteSidecar {
                    source: source.clone(),
                    change: change.clone(),
                    owns_adobe_name: *owns_adobe_name,
                }),
        );
        *self.mark_gen.entry(edit.path.clone()).or_default() += 1;
        let m = self.marks.entry(edit.path.clone()).or_default();
        if rating {
            m.rating = target.rating;
        } else {
            m.label = target.label.clone();
        }
        self.refresh_marks_view(fx);
        // Back to the item, when the filter shows it.
        if let Some(idx) = self.items.iter().position(|i| i.primary.path == edit.path)
            && self.visible.contains(&idx)
        {
            self.go_to(idx, fx);
        }
        if forward {
            self.undo.push(edit);
        } else {
            self.redo.push(edit);
        }
    }

    /// The effect that shows the current item and prefetches its visible neighbours.
    fn show_effect(&self) -> Option<Effect> {
        let cur = self.current?;
        let pos = self.pos()?;
        let n = self.visible.len();
        let at = |p: usize| ItemRef::of(&self.items[self.visible[p]]);
        let window = prefetch_window(pos, n, self.direction);
        let ahead = (1..=DECODE_AHEAD)
            .map_while(|k| match self.direction {
                Direction::Forward => pos.checked_add(k).filter(|&p| p < n),
                Direction::Backward => pos.checked_sub(k),
            })
            .map(at)
            .collect();
        Some(Effect::Show {
            current: ItemRef::of(self.items.get(cur)?),
            prefetch: window.into_iter().map(at).collect(),
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
    /// A mark key on the current item (RATE-1, RATE-2); auto-advances if that is on.
    Mark(MarkKey),
    /// `Shift+A` (RATE-1).
    ToggleAutoAdvance,
    /// `Ctrl+Z` / `Ctrl+Shift+Z` (RATE-3).
    Undo,
    Redo,
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
    /// A sidecar changed on disk, not by Blink (FILE-8). `conflict`: Blink had unsaved changes
    /// for it and dropped them.
    SidecarChanged {
        sidecar: PathBuf,
        conflict: bool,
    },
    /// Result of [`Effect::ReloadMarks`]: the item's marks as now on disk.
    MarksReloaded {
        key: PathBuf,
        marks: Option<Marks>,
        conflict: bool,
        generation: u64,
    },
    /// Writing the sidecar of `source` failed (FILE-6); the change stays queued for a retry.
    SidecarWriteFailed {
        source: PathBuf,
        error: String,
    },
    /// Capture metadata of items, keyed by primary path (FILT-1, FILT-2).
    MetaLoaded(Vec<(PathBuf, ItemMeta)>),
    /// Add a filter condition, replacing one of the same slot (FILT-1).
    AddFilter(Cond),
    /// Remove the filter condition at this index (its chip's ×).
    RemoveFilter(usize),
    /// Replace the whole filter (the filter field, CLI expressions).
    SetFilter(Filter),
    /// FILT-2.
    SetSort(SortKey),
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
    /// Read the marks of `item` again (its sidecar changed on disk, FILE-8) and answer with
    /// [`Action::MarksReloaded`].
    ReloadMarks {
        item: Item,
        conflict: bool,
        /// The item's [`State::mark_gen`] when the reload was asked for.
        generation: u64,
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
    /// Put `xmp:Rating` back to its value in each sidecar from before Blink first wrote it this
    /// session (undo to the original, RATE-3): pair members may have held different values.
    RevertRating,
    /// The same for `xmp:Label`.
    RevertLabel,
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

/// The same file on a case-insensitive file system: same folder (compared as paths, so either
/// separator works), file names equal ignoring ASCII case.
pub fn same_name_ci(a: &std::path::Path, b: &std::path::Path) -> bool {
    a.parent() == b.parent()
        && match (a.file_name(), b.file_name()) {
            (Some(x), Some(y)) => x.eq_ignore_ascii_case(y),
            _ => false,
        }
}

/// Whether `sidecar` is one of the sidecars `items[item]` writes or reads (FILE-4 names, any
/// case).
fn item_has_sidecar(items: &[Item], item: usize, sidecar: &std::path::Path) -> bool {
    let eq = |p: &std::path::Path| same_name_ci(p, sidecar);
    sidecar_writes(items, item, &MarkChange::RevertLabel)
        .iter()
        .any(|e| match e {
            Effect::WriteSidecar {
                source,
                owns_adobe_name,
                ..
            } => {
                let mut dt = source.as_os_str().to_os_string();
                dt.push(".xmp");
                eq(std::path::Path::new(&dt))
                    || (*owns_adobe_name && eq(&source.with_extension("xmp")))
            }
            _ => false,
        })
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
            // Undo history is per folder: its items are gone (ADR 0016).
            s.undo.clear();
            s.redo.clear();
            s.conflicts.clear();
            s.mark_gen.clear();
            // Filters belong to the folder they were set on; the sort order is kept (ADR 0020).
            s.filter = Filter::default();
            s.meta.clear();
            s.visible.clear();
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
                let listed: std::collections::BTreeSet<PathBuf> =
                    s.items.iter().map(|i| i.primary.path.clone()).collect();
                s.meta.retain(|p, _| listed.contains(p));
                s.visible = s.compute_visible();
                s.current = keep
                    .and_then(|p| s.items.iter().position(|i| i.primary.path == p))
                    .filter(|c| s.visible.contains(c))
                    .or(s.visible.first().copied());
                if s.loupe.shown.as_ref().map(|x| &x.path)
                    != s.current_item().map(|i| &i.primary.path)
                {
                    s.loupe.shown = None;
                    s.loupe.pending_full = false;
                }
                fx.extend(s.show_effect());
            }
        }
        Action::Navigate(nav) => {
            let (next, dir) = match nav {
                Nav::Next => (s.neighbour(true), Direction::Forward),
                Nav::Prev => (s.neighbour(false), Direction::Backward),
                Nav::First => (s.visible.first().copied(), Direction::Backward),
                Nav::Last => (s.visible.last().copied(), Direction::Forward),
            };
            if let Some(next) = next.filter(|_| s.current.is_some()) {
                s.direction = dir;
                s.go_to(next, &mut fx);
            }
        }
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
        Action::SidecarChanged { sidecar, conflict } => {
            for i in 0..s.items.len() {
                if item_has_sidecar(&s.items, i, &sidecar) {
                    let key = &s.items[i].primary.path;
                    fx.push(Effect::ReloadMarks {
                        item: s.items[i].clone(),
                        conflict,
                        generation: s.mark_gen.get(key).copied().unwrap_or(0),
                    });
                }
            }
        }
        Action::MarksReloaded {
            key,
            marks,
            conflict,
            generation,
        } if s.mark_gen.get(&key).copied().unwrap_or(0) != generation => {
            // The user marked the item after the reload was asked for: their change is newer
            // than what was read, and the writer applies it to the new disk version.
            let _ = (marks, conflict);
        }
        Action::MarksReloaded {
            key,
            marks,
            conflict,
            ..
        } => {
            s.marks.insert(key.clone(), marks.unwrap_or_default());
            s.refresh_marks_view(&mut fx);
            // Older edits of this item would undo over the other app's change.
            s.undo.retain(|e| e.path != key);
            s.redo.retain(|e| e.path != key);
            let name = key
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let msg = if conflict {
                s.conflicts.insert(key);
                format!(
                    "Sidecar of {name} changed on disk; kept the on-disk version, dropped Blink's unsaved change"
                )
            } else {
                format!("Sidecar of {name} changed on disk; reloaded")
            };
            s.log.push(msg.clone());
            s.toast = Some(msg);
        }
        Action::SidecarWriteFailed { source, error } => {
            let name = source
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            s.toast = Some(format!("Could not write the sidecar of {name}: {error}"));
        }
        Action::MetaLoaded(list) => {
            // Only items of the open folder and mode: a slower read of a previous folder (or
            // of sub-folders before a recursive toggle) is stale.
            let listed: std::collections::BTreeSet<&PathBuf> =
                s.items.iter().map(|i| &i.primary.path).collect();
            let fresh: Vec<_> = list
                .into_iter()
                .filter(|(p, _)| listed.contains(p))
                .collect();
            s.meta.extend(fresh);
            if s.filter.needs_meta() || s.sort == SortKey::CaptureTime {
                s.refresh_view(&mut fx);
            }
        }
        Action::AddFilter(c) => {
            s.filter = s.filter.with(c);
            s.refresh_view(&mut fx);
        }
        Action::RemoveFilter(i) => {
            if i < s.filter.0.len() {
                s.filter.0.remove(i);
                s.refresh_view(&mut fx);
            }
        }
        Action::SetFilter(f) => {
            s.filter = f;
            s.refresh_view(&mut fx);
        }
        Action::SetSort(k) => {
            s.sort = k;
            s.refresh_view(&mut fx);
        }
        Action::MarksLoaded(list) => {
            // Loading runs off-thread: a field the user already set wins over the file's value.
            for (path, m) in list {
                // An edit made before the load recorded "nothing" as the original value: it
                // is the file's (undo then shows what the revert puts back on disk).
                for ed in s.undo.iter_mut().chain(s.redo.iter_mut()) {
                    if ed.path == path && ed.before_original {
                        if ed.rating_changed() {
                            ed.before.rating = ed.before.rating.or(m.rating);
                        } else {
                            ed.before.label = ed.before.label.take().or(m.label.clone());
                        }
                    }
                }
                let e = s.marks.entry(path).or_default();
                e.rating = e.rating.or(m.rating);
                e.label = e.label.take().or(m.label);
            }
            s.refresh_marks_view(&mut fx);
        }
        Action::SetRating { item, rating } => {
            if let Some(it) = s.items.get(item)
                && (-1..=5).contains(&rating)
            {
                let mut m = s.marks.get(&it.primary.path).cloned().unwrap_or_default();
                m.rating = Some(rating);
                s.set_marks(item, m, &mut fx);
                s.refresh_marks_view(&mut fx);
            }
        }
        Action::SetLabel { item, label } => {
            if let Some(it) = s.items.get(item) {
                let mut m = s.marks.get(&it.primary.path).cloned().unwrap_or_default();
                m.label = label;
                s.set_marks(item, m, &mut fx);
                s.refresh_marks_view(&mut fx);
            }
        }
        Action::Mark(key) => {
            if let Some((idx, m)) = s.marks_for_key(key) {
                // The item after this one before the mark re-sorts or filters it (RATE-1).
                let next = s.neighbour(true).filter(|&n| n != idx);
                s.set_marks(idx, m, &mut fx);
                if let Some(n) = next.filter(|_| s.prefs.auto_advance) {
                    s.direction = Direction::Forward;
                    s.go_to(n, &mut fx);
                }
                s.refresh_marks_view(&mut fx);
            }
        }
        Action::ToggleAutoAdvance => {
            s.prefs.auto_advance = !s.prefs.auto_advance;
            s.toast = Some(
                if s.prefs.auto_advance {
                    "Auto-advance on"
                } else {
                    "Auto-advance off"
                }
                .into(),
            );
        }
        Action::Undo => s.step_history(false, &mut fx),
        Action::Redo => s.step_history(true, &mut fx),
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

    fn rating_of(s: &State, p: &str) -> Option<i8> {
        s.marks.get(Path::new(p)).and_then(|m| m.rating)
    }

    fn label_of(s: &State, p: &str) -> Option<String> {
        s.marks.get(Path::new(p)).and_then(|m| m.label.clone())
    }

    /// Paths compare by component, so a Windows join `/f\a.ARW` equals `/f/a.ARW`.
    fn writes(fx: &[Effect]) -> Vec<(PathBuf, MarkChange)> {
        fx.iter()
            .filter_map(|e| match e {
                Effect::WriteSidecar { source, change, .. } => {
                    Some((source.clone(), change.clone()))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn req_rate_1_each_key_sets_stars_reject_or_a_label() {
        let s = opened(&["a.ARW", "b.ARW"]);
        let a = "/f/a.ARW";
        for n in 0..=5u8 {
            let (s2, fx) = reduce(&s, Action::Mark(MarkKey::Stars(n)));
            if n == 0 {
                // "0" on an unrated item changes nothing.
                assert!(fx.is_empty());
                assert!(s2.undo.is_empty());
            } else {
                assert_eq!(rating_of(&s2, a), Some(n as i8));
                assert_eq!(writes(&fx), [(a.into(), MarkChange::Rating(Some(n as i8)))]);
            }
            assert_eq!(s2.current, Some(0), "auto-advance is off by default");
        }
        let (s2, fx) = reduce(&s, Action::Mark(MarkKey::Reject));
        assert_eq!(rating_of(&s2, a), Some(-1));
        assert_eq!(writes(&fx), [(a.into(), MarkChange::Rating(Some(-1)))]);
        let (s3, _) = reduce(&s2, Action::Mark(MarkKey::Stars(0)));
        assert_eq!(rating_of(&s3, a), Some(0));
        for (c, name) in LabelColor::ALL
            .into_iter()
            .zip(["Red", "Yellow", "Green", "Blue", "Purple"])
        {
            let (s2, fx) = reduce(&s, Action::Mark(MarkKey::Label(c)));
            assert_eq!(label_of(&s2, a).as_deref(), Some(name));
            assert_eq!(
                writes(&fx),
                [(a.into(), MarkChange::Label(Some(name.into())))]
            );
            // The same label key again clears it; another one replaces it.
            let (s3, fx) = reduce(&s2, Action::Mark(MarkKey::Label(c)));
            assert_eq!(label_of(&s3, a), None);
            assert_eq!(writes(&fx), [(a.into(), MarkChange::Label(None))]);
            let other = LabelColor::ALL[(c.index() + 1) % 5];
            let (s4, _) = reduce(&s2, Action::Mark(MarkKey::Label(other)));
            assert_eq!(label_of(&s4, a).as_deref(), Some(s.prefs.label_name(other)));
            // A label never touches the rating and vice versa.
            assert_eq!(rating_of(&s4, a), None);
        }
        // Configured label names are what gets written (SPEC §6).
        let mut s5 = s.clone();
        s5.prefs.label_names[0] = "Rot".into();
        let (s5, fx) = reduce(&s5, Action::Mark(MarkKey::Label(LabelColor::Red)));
        assert_eq!(label_of(&s5, a).as_deref(), Some("Rot"));
        assert_eq!(
            writes(&fx),
            [(a.into(), MarkChange::Label(Some("Rot".into())))]
        );
    }

    #[test]
    fn req_rate_1_auto_advance_moves_to_the_next_item_after_a_mark() {
        let s = opened(&["a.ARW", "b.ARW", "c.ARW"]);
        let (s, _) = reduce(&s, Action::ToggleAutoAdvance);
        assert!(s.prefs.auto_advance);
        assert_eq!(s.toast.as_deref(), Some("Auto-advance on"));
        let (s, fx) = reduce(&s, Action::Mark(MarkKey::Stars(3)));
        assert_eq!(rating_of(&s, "/f/a.ARW"), Some(3));
        assert_eq!(s.current, Some(1));
        assert!(fx.iter().any(
            |e| matches!(e, Effect::Show { current, .. } if current.path == Path::new("/f/b.ARW"))
        ));
        let (s, _) = reduce(&s, Action::Mark(MarkKey::Label(LabelColor::Green)));
        assert_eq!(s.current, Some(2));
        // On the last item it stays put.
        let (s, _) = reduce(&s, Action::Mark(MarkKey::Reject));
        assert_eq!((s.current, rating_of(&s, "/f/c.ARW")), (Some(2), Some(-1)));
        let (s, _) = reduce(&s, Action::ToggleAutoAdvance);
        assert!(!s.prefs.auto_advance);
        let (s, _) = reduce(&s, Action::Navigate(Nav::First));
        let (s, _) = reduce(&s, Action::Mark(MarkKey::Stars(1)));
        assert_eq!(s.current, Some(0));
    }

    #[test]
    fn req_rate_2_pick_uses_the_configured_value_and_unpick_only_clears_it() {
        for pick in 1..=5i8 {
            let mut s = opened(&["a.ARW"]);
            s.prefs.pick_value = pick;
            let a = "/f/a.ARW";
            let (p, fx) = reduce(&s, Action::Mark(MarkKey::Pick));
            assert_eq!(rating_of(&p, a), Some(pick));
            assert_eq!(writes(&fx), [(a.into(), MarkChange::Rating(Some(pick)))]);
            let (u, _) = reduce(&p, Action::Mark(MarkKey::Unflag));
            assert_eq!(rating_of(&u, a), Some(0));
            // A rating other than the pick value survives unpick.
            for other in (1..=5).filter(|&r| r != pick) {
                let (r, _) = reduce(&s, Action::Mark(MarkKey::Stars(other as u8)));
                let (u, fx) = reduce(&r, Action::Mark(MarkKey::Unflag));
                assert_eq!(rating_of(&u, a), Some(other));
                assert!(fx.is_empty());
            }
            // Unflag also clears a reject.
            let (x, _) = reduce(&s, Action::Mark(MarkKey::Reject));
            let (u, _) = reduce(&x, Action::Mark(MarkKey::Unflag));
            assert_eq!(rating_of(&u, a), Some(0));
        }
    }

    #[test]
    fn req_rate_3_undo_and_redo_walk_the_history_and_return_to_the_item() {
        let s = opened(&["a.ARW", "a.JPG", "b.ARW"]);
        let (s, _) = reduce(
            &s,
            Action::MarksLoaded(vec![(
                "/f/a.ARW".into(),
                Marks {
                    rating: Some(2),
                    label: None,
                },
            )]),
        );
        let (s, _) = reduce(&s, Action::Mark(MarkKey::Stars(4)));
        let (s, _) = reduce(&s, Action::Mark(MarkKey::Label(LabelColor::Red)));
        let (s, _) = reduce(&s, Action::Mark(MarkKey::Stars(5)));
        let (s, _) = reduce(&s, Action::Navigate(Nav::Next));
        let (s, _) = reduce(&s, Action::Mark(MarkKey::Reject));
        assert_eq!(s.undo.len(), 4);
        // Undo b's reject: b had no rating, so its sidecar is reverted (RATE-3).
        let (s, fx) = reduce(&s, Action::Undo);
        assert_eq!(writes(&fx), [("/f/b.ARW".into(), MarkChange::RevertRating)]);
        assert_eq!(rating_of(&s, "/f/b.ARW"), None);
        // Undo a's 5: back to 4, which Blink set, so it is written as a value; the view
        // returns to a, and both pair members get the write.
        let (s, fx) = reduce(&s, Action::Undo);
        assert_eq!(s.current, Some(0));
        assert_eq!(
            writes(&fx),
            [
                ("/f/a.ARW".into(), MarkChange::Rating(Some(4))),
                ("/f/a.JPG".into(), MarkChange::Rating(Some(4)))
            ]
        );
        let (s, fx) = reduce(&s, Action::Undo);
        assert_eq!(writes(&fx)[0].1, MarkChange::RevertLabel);
        let (s, fx) = reduce(&s, Action::Undo);
        assert_eq!(writes(&fx)[0].1, MarkChange::RevertRating);
        assert_eq!(rating_of(&s, "/f/a.ARW"), Some(2));
        let (s, fx) = reduce(&s, Action::Undo);
        assert!(fx.is_empty());
        assert_eq!(s.toast.as_deref(), Some("Nothing to undo"));
        // Redo replays values.
        let (s, fx) = reduce(&s, Action::Redo);
        assert_eq!(writes(&fx)[0].1, MarkChange::Rating(Some(4)));
        let (s, _) = reduce(&s, Action::Redo);
        assert_eq!(label_of(&s, "/f/a.ARW").as_deref(), Some("Red"));
        // A new edit drops the redo stack.
        let (s, _) = reduce(&s, Action::Mark(MarkKey::Stars(1)));
        assert!(s.redo.is_empty());
        let (s, _) = reduce(&s, Action::Redo);
        assert_eq!(s.toast.as_deref(), Some("Nothing to redo"));
        // Opening a folder starts a new history.
        let (s, _) = reduce(&s, Action::OpenFolder("/g".into()));
        assert!(s.undo.is_empty() && s.redo.is_empty());
    }

    #[test]
    fn req_rate_3_undo_of_a_mark_set_before_the_marks_loaded_shows_the_files_value() {
        let s = opened(&["a.ARW"]);
        let (s, _) = reduce(&s, Action::Mark(MarkKey::Stars(4)));
        let (s, _) = reduce(
            &s,
            Action::MarksLoaded(vec![(
                "/f/a.ARW".into(),
                Marks {
                    rating: Some(2),
                    label: Some("Red".into()),
                },
            )]),
        );
        assert_eq!(rating_of(&s, "/f/a.ARW"), Some(4));
        let (s, fx) = reduce(&s, Action::Undo);
        assert_eq!(writes(&fx), [("/f/a.ARW".into(), MarkChange::RevertRating)]);
        assert_eq!(rating_of(&s, "/f/a.ARW"), Some(2));
        assert_eq!(label_of(&s, "/f/a.ARW").as_deref(), Some("Red"));
    }

    #[test]
    fn req_rate_3_undo_reaches_items_a_rescan_dropped() {
        let s = opened(&["a.ARW", "b.ARW"]);
        let (s, _) = reduce(&s, Action::Navigate(Nav::Next));
        let (s, _) = reduce(&s, Action::Mark(MarkKey::Stars(3)));
        let (s, _) = reduce(
            &s,
            Action::FolderScanned {
                folder: "/f".into(),
                recursive: false,
                files: scanned("/f", &["a.ARW"]),
            },
        );
        let (_, fx) = reduce(&s, Action::Undo);
        assert_eq!(writes(&fx), [("/f/b.ARW".into(), MarkChange::RevertRating)]);
    }

    #[test]
    fn req_file_8_a_reload_read_before_a_newer_mark_is_ignored() {
        let s = opened(&["a.ARW"]);
        let (s, fx) = reduce(
            &s,
            Action::SidecarChanged {
                sidecar: "/f/a.xmp".into(),
                conflict: true,
            },
        );
        let Some(Effect::ReloadMarks { generation, .. }) = fx.first().cloned() else {
            panic!("{fx:?}")
        };
        let disk = Marks {
            rating: Some(2),
            label: None,
        };
        let reloaded = |generation| Action::MarksReloaded {
            key: "/f/a.ARW".into(),
            marks: Some(disk.clone()),
            conflict: true,
            generation,
        };
        // In time: the disk version is shown with a conflict marker.
        let (r, _) = reduce(&s, reloaded(generation));
        assert_eq!(rating_of(&r, "/f/a.ARW"), Some(2));
        assert!(r.conflicts.contains(Path::new("/f/a.ARW")));
        // The user rated meanwhile: the older reload doesn't override it.
        let (s, _) = reduce(&s, Action::Mark(MarkKey::Stars(5)));
        let (s, _) = reduce(&s, reloaded(generation));
        assert_eq!(rating_of(&s, "/f/a.ARW"), Some(5));
        assert!(s.conflicts.is_empty());
        assert_eq!(s.undo.len(), 1);
    }

    #[test]
    fn req_file_8_sidecar_names_match_across_separators_and_case() {
        let joined = Path::new("/f").join("a.ARW.xmp");
        assert!(same_name_ci(&joined, Path::new("/f/a.arw.XMP")));
        assert!(!same_name_ci(Path::new("/f/a.xmp"), Path::new("/g/a.xmp")));
        assert!(!same_name_ci(Path::new("/f/a.xmp"), Path::new("/f/b.xmp")));
    }

    fn cur_name(s: &State) -> Option<String> {
        s.current_item().map(Item::file_name)
    }

    fn visible_names(s: &State) -> Vec<String> {
        s.visible_items().map(Item::file_name).collect()
    }

    #[test]
    fn req_filt_1_filter_chips_change_what_navigation_steps_through() {
        let s = opened(&["a.ARW", "b.ARW", "c.ARW", "d.ARW", "e.JPG"]);
        let (s, _) = reduce(
            &s,
            Action::MarksLoaded(vec![
                (
                    "/f/b.ARW".into(),
                    Marks {
                        rating: Some(3),
                        label: None,
                    },
                ),
                (
                    "/f/d.ARW".into(),
                    Marks {
                        rating: Some(4),
                        label: None,
                    },
                ),
                (
                    "/f/c.ARW".into(),
                    Marks {
                        rating: Some(-1),
                        label: None,
                    },
                ),
            ]),
        );
        assert_eq!(s.visible.len(), 5);
        // Current `a` fails rating ≥ 3: the next passing item becomes current and is shown.
        let (s, fx) = reduce(&s, Action::AddFilter(Cond::RatingAtLeast(3)));
        assert_eq!(visible_names(&s), ["b.ARW", "d.ARW"]);
        assert_eq!(cur_name(&s).as_deref(), Some("b.ARW"));
        assert!(
            matches!(&fx[..], [Effect::Show { current, .. }] if current.path == Path::new("/f/b.ARW"))
        );
        assert_eq!(s.pos(), Some(0));
        // Navigation skips hidden items and stops at the ends of the visible list.
        let (s, _) = reduce(&s, Action::Navigate(Nav::Next));
        assert_eq!(cur_name(&s).as_deref(), Some("d.ARW"));
        let (s, _) = reduce(&s, Action::Navigate(Nav::Next));
        assert_eq!(cur_name(&s).as_deref(), Some("d.ARW"));
        let (s, _) = reduce(&s, Action::Navigate(Nav::First));
        assert_eq!(cur_name(&s).as_deref(), Some("b.ARW"));
        // A second rating chip replaces the first; a kind chip ANDs with it.
        let (s, _) = reduce(&s, Action::AddFilter(Cond::RatingEq(4)));
        assert_eq!(s.filter.0, [Cond::RatingEq(4)]);
        assert_eq!(cur_name(&s).as_deref(), Some("d.ARW"));
        let (s, _) = reduce(&s, Action::AddFilter(Cond::Kind(FileKind::Jpeg)));
        assert!(s.visible.is_empty());
        assert_eq!(s.current, None);
        assert_eq!(s.loupe.shown, None);
        // Nothing to navigate or mark with an empty view.
        assert!(reduce(&s, Action::Navigate(Nav::Next)).1.is_empty());
        assert!(reduce(&s, Action::Mark(MarkKey::Stars(2))).1.is_empty());
        // Removing chips brings items back; the first visible becomes current.
        let (s, _) = reduce(&s, Action::RemoveFilter(1));
        assert_eq!(cur_name(&s).as_deref(), Some("d.ARW"));
        let (s, _) = reduce(&s, Action::SetFilter(Filter::parse("rejected").unwrap()));
        assert_eq!(visible_names(&s), ["c.ARW"]);
        let (s, _) = reduce(&s, Action::RemoveFilter(0));
        assert_eq!(s.visible.len(), 5);
        assert_eq!(cur_name(&s).as_deref(), Some("c.ARW"));
        // Opening a folder drops the filter.
        let (s, _) = reduce(&s, Action::AddFilter(Cond::Picked));
        let (s, _) = reduce(&s, Action::OpenFolder("/g".into()));
        assert!(s.filter.0.is_empty());
    }

    #[test]
    fn req_filt_1_marking_an_item_out_of_the_filter_moves_on_and_undo_brings_it_back() {
        let s = opened(&["a.ARW", "b.ARW", "c.ARW"]);
        let (s, _) = reduce(&s, Action::AddFilter(Cond::NotRejected));
        // Rejecting the current item hides it; the next one becomes current.
        let (s, fx) = reduce(&s, Action::Mark(MarkKey::Reject));
        assert_eq!(visible_names(&s), ["b.ARW", "c.ARW"]);
        assert_eq!(cur_name(&s).as_deref(), Some("b.ARW"));
        assert!(fx.iter().any(|e| matches!(e, Effect::WriteSidecar { .. })));
        // With auto-advance the item after the marked one is current too (not one further).
        let mut s2 = s.clone();
        s2.prefs.auto_advance = true;
        let (s2, _) = reduce(&s2, Action::Mark(MarkKey::Reject));
        assert_eq!(cur_name(&s2).as_deref(), Some("c.ARW"));
        // Rejecting the last visible item falls back to the one before it.
        let (s3, _) = reduce(&s, Action::Navigate(Nav::Last));
        let (s3, _) = reduce(&s3, Action::Mark(MarkKey::Reject));
        assert_eq!(cur_name(&s3).as_deref(), Some("b.ARW"));
        // Undo un-rejects `c` and returns to it.
        let (s3, _) = reduce(&s3, Action::Undo);
        assert_eq!(visible_names(&s3), ["b.ARW", "c.ARW"]);
        assert_eq!(cur_name(&s3).as_deref(), Some("c.ARW"));
    }

    #[test]
    fn req_filt_2_sort_orders_navigation_and_metadata_resorts_keeping_the_current_item() {
        let s = opened(&["a.ARW", "b.ARW", "c.ARW"]);
        assert_eq!(s.sort, SortKey::CaptureTime);
        // No capture times yet: filename order.
        assert_eq!(visible_names(&s), ["a.ARW", "b.ARW", "c.ARW"]);
        let meta = |p: &str, t: &str| {
            (
                PathBuf::from(p),
                ItemMeta {
                    captured: Some(t.into()),
                    ..Default::default()
                },
            )
        };
        let (s, fx) = reduce(
            &s,
            Action::MetaLoaded(vec![
                meta("/f/a.ARW", "2024:01:01 12:00:02"),
                meta("/f/b.ARW", "2024:01:01 12:00:03"),
                meta("/f/c.ARW", "2024:01:01 12:00:01"),
            ]),
        );
        assert_eq!(visible_names(&s), ["c.ARW", "a.ARW", "b.ARW"]);
        // The current item stays current, with new neighbours to prefetch.
        assert_eq!(cur_name(&s).as_deref(), Some("a.ARW"));
        assert_eq!(s.pos(), Some(1));
        let Some(Effect::Show { prefetch, .. }) = fx.first() else {
            panic!("{fx:?}")
        };
        assert_eq!(prefetch[0].path, Path::new("/f/b.ARW"));
        let (s, _) = reduce(&s, Action::Navigate(Nav::Prev));
        assert_eq!(cur_name(&s).as_deref(), Some("c.ARW"));
        // Rating sort: highest first, re-sorted when a rating changes; ties by filename.
        let (s, _) = reduce(&s, Action::SetSort(SortKey::Rating));
        assert_eq!(visible_names(&s), ["a.ARW", "b.ARW", "c.ARW"]);
        let (s, _) = reduce(&s, Action::Mark(MarkKey::Stars(5)));
        assert_eq!(visible_names(&s), ["c.ARW", "a.ARW", "b.ARW"]);
        assert_eq!(cur_name(&s).as_deref(), Some("c.ARW"));
        let (s, _) = reduce(&s, Action::SetSort(SortKey::FileName));
        assert_eq!(visible_names(&s), ["a.ARW", "b.ARW", "c.ARW"]);
        // Metadata for files not listed (a previous folder's slower read) is dropped.
        let (s, _) = reduce(
            &s,
            Action::MetaLoaded(vec![meta("/g/x.ARW", "2024:01:01 00:00:00")]),
        );
        assert!(!s.meta.contains_key(Path::new("/g/x.ARW")));
        // A rescan keeps the sort and drops metadata of items that are gone.
        let (s, _) = reduce(
            &s,
            Action::FolderScanned {
                folder: "/f".into(),
                recursive: false,
                files: scanned("/f", &["c.ARW", "a.ARW"]),
            },
        );
        assert_eq!(visible_names(&s), ["a.ARW", "c.ARW"]);
        assert_eq!(cur_name(&s).as_deref(), Some("c.ARW"));
        assert!(!s.meta.contains_key(Path::new("/f/b.ARW")));
    }
}
