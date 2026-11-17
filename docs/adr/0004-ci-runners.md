# 0004. CI runner labels for the 6 targets

Date: 2026-11-17 · Status: accepted

## Context

SPEC §12 asks to check GitHub's current hosted runner labels before writing workflows. Checked on
2026-11-17 at docs.github.com "GitHub-hosted runners" (standard runners for public repositories).

## Decision

| Target | Label | Notes |
|---|---|---|
| Linux x86-64 | `ubuntu-24.04` | glibc 2.39; `ubuntu-latest` moves to 26.04 in Nov 2026, so we pin |
| Linux ARM64 | `ubuntu-24.04-arm` | Free for public repos |
| macOS ARM64 | `macos-15` | Apple silicon |
| macOS x86-64 | `macos-15-intel` | A hosted Intel runner exists, so no Rosetta cross-test is needed |
| Windows x86-64 | `windows-2025` | |
| Windows ARM64 | `windows-11-arm` | Rust 1.100 preinstalled; we still install the pinned toolchain |

Release builds of the Linux AppImage on the oldest supported glibc (D-6: 2.35) will use
`ubuntu-22.04` / `ubuntu-22.04-arm` in M5. Those images are being deprecated, so M5 may need a
container instead. That will be decided then.

## Alternatives

Floating `*-latest` labels: they change under us and break reproducibility.

## Consequences

Labels are pinned. When GitHub retires one, CI fails loudly and we update this ADR.
