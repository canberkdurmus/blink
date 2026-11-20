# 0012. Loupe state and the image loader

Date: 2026-11-20 · Status: accepted

## Context

M2 adds the loupe (VIEW-1), zoom to 100 % with a full-resolution fallback (VIEW-4), and
zoom/pan (VIEW-9). SPEC §8.1 asks for a pure reducer, and §8.3 asks for decode pools, a prefetch
window, cancellation and cache tiers. Pixels must not travel through the cloned `State`.

## Decision

- **Loupe model in `blink-core::view`.** Zoom levels Fit/50/100/200 % are relative to the
  **full-resolution** size (`Shown::full_size`: sensor crop for RAW/DNG, after orientation;
  the file's own size for a JPEG). The displayed pixels are stretched over that size, so the
  upscaled LOW-RES preview and the later RAW decode occupy the same rectangle and the view
  doesn't jump on the swap. Pan/zoom keep a normalised centre, clamped so the image covers the
  viewport. Zoom at the cursor keeps the image point under the cursor.
- **When a full image is needed (VIEW-4).** `Loupe::needs_full`: the preview is smaller than the
  full size *and* the zoom would upscale it. That covers "LOW-RES" and "non-LOW-RES previews
  smaller than the sensor" with one rule. The reducer emits `Effect::LoadFull` once per item
  (`pending_full` = spinner). The runtime uses the paired JPEG if one exists, else decodes the RAW.
  With "keep zoom" on (the default, VIEW-9), a new item at 100 % requests its full image as soon
  as its preview arrives.
- **Never stale (PERF-3).** Navigation clears `shown`. Only an `ImageReady` for the current item
  sets it. The UI draws only `shown`, so a frame can't show the previous item's pixels.
- **Loader (`blink-runtime::loader`).** Decode workers = logical cores − 1, minimum 1. std has no
  portable physical-core count, and the JPEG decoder fans out internally anyway (ADR 0011). One
  priority queue: the current item first, then the prefetch window (8 ahead, 2 behind, nearest
  first, the first item behind right after the first item ahead). Each `Show` replaces the queue and bumps a
  generation, and jobs check it before decoding (the cancellation token). Full-resolution loads
  run on one dedicated thread, newest request wins, so a 1 s RAW decode never blocks previews.
  The L2 cache is a byte-capped LRU (default 2 GB, PERF-8 tunes it in M4) holding the decoded
  RGB, its histogram, and a UI-prepared form `T`. `T` is built on the worker by a hook (texture
  tiles), so the runtime stays free of egui. `Show` and `LoadFull` answer synchronously on a
  cache hit, so a prefetched item is shown in the same frame (PERF-2).
- **RAW decode.** `RawBackend::develop` uses rawler's `RawDevelop` default pipeline (demosaic,
  white balance, camera matrix, default crop, sRGB gamma) and converts to 8-bit on all cores.
  The output is the sensor crop, then oriented by EXIF for display.

## Consequences

- No new dependencies.
- Screen-resolution DCT decoding during held keys (SPEC §8.3) is not done yet. The parallel
  decoder already meets the item-rate budget (PERF-3). Revisit if PERF-3 fails on slower targets.
- L1 (GPU textures) lives in the UI; the L3 disk thumbnail cache arrives with the grid (M4).
