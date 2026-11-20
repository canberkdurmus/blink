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
