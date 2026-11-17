# 0007. PLAT-2: `directories` crate and portable mode layout

Date: 2026-11-17 · Status: accepted

## Context

PLAT-2: config and caches in platform-standard locations via the `directories` crate; portable mode
when a `blink-portable` file sits next to the binary. SPEC does not say where portable data goes.

## Decision

- Dependency **`directories` `=6.0.0`** in `blink-runtime`. License MIT OR Apache-2.0. Maintained
  (dirs-dev); pure Rust over `dirs-sys` (libc / windows-sys; `option-ext` is MPL-2.0). No network.
  Builds on all 6 targets (CI matrix).
- App ID per D-7: `ProjectDirs::from("io.github", "canberkdurmus", "Blink")`. Resulting locations:
  Linux `$XDG_CONFIG_HOME/blink`, `$XDG_CACHE_HOME/blink`; macOS
  `~/Library/Application Support/io.github.canberkdurmus.Blink`, `~/Library/Caches/io.github.canberkdurmus.Blink`;
  Windows `%APPDATA%\canberkdurmus\Blink\config`, `%LOCALAPPDATA%\canberkdurmus\Blink\cache`.
- Portable mode: the marker must be a **file** named `blink-portable` in the executable's directory.
  Config goes to `<exe dir>/blink-data/config`, caches to `<exe dir>/blink-data/cache`. The marker
  check goes through the `Fs` trait.

## Alternatives

- Data directly next to the binary: clutters the folder the user unpacked.
- `dirs` crate (lower level): SPEC names `directories`.

## Consequences

The runtime's `fs::Fs` trait is introduced here (read-only methods for now); OPEN-1 extends it.
