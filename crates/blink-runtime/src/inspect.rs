//! Inspect one file: read it through [`Fs`], analyse it, decode its preview, and gather the info
//! panel values (CLI-2 `doctor`, VIEW-7). Read-only.

use crate::fs::Fs;
use blink_core::formats::{FileKind, Formats};
use blink_core::info::InfoInput;
use blink_core::preview::PreviewSource;
use blink_decode::raw::RawBackend;
use blink_decode::{Analysis, Preview};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Everything `doctor` reports about a file.
#[derive(Debug)]
pub struct Inspection {
    pub path: PathBuf,
    pub len: u64,
    pub kind: FileKind,
    pub analysis: Analysis,
    /// Decoded default preview (RAW/DNG: chosen embedded image; JPEG: the file).
    pub preview: Option<Result<Preview, String>>,
    pub read_time: Duration,
    pub analyze_time: Duration,
    pub decode_time: Duration,
}

/// Inspection errors.
#[derive(Debug)]
pub enum InspectError {
    Unsupported(PathBuf),
    Io(PathBuf, std::io::Error),
}

impl std::fmt::Display for InspectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(p) => write!(f, "{}: unsupported file type", p.display()),
            Self::Io(p, e) => write!(f, "{}: {e}", p.display()),
        }
    }
}

impl std::error::Error for InspectError {}

pub fn inspect(
    fs: &dyn Fs,
    path: &Path,
    formats: &Formats,
    backend: &dyn RawBackend,
) -> Result<Inspection, InspectError> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let kind = formats
        .classify(&name)
        .ok_or_else(|| InspectError::Unsupported(path.to_path_buf()))?;
    let t = Instant::now();
    let bytes = fs
        .read(path)
        .map_err(|e| InspectError::Io(path.to_path_buf(), e))?;
    let read_time = t.elapsed();
    let t = Instant::now();
    let analysis = blink_decode::analyze(&bytes, kind, backend);
    let analyze_time = t.elapsed();
    let t = Instant::now();
    let preview = match kind {
        FileKind::Heif => None,
        FileKind::Jpeg => Some(
            blink_decode::decode_jpeg_file(&bytes, PreviewSource::Jpeg).map_err(|e| e.to_string()),
        ),
        FileKind::Raw | FileKind::Dng => analysis
            .chosen
            .map(|_| blink_decode::decode_embedded(&bytes, &analysis).map_err(|e| e.to_string())),
    };
    let decode_time = t.elapsed();
    Ok(Inspection {
        path: path.to_path_buf(),
        len: bytes.len() as u64,
        kind,
        analysis,
        preview,
        read_time,
        analyze_time,
        decode_time,
    })
}

impl Inspection {
    /// The preview source the loupe would show.
    pub fn source(&self) -> PreviewSource {
        match &self.preview {
            Some(Ok(p)) => p.source,
            _ => PreviewSource::Unavailable,
        }
    }

    /// Info panel values (VIEW-7).
    pub fn info(&self) -> InfoInput {
        let e = &self.analysis.exif;
        let preview_size = match &self.preview {
            Some(Ok(p)) => Some((p.image.width, p.image.height)),
            _ => None,
        };
        let dimensions = match self.kind {
            FileKind::Jpeg => preview_size,
            _ => self.analysis.sensor,
        };
        InfoInput {
            file_name: self
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            make: e.make.clone(),
            model: e.model.clone(),
            lens: e.lens.clone(),
            focal_length_mm: e.focal_length,
            exposure_time_s: e.exposure_time,
            f_number: e.f_number,
            iso: e.iso,
            exposure_compensation_ev: e.exposure_compensation,
            date_time_original: e.date_time_original.clone(),
            offset_time_original: e.offset_time_original.clone(),
            dimensions,
            preview_source: Some(self.source()),
            preview_size,
        }
    }
}
