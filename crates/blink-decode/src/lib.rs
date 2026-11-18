//! Blink decode: embedded preview extraction, RAW and JPEG decoding, orientation,
//! colour conversion, histogram and clipping. Bytes in, pixels out: the runtime reads files
//! through its `Fs` trait and hands the bytes over.

pub mod color;
pub mod embedded;
pub mod exif;
pub mod image;
pub mod raw;
pub mod tiff;

use blink_core::formats::FileKind;
use blink_core::preview::PreviewSource;
use color::SourceSpace;
use embedded::{EmbeddedImage, EmbeddedKind};
use exif::ExifInfo;
pub use image::{DecodeError, RgbImage};
use raw::{RawBackend, RawProbe};

/// The default RAW backend.
pub fn default_backend() -> raw::Rawler {
    raw::Rawler
}

/// Container format guessed from the file's magic bytes.
pub fn container_name(file: &[u8]) -> &'static str {
    if file.starts_with(&[0xFF, 0xD8]) {
        "JPEG"
    } else if file.starts_with(b"FUJIFILMCCD-RAW") {
        "Fujifilm RAF"
    } else if file.get(4..8) == Some(b"ftyp") {
        "ISO BMFF"
    } else if file.starts_with(b"IIRO") || file.starts_with(b"IIRS") || file.starts_with(b"MMOR") {
        "TIFF (Olympus ORF)"
    } else if file.starts_with(b"IIU\0") {
        "TIFF (Panasonic RW2)"
    } else if file.starts_with(b"II*\0") || file.starts_with(b"MM\0*") {
        "TIFF"
    } else {
        "unknown"
    }
}

/// Everything Blink learns about a file without decoding pixels.
#[derive(Debug, Clone)]
pub struct Analysis {
    pub kind: FileKind,
    pub container: &'static str,
    pub exif: ExifInfo,
    /// Every embedded image found (RAW/DNG only).
    pub embedded: Vec<EmbeddedImage>,
    /// The largest embedded image (VIEW-2).
    pub chosen: Option<EmbeddedImage>,
    /// RAW backend result (RAW/DNG only).
    pub raw: Option<Result<RawProbe, DecodeError>>,
    /// Sensor size: backend crop, else TIFF raw IFD size.
    pub sensor: Option<(u32, u32)>,
}

impl Analysis {
    /// LOW-RES verdict for the chosen preview; `None` when it can't be decided.
    pub fn low_res(&self) -> Option<bool> {
        let c = self.chosen?;
        let (w, h) = self.sensor?;
        Some(blink_core::preview::is_low_res(c.long_edge(), w.max(h)))
    }

    /// The preview source the loupe shows by default for this file alone (no pairing).
    pub fn default_source(&self) -> PreviewSource {
        match self.kind {
            FileKind::Jpeg => PreviewSource::Jpeg,
            FileKind::Heif => PreviewSource::Unavailable,
            FileKind::Raw | FileKind::Dng => match self.chosen {
                Some(c) => {
                    PreviewSource::for_embedded(c.long_edge(), self.sensor.map(|(w, h)| w.max(h)))
                }
                None => PreviewSource::Unavailable,
            },
        }
    }
}

/// Analyse a file's bytes.
pub fn analyze(file: &[u8], kind: FileKind, backend: &dyn RawBackend) -> Analysis {
    let exif = if kind == FileKind::Heif {
        ExifInfo::default()
    } else {
        exif::read(file)
    };
    let (embedded, raw) = if kind.is_raw_like() {
        (embedded::find(file), Some(backend.probe(file)))
    } else {
        (Vec::new(), None)
    };
    let sensor = match &raw {
        Some(Ok(p)) => Some((p.sensor_w, p.sensor_h)),
        _ if kind.is_raw_like() => embedded::tiff_raw_size(file),
        _ => None,
    };
    let chosen = embedded::largest(&embedded);
    Analysis {
        kind,
        container: container_name(file),
        exif,
        embedded,
        chosen,
        raw,
        sensor,
    }
}

/// A decoded preview ready for display: oriented, in sRGB.
#[derive(Debug, Clone)]
pub struct Preview {
    pub image: RgbImage,
    pub source: PreviewSource,
    /// The colour space the pixels were converted from.
    pub space: SourceSpace,
    /// The EXIF orientation that was applied.
    pub orientation: u16,
}

fn space_of(icc: Option<Vec<u8>>, own: Option<&ExifInfo>, container: &ExifInfo) -> SourceSpace {
    if let Some(icc) = icc {
        return SourceSpace::Icc(icc);
    }
    let adobe = match own {
        Some(e) if e.color_space.is_some() || e.interop_index.is_some() => e.is_adobe_rgb(),
        _ => container.is_adobe_rgb(),
    };
    if adobe {
        SourceSpace::AdobeRgb
    } else {
        SourceSpace::Srgb
    }
}

fn finish(
    mut image: RgbImage,
    space: SourceSpace,
    orientation: u16,
    source: PreviewSource,
) -> Result<Preview, DecodeError> {
    color::to_srgb(&mut image, &space).map_err(DecodeError)?;
    Ok(Preview {
        image: image::orient(&image, orientation),
        source,
        space,
        orientation,
    })
}

/// Decode a whole JPEG file for display (standalone or paired JPEG).
pub fn decode_jpeg_file(bytes: &[u8], source: PreviewSource) -> Result<Preview, DecodeError> {
    let d = image::decode_jpeg(bytes)?;
    let own = d
        .exif
        .as_deref()
        .map(exif::from_tiff_bytes)
        .unwrap_or_default();
    let space = space_of(d.icc, None, &own);
    finish(d.image, space, own.orientation.unwrap_or(1), source)
}

/// Decode the chosen embedded preview of a RAW/DNG for display. Orientation comes from the
/// container's EXIF (embedded previews are stored unrotated).
pub fn decode_embedded(file: &[u8], a: &Analysis) -> Result<Preview, DecodeError> {
    let c = a
        .chosen
        .ok_or_else(|| DecodeError("no embedded preview".into()))?;
    let bytes = c
        .bytes(file)
        .ok_or_else(|| DecodeError("preview out of bounds".into()))?;
    let source = a.default_source();
    let orientation = a.exif.orientation.unwrap_or(1);
    match c.kind {
        EmbeddedKind::Jpeg => {
            let d = image::decode_jpeg(bytes)?;
            let own = d.exif.as_deref().map(exif::from_tiff_bytes);
            let space = space_of(d.icc, own.as_ref(), &a.exif);
            finish(d.image, space, orientation, source)
        }
        EmbeddedKind::Rgb8 => {
            let img = RgbImage::new(c.width, c.height, bytes.to_vec())
                .ok_or_else(|| DecodeError("bad RGB preview size".into()))?;
            finish(img, space_of(None, None, &a.exif), orientation, source)
        }
    }
}
