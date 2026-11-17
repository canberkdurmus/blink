# 0002. M0 dependencies: eframe/egui, and xtask dev tooling

Date: 2026-11-17 · Status: accepted

## Context

SPEC §8.4 fixes egui + eframe on wgpu for the GUI and requires an ADR for every non-dev
dependency, covering license, maintenance status and whether it builds on all 6 targets.

## Decision

- **`eframe` / `egui` `=0.37.0`** (exact pin, workspace dependency). License MIT OR Apache-2.0.
  Very actively maintained (emilk/egui, monthly releases). Features: `wgpu`, `accesskit`,
  `default_fonts`, `wayland`, `x11`; default features `glow`, `clipboard`, `links`, `system_fonts`
  and `web_screen_reader` are off to keep the tree small. Builds on all 6 targets (pure Rust apart
  from platform system libraries loaded at runtime); proven by the M0 CI matrix.
  egui's bundled default fonts are OFL-1.1 / Ubuntu-font-1.0, which are compatible with distribution
  next to MPL-2.0 code. The third-party notice (M5) must list them.
- **xtask dependencies** (`quick-xml`, `serde`, `sha2`, `toml`): dev-only, never shipped. All
  MIT/Apache-2.0. HTTP downloads in `corpus fetch` shell out to `curl` (present on every CI image,
  including Windows 10+), which avoids a TLS stack (ring's C build) in xtask.
- **`cargo deny`** enforces the license allow-list in `deny.toml`. Unmaintained advisories are fatal
  for direct dependencies only. Transitive ones (currently zbus/zvariant via AccessKit's AT-SPI
  backend) are tracked by the weekly run.

## Alternatives

- eframe with `glow` (OpenGL): smaller, but SPEC fixes wgpu.
- `ureq` for downloads: pulls in rustls + ring. Not worth it for a dev tool.

## Consequences

eframe updates are deliberate (the exact pin). egui APIs change between minors (0.37 renamed
`App::update` → `App::ui` and `Context::run` → `run_ui`).
