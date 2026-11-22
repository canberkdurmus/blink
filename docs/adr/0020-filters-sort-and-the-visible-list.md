# 0020. FILT-1/FILT-2: filters, sort and the visible list

Date: 2026-11-22 · Status: accepted

## Context

FILT-1 asks for filters on marks (rating ≥/=, rejected, picked, label), on capture metadata
(body, lens, focal range, ISO range, date range) and on file type, combined with AND and shown as
removable chips. FILT-2 asks for a stable sort by capture time (default), filename or rating, with
ties broken by filename. SPEC §8.2 puts filters and sort in `blink-core`. The SPEC doesn't say
what happens to the current item when it stops passing the filter, which direction the rating
sort runs, or whether filters survive opening another folder.

## Decision

1. **Visible list.** `State.items` stays the full scan (sidecar naming and pairing need every
   file). `State.visible` holds the indices of the items that pass the filter, in sort order.
   Navigation, the prefetch window, the decode-ahead list, auto-advance and the counter all work
   on `visible`. `current` is still an index into `items`, and it is always visible (or `None`
   when nothing passes). The reducer recomputes `visible` when the filter, sort, items, marks
   (only if the filter or sort reads marks) or metadata change.
2. **Current item leaves the filter.** When a mark, a reload or a new chip hides the current
   item, the item that followed it in the old order becomes current. If there is none, the one
   before it does. Rejecting under "Not rejected" then behaves like auto-advance. With auto-advance
   on, the item after the marked one is current, not the one after that. Undo moves back to the
   item only when the filter shows it.
3. **Sort directions.** Capture time runs oldest first, and items without a capture time go
   last. Filename is ascending. Rating runs highest first (unrated counts as 0, rejected comes
   last). Every tie is broken by the file name, then by the full path, so the order is total and
   never depends on the scan order (property-tested).
4. **Capture metadata.** `ItemMeta` (body as the info panel shows it, lens, focal length, ISO,
   `DateTimeOriginal`) is read by `blink_runtime::meta` on a background thread after each scan
   and arrives in chunks of 256 items as `Action::MetaLoaded`. The scan starts only once the first
   image is on screen, so it never competes with it (PERF-1). Only the first 256 KiB of each file
   is read, through the new `Fs::read_prefix`. EXIF sits within 128 KiB in every corpus file. A
   prefix that yields no EXIF value at all is read again up to 4 MiB. Files are never read whole:
   2,000 whole-file reads made PERF-1 31 % slower in CI. HEIF files
   aren't parsed in v1 (D-4). An item without a value fails every condition on that field.
   Dates compare as `YYYY-MM-DD` on the camera's local date. No time-zone conversion is done.
5. **Syntax.** The CLI `--filter` and the GUI's expression field share one parser:
   `body=`, `lens=`, `focal=MIN..MAX`, `iso=MIN..MAX`, `date=FROM..TO` (either end may be left
   out, and a single value is a one-value range), plus the existing M1 conditions. Text compares
   ignoring ASCII case. `Filter::expr` round-trips through `Filter::parse` (property-tested).
6. **Chips.** Each condition is a chip (`Rating ≥ 3 ×`), and clicking it removes the condition.
   Conditions added from the UI replace any condition of the same slot (one rating condition,
   one reject flag, one label, …), so `Alt+3` after `Alt+1` gives "Rating ≥ 3", not an empty
   AND. The "+ Filter" menu lists ratings, flags, labels, the folder's bodies and lenses (from
   the loaded metadata), file types, and an expression field for ranges. Global shortcuts are
   off while a text field has keyboard focus.
7. **Scope across folders.** Opening a folder clears the filter, because a leftover filter would
   show an empty folder without saying why. The sort order is kept.
8. **CLI.** `list`/`export` keep their path order (the CLI-1 goldens don't change). They read
   metadata only when the filter needs it.

## Alternatives

- Filtering `items` itself: sidecar naming (FILE-4, shared stems) and FILE-8 reloads need the
  full list.
- Keeping a filtered-out current item until the user navigates (Lightroom's behaviour): this
  needs a "current but not visible" state, which every index computation would have to handle.
- Full-file EXIF reads: 2,000 × 25–50 MB is too much I/O for a background scan (PERF-9).

## Consequences

- When metadata arrives the order can change. The current item stays current, and its new
  neighbours are prefetched.
- `Action` gained `MetaLoaded`, `AddFilter`, `RemoveFilter`, `SetFilter` and `SetSort`.
