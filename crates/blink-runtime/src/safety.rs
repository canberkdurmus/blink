//! Safety log (SPEC §13, M4 gate: "zero source-file writes across the suite").
//!
//! When `BLINK_SAFETY_LOG` names a directory, both file systems ([`crate::fs::RealFs`] and
//! [`crate::fs::MemFs`]) append one line per write call to a file of their own in it:
//! `op \t class \t ok|err \t path`. `class` is what the target path is: a `sidecar`, a Blink
//! `temp` file, an export `list` (FILE-2), or a `source` (anything else: a photo, or a file Blink
//! doesn't own). `cargo xtask verify --full` sets the variable and fails if any successful
//! `rename` (a replace) or `remove_file` touched a `source`: the only calls that can change or
//! remove an existing file's bytes. `write_new` creates a file and fails if it exists; moves
//! (`move_file`, `trash`) and copies change no bytes; they are counted separately. Without the variable nothing is logged
//! and the cost is one cached check.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// What a target path is.
pub fn class(path: &Path) -> &'static str {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    if crate::atomic::is_sidecar_path(path) {
        "sidecar"
    } else if name.starts_with('.') && name.ends_with(".tmp") && name.contains(".blink-") {
        "temp"
    } else if crate::atomic::is_export_path(path) {
        "list"
    } else {
        "source"
    }
}

/// Whether a logged call is a source-file write: it changed or removed a file Blink doesn't own.
pub fn is_violation(op: &str, class: &str, ok: bool) -> bool {
    ok && class == "source" && matches!(op, "rename" | "remove_file")
}

fn sink() -> Option<&'static Mutex<std::fs::File>> {
    static SINK: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();
    SINK.get_or_init(|| {
        let dir = PathBuf::from(std::env::var_os("BLINK_SAFETY_LOG")?);
        std::fs::create_dir_all(&dir).ok()?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(format!("{}-{nanos}.log", std::process::id())))
            .ok()?;
        Some(Mutex::new(file))
    })
    .as_ref()
}

/// Log one write call on `target` (no-op unless `BLINK_SAFETY_LOG` is set).
pub fn log<T>(op: &str, target: &Path, result: &std::io::Result<T>) {
    let Some(sink) = sink() else { return };
    let line = format!(
        "{op}\t{}\t{}\t{}\n",
        class(target),
        if result.is_ok() { "ok" } else { "err" },
        target.display()
    );
    let mut f = sink.lock().unwrap_or_else(|e| e.into_inner());
    let _ = f.write_all(line.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn req_file_7_safety_log_classes_and_violations() {
        assert_eq!(class(Path::new("/f/IMG_1.ARW.xmp")), "sidecar");
        assert_eq!(class(Path::new("/f/IMG_1.xmp")), "sidecar");
        assert_eq!(
            class(&crate::atomic::temp_path(Path::new("/f/IMG_1.xmp"))),
            "temp"
        );
        assert_eq!(class(Path::new("/out/list.csv")), "list");
        assert_eq!(class(Path::new("/f/IMG_1.ARW")), "source");
        assert_eq!(class(Path::new("/f/IMG_1.JPG")), "source");
        assert!(is_violation("rename", "source", true));
        assert!(is_violation("remove_file", "source", true));
        assert!(!is_violation("write_new", "source", true), "only creates");
        assert!(!is_violation("remove_file", "source", false), "refused");
        assert!(
            !is_violation("move_file", "source", true),
            "a move changes no bytes"
        );
        assert!(
            !is_violation("copy_file", "source", true),
            "a copy is a new file"
        );
        assert!(!is_violation("rename", "sidecar", true));
    }
}
