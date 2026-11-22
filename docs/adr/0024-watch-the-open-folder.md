# 0024. OPEN-3: watch the open folder

Status: accepted (2026-11-22)

## Context

OPEN-3: "Watch the open folder: new files appear, deleted files disappear, renamed files
update", accepted by "state reflects each change within 1 s". FILE-8 already runs a `notify`
watcher on the sidecar thread for the open folder.

## Decision

1. The same watcher also reports changes to files that aren't sidecars, including content
   writes, so a file listed while still being copied in is read again. It ignores hidden names
   (temp files, Blink's included) and access events. It sends a single
   `SidecarEvent::FolderChanged`, with no paths. Renames on some backends arrive as
   remove + create, so the folder is rescanned rather than patched per event.
2. The UI gathers a burst (a card copy) for 150 ms from its first event, so a long copy
   still updates the list every ~150 ms plus the scan time. It then dispatches `Action::FolderChanged`,
   which the reducer turns into the existing `ScanFolder` effect. The UI keeps the running
   watch for a rescan of the same folder and mode, so no events are lost while it restarts.
   Reopening a folder always re-creates the watch. Scans of one folder and mode run one at a
   time: a rescan asked for while one runs starts when it returns, so an older listing never
   lands after a newer one.
3. `FolderScanned` keeps the current item. If that item is gone, the next one still listed
   takes its place, and if there is none, the previous one, as for apply rejects. After any scan,
   marks are read only for items not listed before. Capture metadata is read for those, and
   again for any listed item that has none yet.
4. Undo/redo history, marks and filters are untouched: they are keyed by path, and a renamed
   file is a new item (its sidecar moves with it only if the renaming app moved it).

## Consequences

- A rescan lists the folder (one `read_dir`, or a walk when recursive). That is cheap next to
  the 1 s budget, and bursts collapse into one.
- Tests: `req_open_3_added_removed_and_renamed_files_show_within_1_s` (fast, MemFs plus an
  injected event), `full_folder_watch::req_open_3_the_real_watcher_…` (real files and watcher,
  full tier on all 6 targets), and the reducer test for the current-item fallback.
