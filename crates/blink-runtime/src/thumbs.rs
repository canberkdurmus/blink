//! Grid and filmstrip thumbnails (VIEW-5, PERF-6) on their own worker pool, in the order the
//! UI asks for: visible cells first, then outward. A file is read as a 256 KB prefix first;
//! only when its small preview isn't inside does the whole file get read.

use crate::fs::Fs;
use blink_core::state::ItemRef;
use blink_decode::Preview;
use blink_decode::thumb::{ThumbError, thumbnail};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

/// Bytes read before deciding whether the whole file is needed.
pub const PREFIX: usize = 256 << 10;

/// Thumbnail quality tier: `Thumb` serves the small and medium grid and the filmstrip, `Large`
/// the large grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tier {
    Thumb,
    Large,
}

impl Tier {
    /// (smallest source long edge, stored long edge).
    pub fn edges(self) -> (u32, u32) {
        match self {
            // A 160×120 EXIF thumbnail is enough for cells up to ~200 px.
            Self::Thumb => (120, 192),
            Self::Large => (288, 320),
        }
    }
}

/// One finished thumbnail; `None` pixels: the file has none (shown as a placeholder).
pub struct Thumb {
    pub preview: Option<Preview>,
}

type Key = (PathBuf, Tier);

#[derive(Default)]
struct Inner {
    /// Wanted, highest priority first.
    order: Vec<(ItemRef, Tier)>,
    /// Index into `order` of the next candidate.
    next: usize,
    running: HashSet<Key>,
    done: HashMap<Key, Arc<Thumb>>,
    shutdown: bool,
}

struct Shared {
    fs: Arc<dyn Fs>,
    inner: Mutex<Inner>,
    cv: Condvar,
    wake: Box<dyn Fn() + Send + Sync>,
}

fn lock(m: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The thumbnail pool.
pub struct ThumbService {
    shared: Arc<Shared>,
    workers: Vec<std::thread::JoinHandle<()>>,
}

impl ThumbService {
    pub fn new(fs: Arc<dyn Fs>, workers: usize, wake: impl Fn() + Send + Sync + 'static) -> Self {
        let shared = Arc::new(Shared {
            fs,
            inner: Mutex::new(Inner::default()),
            cv: Condvar::new(),
            wake: Box::new(wake),
        });
        let workers = (0..workers.max(1))
            .filter_map(|i| {
                let s = shared.clone();
                std::thread::Builder::new()
                    .name(format!("blink-thumb-{i}"))
                    .spawn(move || worker(&s))
                    .ok()
            })
            .collect();
        Self { shared, workers }
    }

    /// Replace the wanted list (highest priority first). Finished thumbnails stay; those of
    /// tiers not in `order` are dropped to free memory.
    pub fn request(&self, order: Vec<(ItemRef, Tier)>) {
        let mut g = lock(&self.shared.inner);
        let tiers: HashSet<Tier> = order.iter().map(|(_, t)| *t).collect();
        g.done.retain(|(_, t), _| tiers.contains(t));
        g.order = order;
        g.next = 0;
        drop(g);
        self.shared.cv.notify_all();
    }

    /// Forget every thumbnail (another folder; files may have changed).
    pub fn clear(&self) {
        let mut g = lock(&self.shared.inner);
        g.order.clear();
        g.next = 0;
        g.done.clear();
    }

    pub fn get(&self, item: &ItemRef, tier: Tier) -> Option<Arc<Thumb>> {
        lock(&self.shared.inner)
            .done
            .get(&(item.path.clone(), tier))
            .cloned()
    }

    /// Forget one file's thumbnails (it changed on disk).
    pub fn invalidate(&self, path: &std::path::Path) {
        let mut g = lock(&self.shared.inner);
        g.done.retain(|(p, _), _| p != path);
        g.next = 0;
        drop(g);
        self.shared.cv.notify_all();
    }

    /// How many of the wanted thumbnails are finished.
    pub fn done_of_wanted(&self) -> (usize, usize) {
        let g = lock(&self.shared.inner);
        let done = g
            .order
            .iter()
            .filter(|(i, t)| g.done.contains_key(&(i.path.clone(), *t)))
            .count();
        (done, g.order.len())
    }
}

impl Drop for ThumbService {
    fn drop(&mut self) {
        lock(&self.shared.inner).shutdown = true;
        self.shared.cv.notify_all();
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}

fn worker(s: &Shared) {
    loop {
        let (item, tier) = {
            let mut g = lock(&s.inner);
            loop {
                if g.shutdown {
                    return;
                }
                let mut pick = None;
                while g.next < g.order.len() {
                    let (i, t) = g.order[g.next].clone();
                    let k = (i.path.clone(), t);
                    g.next += 1;
                    if !g.done.contains_key(&k) && !g.running.contains(&k) {
                        pick = Some((i, t));
                        break;
                    }
                }
                if let Some(p) = pick {
                    g.running.insert((p.0.path.clone(), p.1));
                    break p;
                }
                g = s.cv.wait(g).unwrap_or_else(|e| e.into_inner());
            }
        };
        // A decoder panic costs this file its thumbnail, never the worker (or `running`).
        let thumb = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            make(s.fs.as_ref(), &item, tier)
        }))
        .unwrap_or(Thumb { preview: None });
        let mut g = lock(&s.inner);
        let k = (item.path.clone(), tier);
        g.running.remove(&k);
        // Still wanted (not cleared, tier still in use)?
        if g.order
            .iter()
            .any(|(i, t)| i.path == item.path && *t == tier)
        {
            g.done.insert(k, Arc::new(thumb));
            drop(g);
            (s.wake)();
        }
    }
}

/// Make one thumbnail: prefix first, then the whole file, then the paired JPEG.
pub fn make(fs: &dyn Fs, item: &ItemRef, tier: Tier) -> Thumb {
    let (min, max) = tier.edges();
    let try_file = |path: &std::path::Path, kind| -> Option<Preview> {
        let prefix = fs.read_prefix(path, PREFIX).ok()?;
        let complete = prefix.len() < PREFIX;
        match thumbnail(&prefix, kind, min, max, complete) {
            Ok(p) => Some(p),
            Err(ThumbError::NeedWholeFile) => {
                let all = fs.read(path).ok()?;
                thumbnail(&all, kind, min, max, true).ok()
            }
            Err(ThumbError::Failed(_)) => None,
        }
    };
    let preview = try_file(&item.path, item.kind).or_else(|| {
        item.paired_jpeg
            .as_deref()
            .and_then(|j| try_file(j, blink_core::formats::FileKind::Jpeg))
    });
    Thumb { preview }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::{FsCall, MemFs};
    use blink_core::formats::FileKind;
    use blink_testkit::fixtures::{jpeg, tiff_with_jpegs};
    use std::time::{Duration, Instant};

    /// A RAW stand-in: a small preview first, then a noisy (so large) big one.
    fn raw_with(thumb: (u32, u32), big: (u32, u32)) -> Vec<u8> {
        let noise = |(w, h): (u32, u32)| -> Vec<u8> {
            let mut x = 0x9e37_79b9_u32;
            (0..w * h * 3)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 17;
                    x ^= x << 5;
                    x as u8
                })
                .collect()
        };
        let (tw, th) = thumb;
        let small = jpeg(&vec![90u8; (tw * th * 3) as usize], tw, th, 80, None);
        tiff_with_jpegs(&[small, jpeg(&noise(big), big.0, big.1, 90, None)])
    }

    fn item(p: &str) -> ItemRef {
        ItemRef {
            path: p.into(),
            kind: FileKind::Raw,
            paired_jpeg: None,
        }
    }

    #[test]
    fn req_perf_6_a_thumbnail_inside_the_prefix_never_reads_the_whole_file() {
        let fs = MemFs::new();
        fs.add_file("/f/a.ARW", raw_with((160, 120), (2400, 1600)));
        assert!(fs.peek("/f/a.ARW").unwrap().len() > PREFIX);
        let t = make(&fs, &item("/f/a.ARW"), Tier::Thumb);
        let p = t.preview.unwrap();
        assert_eq!((p.image.width, p.image.height), (160, 120));
        assert!(
            fs.calls().iter().all(|c| !matches!(c, FsCall::Read(_))),
            "{:?}",
            fs.calls()
        );
        // The large tier needs the big preview: the whole file.
        let t = make(&fs, &item("/f/a.ARW"), Tier::Large);
        assert_eq!(t.preview.unwrap().image.width, 300);
        assert!(fs.calls().iter().any(|c| matches!(c, FsCall::Read(_))));
    }

    #[test]
    fn req_view_5_unreadable_files_get_an_empty_thumbnail_and_pairs_fall_back_to_the_jpeg() {
        let fs = MemFs::new();
        fs.add_file("/f/bad.ARW", b"not a raw".to_vec());
        assert!(
            make(&fs, &item("/f/bad.ARW"), Tier::Thumb)
                .preview
                .is_none()
        );
        fs.add_file(
            "/f/bad.JPG",
            jpeg(&vec![10u8; 400 * 300 * 3], 400, 300, 80, None),
        );
        let mut i = item("/f/bad.ARW");
        i.paired_jpeg = Some("/f/bad.JPG".into());
        let p = make(&fs, &i, Tier::Thumb).preview.unwrap();
        assert_eq!((p.image.width, p.image.height), (133, 100));
    }

    #[test]
    fn req_perf_6_requested_order_is_served_first_and_all_finish() {
        let fs = MemFs::new();
        for n in 0..30 {
            fs.add_file(format!("/f/{n}.ARW"), raw_with((160, 120), (800, 600)));
        }
        let fs = Arc::new(fs);
        let svc = ThumbService::new(fs.clone(), 1, || {});
        let order: Vec<_> = (0..30)
            .rev()
            .map(|n| (item(&format!("/f/{n}.ARW")), Tier::Thumb))
            .collect();
        svc.request(order);
        let t = Instant::now();
        while svc.done_of_wanted().0 < 30 {
            assert!(
                t.elapsed() < Duration::from_secs(10),
                "thumbnails never finished"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        // One worker: files were read in the requested order (last item first).
        let first = fs.calls().into_iter().find_map(|c| match c {
            FsCall::Read(p) => Some(p),
            _ => None,
        });
        let _ = first;
        let reads: Vec<PathBuf> = fs
            .calls()
            .into_iter()
            .filter_map(|c| match c {
                FsCall::ReadPrefix(p, _) => Some(p),
                _ => None,
            })
            .collect();
        assert_eq!(reads.first(), Some(&PathBuf::from("/f/29.ARW")));
        assert_eq!(reads.last(), Some(&PathBuf::from("/f/0.ARW")));
        assert!(svc.get(&item("/f/7.ARW"), Tier::Thumb).is_some());
        // Switching tier drops the other one's thumbnails.
        svc.request(vec![(item("/f/0.ARW"), Tier::Large)]);
        assert!(svc.get(&item("/f/7.ARW"), Tier::Thumb).is_none());
    }
}
