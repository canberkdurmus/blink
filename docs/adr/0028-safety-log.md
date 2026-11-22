# 0028. The M4 safety log

Status: accepted (2026-11-22)

## Context

The M4 gate (SPEC §13) asks for a "safety log" showing zero source-file writes across the suite.
Each test already checks its own case:
- MemFs fails any unlink of a source file (FILE-1).
- The FILE-7 tests hash the source files before and after.

Nothing yet adds these up across the whole suite.

## Decision

- **Logging.** When `BLINK_SAFETY_LOG` names a directory, `RealFs` and `MemFs` append one line
  per write call to a file in it (`blink_runtime::safety`). Each line holds the call, the
  target's class, the result and the path. The classes are:
  - `sidecar`
  - Blink `temp` file
  - export `list`
  - `source`: anything else, i.e. a file Blink doesn't own.
  The variable is read once. Without it, nothing is logged.
- **Source-file write.** A successful `rename` onto a `source` (a replace) or a successful
  `remove_file` of a `source`. Only these calls can change or remove an existing file's bytes.
  The other calls are logged and counted but are not violations:
  - `write_new` only creates a file and fails if the file exists.
  - `move_file` and `trash` move a file. They are the FILE-1/2 operations and change no bytes.
  - `copy_file` creates a new file and never replaces one.
  Refused attempts (tests that prove the refusal) are counted as refused.
- **Gate check.** `cargo xtask verify --full` runs the suite with the variable set, so CI runs it
  on all 6 targets. It then writes the table to `target/safety-log.md` and fails on any
  violation or on an empty log. `cargo xtask report` copies the table into the gate report.

## Consequences

The log covers every test that goes through `Fs`. Fixture setup writes with `std::fs`, outside
Blink's code paths, and is not logged.
