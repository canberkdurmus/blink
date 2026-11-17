# 0003. Requirement tagging, trace semantics and test tiers

Date: 2026-11-17 · Status: accepted

## Context

SPEC §11.2 says tests carry the requirement ID in their name or via an attribute macro, and
`cargo xtask trace` fails if any ID lacks a passing test. Taken literally, trace would fail on every
commit until v1.0 is complete, so `main` could never be green. CLAUDE.md also separates a fast tier
(`verify`) and a full tier (`verify --full`: UI goldens, corpus, oracle, crash injection).

## Decision

- **Naming, no macro.** A test is tagged by its function name: `req_<id>` with the ID lowercased and
  `-` → `_` (`OPEN-1` → `req_open_1_…`, `A11Y-1` → `req_a11y_1_…`). One test may carry several IDs
  (`req_view_2_req_view_3_…`). This needs no proc-macro crate and works with `proptest!`.
- **Required set.** Trace requires a tagged, passing test for: every ID of milestones before
  `Current milestone:` in PROGRESS.md, every ID ticked `- [x]` in PROGRESS.md, and with
  `trace --gate Mn` every ID of milestone Mn. That is the gate check. It reports coverage of all
  §4/§7 IDs every time.
- **Evidence.** Static: the source tree must contain a `fn req_<id>…`. Dynamic: the nextest JUnit
  report of the last run (`target/nextest/<profile>/junit.xml`) must show a passing test and no
  failing test for the ID. The fast tier (`verify`) checks static coverage and failures. The full
  tier (`verify --full`, standalone `trace`) also requires a pass in the last run.
- **Tiers.** Full-tier tests (corpus goldens, oracle, UI pixel goldens, crash injection) have names
  or modules starting with `full_`. The fast tier excludes them with the nextest filter
  `not test(/(^|::)full_/)`. CI runs the full tier on every PR, as SPEC §11.1 requires.

## Alternatives

- An attribute macro crate `#[req("ID")]`: more machinery, and tests still need a unique name.
- Trace over all IDs from day one: permanently red `main`.

## Consequences

Ticking an ID in PROGRESS.md makes it required, so a later regression fails trace. Gate reports
use `trace --gate Mn`.
