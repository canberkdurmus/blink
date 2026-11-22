# 0023. FILE-3: open the current item in an external app

Status: accepted (2026-11-22)

## Context

FILE-3 asks for "Open the current item in an external app from a configurable list (defaults:
darktable, RawTherapee, GIMP, system default)", accepted by "command construction tested per OS".
SPEC §5.2 binds `Ctrl+Shift+E`. SPEC doesn't say which app the key opens, how the list is
configured, or how Windows programs are found.

## Decision

1. **Model** (`blink_core::external`, no I/O). An app has a name, an ordered list of program
   candidates and an argument list. An argument equal to `{file}` becomes the item's primary path,
   otherwise the path is appended. The path is always one argv element and no shell runs, so a
   file name (`&`, spaces, quotes) can't inject a command. This is why Windows uses no `cmd /c start`.
2. **Defaults per OS.**
   - Linux: `darktable`, `rawtherapee`, `gimp`, `xdg-open` on `PATH`.
   - macOS: `open -a darktable|RawTherapee|GIMP <file>` and `open <file>`.
   - Windows: the standard install paths, then the bare name on `PATH`:
     - `C:\Program Files\darktable\bin\darktable.exe`
     - `C:\Program Files\RawTherapee\*\rawtherapee.exe` (version folder; the highest version
       wins by numeric-aware order)
     - `GIMP 3\bin\gimp-3.exe`, `GIMP 3\bin\gimp-3.0.exe`, `GIMP 2\bin\gimp-2.10.exe`
     - `explorer.exe <file>` for the system default

   Path candidates are checked through `Fs` (read-only), off the UI thread.
3. **Config.** `[[open_with]]` tables with `name` and `command = [program, args…]` replace the
   default list. A broken entry is reported in the startup toast and skipped. If every entry is
   broken, the defaults are kept.
4. **UI.** `Ctrl+Shift+E` opens the current item in the first app of the list, and an "Open in"
   menu lists them all. A launch result or error shows in the toast, and errors also go to the
   log. `ProcessLauncher` spawns with null stdio. It watches the child for 1.5 s, so a launcher that
   fails at once (`open -a` without the app, `xdg-open` without a handler) is reported as an error.
   `explorer.exe` is excepted because it exits 1 even on success. A child still running after that
   is reaped on a thread. Tests use a
   recording `Launcher`.

## Consequences

- No new dependency. Launching an app writes nothing, so the safety invariants are untouched.
- A missing default app gives "Could not open in X: …" rather than a hidden failure.
- The Windows defaults can't be checked by CI against real installs. They are path strings tested
  as constructed, and users with other install folders configure `[[open_with]]`. Flag in the M4
  report.
