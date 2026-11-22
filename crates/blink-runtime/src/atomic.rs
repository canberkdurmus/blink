//! Atomic sidecar writes (FILE-6): temp file in the same directory → fsync → rename → fsync of
//! the directory. A failure at any step leaves the old or the new sidecar, never a torn or
//! missing one. Only `.xmp` files can be written or removed here (FILE-7: source files never).

use crate::fs::Fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static SEQ: AtomicU64 = AtomicU64::new(0);

/// Whether `path` names a sidecar (`.xmp`, any case). Everything else is off limits for writes.
pub fn is_sidecar_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("xmp"))
}

/// Whether Blink may unlink `path`: only a sidecar (undo of a sidecar Blink created) or one of
/// its own temp files. Source files are never unlinked (SPEC §1, FILE-1); both file systems
/// enforce this in [`crate::fs::Fs::remove_file`].
pub fn may_remove(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    let temp = name.starts_with('.') && name.ends_with(".tmp") && name.contains(".blink-");
    temp || is_sidecar_path(path)
}

fn refuse(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("refusing to write a non-sidecar file: {}", path.display()),
    )
}

/// The temp file for `target`: hidden, in the same directory, unique per process and call.
pub fn temp_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    target.with_file_name(format!(".{name}.blink-{}-{n}.tmp", std::process::id()))
}

fn dir_of(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

/// Replace (or create) the sidecar `target` with `bytes`, atomically and durably.
pub fn write_atomic(fs: &dyn Fs, target: &Path, bytes: &[u8]) -> io::Result<()> {
    if !is_sidecar_path(target) {
        return Err(refuse(target));
    }
    let tmp = temp_path(target);
    let r = fs
        .write_new(&tmp, bytes)
        .and_then(|()| fs.sync_file(&tmp))
        .and_then(|()| fs.rename(&tmp, target));
    if r.is_err() {
        // Best effort: the temp file is ours. If this fails too, a hidden temp file stays behind;
        // the sidecar itself is untouched.
        if fs.exists(&tmp) {
            let _ = fs.remove_file(&tmp);
        }
        return r;
    }
    fs.sync_dir(dir_of(target))
}

/// Remove a sidecar Blink created (an undo back to "no sidecar", RATE-3), durably.
pub fn remove_sidecar(fs: &dyn Fs, target: &Path) -> io::Result<()> {
    if !is_sidecar_path(target) {
        return Err(refuse(target));
    }
    fs.remove_file(target)?;
    fs.sync_dir(dir_of(target))
}

/// Whether `path` may be written as an item list export (FILE-2): `.txt` or `.csv` only, so a
/// save dialog can never overwrite a photo or a sidecar.
pub fn is_export_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("txt") || e.eq_ignore_ascii_case("csv"))
}

/// Write an item list atomically, replacing an existing `.txt`/`.csv` (the save dialog asked).
pub fn write_export(fs: &dyn Fs, target: &Path, bytes: &[u8]) -> io::Result<()> {
    if !is_export_path(target) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "an item list must be a .txt or .csv file: {}",
                target.display()
            ),
        ));
    }
    let tmp = temp_path(target);
    let r = fs
        .write_new(&tmp, bytes)
        .and_then(|()| fs.sync_file(&tmp))
        .and_then(|()| fs.rename(&tmp, target))
        .and_then(|()| fs.sync_dir(dir_of(target)));
    if r.is_err() {
        let _ = fs.remove_file(&tmp);
    }
    r
}
