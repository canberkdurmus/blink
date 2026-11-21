//! Bench scripts (SPEC §7): replay a scripted scenario against a folder with the real app,
//! headless (an egui context without a GPU), and report timings as JSON.
//!
//! A script is a small JSON object, e.g. `perf/scripts/perf-2-next.json`:
//! `{ "id": "PERF-2", "scenario": "next_prefetched", "runs": 20 }`.
//! Scenarios:
//! - `launch` (PERF-1): construct the app on the folder until the first item is drawn.
//! - `next_prefetched` (PERF-2): with prefetch settled, `→` until the new item is drawn.
//! - `held_key` (PERF-3): `→` repeated at `rate_hz` for `seconds`; ms per item actually drawn,
//!   and the number of frames that drew another item's pixels (must be 0).
//! - `raw_decode` (PERF-5): `Z` on the first item until the RAW decode is drawn; also the longest
//!   UI frame meanwhile.
//!
//! "Drawn" means the frame's loupe painted the item's textures; GPU upload time is not included
//! (ADR 0014).

use crate::{BlinkApp, Options};
use blink_core::preview::PreviewSource;
use std::path::Path;
use std::time::{Duration, Instant};

/// One measured metric.
#[derive(Debug, Clone, PartialEq)]
pub struct Measured {
    pub id: String,
    pub metric: String,
    pub budget_ms: f64,
    pub runs_ms: Vec<f64>,
}

impl Measured {
    pub fn median_ms(&self) -> f64 {
        let mut r = self.runs_ms.clone();
        r.sort_by(f64::total_cmp);
        r.get(r.len() / 2).copied().unwrap_or(f64::NAN)
    }

    /// The JSON line `cargo xtask bench` reads.
    pub fn to_json(&self) -> String {
        let runs: Vec<String> = self.runs_ms.iter().map(|v| format!("{v:.3}")).collect();
        format!(
            "{{\"id\":\"{}\",\"metric\":\"{}\",\"median_ms\":{:.3},\"budget_ms\":{},\"runs_ms\":[{}],\"optimized\":{}}}\n",
            self.id,
            self.metric,
            self.median_ms(),
            self.budget_ms,
            runs.join(", "),
            !cfg!(debug_assertions)
        )
    }
}

/// Result of a script: one or more metrics, plus counters that must be zero.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub metrics: Vec<Measured>,
    /// Frames that drew pixels of an item other than the current one (PERF-3).
    pub stale_frames: u64,
}

/// A parsed bench script.
#[derive(Debug, Clone, PartialEq)]
pub struct Script {
    pub id: String,
    pub scenario: String,
    pub runs: usize,
    pub seconds: f64,
    pub rate_hz: f64,
}

fn json_str(text: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\"");
    let rest = text[text.find(&pat)? + pat.len()..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start().strip_prefix('"')?;
    Some(rest.split('"').next()?.to_string())
}

fn json_num(text: &str, key: &str) -> Option<f64> {
    let pat = format!("\"{key}\"");
    let rest = text[text.find(&pat)? + pat.len()..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

impl Script {
    pub fn parse(text: &str) -> Result<Self, String> {
        let id = json_str(text, "id").ok_or("script: missing \"id\"")?;
        let scenario = json_str(text, "scenario").ok_or("script: missing \"scenario\"")?;
        Ok(Self {
            id,
            scenario,
            runs: json_num(text, "runs").map_or(5, |n| n.max(1.0) as usize),
            seconds: json_num(text, "seconds").unwrap_or(3.0),
            rate_hz: json_num(text, "rate_hz").unwrap_or(40.0),
        })
    }
}

/// The app in an egui context without a renderer.
struct Headless {
    ctx: egui::Context,
    app: BlinkApp,
    longest_frame: Duration,
}

const SIZE: egui::Vec2 = egui::vec2(1600.0, 1000.0);

impl Headless {
    fn launch(folder: &Path, opts: Options) -> Self {
        let ctx = egui::Context::default();
        let app = BlinkApp::new(&ctx, Some(folder.to_path_buf()), opts);
        Self {
            ctx,
            app,
            longest_frame: Duration::ZERO,
        }
    }

    /// Run one frame with `keys` pressed; returns how long the UI thread took.
    fn frame(&mut self, keys: &[egui::Key]) -> Duration {
        // Press and release: a key never released turns later presses into repeats, which
        // mark keys ignore.
        let events = keys
            .iter()
            .flat_map(|&key| {
                [true, false].map(|pressed| egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                })
            })
            .collect();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SIZE)),
            events,
            ..Default::default()
        };
        let t = Instant::now();
        let app = &mut self.app;
        let out = self.ctx.run_ui(input, |ui| app.show(ui));
        let d = t.elapsed();
        out.drop_without_applying_deltas();
        self.longest_frame = self.longest_frame.max(d);
        d
    }

    fn current(&self) -> Option<std::path::PathBuf> {
        self.app
            .state()
            .current_item()
            .map(|i| i.primary.path.clone())
    }

    /// The current item is drawn.
    fn drawn(&self) -> bool {
        let cur = self.current();
        cur.is_some() && self.app.displayed().map(|d| &d.path) == cur.as_ref()
    }

    /// Drawn with nothing left to load.
    fn settle(&mut self, timeout: Duration) -> Result<(), String> {
        let t = Instant::now();
        loop {
            self.frame(&[]);
            if self.drawn() && self.app.loader_idle() {
                return Ok(());
            }
            if t.elapsed() > timeout {
                return Err("bench: the folder never finished loading".into());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Run `script` against `folder`.
pub fn run(script: &Script, folder: &Path, opts: impl Fn() -> Options) -> Result<Report, String> {
    let timeout = Duration::from_secs(120);
    let mut stale_frames = 0;
    let metric = |name: &str, budget: f64, runs: Vec<f64>| Measured {
        id: script.id.clone(),
        metric: name.into(),
        budget_ms: budget,
        runs_ms: runs,
    };
    let metrics = match script.scenario.as_str() {
        "launch" => {
            let mut runs = Vec::new();
            for _ in 0..script.runs {
                let t = Instant::now();
                let mut h = Headless::launch(folder, opts());
                while !h.drawn() {
                    h.frame(&[]);
                    if t.elapsed() > timeout {
                        return Err("bench: first image never drawn".into());
                    }
                    std::thread::sleep(Duration::from_micros(200));
                }
                runs.push(ms(t.elapsed()));
            }
            vec![metric(
                "launch to first image drawn (fresh app and caches)",
                400.0,
                runs,
            )]
        }
        "next_prefetched" => {
            let mut h = Headless::launch(folder, opts());
            let mut runs = Vec::new();
            for _ in 0..script.runs {
                h.settle(timeout)?;
                let t = Instant::now();
                h.frame(&[egui::Key::ArrowRight]);
                while !h.drawn() {
                    h.frame(&[]);
                    if t.elapsed() > timeout {
                        return Err("bench: next item never drawn".into());
                    }
                }
                runs.push(ms(t.elapsed()));
            }
            vec![metric(
                "next item drawn, prefetched (key to end of frame)",
                16.0,
                runs,
            )]
        }
        "held_key" => {
            let mut runs = Vec::new();
            for _ in 0..script.runs {
                let mut h = Headless::launch(folder, opts());
                h.settle(timeout)?;
                let frame_dt = Duration::from_secs_f64(1.0 / 60.0);
                let key_dt = Duration::from_secs_f64(1.0 / script.rate_hz);
                let start = Instant::now();
                let mut next_key = start;
                let mut seen = std::collections::HashSet::new();
                while start.elapsed().as_secs_f64() < script.seconds {
                    let frame_start = Instant::now();
                    let keys: &[egui::Key] = if frame_start >= next_key {
                        next_key += key_dt;
                        &[egui::Key::ArrowRight]
                    } else {
                        &[]
                    };
                    h.frame(keys);
                    if let Some(d) = h.app.displayed() {
                        if Some(&d.path) == h.current().as_ref() {
                            seen.insert(d.path.clone());
                        } else {
                            stale_frames += 1;
                        }
                    }
                    if let Some(rest) = frame_dt.checked_sub(frame_start.elapsed()) {
                        std::thread::sleep(rest);
                    }
                }
                let secs = start.elapsed().as_secs_f64();
                runs.push(secs * 1000.0 / seen.len().max(1) as f64);
            }
            vec![metric(
                "held arrow key: ms per distinct item drawn (budget = 30 items/s)",
                1000.0 / 30.0,
                runs,
            )]
        }
        "raw_decode" => {
            let mut decode = Vec::new();
            let mut frames = Vec::new();
            for _ in 0..script.runs {
                let mut h = Headless::launch(folder, opts());
                h.settle(timeout)?;
                h.longest_frame = Duration::ZERO;
                let t = Instant::now();
                // The A1 II class files carry a full-size preview, so zoom alone wouldn't
                // decode: request the RAW decode explicitly (ADR 0014). Done when the decode is
                // drawn at 100 % with every pixel (full-resolution tiles built and uploaded).
                h.app.dispatch(blink_core::state::Action::SetZoom {
                    zoom: blink_core::view::Zoom::P100,
                    cursor: None,
                });
                h.app.decode_raw();
                h.frame(&[]);
                loop {
                    let drawn_raw = h.app.displayed().is_some_and(|d| {
                        d.source == PreviewSource::RawDecode && d.pixels == d.size
                    });
                    if drawn_raw {
                        break;
                    }
                    if t.elapsed() > timeout {
                        return Err(format!(
                            "bench: RAW decode never drawn ({:?})",
                            h.app.state().toast
                        ));
                    }
                    let f = Instant::now();
                    h.frame(&[]);
                    if let Some(rest) = Duration::from_millis(4).checked_sub(f.elapsed()) {
                        std::thread::sleep(rest);
                    }
                }
                decode.push(ms(t.elapsed()));
                frames.push(ms(h.longest_frame));
            }
            vec![
                metric(
                    "zoom to 100 % until the RAW decode is drawn with every pixel, 45 MP class",
                    1500.0,
                    decode,
                ),
                Measured {
                    id: format!("{}/frame", script.id),
                    metric: "longest UI frame during the RAW decode".into(),
                    budget_ms: 33.0,
                    runs_ms: frames,
                },
            ]
        }
        "rate_durable" => {
            // PERF-7: from a star key to the end of the sidecar's atomic write (temp file,
            // fsync, rename, directory fsync), debounce included. Each run rates the next item,
            // so every run creates a sidecar.
            let mut h = Headless::launch(folder, opts());
            let mut runs = Vec::new();
            let stars = [
                egui::Key::Num1,
                egui::Key::Num2,
                egui::Key::Num3,
                egui::Key::Num4,
                egui::Key::Num5,
            ];
            for i in 0..script.runs {
                h.settle(timeout)?;
                let before = h.app.sidecar_writes_done();
                let t = Instant::now();
                h.frame(&[stars[i % 5]]);
                while h.app.sidecar_writes_done() == before {
                    if t.elapsed() > timeout {
                        return Err(format!(
                            "bench: sidecar never written ({:?})",
                            h.app.state().toast
                        ));
                    }
                    let f = Instant::now();
                    h.frame(&[]);
                    if let Some(rest) = Duration::from_millis(1).checked_sub(f.elapsed()) {
                        std::thread::sleep(rest);
                    }
                }
                runs.push(ms(t.elapsed()));
                h.frame(&[egui::Key::ArrowRight]);
            }
            vec![metric(
                "rating key to sidecar durable on disk (debounce + atomic write)",
                250.0,
                runs,
            )]
        }
        other => return Err(format!("bench: unknown scenario {other:?}")),
    };
    Ok(Report {
        metrics,
        stale_frames,
    })
}

/// Write each metric to `<dir>/<ID>.json` (`/` in IDs becomes `-`).
pub fn write(report: &Report, dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for m in &report.metrics {
        std::fs::write(
            dir.join(format!("{}.json", m.id.replace('/', "-"))),
            m.to_json(),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_parse_with_defaults() {
        let s =
            Script::parse(r#"{ "id": "PERF-3", "scenario": "held_key", "seconds": 2 }"#).unwrap();
        assert_eq!(
            s,
            Script {
                id: "PERF-3".into(),
                scenario: "held_key".into(),
                runs: 5,
                seconds: 2.0,
                rate_hz: 40.0
            }
        );
        assert!(Script::parse("{}").is_err());
        let m = Measured {
            id: "PERF-5/frame".into(),
            metric: "m".into(),
            budget_ms: 33.0,
            runs_ms: vec![3.0, 1.0, 2.0],
        };
        assert_eq!(m.median_ms(), 2.0);
        assert!(m.to_json().contains("\"median_ms\":2.000"));
    }
}
