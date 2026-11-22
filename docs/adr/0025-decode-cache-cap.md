# 0025. PERF-8: the decode cache cap is a hard limit

Status: accepted (2026-11-22)

## Context

PERF-8: "Decode cache cap default 2 GB, configurable; never exceeded in soak". The L2 cache
evicted least-recently-used entries but never the current item's (pinned) entries, so with a
small cap, or a huge full decode, the total could go over.

## Decision

- `Cache::insert` works out room before inserting. It evicts unpinned entries in LRU order,
  then the current item's preview when its full image is cached or arriving (the full image
  supersedes it). If the entry still doesn't fit, it is not cached (logged; only an image
  larger than the whole cache). So `bytes ≤ cap` holds after every insert. A proptest checks
  this over random inserts and pins.
- Config `cache_mb` (MiB, 512 – 1 TiB; default 2048 = `DEFAULT_CACHE_BYTES`). 512 MiB keeps
  the preview and full decode of a 100 MP RAW (7 bytes per pixel in the cache's accounting).
- Soak: `cargo xtask soak --minutes N` runs `full_soak::req_perf_8_soak_…`. That test sends
  random navigation, marks, zoom, filters and undo to the headless app, with a 9 MiB cap over
  40 images, and asserts the cap on every frame. It prints its seed. Nightly runs 10 minutes.
  A 2 s version runs in the fast tier.

## Consequences

The cap covers the decode cache only (as SPEC words it). GPU textures (L1) and images still
drawn hold their own references and are bounded separately by the UI's texture budget.
