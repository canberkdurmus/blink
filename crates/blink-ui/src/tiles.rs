//! Texture tiles. Large images are split into tiles of at most [`TILE`] pixels per side (SPEC
//! §8.3: ≤ 4096 to stay under GPU limits). The tiles are built on the loader's worker threads,
//! so the UI thread only hands ready `Arc<ColorImage>`s to the renderer.

use blink_decode::{Preview, RgbImage};
use egui::{Color32, ColorImage};
use std::sync::Arc;

/// Tile edge in pixels.
pub const TILE: u32 = 2048;

/// One tile of an image.
#[derive(Debug, Clone)]
pub struct Tile {
    /// Position and size in image pixels.
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub image: Arc<ColorImage>,
}

/// An image ready for upload.
#[derive(Debug, Clone, Default)]
pub struct Prepared {
    pub tiles: Vec<Tile>,
}

fn tile_grid(w: u32, h: u32) -> impl Iterator<Item = (u32, u32, u32, u32)> {
    (0..h.div_ceil(TILE)).flat_map(move |ty| {
        (0..w.div_ceil(TILE)).map(move |tx| {
            let (x, y) = (tx * TILE, ty * TILE);
            (x, y, TILE.min(w - x), TILE.min(h - y))
        })
    })
}

/// Split an RGB image into opaque RGBA tiles.
pub fn tiles_of(img: &RgbImage, pixel: impl Fn([u8; 3]) -> Color32) -> Vec<Tile> {
    let stride = img.width as usize * 3;
    tile_grid(img.width, img.height)
        .map(|(x, y, w, h)| {
            let mut px = Vec::with_capacity(w as usize * h as usize);
            for row in y..y + h {
                let start = row as usize * stride + x as usize * 3;
                let line = &img.data[start..start + w as usize * 3];
                px.extend(line.as_chunks::<3>().0.iter().map(|p| pixel(*p)));
            }
            Tile {
                x,
                y,
                w,
                h,
                image: Arc::new(ColorImage::new([w as usize, h as usize], px)),
            }
        })
        .collect()
}

/// The loader's `prepare` hook: displayed pixels as tiles.
pub fn prepare(p: &Preview) -> Prepared {
    Prepared {
        tiles: tiles_of(&p.image, |[r, g, b]| Color32::from_rgb(r, g, b)),
    }
}

/// Clipping overlay tiles (VIEW-6): red over clipped highlights, blue over clipped shadows,
/// transparent elsewhere.
pub fn overlay(img: &RgbImage) -> Vec<Tile> {
    use blink_decode::stats::{Clip, HIGHLIGHT_RGBA, SHADOW_RGBA, clip_of};
    let c = |[r, g, b, a]: [u8; 4]| Color32::from_rgba_unmultiplied(r, g, b, a);
    let (hi, sh) = (c(HIGHLIGHT_RGBA), c(SHADOW_RGBA));
    tiles_of(img, |p| match clip_of(p) {
        Clip::Highlight => hi,
        Clip::Shadow => sh,
        Clip::None => Color32::TRANSPARENT,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_cover_the_image_and_stay_under_the_gpu_limit() {
        let img = RgbImage::new(4100, 2049, vec![9; 4100 * 2049 * 3]).unwrap();
        let t = tiles_of(&img, |[r, g, b]| Color32::from_rgb(r, g, b));
        assert_eq!(t.len(), 3 * 2);
        let area: u64 = t.iter().map(|t| u64::from(t.w) * u64::from(t.h)).sum();
        assert_eq!(area, 4100 * 2049);
        assert!(t.iter().all(|t| t.w <= 4096 && t.h <= 4096));
        let last = t.last().unwrap();
        assert_eq!((last.x, last.y, last.w, last.h), (4096, 2048, 4, 1));
        assert_eq!(last.image.size, [4, 1]);
    }

    #[test]
    fn req_view_6_overlay_tiles_colour_clipped_pixels() {
        let img = RgbImage::new(3, 1, vec![255, 0, 0, 0, 0, 0, 100, 100, 100]).unwrap();
        let t = overlay(&img);
        assert_eq!(
            t[0].image.pixels,
            [
                Color32::from_rgb(255, 0, 0),
                Color32::from_rgb(0, 0, 255),
                Color32::TRANSPARENT
            ]
        );
    }
}
