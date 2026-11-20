//! Image loader: executes [`Effect::Show`] and [`Effect::LoadFull`] (SPEC §8.3).
//!
//! - Decode workers (cores − 1, min 1) take jobs from a priority queue: the current item's
//!   preview first, then the prefetch window in order. A new `Show` replaces the queue and bumps
//!   the generation; a job checks its generation (its cancellation token) before decoding, so
//!   stale jobs are dropped.
//! - Full-resolution loads (paired JPEG or RAW decode, VIEW-4) run on their own thread, newest
//!   request wins, so a long RAW decode never delays previews.
//! - Results are cached (L2, byte-limited LRU) together with a UI-specific prepared form `T`
//!   (e.g. texture tiles), produced on the worker by the `prepare` hook so the UI thread never
//!   converts pixels. The UI receives [`Action::ImageReady`] / [`Action::ImageFailed`] through
//!   [`Loader::poll`] and is woken through the `wake` hook.
//!
//! All file reads go through [`Fs`].
//!
//! [`Effect::Show`]: blink_core::state::Effect::Show
//! [`Effect::LoadFull`]: blink_core::state::Effect::LoadFull

use crate::fs::Fs;
use blink_core::formats::FileKind;
use blink_core::preview::PreviewSource;
use blink_core::state::{Action, Effect, ItemRef, Level};
use blink_decode::raw::RawBackend;
use blink_decode::stats::{Histogram, histogram};
use blink_decode::{Analysis, Preview};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

/// Default L2 cache cap (PERF-8): 2 GB.
pub const DEFAULT_CACHE_BYTES: usize = 2 << 30;

/// One loaded image.
#[derive(Debug)]
pub struct Loaded<T> {
    pub path: PathBuf,
    pub level: Level,
    pub source: PreviewSource,
    /// Displayed pixel size (after orientation).
    pub size: (u32, u32),
    /// Full-resolution size the zoom refers to (after orientation).
    pub full_size: (u32, u32),
    /// Histogram of the displayed pixels (VIEW-6).
    pub histogram: Histogram,
    /// The UI's prepared form of the pixels.
    pub prepared: T,
    /// Decoded RGB pixels, kept for the clipping overlay and tests.
    pub preview: Arc<Preview>,
}

impl<T> Loaded<T> {
    fn bytes(&self) -> usize {
        // RGB pixels plus (typically) RGBA texture data.
        self.size.0 as usize * self.size.1 as usize * 7
    }

    fn ready(&self) -> Action {
        Action::ImageReady {
            path: self.path.clone(),
            level: self.level,
            source: self.source,
            size: self.size,
            full_size: self.full_size,
        }
    }
}

type Prepare<T> = dyn Fn(&Preview) -> T + Send + Sync;
type Key = (PathBuf, Level);

struct Cache<T> {
    map: HashMap<Key, (Arc<Loaded<T>>, u64)>,
    tick: u64,
    bytes: usize,
    cap: usize,
}

impl<T> Cache<T> {
    fn get(&mut self, k: &Key) -> Option<Arc<Loaded<T>>> {
        self.tick += 1;
        let t = self.tick;
        self.map.get_mut(k).map(|(v, used)| {
            *used = t;
            v.clone()
        })
    }

    fn insert(&mut self, v: Arc<Loaded<T>>, pinned: &[Key]) {
        self.tick += 1;
        let k = (v.path.clone(), v.level);
        self.bytes += v.bytes();
        if let Some((old, _)) = self.map.insert(k, (v, self.tick)) {
            self.bytes -= old.bytes();
        }
        while self.bytes > self.cap {
            // Evict the least recently used entry that isn't pinned (current item).
            let victim = self
                .map
                .iter()
                .filter(|(k, _)| !pinned.contains(k))
                .min_by_key(|(_, (_, used))| *used)
                .map(|(k, _)| k.clone());
            let Some(k) = victim else { break };
            if let Some((old, _)) = self.map.remove(&k) {
                self.bytes -= old.bytes();
            }
        }
    }
}

struct Job {
    item: ItemRef,
    generation: u64,
}

struct Shared<T> {
    fs: Arc<dyn Fs>,
    backend: Arc<dyn RawBackend>,
    prepare: Box<Prepare<T>>,
    wake: Box<dyn Fn() + Send + Sync>,
    queue: Mutex<Queue>,
    queue_cv: Condvar,
    full: Mutex<FullSlot>,
    full_cv: Condvar,
    cache: Mutex<Cache<T>>,
    generation: AtomicU64,
    /// Primary path of the current item (pinned in the cache, never cancelled).
    current: Mutex<Option<PathBuf>>,
    results: Mutex<Sender<Action>>,
}

/// Queue state. `wanted` (current + prefetch window of the latest `Show`) and `running` live
/// under one lock: a worker decides "cancel" and leaves `running` atomically, so `show()` can
/// never skip a path whose running job is about to be dropped.
#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    wanted: std::collections::HashSet<PathBuf>,
    /// Paths with a job running right now (not re-queued by a new Show).
    running: Vec<PathBuf>,
    shutdown: bool,
}

#[derive(Default)]
struct FullSlot {
    next: Option<ItemRef>,
    shutdown: bool,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The image loader. Dropping it stops its threads.
pub struct Loader<T: Send + Sync + 'static> {
    shared: Arc<Shared<T>>,
    rx: Receiver<Action>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl<T: Send + Sync + 'static> Loader<T> {
    /// Start a loader with `workers` decode threads (see [`default_workers`]).
    pub fn new(
        fs: Arc<dyn Fs>,
        backend: Arc<dyn RawBackend>,
        workers: usize,
        cache_bytes: usize,
        prepare: impl Fn(&Preview) -> T + Send + Sync + 'static,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let (tx, rx) = channel();
        let shared = Arc::new(Shared {
            fs,
            backend,
            prepare: Box::new(prepare),
            wake: Box::new(wake),
            queue: Mutex::new(Queue::default()),
            queue_cv: Condvar::new(),
            full: Mutex::new(FullSlot::default()),
            full_cv: Condvar::new(),
            cache: Mutex::new(Cache {
                map: HashMap::new(),
                tick: 0,
                bytes: 0,
                cap: cache_bytes,
            }),
            generation: AtomicU64::new(0),
            current: Mutex::new(None),
            results: Mutex::new(tx),
        });
        let mut threads = Vec::new();
        for i in 0..workers.max(1) {
            let s = shared.clone();
            threads.push(
                std::thread::Builder::new()
                    .name(format!("blink-decode-{i}"))
                    .spawn(move || worker(&s))
                    .expect("spawn decode worker"),
            );
        }
        let s = shared.clone();
        threads.push(
            std::thread::Builder::new()
                .name("blink-full".into())
                .spawn(move || full_worker(&s))
                .expect("spawn full-resolution worker"),
        );
        Self {
            shared,
            rx,
            threads,
        }
    }

    /// Execute [`Effect::Show`](blink_core::state::Effect::Show). Returns the `ImageReady`
    /// action at once when the current item's preview is cached, so the UI can show it in the
    /// same frame (PERF-2).
    pub fn show(&self, current: &ItemRef, prefetch: &[ItemRef]) -> Option<Action> {
        let g = self.shared.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *lock(&self.shared.current) = Some(current.path.clone());
        let mut q = lock(&self.shared.queue);
        let mut cache = lock(&self.shared.cache);
        let hit = cache.get(&(current.path.clone(), Level::Preview));
        let mut jobs = VecDeque::new();
        q.wanted.clear();
        for item in std::iter::once(current).chain(prefetch) {
            q.wanted.insert(item.path.clone());
            let cached = cache.get(&(item.path.clone(), Level::Preview)).is_some();
            if !cached && !q.running.contains(&item.path) {
                jobs.push_back(Job {
                    item: item.clone(),
                    generation: g,
                });
            }
        }
        drop(cache);
        q.jobs = jobs; // stale jobs are cancelled
        drop(q);
        self.shared.queue_cv.notify_all();
        hit.map(|l| l.ready())
    }

    /// Execute [`Effect::LoadFull`](blink_core::state::Effect::LoadFull). Returns `ImageReady`
    /// at once when it is cached.
    pub fn load_full(&self, item: &ItemRef) -> Option<Action> {
        if let Some(l) = lock(&self.shared.cache).get(&(item.path.clone(), Level::Full)) {
            return Some(l.ready());
        }
        lock(&self.shared.full).next = Some(item.clone());
        self.shared.full_cv.notify_all();
        None
    }

    /// Execute a loading effect; other effects are not the loader's and return `None`.
    pub fn execute(&self, effect: &Effect) -> Option<Action> {
        match effect {
            Effect::Show { current, prefetch } => self.show(current, prefetch),
            Effect::LoadFull { item } => self.load_full(item),
            _ => None,
        }
    }

    /// Results that arrived since the last call.
    pub fn poll(&self) -> Vec<Action> {
        self.rx.try_iter().collect()
    }

    /// Block until the next result arrives (tests and the bench harness).
    pub fn wait(&self, timeout: std::time::Duration) -> Option<Action> {
        self.rx.recv_timeout(timeout).ok()
    }

    /// The cached image of `path` at `level`.
    pub fn get(&self, path: &Path, level: Level) -> Option<Arc<Loaded<T>>> {
        lock(&self.shared.cache).get(&(path.to_path_buf(), level))
    }

    /// The best cached image of `path`: full resolution if loaded, else the preview.
    pub fn best(&self, path: &Path) -> Option<Arc<Loaded<T>>> {
        self.get(path, Level::Full)
            .or_else(|| self.get(path, Level::Preview))
    }

    /// No preview job queued or running.
    pub fn idle(&self) -> bool {
        let q = lock(&self.shared.queue);
        q.jobs.is_empty() && q.running.is_empty()
    }

    /// Bytes held by the L2 cache.
    pub fn cache_bytes(&self) -> usize {
        lock(&self.shared.cache).bytes
    }
}

impl<T: Send + Sync + 'static> Drop for Loader<T> {
    fn drop(&mut self) {
        lock(&self.shared.queue).shutdown = true;
        lock(&self.shared.full).shutdown = true;
        self.shared.queue_cv.notify_all();
        self.shared.full_cv.notify_all();
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

/// Decode pool size: cores − 1, at least 1 (SPEC §8.3). std reports logical cores; see ADR 0012.
pub fn default_workers() -> usize {
    std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .saturating_sub(1)
        .max(1)
}

fn send<T>(s: &Shared<T>, a: Action) {
    let _ = lock(&s.results).send(a);
    (s.wake)();
}

fn worker<T>(s: &Shared<T>) {
    loop {
        let job = {
            let mut q = lock(&s.queue);
            loop {
                if q.shutdown {
                    return;
                }
                if let Some(j) = q.jobs.pop_front() {
                    q.running.push(j.item.path.clone());
                    break j;
                }
                q = s.queue_cv.wait(q).unwrap_or_else(|e| e.into_inner());
            }
        };
        let result = load_preview(s, &job);
        let leave = || lock(&s.queue).running.retain(|p| p != &job.item.path);
        match result {
            Some(Ok(l)) => {
                let a = l.ready();
                lock(&s.cache).insert(Arc::new(l), &pinned(s));
                leave(); // after the insert: `show()` sees it cached or running, never neither
                send(s, a);
            }
            Some(Err(error)) => {
                leave();
                send(
                    s,
                    Action::ImageFailed {
                        path: job.item.path.clone(),
                        level: Level::Preview,
                        error,
                    },
                )
            }
            None => {} // cancelled; `cancel_if_stale` already left `running`
        }
    }
}

/// Cache keys never evicted: both levels of the current item.
fn pinned<T>(s: &Shared<T>) -> Vec<Key> {
    lock(&s.current)
        .clone()
        .into_iter()
        .flat_map(|p| [(p.clone(), Level::Preview), (p, Level::Full)])
        .collect()
}

/// Cancellation check: a job is stale when the latest `Show` no longer wants its path. A stale
/// job leaves `running` under the same lock, atomically with the decision.
fn is_stale<T>(s: &Shared<T>, job: &Job) -> bool {
    let mut q = lock(&s.queue);
    let stale =
        job.generation != s.generation.load(Ordering::SeqCst) && !q.wanted.contains(&job.item.path);
    if stale {
        q.running.retain(|p| p != &job.item.path);
    }
    stale
}

fn finish_load<T>(
    s: &Shared<T>,
    path: &Path,
    level: Level,
    preview: Preview,
    full_size: Option<(u32, u32)>,
) -> Loaded<T> {
    let size = (preview.image.width, preview.image.height);
    let histogram = histogram(&preview.image, preview.source);
    let prepared = (s.prepare)(&preview);
    Loaded {
        path: path.to_path_buf(),
        level,
        source: preview.source,
        size,
        full_size: full_size.unwrap_or(size),
        histogram,
        prepared,
        preview: Arc::new(preview),
    }
}

fn load_preview<T>(s: &Shared<T>, job: &Job) -> Option<Result<Loaded<T>, String>> {
    if is_stale(s, job) {
        return None;
    }
    let item = &job.item;
    let read = |p: &Path| s.fs.read(p).map_err(|e| format!("{}: {e}", p.display()));
    let out = match item.kind {
        FileKind::Heif => Err("preview unavailable for HEIF in v1".to_string()),
        FileKind::Jpeg => read(&item.path).and_then(|b| {
            if is_stale(s, job) {
                return Err(String::new());
            }
            blink_decode::decode_jpeg_file(&b, PreviewSource::Jpeg)
                .map(|p| (p, None))
                .map_err(|e| e.to_string())
        }),
        FileKind::Raw | FileKind::Dng => read(&item.path).and_then(|b| {
            if is_stale(s, job) {
                return Err(String::new());
            }
            let a = blink_decode::analyze(&b, item.kind, s.backend.as_ref());
            match (&a.chosen, &item.paired_jpeg) {
                (Some(_), _) => blink_decode::decode_embedded(&b, &a)
                    .map(|p| (p, a.full_size()))
                    .map_err(|e| e.to_string()),
                // No embedded preview: the paired JPEG is the display fallback (D-4).
                (None, Some(j)) => read(j).and_then(|jb| {
                    blink_decode::decode_jpeg_file(&jb, PreviewSource::PairedJpeg)
                        .map(|p| (p, a.full_size()))
                        .map_err(|e| e.to_string())
                }),
                (None, None) => Err("no embedded preview".into()),
            }
            .map(|(p, full)| {
                let full = full_at_least_own(&p, full);
                (p, Some(full))
            })
        }),
    };
    let out = match out {
        Ok((p, full)) => Ok(finish_load(s, &item.path, Level::Preview, p, full)),
        Err(e) if e.is_empty() => return None,
        Err(e) => Err(e),
    };
    Some(out)
}

/// The full size is at least the preview's own size (unknown sensor, or a preview larger than
/// the reported crop).
fn full_at_least_own(p: &Preview, full: Option<(u32, u32)>) -> (u32, u32) {
    let own = (p.image.width, p.image.height);
    match full {
        Some(f) if f.0.max(f.1) >= own.0.max(own.1) => f,
        _ => own,
    }
}

fn full_worker<T>(s: &Shared<T>) {
    loop {
        let item = {
            let mut slot = lock(&s.full);
            loop {
                if slot.shutdown {
                    return;
                }
                if let Some(n) = slot.next.take() {
                    break n;
                }
                slot = s.full_cv.wait(slot).unwrap_or_else(|e| e.into_inner());
            }
        };
        let result = load_full(s, &item);
        match result {
            Ok(l) => {
                let a = l.ready();
                lock(&s.cache).insert(Arc::new(l), &pinned(s));
                send(s, a);
            }
            Err(error) => send(
                s,
                Action::ImageFailed {
                    path: item.path.clone(),
                    level: Level::Full,
                    error,
                },
            ),
        }
    }
}

fn load_full<T>(s: &Shared<T>, item: &ItemRef) -> Result<Loaded<T>, String> {
    let read = |p: &Path| s.fs.read(p).map_err(|e| format!("{}: {e}", p.display()));
    let (preview, full): (Preview, Option<(u32, u32)>) = match (&item.paired_jpeg, item.kind) {
        (Some(j), _) => {
            let b = read(j)?;
            let p = blink_decode::decode_jpeg_file(&b, PreviewSource::PairedJpeg)
                .map_err(|e| e.to_string())?;
            (p, None)
        }
        (None, FileKind::Raw | FileKind::Dng) => {
            let b = read(&item.path)?;
            let a: Analysis = blink_decode::analyze(&b, item.kind, s.backend.as_ref());
            let p = blink_decode::decode_raw(&b, &a, s.backend.as_ref())
                .map_err(|e| format!("RAW decode failed: {e}"))?;
            (p, None)
        }
        (None, _) => return Err("no full-resolution source".into()),
    };
    Ok(finish_load(s, &item.path, Level::Full, preview, full))
}
