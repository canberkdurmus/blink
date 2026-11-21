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

/// Long edge of the screen-resolution variant (SPEC §8.3: navigate at screen resolution).
pub const SCREEN_LONG: u32 = 2560;

/// A set of tiles covering one image at one resolution.
#[derive(Debug, Clone, Default)]
pub struct TileSet {
    /// Pixel size of this variant.
    pub size: (u32, u32),
    pub tiles: Vec<Tile>,
}

/// An image ready for upload. Images larger than [`SCREEN_LONG`] get a screen-resolution
/// variant at once, and their full-resolution tiles only when a zoom needs them
/// ([`Prepared::full_or_build`], off the UI thread). Smaller images have only full tiles.
/// So moving to the next item builds and uploads a few MB, never ~100 MB (PERF-2, PERF-3).
#[derive(Debug, Default)]
pub struct Prepared {
    full: std::sync::OnceLock<TileSet>,
    pub screen: Option<TileSet>,
}

impl Prepared {
    /// Full-resolution tiles, if built.
    pub fn full(&self) -> Option<&TileSet> {
        self.full.get()
    }

    /// Full-resolution tiles of `img` (the pixels this was prepared from), building them now
    /// if needed. Blocks for tens of ms on a large image: call it off the UI thread.
    pub fn full_or_build(&self, img: &RgbImage) -> &TileSet {
        self.full.get_or_init(|| full_set(img))
    }
}

fn full_set(img: &RgbImage) -> TileSet {
    TileSet {
        size: (img.width, img.height),
        tiles: tiles_of(img, |[r, g, b]| Color32::from_rgb(r, g, b)),
    }
}

/// Box-filter downscale by an integer factor `k` (all cores).
pub fn downscale(img: &RgbImage, k: u32) -> RgbImage {
    let (w, h) = (img.width.div_ceil(k), img.height.div_ceil(k));
    let mut out = vec![0u8; w as usize * h as usize * 3];
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let rows = (h as usize).div_ceil(threads).max(1);
    std::thread::scope(|s| {
        for (band, chunk) in out.chunks_mut(rows * w as usize * 3).enumerate() {
            s.spawn(move || {
                for (r, row) in chunk.chunks_mut(w as usize * 3).enumerate() {
                    let oy = (band * rows + r) as u32;
                    let y0 = oy * k;
                    let y1 = (y0 + k).min(img.height);
                    for ox in 0..w {
                        let x0 = ox * k;
                        let x1 = (x0 + k).min(img.width);
                        let mut acc = [0u32; 3];
                        for y in y0..y1 {
                            let base = (y as usize * img.width as usize + x0 as usize) * 3;
                            let line = &img.data[base..base + (x1 - x0) as usize * 3];
                            for p in line.as_chunks::<3>().0 {
                                acc[0] += u32::from(p[0]);
                                acc[1] += u32::from(p[1]);
                                acc[2] += u32::from(p[2]);
                            }
                        }
                        let n = (y1 - y0) * (x1 - x0);
                        let o = ox as usize * 3;
                        for c in 0..3 {
                            row[o + c] = ((acc[c] + n / 2) / n) as u8;
                        }
                    }
                }
            });
        }
    });
    RgbImage::new(w, h, out).expect("downscale size")
}

fn tile_grid(w: u32, h: u32) -> impl Iterator<Item = (u32, u32, u32, u32)> {
    (0..h.div_ceil(TILE)).flat_map(move |ty| {
        (0..w.div_ceil(TILE)).map(move |tx| {
            let (x, y) = (tx * TILE, ty * TILE);
            (x, y, TILE.min(w - x), TILE.min(h - y))
        })
    })
}

/// Split an RGB image into RGBA tiles, one thread per tile.
pub fn tiles_of(img: &RgbImage, pixel: impl Fn([u8; 3]) -> Color32 + Sync) -> Vec<Tile> {
    let stride = img.width as usize * 3;
    let grid: Vec<_> = tile_grid(img.width, img.height).collect();
    let pixel = &pixel;
    std::thread::scope(|s| {
        let handles: Vec<_> = grid
            .into_iter()
            .map(|(x, y, w, h)| {
                s.spawn(move || {
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
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("tile thread"))
            .collect()
    })
}

/// The loader's `prepare` hook: displayed pixels as tiles.
pub fn prepare(p: &Preview) -> Prepared {
    let img = &p.image;
    let long = img.width.max(img.height);
    let out = Prepared::default();
    if long > SCREEN_LONG {
        let small = downscale(img, long.div_ceil(SCREEN_LONG));
        Prepared {
            screen: Some(full_set(&small)),
            ..out
        }
    } else {
        let _ = out.full.set(full_set(img));
        out
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
    fn large_images_get_a_screen_resolution_variant() {
        let img = RgbImage::new(6000, 4000, vec![200; 6000 * 4000 * 3]).unwrap();
        let p = prepare(&blink_decode::Preview {
            image: img,
            source: blink_core::preview::PreviewSource::Embedded,
            space: blink_decode::color::SourceSpace::Srgb,
            orientation: 1,
        });
        assert!(p.full().is_none(), "full tiles are built on demand");
        let s = p.screen.as_ref().unwrap();
        assert_eq!(s.size, (2000, 1334));
        assert_eq!(s.tiles[0].image.pixels[0], Color32::from_rgb(200, 200, 200));
        // Box filter averages, rounding to nearest.
        let d = downscale(&RgbImage::new(2, 1, vec![0, 0, 0, 3, 3, 3]).unwrap(), 2);
        assert_eq!(d.data, [2, 2, 2]);
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
