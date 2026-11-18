# Progress

Current milestone: M1

Ticked IDs list their tests. `cargo xtask trace` requires every ticked ID and every ID of earlier
milestones to have a tagged, passing test.

## Next up

M1 Headless core. Start with the corpus, which unblocks almost every M1 ID: pick redistributable
(CC0) files from raw.pixls.us covering SPEC §11.3, fill `corpus/manifest.toml` (bootstrap the expected
values from exiftool), and add the synthetic fixture generator to xtask. Then, in dependency order:
PLAT-2 (dirs/portable) → OPEN-4 + rawler `RawBackend` (ADR for rawler, zune-jpeg) → OPEN-1/OPEN-2
(`Fs` trait + scan/pairing in runtime, pure pairing logic in core) → VIEW-2/VIEW-3/VIEW-8/VIEW-7 →
VIEW-10 (moxcms ADR) → RATE-4 (blink-xmp read) → CLI-1/CLI-2 → PERF-4.

## M0 Foundations (infrastructure)

- [x] Workspace and crate stubs, empty egui window
- [x] xtask: verify, trace, corpus fetch, bench, report, guard
- [x] PROGRESS.md, BLOCKED.md, HUMAN_TODO.md, ADR 0001, docs/ideas.md
- [x] Workflows: ci.yml (6 targets), nightly.yml, weekly.yml, release.yml
- [x] `.claude/` settings, hooks, agents, skill
- [x] CI green on all 6 targets (run 44182943822)
- [x] `v0.0.1-m0` pre-release built on all targets
- [x] Gate report docs/reports/M0.md

## M1 Headless core

- [ ] OPEN-1
- [ ] OPEN-2
- [ ] OPEN-4
- [ ] VIEW-2
- [ ] VIEW-3
- [ ] VIEW-7
- [ ] VIEW-8
- [ ] VIEW-10
- [ ] RATE-4
- [ ] CLI-1
- [ ] CLI-2
- [x] PLAT-2 — `req_plat_2_portable_marker_puts_config_and_cache_next_to_binary`, `req_plat_2_without_marker_uses_platform_standard_dirs`, `req_plat_2_marker_must_be_a_file_next_to_the_binary` (#5)
- [ ] PERF-4

## M2 Loupe and speed

- [ ] VIEW-1
- [ ] VIEW-4
- [ ] VIEW-6
- [ ] VIEW-9
- [ ] PERF-1
- [ ] PERF-2
- [ ] PERF-3
- [ ] PERF-5

## M3 Ratings and sidecars

- [ ] RATE-1
- [ ] RATE-2
- [ ] RATE-3
- [ ] FILE-4
- [ ] FILE-5
- [ ] FILE-6
- [ ] FILE-7
- [ ] FILE-8
- [ ] FILE-9
- [ ] PERF-7

## M4 Grid, filters, files

- [ ] VIEW-5
- [ ] FILT-1
- [ ] FILT-2
- [ ] FILE-1
- [ ] FILE-2
- [ ] FILE-3
- [ ] OPEN-3
- [ ] PERF-6
- [ ] PERF-8
- [ ] PERF-9

## M5 Release candidate

- [ ] KEY-1
- [ ] SESS-1
- [ ] A11Y-1
- [ ] PLAT-1
- [ ] Packaging (SPEC §9)
- [ ] README
- [ ] User guide

## Session log

- 2026-11-17 · M0 closed · workspace, xtask, 4 workflows green on 6 targets, .claude setup, v0.0.1-m0, ADRs 0001–0005
