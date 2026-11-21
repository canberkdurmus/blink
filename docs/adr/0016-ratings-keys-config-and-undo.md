# 0016. Rating keys, pick, config file and undo

Date: 2026-11-21 · Status: accepted

## Context

RATE-1 (one key per mark, optional auto-advance), RATE-2 (configurable pick value; unpick only
clears a rating equal to it) and RATE-3 (unlimited undo/redo; undo-all restores every sidecar
byte for byte, including absence). SPEC §5.2 gives the keys. SPEC §6 lets label names be
configured.

## Decision

1. **Keys** (`blink` preset, SPEC §5.2): `0`–`5` set stars; `P` pick, `X` reject, `U` unflag;
   `6` `7` `8` `9` `Shift+9` toggle Red/Yellow/Green/Blue/Purple; `Shift+A` toggles auto-advance;
   `Ctrl/Cmd+Z` undo, `Ctrl/Cmd+Shift+Z` redo. Digits are ignored with Alt (the M4 filters use
   `Alt+0`…`Alt+5`) and with Ctrl/Cmd. KEY-1 (M5) makes all of these remappable.
2. **Label keys toggle.** Pressing the key of the current label removes it, and another label key
   replaces it. Otherwise no key could clear a label (Lightroom and Bridge behave the same).
3. **Auto-advance** moves to the next item after stars, pick, reject and unflag, not after labels
   (labels are often stacked on top of a rating). It is off by default. The config file sets the
   start value, and `Shift+A` toggles it with a toast.
4. **Pick and unflag (RATE-2).** Pick sets the rating to `pick_value` (1–5, default 1, D-2).
   Unflag sets 0 when the rating is the pick value or −1 (reject), and leaves every other star
   rating alone. The overlay shows `PICK` when the rating equals the pick value.
5. **Config file** `config.toml` in the PLAT-2 config directory: `pick_value`,
   `sidecar_style` (`adobe`/`darktable`, D-3), `auto_advance`, `[labels]` names. A missing file
   means defaults. An invalid file (unknown keys, pick value out of range, empty label name)
   means defaults plus a toast, or a stderr line in the CLI. Blink always starts. New dependencies
   in `blink-runtime`: **`toml` 1** and **`serde` 1** (derive), both MIT/Apache-2.0, widely
   maintained, pure Rust, build on all 6 targets, no network. KEY-1's TOML keymaps will need them
   too. They already sit in the lockfile through xtask and testkit.
6. **Undo (RATE-3).** The reducer keeps an `Edit { key, before, after }` per mark change that
   actually changes something, with unlimited depth. A new edit clears the redo stack. Opening
   another folder starts a new history: the session is the folder being culled. Undo and redo
   don't move the current item.
7. **Byte-exact restore.** Replaces ADR 0015 §7's comparison of values per file. The reducer
   remembers each item's on-disk marks (`disk_marks`, from RATE-4 loading). Whenever an item's
   marks return to them, through undo, redo or by hand, every member gets
   `MarkChange::Restore { rating, label }` instead of field changes. The writer then puts back
   each sidecar's bytes from before Blink first wrote it this session, or removes a sidecar
   Blink created. That holds even when a pair's sidecars disagreed (e.g. `a.xmp` 2★, `a.JPG.xmp`
   1★). If another app changed a sidecar since Blink last wrote it, the carried values are
   written into the current file instead, so that app's changes survive (FILE-5). A write that
   wouldn't change any owned field is skipped. The property test runs random sequences of every
   mark action, navigation, undo, redo and flush point over pairs, every sidecar style and
   missing sidecars. After undo-all it compares every file byte for byte.
8. **File operations** (FILE-1 apply rejects, FILE-2 copy/move) are M4. They join the same
   undo stack when they are built. RATE-3's file-operation part is tested then.

## Consequences

The CLI reads the same config: pick value for `picked` filters and exports, sidecar style when
`--style` is not given.
