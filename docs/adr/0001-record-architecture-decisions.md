# 0001. Record architecture decisions

Date: 2026-11-17 · Status: accepted

## Context

Blink is built by an unattended agent. The owner doesn't review code, so decisions,
interpretations of the spec and dependency choices must be recorded where future sessions and
the owner can find them.

## Decision

We keep Architecture Decision Records in `docs/adr/NNNN-title.md` (numbered, never renumbered).
Each has: Context, Decision, Alternatives, Consequences. Superseded ADRs stay and are marked
`Status: superseded by NNNN`. Every new non-dev dependency, every `unsafe` outside the no-unsafe
crates, and every spec ambiguity gets an ADR.

## Alternatives

- Decisions only in commit messages: hard to find, no structure.
- A single DECISIONS.md: grows into a merge-conflict magnet.

## Consequences

Gate reports list the ADRs added during the milestone.
