//! Capture metadata of every item for filters and the capture-time sort (FILT-1, FILT-2).
//! Read-only, through [`Fs`]: only the start of each file is read.

use crate::fs::Fs;
use blink_core::filter::ItemMeta;
use blink_core::formats::FileKind;
use blink_core::item::Item;
use std::path::{Path, PathBuf};

/// Bytes read from the start of a file. EXIF sits within the first 128 KiB of every corpus file
/// (RAF: inside the embedded JPEG); a file whose prefix yields nothing is read whole.
pub const PREFIX: usize = 256 * 1024;

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
    if e.model.is_some() || e.date_time_original.is_some() || head.len() < PREFIX {
        return Some(from_exif(&e));
    }
    let whole = fs.read(path).ok()?;
    Some(from_exif(&blink_decode::exif::read(&whole)))
}

/// Capture metadata of each item's primary file, in `items` order. Items whose metadata can't
/// be read are left out (metadata filters then exclude them).
pub fn load_meta(fs: &dyn Fs, items: &[Item]) -> Vec<(PathBuf, ItemMeta)> {
    items
        .iter()
        .filter_map(|i| {
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
    fn req_filt_1_metadata_is_read_from_the_file_start_and_whole_only_when_needed() {
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
        // Read-only: no writes, and HEIF isn't opened.
        assert!(fs.calls().iter().all(|c| !c.is_write()));
        assert!(!fs.calls().contains(&FsCall::Read("/f/x.HEIC".into())));
    }
}
