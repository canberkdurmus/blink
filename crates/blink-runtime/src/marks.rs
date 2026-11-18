//! Load ratings and labels for items on open (RATE-4, ADR 0009). Read-only: never writes.

use crate::fs::Fs;
use blink_core::formats::FileKind;
use blink_core::item::{Item, Member};
use blink_core::state::Marks;
use std::path::{Path, PathBuf};

/// Sidecar paths that may belong to `source`: Adobe-style `IMG.xmp` / `IMG.XMP` and
/// darktable-style `IMG.ARW.xmp` / `IMG.ARW.XMP`.
pub fn sidecar_candidates(source: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for ext in ["xmp", "XMP"] {
        out.push(source.with_extension(ext));
    }
    if let Some(name) = source.file_name() {
        for ext in ["xmp", "XMP"] {
            let mut n = name.to_os_string();
            n.push(".");
            n.push(ext);
            out.push(source.with_file_name(n));
        }
    }
    out
}

/// The sidecar to read for `source`: the newest existing candidate; Adobe-style wins ties.
pub fn find_sidecar(fs: &dyn Fs, source: &Path) -> Option<PathBuf> {
    let mut best: Option<(PathBuf, Option<std::time::SystemTime>)> = None;
    for c in sidecar_candidates(source) {
        let Ok(m) = fs.metadata(&c) else { continue };
        if m.is_dir || best.as_ref().is_some_and(|(p, _)| *p == c) {
            continue;
        }
        let newer = match &best {
            None => true,
            Some((_, t)) => m.modified > *t,
        };
        if newer {
            best = Some((c, m.modified));
        }
    }
    best.map(|(p, _)| p)
}

fn to_marks(f: blink_xmp::OwnedFields) -> Marks {
    Marks {
        rating: f.rating,
        label: f.label,
    }
}

fn from_sidecar(fs: &dyn Fs, m: &Member) -> Option<Marks> {
    let p = find_sidecar(fs, &m.path)?;
    let bytes = fs.read(&p).ok()?;
    blink_xmp::read_owned(&bytes).ok().map(to_marks)
}

fn from_embedded(fs: &dyn Fs, m: &Member) -> Option<Marks> {
    if !matches!(m.kind, FileKind::Dng | FileKind::Jpeg) {
        return None;
    }
    let bytes = fs.read(&m.path).ok()?;
    let packet = match m.kind {
        FileKind::Jpeg => blink_xmp::jpeg_xmp_packet(&bytes),
        _ => blink_xmp::tiff_xmp_packet(&bytes),
    }?;
    blink_xmp::read_owned(packet).ok().map(to_marks)
}

/// Marks of one item: the first member (primary first) with a readable sidecar; otherwise the
/// embedded XMP of a DNG/JPEG member. `None` when nothing is recorded.
pub fn load_item_marks(fs: &dyn Fs, item: &Item) -> Option<Marks> {
    item.members()
        .find_map(|m| from_sidecar(fs, m))
        .or_else(|| item.members().find_map(|m| from_embedded(fs, m)))
}

/// Marks for every item that has any, keyed by primary path (feeds `Action::MarksLoaded`).
pub fn load_marks(fs: &dyn Fs, items: &[Item]) -> Vec<(PathBuf, Marks)> {
    items
        .iter()
        .filter_map(|i| load_item_marks(fs, i).map(|m| (i.primary.path.clone(), m)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::{FsCall, MemFs};
    use blink_core::formats::Formats;
    use blink_core::item::build_items;
    use blink_testkit::sidecars;
    use std::time::{Duration, SystemTime};

    fn items(fs: &MemFs, dir: &str) -> Vec<Item> {
        let files = crate::scan::scan_folder(fs, Path::new(dir), false).unwrap();
        build_items(&files, &Formats::new(["arw", "nef"]))
    }

    fn marks(r: i8, l: Option<&str>) -> Marks {
        Marks {
            rating: Some(r),
            label: l.map(str::to_string),
        }
    }

    #[test]
    fn req_rate_4_sidecars_from_each_app_are_read_on_open() {
        let fs = MemFs::new();
        fs.add_file("/s/a.ARW", b"raw".to_vec());
        fs.add_file("/s/a.xmp", sidecars::lightroom(4, Some("Red")));
        fs.add_file("/s/b.ARW", b"raw".to_vec());
        fs.add_file("/s/b.ARW.xmp", sidecars::darktable(-1));
        fs.add_file("/s/c.NEF", b"raw".to_vec());
        fs.add_file("/s/c.xmp", sidecars::bridge(5, Some("Green")));
        fs.add_file("/s/d.ARW", b"raw".to_vec());
        fs.add_file("/s/d.XMP", sidecars::digikam(3, Some("Purple")));
        fs.add_file("/s/e.ARW", b"raw".to_vec());
        let got = load_marks(&fs, &items(&fs, "/s"));
        assert_eq!(
            got,
            vec![
                ("/s/a.ARW".into(), marks(4, Some("Red"))),
                ("/s/b.ARW".into(), marks(-1, None)),
                ("/s/c.NEF".into(), marks(5, Some("Green"))),
                ("/s/d.ARW".into(), marks(3, Some("Purple"))),
            ]
        );
        // Read-only: only metadata/read calls.
        assert!(fs.calls().iter().all(|c| matches!(
            c,
            FsCall::Read(_) | FsCall::Metadata(_) | FsCall::ReadDir(_)
        )));
    }

    #[test]
    fn req_rate_4_newest_sidecar_wins_when_both_styles_exist() {
        let fs = MemFs::new();
        fs.add_file("/s/a.ARW", b"raw".to_vec());
        fs.add_file("/s/a.xmp", sidecars::lightroom(1, None));
        fs.add_file("/s/a.ARW.xmp", sidecars::darktable(2));
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        fs.set_modified("/s/a.xmp", t0);
        fs.set_modified("/s/a.ARW.xmp", t0 + Duration::from_secs(5));
        let it = items(&fs, "/s");
        assert_eq!(load_item_marks(&fs, &it[0]), Some(marks(2, None)));
        fs.set_modified("/s/a.xmp", t0 + Duration::from_secs(9));
        assert_eq!(load_item_marks(&fs, &it[0]), Some(marks(1, None)));
        // Equal times: Adobe-style (D-3 default).
        fs.set_modified("/s/a.ARW.xmp", t0 + Duration::from_secs(9));
        assert_eq!(load_item_marks(&fs, &it[0]), Some(marks(1, None)));
    }

    #[test]
    fn req_rate_4_embedded_xmp_used_when_no_sidecar_exists() {
        let packet = sidecars::bridge(2, Some("Blue"));
        let mut seg = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
        seg.extend_from_slice(packet.as_bytes());
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
        jpeg.extend(((seg.len() + 2) as u16).to_be_bytes());
        jpeg.extend(&seg);
        jpeg.extend([0xFF, 0xD9]);
        let fs = MemFs::new();
        fs.add_file("/s/p.JPG", jpeg.clone());
        fs.add_file("/s/q.JPG", jpeg);
        fs.add_file("/s/q.xmp", sidecars::lightroom(5, None));
        let it = items(&fs, "/s");
        assert_eq!(load_item_marks(&fs, &it[0]), Some(marks(2, Some("Blue"))));
        // A sidecar beats embedded XMP.
        assert_eq!(load_item_marks(&fs, &it[1]), Some(marks(5, None)));
        // Display only: nothing but reads, and no sidecar appears for the embedded-only file.
        assert!(fs.calls().iter().all(|c| matches!(
            c,
            FsCall::Read(_) | FsCall::Metadata(_) | FsCall::ReadDir(_)
        )));
        assert!(!fs.exists(Path::new("/s/p.xmp")) && !fs.exists(Path::new("/s/p.JPG.xmp")));
    }

    #[test]
    fn req_rate_4_pair_uses_companion_sidecar_when_primary_has_none() {
        let fs = MemFs::new();
        fs.add_file("/s/a.ARW", b"raw".to_vec());
        fs.add_file("/s/a.JPG", vec![0xFF, 0xD8, 0xFF, 0xD9]);
        fs.add_file("/s/a.JPG.xmp", sidecars::digikam(3, None));
        let it = items(&fs, "/s");
        assert_eq!(it.len(), 1);
        assert_eq!(load_item_marks(&fs, &it[0]), Some(marks(3, None)));
    }
}
