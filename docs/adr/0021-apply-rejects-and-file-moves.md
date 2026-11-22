# 0021. FILE-1: apply rejects, file moves and the `trash` dependency

Date: 2026-11-22 · Status: accepted

## Context

FILE-1: "Apply rejects" moves rejected items, with all pair members and sidecars, to `_rejected/`
in the same folder (the default) or to the OS trash. A confirmation dialog shows the count. Blink
must never unlink a source file. The acceptance test: the mock FS call log has no unlink of a
source file in any test. RATE-3 says file operations join the undo history (ADR 0016 §4).

## Decision

1. **Fs operations.** `Fs` gains `create_dir`, `move_file` and `trash`. `move_file` never replaces
   a file. If the target exists it fails with `AlreadyExists`. `RealFs` checks the target, then
   renames. std has no portable no-replace rename, so another program creating the same name in
   between would be replaced. Blink itself never does that, and the target is inside
   `_rejected/`, which only Blink writes to. `MemFs` logs `CreateDir`, `MoveFile` and `Trash`.
2. **No unlink, enforced everywhere.** `Fs::remove_file` accepts only a sidecar (an undo that
   takes back a sidecar Blink created) or one of Blink's temp files (`atomic::may_remove`).
   `RealFs` refuses anything else with `PermissionDenied`. `MemFs` panics with "FILE-1 violation",
   so any test whose code path unlinks a source file fails. That covers every test on the mock
   FS, not only the FILE-1 tests. A test checks that the guard fires.
3. **What moves.** All rejected items of the folder (`xmp:Rating = -1`) move, whatever the
   filter shows. Each item moves with every member, then every sidecar in its folder that belongs
   to a member by FILE-4 naming (any case). The reducer passes each member's `owns_adobe_name`.
   A `stem.xmp` shared by unpaired same-stem files therefore stays behind.
4. **Name clashes.** If any target name is taken in `_rejected/`, the whole item gets the first
   suffix `-n` that is free for all of its files (`IMG_1-1.ARW`, `IMG_1-1.ARW.xmp`). The pair and
   its sidecars keep matching names. If one move fails, the item's files that already moved go
   back and the failure is reported. A failed rollback is reported too. Every file still exists
   at one of its two paths.
5. **Durability and ordering.** File operations run on the sidecar I/O thread
   (`SidecarService::file_op`), never on the UI thread. Queued sidecar writes are flushed first,
   then the files move, then every touched directory is fsynced. Before any watcher event for the
   moves is handled, the writer re-keys its session state for each moved sidecar
   (`SidecarWriter::moved`): the baseline that an undo-to-original restores, and the bytes Blink
   last wrote. The watcher then sees the move as Blink's own change, not another app's (FILE-8),
   and undoing a mark after the move is undone still restores the original bytes. Marks are
   ignored, with a toast, while an operation runs. A late debounced write would otherwise
   recreate a sidecar next to a file that just moved.
6. **Dialog.** `Ctrl+Shift+Delete` (`Cmd` on macOS; `Backspace` also works) or the
   "Apply rejects…" button opens a modal: "Move N rejected items to _rejected/?" (or "to the
   trash?"), with Move / Cancel, and Enter / Escape. While it is open, no other shortcut works. With
   no rejected items there is a toast instead of a dialog.
7. **Undo.** A move to `_rejected/` is one history entry (`Edit::Files`, the exact
   `(from, to)` list). Undo moves the files back, newest first, and rescans. Redo makes the same
   moves again. The history stays linear, so mark edits made before the move are undone only after
   the move is undone, when the files are back at the paths those edits recorded. While an
   operation runs, undo, redo and apply rejects wait. Moves that an undo or redo could not make
   (a file took the name meanwhile) stay on their stack, so they can be retried. A result that
   arrives after another folder was opened is not recorded in the new folder's history.
   **Moves to the trash are not undoable in Blink on any platform.** The `trash` crate can't
   restore on macOS. On Linux and Windows its restore API (`os_limited`) has to find the item
   again by original path and deletion time. That is fragile, and it would make the same key
   behave differently per platform. Restoring from the system trash works everywhere. The
   default mode (`_rejected/`) is fully undoable. This is a deviation from RATE-3's "file
   operations" for the opt-in trash mode and is flagged in the M4 gate report.
8. **Config.** `rejects = "folder" | "trash"` in `config.toml`. The default is `folder`.
9. **Dependency: `trash` 5.4.0** (exact pin, default features). The license is MIT, which is
   compatible with MPL-2.0 distribution. It is the maintained 5.x line and the
   crate SPEC §8.4 names. It uses freedesktop trash on Linux, `IFileOperation` on Windows, and
   NSFileManager on macOS. We select NSFileManager explicitly, so there is no Finder automation
   prompt. It makes no network access. New transitive crates are `jiff`, `jiff-core` and
   `jiff-static` (MIT/Unlicense) and `urlencoding` (MIT); `cargo deny check licenses` passes. CI
   builds it on all 6 targets.
10. **Trash only by rename.** On Linux the crate falls back to the home trash when a volume has
    no usable top-directory trash, and it moves across volumes by copy + unlink, with no fsync.
    On Windows a drive without a Recycle Bin (most removable drives) may delete for good.
    `RealFs::trash` therefore refuses, with a message pointing to `rejects = "folder"`, unless
    the move is a rename: on Linux the file must be on the home trash's volume, and on Windows
    the drive root must have `$Recycle.Bin`. macOS's NSFileManager fails instead of deleting.
    Files on a card or USB drive therefore use `_rejected/`.

## Consequences

- The safety layer (SPEC §11.1) now holds by construction on both file systems.
- Cross-volume moves are not needed for `_rejected/` (same folder). FILE-2's "move to a chosen
  folder" reuses `move_file` and decides about other volumes separately.
