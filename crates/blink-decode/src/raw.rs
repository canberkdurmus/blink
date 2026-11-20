//! The RAW decoding backend, behind a trait so rawler (whose API is not SemVer-stable) is
//! isolated to this module (SPEC §8.4).

use crate::image::{DecodeError, RgbImage};

/// What a RAW backend reports about a file without decoding pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawProbe {
    pub make: String,
    pub model: String,
    /// Recommended crop of the sensor image, in pixels (the "sensor size" of SPEC §3).
    pub sensor_w: u32,
    pub sensor_h: u32,
}

/// A RAW decoding backend.
pub trait RawBackend: Send + Sync {
    /// Backend name and version, for `blink-cli doctor`.
    fn name(&self) -> &'static str;
    /// Lower-case file extensions the backend can open.
    fn extensions(&self) -> Vec<String>;
    /// Identify the camera and sensor size without decoding pixel data.
    fn probe(&self, file: &[u8]) -> Result<RawProbe, DecodeError>;
    /// Decode and develop the sensor data to 8-bit sRGB at the sensor's crop size, unrotated
    /// (VIEW-4 "RAW" source).
    fn develop(&self, file: &[u8]) -> Result<RgbImage, DecodeError>;
}

/// Convert gamma-encoded 0..1 floats to 8-bit, on all cores (a 45 MP image has 135 M values).
fn to_u8(src: &[f32]) -> Vec<u8> {
    let mut out = vec![0u8; src.len()];
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let chunk = src.len().div_ceil(threads).max(1 << 16);
    std::thread::scope(|s| {
        for (i, o) in src.chunks(chunk).zip(out.chunks_mut(chunk)) {
            s.spawn(move || {
                for (v, d) in i.iter().zip(o) {
                    *d = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                }
            });
        }
    });
    out
}

/// rawler 0.8.0.
#[derive(Debug, Default, Clone, Copy)]
pub struct Rawler;

fn guard<T>(f: impl FnOnce() -> Result<T, DecodeError>) -> Result<T, DecodeError> {
    // rawler may panic on malformed input; never let that take down a worker.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
        .unwrap_or_else(|_| Err(DecodeError("raw: decoder panicked".into())))
}

impl RawBackend for Rawler {
    fn name(&self) -> &'static str {
        "rawler 0.8.0"
    }

    fn extensions(&self) -> Vec<String> {
        let mut v: Vec<String> = rawler::decoders::supported_extensions()
            .iter()
            .map(|e| e.to_ascii_lowercase())
            .collect();
        v.sort();
        v.dedup();
        v
    }

    fn probe(&self, file: &[u8]) -> Result<RawProbe, DecodeError> {
        guard(|| {
            let src = rawler::rawsource::RawSource::new_from_slice(file);
            let dec = rawler::get_decoder(&src).map_err(|e| DecodeError(format!("raw: {e}")))?;
            let params = rawler::decoders::RawDecodeParams::default();
            let img = dec
                .raw_image(&src, &params, true)
                .map_err(|e| DecodeError(format!("raw: {e}")))?;
            let (w, h) = match img.crop_area.or(img.active_area) {
                Some(r) => (r.d.w, r.d.h),
                None => (img.width, img.height),
            };
            Ok(RawProbe {
                make: img.clean_make.clone(),
                model: img.clean_model.clone(),
                sensor_w: w as u32,
                sensor_h: h as u32,
            })
        })
    }

    fn develop(&self, file: &[u8]) -> Result<RgbImage, DecodeError> {
        use rawler::imgop::develop::{Intermediate, RawDevelop};
        guard(|| {
            let err = |e: rawler::RawlerError| DecodeError(format!("raw: {e}"));
            let src = rawler::rawsource::RawSource::new_from_slice(file);
            let dec = rawler::get_decoder(&src).map_err(err)?;
            let params = rawler::decoders::RawDecodeParams::default();
            let raw = dec.raw_image(&src, &params, false).map_err(err)?;
            let dev = RawDevelop::default()
                .develop_intermediate(&raw)
                .map_err(err)?;
            let (w, h) = (dev.dim().w as u32, dev.dim().h as u32);
            let data = match dev {
                Intermediate::Monochrome(p) => {
                    to_u8(&p.data).iter().flat_map(|&v| [v, v, v]).collect()
                }
                Intermediate::ThreeColor(p) => to_u8(p.data.as_flattened()),
                Intermediate::FourColor(_) => {
                    return Err(DecodeError("raw: 4-colour output not supported".into()));
                }
            };
            RgbImage::new(w, h, data).ok_or_else(|| DecodeError("raw: bad develop size".into()))
        })
    }
}
