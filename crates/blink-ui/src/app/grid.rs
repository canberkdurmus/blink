//! Grid view and filmstrip (VIEW-5): virtualised rows of thumbnails from the thumbnail pool,
//! visible cells requested first (PERF-6).

use super::*;
use blink_core::state::ItemRef;
use blink_core::view::{ThumbSize, ViewMode};
use blink_runtime::thumbs::Tier;

/// Space around each thumbnail, in points.
const PAD: f32 = 6.0;
/// Thumbnail textures uploaded per frame at most, so a frame stays short while scrolling.
const THUMB_UPLOADS_PER_FRAME: usize = 48;
/// Thumbnail textures kept; the ones farthest from the visible cells go first.
const THUMB_TEXTURES: usize = 900;
const SELECTED: Color32 = Color32::from_rgb(90, 150, 230);

/// Per-frame thumbnail state of the app.
#[derive(Default)]
pub(super) struct GridState {
    /// Uploaded thumbnails; `None`: the file has no thumbnail.
    tex: HashMap<(PathBuf, Tier), Option<TextureHandle>>,
    /// The last order sent to the pool (positions and tier), so it is sent only on change.
    asked: Option<(usize, usize, Tier, u64)>,
    /// Grid columns in the last frame (for `↑`/`↓`).
    pub cols: usize,
    /// Visible positions in the last frame.
    pub shown: std::ops::Range<usize>,
    /// The current item when the grid last scrolled to it.
    followed: Option<usize>,
    uploads_left: usize,
}

impl BlinkApp {
    fn tier(&self) -> Tier {
        if self.state.view == ViewMode::Grid && self.state.thumb_size == ThumbSize::Large {
            Tier::Large
        } else {
            Tier::Thumb
        }
    }

    /// Ask the pool for the thumbnails at visible positions `range` first, then outward.
    fn request_thumbs(&mut self, range: std::ops::Range<usize>) {
        use std::hash::{Hash, Hasher};
        let tier = self.tier();
        let n = self.state.visible.len();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.state.visible.hash(&mut h);
        self.state.folder.hash(&mut h);
        let key = (range.start, range.end, tier, h.finish());
        if self.grid.asked == Some(key) {
            return;
        }
        self.grid.asked = Some(key);
        let (start, end) = (range.start.min(n), range.end.min(n));
        let mut order = Vec::with_capacity(n);
        order.extend(start..end);
        let (mut lo, mut hi) = (start, end);
        while lo > 0 || hi < n {
            if hi < n {
                order.push(hi);
                hi += 1;
            }
            if lo > 0 {
                lo -= 1;
                order.push(lo);
            }
        }
        let wanted = order
            .into_iter()
            .map(|p| (ItemRef::of(&self.state.items[self.state.visible[p]]), tier))
            .collect();
        self.thumbs.request(wanted);
        // Drop the textures farthest from what is shown.
        if self.grid.tex.len() > THUMB_TEXTURES {
            let near: std::collections::HashSet<PathBuf> =
                (start.saturating_sub(THUMB_TEXTURES / 2)..(end + THUMB_TEXTURES / 2).min(n))
                    .map(|p| self.state.items[self.state.visible[p]].primary.path.clone())
                    .collect();
            self.grid
                .tex
                .retain(|(p, t), _| *t == tier && near.contains(p));
        }
    }

    /// The texture of item `i`'s thumbnail: `None` while it is coming, `Some(None)` if the
    /// file has none.
    fn thumb_texture(&mut self, i: usize) -> Option<Option<TextureHandle>> {
        let tier = self.tier();
        let item = ItemRef::of(&self.state.items[i]);
        let key = (item.path.clone(), tier);
        if let Some(t) = self.grid.tex.get(&key) {
            return Some(t.clone());
        }
        if self.grid.uploads_left == 0 {
            return None;
        }
        let th = self.thumbs.get(&item, tier)?;
        self.grid.uploads_left -= 1;
        let tex = th.preview.as_ref().map(|p| {
            let img = egui::ColorImage::from_rgb(
                [p.image.width as usize, p.image.height as usize],
                &p.image.data,
            );
            self.ctx.load_texture(
                format!("thumb:{}", item.path.display()),
                img,
                egui::TextureOptions::LINEAR,
            )
        });
        self.grid.tex.insert(key, tex.clone());
        Some(tex)
    }

    /// Forget thumbnails (another folder was opened).
    pub(super) fn clear_thumbs(&mut self) {
        self.thumbs.clear();
        self.grid = GridState::default();
    }

    /// Every visible cell has its thumbnail on screen (PERF-6 bench).
    pub fn grid_visible_drawn(&self) -> bool {
        let tier = self.tier();
        let r = self.grid.shown.clone();
        !r.is_empty()
            && r.clone().all(|p| {
                self.state
                    .visible
                    .get(p)
                    .map(|&i| &self.state.items[i].primary.path)
                    .is_some_and(|path| self.grid.tex.contains_key(&(path.clone(), tier)))
            })
    }

    /// Thumbnails finished of those wanted (PERF-6 bench).
    pub fn thumbs_done(&self) -> (usize, usize) {
        self.thumbs.done_of_wanted()
    }

    /// One cell: thumbnail fitted into `rect`, the current item framed, marks at the bottom.
    fn cell(&mut self, ui: &mut egui::Ui, rect: Rect, pos: usize) -> egui::Response {
        let i = self.state.visible[pos];
        let resp = ui.interact(rect, ui.scope_id().with(("cell", i)), Sense::click());
        let name = self.state.items[i]
            .primary
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let current = self.state.current == Some(i);
        resp.widget_info(|| {
            egui::WidgetInfo::selected(egui::accesskit::Role::Button, true, current, &name)
        });
        let p = ui.painter_at(rect);
        p.rect_filled(
            rect.shrink(1.0),
            3.0,
            Color32::from_gray(if current { 52 } else { 34 }),
        );
        let inner = rect.shrink(PAD);
        match self.thumb_texture(i) {
            Some(Some(t)) => {
                let [w, h] = t.size().map(|v| v as f32);
                let s = (inner.width() / w).min(inner.height() / h);
                let r = Rect::from_center_size(inner.center(), egui::vec2(w * s, h * s));
                p.image(
                    t.id(),
                    r,
                    Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            Some(None) => {
                p.text(
                    inner.center(),
                    egui::Align2::CENTER_CENTER,
                    "NO PREVIEW",
                    egui::FontId::proportional(10.0),
                    Color32::from_gray(140),
                );
            }
            None => {}
        }
        if current {
            p.rect_stroke(
                rect.shrink(1.0),
                3.0,
                egui::Stroke::new(2.0, SELECTED),
                egui::StrokeKind::Inside,
            );
        }
        let path = self.state.items[i].primary.path.clone();
        if let Some(o) = self.marks_of(&path) {
            let y = inner.max.y - 7.0;
            if o.rejected {
                let c = egui::pos2(inner.min.x + 7.0, y);
                let st = egui::Stroke::new(2.0, Color32::from_rgb(230, 80, 70));
                p.line_segment([c - egui::vec2(4.0, 4.0), c + egui::vec2(4.0, 4.0)], st);
                p.line_segment([c + egui::vec2(-4.0, 4.0), c + egui::vec2(4.0, -4.0)], st);
            }
            for k in 0..o.stars {
                let c = egui::pos2(inner.min.x + 7.0 + f32::from(k) * 12.0, y);
                paint_star(&p, c, 6.5, Color32::BLACK);
                paint_star(&p, c, 5.5, Color32::from_rgb(240, 200, 60));
            }
            if let Some((_, Some(c))) = o.label {
                let dot = egui::pos2(inner.max.x - 7.0, inner.max.y - 7.0);
                p.circle_filled(dot, 5.0, label_color(c));
            }
        }
        resp
    }

    /// The grid (VIEW-5): rows of cells, virtualised; follows the current item.
    pub(super) fn grid(&mut self, ui: &mut egui::Ui) {
        self.grid.uploads_left = THUMB_UPLOADS_PER_FRAME;
        let n = self.state.visible.len();
        let size = self.state.thumb_size.cell() + 2.0 * PAD;
        let cols = ((ui.available_width() / size).floor() as usize).max(1);
        self.grid.cols = cols;
        let rows = n.div_ceil(cols);
        let mut area = egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_salt("grid");
        // Keep the current item in view when it moved by keys or from the loupe.
        let cur_pos = self.state.pos();
        if cur_pos != self.grid.followed
            && let Some(p) = cur_pos
        {
            let row = p / cols;
            let (first, last) = (
                self.grid.shown.start / cols.max(1),
                self.grid.shown.end.saturating_sub(1) / cols.max(1),
            );
            if self.grid.shown.is_empty() || row < first || row > last {
                let view_rows = (ui.available_height() / size).floor().max(1.0) as usize;
                let top = if row < first || self.grid.shown.is_empty() {
                    row
                } else {
                    row + 1 - view_rows.min(row + 1)
                };
                area = area.vertical_scroll_offset(top as f32 * size);
            }
            self.grid.followed = cur_pos;
        }
        let mut actions = Vec::new();
        let mut shown = 0..0;
        area.show_rows(ui, size, rows, |ui, row_range| {
            shown = (row_range.start * cols).min(n)..(row_range.end * cols).min(n);
            for row in row_range {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(cols as f32 * size, size), Sense::hover());
                for c in 0..cols {
                    let pos = row * cols + c;
                    if pos >= n {
                        break;
                    }
                    let cell = Rect::from_min_size(
                        rect.min + egui::vec2(c as f32 * size, 0.0),
                        egui::vec2(size, size),
                    );
                    let r = self.cell(ui, cell, pos);
                    let i = self.state.visible[pos];
                    if r.double_clicked() {
                        actions.push(Action::Select(i));
                        actions.push(Action::SetView(ViewMode::Loupe));
                    } else if r.clicked() {
                        actions.push(Action::Select(i));
                    }
                }
            }
        });
        if n == 0 {
            let msg = if self.state.folder.is_none() {
                "Open a folder (Ctrl+O) or drop one here"
            } else {
                "No items"
            };
            ui.centered_and_justified(|ui| ui.weak(msg));
        }
        self.grid.shown = shown.clone();
        self.request_thumbs(shown);
        for a in actions {
            self.dispatch(a);
        }
    }

    /// The loupe's filmstrip (VIEW-5, `T`): small thumbnails around the current item.
    pub(super) fn filmstrip(&mut self, ui: &mut egui::Ui) {
        self.grid.uploads_left = THUMB_UPLOADS_PER_FRAME;
        let n = self.state.visible.len();
        let size = ThumbSize::Small.cell() * 0.75 + 2.0 * PAD;
        let fit = ((ui.available_width() / size).floor() as usize).max(1);
        let cur = self.state.pos().unwrap_or(0);
        let start = cur.saturating_sub(fit / 2).min(n.saturating_sub(fit));
        let range = start..(start + fit).min(n);
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), size), Sense::hover());
        let mut actions = Vec::new();
        for (k, pos) in range.clone().enumerate() {
            let cell = Rect::from_min_size(
                rect.min + egui::vec2(k as f32 * size, 0.0),
                egui::vec2(size, size),
            );
            if self.cell(ui, cell, pos).clicked() {
                actions.push(Action::Select(self.state.visible[pos]));
            }
        }
        self.grid.shown = range.clone();
        self.request_thumbs(range);
        for a in actions {
            self.dispatch(a);
        }
    }
}
