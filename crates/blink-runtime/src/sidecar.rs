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

/// Changes waiting for the debounce, for one source file.
#[derive(Debug, Clone)]
struct Pending {
    owns_adobe_name: bool,
    rating: Option<Option<i8>>,
    label: Option<Option<String>>,
    since: Instant,
    /// Failed attempts so far (retries back off).
    attempts: u32,
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
        let p = self.pending.entry(source).or_insert(Pending {
            owns_adobe_name,
            rating: None,
            label: None,
            since: now,
            attempts: 0,
        });
        p.owns_adobe_name = owns_adobe_name;
        match change {
            MarkChange::Rating(r) => p.rating = Some(r),
            MarkChange::Label(l) => p.label = Some(l),
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
        let Some(p) = self.pending.remove(&source) else {
            return WriteReport {
                source,
                sidecars: vec![],
                result: Ok(()),
            };
        };
        let targets = sidecar_targets(self.fs, &source, self.style, p.owns_adobe_name);
        let mut result = Ok(());
        for t in &targets {
            if let Err(e) = self.apply(t, &p) {
                result = Err(format!("{}: {e}", t.display()));
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
        let edit = OwnedEdit {
            rating: match p.rating {
                None => Edit::Keep,
                Some(Some(r)) => Edit::Set(r),
                Some(None) => Edit::Remove,
            },
            label: match &p.label {
                None => Edit::Keep,
                Some(Some(l)) => Edit::Set(l.clone()),
                Some(None) => Edit::Remove,
            },
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
        if r.is_ok() {
            let now_on_disk = if restore { base } else { Some(new) };
            self.last_written.insert(target.to_path_buf(), now_on_disk);
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
}

/// The GUI's sidecar writer: a [`SidecarWriter`] on its own I/O thread (SPEC §8.3), so the UI
/// thread never waits for a disk. Queued changes are written [`DEBOUNCE`] after the first one;
/// dropping the service writes everything still queued.
pub struct SidecarService {
    tx: Option<std::sync::mpsc::Sender<Msg>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl SidecarService {
    /// Start the writer thread. `on_report` gets the outcome of every write.
    pub fn spawn(
        fs: std::sync::Arc<dyn Fs>,
        clock: std::sync::Arc<dyn Clock>,
        style: SidecarStyle,
        on_report: impl Fn(WriteReport) + Send + 'static,
    ) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<Msg>();
        let handle = std::thread::Builder::new()
            .name("blink-sidecars".into())
            .spawn(move || {
                let mut w = SidecarWriter::new(fs.as_ref(), clock.as_ref(), style);
                loop {
                    let msg = match w.next_due() {
                        None => rx.recv().ok(),
                        Some(due) => {
                            let wait = due.saturating_duration_since(Instant::now());
                            match rx.recv_timeout(wait) {
                                Ok(m) => Some(m),
                                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                                    for r in w.flush_due(Instant::now()) {
                                        on_report(r);
                                    }
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
                            for r in w.flush_all() {
                                on_report(r);
                            }
                            let _ = done.send(());
                        }
                        None => {
                            for r in w.flush_all() {
                                on_report(r);
                            }
                            return;
                        }
                    }
                    for r in w.flush_due(Instant::now()) {
                        on_report(r);
                    }
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
        if let Some(tx) = &self.tx {
            let _ = tx.send(Msg::Queue {
                source,
                change,
                owns_adobe_name,
            });
        }
    }

    /// Write everything queued now and wait until it is on disk.
    pub fn flush(&self) {
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        if let Some(tx) = &self.tx
            && tx.send(Msg::Flush(done_tx)).is_ok()
        {
            let _ = done_rx.recv();
        }
    }
}

impl Drop for SidecarService {
    fn drop(&mut self) {
        self.tx.take();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
