//! Colour conversion to the display space, sRGB (VIEW-10), via moxcms.

use crate::image::RgbImage;
use moxcms::{ColorProfile, Layout, TransformOptions};

/// The colour space an image's pixels are encoded in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceSpace {
    Srgb,
    AdobeRgb,
    /// An embedded ICC profile.
    Icc(Vec<u8>),
}

impl SourceSpace {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Srgb => "sRGB",
            Self::AdobeRgb => "Adobe RGB",
            Self::Icc(_) => "ICC",
        }
    }
}

/// Convert `img` from `space` to sRGB in place. sRGB input is left untouched. An ICC profile that
/// can't be parsed or applied to RGB is treated as sRGB (the image is still shown).
pub fn to_srgb(img: &mut RgbImage, space: &SourceSpace) -> Result<(), String> {
    let src = match space {
        SourceSpace::Srgb => return Ok(()),
        SourceSpace::AdobeRgb => ColorProfile::new_adobe_rgb(),
        SourceSpace::Icc(bytes) => match ColorProfile::new_from_slice(bytes) {
            Ok(p) => p,
            Err(_) => return Ok(()),
        },
    };
    let dst = ColorProfile::new_srgb();
    // A profile moxcms can't apply to RGB data (gray, CMYK, broken): show the pixels as they are
    // rather than nothing.
    let Ok(t) =
        src.create_transform_8bit(Layout::Rgb, &dst, Layout::Rgb, TransformOptions::default())
    else {
        return Ok(());
    };
    let mut out = vec![0u8; img.data.len()];
    t.transform(&img.data, &mut out)
        .map_err(|e| format!("colour transform: {e:?}"))?;
    img.data = out;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unusable_icc_profiles_leave_pixels_untouched() {
        let mut gray = ColorProfile::new_gray_with_gamma(2.2);
        gray.rendering_intent = moxcms::RenderingIntent::Perceptual;
        let gray_icc = gray.encode().expect("encode gray profile");
        for icc in [gray_icc, b"not an icc profile".to_vec()] {
            let mut img = RgbImage::new(1, 1, vec![10, 20, 30]).unwrap();
            to_srgb(&mut img, &SourceSpace::Icc(icc)).unwrap();
            assert_eq!(img.data, [10, 20, 30]);
        }
    }
}
