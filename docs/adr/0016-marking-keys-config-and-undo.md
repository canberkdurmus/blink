# 0016. Marking keys, the config file, and undo/redo

Date: 2026-11-22 · Status: accepted

## Context

RATE-1, RATE-2 and RATE-3 (M3) add the mark keys of SPEC §5.2, the configurable pick value
(D-2) and session undo/redo. SPEC leaves some points open:

- RATE-1 says "each one key" but §5.2 has no key that clears a label.
- RATE-2 says "unpick restores 0 only if the rating equals the pick value". `U` is "unflag", which
  also has to clear a reject.
- RATE-2's acceptance is "config tests", but Blink had no config file yet.
- RATE-3's acceptance wants undo-all to restore every sidecar byte for byte. The members of a pair
  can hold different values on disk (e.g. a Lightroom `a.xmp` rated 4, a digiKam `a.JPG.xmp`
  rated 1), while the reducer keeps one value per item.
- RATE-3 also covers file operations. Those arrive in M4 (FILE-1, FILE-2).

## Decision

1. **Keys** (`Action::Mark(MarkKey)`, current item): `0`–`5` set the stars. `P` sets the pick value,
   `X` sets −1. `U` sets 0 when the rating is the pick value or −1, and otherwise does nothing.
   `6`–`9` and `Shift+9` set a label. **A label key on an item that already has that label clears
   it** (that is how Lightroom and darktable toggle labels), so each label stays one key. Digits
   match on the physical key (the logical key is the fallback), so they sit in the same place on
   every layout and `Shift+9` is purple whatever the layout makes of it. Key repeats
   are ignored for mark keys. `0` on an unrated item changes nothing, because "no rating" already
   means 0 in XMP. `Shift+A` toggles auto-advance: after a mark key, Blink moves to the next item
   (it stays on the last one). Remapping comes with KEY-1 (M5).
2. **Overlay**: the stars, `PICK` when the rating equals the pick value, `REJECTED`, and the label
   with a colour swatch. The top bar has an "Auto-advance" checkbox.
3. **Config file**: `config.toml` in the PLAT-2 config directory, with the keys `pick_value`
   (1–5, default 1), `auto_advance`, `sidecar_style` (`adobe`/`darktable`, D-3) and `[labels]`
   `red`…`purple` (the `xmp:Label` names, SPEC §6). A missing file means the defaults. A broken
   key keeps its default, and the GUI shows the problem as a toast at start. Two colours with one
   name are rejected, because they couldn't be told apart when read back. The CLI keeps the D-2/D-3
   defaults and its `--style` flag, so its output depends only on its arguments.
   **Dependency**: `toml` 1 (MIT/Apache-2.0, already used by dev tooling, and needed by KEY-1's
   TOML keymaps anyway; maintained by the toml-rs project, with frequent releases) becomes a runtime dependency of `blink-runtime`, with default features off
   and `std` + `parse` on. That means no serde and no derive: the file is read as a `toml::Table`.
4. **Undo history** (`State::undo` / `State::redo`, in the pure reducer): each mark change is one
   `MarkEdit` (item primary path, the marks before and after, one field changed). There is no
   limit on its length. A new edit clears the redo stack. Each edit keeps its write targets (every member's
   source and `owns_adobe_name`). Undo and redo write those same sidecars, even when a rescan
   (recursive toggle, a file moved away) dropped the item, and move the view to the item if it
   is still listed. An edit made before the item's marks finished loading recorded "nothing" as
   its original value. `MarksLoaded` fills that in, so after the undo the view shows what the
   revert puts back on disk. When an undo takes a field back to the value it had
   at session start (no earlier edit in the history touched it), the reducer emits
   `MarkChange::RevertRating` / `RevertLabel` instead of a value. The writer then sets each
   sidecar's field to that sidecar's own value from before Blink first wrote it. ADR 0015 §7's
   restore then gives back the original bytes, including absence. Pair members with different
   values come back exactly, and so do out-of-range ratings Blink doesn't read. The history is
   per open folder (`OpenFolder` clears it), because its items are gone after that. Sidecars of
   the previous folder stay as they were written. File operations join the history when they
   exist (M4).
5. **Property test** (`blink-runtime/tests/undo.rs`): a folder with every sidecar situation
   (Lightroom, darktable, both styles, none, a pair with different per-member values, a standalone
   JPEG, unpaired same-stem RAWs, an out-of-range rating), random sequences of mark keys,
   navigation, undo, redo, auto-advance toggles and debounce-timed flushes, in both D-3 styles and
   with every pick value, then undo-all. Every file must equal the starting snapshot byte for
   byte. A mutation that writes the before value instead of reverting fails it.

## Consequences

- `MarkChange` has two more variants. The CLI's dry-run prints them as "revert".
- `Options` gains `marks` and `notice`. `blink_ui::configured_options()` reads the config.
