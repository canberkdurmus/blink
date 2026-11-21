# 0017. Nightly interop oracle: darktable via its Lua API; digiKam deferred

Date: 2026-11-21 · Status: accepted

## Context

SPEC §6 names exiftool (every commit) and darktable and digiKam imports (nightly) as interop
oracles. The M3 gate needs "nightly darktable interop green twice in a row". `darktable-cli` is
no oracle for marks: it applies only the history from an XMP and exports rating 0 whatever the
sidecar says (checked by hand with darktable 5.x). digiKam has no headless import or query
interface: its CLI tools only convert or batch-process.

## Decision

- `cargo xtask interop` drives **darktable itself** under `xvfb-run`, with
  `--library :memory:` (no state carried between runs) and
  `--luacmd "dofile('tests/interop/darktable_probe.lua')"`. The probe imports each file
  (`darktable.database.import`) and writes the rating and colour labels darktable read from
  the sidecar, then quits. Cases: a new darktable-style sidecar, a darktable-written sidecar
  updated by Blink and re-imported (its history must survive), a reject, and Adobe-style and
  darktable-style sidecars that are both present and both updated. Input is a corpus RAW
  (`canon/EOS-20D.CR2`).
- The nightly `interop` job caches and fetches the corpus, installs `darktable xvfb xauth`,
  runs the task and prints `target/interop/report.txt`.
- **digiKam is deferred.** Without a headless import there is no oracle short of scripting its
  GUI. Its sidecar dialect is covered by the RATE-4 digiKam fixture, and the exiftool oracle
  (an Exiv2-compatible reader) covers what Blink writes. The job no longer installs digiKam. If
  a scriptable digiKam path turns up, it goes into `docs/ideas.md` and gets its own ADR.

## Consequences

The gate condition "nightly darktable interop green twice in a row" is checked on `nightly.yml`
runs (scheduled or `workflow_dispatch`). The M3 report lists both run links.
