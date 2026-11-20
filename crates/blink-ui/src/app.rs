//! The Blink window: dispatches actions through the reducer, executes effects through the
//! runtime and draws the loupe (VIEW-1), its badge (VIEW-3), zoom/pan (VIEW-4, VIEW-9), the
//! clipping overlay and histogram (VIEW-6) and the info panel (VIEW-7).

use crate::tiles::{self, Prepared, Tile};
use blink_core::formats::Formats;
use blink_core::preview::PreviewSource;
use blink_core::state::{Action, Effect, Level, Nav, State, reduce};
use blink_core::view::Zoom;
use blink_decode::raw::RawBackend;
use blink_runtime::fs::Fs;
use blink_runtime::loader::{DEFAULT_CACHE_BYTES, Loaded, Loader, default_workers};
use egui::{Color32, Key, Pos2, Rect, Sense, TextureHandle, TextureOptions, Vec2};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, OnceLock};

/// Loupe background: neutral dark grey, no tint (SPEC §5.1).
pub const LOUPE_BG: Color32 = Color32::from_gray(24);
/// Full-resolution tiles uploaded per frame, so a 45 MP swap never stalls a frame (PERF-5).
const FULL_TILES_PER_FRAME: usize = 2;
/// L1: GPU images kept (count-limited, SPEC §8.3).
const L1_IMAGES: usize = 6;

type Key2 = (PathBuf, Level);

/// What the last frame drew in the loupe (for tests and the bench harness).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Displayed {
    pub path: PathBuf,
    pub source: PreviewSource,
    pub size: (u32, u32),
}

struct Uploaded {
    tiles: Vec<(Tile, TextureHandle)>,
    total: usize,
}

/// Construction options.
pub struct Options {
    pub fs: Arc<dyn Fs>,
    pub backend: Arc<dyn RawBackend>,
    pub workers: usize,
    pub cache_bytes: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            fs: Arc::new(blink_runtime::fs::RealFs),
            backend: Arc::new(blink_decode::default_backend()),
            workers: default_workers(),
            cache_bytes: DEFAULT_CACHE_BYTES,
        }
    }
}

/// The top-level egui application.
pub struct BlinkApp {
    state: State,
    fs: Arc<dyn Fs>,
    loader: Loader<Prepared>,
    ctx: egui::Context,
    /// The context the loader wakes; follows the context the app is drawn in.
    wake_ctx: Arc<std::sync::Mutex<egui::Context>>,
    tx: Sender<Action>,
    rx: Receiver<Action>,
    textures: HashMap<Key2, Uploaded>,
    /// L1 recency, most recent last.
    l1_order: VecDeque<Key2>,
    overlays: HashMap<Key2, Arc<OnceLock<Vec<Tile>>>>,
    overlay_textures: HashMap<Key2, Vec<(Tile, TextureHandle)>>,
    displayed: Option<Displayed>,
    toast_at: f64,
    drag_moved: bool,
    /// Loupe area of the last frame (points).
    loupe_rect: Option<Rect>,
}

impl BlinkApp {
    /// Create the app. `folder` is the `blink [folder]` argument (OPEN-1).
    pub fn new(ctx: &egui::Context, folder: Option<PathBuf>, opts: Options) -> Self {
        apply_theme(ctx);
        let wake_ctx = Arc::new(std::sync::Mutex::new(ctx.clone()));
        let wake = wake_ctx.clone();
        let loader = Loader::new(
            opts.fs.clone(),
            opts.backend.clone(),
            opts.workers,
            opts.cache_bytes,
            tiles::prepare,
            move || {
                if let Ok(c) = wake.lock() {
                    c.request_repaint();
                }
            },
        );
        let (tx, rx) = channel();
        let mut app = Self {
            state: State::new(Formats::new(opts.backend.extensions())),
            fs: opts.fs,
            loader,
            ctx: ctx.clone(),
            wake_ctx,
            tx,
            rx,
            textures: HashMap::new(),
            l1_order: VecDeque::new(),
            overlays: HashMap::new(),
            overlay_textures: HashMap::new(),
            displayed: None,
            toast_at: 0.0,
            drag_moved: false,
            loupe_rect: None,
        };
        if let Some(f) = folder {
            app.open_path(&f);
        }
        app
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    /// What the last frame drew in the loupe.
    pub fn displayed(&self) -> Option<&Displayed> {
        self.displayed.as_ref()
    }

    /// The loaded image the loupe would draw for the current item, if any.
    pub fn loaded(&self) -> Option<Arc<Loaded<Prepared>>> {
        let shown = self.state.loupe.shown.as_ref()?;
        self.loader
            .get(&shown.path, Level::Full)
            .filter(|l| l.source == shown.source)
            .or_else(|| self.loader.get(&shown.path, Level::Preview))
    }

    /// No decode job is queued or running.
    pub fn loader_idle(&self) -> bool {
        self.loader.idle()
    }

    /// Open a folder, or the folder of a file (command line, drop, dialog — OPEN-1).
    pub fn open_path(&mut self, p: &Path) {
        let is_dir = self.fs.metadata(p).map(|m| m.is_dir).unwrap_or(false);
        let folder = if is_dir {
            p.to_path_buf()
        } else {
            p.parent().map(Path::to_path_buf).unwrap_or_default()
        };
        self.dispatch(Action::OpenFolder(folder));
    }

    /// Run an action through the reducer and execute its effects.
    pub fn dispatch(&mut self, action: Action) {
        let mut queue = VecDeque::from([action]);
        while let Some(a) = queue.pop_front() {
            let scanned = matches!(a, Action::FolderScanned { .. });
            let (s, fx) = reduce(&self.state, a);
            let toast_changed = s.toast != self.state.toast;
            self.state = s;
            if toast_changed {
                self.toast_at = self.ctx.input(|i| i.time);
            }
            if scanned {
                self.load_marks();
            }
            for e in fx {
                match &e {
                    Effect::ScanFolder { folder, recursive } => {
                        self.scan(folder.clone(), *recursive)
                    }
                    // Sidecar writing arrives with FILE-4..6 (M3).
                    Effect::WriteSidecar { .. } => {}
                    Effect::Show { .. } | Effect::LoadFull { .. } => {
                        queue.extend(self.loader.execute(&e));
                    }
                }
            }
        }
    }

    fn scan(&self, folder: PathBuf, recursive: bool) {
        let (fs, tx, ctx) = (self.fs.clone(), self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let files = blink_runtime::scan::scan_folder(fs.as_ref(), &folder, recursive)
                .unwrap_or_default();
            let _ = tx.send(Action::FolderScanned {
                folder,
                recursive,
                files,
            });
            ctx.request_repaint();
        });
    }

    fn load_marks(&self) {
        let (fs, tx, ctx) = (self.fs.clone(), self.tx.clone(), self.ctx.clone());
        let items = self.state.items.clone();
        std::thread::spawn(move || {
            let m = blink_runtime::marks::load_marks(fs.as_ref(), &items);
            let _ = tx.send(Action::MarksLoaded(m));
            ctx.request_repaint();
        });
    }

    /// Feed results from background threads.
    pub fn pump(&mut self) {
        let mut incoming: Vec<Action> = self.rx.try_iter().collect();
        incoming.extend(self.loader.poll());
        for a in incoming {
            self.dispatch(a);
        }
    }

    /// Draw one frame into the root UI.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        if ui.ctx() != &self.ctx {
            self.ctx = ui.ctx().clone();
            apply_theme(&self.ctx);
            if let Ok(mut c) = self.wake_ctx.lock() {
                *c = self.ctx.clone();
            }
            self.textures.clear();
            self.overlay_textures.clear();
            self.l1_order.clear();
        }
        self.pump();
        self.handle_keys(ui.ctx());
        self.handle_drop(ui.ctx());
        self.top_bar(ui);
        if self.state.loupe.info_panel {
            egui::Panel::right("info")
                .resizable(false)
                .exact_size(300.0)
                .show(ui, |ui| self.info_panel(ui));
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(LOUPE_BG))
            .show(ui, |ui| self.loupe(ui));
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|i| i.events.clone());
        let mut actions = Vec::new();
        let mut open = false;
        let pointer = self.pointer_in_loupe(ctx);
        for ev in events {
            let egui::Event::Key {
                key,
                pressed: true,
                modifiers: m,
                ..
            } = ev
            else {
                continue;
            };
            let a = match key {
                Key::ArrowRight => Some(Action::Navigate(Nav::Next)),
                Key::ArrowLeft => Some(Action::Navigate(Nav::Prev)),
                Key::Space if m.shift => Some(Action::Navigate(Nav::Prev)),
                Key::Space => Some(Action::Navigate(Nav::Next)),
                Key::Home => Some(Action::Navigate(Nav::First)),
                Key::End => Some(Action::Navigate(Nav::Last)),
                Key::Z if !m.command => Some(Action::ToggleZoom100 { cursor: pointer }),
                Key::F if !m.command => Some(Action::SetZoom {
                    zoom: Zoom::Fit,
                    cursor: None,
                }),
                Key::J => Some(Action::ToggleClipping),
                Key::I => Some(Action::ToggleInfoPanel),
                Key::O if m.command => {
                    open = true;
                    None
                }
                Key::F11 => {
                    let full = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!full));
                    None
                }
                _ => None,
            };
            actions.extend(a);
        }
        for a in actions {
            self.dispatch(a);
        }
        if open {
            self.open_dialog();
        }
    }

    fn open_dialog(&mut self) {
        if let Some(folder) = rfd::FileDialog::new().pick_folder() {
            self.dispatch(Action::OpenFolder(folder));
        }
    }

    fn handle_drop(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if let Some(p) = dropped.into_iter().next().map(|f| f.path().to_path_buf()) {
            self.open_path(&p);
        }
    }

    /// The pointer position in loupe viewport pixels, when it is over the loupe.
    fn pointer_in_loupe(&self, ctx: &egui::Context) -> Option<[f32; 2]> {
        let rect = self.loupe_rect?;
        let p = ctx.input(|i| i.pointer.hover_pos())?;
        let ppp = ctx.pixels_per_point();
        rect.contains(p)
            .then_some([(p.x - rect.min.x) * ppp, (p.y - rect.min.y) * ppp])
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let mut actions = Vec::new();
        let mut open = false;
        egui::Panel::top("top").show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Open…").clicked() {
                    open = true;
                }
                let folder = self
                    .state
                    .folder
                    .as_ref()
                    .map(|f| f.display().to_string())
                    .unwrap_or_else(|| "No folder open".into());
                ui.label(folder);
                ui.separator();
                ui.label(counter(self.state.current, self.state.items.len()));
                ui.separator();
                for z in Zoom::LEVELS {
                    if ui
                        .selectable_label(self.state.loupe.zoom == z, z.label())
                        .clicked()
                    {
                        actions.push(Action::SetZoom {
                            zoom: z,
                            cursor: None,
                        });
                    }
                }
                let mut keep = self.state.loupe.keep_zoom;
                if ui
                    .checkbox(&mut keep, "Keep zoom")
                    .on_hover_text("Keep zoom and position when moving to the next item")
                    .changed()
                {
                    actions.push(Action::SetKeepZoom(keep));
                }
                let mut clip = self.state.loupe.clipping;
                if ui.checkbox(&mut clip, "Clipping (J)").changed() {
                    actions.push(Action::ToggleClipping);
                }
            });
        });
        for a in actions {
            self.dispatch(a);
        }
        if open {
            self.open_dialog();
        }
    }

    fn info_panel(&mut self, ui: &mut egui::Ui) {
        let Some(l) = self.loaded() else {
            ui.label("—");
            return;
        };
        egui::Grid::new("info-grid").num_columns(2).show(ui, |ui| {
            for row in l.info.rows() {
                ui.label(row.label);
                ui.label(row.value);
                ui.end_row();
            }
        });
        ui.separator();
        histogram_widget(ui, &l.histogram);
    }

    fn loupe(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let ctx = ui.ctx().clone();
        self.loupe_rect = Some(rect);
        let ppp = ctx.pixels_per_point();
        let viewport = [rect.width() * ppp, rect.height() * ppp];
        if viewport != self.state.loupe.viewport {
            self.dispatch(Action::Resize(viewport));
        }
        let resp = ui.interact(rect, ui.scope_id().with("loupe"), Sense::click_and_drag());
        if resp.drag_started() {
            self.drag_moved = false;
        }
        if resp.dragged() {
            let d = resp.drag_delta() * ppp;
            if d != Vec2::ZERO {
                self.drag_moved = true;
                self.dispatch(Action::Pan([d.x, d.y]));
            }
        }
        if resp.clicked()
            && !self.drag_moved
            && let Some(p) = resp.interact_pointer_pos()
        {
            let c = [(p.x - rect.min.x) * ppp, (p.y - rect.min.y) * ppp];
            self.dispatch(Action::ToggleZoom100 { cursor: Some(c) });
        }

        let painter = ui.painter_at(rect);
        self.displayed = None;
        if self.state.folder.is_none() {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Drop a folder here, or press Ctrl+O",
                egui::FontId::proportional(18.0),
                Color32::from_gray(160),
            );
            return;
        }
        let current = self.state.current_item().map(|i| i.primary.path.clone());
        let shown = self
            .state
            .loupe
            .shown
            .clone()
            .filter(|s| Some(&s.path) == current.as_ref());
        if let (Some(shown), Some(img_rect)) = (shown.clone(), self.state.loupe.image_rect()) {
            let screen = Rect::from_min_size(
                rect.min + Vec2::new(img_rect.x, img_rect.y) / ppp,
                Vec2::new(img_rect.w, img_rect.h) / ppp,
            );
            if let Some(l) = self.drawable(&shown.path, shown.source) {
                let key = (l.path.clone(), l.level);
                let full = (l.size.0 as f32, l.size.1 as f32);
                if let Some(up) = self.textures.get(&key) {
                    for (t, tex) in &up.tiles {
                        painter.image(
                            tex.id(),
                            tile_rect(screen, full, t),
                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                }
                if self.state.loupe.clipping {
                    self.draw_overlay(&painter, &l, screen, full);
                }
                self.displayed = Some(Displayed {
                    path: l.path.clone(),
                    source: l.source,
                    size: l.size,
                });
            }
        }
        // Overlay (top-left): preview-source badge (VIEW-3) and marks.
        let item_name = self.state.current_item().map(|i| i.file_name());
        let badge = match (&shown, &self.displayed) {
            (Some(s), _) if s.source == PreviewSource::Unavailable => Some(s.source.badge()),
            (_, Some(d)) => Some(d.source.badge()),
            _ => None,
        };
        // Real widgets (not painted text) so screen readers and UI tests see them.
        let overlay = egui::UiBuilder::new()
            .max_rect(rect.shrink(8.0))
            .layout(egui::Layout::top_down(egui::Align::Min));
        let marks = self.marks_text();
        ui.scope_builder(overlay, |ui| {
            if let Some(b) = badge {
                egui::Frame::NONE
                    .fill(Color32::from_black_alpha(170))
                    .corner_radius(3.0)
                    .inner_margin(egui::Margin::symmetric(6, 3))
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(b)
                                .monospace()
                                .size(13.0)
                                .color(Color32::WHITE),
                        );
                    });
            }
            if let Some(text) = marks {
                ui.label(
                    egui::RichText::new(text)
                        .size(16.0)
                        .color(Color32::from_gray(230)),
                );
            }
        });
        if self.state.loupe.pending_full {
            let r =
                Rect::from_min_size(rect.right_top() + Vec2::new(-36.0, 12.0), Vec2::splat(24.0));
            egui::Spinner::new().paint_at(ui, r);
        }
        if let Some(t) = &self.state.toast
            && ctx.input(|i| i.time) - self.toast_at < 4.0
        {
            painter.text(
                rect.left_bottom() + Vec2::new(12.0, -12.0),
                egui::Align2::LEFT_BOTTOM,
                t,
                egui::FontId::proportional(14.0),
                Color32::from_gray(220),
            );
        }
        if shown.is_none() && item_name.is_some() {
            // Loading: never draw another item's pixels (PERF-3).
            ctx.request_repaint();
        }
    }

    fn marks_text(&self) -> Option<String> {
        let item = self.state.current_item()?;
        let m = self.state.marks.get(&item.primary.path)?;
        let mut s = match m.rating {
            Some(-1) => "REJECTED".to_string(),
            Some(n) if n > 0 => "★".repeat(n as usize),
            _ => String::new(),
        };
        if let Some(l) = &m.label {
            if !s.is_empty() {
                s.push_str("  ");
            }
            s.push_str(l);
        }
        (!s.is_empty()).then_some(s)
    }

    /// The image to draw for `path`: the one matching the shown source if its textures are
    /// complete; while a full-resolution image is still uploading, the preview stays.
    fn drawable(&mut self, path: &Path, source: PreviewSource) -> Option<Arc<Loaded<Prepared>>> {
        let full = self
            .loader
            .get(path, Level::Full)
            .filter(|l| l.source == source);
        if let Some(f) = full {
            if self.upload(&f, FULL_TILES_PER_FRAME) {
                return Some(f);
            }
            self.ctx.request_repaint();
        }
        let p = self.loader.get(path, Level::Preview)?;
        self.upload(&p, usize::MAX);
        Some(p)
    }

    /// Upload up to `budget` more tiles of `l`. Returns true when all are on the GPU.
    fn upload(&mut self, l: &Loaded<Prepared>, budget: usize) -> bool {
        let key = (l.path.clone(), l.level);
        self.l1_order.retain(|k| k != &key);
        self.l1_order.push_back(key.clone());
        while self.l1_order.len() > L1_IMAGES {
            if let Some(old) = self.l1_order.pop_front() {
                self.textures.remove(&old);
                self.overlay_textures.remove(&old);
                self.overlays.remove(&old);
            }
        }
        let up = self.textures.entry(key).or_insert_with(|| Uploaded {
            tiles: Vec::new(),
            total: l.prepared.tiles.len(),
        });
        let start = up.tiles.len();
        for t in l.prepared.tiles.iter().skip(start).take(budget) {
            let tex = self.ctx.load_texture(
                format!("{}#{}x{}", l.path.display(), t.x, t.y),
                t.image.clone(),
                TextureOptions::LINEAR,
            );
            up.tiles.push((t.clone(), tex));
        }
        up.tiles.len() == up.total
    }

    fn draw_overlay(
        &mut self,
        painter: &egui::Painter,
        l: &Arc<Loaded<Prepared>>,
        screen: Rect,
        full: (f32, f32),
    ) {
        let key = (l.path.clone(), l.level);
        let cell = self.overlays.entry(key.clone()).or_insert_with(|| {
            let cell = Arc::new(OnceLock::new());
            let (c, l, ctx) = (cell.clone(), l.clone(), self.ctx.clone());
            std::thread::spawn(move || {
                let _ = c.set(tiles::overlay(&l.preview.image));
                ctx.request_repaint();
            });
            cell
        });
        let Some(tiles) = cell.get() else {
            return;
        };
        let ctx = &self.ctx;
        let texs = self.overlay_textures.entry(key).or_insert_with(|| {
            tiles
                .iter()
                .map(|t| {
                    let tex = ctx.load_texture(
                        format!("clip#{}x{}", t.x, t.y),
                        t.image.clone(),
                        TextureOptions::NEAREST,
                    );
                    (t.clone(), tex)
                })
                .collect()
        });
        for (t, tex) in texs.iter() {
            painter.image(
                tex.id(),
                tile_rect(screen, full, t),
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
    }
}

/// Screen rectangle of tile `t` of an image of `size` pixels drawn over `screen`.
fn tile_rect(screen: Rect, size: (f32, f32), t: &Tile) -> Rect {
    let sx = screen.width() / size.0;
    let sy = screen.height() / size.1;
    Rect::from_min_size(
        screen.min + Vec2::new(t.x as f32 * sx, t.y as f32 * sy),
        Vec2::new(t.w as f32 * sx, t.h as f32 * sy),
    )
}

/// `312 / 2,041` (SPEC §5.1).
pub fn counter(current: Option<usize>, len: usize) -> String {
    match current {
        Some(i) if len > 0 => format!("{} / {}", thousands(i + 1), thousands(len)),
        _ => format!("0 / {}", thousands(len)),
    }
}

fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Luminance (filled) and R/G/B (lines) histogram, labelled with its source (VIEW-6).
fn histogram_widget(ui: &mut egui::Ui, h: &blink_decode::stats::Histogram) {
    ui.label(h.label());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(256.0, 100.0), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, Color32::from_gray(16));
    let max = [&h.luma, &h.red, &h.green, &h.blue]
        .iter()
        .flat_map(|a| a[1..255].iter())
        .copied()
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    let y = |v: u32| rect.max.y - (v as f32 / max).min(1.0) * rect.height();
    for (i, &v) in h.luma.iter().enumerate() {
        let x = rect.min.x + i as f32 + 0.5;
        p.line_segment(
            [Pos2::new(x, rect.max.y), Pos2::new(x, y(v))],
            egui::Stroke::new(1.0, Color32::from_gray(110)),
        );
    }
    for (a, c) in [
        (&h.red, Color32::from_rgb(230, 60, 60)),
        (&h.green, Color32::from_rgb(60, 200, 60)),
        (&h.blue, Color32::from_rgb(80, 120, 255)),
    ] {
        let pts: Vec<Pos2> = a
            .iter()
            .enumerate()
            .map(|(i, &v)| Pos2::new(rect.min.x + i as f32 + 0.5, y(v)))
            .collect();
        p.add(egui::Shape::line(pts, egui::Stroke::new(1.0, c)));
    }
    ui.label(format!(
        "Clipped: {} highlights, {} shadows",
        h.highlights_clipped, h.shadows_clipped
    ));
}

/// Dark neutral theme (SPEC §5.1): chrome at or below 18 % grey, no tinted backgrounds.
pub fn apply_theme(ctx: &egui::Context) {
    ctx.set_visuals(egui::Visuals::dark());
}

impl eframe::App for BlinkApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_uses_thousands_separators() {
        assert_eq!(counter(Some(311), 2041), "312 / 2,041");
        assert_eq!(counter(None, 0), "0 / 0");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }
}
