//! Histogram and clipping overlay (VIEW-6). Both are computed from the **displayed** image (the
//! oriented sRGB pixels the loupe shows) and carry the preview source they were computed from.

use crate::image::RgbImage;
use blink_core::preview::PreviewSource;

/// A highlight is clipped when any channel is at or above this value.
pub const HIGHLIGHT_MIN: u8 = 254;
/// A shadow is clipped when every channel is at or below this value.
pub const SHADOW_MAX: u8 = 1;

/// Overlay colour of clipped highlights (opaque red, RGBA).
pub const HIGHLIGHT_RGBA: [u8; 4] = [255, 0, 0, 255];
/// Overlay colour of clipped shadows (opaque blue, RGBA).
pub const SHADOW_RGBA: [u8; 4] = [0, 0, 255, 255];

/// Clipping class of one pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clip {
    None,
    /// Any channel ≥ 254.
    Highlight,
    /// All channels ≤ 1.
    Shadow,
}

/// Classify one pixel. The two classes can't overlap (≥ 254 somewhere vs ≤ 1 everywhere).
pub fn clip_of([r, g, b]: [u8; 3]) -> Clip {
    if r >= HIGHLIGHT_MIN || g >= HIGHLIGHT_MIN || b >= HIGHLIGHT_MIN {
        Clip::Highlight
    } else if r <= SHADOW_MAX && g <= SHADOW_MAX && b <= SHADOW_MAX {
        Clip::Shadow
    } else {
        Clip::None
    }
}

/// Per-pixel clipping mask, row-major.
pub fn clipping_mask(img: &RgbImage) -> Vec<Clip> {
    img.data
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| clip_of(*p))
        .collect()
}

/// The clipping overlay as RGBA, row-major: red over clipped highlights, blue over clipped
/// shadows, fully transparent elsewhere. Drawn on top of the displayed image.
pub fn clipping_overlay_rgba(img: &RgbImage) -> Vec<u8> {
    let mut out = vec![0u8; img.data.len() / 3 * 4];
    for (p, o) in img
        .data
        .as_chunks::<3>()
        .0
        .iter()
        .zip(out.as_chunks_mut::<4>().0)
    {
        match clip_of(*p) {
            Clip::Highlight => *o = HIGHLIGHT_RGBA,
            Clip::Shadow => *o = SHADOW_RGBA,
            Clip::None => {}
        }
    }
    out
}

/// Rec. 709 luma of gamma-encoded sRGB values, in integer arithmetic:
/// `(54 R + 183 G + 19 B + 128) / 256` (the weights 0.2126, 0.7152, 0.0722 scaled to sum to 256).
pub fn luma([r, g, b]: [u8; 3]) -> u8 {
    ((54 * u32::from(r) + 183 * u32::from(g) + 19 * u32::from(b) + 128) >> 8) as u8
}

/// Luminance and per-channel histograms of the displayed image, 256 bins each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Histogram {
    /// The source of the image the histogram was computed from (shown as its label).
    pub source: PreviewSource,
    pub red: [u32; 256],
    pub green: [u32; 256],
    pub blue: [u32; 256],
    pub luma: [u32; 256],
    /// Pixels in the clipping classes (same rule as the overlay).
    pub highlights_clipped: u32,
    pub shadows_clipped: u32,
}

impl Histogram {
    /// The label shown with the histogram, e.g. `Histogram · EMBEDDED · LOW-RES`.
    pub fn label(&self) -> String {
        format!("Histogram · {}", self.source.badge())
    }
}

/// Compute the histograms of `img`, which was displayed from `source`.
pub fn histogram(img: &RgbImage, source: PreviewSource) -> Histogram {
    let mut h = Histogram {
        source,
        red: [0; 256],
        green: [0; 256],
        blue: [0; 256],
        luma: [0; 256],
        highlights_clipped: 0,
        shadows_clipped: 0,
    };
    for &px in img.data.as_chunks::<3>().0 {
        h.red[usize::from(px[0])] += 1;
        h.green[usize::from(px[1])] += 1;
        h.blue[usize::from(px[2])] += 1;
        h.luma[usize::from(luma(px))] += 1;
        match clip_of(px) {
            Clip::Highlight => h.highlights_clipped += 1,
            Clip::Shadow => h.shadows_clipped += 1,
            Clip::None => {}
        }
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 4×2 image with known clipping:
    /// row 0: highlight (R=254), highlight (B=255), shadow (1,1,1), none (2,1,1)
    /// row 1: shadow (0,0,0), none (253,253,253), highlight (255,255,255), none (1,1,2)
    fn synthetic() -> RgbImage {
        #[rustfmt::skip]
        let px: [[u8; 3]; 8] = [
            [254, 10, 10], [0, 0, 255], [1, 1, 1], [2, 1, 1],
            [0, 0, 0], [253, 253, 253], [255, 255, 255], [1, 1, 2],
        ];
        RgbImage::new(4, 2, px.concat()).unwrap()
    }

    #[test]
    fn req_view_6_clipping_mask_is_exact_on_a_synthetic_image() {
        use Clip::*;
        let img = synthetic();
        assert_eq!(
            clipping_mask(&img),
            [
                Highlight, Highlight, Shadow, None, Shadow, None, Highlight, None
            ]
        );
        let o = clipping_overlay_rgba(&img);
        assert_eq!(o.len(), 8 * 4);
        let px = |i: usize| [o[i * 4], o[i * 4 + 1], o[i * 4 + 2], o[i * 4 + 3]];
        assert_eq!(px(0), HIGHLIGHT_RGBA);
        assert_eq!(px(1), HIGHLIGHT_RGBA);
        assert_eq!(px(2), SHADOW_RGBA);
        assert_eq!(px(3), [0, 0, 0, 0]);
        assert_eq!(px(4), SHADOW_RGBA);
        assert_eq!(px(5), [0, 0, 0, 0]);
        assert_eq!(px(6), HIGHLIGHT_RGBA);
        assert_eq!(px(7), [0, 0, 0, 0]);
    }

    #[test]
    fn req_view_6_thresholds_are_254_any_channel_and_1_all_channels() {
        for v in 0..=255u8 {
            let want = if v >= 254 {
                Clip::Highlight
            } else {
                Clip::None
            };
            assert_eq!(clip_of([v, 100, 100]), want, "R={v}");
            assert_eq!(clip_of([100, v, 100]), want, "G={v}");
            assert_eq!(clip_of([100, 100, v]), want, "B={v}");
            let all = if v <= 1 {
                Clip::Shadow
            } else if v >= 254 {
                Clip::Highlight
            } else {
                Clip::None
            };
            assert_eq!(clip_of([v, v, v]), all, "grey {v}");
        }
        // One channel above 1 is not a clipped shadow.
        assert_eq!(clip_of([0, 0, 2]), Clip::None);
    }

    #[test]
    fn req_view_6_histograms_are_exact_and_labelled_with_the_source() {
        let img = synthetic();
        let h = histogram(&img, PreviewSource::EmbeddedLowRes);
        let bins = |a: &[u32; 256]| {
            a.iter()
                .enumerate()
                .filter(|(_, n)| **n > 0)
                .map(|(i, n)| (i, *n))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            bins(&h.red),
            [(0, 2), (1, 2), (2, 1), (253, 1), (254, 1), (255, 1)]
        );
        assert_eq!(
            bins(&h.green),
            [(0, 2), (1, 3), (10, 1), (253, 1), (255, 1)]
        );
        assert_eq!(
            bins(&h.blue),
            [(0, 1), (1, 2), (2, 1), (10, 1), (253, 1), (255, 2)]
        );
        // Luma (54R + 183G + 19B + 128) >> 8 of each pixel.
        assert_eq!(luma([254, 10, 10]), 61);
        assert_eq!(luma([0, 0, 255]), 19);
        assert_eq!(luma([255, 255, 255]), 255);
        assert_eq!(luma([253, 253, 253]), 253);
        assert_eq!(
            bins(&h.luma),
            [(0, 1), (1, 3), (19, 1), (61, 1), (253, 1), (255, 1)]
        );
        assert_eq!(h.red.iter().sum::<u32>(), 8);
        assert_eq!((h.highlights_clipped, h.shadows_clipped), (3, 2));
        assert_eq!(h.source, PreviewSource::EmbeddedLowRes);
        assert_eq!(h.label(), "Histogram · EMBEDDED · LOW-RES");
        assert_eq!(
            histogram(&img, PreviewSource::RawDecode).label(),
            "Histogram · RAW"
        );
    }
}
