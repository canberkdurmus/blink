# Blocked

Items that failed 3 attempts. Each entry is a `## <ID or topic>` heading with: symptom, attempts,
hypothesis, next idea. Remove entries once resolved.

## PERF-4 — embedded preview extract + decode ≤ 25 ms median for a 6 MB JPEG

- **Symptom:** the PERF-4 bench (6000×4000, 4:2:0, q90, 6.1 MB baseline JPEG, no restart markers;
  locate + choose + full decode, one thread) measures **~70–76 ms** release on the reference-class
  machine (Ryzen 7 PRO 7840U), and ~115 ms on the CI Linux x86-64 runner. Budget: 25 ms.
- **Attempts:**
  1. (ADR 0008) zune-jpeg vs jpeg-decoder, with and without DCT scaling (1/2, 1/4): zune-jpeg full
     decode is the fastest option; DCT scaling barely helps.
  2. (2026-11-18) Measured the serial floor on the bench file (release, scratch crate, median of 7):
     zune-jpeg RGB 69.6 ms, YCbCr out 74.1 ms, luma only 54.1 ms; jpeg-decoder full 80.0 ms,
     jpeg-decoder 1/8 scale (≈ entropy decoding only) 56.0 ms. Entropy (Huffman) decoding alone is
     ~2× the budget, and it is serial.
  3. (2026-11-18) Restart-interval parallelism: surveyed the chosen preview of every corpus file.
     Only Fujifilm, Olympus, Pentax and Apple previews carry DRI; Sony, Nikon and Canon (and the
     bench file) don't, so splitting at RST markers can't meet the budget in general.
- **Hypothesis:** 25 ms for a 6 MB baseline JPEG means ~1 GP/s, about 3–4× what any single-threaded
  decoder achieves. Without restart markers it can only be met with a speculative parallel Huffman
  decoder (self-synchronising chunks, DC fix-up by prefix sum, then parallel IDCT/colour).
- **Next idea:** dedicated attempt 1: an in-house parallel baseline-JPEG decoder in `blink-decode`
  (speculative chunk sync), falling back to zune-jpeg for anything unsupported, verified against
  zune-jpeg on the corpus and synthetic files. Dedicated attempt 2 depends on its numbers. If both
  fail, escalate: the owner decides whether PERF-4's budget changes (SPEC is owner-owned).
