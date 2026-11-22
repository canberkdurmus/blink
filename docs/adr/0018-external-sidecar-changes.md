# 0018. External sidecar changes: watcher, reload, conflicts

Date: 2026-11-22 · Status: accepted

## Context

FILE-8: if a sidecar changes on disk, reload it. If Blink also has unsaved changes for it, keep
the on-disk version, show a conflict marker on the item, and log it. SPEC §8.4 names `notify`.

## Decision

- **Dependency: `notify` `=9.0.0`** (CC0-1.0, the standard Rust file watcher, actively
  maintained, pure Rust on Linux/Windows; on macOS it uses FSEvents through the `objc2` bindings,
  with no C build). It builds on all 6 targets and has no network access. Default features are
  off except `macos_fsevent`. The pin is exact because the event API changes between majors.
- The `SidecarService` I/O thread owns the watcher of the open folder (recursive when the scan
  is). Watcher events for `.xmp` paths go into the same channel as writes, so they are
  serialized with them. Access events are ignored.
- **Own writes are not external**: the writer remembers the bytes it last wrote per sidecar. An
  event whose file holds them (Blink's own rename, or repeated events) is dropped.
- **External change**: the new bytes become the writer's reference for later writes and
  restores. Queued (debounced, not yet written) changes of every source that writes that
  sidecar (FILE-4 names) are dropped, so the on-disk version wins. The reducer gets
  `SidecarChanged { sidecar, conflict }`, maps it to the item(s) and re-reads their marks
  (`ReloadMarks` → `MarksReloaded`). The reload replaces the item's marks and removes that
  item's undo/redo entries, which would otherwise undo over the other app's change. The new
  bytes are also the writer's restore point, so a later change to that item followed by an
  undo brings back the other app's version, not the session-start bytes (ADR 0016).
- **Conflict** (unsaved changes were dropped): the item gets a `CONFLICT` marker in the overlay
  (SPEC §5.1). The message goes to the toast, the session log (`State::log`) and stderr. The
  marker stays until the user marks the item again.
- An external change while nothing is queued is a plain reload, with a toast and no marker.
- **Write-time check.** Watcher events can arrive late (or not at all on some network file
  systems), so the writer doesn't rely on them alone. When a source's first change is queued, it
  snapshots that source's sidecars. Just before the debounced write it compares the snapshot with
  the disk. If a sidecar differs (changed, created or deleted by another app), Blink writes
  nothing for that source and reports the same conflict as a watcher event. The later watcher
  event for that change is then a duplicate (the file holds the known bytes) and is dropped.
  A read error is no evidence: the write goes ahead and fails or retries as usual.
- **Paths.** FSEvents reports real paths (`/private/var/…` for `/var/…`, a symlinked volume's
  target), so event paths are mapped back onto the folder as opened. Known sidecars are matched
  case-insensitively (APFS, NTFS).
- **Stale reloads.** Each item has an edit generation, which goes up with every mark change,
  undo and redo. A reload carries the generation it was asked at, and is ignored if the user
  marked the item since. The user's change is newer, and the writer applies it to the new disk
  version.
- **Failed late steps.** If the rename succeeded but the directory fsync failed, the bytes on
  disk are still Blink's. The retry writes normally and reports no conflict.
- The watcher watches real paths (it is not file I/O). Every read still goes through `Fs`. Tests
  inject events with `SidecarService::changed` / `BlinkApp::sidecars_changed`, and the full-tier
  test runs a real watcher on a temp folder.

## Consequences

The integration test drives a real watcher on a temp folder (an atomic replace by "another app")
and the GUI with injected events (`BlinkApp::sidecars_changed`).
