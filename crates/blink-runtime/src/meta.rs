//! Capture metadata of every item for filters and the capture-time sort (FILT-1, FILT-2).
//! Read-only, through [`Fs`]: only the start of each file is read.

use crate::fs::Fs;
use blink_core::filter::ItemMeta;
use blink_core::formats::FileKind;
use blink_core::item::Item;
use std::path::{Path, PathBuf};

/// Bytes read from the start of a file. EXIF sits within the first 128 KiB of every corpus file
/// (RAF: inside the embedded JPEG).
pub const PREFIX: usize = 256 * 1024;
/// A file whose first [`PREFIX`] bytes yield no EXIF value at all is read again up to this
/// size. Never the whole file: 2,000 RAWs read whole would compete with the decoder (PERF-1).
pub const LONG_PREFIX: usize = 4 * 1024 * 1024;

fn from_exif(e: &blink_decode::exif::ExifInfo) -> ItemMeta {
    ItemMeta {
        body: blink_core::info::body(e.make.as_deref(), e.model.as_deref()),
        lens: e.lens.clone(),
        focal_mm: e.focal_length,
        iso: e.iso,
        captured: e.date_time_original.clone(),
    }
}

/// Capture metadata of one file. `None`: unreadable, or a HEIF (not parsed in v1, D-4).
pub fn file_meta(fs: &dyn Fs, path: &Path, kind: FileKind) -> Option<ItemMeta> {
    if kind == FileKind::Heif {
        return None;
    }
    let head = fs.read_prefix(path, PREFIX).ok()?;
    let e = blink_decode::exif::read(&head);
    if e != blink_decode::exif::ExifInfo::default() || head.len() < PREFIX {
        return Some(from_exif(&e));
    }
    let longer = fs.read_prefix(path, LONG_PREFIX).ok()?;
    Some(from_exif(&blink_decode::exif::read(&longer)))
}

/// Capture metadata of each item's primary file, in `items` order. Items whose metadata can't
/// be read are left out (metadata filters then exclude them).
pub fn load_meta(fs: &dyn Fs, items: &[Item]) -> Vec<(PathBuf, ItemMeta)> {
    load_meta_yielding(fs, items, &|| false)
}

/// [`load_meta`], waiting before each file while `busy()` (the image loader has work), so the
/// scan only uses I/O and CPU the loupe doesn't need (PERF-1, PERF-5).
pub fn load_meta_yielding(
    fs: &dyn Fs,
    items: &[Item],
    busy: &dyn Fn() -> bool,
) -> Vec<(PathBuf, ItemMeta)> {
    items
        .iter()
        .filter_map(|i| {
            while busy() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            file_meta(fs, &i.primary.path, i.primary.kind).map(|m| (i.primary.path.clone(), m))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::{FsCall, MemFs};
    use blink_core::item::Member;

    /// A little-endian TIFF whose IFD0 holds `Model`, its text stored at `at`.
    fn tiff_with_model(model: &str, at: usize) -> Vec<u8> {
        let text = format!("{model}\0");
        let mut b = b"II*\0".to_vec();
        b.extend(8u32.to_le_bytes());
        b.extend(1u16.to_le_bytes());
        b.extend(0x0110u16.to_le_bytes());
        b.extend(2u16.to_le_bytes());
        b.extend((text.len() as u32).to_le_bytes());
        b.extend((at as u32).to_le_bytes());
        b.extend(0u32.to_le_bytes());
        b.resize(at, 0);
        b.extend(text.as_bytes());
        b
    }

    fn item(path: &str, kind: FileKind) -> Item {
        Item {
            primary: Member {
                path: path.into(),
                kind,
            },
            companions: vec![],
        }
    }

    #[test]
    fn req_filt_1_metadata_is_read_from_the_file_start_and_never_whole() {
        let fs = MemFs::new();
        fs.add_file("/f/near.DNG", tiff_with_model("NEAR", 64));
        fs.add_file("/f/far.DNG", tiff_with_model("FAR AWAY", PREFIX + 10));
        fs.add_file("/f/x.HEIC", b"heif".to_vec());
        let items = [
            item("/f/near.DNG", FileKind::Dng),
            item("/f/far.DNG", FileKind::Dng),
            item("/f/x.HEIC", FileKind::Heif),
            item("/f/gone.DNG", FileKind::Dng),
        ];
        let got = load_meta(&fs, &items);
        let bodies: Vec<_> = got
            .iter()
            .map(|(p, m)| (p.display().to_string(), m.body.clone()))
            .collect();
        assert_eq!(
            bodies,
            [
                ("/f/near.DNG".to_string(), Some("NEAR".to_string())),
                ("/f/far.DNG".to_string(), Some("FAR AWAY".to_string())),
            ]
        );
        // Never a whole-file read: prefixes only (PERF-1).
        assert!(!fs.calls().iter().any(|c| matches!(c, FsCall::Read(_))));
        // A file with EXIF in the prefix but no model or date isn't read again.
        let fs2 = MemFs::new();
        let mut b = tiff_with_model("XCAMERA", 64);
        b.resize(PREFIX * 2, 0);
        fs2.add_file("/f/y.DNG", b);
        let got = load_meta(&fs2, &[item("/f/y.DNG", FileKind::Dng)]);
        assert_eq!(got[0].1.body.as_deref(), Some("XCAMERA"));
        assert_eq!(fs2.calls(), [FsCall::ReadPrefix("/f/y.DNG".into(), PREFIX)]);
        // The far one needed the longer prefix.
        assert!(
            fs.calls()
                .contains(&FsCall::ReadPrefix("/f/far.DNG".into(), LONG_PREFIX))
        );
        // Read-only: no writes, and HEIF isn't opened.
        assert!(fs.calls().iter().all(|c| !c.is_write()));
        assert!(
            !fs.calls()
                .iter()
                .any(|c| matches!(c, FsCall::ReadPrefix(p, _) if p.ends_with("x.HEIC")))
        );
    }

    #[test]
    fn req_filt_1_the_metadata_scan_waits_while_the_loader_is_busy() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let fs = MemFs::new();
        fs.add_file("/f/a.DNG", tiff_with_model("CAMERA A", 64));
        let polls = AtomicUsize::new(0);
        // Busy for the first three polls: no file is read until it isn't.
        let busy = || {
            let n = polls.fetch_add(1, Ordering::SeqCst);
            if n < 3 {
                assert!(fs.calls().is_empty(), "read while the loader was busy");
            }
            n < 3
        };
        let got = load_meta_yielding(&fs, &[item("/f/a.DNG", FileKind::Dng)], &busy);
        assert_eq!(got.len(), 1);
        assert_eq!(polls.load(Ordering::SeqCst), 4);
    }
}
