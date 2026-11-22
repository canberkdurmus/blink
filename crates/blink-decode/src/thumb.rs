//! Grid thumbnails (VIEW-5, PERF-6): the smallest embedded image that is big enough, decoded
//! on one thread, box-downscaled, converted to sRGB and oriented. Works on a file prefix when
//! the image lies inside it, so most thumbnails never read the whole file.

use crate::embedded::{self, EmbeddedImage, EmbeddedKind};
use crate::image::{self, DecodeError, RgbImage};
use crate::{Preview, exif, finish, space_of};
use blink_core::formats::FileKind;
use blink_core::preview::PreviewSource;

/// Why a thumbnail wasn't made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThumbError {
    /// The bytes given are a prefix and the image needed lies beyond it: read the whole file.
    NeedWholeFile,
    Failed(DecodeError),
}

impl From<DecodeError> for ThumbError {
    fn from(e: DecodeError) -> Self {
        Self::Failed(e)
    }
}

/// The embedded image a thumbnail with long edge `min_edge` is made from: the smallest whose
/// long edge reaches it, else the largest there is.
pub fn pick(images: &[EmbeddedImage], min_edge: u32) -> Option<EmbeddedImage> {
    let mut by_size: Vec<_> = images.to_vec();
    by_size.sort_by_key(|e| (e.pixels(), e.long_edge(), e.len));
    by_size
        .iter()
        .copied()
        .find(|e| e.long_edge() >= min_edge)
        .or_else(|| by_size.last().copied())
}

/// Average `f×f` blocks so the long edge is at most `max_edge`.
pub fn downscale(img: &RgbImage, max_edge: u32) -> RgbImage {
    let long = img.width.max(img.height);
    if long <= max_edge || max_edge == 0 {
        return img.clone();
    }
    let f = long.div_ceil(max_edge);
    let (w, h) = ((img.width / f).max(1), (img.height / f).max(1));
    let mut out = vec![0u8; w as usize * h as usize * 3];
    let stride = img.width as usize * 3;
    for y in 0..h as usize {
        for x in 0..w as usize {
            let mut sum = [0u32; 3];
            for dy in 0..f as usize {
                let row = (y * f as usize + dy) * stride;
                for dx in 0..f as usize {
                    let i = row + (x * f as usize + dx) * 3;
                    for c in 0..3 {
                        sum[c] += u32::from(img.data[i + c]);
                    }
                }
            }
            let n = f * f;
            let o = (y * w as usize + x) * 3;
            for c in 0..3 {
                out[o + c] = ((sum[c] + n / 2) / n) as u8;
            }
        }
    }
    RgbImage {
        width: w,
        height: h,
        data: out,
    }
}

/// A thumbnail of `file` (all of it if `complete`, else a prefix) with long edge at most
/// `max_edge`, from an image with long edge at least `min_edge` when the file has one.
pub fn thumbnail(
    file: &[u8],
    kind: FileKind,
    min_edge: u32,
    max_edge: u32,
    complete: bool,
) -> Result<Preview, ThumbError> {
    match kind {
        FileKind::Heif => Err(DecodeError("HEIF previews are not decoded".into()).into()),
        FileKind::Jpeg => {
            if !complete {
                return Err(ThumbError::NeedWholeFile);
            }
            let d = image::decode_jpeg_threads(file, 1)?;
            let own = d
                .exif
                .as_deref()
                .map(exif::from_tiff_bytes)
                .unwrap_or_default();
            let space = space_of(d.icc, None, &own);
            let small = downscale(&d.image, max_edge);
            Ok(finish(
                small,
                space,
                own.orientation.unwrap_or(1),
                PreviewSource::Jpeg,
            )?)
        }
        FileKind::Raw | FileKind::Dng => {
            let images = embedded::find(file);
            let Some(c) = pick(&images, min_edge) else {
                return Err(if complete {
                    DecodeError("no embedded preview".into()).into()
                } else {
                    ThumbError::NeedWholeFile
                });
            };
            // A bigger image may start past the prefix: only settle for a too-small one when
            // the whole file was searched.
            if !complete && c.long_edge() < min_edge {
                return Err(ThumbError::NeedWholeFile);
            }
            let Some(bytes) = c.bytes(file) else {
                return Err(if complete {
                    DecodeError("preview out of bounds".into()).into()
                } else {
                    ThumbError::NeedWholeFile
                });
            };
            let container = exif::read(file);
            let orientation = container.orientation.unwrap_or(1);
            match c.kind {
                EmbeddedKind::Jpeg => {
                    let d = match image::decode_jpeg_threads(bytes, 1) {
                        Ok(d) => d,
                        // A stream cut by the prefix: try again with the whole file.
                        Err(_) if !complete => return Err(ThumbError::NeedWholeFile),
                        Err(e) => return Err(e.into()),
                    };
                    let own = d.exif.as_deref().map(exif::from_tiff_bytes);
                    let space = space_of(d.icc, own.as_ref(), &container);
                    Ok(finish(
                        downscale(&d.image, max_edge),
                        space,
                        orientation,
                        PreviewSource::Embedded,
                    )?)
                }
                EmbeddedKind::Rgb8 => {
                    let img = RgbImage::new(c.width, c.height, bytes.to_vec())
                        .ok_or_else(|| DecodeError("bad RGB preview size".into()))?;
                    Ok(finish(
                        downscale(&img, max_edge),
                        space_of(None, None, &container),
                        orientation,
                        PreviewSource::Embedded,
                    )?)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blink_testkit::fixtures::{jpeg, quadrants, tiff_with_jpegs};

    fn emb(w: u32, h: u32) -> EmbeddedImage {
        EmbeddedImage {
            kind: EmbeddedKind::Jpeg,
            offset: 0,
            len: 1,
            width: w,
            height: h,
        }
    }

    #[test]
    fn req_view_5_the_smallest_big_enough_embedded_image_is_used() {
        let imgs = [emb(6000, 4000), emb(160, 120), emb(1616, 1080)];
        assert_eq!(pick(&imgs, 150).unwrap().width, 160);
        assert_eq!(pick(&imgs, 300).unwrap().width, 1616);
        assert_eq!(pick(&imgs, 9000).unwrap().width, 6000, "else the largest");
        assert_eq!(pick(&[], 100), None);
    }

    #[test]
    fn req_view_5_downscale_averages_blocks_to_the_long_edge() {
        let img = RgbImage {
            width: 4,
            height: 2,
            data: [[0u8, 0, 0], [255, 255, 255]]
                .iter()
                .cycle()
                .take(8)
                .flatten()
                .copied()
                .collect(),
        };
        let s = downscale(&img, 2);
        assert_eq!((s.width, s.height), (2, 1));
        assert!(s.data.iter().all(|&v| v == 128), "{:?}", s.data);
        assert_eq!(downscale(&img, 10), img);
    }

    #[test]
    fn req_view_5_thumbnail_reads_the_small_preview_from_a_prefix() {
        let q = [[200, 40, 40], [40, 200, 40], [40, 40, 200], [250, 250, 250]];
        let small = jpeg(&quadrants(160, 120, q), 160, 120, 90, None);
        let big = jpeg(&quadrants(1200, 800, q), 1200, 800, 90, None);
        let file = tiff_with_jpegs(&[small.clone(), big]);
        let at = file
            .windows(small.len())
            .position(|w| w == small.as_slice())
            .unwrap();
        let prefix = &file[..at + small.len()];
        let t = thumbnail(prefix, FileKind::Raw, 150, 128, false).unwrap();
        assert_eq!((t.image.width, t.image.height), (80, 60));
        // Top-left quadrant stays red.
        let p = t.image.pixel(5, 5);
        assert!(p[0] > 150 && p[1] < 90, "{p:?}");
        // Asking for more than the small one needs the big one, which is past the prefix.
        assert_eq!(
            thumbnail(prefix, FileKind::Raw, 300, 256, false).unwrap_err(),
            ThumbError::NeedWholeFile
        );
        let t = thumbnail(&file, FileKind::Raw, 300, 256, true).unwrap();
        assert_eq!(t.image.width.max(t.image.height), 240);
        assert!(thumbnail(&file, FileKind::Heif, 100, 100, true).is_err());
        assert_eq!(
            thumbnail(&file[..64], FileKind::Raw, 100, 100, false).unwrap_err(),
            ThumbError::NeedWholeFile
        );
    }
}
