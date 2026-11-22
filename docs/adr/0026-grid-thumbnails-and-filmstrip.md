# 0026. VIEW-5 / PERF-6: grid, thumbnails and filmstrip

Status: accepted (2026-11-22)

## Context

VIEW-5: "Grid view with 3 thumbnail sizes; filmstrip toggle in the loupe", accepted by a
screenshot golden and the PERF-6 grid benchmark ("grid of 2,000 thumbnails, cold: visible ones
≤ 1 s, all ≤ 10 s"). Keys (SPEC §5.2): `G` grid, `E`/`Enter` loupe, `T` filmstrip. SPEC §8.3
also describes an L3 on-disk thumbnail cache.

## Decision

1. **Thumbnails** (`blink_decode::thumb`). The source is the smallest embedded image whose
   long edge reaches the tier's minimum, else the largest there is. It is decoded on one thread,
   box-downscaled, converted to sRGB and oriented like the loupe (VIEW-8, VIEW-10).
   - The `Thumb` tier (small and medium cells, filmstrip) needs 120 px and stores ≤ 192 px, so
     the 160×120 EXIF/TIFF thumbnail most RAWs carry is enough.
   - The `Large` tier needs 288 px and stores ≤ 320 px.
   - Standalone JPEGs are decoded whole. HEIF has none ("NO PREVIEW"). A RAW without a usable
     preview falls back to its paired JPEG.
2. **Reads** (`blink_runtime::thumbs`). Each file is read as a 256 KB prefix first, and in full
   only when the needed image lies beyond it. A worker pool (cores − 1) serves the order the UI
   sends: visible cells first, then outward from them. The order is re-sent only when the
   visible range, the list or the tier changes. Switching tier drops the other tier's
   thumbnails. Opening a folder clears them all.
3. **State** (`blink-core`). `view` (loupe/grid), `thumb_size` (S 96, M 160 default, L 288 pt
   cells) and `filmstrip` live in the reducer. `Select(i)` and `Step(±n)` move the current item.
   `↑`/`↓` in the grid step by the column count. A double click opens the loupe.
4. **UI.** Virtualised rows (`ScrollArea::show_rows`). At most 48 thumbnail textures are
   uploaded per frame, and 900 are kept, farthest-from-view dropped first. The current item is
   framed and scrolled into view. Stars, reject and label are painted on each cell. Each cell is
   an accessible button named after its file. The filmstrip is a bottom panel of small cells
   centred on the current item.
5. **L3 disk cache: not built in M4.** PERF-6 is defined cold, and prefix reads of the embedded
   thumbnail make 2,000 thumbnails in ~0.25 s locally. The cache would add writes into the
   platform cache dir and an invalidation scheme without changing any acceptance number.
   Recorded in `docs/ideas.md` for later; flagged in the M4 report.

## Consequences

- Thumbnail memory: ≤ 2,000 × 192×128×3 bytes ≈ 150 MB for the `Thumb` tier, ~600 MB for
  `Large`. This is outside the PERF-8 decode cache, which SPEC defines for decodes.
- Benchmark: `perf/scripts/perf-6-grid.json`, scenario `grid_cold`. Metrics `PERF-6` (visible)
  and `PERF-6/all`.
