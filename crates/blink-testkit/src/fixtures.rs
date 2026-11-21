//! Synthetic fixtures (SPEC §11.3). Generated in memory by tests and written to
//! `corpus/synthetic/` by `cargo xtask fixtures` for manual inspection.

use jpeg_encoder::{ColorType, Encoder, SamplingFactor};

pub const RED: [u8; 3] = [255, 0, 0];
pub const GREEN: [u8; 3] = [0, 255, 0];
pub const BLUE: [u8; 3] = [0, 0, 255];
pub const WHITE: [u8; 3] = [255, 255, 255];

/// Width and height of the *displayed* orientation fixture.
pub const ORIENT_W: u32 = 64;
pub const ORIENT_H: u32 = 32;

/// Little-endian TIFF with IFD0 (Orientation), an Exif IFD (ColorSpace) and an Interop IFD
/// (InteropIndex). Every value fits inline. `None` omits the tag (and empty IFDs).
pub fn exif_tiff(
    orientation: Option<u16>,
    color_space: Option<u16>,
    interop: Option<&str>,
) -> Vec<u8> {
    type E = (u16, u16, u32, [u8; 4]);
    fn short(tag: u16, v: u16) -> E {
        let b = v.to_le_bytes();
        (tag, 3, 1, [b[0], b[1], 0, 0])
    }
    fn long(tag: u16, v: u32) -> E {
        (tag, 4, 1, v.to_le_bytes())
    }
    fn ifd_len(n: usize) -> u32 {
        (2 + 12 * n + 4) as u32
    }
    fn write(out: &mut Vec<u8>, entries: &[E]) {
        out.extend((entries.len() as u16).to_le_bytes());
        for (tag, typ, count, val) in entries {
            out.extend(tag.to_le_bytes());
            out.extend(typ.to_le_bytes());
            out.extend(count.to_le_bytes());
            out.extend(val);
        }
        out.extend(0u32.to_le_bytes());
    }
    let interop_entries: Vec<E> = interop
        .map(|s| {
            let mut v = [0u8; 4];
            for (i, b) in s.bytes().take(3).enumerate() {
                v[i] = b;
            }
            vec![(0x0001, 2, 4, v)]
        })
        .unwrap_or_default();
    let has_exif = color_space.is_some() || !interop_entries.is_empty();
    let n0 = usize::from(orientation.is_some()) + usize::from(has_exif);
    let n_exif = usize::from(color_space.is_some()) + usize::from(!interop_entries.is_empty());
    let exif_off = 8 + ifd_len(n0);
    let interop_off = exif_off + ifd_len(n_exif);

    let mut ifd0 = Vec::new();
    if let Some(o) = orientation {
        ifd0.push(short(0x0112, o));
    }
    if has_exif {
        ifd0.push(long(0x8769, exif_off));
    }
    let mut exif = Vec::new();
    if let Some(cs) = color_space {
        exif.push(short(0xA001, cs));
    }
    if !interop_entries.is_empty() {
        exif.push(long(0xA005, interop_off));
    }
    let mut out = b"II*\0".to_vec();
    out.extend(8u32.to_le_bytes());
    write(&mut out, &ifd0);
    if has_exif {
        write(&mut out, &exif);
        if !interop_entries.is_empty() {
            write(&mut out, &interop_entries);
        }
    }
    out
}

/// Encode RGB pixels as a 4:4:4 JPEG, optionally with an EXIF APP1 segment (TIFF payload).
pub fn jpeg(rgb: &[u8], w: u32, h: u32, quality: u8, exif: Option<&[u8]>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = Encoder::new(&mut out, quality);
    enc.set_sampling_factor(SamplingFactor::R_4_4_4);
    if let Some(t) = exif {
        let mut seg = b"Exif\0\0".to_vec();
        seg.extend_from_slice(t);
        enc.add_app_segment(1, seg).expect("app1 segment");
    }
    enc.encode(rgb, w as u16, h as u16, ColorType::Rgb)
        .expect("encode jpeg");
    out
}

/// An image of four solid quadrants: `[tl, tr, bl, br]`.
pub fn quadrants(w: u32, h: u32, q: [[u8; 3]; 4]) -> Vec<u8> {
    let mut v = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let i = usize::from(y >= h / 2) * 2 + usize::from(x >= w / 2);
            v.extend_from_slice(&q[i]);
        }
    }
    v
}

/// JPEG whose EXIF orientation is `o` (1–8) and whose *displayed* image (64×32) has red top-left,
/// green top-right, blue bottom-left and white bottom-right. The stored quadrants are derived from
/// the EXIF definition of each orientation, independently of Blink's code.
pub fn orientation_jpeg(o: u16) -> Vec<u8> {
    let (r, g, b, w) = (RED, GREEN, BLUE, WHITE);
    // Stored [tl, tr, bl, br] such that applying orientation `o` yields [r, g, b, w].
    let stored = match o {
        1 => [r, g, b, w],
        2 => [g, r, w, b], // mirrored horizontally
        3 => [w, b, g, r], // rotated 180°
        4 => [b, w, r, g], // mirrored vertically
        5 => [r, b, g, w], // transposed
        6 => [g, w, r, b], // display = stored rotated 90° CW
        7 => [w, g, b, r], // transversed
        8 => [b, r, w, g], // display = stored rotated 90° CCW
        _ => panic!("orientation {o}"),
    };
    let (sw, sh) = if o >= 5 {
        (ORIENT_H, ORIENT_W)
    } else {
        (ORIENT_W, ORIENT_H)
    };
    let tiff = exif_tiff(Some(o), None, None);
    jpeg(&quadrants(sw, sh, stored), sw, sh, 95, Some(&tiff))
}

/// Colour patches used for the Adobe RGB fixture (encoded values).
pub const ADOBE_PATCHES: [[u8; 3]; 8] = [
    [200, 60, 50],
    [60, 180, 70],
    [50, 70, 200],
    [128, 128, 128],
    [230, 200, 150],
    [90, 40, 120],
    [20, 20, 20],
    [240, 240, 240],
];

/// 128×16 JPEG of [`ADOBE_PATCHES`] (16×16 each) tagged as Adobe RGB the way cameras do it:
/// EXIF ColorSpace = 0xFFFF (uncalibrated) and InteropIndex = "R03".
pub fn adobe_rgb_jpeg() -> Vec<u8> {
    let (w, h) = (16 * ADOBE_PATCHES.len() as u32, 16);
    let mut px = Vec::new();
    for _ in 0..h {
        for p in ADOBE_PATCHES {
            for _ in 0..16 {
                px.extend_from_slice(&p);
            }
        }
    }
    let tiff = exif_tiff(Some(1), Some(0xFFFF), Some("R03"));
    jpeg(&px, w, h, 95, Some(&tiff))
}

/// A RAW-like container: a TIFF whose IFD0 points at an embedded JPEG through
/// JPEGInterchangeFormat, followed by filler. Good enough for embedded-preview tests.
pub fn tiff_with_jpegs(jpegs: &[Vec<u8>]) -> Vec<u8> {
    let mut out = exif_tiff(Some(1), None, None);
    for j in jpegs {
        out.extend(std::iter::repeat_n(0x5A, 37));
        out.extend_from_slice(j);
    }
    out
}

/// JPEG encoding options for decoder tests.
#[derive(Clone, Copy, Debug)]
pub struct JpegOpts {
    pub quality: u8,
    /// Luma sampling factors (h, v) with 1×1 chroma: (1,1) 4:4:4, (2,1) 4:2:2, (2,2) 4:2:0,
    /// (1,2) 4:4:0, (4,1) 4:1:1. Ignored for grayscale.
    pub sampling: (u8, u8),
    pub gray: bool,
    pub progressive: bool,
    pub restart_interval: u16,
}

impl Default for JpegOpts {
    fn default() -> Self {
        JpegOpts {
            quality: 90,
            sampling: (2, 2),
            gray: false,
            progressive: false,
            restart_interval: 0,
        }
    }
}

/// Encode an RGB image (converted to luma when `opts.gray`) with the given options.
pub fn jpeg_with(rgb: &[u8], w: u32, h: u32, opts: JpegOpts) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = Encoder::new(&mut out, opts.quality);
    enc.set_sampling_factor(match opts.sampling {
        (1, 1) => SamplingFactor::R_4_4_4,
        (2, 1) => SamplingFactor::R_4_2_2,
        (1, 2) => SamplingFactor::R_4_4_0,
        (4, 1) => SamplingFactor::R_4_1_1,
        _ => SamplingFactor::R_4_2_0,
    });
    enc.set_progressive(opts.progressive);
    if opts.restart_interval > 0 {
        enc.set_restart_interval(opts.restart_interval);
    }
    if opts.gray {
        let luma: Vec<u8> = rgb
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| ((p[0] as u32 * 77 + p[1] as u32 * 150 + p[2] as u32 * 29) >> 8) as u8)
            .collect();
        enc.encode(&luma, w as u16, h as u16, ColorType::Luma)
    } else {
        enc.encode(rgb, w as u16, h as u16, ColorType::Rgb)
    }
    .expect("encode jpeg");
    out
}

/// Deterministic photo-like RGB pixels: gradients plus fine texture (compresses like a
/// detailed photo, ~2 bits per pixel at quality 90).
pub fn photo_rgb(w: u32, h: u32) -> Vec<u8> {
    let mut px = Vec::with_capacity((w * h * 3) as usize);
    let mut seed = 0x1234_5678u32;
    for y in 0..h {
        for x in 0..w {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let n = (seed >> 28) as u8; // 0..15 texture
            let r = ((x * 255 / w) as u8).wrapping_add(n);
            let g = ((y * 255 / h) as u8).wrapping_add(n / 2);
            let b = (((x + y) % 512) / 2) as u8;
            px.extend_from_slice(&[r, g, b.wrapping_add(n)]);
        }
    }
    px
}

/// Deterministic photo-like 6000×4000 RGB image (gradients plus fine texture) that encodes to
/// roughly 6 MB at quality 90 with 4:2:0 subsampling — the PERF-4 reference JPEG.
pub fn perf_jpeg_6mb() -> Vec<u8> {
    let (w, h) = (6000u32, 4000u32);
    let px = photo_rgb(w, h);
    let mut out = Vec::new();
    let mut enc = Encoder::new(&mut out, 90);
    enc.set_sampling_factor(SamplingFactor::R_4_2_0);
    enc.encode(&px, w as u16, h as u16, ColorType::Rgb)
        .expect("encode jpeg");
    out
}

/// A folder of `n` RAW stand-ins for the GUI bench scripts (PERF-1/2/3), created once under
/// `target/perf-fixtures/` and reused: `IMG_0000.ARW`… are hard links to one TIFF-container file
/// with a 160×120 thumbnail and the 6000×4000, ~6 MB JPEG preview of PERF-4 (24 MP class).
/// Hard links keep the folder at one file's size on disk; every name is still a separate item.
pub fn perf_folder(n: usize) -> std::path::PathBuf {
    let dir = crate::corpus::workspace_root()
        .join("target")
        .join("perf-fixtures")
        .join(format!("raw-{n}"));
    if dir.join(".complete").exists() {
        return dir;
    }
    // Several test processes may ask at once: each builds its own temp folder and renames it
    // into place. A folder another process may be reading is never deleted.
    let tmp = dir.with_file_name(format!("raw-{n}.tmp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create perf folder");
    let thumb = jpeg(&[128; 160 * 120 * 3], 160, 120, 80, None);
    let src = tmp.join("source.bin");
    std::fs::write(&src, tiff_with_jpegs(&[thumb, perf_jpeg_6mb()])).expect("write perf file");
    for i in 0..n {
        let p = tmp.join(format!("IMG_{i:04}.ARW"));
        if std::fs::hard_link(&src, &p).is_err() {
            std::fs::copy(&src, &p).expect("copy perf file");
        }
    }
    std::fs::write(tmp.join(".complete"), b"").expect("mark perf folder");
    if std::fs::rename(&tmp, &dir).is_err() {
        // Another process won the race (or a stale incomplete folder is in the way).
        let _ = std::fs::remove_dir_all(&tmp);
        if !dir.join(".complete").exists() {
            let _ = std::fs::remove_dir_all(&dir);
            return perf_folder(n);
        }
    }
    dir
}
