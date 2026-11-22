//! Standard EXIF fields for the info panel (VIEW-7), orientation (VIEW-8) and colour space
//! (VIEW-10). Reads IFD0 + ExifIFD + InteropIFD from TIFF-based RAWs, JPEG APP1, Fujifilm RAF
//! (via its embedded JPEG) and Canon CR3 (CMT1/CMT2 boxes). Maker notes are not parsed (ADR 0006).

use crate::tiff::{Ifd, Tiff};

/// EXIF values Blink shows. `None` means the file does not carry the tag.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExifInfo {
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens: Option<String>,
    pub focal_length: Option<f64>,
    pub exposure_time: Option<f64>,
    pub f_number: Option<f64>,
    pub iso: Option<u32>,
    pub exposure_compensation: Option<f64>,
    pub date_time_original: Option<String>,
    pub offset_time_original: Option<String>,
    pub orientation: Option<u16>,
    pub color_space: Option<u16>,
    pub interop_index: Option<String>,
}

const T_MAKE: u16 = 0x010F;
const T_MODEL: u16 = 0x0110;
const T_ORIENTATION: u16 = 0x0112;
const T_EXIF_IFD: u16 = 0x8769;
const T_EXPOSURE_TIME: u16 = 0x829A;
const T_FNUMBER: u16 = 0x829D;
const T_ISO: u16 = 0x8827;
const T_DTO: u16 = 0x9003;
const T_OFFSET_TIME_ORIGINAL: u16 = 0x9011;
const T_EXPOSURE_BIAS: u16 = 0x9204;
const T_FOCAL_LENGTH: u16 = 0x920A;
const T_COLOR_SPACE: u16 = 0xA001;
const T_INTEROP_IFD: u16 = 0xA005;
const T_LENS_MODEL: u16 = 0xA434;
const T_INTEROP_INDEX: u16 = 0x0001;

impl ExifInfo {
    /// Fill fields that are still `None` from one IFD.
    fn merge_ifd(&mut self, t: &Tiff<'_>, ifd: &Ifd) {
        let s = |tag| ifd.get(tag).and_then(|e| t.ascii(e));
        let r = |tag| ifd.get(tag).and_then(|e| t.rational(e));
        let u = |tag| ifd.get(tag).and_then(|e| t.uint(e));
        fill(&mut self.make, s(T_MAKE));
        fill(&mut self.model, s(T_MODEL));
        fill(&mut self.lens, s(T_LENS_MODEL));
        fill(&mut self.focal_length, r(T_FOCAL_LENGTH));
        fill(&mut self.exposure_time, r(T_EXPOSURE_TIME));
        fill(&mut self.f_number, r(T_FNUMBER));
        fill(
            &mut self.iso,
            u(T_ISO)
                .and_then(|v| u32::try_from(v).ok())
                .filter(|&v| v > 0),
        );
        fill(&mut self.exposure_compensation, r(T_EXPOSURE_BIAS));
        fill(&mut self.date_time_original, s(T_DTO));
        fill(&mut self.offset_time_original, s(T_OFFSET_TIME_ORIGINAL));
        fill(
            &mut self.orientation,
            u(T_ORIENTATION)
                .and_then(|v| u16::try_from(v).ok())
                .filter(|v| (1..=8).contains(v)),
        );
        fill(
            &mut self.color_space,
            u(T_COLOR_SPACE).and_then(|v| u16::try_from(v).ok()),
        );
    }

    /// Read IFD0 of `t` and the Exif/Interop IFDs it points to. ExifIFD values win over IFD0.
    fn read_tiff(&mut self, t: &Tiff<'_>) {
        let Some(ifd0) = t.ifd(t.first_ifd()) else {
            return;
        };
        if let Some(exif) = ifd0
            .get(T_EXIF_IFD)
            .and_then(|e| t.uint(e))
            .and_then(|o| t.ifd(o as usize))
        {
            self.merge_ifd(t, &exif);
            if let Some(interop) = exif
                .get(T_INTEROP_IFD)
                .and_then(|e| t.uint(e))
                .and_then(|o| t.ifd(o as usize))
            {
                fill(
                    &mut self.interop_index,
                    interop.get(T_INTEROP_INDEX).and_then(|e| t.ascii(e)),
                );
            }
        }
        self.merge_ifd(t, &ifd0);
    }

    /// Whether the EXIF data declares Adobe RGB: ColorSpace = 2 (non-standard but written by some
    /// bodies), or ColorSpace = 0xFFFF (uncalibrated) with Interop index `R03` (DCF option file).
    pub fn is_adobe_rgb(&self) -> bool {
        match self.color_space {
            Some(2) => true,
            Some(0xFFFF) => self.interop_index.as_deref() == Some("R03"),
            None => self.interop_index.as_deref() == Some("R03"),
            _ => false,
        }
    }
}

fn fill<T>(slot: &mut Option<T>, v: Option<T>) {
    if slot.is_none() {
        *slot = v;
    }
}

/// EXIF from a bare TIFF block (e.g. the payload of a JPEG APP1 `Exif\0\0` segment).
pub fn from_tiff_bytes(b: &[u8]) -> ExifInfo {
    let mut info = ExifInfo::default();
    if let Some(t) = Tiff::parse(b, 0) {
        info.read_tiff(&t);
    }
    info
}

/// The TIFF payload of a JPEG's first APP1 `Exif` segment.
pub fn jpeg_exif_payload(jpeg: &[u8]) -> Option<&[u8]> {
    if jpeg.get(0..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2;
    loop {
        if *jpeg.get(i)? != 0xFF {
            return None;
        }
        let m = *jpeg.get(i + 1)?;
        if m == 0xFF {
            i += 1;
            continue;
        }
        if m == 0xDA || m == 0xD9 {
            return None;
        }
        let len = u16::from_be_bytes([*jpeg.get(i + 2)?, *jpeg.get(i + 3)?]) as usize;
        if len < 2 {
            return None;
        }
        let seg = jpeg.get(i + 4..i + 2 + len)?;
        if m == 0xE1 && seg.starts_with(b"Exif\0\0") {
            return Some(&seg[6..]);
        }
        i += 2 + len;
    }
}

/// Find an ISO-BMFF box of type `name` anywhere in the first `limit` bytes (CR3 metadata boxes
/// sit at the start of the file). Returns its payload.
fn find_box<'a>(b: &'a [u8], name: &[u8; 4], limit: usize) -> Option<&'a [u8]> {
    let hay = &b[..b.len().min(limit)];
    let mut from = 0;
    while let Some(p) = hay[from..].windows(4).position(|w| w == name) {
        let at = from + p;
        if at >= 4 {
            let size = u32::from_be_bytes(b[at - 4..at].try_into().ok()?) as usize;
            if size >= 8
                && let Some(payload) = b.get(at + 4..at - 4 + size)
            {
                return Some(payload);
            }
        }
        from = at + 1;
    }
    None
}

/// Read EXIF from a whole file of any supported container.
pub fn read(file: &[u8]) -> ExifInfo {
    let mut info = ExifInfo::default();
    if file.starts_with(&[0xFF, 0xD8]) {
        if let Some(p) = jpeg_exif_payload(file) {
            info = from_tiff_bytes(p);
        }
    } else if file.starts_with(b"FUJIFILMCCD-RAW") {
        // RAF: big-endian offset/length of the embedded JPEG at bytes 84..92.
        let off = file
            .get(84..88)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize);
        let len = file
            .get(88..92)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize);
        if let (Some(off), Some(len)) = (off, len)
            // A file prefix (metadata scan) may end inside the JPEG; its EXIF comes first.
            && let Some(jpeg) = file.get(off..off.saturating_add(len).min(file.len()))
            && let Some(p) = jpeg_exif_payload(jpeg)
        {
            info = from_tiff_bytes(p);
        }
    } else if file.get(4..8) == Some(b"ftyp") {
        // CR3: CMT1 = IFD0, CMT2 = Exif IFD, each a standalone TIFF.
        const LIMIT: usize = 1 << 20;
        if let Some(cmt2) = find_box(file, b"CMT2", LIMIT)
            && let Some(t) = Tiff::parse(cmt2, 0)
            && let Some(ifd) = t.ifd(t.first_ifd())
        {
            info.merge_ifd(&t, &ifd);
        }
        if let Some(cmt1) = find_box(file, b"CMT1", LIMIT)
            && let Some(t) = Tiff::parse(cmt1, 0)
            && let Some(ifd) = t.ifd(t.first_ifd())
        {
            info.merge_ifd(&t, &ifd);
        }
    } else if let Some(t) = Tiff::parse(file, 0) {
        info.read_tiff(&t);
        // Panasonic RW2 keeps ISO in IFD0 tag 0x0017; its reduced ExifIFD has none.
        if file.starts_with(b"IIU\0")
            && let Some(ifd0) = t.ifd(t.first_ifd())
        {
            fill(
                &mut info.iso,
                ifd0.get(0x0017)
                    .and_then(|e| t.uint(e))
                    .and_then(|v| u32::try_from(v).ok())
                    .filter(|&v| v > 0),
            );
        }
    }
    info
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tiff::build::*;

    #[test]
    fn tiff_exif_fields_and_adobe_detection() {
        let b = tiff_le(&[
            ascii(T_MAKE, "SONY"),
            short(T_ORIENTATION, 8),
            ascii(T_DTO, "2012:07:15 16:30:58"),
            short(T_COLOR_SPACE, 0xFFFF),
        ]);
        let e = from_tiff_bytes(&b);
        assert_eq!(e.make.as_deref(), Some("SONY"));
        assert_eq!(e.orientation, Some(8));
        assert_eq!(e.date_time_original.as_deref(), Some("2012:07:15 16:30:58"));
        assert!(!e.is_adobe_rgb());
        let adobe = ExifInfo {
            color_space: Some(0xFFFF),
            interop_index: Some("R03".into()),
            ..Default::default()
        };
        assert!(adobe.is_adobe_rgb());
        assert!(
            ExifInfo {
                color_space: Some(2),
                ..Default::default()
            }
            .is_adobe_rgb()
        );
        assert!(
            !ExifInfo {
                color_space: Some(1),
                interop_index: Some("R98".into()),
                ..Default::default()
            }
            .is_adobe_rgb()
        );
    }

    #[test]
    fn garbage_never_panics() {
        for n in 0..64 {
            let junk: Vec<u8> = (0..n).map(|i| (i * 37 % 251) as u8).collect();
            let _ = read(&junk);
        }
        let _ = read(b"FUJIFILMCCD-RAW 0201");
        let _ = read(b"\0\0\0\x18ftypcrx \0\0\0\x10CMT1II*\0");
    }
}
