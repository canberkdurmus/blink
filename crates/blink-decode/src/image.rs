//! Decoded images, JPEG decoding (zune-jpeg) and EXIF orientation (VIEW-8).

/// An 8-bit interleaved RGB image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl RgbImage {
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Option<Self> {
        (data.len() == width as usize * height as usize * 3).then_some(Self {
            width,
            height,
            data,
        })
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 3] {
        let i = (y as usize * self.width as usize + x as usize) * 3;
        [self.data[i], self.data[i + 1], self.data[i + 2]]
    }
}

/// Decode errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError(pub String);

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DecodeError {}

/// A decoded JPEG and the metadata needed to display it correctly.
#[derive(Debug, Clone)]
pub struct DecodedJpeg {
    pub image: RgbImage,
    /// The JPEG's own EXIF TIFF payload, if any.
    pub exif: Option<Vec<u8>>,
    /// Embedded ICC profile, if any.
    pub icc: Option<Vec<u8>>,
}

/// JPEGs at least this large use the parallel decoder (PERF-4); smaller ones gain nothing.
const PARALLEL_MIN_BYTES: usize = 128 * 1024;

/// Decode a JPEG to 8-bit RGB. Large baseline JPEGs are decoded on several threads
/// (`pjpeg`, ADR 0011); everything else, and anything `pjpeg` rejects, goes through zune-jpeg.
pub fn decode_jpeg(bytes: &[u8]) -> Result<DecodedJpeg, DecodeError> {
    if bytes.len() >= PARALLEL_MIN_BYTES {
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
        if let Some(d) = decode_jpeg_parallel(bytes, threads) {
            return Ok(d);
        }
    }
    decode_jpeg_zune(bytes)
}

/// Parallel decode on `threads` threads; `None` when the file needs zune-jpeg.
/// EXIF and ICC come from zune-jpeg's header parse.
pub fn decode_jpeg_parallel(bytes: &[u8], threads: usize) -> Option<DecodedJpeg> {
    use zune_jpeg::JpegDecoder;
    use zune_jpeg::zune_core::bytestream::ZCursor;
    use zune_jpeg::zune_core::options::DecoderOptions;
    let opts = DecoderOptions::default()
        .set_max_width(1 << 16)
        .set_max_height(1 << 16)
        .set_strict_mode(false);
    let mut d = JpegDecoder::new_with_options(ZCursor::new(bytes), opts);
    d.decode_headers().ok()?;
    let (w, h, rgb) = crate::pjpeg::decode(bytes, threads)?;
    let image = RgbImage::new(u32::try_from(w).ok()?, u32::try_from(h).ok()?, rgb)?;
    Some(DecodedJpeg {
        image,
        exif: d.exif().cloned(),
        icc: d.icc_profile(),
    })
}

/// Decode a JPEG to 8-bit RGB with zune-jpeg (one thread).
pub fn decode_jpeg_zune(bytes: &[u8]) -> Result<DecodedJpeg, DecodeError> {
    use zune_jpeg::JpegDecoder;
    use zune_jpeg::zune_core::bytestream::ZCursor;
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;
    let opts = DecoderOptions::default()
        .jpeg_set_out_colorspace(ColorSpace::RGB)
        .set_max_width(1 << 16)
        .set_max_height(1 << 16)
        .set_strict_mode(false);
    let mut d = JpegDecoder::new_with_options(ZCursor::new(bytes), opts);
    let pixels = d
        .decode()
        .map_err(|e| DecodeError(format!("jpeg: {e:?}")))?;
    let info = d
        .info()
        .ok_or_else(|| DecodeError("jpeg: no header".into()))?;
    let image = RgbImage::new(u32::from(info.width), u32::from(info.height), pixels)
        .ok_or_else(|| DecodeError("jpeg: unexpected output size".into()))?;
    Ok(DecodedJpeg {
        image,
        exif: d.exif().cloned(),
        icc: d.icc_profile(),
    })
}

/// Apply an EXIF orientation (1–8) for display. Unknown values are treated as 1.
pub fn orient(img: &RgbImage, orientation: u16) -> RgbImage {
    let (w, h) = (img.width as usize, img.height as usize);
    if !(2..=8).contains(&orientation) {
        return img.clone();
    }
    let swap = orientation >= 5;
    let (dw, dh) = if swap { (h, w) } else { (w, h) };
    let mut out = vec![0u8; img.data.len()];
    for sy in 0..h {
        for sx in 0..w {
            let (dx, dy) = match orientation {
                2 => (w - 1 - sx, sy),
                3 => (w - 1 - sx, h - 1 - sy),
                4 => (sx, h - 1 - sy),
                5 => (sy, sx),
                6 => (h - 1 - sy, sx),
                7 => (h - 1 - sy, w - 1 - sx),
                _ => (sy, w - 1 - sx), // 8
            };
            let s = (sy * w + sx) * 3;
            let d = (dy * dw + dx) * 3;
            out[d..d + 3].copy_from_slice(&img.data[s..s + 3]);
        }
    }
    RgbImage {
        width: dw as u32,
        height: dh as u32,
        data: out,
    }
}

/// The orientation that undoes `o` (used to build fixtures: stored = orient(display, inverse(o))).
pub fn inverse_orientation(o: u16) -> u16 {
    match o {
        6 => 8,
        8 => 6,
        o => o,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img2x1() -> RgbImage {
        // [A B] — A at (0,0), B at (1,0)
        RgbImage::new(2, 1, vec![1, 1, 1, 2, 2, 2]).unwrap()
    }

    #[test]
    fn orientation_round_trips() {
        let src = RgbImage::new(3, 2, (0..18).collect()).unwrap();
        for o in 1..=8 {
            let back = orient(&orient(&src, o), inverse_orientation(o));
            assert_eq!(back, src, "orientation {o}");
        }
        let r = orient(&img2x1(), 6); // 90° CW: A on top
        assert_eq!((r.width, r.height), (1, 2));
        assert_eq!(r.pixel(0, 0), [1, 1, 1]);
        let l = orient(&img2x1(), 8); // 90° CCW: B on top
        assert_eq!(l.pixel(0, 0), [2, 2, 2]);
    }
}
