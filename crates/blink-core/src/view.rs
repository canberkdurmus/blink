//! Loupe view state (VIEW-1, VIEW-4, VIEW-9): zoom, pan, what is on screen, and the geometry
//! that places the image in the viewport. Pure: the UI reports the viewport size and cursor, the
//! runtime reports what it decoded; pixels never live here.

use crate::preview::PreviewSource;

/// Zoom levels of the loupe (VIEW-9). Percentages are of the full-resolution image: at 100 %
/// one image pixel covers one physical screen pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Zoom {
    Fit,
    P50,
    P100,
    P200,
}

impl Zoom {
    /// All levels, smallest first.
    pub const LEVELS: [Zoom; 4] = [Zoom::Fit, Zoom::P50, Zoom::P100, Zoom::P200];

    /// Fixed scale (screen px per full-resolution px); `None` for Fit.
    pub fn factor(self) -> Option<f32> {
        match self {
            Self::Fit => None,
            Self::P50 => Some(0.5),
            Self::P100 => Some(1.0),
            Self::P200 => Some(2.0),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Fit => "Fit",
            Self::P50 => "50%",
            Self::P100 => "100%",
            Self::P200 => "200%",
        }
    }
}

/// What the loupe shows for the current item.
#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    /// Primary path of the item these pixels belong to.
    pub path: std::path::PathBuf,
    pub source: PreviewSource,
    /// Size of the displayed pixels (after orientation).
    pub size: (u32, u32),
    /// Size of the full-resolution image (after orientation): the sensor's crop for a RAW, the
    /// file's own size for a JPEG. Zoom percentages refer to this size.
    pub full_size: (u32, u32),
}

impl Shown {
    /// The displayed pixels are smaller than the full-resolution image.
    pub fn is_partial(&self) -> bool {
        self.size.0.max(self.size.1) < self.full_size.0.max(self.full_size.1)
    }
}

/// A rectangle in viewport pixels (origin: top-left of the loupe area).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Loupe state.
#[derive(Debug, Clone, PartialEq)]
pub struct Loupe {
    pub zoom: Zoom,
    /// Viewport centre in normalised image coordinates (0..1 on each axis).
    pub center: [f32; 2],
    /// Keep zoom and position when moving to another item (VIEW-9). On by default.
    pub keep_zoom: bool,
    /// Clipping overlay (VIEW-6), toggled with `J`.
    pub clipping: bool,
    /// Right panel with info and histogram, toggled with `I`.
    pub info_panel: bool,
    /// Loupe area in physical pixels.
    pub viewport: [f32; 2],
    /// What is on screen; `None` while the current item's first pixels are loading.
    pub shown: Option<Shown>,
    /// A full-resolution image (paired JPEG or RAW decode) is being loaded: show a spinner.
    pub pending_full: bool,
}

impl Default for Loupe {
    fn default() -> Self {
        Self {
            zoom: Zoom::Fit,
            center: [0.5, 0.5],
            keep_zoom: true,
            clipping: false,
            info_panel: false,
            viewport: [1280.0, 800.0],
            shown: None,
            pending_full: false,
        }
    }
}

impl Loupe {
    /// Screen pixels per full-resolution image pixel.
    pub fn scale(&self) -> Option<f32> {
        let s = self.shown.as_ref()?;
        let (fw, fh) = (s.full_size.0.max(1) as f32, s.full_size.1.max(1) as f32);
        Some(match self.zoom.factor() {
            Some(f) => f,
            None => (self.viewport[0] / fw).min(self.viewport[1] / fh),
        })
    }

    /// Where the image is drawn, in viewport pixels. The displayed pixels are stretched over the
    /// full-resolution size, so a low-res preview is upscaled until the full image arrives.
    pub fn image_rect(&self) -> Option<Rect> {
        let s = self.shown.as_ref()?;
        let k = self.scale()?;
        let w = s.full_size.0 as f32 * k;
        let h = s.full_size.1 as f32 * k;
        Some(Rect {
            x: self.viewport[0] / 2.0 - self.center[0] * w,
            y: self.viewport[1] / 2.0 - self.center[1] * h,
            w,
            h,
        })
    }

    /// Keep the image covering the viewport where it is larger, centred where it is smaller.
    pub fn clamp_center(&mut self) {
        // Nothing shown (the next item is loading): keep the position for it (VIEW-9).
        let Some(r) = self.image_rect() else {
            return;
        };
        for (axis, size) in [r.w, r.h].into_iter().enumerate() {
            let half = self.viewport[axis] / 2.0 / size;
            self.center[axis] = if half >= 0.5 {
                0.5
            } else {
                self.center[axis].clamp(half, 1.0 - half)
            };
        }
    }

    /// Change the zoom. With a cursor (viewport pixels), the image point under the cursor stays
    /// under the cursor (zoom at cursor position, VIEW-4).
    pub fn set_zoom(&mut self, zoom: Zoom, cursor: Option<[f32; 2]>) {
        let anchor = cursor.and_then(|c| {
            let r = self.image_rect()?;
            Some((
                c,
                [
                    ((c[0] - r.x) / r.w).clamp(0.0, 1.0),
                    ((c[1] - r.y) / r.h).clamp(0.0, 1.0),
                ],
            ))
        });
        self.zoom = zoom;
        if zoom == Zoom::Fit {
            self.center = [0.5, 0.5];
        } else if let (Some((c, p)), Some(r)) = (anchor, self.image_rect()) {
            self.center = [
                p[0] + (self.viewport[0] / 2.0 - c[0]) / r.w,
                p[1] + (self.viewport[1] / 2.0 - c[1]) / r.h,
            ];
        }
        self.clamp_center();
    }

    /// Pan by a drag of `delta` viewport pixels (the image follows the pointer).
    pub fn pan(&mut self, delta: [f32; 2]) {
        if let Some(r) = self.image_rect() {
            self.center[0] -= delta[0] / r.w;
            self.center[1] -= delta[1] / r.h;
            self.clamp_center();
        }
    }

    /// At the current zoom, the displayed pixels would be upscaled: a full-resolution image is
    /// needed (VIEW-4: LOW-RES previews, and previews smaller than the sensor).
    pub fn needs_full(&self) -> bool {
        let (Some(s), Some(k)) = (self.shown.as_ref(), self.zoom.factor()) else {
            return false;
        };
        let on_screen = s.full_size.0.max(s.full_size.1) as f32 * k;
        s.is_partial() && (s.size.0.max(s.size.1) as f32) < on_screen
    }
}

/// Travel direction, for the prefetch window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Forward,
    Backward,
}

/// How far [`Effect::Show`](crate::state::Effect::Show) lists items ahead for decode-ahead
/// pacing (PERF-3): a held key on a slow machine passes more items during one decode than the
/// prefetch window holds. These items are not prefetched; the loader decodes only the one the
/// user reaches when a decode started now ends (ADR 0014).
pub const DECODE_AHEAD: usize = 32;

/// Prefetch window (SPEC §8.3): 8 ahead and 2 behind in the travel direction, nearest first,
/// with the first item behind right after the first item ahead (cheap direction changes).
pub fn prefetch_window(current: usize, len: usize, dir: Direction) -> Vec<usize> {
    let step = |n: usize, ahead: bool| -> Option<usize> {
        let forward = (dir == Direction::Forward) == ahead;
        if forward {
            current.checked_add(n).filter(|&i| i < len)
        } else {
            current.checked_sub(n)
        }
    };
    let order = [
        (1, true),
        (1, false),
        (2, true),
        (3, true),
        (4, true),
        (5, true),
        (6, true),
        (7, true),
        (8, true),
        (2, false),
    ];
    order
        .iter()
        .filter_map(|&(n, ahead)| step(n, ahead))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loupe(size: (u32, u32), full: (u32, u32)) -> Loupe {
        Loupe {
            viewport: [1000.0, 500.0],
            shown: Some(Shown {
                path: "/f/a.ARW".into(),
                source: PreviewSource::Embedded,
                size,
                full_size: full,
            }),
            ..Loupe::default()
        }
    }

    #[test]
    fn req_view_1_fit_scales_the_image_into_the_viewport_and_centres_it() {
        // 4000×2000 into 1000×500 → scale 0.25, fills the viewport exactly.
        let l = loupe((4000, 2000), (4000, 2000));
        assert_eq!(l.scale(), Some(0.25));
        assert_eq!(
            l.image_rect(),
            Some(Rect {
                x: 0.0,
                y: 0.0,
                w: 1000.0,
                h: 500.0
            })
        );
        // Portrait 2000×4000 → height-limited, centred horizontally.
        let l = loupe((2000, 4000), (2000, 4000));
        let r = l.image_rect().unwrap();
        assert_eq!((r.w, r.h), (250.0, 500.0));
        assert_eq!((r.x, r.y), (375.0, 0.0));
        // A low-res preview is drawn at the full image's fitted size (upscaled).
        let l = loupe((1616, 1080), (4912, 3264));
        let r = l.image_rect().unwrap();
        assert!((r.h - 500.0).abs() < 0.01 && r.w < 1000.0);
        // Nothing shown yet → nothing drawn.
        assert_eq!(Loupe::default().image_rect(), None);
    }

    #[test]
    fn req_view_9_zoom_levels_are_fit_50_100_200_of_the_full_image() {
        let mut l = loupe((1616, 1080), (4912, 3264));
        for (z, k) in [(Zoom::P50, 0.5), (Zoom::P100, 1.0), (Zoom::P200, 2.0)] {
            l.set_zoom(z, None);
            assert_eq!(l.scale(), Some(k));
            let r = l.image_rect().unwrap();
            assert_eq!((r.w, r.h), (4912.0 * k, 3264.0 * k));
        }
        l.set_zoom(Zoom::Fit, None);
        assert_eq!(l.center, [0.5, 0.5]);
        assert_eq!(
            Zoom::LEVELS.map(Zoom::label),
            ["Fit", "50%", "100%", "200%"]
        );
    }

    #[test]
    fn req_view_9_pan_with_drag_moves_the_image_and_stays_inside() {
        let mut l = loupe((4000, 2000), (4000, 2000));
        l.set_zoom(Zoom::P100, None);
        let r0 = l.image_rect().unwrap();
        l.pan([100.0, -50.0]);
        let r1 = l.image_rect().unwrap();
        assert!((r1.x - (r0.x + 100.0)).abs() < 0.01);
        assert!((r1.y - (r0.y - 50.0)).abs() < 0.01);
        // Dragging far right stops at the image's left edge.
        l.pan([1e6, 1e6]);
        let r = l.image_rect().unwrap();
        assert!(r.x.abs() < 0.01 && r.y.abs() < 0.01, "{r:?}");
        l.pan([-1e6, -1e6]);
        let r = l.image_rect().unwrap();
        assert!((r.x + r.w - 1000.0).abs() < 0.01 && (r.y + r.h - 500.0).abs() < 0.01);
        // At Fit there is nothing to pan.
        l.set_zoom(Zoom::Fit, None);
        l.pan([100.0, 100.0]);
        assert_eq!(l.center, [0.5, 0.5]);
    }

    #[test]
    fn req_view_4_zoom_at_cursor_keeps_the_point_under_the_cursor() {
        let mut l = loupe((4000, 2000), (4000, 2000));
        // Cursor at viewport (750, 125) = image point (0.75, 0.25) at Fit.
        l.set_zoom(Zoom::P100, Some([750.0, 125.0]));
        let r = l.image_rect().unwrap();
        let p = [(750.0 - r.x) / r.w, (125.0 - r.y) / r.h];
        assert!(
            (p[0] - 0.75).abs() < 1e-4 && (p[1] - 0.25).abs() < 1e-4,
            "{p:?}"
        );
        // Near a corner the point moves only as far as clamping requires.
        l.set_zoom(Zoom::Fit, None);
        l.set_zoom(Zoom::P100, Some([0.0, 0.0]));
        let r = l.image_rect().unwrap();
        assert_eq!((r.x, r.y), (0.0, 0.0));
    }

    #[test]
    fn req_view_4_full_resolution_is_needed_only_when_the_preview_would_be_upscaled() {
        // LOW-RES 1616 px preview of a 4912 px sensor: needed at 50 % and above.
        let mut l = loupe((1616, 1080), (4912, 3264));
        assert!(!l.needs_full());
        l.set_zoom(Zoom::P50, None);
        assert!(l.needs_full());
        // A 6000 px preview of a 6048 px sensor is "smaller than the sensor": needed at 100 %.
        let mut l = loupe((6000, 4000), (6048, 4024));
        l.set_zoom(Zoom::P50, None);
        assert!(!l.needs_full());
        l.set_zoom(Zoom::P100, None);
        assert!(l.needs_full());
        // Full-size pixels never need more.
        let mut l = loupe((6048, 4024), (6048, 4024));
        l.set_zoom(Zoom::P200, None);
        assert!(!l.needs_full());
    }

    #[test]
    fn prefetch_window_is_8_ahead_2_behind_in_the_travel_direction() {
        use Direction::*;
        assert_eq!(
            prefetch_window(10, 100, Forward),
            [11, 9, 12, 13, 14, 15, 16, 17, 18, 8]
        );
        assert_eq!(
            prefetch_window(10, 100, Backward),
            [9, 11, 8, 7, 6, 5, 4, 3, 2, 12]
        );
        assert_eq!(prefetch_window(0, 3, Forward), [1, 2]);
        assert_eq!(prefetch_window(1, 3, Backward), [0, 2]);
        assert!(prefetch_window(0, 1, Forward).is_empty());
    }
}
