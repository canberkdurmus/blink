//! Image loader: executes [`Effect::Show`] and [`Effect::LoadFull`] (SPEC §8.3).
//!
//! - Decode workers (cores − 1, min 1) take jobs from a priority queue: the current item's
//!   preview first, then the prefetch window in order. A new `Show` replaces the queue and bumps
//!   the generation; a job checks its generation (its cancellation token) before decoding, so
//!   stale jobs are dropped.
//! - **Pacing (PERF-3).** When moves come faster than a decode takes (a held arrow key on a
//!   machine with few cores), decoding the new current item is wasted: the user has moved on
//!   before it ends. The loader then starts with the item the user will reach when a decode
//!   started now finishes ([`lead`]), so every finished decode is drawn. Once the moves stop
//!   ([`Pace::settled`]), a free worker takes the current item first again.
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
use blink_core::info::InfoInput;
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

/// Move rate and decode time, measured by the loader to decode ahead of a held key (PERF-3).
#[derive(Debug, Default)]
struct Pace {
    last_show: Option<std::time::Instant>,
    /// The item shown at the last `Show`.
    current: Option<PathBuf>,
    /// The items ahead at the last `Show`, nearest first: arriving at the n-th is n steps.
    ahead: Vec<PathBuf>,
    /// Smoothed seconds between steps; `None` when not moving steadily.
    interval: Option<f64>,
    /// Smoothed seconds per preview job (read, decode, prepare).
    latency: Option<f64>,
}

fn smooth(old: Option<f64>, new: f64) -> f64 {
    old.map_or(new, |o| 0.7 * o + 0.3 * new)
}

impl Pace {
    fn shown(&mut self, current: &Path, ahead: &[ItemRef], now: std::time::Instant) {
        if self.current.as_deref() == Some(current) {
            return; // the same item again (e.g. a zoom): not a move, the run goes on
        }
        // A delayed frame can carry several key repeats: a move n items ahead is n steps.
        let steps = self.ahead.iter().position(|a| a == current).map(|i| i + 1);
        self.interval = match (steps, self.last_show) {
            (Some(n), Some(t)) => {
                let per_step = (now - t).as_secs_f64() / n as f64;
                // A pause longer than a key repeat is not part of the same run.
                (per_step < 0.25).then(|| smooth(self.interval, per_step))
            }
            _ => None,
        };
        self.last_show = Some(now);
        self.current = Some(current.to_path_buf());
        self.ahead = ahead.iter().map(|a| a.path.clone()).collect();
    }

    /// No move for longer than two step intervals: the user stopped on the current item.
    fn settled(&self, now: std::time::Instant) -> bool {
        match (self.interval, self.last_show) {
            (Some(i), Some(t)) => (now - t).as_secs_f64() > 2.0 * i,
            _ => true,
        }
    }
}

/// How many items ahead to start decoding: 0 while decodes keep up with the moves, else the
/// first item the user won't have passed when a decode started now ends (capped at `ahead`).
fn lead(interval: Option<f64>, latency: Option<f64>, ahead: usize) -> usize {
    match (interval, latency) {
        (Some(i), Some(l)) if i > 0.0 && l > i => ((l / i).ceil() as usize).min(ahead),
        _ => 0,
    }
}

/// Items decoded from the lead item on (PERF-3), like the prefetch window's 8 ahead.
const FAR_RUN: usize = 8;

/// Job order for a `Show` with lead `k`: from the lead item on, then the current item, then
/// the rest of the window in order.
fn priority<'a>(
    current: &'a ItemRef,
    prefetch: &'a [ItemRef],
    ahead: &'a [ItemRef],
    k: usize,
) -> Vec<&'a ItemRef> {
    if k == 0 {
        return std::iter::once(current).chain(prefetch).collect();
    }
    // The lead item and the next few: as many as the prefetch window holds ahead.
    let far = &ahead[k - 1..(k - 1 + FAR_RUN).min(ahead.len())];
    far.iter()
        .chain(std::iter::once(current))
        .chain(prefetch.iter().filter(|p| !far.contains(p)))
        .collect()
}

/// The next job: the queue's first, except that once the moves stopped (`settled_current` is
/// the current item) the current item's job goes first, even behind lead items (PERF-3).
fn next_job(jobs: &mut VecDeque<Job>, settled_current: Option<&Path>) -> Option<Job> {
    let at = settled_current
        .and_then(|c| jobs.iter().position(|j| j.item.path == c))
        .unwrap_or(0);
    jobs.remove(at)
}

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
    /// Histogram of the displayed pixels (VIEW-6), computed on first use.
    histogram: std::sync::OnceLock<Histogram>,
    /// Info panel values for these pixels (VIEW-7).
    pub info: InfoInput,
    /// The UI's prepared form of the pixels.
    pub prepared: T,
    /// Decoded RGB pixels, kept for the clipping overlay and tests.
    pub preview: Arc<Preview>,
}

impl<T> Loaded<T> {
    /// Histogram of the displayed pixels (VIEW-6). Computed on first call (all cores, a few
    /// ms); call it off the UI thread, or check [`Self::histogram_ready`].
    pub fn histogram(&self) -> &Histogram {
        self.histogram
            .get_or_init(|| histogram(&self.preview.image, self.source))
    }

    /// The histogram if it has been computed.
    pub fn histogram_ready(&self) -> Option<&Histogram> {
        self.histogram.get()
    }

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

    /// Cache `v`, evicting least recently used entries so the total never exceeds the cap
    /// (PERF-8). Pinned entries (the current item) go last: first its preview once its full
    /// image is cached (the full one supersedes it), then, if `v` alone can't fit, `v` itself
    /// is not kept. Returns whether `v` was kept.
    fn insert(&mut self, v: Arc<Loaded<T>>, pinned: &[Key]) -> bool {
        self.tick += 1;
        let k = (v.path.clone(), v.level);
        if let Some((old, _)) = self.map.remove(&k) {
            self.bytes -= old.bytes();
        }
        let need = v.bytes();
        while self.bytes + need > self.cap {
            let superseded = |key: &Key| {
                key.1 == Level::Preview
                    && (self.map.contains_key(&(key.0.clone(), Level::Full))
                        || k == (key.0.clone(), Level::Full))
            };
            let victim = self
                .map
                .iter()
                .filter(|(key, _)| !pinned.contains(key))
                .min_by_key(|(_, (_, used))| *used)
                .map(|(key, _)| key.clone())
                .or_else(|| self.map.keys().find(|key| superseded(key)).cloned());
            let Some(victim) = victim else { break };
            if let Some((old, _)) = self.map.remove(&victim) {
                self.bytes -= old.bytes();
            }
        }
        if self.bytes + need > self.cap {
            return false;
        }
        self.bytes += need;
        self.map.insert(k, (v, self.tick));
        true
    }
}

struct Job {
    item: ItemRef,
    generation: u64,
    /// [`Shared::epoch`] when queued: a job from before a [`Loader::clear`] never reports.
    epoch: u64,
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
    /// A full-resolution load is running.
    full_running: std::sync::atomic::AtomicBool,
    cache: Mutex<Cache<T>>,
    generation: AtomicU64,
    pace: Mutex<Pace>,
    /// Bumped by [`Loader::clear`]: results of jobs started before it are dropped.
    epoch: AtomicU64,
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
    /// Paths with a job running right now (not re-queued by a new Show), with the job's epoch.
    running: Vec<(PathBuf, u64)>,
    shutdown: bool,
}

#[derive(Default)]
struct FullSlot {
    next: Option<(ItemRef, u64)>,
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
            full_running: std::sync::atomic::AtomicBool::new(false),
            cache: Mutex::new(Cache {
                map: HashMap::new(),
                tick: 0,
                bytes: 0,
                cap: cache_bytes,
            }),
            generation: AtomicU64::new(0),
            pace: Mutex::new(Pace::default()),
            epoch: AtomicU64::new(0),
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
        self.show_ahead(current, prefetch, &[])
    }

    /// [`show`](Self::show) knowing which of `prefetch` lie ahead in the travel direction,
    /// nearest first: while moves outpace decodes, decoding starts [`lead`] items ahead.
    pub fn show_ahead(
        &self,
        current: &ItemRef,
        prefetch: &[ItemRef],
        ahead: &[ItemRef],
    ) -> Option<Action> {
        let k = {
            let mut p = lock(&self.shared.pace);
            p.shown(&current.path, ahead, std::time::Instant::now());
            lead(p.interval, p.latency, ahead.len())
        };
        let order = priority(current, prefetch, ahead, k);
        let g = self.shared.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *lock(&self.shared.current) = Some(current.path.clone());
        let mut q = lock(&self.shared.queue);
        let mut cache = lock(&self.shared.cache);
        let epoch = self.shared.epoch.load(Ordering::SeqCst);
        let hit = cache.get(&(current.path.clone(), Level::Preview));
        let mut jobs = VecDeque::new();
        q.wanted.clear();
        for item in order {
            q.wanted.insert(item.path.clone());
            let cached = cache.get(&(item.path.clone(), Level::Preview)).is_some();
            let running = q
                .running
                .iter()
                .any(|(p, e)| p == &item.path && *e == epoch);
            if !cached && !running {
                jobs.push_back(Job {
                    item: item.clone(),
                    generation: g,
                    epoch,
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
        let epoch = self.shared.epoch.load(Ordering::SeqCst);
        lock(&self.shared.full).next = Some((item.clone(), epoch));
        self.shared.full_cv.notify_all();
        None
    }

    /// Execute a loading effect; other effects are not the loader's and return `None`.
    pub fn execute(&self, effect: &Effect) -> Option<Action> {
        match effect {
            Effect::Show {
                current,
                prefetch,
                ahead,
            } => self.show_ahead(current, prefetch, ahead),
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

    /// A probe, usable from any thread, that is true while the loader has work: preview jobs
    /// queued or running, or a full-resolution load waiting or running. Background scans
    /// (capture metadata) wait while it is true, so they never slow what the user is looking at.
    pub fn busy_probe(&self) -> impl Fn() -> bool + Send + Sync + 'static {
        let s = self.shared.clone();
        move || {
            let q = lock(&s.queue);
            let previews = !q.jobs.is_empty() || !q.running.is_empty();
            drop(q);
            previews || s.full_running.load(Ordering::SeqCst) || lock(&s.full).next.is_some()
        }
    }

    /// No preview job queued or running.
    pub fn idle(&self) -> bool {
        let q = lock(&self.shared.queue);
        q.jobs.is_empty() && q.running.is_empty()
    }

    /// Forget everything: cancel queued jobs and empty the cache (a folder was (re)opened, its
    /// files may have changed on disk). Jobs already running finish but neither fill the cache
    /// nor report: their bytes may be from before the reopen.
    pub fn clear(&self) {
        self.shared.epoch.fetch_add(1, Ordering::SeqCst);
        self.shared.generation.fetch_add(1, Ordering::SeqCst);
        let mut q = lock(&self.shared.queue);
        q.jobs.clear();
        q.wanted.clear();
        drop(q);
        *lock(&self.shared.current) = None;
        lock(&self.shared.full).next = None;
        let mut c = lock(&self.shared.cache);
        c.map.clear();
        c.bytes = 0;
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
                let settled = lock(&s.pace).settled(std::time::Instant::now());
                let cur = settled.then(|| lock(&s.current).clone()).flatten();
                if let Some(j) = next_job(&mut q.jobs, cur.as_deref()) {
                    q.running.push((j.item.path.clone(), j.epoch));
                    // Share the cores between the jobs running now: an idle loader decodes
                    // the current item on all of them, a busy one doesn't oversubscribe.
                    let threads = (cores() / q.running.len()).max(1);
                    break (j, threads);
                }
                q = s.queue_cv.wait(q).unwrap_or_else(|e| e.into_inner());
            }
        };
        let (job, threads) = job;
        let started = std::time::Instant::now();
        let result = load_preview(s, &job, threads);
        if let Some(Ok(_)) = &result {
            let mut p = lock(&s.pace);
            p.latency = Some(smooth(p.latency, started.elapsed().as_secs_f64()));
        }
        let leave = || {
            lock(&s.queue)
                .running
                .retain(|(p, e)| !(p == &job.item.path && *e == job.epoch))
        };
        match result {
            Some(Ok(l)) => {
                let a = l.ready();
                let pinned = pinned(s);
                let fresh = {
                    // Checked under the cache lock, which `clear()` takes after bumping the epoch.
                    let mut c = lock(&s.cache);
                    let fresh = job.epoch == s.epoch.load(Ordering::SeqCst);
                    if fresh && !c.insert(Arc::new(l), &pinned) {
                        eprintln!(
                            "blink: an image is larger than the whole decode cache; raise cache_mb"
                        );
                    }
                    fresh
                };
                leave(); // after the insert: `show()` sees it cached or running, never neither
                if fresh {
                    send(s, a);
                }
            }
            Some(Err(_)) if job.epoch != s.epoch.load(Ordering::SeqCst) => leave(),
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
    let cleared = job.epoch != s.epoch.load(Ordering::SeqCst);
    let stale = cleared
        || (job.generation != s.generation.load(Ordering::SeqCst)
            && !q.wanted.contains(&job.item.path));
    if stale {
        q.running
            .retain(|(p, e)| !(p == &job.item.path && *e == job.epoch));
    }
    stale
}

fn finish_load<T>(
    s: &Shared<T>,
    item: &ItemRef,
    level: Level,
    (preview, analysis): (Preview, Analysis),
    full_size: Option<(u32, u32)>,
) -> Loaded<T> {
    let path = &item.path;
    let size = (preview.image.width, preview.image.height);
    let info = crate::inspect::info_input(path, item.kind, &analysis, Some(&preview));
    let prepared = (s.prepare)(&preview);
    Loaded {
        path: path.to_path_buf(),
        level,
        source: preview.source,
        size,
        full_size: full_size.unwrap_or(size),
        histogram: std::sync::OnceLock::new(),
        info,
        prepared,
        preview: Arc::new(preview),
    }
}

fn read<T>(s: &Shared<T>, p: &Path) -> Result<Vec<u8>, String> {
    s.fs.read(p).map_err(|e| format!("{}: {e}", p.display()))
}

/// Outcome of a preview job: `None` = cancelled.
type Outcome<T> = Option<Result<Loaded<T>, String>>;

fn cores() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

fn load_preview<T>(s: &Shared<T>, job: &Job, threads: usize) -> Outcome<T> {
    if is_stale(s, job) {
        return None;
    }
    let item = &job.item;
    if item.kind == FileKind::Heif {
        return Some(Err("preview unavailable for HEIF in v1".into()));
    }
    let bytes = match read(s, &item.path) {
        Ok(b) => b,
        Err(e) => return Some(Err(e)),
    };
    if is_stale(s, job) {
        return None;
    }
    let a = blink_decode::analyze(&bytes, item.kind, s.backend.as_ref());
    let decoded = match (item.kind, &a.chosen, &item.paired_jpeg) {
        (FileKind::Jpeg, _, _) => {
            blink_decode::decode_jpeg_file_threads(&bytes, PreviewSource::Jpeg, threads)
                .map(|p| (p, None))
                .map_err(|e| e.to_string())
        }
        (_, Some(_), _) => blink_decode::decode_embedded_threads(&bytes, &a, threads)
            .map(|p| {
                let full = full_at_least_own(&p, a.full_size());
                (p, Some(full))
            })
            .map_err(|e| e.to_string()),
        // No embedded preview: the paired JPEG is the display fallback (D-4).
        (_, None, Some(j)) => read(s, j).and_then(|jb| {
            blink_decode::decode_jpeg_file_threads(&jb, PreviewSource::PairedJpeg, threads)
                .map(|p| {
                    let full = full_at_least_own(&p, a.full_size());
                    (p, Some(full))
                })
                .map_err(|e| e.to_string())
        }),
        (_, None, None) => Err("no embedded preview".into()),
    };
    Some(decoded.map(|(p, full)| finish_load(s, item, Level::Preview, (p, a), full)))
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
                    s.full_running.store(true, Ordering::SeqCst);
                    break n;
                }
                slot = s.full_cv.wait(slot).unwrap_or_else(|e| e.into_inner());
            }
        };
        let (item, epoch) = item;
        let result = load_full(s, &item);
        s.full_running.store(false, Ordering::SeqCst);
        let pinned = pinned(s);
        let mut c = lock(&s.cache);
        if epoch != s.epoch.load(Ordering::SeqCst) {
            continue; // a `clear()` happened meanwhile: the bytes may be outdated
        }
        match result {
            Ok(l) => {
                let a = l.ready();
                if !c.insert(Arc::new(l), &pinned) {
                    eprintln!(
                        "blink: an image is larger than the whole decode cache; raise cache_mb"
                    );
                }
                drop(c);
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
    let (preview, a) = match (&item.paired_jpeg, item.kind) {
        (Some(j), _) => {
            let jb = read(s, j)?;
            let p = blink_decode::decode_jpeg_file(&jb, PreviewSource::PairedJpeg)
                .map_err(|e| e.to_string())?;
            // Info comes from the RAW; an unreadable RAW doesn't fail the JPEG zoom.
            let a = match read(s, &item.path) {
                Ok(b) => blink_decode::analyze(&b, item.kind, s.backend.as_ref()),
                Err(_) => blink_decode::analyze(&jb, FileKind::Jpeg, s.backend.as_ref()),
            };
            (p, a)
        }
        (None, FileKind::Raw | FileKind::Dng) => {
            let b = read(s, &item.path)?;
            let a: Analysis = blink_decode::analyze(&b, item.kind, s.backend.as_ref());
            let p = blink_decode::decode_raw(&b, &a, s.backend.as_ref())
                .map_err(|e| format!("RAW decode failed: {e}"))?;
            (p, a)
        }
        (None, _) => return Err("no full-resolution source".into()),
    };
    Ok(finish_load(s, item, Level::Full, (preview, a), None))
}

#[cfg(test)]
mod pace_tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn item(n: usize) -> ItemRef {
        ItemRef {
            path: format!("/f/{n}.ARW").into(),
            kind: FileKind::Raw,
            paired_jpeg: None,
        }
    }

    #[test]
    fn req_perf_3_lead_is_zero_while_decodes_keep_up() {
        assert_eq!(lead(None, Some(0.14), 8), 0, "not moving steadily");
        assert_eq!(
            lead(Some(0.2), Some(0.14), 8),
            0,
            "decode faster than the moves"
        );
        assert_eq!(lead(Some(0.025), None, 8), 0, "no decode measured yet");
    }

    #[test]
    fn req_perf_3_lead_is_the_first_item_not_passed_when_a_decode_ends() {
        assert_eq!(lead(Some(0.025), Some(0.14), 8), 6); // 5.6 items pass meanwhile
        assert_eq!(lead(Some(0.025), Some(0.5), 8), 8, "capped at the window");
        assert_eq!(lead(Some(0.025), Some(0.5), 0), 0);
    }

    #[test]
    fn req_perf_3_lead_items_go_first_and_the_current_item_first_once_settled() {
        let (cur, ahead) = (item(0), (1..=8).map(item).collect::<Vec<_>>());
        let mut prefetch = vec![item(1), item(99)];
        prefetch.extend((2..=8).map(item));
        let names = |v: Vec<&ItemRef>| v.iter().map(|i| i.path.clone()).collect::<Vec<_>>();
        assert_eq!(
            names(priority(&cur, &prefetch, &ahead, 0)),
            names(std::iter::once(&cur).chain(&prefetch).collect())
        );
        let order = names(priority(&cur, &prefetch, &ahead, 6));
        let want: Vec<PathBuf> = [6, 7, 8, 0, 1, 99, 2, 3, 4, 5]
            .map(|n| item(n).path)
            .to_vec();
        assert_eq!(order, want);

        let mut jobs: VecDeque<Job> = [6, 7, 0, 1]
            .map(|n| Job {
                item: item(n),
                generation: 0,
                epoch: 0,
            })
            .into();
        let p = |j: Option<Job>| j.unwrap().item.path;
        assert_eq!(p(next_job(&mut jobs, None)), item(6).path, "still moving");
        assert_eq!(
            p(next_job(&mut jobs, Some(&item(0).path))),
            item(0).path,
            "stopped"
        );
        assert_eq!(p(next_job(&mut jobs, Some(&item(0).path))), item(7).path);
    }

    #[test]
    fn req_perf_3_steady_steps_measure_the_interval_and_a_stop_settles() {
        let t0 = Instant::now();
        let mut p = Pace::default();
        p.shown(&item(0).path, &[item(1)], t0);
        assert_eq!(p.interval, None);
        for n in 1..=5 {
            p.shown(
                &item(n).path,
                &[item(n + 1)],
                t0 + Duration::from_millis(25 * n as u64),
            );
        }
        let i = p.interval.unwrap();
        assert!((i - 0.025).abs() < 1e-6, "{i}");
        let last = t0 + Duration::from_millis(125);
        assert!(!p.settled(last + Duration::from_millis(40)));
        assert!(p.settled(last + Duration::from_millis(60)));
        // A jump (not the next item ahead) ends the run.
        p.shown(
            &item(50).path,
            &[item(51)],
            last + Duration::from_millis(25),
        );
        assert_eq!(p.interval, None);
        assert!(p.settled(last + Duration::from_millis(26)));
        // So does a long pause.
        p.shown(
            &item(51).path,
            &[item(52)],
            last + Duration::from_millis(400),
        );
        assert_eq!(p.interval, None);
    }

    #[test]
    fn req_perf_3_a_lead_past_the_prefetch_window_decodes_a_short_run_from_the_lead() {
        // 280 ms decodes at 25 ms per step: start 12 items ahead, beyond the 8 prefetched.
        let (cur, ahead) = (item(0), (1..=32).map(item).collect::<Vec<_>>());
        let prefetch: Vec<_> = (1..=8).map(item).collect();
        let k = lead(Some(0.025), Some(0.28), ahead.len());
        assert_eq!(k, 12);
        let order: Vec<_> = priority(&cur, &prefetch, &ahead, k)
            .iter()
            .map(|i| i.path.clone())
            .collect();
        let want: Vec<_> = (12..=19)
            .chain([0])
            .chain(1..=8)
            .map(|n| item(n).path)
            .collect();
        assert_eq!(
            order, want,
            "8 items from the lead, then the current item and the window"
        );
    }

    #[test]
    fn req_perf_3_a_frame_that_merges_key_repeats_keeps_the_run() {
        // A busy machine delays a frame; two 25 ms key repeats land in it. The run goes on,
        // at 25 ms per item, and so does a repeated `Show` of the same item (e.g. a zoom).
        let t0 = Instant::now();
        let ahead = |n: usize| ((n + 1)..=(n + 8)).map(item).collect::<Vec<_>>();
        let mut p = Pace::default();
        for n in 0..=3 {
            p.shown(
                &item(n).path,
                &ahead(n),
                t0 + Duration::from_millis(25 * n as u64),
            );
        }
        p.shown(&item(5).path, &ahead(5), t0 + Duration::from_millis(125));
        let i = p.interval.expect("still a run");
        assert!((i - 0.025).abs() < 1e-6, "{i}");
        p.shown(&item(5).path, &ahead(5), t0 + Duration::from_millis(130));
        assert_eq!(p.interval, Some(i));
        // A frame stalled 200 ms that covered 8 repeats is still the same run.
        p.shown(&item(13).path, &ahead(13), t0 + Duration::from_millis(325));
        assert!(p.interval.is_some());
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use proptest::prelude::*;

    fn loaded(n: u8, level: Level, w: u32) -> Arc<Loaded<()>> {
        Arc::new(Loaded {
            path: format!("/f/{n}.ARW").into(),
            level,
            source: PreviewSource::Embedded,
            size: (w, 1),
            full_size: (w, 1),
            histogram: std::sync::OnceLock::new(),
            info: Default::default(),
            prepared: (),
            preview: Arc::new(Preview {
                image: blink_decode::image::RgbImage {
                    width: 0,
                    height: 0,
                    data: vec![],
                },
                source: PreviewSource::Embedded,
                space: blink_decode::color::SourceSpace::Srgb,
                orientation: 1,
            }),
        })
    }

    fn cache(cap: usize) -> Cache<()> {
        Cache {
            map: HashMap::new(),
            tick: 0,
            bytes: 0,
            cap,
        }
    }

    fn total(c: &Cache<()>) -> usize {
        c.map.values().map(|(v, _)| v.bytes()).sum()
    }

    #[test]
    fn req_perf_8_the_current_items_full_image_supersedes_its_preview_under_the_cap() {
        // 7 bytes per pixel: cap of 100 px.
        let mut c = cache(700);
        let cur = (PathBuf::from("/f/0.ARW"), Level::Preview);
        let pinned = [cur.clone(), (cur.0.clone(), Level::Full)];
        assert!(c.insert(loaded(1, Level::Preview, 30), &pinned));
        assert!(c.insert(loaded(0, Level::Preview, 40), &pinned));
        // The full image needs 80 px: the other item goes, then the superseded preview.
        assert!(c.insert(loaded(0, Level::Full, 80), &pinned));
        assert_eq!(c.map.len(), 1);
        assert!(c.map.contains_key(&(cur.0.clone(), Level::Full)));
        // Larger than the whole cache: not kept, the cap holds.
        assert!(!c.insert(loaded(2, Level::Full, 101), &pinned));
        assert!(c.bytes <= c.cap);
    }

    proptest! {
        /// PERF-8: whatever is inserted and whichever item is current, the cache never holds
        /// more than its cap, and its byte count is exact.
        #[test]
        fn req_perf_8_random_inserts_never_exceed_the_cap(
            cap in 1usize..2000,
            ops in proptest::collection::vec((0u8..6, any::<bool>(), 1u32..120, 0u8..6), 1..80),
        ) {
            let mut c = cache(cap);
            for (n, full, w, current) in ops {
                let p = PathBuf::from(format!("/f/{current}.ARW"));
                let pinned = [(p.clone(), Level::Preview), (p, Level::Full)];
                let level = if full { Level::Full } else { Level::Preview };
                c.insert(loaded(n, level, w), &pinned);
                prop_assert!(c.bytes <= c.cap, "{} > {}", c.bytes, c.cap);
                prop_assert_eq!(c.bytes, total(&c));
            }
        }
    }
}
