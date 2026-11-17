---
name: close-requirement
description: Close one Blink requirement ID (or a few tightly coupled IDs) end to end. Branch, failing test first, implement, verify, review, PR, green CI, squash-merge, tick PROGRESS.md. Use for every SPEC §4/§7 ID.
---

# Close a requirement

Arguments: one or more IDs (e.g. `VIEW-2` or `VIEW-2 VIEW-3`). Read their rows in SPEC.md first
(requirement and acceptance column) and any ADRs that mention them.

1. **Branch** from an up-to-date main:
   `git checkout main && git pull --ff-only && git checkout -b req/<ID>-<slug>`
2. **Failing test first.** Write test(s) named `req_<id>_<what>` (ID lowercased, `-` → `_`; see
   ADR 0003). Tests that need the corpus, exiftool, UI goldens or crash injection go in the full tier
   (name or module starts with `full_`). Run them and **see them fail**
   (`cargo nextest run -E 'test(req_<id>)'`), keeping the output as evidence.
3. **Implement** the smallest change that satisfies the SPEC text and the acceptance column.
   Respect the crate boundaries (SPEC §8.2) and the safety invariants (all file access via `Fs`).
   A new non-dev dependency needs an ADR in `docs/adr/`.
4. **Verify**: `cargo xtask verify` (and `cargo xtask verify --full` if you touched full-tier tests)
   until green. Show the command output.
5. **Review**: run the `reviewer` subagent with the diff range `main...HEAD`, the IDs and their
   SPEC text. Fix real gaps only.
6. **Commit** with Conventional Commits and a trailer per ID, e.g.:
   ```
   feat(decode): choose the largest embedded preview

   Req: VIEW-2
   ```
   Push, then `gh pr create --fill` (title = commit subject; body lists the IDs and the tests).
7. **CI**: `gh pr checks --watch --fail-fast`. If red, use the `ci-doctor` subagent, fix, push.
   Never merge red. Platform-dependent IDs need green CI on every applicable target.
8. **Merge**: `gh pr merge --squash --delete-branch`, then `git checkout main && git pull --ff-only`.
9. **Tick** the ID in PROGRESS.md with the test name(s):
   `- [x] VIEW-2 — \`req_view_2_largest_preview_chosen\``. Commit to main
   (`docs(progress): tick VIEW-2`) and push. From then on `cargo xtask trace` requires it.
   If branch protection rejects the push, tick in a tiny PR instead and merge it when CI is green.
