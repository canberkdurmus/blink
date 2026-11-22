# 0022. FILE-2: copy or move the filtered items, export the item list

Date: 2026-11-22 · Status: accepted

## Context

FILE-2: copy or move the current filter's items to a chosen folder, and export the item list as
`.txt` (paths) or `.csv` (path, rating, label, pick). Acceptance: integration tests.

## Decision

1. **Which items.** The visible items (the filter applied, in sort order). Each item goes with
   every member and every sidecar that belongs to it, as in ADR 0021 §3.
2. **Copy.** `Fs::copy_file` never replaces a file. `RealFs` copies into a Blink temp file next
   to the target, fsyncs it, then moves it into place with the no-replace `move_file`. A
   crash or a full disk therefore never leaves a partial file under the real name. Name clashes
   in the destination get one free `-n` suffix for the whole item (ADR 0021 §4). A copy changes
   nothing in the open folder and does not enter the undo history. If an item's copy fails
   partway, the copies already made stay and the failure is reported. Blink never unlinks a
   photo, not even its own copy.
3. **Move.** `move_file`, the same as apply rejects, with rollback of a half-moved item. It
   enters the undo history (RATE-3), and the moved items leave the list. A destination on
   another volume fails with "the destination is on another volume; copy instead". A
   cross-volume move would need copy + unlink of the source, and the SPEC forbids that.
   Moving into the item's own folder, or into a path that isn't a folder, is refused.
4. **Export.** `blink_core::export::item_list` builds the text (moved from the CLI, so
   `blink-cli export` and the GUI write the same thing). The `.txt` has one path per member file.
   The `.csv` has the header `path,rating,label,pick`, then one row per member with RFC 4180
   quoting. `pick` means the rating equals the configured pick value. `atomic::write_export`
   writes only `.txt`/`.csv` paths: temp file, fsync, rename, directory fsync. A save dialog
   therefore can never overwrite a photo or a sidecar. An existing list is replaced, because the
   save dialog already asked.
5. **UI.** A "Files" menu: "Copy filtered items to…", "Move filtered items to…" (folder picker)
   and "Export list as .txt… / .csv…" (save dialog). They run on the sidecar I/O thread (copies
   and moves) or on a worker thread (export).

## Consequences

- The CLI's export output is unchanged (CLI-1 goldens pass).

## Copies run beside culling

A copy moves nothing, so it doesn't hold the file-operation lock (`file_op_running`): it runs on
its own thread through `Fs`, counted in `copies_running`. Marks and rating undo keep working and
their debounced sidecar writes don't queue behind the copy. Operations that move files (apply
rejects, move, undo/redo of a file edit) wait until copies finish, so no file is pulled from under
a copy. A sidecar rewritten during a copy is copied as its old or its new version (atomic rename).
