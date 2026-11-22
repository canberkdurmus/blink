//! Execute [`Effect::WriteSidecar`]: sidecar naming (FILE-4), foreign-content-preserving rewrite
//! (FILE-5), atomic writes (FILE-6), `xmp:MetadataDate` (FILE-9) and the rating-write debounce.
//!
//! Byte-exact undo (RATE-3): the writer remembers each sidecar's bytes (or absence) from before
//! Blink first wrote it this session. Whenever the owned fields come back to those original
//! values, the original bytes are restored (or the sidecar Blink created is removed), so undoing
//! everything leaves every sidecar byte for byte as it was.
//!
//! [`Effect::WriteSidecar`]: blink_core::state::Effect::WriteSidecar

use crate::atomic::{remove_sidecar, write_atomic};
use crate::fs::Fs;
use blink_core::state::MarkChange;
use blink_xmp::{Edit, OwnedEdit};
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Pending rating/label changes are written this long after the first one (FILE-6: ≤ 250 ms;
/// PERF-7: durable within 250 ms of the key, so the write itself has the remaining time).
pub const DEBOUNCE: Duration = Duration::from_millis(100);

/// Style of a sidecar Blink creates when a file has none (D-3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SidecarStyle {
    /// `IMG_0001.xmp` (default).
    #[default]
    Adobe,
    /// `IMG_0001.ARW.xmp`.
    Darktable,
}

/// Wall clock for `xmp:MetadataDate` (SPEC §8.1: the clock sits behind a trait).
pub trait Clock: Send + Sync {
    /// Seconds since the Unix epoch.
    fn unix_secs(&self) -> i64;
}

/// The system clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn unix_secs(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64)
    }
}

/// A fixed clock for tests.
#[derive(Debug, Clone, Copy)]
pub struct FixedClock(pub i64);

impl Clock for FixedClock {
    fn unix_secs(&self) -> i64 {
        self.0
    }
}

/// Adobe-style sidecar path: `IMG.ARW` → `IMG.xmp`.
pub fn adobe_path(source: &Path, ext: &str) -> PathBuf {
    source.with_extension(ext)
}

/// darktable-style sidecar path: `IMG.ARW` → `IMG.ARW.xmp`.
pub fn darktable_path(source: &Path, ext: &str) -> PathBuf {
    // Append to the whole path: `with_file_name` would re-join with the platform separator
    // (`/f\a.ARW.xmp` on Windows).
    let mut s = source.as_os_str().to_os_string();
    s.push(".");
    s.push(ext);
    PathBuf::from(s)
}

fn existing(fs: &dyn Fs, make: fn(&Path, &str) -> PathBuf, source: &Path) -> Option<PathBuf> {
    // `.xmp` before `.XMP`; on a case-insensitive file system both name one file, so stop at
    // the first hit.
    ["xmp", "XMP"]
        .into_iter()
        .map(|e| make(source, e))
        .find(|p| fs.metadata(p).is_ok_and(|m| !m.is_dir))
}

/// The sidecars to write for `source` (FILE-4): every existing one (Adobe-style and/or
/// darktable-style; both if both exist), else a new one in `style`. A file that doesn't own the
/// Adobe-style name (a pair companion, ADR 0015) only ever uses the darktable-style name.
pub fn sidecar_targets(
    fs: &dyn Fs,
    source: &Path,
    style: SidecarStyle,
    owns_adobe_name: bool,
) -> Vec<PathBuf> {
    let adobe = owns_adobe_name
        .then(|| existing(fs, adobe_path, source))
        .flatten();
    let dt = existing(fs, darktable_path, source);
    let found: Vec<PathBuf> = adobe.into_iter().chain(dt).collect();
    if !found.is_empty() {
        return found;
    }
    vec![match style {
        SidecarStyle::Adobe if owns_adobe_name => adobe_path(source, "xmp"),
        _ => darktable_path(source, "xmp"),
    }]
}

/// Whether `sidecar` is one of the names `source` writes (FILE-4), on any file system case.
pub fn belongs_to(sidecar: &Path, source: &Path, owns_adobe_name: bool) -> bool {
    let eq = |p: PathBuf| blink_core::state::same_name_ci(&p, sidecar);
    eq(darktable_path(source, "xmp")) || (owns_adobe_name && eq(adobe_path(source, "xmp")))
}

/// A sidecar changed on disk, not by Blink (FILE-8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalChange {
    pub sidecar: PathBuf,
    /// Blink had unsaved (queued) changes for these sources; they were dropped, so the on-disk
    /// version wins.
    pub dropped: Vec<PathBuf>,
}

impl ExternalChange {
    /// Blink had unsaved changes for this sidecar: show a conflict marker and log it.
    pub fn conflict(&self) -> bool {
        !self.dropped.is_empty()
    }
}

/// A queued change of one owned field.
#[derive(Debug, Clone)]
enum Want<T> {
    /// Set (`Some`) or remove (`None`).
    Value(Option<T>),
    /// Back to the sidecar's value from before Blink first wrote it this session (RATE-3).
    Original,
}

/// Changes waiting for the debounce, for one source file.
#[derive(Debug, Clone)]
struct Pending {
    owns_adobe_name: bool,
    rating: Option<Want<i8>>,
    label: Option<Want<String>>,
    since: Instant,
    /// Failed attempts so far (retries back off).
    attempts: u32,
    /// The sidecars as they were when the first change was queued: if one differs at write time,
    /// another app changed it meanwhile and the on-disk version wins (FILE-8).
    seen: BTreeMap<PathBuf, Option<Vec<u8>>>,
}

/// Outcome of writing one source file's sidecars.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteReport {
    pub source: PathBuf,
    /// Sidecars written, restored or removed.
    pub sidecars: Vec<PathBuf>,
    /// `Err`: nothing was changed for the failing sidecar (an unparseable sidecar is never
    /// overwritten; a failed atomic write leaves the old file). The changes stay queued.
    pub result: Result<(), String>,
}

/// Queues, coalesces and writes sidecar changes. Not thread-safe on its own; the GUI runs it on
/// its I/O thread ([`crate::sidecar_service`]), the CLI calls [`SidecarWriter::flush_all`].
pub struct SidecarWriter<'a> {
    fs: &'a dyn Fs,
    clock: &'a dyn Clock,
    pub style: SidecarStyle,
    pending: BTreeMap<PathBuf, Pending>,
    /// Bytes of each sidecar before Blink first wrote it this session (`None`: didn't exist).
    baseline: BTreeMap<PathBuf, Option<Vec<u8>>>,
    /// Bytes Blink last wrote to each sidecar (`None`: it removed it). A file that no longer
    /// holds them was changed by another app, so it is never restored over.
    last_written: BTreeMap<PathBuf, Option<Vec<u8>>>,
    /// External changes found at write time (see [`SidecarWriter::take_external`]).
    external: Vec<ExternalChange>,
}

/// Re-queue a failed write: retries back off from `DEBOUNCE` up to 30 s, so a sidecar that
/// can't be written (unparseable, read-only folder) doesn't keep the I/O thread busy.
fn retry(p: Pending, now: Instant) -> Pending {
    let attempts = p.attempts + 1;
    let wait = (DEBOUNCE * 2u32.saturating_pow(attempts.min(9))).min(Duration::from_secs(30));
    Pending {
        since: now + wait - DEBOUNCE,
        attempts,
        ..p
    }
}

fn read_opt(fs: &dyn Fs, p: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs.read(p) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

fn owned_of(b: Option<&[u8]>) -> Option<blink_xmp::OwnedFields> {
    match b {
        None => Some(Default::default()),
        Some(b) => blink_xmp::read_owned(b).ok(),
    }
}

impl<'a> SidecarWriter<'a> {
    pub fn new(fs: &'a dyn Fs, clock: &'a dyn Clock, style: SidecarStyle) -> Self {
        Self {
            fs,
            clock,
            style,
            pending: BTreeMap::new(),
            baseline: BTreeMap::new(),
            last_written: BTreeMap::new(),
            external: Vec::new(),
        }
    }

    /// Queue a change; later changes to the same field of the same file replace earlier ones.
    pub fn queue(
        &mut self,
        source: PathBuf,
        change: MarkChange,
        owns_adobe_name: bool,
        now: Instant,
    ) {
        if !self.pending.contains_key(&source) {
            let seen = sidecar_targets(self.fs, &source, self.style, owns_adobe_name)
                .into_iter()
                .map(|t| {
                    let b = read_opt(self.fs, &t).ok().flatten();
                    (t, b)
                })
                .collect();
            self.pending.insert(
                source.clone(),
                Pending {
                    owns_adobe_name,
                    rating: None,
                    label: None,
                    since: now,
                    attempts: 0,
                    seen,
                },
            );
        }
        let Some(p) = self.pending.get_mut(&source) else {
            return;
        };
        p.owns_adobe_name = owns_adobe_name;
        match change {
            MarkChange::Rating(r) => p.rating = Some(Want::Value(r)),
            MarkChange::Label(l) => p.label = Some(Want::Value(l)),
            MarkChange::RevertRating => p.rating = Some(Want::Original),
            MarkChange::RevertLabel => p.label = Some(Want::Original),
        }
    }

    /// When the oldest queued change is due.
    pub fn next_due(&self) -> Option<Instant> {
        self.pending.values().map(|p| p.since + DEBOUNCE).min()
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Whether changes for `source` are queued.
    pub fn is_pending(&self, source: &Path) -> bool {
        self.pending.contains_key(source)
    }

    /// Drop queued changes for `source` (FILE-8: the on-disk version wins a conflict).
    pub fn discard(&mut self, source: &Path) -> bool {
        self.pending.remove(source).is_some()
    }

    /// `sidecar` changed on disk (a watcher event). `None` if it holds what Blink last wrote (its
    /// own write) or isn't a sidecar. Otherwise the new bytes become the reference for later
    /// writes and restores, and queued changes of the files it belongs to are dropped: the
    /// on-disk version wins (FILE-8).
    pub fn external_change(&mut self, sidecar: &Path) -> Option<ExternalChange> {
        if !crate::atomic::is_sidecar_path(sidecar) {
            return None;
        }
        // Event paths may differ in case from the scan's on case-insensitive file systems: use
        // the name Blink knows the sidecar by.
        let sidecar = self
            .last_written
            .keys()
            .chain(self.baseline.keys())
            .find(|k| blink_core::state::same_name_ci(k, sidecar))
            .cloned()
            .unwrap_or_else(|| sidecar.to_path_buf());
        let sidecar = sidecar.as_path();
        let current = read_opt(self.fs, sidecar).ok()?;
        if self.last_written.get(sidecar) == Some(&current) {
            return None;
        }
        self.baseline.insert(sidecar.to_path_buf(), current.clone());
        self.last_written.insert(sidecar.to_path_buf(), current);
        let dropped: Vec<PathBuf> = self
            .pending
            .iter()
            .filter(|(src, p)| belongs_to(sidecar, src, p.owns_adobe_name))
            .map(|(src, _)| src.clone())
            .collect();
        for d in &dropped {
            self.pending.remove(d);
        }
        Some(ExternalChange {
            sidecar: sidecar.to_path_buf(),
            dropped,
        })
    }

    /// Blink moved (or, with `to == None`, trashed) these files (FILE-1). Sidecars among them
    /// keep their session state under the new name: the baseline an undo restores, and the bytes
    /// Blink last wrote, so the watcher sees the move as Blink's own change, not another app's.
    pub fn moved(&mut self, moves: &[(PathBuf, Option<PathBuf>)]) {
        for (from, to) in moves {
            if !crate::atomic::is_sidecar_path(from) {
                continue;
            }
            let held = match self.last_written.remove(from) {
                Some(b) => b,
                None => to
                    .as_ref()
                    .and_then(|t| read_opt(self.fs, t).ok().flatten()),
            };
            let base = self.baseline.remove(from);
            // Blink made `from` absent.
            self.last_written.insert(from.clone(), None);
            if let Some(to) = to {
                self.last_written.insert(to.clone(), held);
                if let Some(b) = base {
                    self.baseline.insert(to.clone(), b);
                }
            }
        }
    }

    /// External changes the writer found when it was about to write (FILE-8); the GUI handles
    /// them like watcher events.
    pub fn take_external(&mut self) -> Vec<ExternalChange> {
        std::mem::take(&mut self.external)
    }

    /// Write everything that is due at `now`.
    pub fn flush_due(&mut self, now: Instant) -> Vec<WriteReport> {
        let due: Vec<PathBuf> = self
            .pending
            .iter()
            .filter(|(_, p)| p.since + DEBOUNCE <= now)
            .map(|(k, _)| k.clone())
            .collect();
        due.into_iter().map(|s| self.flush_one(s, now)).collect()
    }

    /// Write everything now (on exit, and in the CLI).
    pub fn flush_all(&mut self) -> Vec<WriteReport> {
        let all: Vec<PathBuf> = self.pending.keys().cloned().collect();
        let now = Instant::now();
        all.into_iter().map(|s| self.flush_one(s, now)).collect()
    }

    fn flush_one(&mut self, source: PathBuf, now: Instant) -> WriteReport {
        let Some(mut p) = self.pending.remove(&source) else {
            return WriteReport {
                source,
                sidecars: vec![],
                result: Ok(()),
            };
        };
        let targets = sidecar_targets(self.fs, &source, self.style, p.owns_adobe_name);
        // Changed on disk since the change was queued (and the watcher hasn't said so yet):
        // keep the on-disk version, drop Blink's unsaved change (FILE-8).
        // (A read error is no evidence: the write below fails and is retried.)
        let changed = targets.iter().find(|t| {
            read_opt(self.fs, t).is_ok_and(|now| p.seen.get(*t).cloned().flatten() != now)
        });
        if let Some(t) = changed.cloned() {
            let current = read_opt(self.fs, &t).ok().flatten();
            self.baseline.insert(t.clone(), current.clone());
            self.last_written.insert(t.clone(), current);
            self.external.push(ExternalChange {
                sidecar: t,
                dropped: vec![source.clone()],
            });
            return WriteReport {
                source,
                sidecars: vec![],
                result: Ok(()),
            };
        }
        let mut result = Ok(());
        for t in &targets {
            if let Err(e) = self.apply(t, &p) {
                result = Err(format!("{}: {e}", t.display()));
            }
            // What Blink put on disk is not another app's change, also when a later step of
            // the write failed (a retry must not see a conflict).
            if let Some(w) = self.last_written.get(t) {
                p.seen.insert(t.clone(), w.clone());
            }
        }
        if result.is_err() {
            // Keep the changes; the next flush retries (a sidecar written meanwhile is fine:
            // rewriting it with the same values is idempotent apart from MetadataDate).
            self.pending.insert(source.clone(), retry(p, now));
        }
        WriteReport {
            source,
            sidecars: targets,
            result,
        }
    }

    fn apply(&mut self, target: &Path, p: &Pending) -> Result<(), String> {
        let current = read_opt(self.fs, target).map_err(|e| e.to_string())?;
        let base = self
            .baseline
            .entry(target.to_path_buf())
            .or_insert_with(|| current.clone())
            .clone();
        // `Original` needs the field's value in the session's first version of this sidecar.
        let original = if matches!(p.rating, Some(Want::Original))
            || matches!(p.label, Some(Want::Original))
        {
            owned_of(base.as_deref()).ok_or("the original sidecar can't be parsed")?
        } else {
            Default::default()
        };
        fn edit<T: Clone>(w: &Option<Want<T>>, original: &Option<T>) -> Edit<T> {
            let v = match w {
                None => return Edit::Keep,
                Some(Want::Value(v)) => v,
                Some(Want::Original) => original,
            };
            match v {
                Some(v) => Edit::Set(v.clone()),
                None => Edit::Remove,
            }
        }
        let edit = OwnedEdit {
            rating: edit(&p.rating, &original.rating),
            label: edit(&p.label, &original.label),
            metadata_date: blink_xmp::iso8601_utc(self.clock.unix_secs()),
        };
        let new = blink_xmp::write_owned(current.as_deref(), &edit).map_err(|e| e.to_string())?;
        // Back at the original values: restore the original bytes exactly (RATE-3).
        // ... but only if the file is still what Blink left there: never over another app's
        // later changes (FILE-5).
        let ours = match self.last_written.get(target) {
            Some(w) => *w == current,
            None => current == base,
        };
        let restore =
            ours && owned_of(base.as_deref()).is_some_and(|b| Some(b) == owned_of(Some(&new)));
        let fs = self.fs;
        let r = match (restore, &base) {
            (true, None) if current.is_some() => remove_sidecar(fs, target),
            (true, None) => Ok(()),
            (true, Some(b)) if current.as_ref() == Some(b) => Ok(()),
            (true, Some(b)) => write_atomic(fs, target, b),
            (false, _) => write_atomic(fs, target, &new),
        };
        let intended = if restore { base } else { Some(new) };
        // Also after a failure: the rename may have happened before a later step (the
        // directory fsync) failed, and then these bytes are Blink's, not another app's.
        if r.is_ok() || read_opt(fs, target).is_ok_and(|now| now == intended) {
            self.last_written.insert(target.to_path_buf(), intended);
        }
        r.map_err(|e| e.to_string())
    }
}

enum Msg {
    Queue {
        source: PathBuf,
        change: MarkChange,
        owns_adobe_name: bool,
    },
    Flush(std::sync::mpsc::Sender<()>),
    /// Watch a folder for sidecar changes (FILE-8); `None` stops watching.
    Watch(Option<(PathBuf, bool)>),
    /// Paths a watcher reported.
    Changed(Vec<PathBuf>),
    /// Run a file operation after writing everything queued (FILE-1), then report it.
    FileOp(
        blink_core::state::FileOp,
        Box<dyn FnOnce(crate::files::Outcome) + Send>,
    ),
    /// Write what is queued and stop (the thread holds a sender for its watcher, so the channel
    /// alone never disconnects).
    Shutdown,
}

/// What the service reports back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidecarEvent {
    Written(WriteReport),
    External(ExternalChange),
}

/// The GUI's sidecar writer: a [`SidecarWriter`] on its own I/O thread (SPEC §8.3), so the UI
/// thread never waits for a disk. Queued changes are written [`DEBOUNCE`] after the first one;
/// dropping the service writes everything still queued. It also watches the open folder and
/// reports sidecars that other apps change (FILE-8).
pub struct SidecarService {
    tx: Option<std::sync::mpsc::Sender<Msg>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl SidecarService {
    /// Start the writer thread. `on_event` gets the outcome of every write and every external
    /// change.
    pub fn spawn(
        fs: std::sync::Arc<dyn Fs>,
        clock: std::sync::Arc<dyn Clock>,
        style: SidecarStyle,
        on_event: impl Fn(SidecarEvent) + Send + 'static,
    ) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<Msg>();
        let watch_tx = tx.clone();
        let handle = std::thread::Builder::new()
            .name("blink-sidecars".into())
            .spawn(move || {
                let mut w = SidecarWriter::new(fs.as_ref(), clock.as_ref(), style);
                let mut watcher: Option<notify::RecommendedWatcher> = None;
                let report = |w: &mut SidecarWriter, all: bool| {
                    let reps = if all {
                        w.flush_all()
                    } else {
                        w.flush_due(Instant::now())
                    };
                    for r in reps {
                        on_event(SidecarEvent::Written(r));
                    }
                    for c in w.take_external() {
                        on_event(SidecarEvent::External(c));
                    }
                };
                loop {
                    let msg = match w.next_due() {
                        None => rx.recv().ok(),
                        Some(due) => {
                            let wait = due.saturating_duration_since(Instant::now());
                            match rx.recv_timeout(wait) {
                                Ok(m) => Some(m),
                                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                                    report(&mut w, false);
                                    continue;
                                }
                                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => None,
                            }
                        }
                    };
                    match msg {
                        Some(Msg::Queue {
                            source,
                            change,
                            owns_adobe_name,
                        }) => w.queue(source, change, owns_adobe_name, Instant::now()),
                        Some(Msg::Flush(done)) => {
                            report(&mut w, true);
                            let _ = done.send(());
                        }
                        Some(Msg::Watch(target)) => {
                            watcher =
                                target.and_then(|(dir, rec)| watch(&dir, rec, watch_tx.clone()));
                        }
                        Some(Msg::FileOp(op, done)) => {
                            // Queued writes land before their files move; the moves are
                            // recorded before any watcher event for them is handled.
                            report(&mut w, true);
                            let out = crate::files::run(fs.as_ref(), &op);
                            let moved: Vec<(PathBuf, Option<PathBuf>)> = out
                                .moves
                                .iter()
                                .map(|(a, b)| (a.clone(), Some(b.clone())))
                                .chain(out.trashed.iter().map(|a| (a.clone(), None)))
                                .collect();
                            w.moved(&moved);
                            done(out);
                        }
                        Some(Msg::Changed(paths)) => {
                            for p in paths {
                                if let Some(c) = w.external_change(&p) {
                                    on_event(SidecarEvent::External(c));
                                }
                            }
                        }
                        Some(Msg::Shutdown) | None => {
                            drop(watcher.take());
                            report(&mut w, true);
                            return;
                        }
                    }
                    report(&mut w, false);
                }
            })
            .ok();
        Self {
            tx: Some(tx),
            handle,
        }
    }

    /// Queue a change from an [`Effect::WriteSidecar`](blink_core::state::Effect::WriteSidecar).
    pub fn queue(&self, source: PathBuf, change: MarkChange, owns_adobe_name: bool) {
        self.send(Msg::Queue {
            source,
            change,
            owns_adobe_name,
        });
    }

    /// Watch `folder` (and its sub-folders if `recursive`) for sidecar changes; replaces the
    /// previous watch.
    pub fn watch(&self, folder: PathBuf, recursive: bool) {
        self.send(Msg::Watch(Some((folder, recursive))));
    }

    /// Run a file operation on the sidecar thread (FILE-1): queued sidecar writes first, then
    /// the moves; `done` gets the outcome on that thread.
    pub fn file_op(
        &self,
        op: blink_core::state::FileOp,
        done: impl FnOnce(crate::files::Outcome) + Send + 'static,
    ) {
        self.send(Msg::FileOp(op, Box::new(done)));
    }

    /// Report that `paths` changed, as a watcher would (tests; file systems without events).
    pub fn changed(&self, paths: Vec<PathBuf>) {
        self.send(Msg::Changed(paths));
    }

    fn send(&self, m: Msg) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(m);
        }
    }

    /// Write everything queued now and wait until it is on disk. Also waits for every message
    /// sent before it (watch events included) to be handled.
    pub fn flush(&self) {
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        if let Some(tx) = &self.tx
            && tx.send(Msg::Flush(done_tx)).is_ok()
        {
            let _ = done_rx.recv();
        }
    }
}

/// A notify watcher that forwards sidecar paths to the service thread. `None` if the folder
/// can't be watched (then external changes are noticed only when Blink next writes: the
/// writer never restores over a file that no longer holds what it wrote).
fn watch(
    dir: &Path,
    recursive: bool,
    tx: std::sync::mpsc::Sender<Msg>,
) -> Option<notify::RecommendedWatcher> {
    use notify::Watcher;
    // Some backends (FSEvents) report real paths: `/private/var/…` for `/var/…`, a symlinked
    // volume's target. Map them back onto the folder as opened, which is how the scan, the
    // writer and the reducer name every file.
    let opened = dir.to_path_buf();
    let real = std::fs::canonicalize(dir).unwrap_or_else(|_| opened.clone());
    let map = move |p: PathBuf| match p.strip_prefix(&real) {
        Ok(rest) if real != opened => opened.join(rest),
        _ => p,
    };
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res
            && !matches!(ev.kind, notify::EventKind::Access(_))
        {
            let paths: Vec<PathBuf> = ev
                .paths
                .into_iter()
                .filter(|p| crate::atomic::is_sidecar_path(p))
                .map(&map)
                .collect();
            if !paths.is_empty() {
                let _ = tx.send(Msg::Changed(paths));
            }
        }
    })
    .ok()?;
    let mode = if recursive {
        notify::RecursiveMode::Recursive
    } else {
        notify::RecursiveMode::NonRecursive
    };
    w.watch(dir, mode).ok()?;
    Some(w)
}

impl Drop for SidecarService {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Msg::Shutdown);
        }
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
