# 0008. Decode pipeline: preview location, EXIF, colour, and its dependencies

Date: 2026-11-17 · Status: accepted

## Context

M1 needs `blink-decode` to find every embedded image (VIEW-2, CLI-2), mark LOW-RES (§3), show
the right badge (VIEW-3), read EXIF for the info panel (VIEW-7), honour orientation (VIEW-8),
convert Adobe RGB previews (VIEW-10) and do it fast (PERF-4). SPEC §8.4 fixes rawler behind a
`RawBackend` trait, zune-jpeg and a pure-Rust CMS (evaluate moxcms).

## Decision

### Dependencies (all in `blink-decode`)

| Crate | Version | License | Notes |
|---|---|---|---|
| `rawler` | `=0.8.0`, default features off (no `rawdb`/`ureq`, so no network code) | LGPL-2.1 | dnglab project, active. Pure Rust (memmap2/rayon/jxl-oxide/image). Wrapped by `raw::RawBackend`; only `raw.rs` names it. LGPL: Blink is open source, so users can rebuild with a modified rawler; the third-party notice (M5) states this. |
| `zune-jpeg` | `=0.5.15` | MIT OR Apache-2.0 OR Zlib | Pure Rust, SIMD on x86 and NEON. 0.5.16 is a release candidate, so it's not used. |
| `moxcms` | `=0.9.2` | BSD-3-Clause OR Apache-2.0 | Pure Rust ICC CMS with built-in sRGB and Adobe RGB profiles and 8-bit transforms. It meets VIEW-10's ΔE2000 < 2 against an independent reference, so `lcms2` (C) isn't needed. |
| `memchr` | `2` | MIT OR Unlicense | Already in the tree via rawler. SIMD `memmem` search for JPEG SOI markers: 53 MB in ~2 ms, against ~20 ms for a byte loop. |

They all build on the 6 CI targets (pure Rust) and none adds network access, telemetry or an updater.

### Locating embedded images

The locator is format-agnostic, so every rawler format works without a per-maker parser:

1. Scan for `FF D8 FF` and validate each hit by walking its marker segments to EOI. SOF0/1/2 only:
   lossless SOF3 (raw data in CR2/DNG) is rejected. Nested EXIF thumbnails are found too, and they
   are listed in doctor.
2. Walk the TIFF IFDs (IFD0 chain plus SubIFDs) for uncompressed 8-bit RGB previews, as in Leica DNGs.

**Largest** means most pixels, then the longer edge, then the most bytes. Sensor size is rawler's
recommended crop. When rawler can't decode a file (Nikon HE\* in 0.8.0), it is the TIFF raw IFD's
size or the DNG DefaultCropSize. An unknown sensor size is never LOW-RES.

### EXIF and the info panel

A small bounds-checked TIFF reader in `blink-decode` reads only standard EXIF tags. The lens is EXIF
`LensModel` (0xA434), not rawler's normalised lens name and not exiftool's maker-note `LensID`.
That matches `exiftool -EXIF:LensModel` exactly. Containers: TIFF-based (incl. ORF and RW2 magics,
with RW2 ISO from IFD0 tag 0x0017), JPEG APP1, RAF (its embedded JPEG) and CR3 (CMT1/CMT2 boxes).

### Orientation and colour

Embedded previews are stored unrotated, so the container's IFD0 orientation is applied. Standalone
and paired JPEGs use their own EXIF. A preview counts as Adobe RGB when its own EXIF (if it has
colour tags), otherwise the container's EXIF, says ColorSpace = 2, or ColorSpace = 0xFFFF / absent
with InteropIndex `R03`. An embedded ICC profile takes precedence and is converted with moxcms.

### Badge for standalone JPEGs (SPEC gap)

§3 and VIEW-3 list the badges `EMBEDDED`, `EMBEDDED · LOW-RES`, `PAIRED JPEG`, `RAW` and
`NO PREVIEW`, but a standalone JPEG (D-5) is none of them. Calling it `RAW` or `PAIRED JPEG` would
break promise 3 ("never mislead"). So `PreviewSource::Jpeg` shows the badge **`JPEG`**. This is
flagged in the M1 gate report.

### PERF-4 measurement and the budget gap

`req_perf_4_…` builds a deterministic 6.1 MB, 6000×4000, 4:2:0, q90 JPEG, embeds it in a TIFF
container next to a thumbnail, and times locate + choose + full decode on one thread (median of 5).
`cargo xtask bench` runs it in release mode and compares the median with
`perf/baseline-<target>.json` (> 10 % fails) and with the 25 ms budget.

Measured on a 2023 mid-range laptop (Ryzen 7 PRO 7840U), release: **~76 ms**, about 3× the budget.
The time is almost all zune-jpeg's full-resolution decode. Alternatives measured on a 13 MB JPEG:

| Decoder | Time |
|---|---|
| zune-jpeg, full resolution | 150 ms |
| jpeg-decoder, full resolution | 190 ms |
| jpeg-decoder, 1/2 DCT scaling | 170 ms |
| jpeg-decoder, 1/4 DCT scaling | 135 ms |

Entropy decoding dominates, so DCT scaling alone won't reach 25 ms. The budget is reported as
**not met** in the gate report, and CI guards against regressions from the measured baseline.
Options for M2 (PERF-2/3, where held-key speed is actually felt):

- decode previews at screen resolution, in parallel across restart intervals where the JPEG has them;
- use the mid-size embedded preview (e.g. 1616×1080) during held-key navigation and the largest when
  the user stops;
- rely on prefetch, so per-file latency is hidden.

## Consequences

The rawler API churn is contained in `raw.rs`. Pixel decode of RAW data (VIEW-4) adds a
`decode` method to `RawBackend` in M2.
