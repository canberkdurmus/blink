# Blink — Specification v1.0

Owner: Can · Spec version: 2026-11-17 · Status: approved for autonomous build

This file is **owned by the human**. Claude Code never edits it. If something here is wrong,
contradictory or impossible, record the interpretation in an ADR (`docs/adr/`) and flag it in
the next gate report (see CLAUDE.md).

---

## 0. Decisions already made

| ID | Decision | Value |
|---|---|---|
| D-1 | License | **MPL-2.0** for all Blink code (compatible with LGPL-2.1 rawler, app-store friendly) |
| D-2 | Pick flag | Pick = **1★** by default; configurable 1–5 |
| D-3 | Sidecar style for new sidecars | **Adobe-style** `IMG_0001.xmp`; configurable to darktable-style `IMG_0001.ARW.xmp` |
| D-4 | Pairing | RAW pairs with same-stem **JPEG and HEIF**. HEIF is used only for rating propagation (not decoded in v1). JPEG is also a display fallback |
| D-5 | Standalone files | Standalone JPEG and DNG are first-class items. Standalone HEIF is listed but shows "preview unavailable" in v1 |
| D-6 | Minimum OS | Linux glibc 2.35+ (Ubuntu 22.04 era), X11 and Wayland · macOS 12+ · Windows 10 22H2+ / Windows 11 (incl. ARM64) |
| D-7 | Name / IDs | App `Blink`, binary `blink`, CLI `blink-cli`, app ID `io.github.<owner>.Blink` |

---

## 1. Product thesis

Blink is a free, open-source RAW **culling** tool for Linux, macOS and Windows (x86-64 and ARM64).
One job: get from a folder of ~2,000 RAWs to a trustworthy set of keepers faster than Photo
Mechanic, with FastRawViewer-style honesty about image quality, and hand the decisions to any
editor through standard XMP sidecars.

Three promises, in priority order:

1. **Never lose or corrupt a file.** Blink never writes into RAW or JPEG files, only XMP sidecars.
   It preserves every sidecar field it does not own. It never hard-deletes anything.
2. **Never make the user wait.** Holding "next" shows a new frame on every keypress. Stalls are bugs.
3. **Never mislead.** Blink always shows where the pixels on screen came from (low-res embedded
   preview, full embedded JPEG, paired JPEG, or a real RAW decode).

Non-goals (never build): editing/developing, a catalog database, cloud sync, accounts, telemetry,
plugin marketplace, video.

---

## 2. Scope

**Build only v1.0.** Later rungs are listed so the architecture leaves room. Do not build them.

| Rung | Content |
|---|---|
| **v1.0** (build now) | Everything in §4 |
| v1.1 | Burst/scene grouping by capture-time gap, 2-up/4-up compare with synced zoom, focus peaking, multi-camera time offset |
| v1.2 | Ingest from cards to primary + backup with checksum verify and rename templates |
| v2.0 | Optional local-only ONNX assists (blur score, eyes closed, near-duplicates, face close-ups), advisory only |

Ideas that come up while building go to `docs/ideas.md`, never into code.

---

## 3. Glossary

- **Item**: one shot as the user sees it: a RAW, optionally paired with a JPEG and/or HEIF of the same stem in the same folder. Or a standalone JPEG/DNG/HEIF.
- **Preview source**: where the displayed pixels came from: `EmbeddedLowRes`, `Embedded`, `PairedJpeg`, `RawDecode`, `Unavailable`.
- **LOW-RES**: the best embedded preview's long edge is < 50% of the sensor's long edge (from metadata).
- **Sidecar**: an `.xmp` file next to the source file holding ratings and labels.
- **Owned fields**: the XMP fields Blink writes: `xmp:Rating`, `xmp:Label`, `xmp:MetadataDate`. Everything else is foreign and must be preserved.

---

## 4. Functional requirements (v1.0)

Every requirement has an ID. v1.0 is done when every ID has at least one passing automated test
tagged with that ID (`#[req("ID")]` or the equivalent naming, see §11), on all CI targets where it applies.

### 4.1 Opening

| ID | Requirement | Acceptance |
|---|---|---|
| OPEN-1 | Open a folder via CLI arg, drag-and-drop, or dialog (`Ctrl/Cmd+O`). No import. Non-recursive by default, toggle for recursive | Corpus folder opens; item list equals the directory listing filtered by supported extensions |
| OPEN-2 | Pair same-stem RAW + JPEG/HEIF into one item (D-4). Ratings apply to every member of the pair | Pairing fixture: N pairs show as N items; every member gets its own sidecar with the same values |
| OPEN-3 | Watch the open folder: new files appear, deleted files disappear, renamed files update | Integration test adds, removes and renames files; state reflects each change within 1 s |
| OPEN-4 | Supported formats: every RAW format rawler decodes, plus DNG, JPEG, HEIF (listing only) | `blink-cli doctor` lists format support; corpus covers the major brands (§11.3) |

### 4.2 Viewing

| ID | Requirement | Acceptance |
|---|---|---|
| VIEW-1 | Loupe view, fit to window, embedded preview by default | State snapshot + Linux screenshot golden |
| VIEW-2 | Choose the **largest** embedded preview (cameras like the Sony A1 store several). Mark LOW-RES per §3 | Per-camera golden: chosen preview dimensions ≥ every other embedded image in the file |
| VIEW-3 | Preview-source badge always visible in the loupe: `EMBEDDED`, `EMBEDDED · LOW-RES`, `PAIRED JPEG`, `RAW`, `NO PREVIEW` | Badge state equals the expected value for each corpus file |
| VIEW-4 | Zoom to 100% (key or click at cursor position). If the preview is LOW-RES, use the paired JPEG if present, otherwise decode the RAW in the background, keep showing the upscaled preview with a spinner, then swap in. Same for non-LOW-RES previews smaller than the sensor | Zoom on a low-res ARW ends with source `RawDecode` and pixel dimensions equal to the sensor's active area |
| VIEW-5 | Grid view with 3 thumbnail sizes; filmstrip toggle in the loupe | Screenshot golden; grid-scroll benchmark (PERF-6) |
| VIEW-6 | Clipping overlay (highlights ≥ 254 in any channel → red, shadows ≤ 1 in all channels → blue), and a luminance + RGB histogram. Both computed from the **displayed** image and labelled with its source | Synthetic images with known clipped pixels give exact masks and histograms |
| VIEW-7 | Info panel: filename, body, lens, focal length, shutter, aperture, ISO, exposure compensation, capture time (with offset if known), dimensions, preview source and size | Values match `exiftool -j` on the whole corpus (§11.4) |
| VIEW-8 | Respect EXIF orientation for display only (never writes) | Fixtures covering all 8 orientations |
| VIEW-9 | Pan with drag; zoom levels fit / 50% / 100% / 200%; zoom position kept when moving to the next item (the "check the same eye in each frame" workflow), toggleable | State tests |
| VIEW-10 | Colour: treat display as sRGB. Convert embedded previews tagged Adobe RGB (EXIF ColorSpace / interop index R03) to sRGB | Fixture with an Adobe RGB preview converts within ΔE2000 < 2 of a reference |

### 4.3 Rating and labelling

| ID | Requirement | Acceptance |
|---|---|---|
| RATE-1 | Stars 0–5, reject, colour labels (Red, Yellow, Green, Blue, Purple), each one key. Optional auto-advance after rating | Reducer unit tests; XMP round-trip test |
| RATE-2 | Pick maps to a configurable star value (D-2). Unpick restores 0 only if the rating equals the pick value | Config tests |
| RATE-3 | Unlimited undo/redo within a session for ratings, labels and file operations | Property test: any action sequence followed by undo-all restores every sidecar byte for byte (including absence) |
| RATE-4 | Read existing ratings and labels from sidecars on open; when no sidecar exists, read embedded XMP (DNG/JPEG) for display without writing anything | Fixtures with Lightroom-, darktable-, Bridge- and digiKam-written sidecars |

### 4.4 Filtering and sorting

| ID | Requirement | Acceptance |
|---|---|---|
| FILT-1 | Filters: rating (≥, =), rejected/not rejected, picked, label, body, lens, focal range, ISO range, date range, file type. Combined with AND. Shown as removable chips | Filter results equal expected sets on the corpus |
| FILT-2 | Sort: capture time (default), filename, rating. Stable; ties broken by filename | Sort tests |

### 4.5 Files

| ID | Requirement | Acceptance |
|---|---|---|
| FILE-1 | "Apply rejects" moves rejected items (all pair members and sidecars) to `_rejected/` in the same folder (default) or to the OS trash. Confirmation dialog shows the count. **Never** unlink a source file | Mock FS call log contains no unlink of any source file in any test |
| FILE-2 | Copy or move the current filter's items to a chosen folder; export the item list as `.txt` (paths) or `.csv` (path, rating, label, pick) | Integration tests |
| FILE-3 | Open the current item in an external app from a configurable list (defaults: darktable, RawTherapee, GIMP, system default) | Command construction tested per OS |
| FILE-4 | Sidecar naming: update an existing sidecar (Adobe-style or darktable-style). If both exist, update both. Otherwise create the style from D-3 | Fixture matrix |
| FILE-5 | Preserve foreign content: change only owned fields; all other XML stays semantically identical (same namespaces, elements, attributes, values; whitespace may differ) | Property test with generated foreign XMP |
| FILE-6 | Atomic writes: temp file in the same directory → fsync → rename (on Windows use `ReplaceFileW` semantics). Debounce rating writes ≤ 250 ms | Crash-injection test: failure at every step leaves the old or the new sidecar, never a corrupt or missing one |
| FILE-7 | Never modify RAW/JPEG/HEIF bytes | Hash every source file before and after full test sessions |
| FILE-8 | External changes: if a sidecar changes on disk, reload it. If Blink also has unsaved changes for it, keep the on-disk version, show a conflict marker on the item, and log it | Integration test |
| FILE-9 | Write `xmp:MetadataDate` (ISO 8601 with offset) whenever Blink changes a sidecar, so Lightroom notices external changes | XMP tests |

### 4.6 Keyboard, sessions, CLI, accessibility, platforms

| ID | Requirement | Acceptance |
|---|---|---|
| KEY-1 | Every action is remappable via a TOML keymap. Presets ship: `blink` (default, §5.2), `lightroom`, `photo-mechanic` (agent researches the vendors' documented defaults and cites sources in an ADR) | Keymaps parse; static check finds no conflicting bindings |
| SESS-1 | Relaunch restores the last folder, current item, zoom mode and filters | Kill-and-relaunch test |
| CLI-1 | `blink [folder]` opens the GUI. `blink-cli` runs headless over the same core: `scan`, `list --filter`, `rate`, `label`, `export`, `doctor` | Golden output tests |
| CLI-2 | `blink-cli doctor <file>` prints format, camera, every embedded image found (type, size, offset), chosen preview, LOW-RES verdict, decode result and timing | Golden output; used in bug reports |
| A11Y-1 | Full keyboard operation; high-contrast theme; UI exposed via AccessKit | UI tests query the accessibility tree |
| PLAT-1 | Linux x86-64 and ARM64 (X11 and Wayland), macOS ARM64 and x86-64, Windows x86-64 and ARM64. HiDPI and fractional scaling | Full test suite green on all CI targets (§12) |
| PLAT-2 | Config and caches in platform-standard locations (`directories` crate). Portable mode if a `blink-portable` file sits next to the binary | Unit tests |

---

## 5. UI specification

### 5.1 Layout

- Dark neutral-grey theme by default (UI chrome ≈ 18% grey or darker, no tinted colours behind photos). High-contrast theme. No animation longer than 100 ms.
- **Top bar**: folder path, item counter (`312 / 2,041`), active filter chips, sort selector.
- **Centre**: loupe or grid.
- **Loupe overlay** (top-left): preview-source badge (VIEW-3), rating stars, pick/reject marker, colour label, conflict marker when present.
- **Bottom**: filmstrip (toggle).
- **Right panel** (toggle): info (VIEW-7) and histogram (VIEW-6).
- Toast area for non-blocking messages (e.g. "Decoding RAW…", "Sidecar changed on disk").
- Dialogs only for: open folder, apply rejects confirmation, copy/move destination, preferences.

### 5.2 Default keymap (`blink` preset)

| Action | Key |
|---|---|
| Next / previous item | `→` / `←`, also `Space` / `Shift+Space` |
| First / last | `Home` / `End` |
| Grid / loupe | `G` / `E` or `Enter` |
| Zoom 100% toggle (at cursor) | `Z` |
| Fit | `F` |
| Stars 0–5 | `0`–`5` |
| Pick / reject / unflag | `P` / `X` / `U` |
| Labels red, yellow, green, blue, purple | `6`, `7`, `8`, `9`, `Shift+9` |
| Toggle auto-advance | `Shift+A` |
| Clipping overlay | `J` |
| Info panel | `I` |
| Filmstrip | `T` |
| Fullscreen | `F11` (macOS: `Ctrl+Cmd+F`) |
| Undo / redo | `Ctrl+Z` / `Ctrl+Shift+Z` (macOS `Cmd`) |
| Open folder | `Ctrl+O` |
| Apply rejects | `Ctrl+Shift+Delete` (always confirms) |
| Open in external app | `Ctrl+Shift+E` |
| Filter by rating ≥ n | `Alt+0`…`Alt+5` |

---

## 6. Interop contract (XMP)

| Blink concept | Field | Notes |
|---|---|---|
| Stars 1–5 | `xmp:Rating` 1–5 | XMP spec: −1 rejected, 0 unrated, 1–5 stars |
| Reject | `xmp:Rating="-1"` | Honoured by darktable and Bridge. Lightroom keeps its reject flag in its catalog, not XMP. The README must say so |
| Pick | `xmp:Rating` = pick value (D-2) | No portable pick field exists |
| Colour label | `xmp:Label` = `Red`/`Yellow`/`Green`/`Blue`/`Purple` | Names configurable for apps that differ |
| Change time | `xmp:MetadataDate` | FILE-9 |

Namespace `xmp` = `http://ns.adobe.com/xap/1.0/`. New sidecars are minimal and valid:
`x:xmpmeta` → `rdf:RDF` → one `rdf:Description` with the owned fields as attributes.
When editing an existing sidecar, keep the original form (attribute vs element) of each field.

Interop oracles: exiftool (every commit, Linux), darktable and digiKam imports (nightly, Linux).

---

## 7. Performance budgets

Absolute budgets are for a mid-range 2023 laptop on local NVMe with 24–45 MP files. They are
checked at gates on the owner's machine. CI compares against stored per-target baselines and fails
on a regression > 10% (median of 5 runs).

| ID | Metric | Budget |
|---|---|---|
| PERF-1 | Launch → first image displayed (2,000 RAWs, cold cache) | ≤ 400 ms |
| PERF-2 | Next item displayed, prefetched, embedded preview | ≤ 16 ms |
| PERF-3 | Held arrow key, sustained | ≥ 30 items/s, no frame shows a stale item |
| PERF-4 | Embedded preview extract + decode, per file (worker thread) | ≤ 25 ms median for a 6 MB JPEG |
| PERF-5 | Full RAW decode for 100% zoom, 45 MP | ≤ 1.5 s; no UI frame > 33 ms meanwhile |
| PERF-6 | Grid of 2,000 thumbnails, cold | Visible ones ≤ 1 s, all ≤ 10 s |
| PERF-7 | Rating key → sidecar durable on disk | ≤ 250 ms |
| PERF-8 | Memory | Decode cache cap default 2 GB, configurable; never exceeded in soak |
| PERF-9 | Slow storage (USB/network) | Prefetch window adapts to measured throughput; UI thread never blocks on I/O |

Measurement: the runtime emits timestamped trace events (feature `trace`). `blink --bench-script <file>`
replays an action script against a folder and writes JSON results. `cargo xtask bench` runs the
standard scripts and compares with `perf/baseline-<target>.json`.

---

## 8. Architecture

### 8.1 Pattern

`Action` → pure `reduce(&State, Action) -> (State, Vec<Effect>)` → the runtime executes effects
(read, decode, write sidecar, move file) and feeds results back as actions. The file system and the
clock sit behind traits (`Fs`, `Clock`), so tests use a recording mock FS and a fake clock. Any
session can be recorded as an action log and replayed deterministically.

### 8.2 Crates

| Crate | Responsibility | I/O |
|---|---|---|
| `blink-core` | State, actions, reducer, filters, sort, keymap resolution, undo stack | None. `#![forbid(unsafe_code)]` |
| `blink-xmp` | Parse and write sidecars, preserving foreign content | None (bytes in/out). `#![forbid(unsafe_code)]` |
| `blink-decode` | Locate/extract embedded previews, RAW decode, JPEG decode, orientation, colour conversion, histogram, clipping | Read only |
| `blink-runtime` | Effect executor, worker pools, prefetch scheduler, caches, watcher, atomic writes, trash/move | Yes, via `Fs` |
| `blink-ui` | egui views, theming, input mapping | Through runtime only |
| `blink` (bin) | GUI entry point | — |
| `blink-cli` (bin) | Headless commands | Through runtime |
| `xtask` | `verify`, `corpus`, `bench`, `trace`, `report`, `release` | Dev only |

### 8.3 Threads and caches

- UI thread renders only. Decode pool = physical cores − 1 (min 1). Separate I/O pool.
- Prefetch window: 8 ahead and 2 behind in the travel direction; flips on direction change. Every job carries a cancellation token; jumping cancels stale jobs.
- During fast navigation (held key), decode embedded previews at screen resolution only (DCT scaling where the JPEG decoder supports it).
- Cache tiers: L1 GPU textures (count-limited), L2 decoded RGBA (byte-limited, PERF-8), L3 on-disk thumbnails in the platform cache dir, keyed by hash(path, size, mtime, cache-version).
- Large images are uploaded as tiles (≤ 4096 px) to stay under GPU texture limits.

### 8.4 Technology

| Need | Choice | Notes |
|---|---|---|
| Language | Rust stable, MSRV pinned in `rust-toolchain.toml` | |
| GUI | egui + eframe on wgpu | Use `egui_kittest` for UI tests if it fits. Otherwise state + AccessKit tests. Record the choice in an ADR |
| RAW | `rawler` (LGPL-2.1), exact version pin, wrapped behind a `RawBackend` trait | API is not SemVer-stable |
| JPEG | `zune-jpeg` | Pure Rust; avoids C builds on Windows ARM |
| XMP | `quick-xml` with an event-preserving rewrite | FILE-5 |
| Colour | Pure-Rust CMS (evaluate `moxcms`; `lcms2` as fallback with an ADR) | VIEW-10 |
| Watch / trash / dirs | `notify`, `trash`, `directories` | |
| Dialogs | `rfd` | |

**Dependency policy**: every new non-dev dependency needs an ADR covering license (must be compatible
with MPL-2.0 distribution), maintenance status, and whether it builds on all 6 targets. No dependency
may add network access, telemetry or an auto-updater.

---

## 9. Packaging and release

| Platform | Artifacts |
|---|---|
| Linux | AppImage (x86-64, ARM64; built on the oldest supported glibc), Flatpak manifest, `.deb` |
| macOS | Universal `.dmg` (unsigned until the owner provides signing credentials) |
| Windows | `.msi` for x64 and ARM64 (unsigned until credentials exist), portable `.zip` |

Releases are built by CI from tags `v*`. Pre-releases (`v1.0.0-mN`) are published at each milestone
gate. The final `v1.0.0` tag is made by the owner.

---

## 10. Repository files that drive the process

| Path | Purpose |
|---|---|
| `CLAUDE.md` | How Claude Code works on this repo (owner-provided; Claude may append to its "Learned" section only) |
| `SPEC.md` | This file. Owner-owned |
| `PROGRESS.md` | Current milestone, per-milestone ID checklists, "Next up", session log |
| `BLOCKED.md` | Items that failed 3 attempts: what was tried, current hypothesis |
| `HUMAN_TODO.md` | Non-blocking things the owner should do (e.g. enable branch protection, buy signing certs) |
| `docs/adr/NNNN-title.md` | Decision records (context, decision, alternatives, consequences) |
| `docs/reports/Mn.md` | Gate reports |
| `docs/ideas.md` | Out-of-scope ideas |
| `.autopilot/NEEDS_HUMAN.md` | Blocking request; the launcher stops (git-ignored) |
| `.autopilot/DONE` | Final gate reached; the launcher stops (git-ignored) |

---

## 11. Testing strategy

### 11.1 Layers

| Layer | Proves | Tool | Runs |
|---|---|---|---|
| Unit / reducer | Logic of every action, filter, sort, keymap | `cargo nextest` | Every PR |
| Property | FILE-5, RATE-3, filter algebra | `proptest` | Every PR |
| Corpus goldens | Per file: chosen preview, badge, EXIF, decode dimensions, decode hash | Manifest + fixtures | Every PR (cached corpus) |
| Oracle | EXIF and written XMP agree with exiftool | `exiftool -j` | Every PR (Linux) |
| UI | Scripted actions → accessibility queries + screenshot goldens | `egui_kittest` (or fallback), software rendering | Every PR. Pixel goldens Linux only |
| Crash injection | FILE-6 | Failing mock FS | Every PR |
| Safety | FILE-1, FILE-7 | Mock FS call log + source hashes | Every PR |
| Soak | 10 min random actions: no panic, memory cap, invariants | Replay harness | Nightly |
| Fuzz | XMP parser, container glue | `cargo-fuzz` | Nightly, 20 min per target |
| Mutation | Tests catch broken logic in core and xmp | `cargo-mutants` | Weekly; surviving mutants become issues |
| Performance | PERF-* | criterion + bench scripts | Every PR (Linux), nightly (all) |
| Traceability | Every ID in §4 and §7 has ≥ 1 passing tagged test | `cargo xtask trace` | Every PR |

### 11.2 Requirement tagging

Tests carry the ID in their name (`fn req_view_2_largest_preview_chosen()`) or through an attribute
macro provided by a small dev crate. `cargo xtask trace` parses test names and nextest JUnit output
and fails if any ID lacks a passing test.

### 11.3 Corpus

- Source: raw.pixls.us. Use only files whose license permits redistribution in CI caches (prefer CC0). Record each file's license in the manifest.
- Coverage: at least Sony (one old body with a ~1616×1080 preview, plus A1 or another body with two previews), Canon CR2 and CR3, Nikon NEF (incl. a Z body), Fujifilm RAF X-Trans, OM/Olympus ORF, Panasonic RW2, Pentax PEF, Leica/phone DNG.
- `corpus/manifest.toml`: `path`, `sha256`, `url`, `license`, `camera`, expected `{preview_w, preview_h, badge, sensor_w, sensor_h, exif.*}`. Files are fetched by `cargo xtask corpus fetch` into a git-ignored cache and cached in CI.
- Synthetic fixtures generated by `xtask`: the 8 orientations, RAW+JPEG/HEIF pairs, truncated/corrupt files, sidecars in Lightroom/darktable/Bridge/digiKam styles, random foreign XMP.

### 11.4 Expected values

Expected EXIF values are bootstrapped from exiftool output, then frozen in the manifest. Changing a
frozen value requires a `golden-update:` commit trailer with a reason.

---

## 12. CI/CD

GitHub Actions on hosted runners. Before writing workflows, check GitHub's current runner labels.
Targets: Linux x86-64, Linux ARM64, macOS ARM64, macOS x86-64, Windows x86-64, Windows ARM64. If no
hosted Intel macOS runner exists, cross-compile `x86_64-apple-darwin` on ARM macOS and run tests
under Rosetta 2. If that is impossible, run build-only and note it in `HUMAN_TODO.md`.

Workflows:

- `ci.yml` (PRs and `main`): fmt, clippy `-D warnings`, nextest, trace, UI tests, perf compare (Linux), corpus cache. Full 6-target matrix.
- `nightly.yml`: soak, fuzz, darktable/digiKam interop, perf on all targets. On failure it opens or updates an issue labelled `nightly-failure`.
- `weekly.yml`: cargo-mutants, `cargo deny` (licenses/advisories), dependency update PR.
- `release.yml`: on tag `v*`: build all artifacts (§9), create a GitHub Release (pre-release for `-mN`).

Guard checks in `ci.yml`:

- Changes under `corpus/manifest.toml`, `**/snapshots/**` or `perf/baseline-*` fail the build unless a commit in the PR has a `golden-update:` trailer.
- Removed or `#[ignore]`d tests fail the build unless a commit has a `test-fix:` trailer.

---

## 13. Milestones and gates

Each milestone closes only when its gate passes. The gate report `docs/reports/Mn.md` contains: IDs
closed (with test names), CI run links, screenshots from UI tests, perf table vs budget with raw
numbers, open `BLOCKED.md` items, ADRs added, and download links for the pre-release.

| Milestone | IDs | Gate |
|---|---|---|
| **M0 Foundations** | — (infrastructure) | Workspace, xtask (`verify`, `trace`, `corpus fetch`), CI on 6 targets green with an empty window app, nightly/weekly/release workflows present, PROGRESS.md and `.claude/` setup done. `v0.0.1-m0` pre-release builds on all targets |
| **M1 Headless core** | OPEN-1, OPEN-2, OPEN-4, VIEW-2, VIEW-3, VIEW-7, VIEW-8, VIEW-10, RATE-4, CLI-1, CLI-2, PLAT-2, PERF-4 | `blink-cli doctor` and `list` correct on the whole corpus; all M1 IDs traced |
| **M2 Loupe and speed** | VIEW-1, VIEW-4, VIEW-6, VIEW-9, PERF-1, PERF-2, PERF-3, PERF-5 | All M2 IDs traced; perf within budget on CI baselines; report asks the owner to confirm absolute budgets on his laptop (non-blocking, via HUMAN_TODO) |
| **M3 Ratings and sidecars** | RATE-1, RATE-2, RATE-3, FILE-4, FILE-5, FILE-6, FILE-7, FILE-8, FILE-9, PERF-7 | Property, crash-injection and exiftool oracle tests green; nightly darktable interop green twice in a row |
| **M4 Grid, filters, files** | VIEW-5, FILT-1, FILT-2, FILE-1, FILE-2, FILE-3, OPEN-3, PERF-6, PERF-8, PERF-9 | Safety log shows zero source-file writes across the suite; all M4 IDs traced |
| **M5 Release candidate** | KEY-1, SESS-1, A11Y-1, PLAT-1, packaging (§9), README, user guide | Every ID in §4 and §7 traced on all targets; 7 consecutive green nightlies; `v1.0.0-rc.1` published; then create `.autopilot/DONE` |

Within a milestone, work order is free, but follow dependencies (e.g. VIEW-4 needs VIEW-2).

---

## 14. Definition of done for v1.0.0-rc.1

- Every requirement ID traced and green on every applicable target.
- Perf baselines recorded for all targets; CI-measured budgets met.
- README (install, supported cameras, XMP interop table incl. the Lightroom reject caveat, keymaps), CONTRIBUTING, CHANGELOG, LICENSE (MPL-2.0), third-party license notices generated by `cargo about` or equivalent.
- No open `nightly-failure` issues; `BLOCKED.md` empty or every entry explicitly deferred to v1.1 with an ADR.
