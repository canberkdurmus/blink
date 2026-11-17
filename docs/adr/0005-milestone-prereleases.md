# 0005. Milestone pre-release tags and M0–M4 artifacts

Date: 2026-11-17 · Status: accepted

## Context

CLAUDE.md and the M0 gate name the first pre-release `v0.0.1-m0`. SPEC §9 names milestone
pre-releases `v1.0.0-mN`. Full packaging (AppImage, Flatpak, .deb, .dmg, .msi) belongs to M5.

## Decision

- M0 is tagged `v0.0.1-m0` as the gate says. M1–M4 are tagged `v1.0.0-m1` … `v1.0.0-m4` per SPEC §9.
  M5 publishes `v1.0.0-rc.1`.
- Until M5, `release.yml` builds `blink` and `blink-cli` in release mode on all 6 targets and
  publishes them as `.tar.gz` (Linux/macOS) or `.zip` (Windows) archives with LICENSE. Tags with a
  `-` suffix are published as GitHub pre-releases.

## Alternatives

Building every installer format from M0: it is M5 scope, and doing it now would only add risk.

## Consequences

`release.yml` grows the §9 artifacts in M5.
