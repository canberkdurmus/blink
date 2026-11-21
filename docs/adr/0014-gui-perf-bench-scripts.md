# 0014. GUI performance: bench scripts, headless measurement, throughput changes

Date: 2026-11-21 · Status: accepted

## Context

M2's gate needs PERF-1, PERF-2, PERF-3 and PERF-5 "within budget on CI baselines" (SPEC §7, §13).
SPEC §7: "`blink --bench-script <file>` replays an action script against a folder and writes
JSON results. `cargo xtask bench` runs the standard scripts and compares with
`perf/baseline-<target>.json`." CI runners have no GPU and no display.

## Decision

- **Scripts** live in `perf/scripts/*.json` (`id`, `scenario`, `runs`, plus `seconds`/`rate_hz`
  for the held key). `blink_ui::bench` runs them against the **real `BlinkApp`**, the real loader
  and `RealFs`, **headless**: an `egui::Context` with no renderer, 1600×1000 points.
  "Displayed" means the frame's loupe drew the current item's textures. GPU upload time and
  vsync are not part of the numbers. `blink --bench-script <script> <folder> [--out <dir>]`
  runs a script from the command line. The `req_perf_{1,2,3,5}_*` tests in `blink-ui/tests/perf.rs`
  (full tier: they take seconds and PERF-5 needs the corpus) run the same scripts and write
  `target/perf/<ID>.json`, which `cargo xtask bench` compares with the baseline. The CI perf job
  now fetches the corpus.
- **Scenarios.**
  - PERF-1 `launch`: from constructing the app on a folder to the first frame that draws item 1.
    A fresh app and empty Blink caches each run. The OS page cache can't be dropped without
    root, so "cold" means cold for Blink.
  - PERF-2 `next_prefetched`: once prefetch has settled, the time from the `→` key to the end of
    the frame that draws the next item. 21 presses, median.
  - PERF-3 `held_key`: `→` at 40 Hz (a typical OS key-repeat rate) for 3 s at 60 frames/s. Metric:
    ms per distinct item drawn (budget 1000/30 ms), so the 40 Hz key rate caps it at 25 ms.
    Frames that draw another item's pixels are counted, and the test requires zero.
  - PERF-5 `raw_decode`: the 45 MP class file in the corpus (Sony ILCE-1M2, 8640×5760) carries a
    **full-size** embedded preview, so VIEW-4 correctly never decodes it on zoom. The script
    therefore dispatches `Action::DecodeRaw` (an explicit "decode the RAW now"; it ignores a
    paired JPEG) together with zoom 100 %, and measures until the RAW decode is drawn **with every
    pixel** (`Displayed::pixels == size`), including the full-resolution tile build and upload. It also
    reports the longest UI frame meanwhile as `PERF-5/frame` (budget 33 ms).
- **Synthetic folder for PERF-1/2/3**: `blink_testkit::fixtures::perf_folder(2000)` creates 2,000
  `.ARW` hard links (copies where links fail) to one TIFF-container file with the 6000×4000
  PERF-4 JPEG preview (24 MP class), under `target/perf-fixtures/`. rawler can't probe it, so
  that cost is a fast failure. Real RAWs add a few ms of probing per file.
- **Throughput changes the first measurement forced** (PERF-3 was ~6 items/s):
  - The loader gives each job `cores / running jobs` decode threads. An idle loader decodes the
    current item on all cores (latency). A busy one no longer runs ~15 jobs × 16 threads.
  - Histograms are computed lazily (`Loaded::histogram`, on all cores, off the UI thread, only
    when the info panel shows them) and not for every prefetched item.
  - Texture tiles are built in parallel.
  - **Screen-resolution variant** (SPEC §8.3 "navigate at screen resolution"): an image whose
    long edge exceeds 2,560 px is prepared as box-filtered tiles of at most 2,560 px. The loupe
    draws them while the image is shown no larger than that, and its real pixels otherwise
    (zoom, or a loupe wider than 2,560 px). Full-resolution tiles are built on demand by **one**
    builder thread that holds requests weakly and builds only the newest request for the current
    item, so holding → at 100 % never runs several ~100 MB builds at once. While they build, the
    best complete variant stays on screen.
  - `Loader::clear()` (folder reopened) bumps an epoch: jobs started before it finish but never
    fill the cache or report, and don't count as "already running" for the next `Show`.

## Consequences

- Local numbers (Ryzen 7 PRO 7840U): PERF-1 192 ms, PERF-2 0.6 ms, PERF-3 25 ms/item (every key
  repeat drawn, 0 stale frames), PERF-5 748 ms, longest frame 0.9 ms.
- CI runners (4 cores, no GPU) are much slower than the reference-class laptop (e.g. PERF-3
  ~270 ms/item, PERF-5 ~2.1 s). Per SPEC §7 their numbers are **regression baselines**; the
  absolute budgets are judged on the reference-class machine (as for PERF-4, M1 report).
- The gate report asks the owner to confirm the absolute budgets on real hardware with a GPU
  (HUMAN_TODO).
- `DecodeRaw` is not bound to a key in v1. It exists for honest measurement and stays covered
  by tests.
