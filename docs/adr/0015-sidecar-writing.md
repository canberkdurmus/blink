# 0015. Sidecar writing: naming, rewrite, atomic replace, debounce, byte-exact undo

Date: 2026-11-21 · Status: accepted

## Context

M3 makes Blink write sidecars (FILE-4, FILE-5, FILE-6, FILE-7, FILE-9) and executes the
`WriteSidecar` effects that M1 left unexecuted (ADR 0010). Some points are not fixed by SPEC:

- OPEN-2's acceptance says every pair member gets its own sidecar, but with the default Adobe
  style (D-3) `a.ARW` and `a.JPG` both map to `a.xmp` (ADR 0010 note).
- FILE-6 says "on Windows use `ReplaceFileW` semantics". Calling `ReplaceFileW` needs `unsafe`
  FFI and a new dependency.
- FILE-9 says "ISO 8601 with offset". The local UTC offset needs a time-zone dependency.
- RATE-3 wants undo-all to restore every sidecar byte for byte, but every write changes
  `xmp:MetadataDate`.

## Decision

1. **Naming (FILE-4).** For each member file Blink updates every existing sidecar:
   Adobe-style `IMG.xmp` and/or darktable-style `IMG.ARW.xmp` (`.xmp` before `.XMP`; on
   case-insensitive file systems both name one file). Both are updated if both exist. If neither
   exists, Blink creates the D-3 style. **The Adobe-style name belongs to one file only**: the RAW
   of a pair, or a standalone file whose stem no other item shares. JPEG/HEIF companions, and
   same-stem files that didn't pair (e.g. `a.ARW` + `a.NEF`), only ever use (and create) their
   darktable-style name. So `a.ARW` + `a.JPG` get `a.xmp` + `a.JPG.xmp`: every member has its own
   sidecar with the same values (OPEN-2), and no file's write clobbers another's sidecar. The
   reducer computes this as `owns_adobe_name` on the effect, because only it sees every item.
2. **Rewrite (FILE-5).** `blink-xmp::write_owned` splices the original bytes. Only the values of
   `xmp:Rating`, `xmp:Label` and `xmp:MetadataDate` change, and each keeps its form: an attribute
   stays an attribute (its `rdf:Description` start tag is rebuilt, so whitespace between that
   tag's attributes may change) and an element keeps its element, with new text. A field that
   doesn't exist yet is added as an attribute of the first `rdf:Description`, using the prefix
   already bound to the XMP namespace (`xap:` in old files). If none is bound, Blink declares
   `xmlns:xmp`, or a free prefix if `xmp` is bound to something else, so no foreign binding is
   shadowed. If there is no `rdf:Description`, one is added inside `rdf:RDF`. Blink never
   overwrites a sidecar it cannot parse: the write fails, the file stays untouched, the change
   stays queued and the UI shows a toast. Blank files count as absent. New sidecars are minimal
   (SPEC §6).
3. **Atomic writes (FILE-6).** Steps: create the hidden temp file `.NAME.blink-PID-N.tmp` in the
   same directory, fsync it, rename it over the target, then fsync the directory (Unix). All
   steps go through `Fs` (new methods `write_new`, `sync_file`, `rename`, `sync_dir`,
   `remove_file`; read-only defaults for test wrappers). On Windows, `std::fs::rename` replaces
   the target atomically (`SetFileInformationByHandle` with `FileRenameInfoEx` and
   `FILE_RENAME_FLAG_REPLACE_IF_EXISTS | POSIX_SEMANTICS`, falling back to `MoveFileExW` with
   `MOVEFILE_REPLACE_EXISTING`). That gives the property FILE-6 asks for: the target is the old
   or the new file, never missing. `ReplaceFileW` would also carry over ACLs and alternate data
   streams, which sidecars don't use. It is not worth `unsafe` FFI and a dependency (CLAUDE.md).
   The Windows directory fsync is a no-op: NTFS journals the rename. The writer refuses any
   path that isn't `.xmp` (FILE-7 defence in depth). `remove_file` is only used for its own temp
   files and for sidecars an undo takes back.
4. **Crash model.** `MemFs` can fail the n-th write call (and every later one: the process
   died), tear a failing `write_new` in half, and simulate a power loss. A power loss undoes
   directory changes not yet covered by `sync_dir` and empties files never fsynced. The FILE-6
   test runs every failure point, with and without a power loss, on every sidecar style. A
   self-test checks that the model catches a writer that skips the fsync.
5. **Debounce.** Changes are queued per source file and coalesced (the last value per field
   wins). They are written `DEBOUNCE` = 100 ms after the *first* queued change. Later changes
   don't extend the wait, so the latency stays bounded. That leaves 150 ms of PERF-7's 250 ms
   for the write. The GUI runs the writer on its own I/O thread (`SidecarService`). Dropping
   the service (app exit) flushes everything still queued. The CLI writes at once.
6. **MetadataDate (FILE-9)** is UTC with an explicit `+00:00` offset, e.g.
   `2026-11-21T09:05:00+00:00`. This is ISO 8601 with offset, it compares correctly in
   Lightroom, and it needs no time-zone dependency. It is written on every change.
7. **Byte-exact undo (for RATE-3).** *(Amended by ADR 0016 §7: the restore is triggered by the
   reducer's `MarkChange::Restore`, not by comparing values per file.)* The writer keeps each sidecar's bytes (or absence) from
   before Blink first wrote it in the session. When a write would leave the owned fields equal
   to those original values, the original bytes are restored instead (`MetadataDate` included).
   A sidecar Blink created is removed. Undoing everything therefore restores every sidecar byte
   for byte, including absence. The same holds when the user manually puts a value back.
   The restore happens only while the file still holds the bytes Blink last wrote: if another
   app changed it in the meantime, the normal FILE-5 splice runs instead, so that app's changes
   survive. A restore also brings back the original `xmp:MetadataDate`. Strictly, FILE-9 says
   every change Blink makes sets it, so this is a FILE-9 vs RATE-3 conflict. Byte-exact undo
   wins, because the file then equals what other apps last saw. The M3 gate report flags it.
10. **Retries back off.** A failed write stays queued and is retried after 200 ms, 400 ms and
   so on, up to 30 s. A sidecar that can never be written doesn't keep the I/O thread busy.
8. **`MarkChange::Rating(Option<i8>)`**: `None` removes `xmp:Rating` (undo back to "no rating").
9. **Dev dependency**: `proptest` 1 (MIT/Apache-2.0) for the FILE-5 property test (and RATE-3
   later). Dev-only, so no runtime policy check is needed beyond `cargo deny`.

## Consequences

- The CLI's `rate`/`label` now write. `--dry-run` keeps the M1 output. `--style` picks D-3.
- The M2 overlay drew ratings with a `★` glyph that the default fonts lack. A folder with rated
  sidecars would have panicked the GUI. Stars are now painted shapes, exposed to AccessKit as
  "N stars".
- FILE-8 (external changes) builds on the writer's knowledge of what it last wrote.
