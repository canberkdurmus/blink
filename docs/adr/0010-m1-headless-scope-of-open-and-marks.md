# 0010. M1 headless scope of OPEN-1, OPEN-2 and the CLI mark commands

Date: 2026-11-18 · Status: accepted

## Context

M1 is "Headless core" (SPEC §13). Its gate is "`blink-cli doctor` and `list` correct on the whole
corpus; all M1 IDs traced". Some M1 IDs mention behaviour that needs the GUI or sidecar writing,
and those arrive later:

- OPEN-1 lists three entry points: CLI argument, drag-and-drop, and a dialog (`Ctrl/Cmd+O`). Until
  M2 (VIEW-1), the GUI has no view that shows items.
- OPEN-2's acceptance says "every member gets its own sidecar with the same values". Writing
  sidecars is FILE-4/5/6 in M3.
- CLI-1 lists `rate` and `label`, which write sidecars.

## Decision

1. **OPEN-1 in M1** is the headless open: `Action::OpenFolder` / `SetRecursive` in the reducer, the
   `Fs`-based scan (non-recursive by default, recursive toggle), and the CLI folder argument of
   `blink-cli scan|list|export`. Drag-and-drop, the `Ctrl/Cmd+O` dialog and `blink [folder]`
   dispatch the same `OpenFolder` action. They are wired in M2 together with VIEW-1, when the GUI
   can show the result. The dialog's dependency gets its own ADR then. The M1 gate report flags
   this split.
2. **OPEN-2 in M1**: pairing is complete. A rating sets the value for every member and emits one
   `WriteSidecar` effect per member. The effect carries only the changed field (`MarkChange`), so a
   writer can never clear a label it wasn't asked to change. FILE-4 (M3) executes the effects.
   That is where the "own sidecar" acceptance is checked on disk.
   Note for FILE-4: with the default Adobe style (D-3), `a.ARW` and `a.JPG` both map to `a.xmp`.
   So the two effects target one file, and "its own sidecar" for a JPEG companion needs the
   darktable-style name `a.JPG.xmp` (or FILE-4's rule for that case). Decide this in FILE-4's ADR.
3. **CLI `rate` / `label`** run through the reducer and support `--dry-run`, which prints the
   planned writes. Without `--dry-run` they exit with an error that names M3 (FILE-4..6). This
   way the CLI never writes a half-specified sidecar.
4. **Loading is a merge, not a replace.** Marks are read off the UI thread, so a rating the user
   sets before `MarksLoaded` arrives wins over the file's value. Fields the user didn't touch are
   filled from disk.
5. **Recursive scans don't descend into symlinked directories or junctions.** This keeps link
   loops from making a scan run forever. Symlinked *files* are still listed. On Windows,
   `FileType::is_symlink` reports name-surrogate reparse points (symlinks and junctions).

## Consequences

The M1 gate can be judged headless. M2 must add OPEN-1 tests for the drop and dialog paths. M3
(FILE-4) must cover the pair-sidecar naming case above.
