# Human TODO

Non-blocking things for the owner. Newest at the bottom.

- [ ] Settings → Actions → General → Workflow permissions: tick "Allow GitHub Actions to create and
  approve pull requests". The weekly dependency-update job needs it. Until then the agent opens that
  PR by hand from the `deps/weekly-cargo-update` branch.
- [ ] Enable branch protection on `main` requiring these checks: `Test linux-x86_64`,
  `Test linux-arm64`, `Test macos-arm64`, `Test macos-x86_64`, `Test windows-x86_64`,
  `Test windows-arm64`, `Guard (goldens, removed tests)`, `Perf compare (Linux)`. Leave
  "Require a pull request" on, but allow squash merges by the repo owner account (the agent merges its own PRs).
- Confirm PERF-4's reading in ADR 0011: one preview decode may use all cores. See docs/reports/M1.md.
- [ ] M2: confirm the absolute PERF-1/2/3/5 budgets on your laptop (SPEC §13 M2 gate):
  `cargo xtask corpus fetch && cargo xtask bench`. The numbers the agent measured (Ryzen 7 PRO
  7840U, headless, no GPU upload) are in docs/reports/M2.md. Also check by eye that zooming to
  100 % on a 45 MP RAW (Z, after the decode) doesn't stutter: the headless bench can't time the
  GPU upload of the full-resolution tiles (ADR 0014).
- [ ] The build machine's disk ran full on 2026-11-22: 870 GB of 937 GB used, about 90 GB of it
  Blink's `target/` dirs. The agent now removes merged worktrees, but only about 20 GB is free.
  Please free space outside the repo, or give the agent a larger disk.
