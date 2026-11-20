# 0011. PERF-4: parallel baseline-JPEG decoder

Date: 2026-11-19 · Status: accepted

## Context

PERF-4 (SPEC §7): "Embedded preview extract + decode, per file (worker thread) — ≤ 25 ms median
for a 6 MB JPEG". With zune-jpeg on one thread the bench file (6000×4000, 4:2:0, q90, 6.1 MB, no
restart markers) took ~70–76 ms on the reference-class laptop (Ryzen 7 PRO 7840U, 8 cores /
16 threads) and ~118 ms on the CI Linux runner (ADR 0008, `BLOCKED.md`). Huffman decoding alone
is ~45–55 ms and is serial, and 7 of the 11 corpus previews (Sony, Canon, Nikon, Panasonic) have
no restart markers, so splitting at RST markers can't help.

## Decision

1. **Reading of "per file (worker thread)".** The work for one file runs off the UI thread, on a
   worker. The worker may fan the decode of one large JPEG out over the CPU's cores. SPEC doesn't
   say "single-threaded", and the user-visible latency of one preview is what PERF-2/3 build on.
   (SPEC is owner-owned; this reading is flagged in the M1 gate report.)
2. **`blink-decode/src/pjpeg.rs`**, an in-house decoder for single-scan baseline JPEGs
   (8-bit, Huffman, 1 or 3 components, luma 1×1/2×1/1×2/2×2 with 1×1 chroma, no Adobe
   non-YCbCr transform):
   - **Scan:** the destuffed entropy data is split into one chunk per thread. Each chunk is parsed
     from an arbitrary bit offset without writing pixels, recording its first 1024 block starts
     and a checkpoint every 32 MCUs.
   - **Sync:** each chunk keeps parsing past its end until it reaches a block start the next
     chunk recorded at the same position within the MCU; from there the two parses are
     identical (JPEG's Huffman codes self-synchronise). A serial pass turns the per-chunk block
     counts and DC sums into global block indices and true DC predictors. An invalid code
     inside the true stream, a missing sync, or too few blocks rejects the file.
   - **Restart markers (DRI):** scan and sync are skipped. Every restart interval starts
     byte-aligned with DC predictors at zero, so the RST positions found while destuffing are
     exact start points. The RST count must match the MCU count.
   - **Decode:** one band of MCU rows per thread, starting from the nearest checkpoint. Each band
     decodes, inverse-transforms (libjpeg islow), upsamples (triangle filter, as zune-jpeg) and
     colour-converts one MCU row at a time into its own `&mut` slice of the RGB output, so the
     only large allocation is the output. The two luma rows at each band boundary that need the
     other band's chroma row are finished in a short second parallel pass.
   - Output matches zune-jpeg within rounding (corpus: max |Δ| ≤ 5; tests allow ≤ 8 and mean
     < 0.5) and is **bit-identical for any thread or chunk count** (tested).
3. **Fallback.** `image::decode_jpeg` uses `pjpeg` for JPEGs ≥ 128 KiB (chunks of at least
   32 KiB); anything it rejects (progressive, 4:1:1 and other layouts, CMYK, corrupt or truncated
   data, a panic in a worker on hostile input) is decoded by zune-jpeg as before. EXIF and ICC
   still come from zune-jpeg's header parse. Every JPEG preview in the corpus (11 of 12 files;
   Leica M8's is uncompressed) takes the parallel path.
4. **One `unsafe` call (CLAUDE.md: `unsafe` outside core/xmp needs an ADR).** x86-64's SSE2
   baseline has no 32-bit vector multiply, so the IDCT and colour conversion barely vectorise
   (IDCT 36 ms → 13 ms single-threaded with AVX2). `decode_band_avx2` is the same code compiled
   with `#[target_feature(enable = "avx2")]`; it is called only after
   `is_x86_feature_detected!("avx2")`, which is the whole safety argument. Every other line of
   `pjpeg` is safe Rust: disjoint output bands come from `split_at_mut`, threads from
   `std::thread::scope`. On aarch64 NEON is baseline and the plain path vectorises.
5. **No new dependency.** The decoder uses std and `memchr` (already a dependency).
6. **Bench methodology.** `cargo xtask bench` runs perf tests one at a time (`--test-threads 1`),
   so the timed test doesn't share cores with other tests. The median stays over 5 runs (SPEC §7).

## Measurements (Ryzen 7 PRO 7840U, release, shared desktop session)

| Phase (16 threads) | Wall time |
|---|---|
| Header parse + destuff (serial) | ~0.8 ms |
| Scan | ~3.5 ms |
| Sync + resolve | ~0.4 ms |
| Bands (decode, IDCT, upsample, colour) | ~10–11 ms |
| Edge rows, join | ~0.5 ms |

`cargo xtask bench` PERF-4 medians: **20.1, 20.3, 20.2 ms** (budget 25 ms; zune-jpeg: ~76 ms).

Corpus previews, `decode_jpeg` median of 11 (zune-jpeg → parallel):

| Preview | Size | zune-jpeg | parallel |
|---|---|---|---|
| Sony ILCE-1M2 | 8640×5760, 2.8 MB | 74 ms | 25 ms |
| Fujifilm X-S10 (DRI) | 4416×2944, 2.7 MB | 38 ms | 8.3 ms |
| Pentax K10D (DRI) | 3872×2592, 1.3 MB | 23 ms | 6.0 ms |
| Canon EOS R6 | 3408×2272, 0.9 MB | 16 ms | 4.6 ms |
| Olympus E-M10 (DRI) | 3200×2400, 0.9 MB | 15 ms | 3.9 ms |
| Nikon Z 6 | 3024×2016, 0.4 MB | 9.1 ms | 3.2 ms |

The 50 MP Sony preview is bounded by writing its 150 MB RGB output.

**Power state matters.** The numbers above are on AC power. On battery (amd-pstate EPP
`balance_power`) the same laptop caps all-core clocks while single-core turbo barely changes:
the PERF-4 decode then takes **~35–41 ms** (zune-jpeg: ~73 ms). PERF-4 is assessed on AC; the
battery figure is flagged in the M1 gate report for the owner.
Single-threaded the same code takes ~95 ms, so on a 4-core machine expect roughly 30–35 ms; the
CI Linux baseline is re-recorded from CI after this change.

## Consequences

- PERF-4 meets its budget on the reference-class machine; the CI baseline guards regressions.
- The app's decode workers (M2) each fan out over all cores for large previews. When several
  workers decode at once (prefetch), they share the cores; throughput is no worse than one
  thread per file, and the file the user is waiting for finishes sooner.
