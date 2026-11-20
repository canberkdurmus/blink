# 0013. Folder dialog (rfd) and UI tests (egui_kittest)

Date: 2026-11-20 · Status: accepted

## Context

VIEW-1 brings the GUI loupe and the OPEN-1 entry points that ADR 0010 deferred: drag-and-drop,
the `Ctrl/Cmd+O` dialog and `blink [folder]`. SPEC §8.4 names `rfd` for dialogs and asks for an
ADR on the UI test approach ("`egui_kittest` if it fits"). SPEC §11.1 wants pixel goldens on Linux.

## Decision

- **`rfd =0.18.0`** (MIT). This is the most used native file dialog crate for Rust, released
  regularly. Default features: the `xdg-portal` backend on Linux (pure Rust over D-Bus, so no GTK
  build dependency) and `wayland`. It uses Win32 on Windows and AppKit on macOS, so it builds on
  all 6 targets with no C toolchain beyond what eframe already needs. It has no network access.
  The sync `pick_folder()` runs on the UI thread. SPEC §5.1 allows a modal dialog for "open folder".
  On Linux, a desktop without an xdg portal returns `None`: the dialog does nothing, and
  drag-and-drop and `blink [folder]` still work.
- **All three entry points dispatch `Action::OpenFolder`** through `BlinkApp::open_path`. A
  dropped or passed *file* opens its folder. Tests cover the command-line argument and the drop
  event. A native dialog can't run in CI, so the dialog path is a one-line call into the same
  function.
- **UI tests use `egui_kittest =0.37.0`** (dev-only), matching the pinned egui. State and
  AccessKit queries (`get_by_label`) run in the fast tier on every target. Overlay text such as the
  preview-source badge is drawn as real label widgets, not painted text, so it is queryable and
  screen readers can read it (this also prepares for A11Y-1). The **screenshot golden** renders with
  wgpu on lavapipe (Mesa's software Vulkan, `mesa-vulkan-drivers` installed in CI). It is Linux-only
  and in the full tier (`full_ui` module). Snapshots live in `crates/blink-ui/tests/snapshots/`, and
  the CI guard enforces `golden-update:` trailers for them. Regenerate with `UPDATE_SNAPSHOTS=1`.
- **Textures** are tiles of at most 2048 px (SPEC §8.3: ≤ 4096), built on loader workers as
  `Arc<ColorImage>`, so the UI thread does no pixel conversion. A preview is uploaded in one
  frame (PERF-2). A full-resolution image is uploaded at 2 tiles per frame while the upscaled
  preview stays on screen (PERF-5: no frame > 33 ms). L1 keeps the textures of the last 6 images.

## Consequences

rfd's Linux portal path pulls `ashpd`/`zbus`. `zbus` is already in the tree via AccessKit (ADR
0002). `cargo deny` covers the new licenses (MIT/Apache-2.0).
