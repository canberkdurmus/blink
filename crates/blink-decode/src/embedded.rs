//! Locate every embedded image in a RAW container (VIEW-2, CLI-2).
//!
//! Two complementary passes, both format-agnostic so they work for every container rawler
//! supports (TIFF-based, CR3/BMFF, RAF, ORF maker notes…):
//!
//! 1. A scan for JPEG streams (`FF D8 FF`). Each candidate is validated by walking its marker
//!    segments to EOI, and must be baseline/extended/progressive DCT (SOF0/1/2). Lossless JPEG
//!    (SOF3, used for raw sensor data in CR2/DNG) is rejected. Inside entropy-coded data `FF D8`
//!    can't occur, so nested EXIF thumbnails are found while their parents stay intact.
//! 2. A TIFF walk (IFD0 chain + SubIFDs) for uncompressed 8-bit RGB previews (e.g. Leica DNG).

use crate::tiff::{Tiff, walk_image_ifds};

/// How an embedded image is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddedKind {
    Jpeg,
    /// Uncompressed interleaved 8-bit RGB (TIFF strips).
    Rgb8,
}

impl EmbeddedKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Jpeg => "JPEG",
            Self::Rgb8 => "RGB8",
        }
    }
}

/// One embedded image: where it is and how big it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddedImage {
    pub kind: EmbeddedKind,
    pub offset: usize,
    pub len: usize,
    pub width: u32,
    pub height: u32,
}

impl EmbeddedImage {
    pub fn pixels(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    pub fn long_edge(&self) -> u32 {
        self.width.max(self.height)
    }

    /// The image's bytes inside the container.
    pub fn bytes<'a>(&self, file: &'a [u8]) -> Option<&'a [u8]> {
        file.get(self.offset..self.offset.checked_add(self.len)?)
    }
}

/// Every embedded image in `file`, sorted by offset.
pub fn find(file: &[u8]) -> Vec<EmbeddedImage> {
    let raw_data = raw_data_offsets(file);
    let mut out: Vec<_> = find_jpegs(file)
        .into_iter()
        .filter(|e| !raw_data.contains(&e.offset))
        .collect();
    out.extend(find_tiff_rgb(file));
    out.sort_by_key(|e| e.offset);
    out.dedup_by_key(|e| e.offset);
    out
}

/// The largest embedded image (by pixel count, then long edge, then byte size). VIEW-2.
pub fn largest(images: &[EmbeddedImage]) -> Option<EmbeddedImage> {
    images
        .iter()
        .copied()
        .max_by_key(|e| (e.pixels(), e.long_edge(), e.len))
}

/// Scan for valid JPEG streams.
pub fn find_jpegs(file: &[u8]) -> Vec<EmbeddedImage> {
    memchr::memmem::find_iter(file, &[0xFF, 0xD8, 0xFF])
        .filter_map(|at| {
            parse_jpeg(file, at).map(|(w, h, len)| EmbeddedImage {
                kind: EmbeddedKind::Jpeg,
                offset: at,
                len,
                width: w,
                height: h,
            })
        })
        .collect()
}

fn be16(b: &[u8], at: usize) -> Option<usize> {
    Some(u16::from_be_bytes([*b.get(at)?, *b.get(at + 1)?]) as usize)
}

/// Validate a JPEG starting at `start` (at its SOI). Returns (width, height, byte length).
fn parse_jpeg(b: &[u8], start: usize) -> Option<(u32, u32, usize)> {
    let mut i = start + 2;
    let mut dims: Option<(u32, u32)> = None;
    let mut first = true;
    loop {
        if *b.get(i)? != 0xFF {
            return None;
        }
        let m = *b.get(i + 1)?;
        if m == 0xFF {
            i += 1;
            continue;
        }
        if first {
            // The first segment after SOI must be one a real JPEG starts with.
            if !matches!(m, 0xE0..=0xEF | 0xDB | 0xC4 | 0xC0..=0xC2 | 0xDD | 0xFE) {
                return None;
            }
            first = false;
        }
        i += 2;
        match m {
            0xD8 => return None,
            0x01 | 0xD0..=0xD7 => {}
            0xD9 => {
                let (w, h) = dims?;
                return Some((w, h, i - start));
            }
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => return None,
            0xDA => {
                dims?;
                let len = be16(b, i)?;
                if len < 2 {
                    return None;
                }
                i += len;
                // Entropy-coded data: skip to the next marker that isn't a stuffed 0x00 or RSTn.
                loop {
                    let p = memchr::memchr(0xFF, b.get(i..)?)?;
                    i += p;
                    let n = *b.get(i + 1)?;
                    if n == 0x00 || (0xD0..=0xD7).contains(&n) {
                        i += 2;
                    } else if n == 0xFF {
                        i += 1;
                    } else {
                        break;
                    }
                }
            }
            _ => {
                let len = be16(b, i)?;
                if len < 2 {
                    return None;
                }
                if matches!(m, 0xC0..=0xC2) {
                    if dims.is_some() || *b.get(i + 2)? != 8 {
                        return None;
                    }
                    let h = be16(b, i + 3)? as u32;
                    let w = be16(b, i + 5)? as u32;
                    if w == 0 || h == 0 {
                        return None;
                    }
                    dims = Some((w, h));
                }
                i = i.checked_add(len)?;
            }
        }
    }
}

/// Uncompressed 8-bit RGB images referenced from TIFF IFDs.
fn find_tiff_rgb(file: &[u8]) -> Vec<EmbeddedImage> {
    let mut out = Vec::new();
    let Some(t) = Tiff::parse(file, 0) else {
        return out;
    };
    walk_image_ifds(&t, |ifd| {
        let u = |tag| ifd.get(tag).and_then(|e| t.uint(e));
        let compression = u(0x103).unwrap_or(1);
        let photometric = u(0x106);
        let spp = u(0x115).unwrap_or(1);
        let bps = ifd.get(0x102).map(|e| t.uints(e, 4)).unwrap_or_default();
        if compression != 1 || photometric != Some(2) || spp != 3 {
            return;
        }
        if bps.is_empty() || bps.iter().any(|&b| b != 8) {
            return;
        }
        let (Some(w), Some(h)) = (u(0x100), u(0x101)) else {
            return;
        };
        let (Ok(w), Ok(h)) = (u32::try_from(w), u32::try_from(h)) else {
            return;
        };
        let offs = ifd
            .get(0x111)
            .map(|e| t.uints(e, 1 << 16))
            .unwrap_or_default();
        let counts = ifd
            .get(0x117)
            .map(|e| t.uints(e, 1 << 16))
            .unwrap_or_default();
        if offs.is_empty() || offs.len() != counts.len() {
            return;
        }
        // Strips must be contiguous so the image is one byte range.
        let start = offs[0] as usize;
        let mut end = start;
        for (o, c) in offs.iter().zip(&counts) {
            if *o as usize != end {
                return;
            }
            let Some(e) = end.checked_add(*c as usize) else {
                return;
            };
            end = e;
        }
        let Some(need) = (w as usize)
            .checked_mul(h as usize)
            .and_then(|n| n.checked_mul(3))
        else {
            return;
        };
        if end - start < need || end > file.len() || w == 0 || h == 0 {
            return;
        }
        out.push(EmbeddedImage {
            kind: EmbeddedKind::Rgb8,
            offset: start,
            len: need,
            width: w,
            height: h,
        });
    });
    out
}

/// Strip/tile offsets of full-resolution raw image IFDs (NewSubfileType bit 0 clear, CFA or
/// LinearRaw). Lossy DNGs store the sensor image as baseline-JPEG tiles; those are raw data, not
/// previews.
fn raw_data_offsets(file: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    let Some(t) = Tiff::parse(file, 0) else {
        return out;
    };
    walk_image_ifds(&t, |ifd| {
        let u = |tag| ifd.get(tag).and_then(|e| t.uint(e));
        if u(0xFE).unwrap_or(0) & 1 != 0 || !matches!(u(0x106), Some(32803 | 34892)) {
            return;
        }
        for tag in [0x111, 0x144] {
            if let Some(e) = ifd.get(tag) {
                out.extend(t.uints(e, 1 << 20).into_iter().map(|o| o as usize));
            }
        }
    });
    out.sort_unstable();
    out
}

/// Size of the raw sensor image from TIFF metadata, for containers the RAW backend can't decode:
/// the DNG DefaultCropSize, or the full-resolution CFA/LinearRaw IFD's ImageWidth/Height.
pub fn tiff_raw_size(file: &[u8]) -> Option<(u32, u32)> {
    let t = Tiff::parse(file, 0)?;
    let mut best: Option<(u32, u32)> = None;
    walk_image_ifds(&t, |ifd| {
        let u = |tag| ifd.get(tag).and_then(|e| t.uint(e));
        let subfile = u(0xFE).unwrap_or(0);
        if subfile & 1 != 0 || !matches!(u(0x106), Some(32803 | 34892)) {
            return;
        }
        let size = match ifd.get(0xC620).map(|e| t.uints(e, 2)) {
            Some(v) if v.len() == 2 => Some((v[0] as u32, v[1] as u32)),
            _ => u(0x100).zip(u(0x101)).map(|(w, h)| (w as u32, h as u32)),
        };
        if let Some((w, h)) = size
            && w > 0
            && h > 0
            && best
                .is_none_or(|(bw, bh)| u64::from(w) * u64::from(h) > u64::from(bw) * u64::from(bh))
        {
            best = Some((w, h));
        }
    });
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal structurally valid baseline JPEG header + 1 byte of scan + EOI.
    fn fake_jpeg(w: u16, h: u16, sof: u8) -> Vec<u8> {
        let mut j = vec![0xFF, 0xD8];
        j.extend([0xFF, 0xDB, 0x00, 0x03, 0x00]);
        j.extend([0xFF, sof, 0x00, 0x0B, 8]);
        j.extend(h.to_be_bytes());
        j.extend(w.to_be_bytes());
        j.extend([1, 1, 0x11, 0]);
        j.extend([0xFF, 0xDA, 0x00, 0x08, 1, 1, 0, 0, 63, 0]);
        j.extend([0x12, 0xFF, 0x00, 0x34, 0xFF, 0xD0, 0x56]);
        j.extend([0xFF, 0xD9]);
        j
    }

    #[test]
    fn finds_nested_and_sequential_jpegs_and_rejects_lossless() {
        let mut f = vec![0u8; 100];
        f.extend([0xFF, 0xD8, 0xFF, 0x00]); // bogus SOI
        let a = f.len();
        f.extend(fake_jpeg(160, 120, 0xC0));
        f.extend([0xAB; 10]);
        f.extend(fake_jpeg(4000, 3000, 0xC3)); // lossless raw data
        let b = f.len();
        f.extend(fake_jpeg(1616, 1080, 0xC2));
        let found = find_jpegs(&f);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(
            (found[0].offset, found[0].width, found[0].height),
            (a, 160, 120)
        );
        assert_eq!(found[0].len, fake_jpeg(160, 120, 0xC0).len());
        assert_eq!((found[1].offset, found[1].width), (b, 1616));
        assert_eq!(largest(&found).unwrap().width, 1616);
    }

    #[test]
    fn huge_or_negative_rgb_preview_dimensions_do_not_panic() {
        use crate::tiff::build::*;
        for (w, h) in [(u32::MAX, u32::MAX), (0xFFFF_FFFF, 3), (1 << 31, 1 << 31)] {
            let b = tiff_le(&[
                (0x100, 4, 1, w.to_le_bytes().to_vec()),
                (0x101, 4, 1, h.to_le_bytes().to_vec()),
                short(0x103, 1),
                short(0x106, 2),
                short(0x115, 3),
                (
                    0x102,
                    3,
                    3,
                    [8u16, 8, 8].iter().flat_map(|v| v.to_le_bytes()).collect(),
                ),
                (0x111, 4, 1, 8u32.to_le_bytes().to_vec()),
                (0x117, 4, 1, u32::MAX.to_le_bytes().to_vec()),
            ]);
            assert!(find(&b).is_empty());
        }
        // Negative SLONG width.
        let b = tiff_le(&[
            (0x100, 9, 1, (-5i32).to_le_bytes().to_vec()),
            short(0x101, 4),
        ]);
        assert!(find(&b).is_empty());
    }

    #[test]
    fn lossy_dng_raw_tiles_are_not_previews() {
        use crate::tiff::build::*;
        let tile = fake_jpeg(256, 256, 0xC0);
        let thumb = fake_jpeg(160, 120, 0xC0);
        // IFD0: raw (LinearRaw, NewSubfileType 0) with one tile at a known offset.
        let header_guess = tiff_le(&[
            (0xFE, 4, 1, 0u32.to_le_bytes().to_vec()),
            short(0x106, 34892),
            (0x144, 4, 1, 0u32.to_le_bytes().to_vec()),
        ]);
        let tile_at = header_guess.len() as u32;
        let mut b = tiff_le(&[
            (0xFE, 4, 1, 0u32.to_le_bytes().to_vec()),
            short(0x106, 34892),
            (0x144, 4, 1, tile_at.to_le_bytes().to_vec()),
        ]);
        assert_eq!(b.len(), header_guess.len());
        b.extend(&tile);
        b.extend(&thumb);
        let found = find(&b);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].width, 160);
    }

    #[test]
    fn truncated_jpeg_is_rejected_without_panic() {
        let j = fake_jpeg(10, 10, 0xC0);
        for n in 0..j.len() {
            assert!(find_jpegs(&j[..n]).is_empty());
        }
        assert_eq!(find_jpegs(&j).len(), 1);
    }
}
