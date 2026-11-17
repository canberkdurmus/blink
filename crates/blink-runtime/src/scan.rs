//! Folder scanning (OPEN-1). Lists files through [`Fs`]; pairing happens in `blink-core`.

use crate::fs::Fs;
use blink_core::item::ScannedFile;
use std::io;
use std::path::Path;

/// Folder that "Apply rejects" moves rejected items into (FILE-1). Never scanned.
pub const REJECTED_DIR: &str = "_rejected";

/// List the files in `folder`; with `recursive`, also every sub-folder except hidden ones and
/// `_rejected`. Sorted by path. Errors only if `folder` itself can't be read; unreadable
/// sub-folders are skipped.
pub fn scan_folder(fs: &dyn Fs, folder: &Path, recursive: bool) -> io::Result<Vec<ScannedFile>> {
    let mut out = Vec::new();
    let mut stack = vec![folder.to_path_buf()];
    let mut first = true;
    while let Some(dir) = stack.pop() {
        let entries = match fs.read_dir(&dir) {
            Ok(e) => e,
            Err(e) if first => return Err(e),
            Err(_) => continue,
        };
        first = false;
        for e in entries {
            let name = e
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if e.meta.is_dir {
                if recursive && !name.starts_with('.') && name != REJECTED_DIR {
                    stack.push(e.path);
                }
            } else {
                out.push(ScannedFile {
                    path: e.path,
                    len: e.meta.len,
                });
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::MemFs;
    use std::path::PathBuf;

    fn fs() -> MemFs {
        let fs = MemFs::new();
        for p in [
            "/s/1.ARW",
            "/s/1.JPG",
            "/s/sub/2.ARW",
            "/s/sub/deeper/3.ARW",
            "/s/_rejected/4.ARW",
            "/s/.hidden/5.ARW",
        ] {
            fs.add_file(p, b"x".to_vec());
        }
        fs
    }

    fn names(v: &[ScannedFile]) -> Vec<PathBuf> {
        v.iter().map(|f| f.path.clone()).collect()
    }

    #[test]
    fn req_open_1_non_recursive_lists_only_the_folder() {
        let got = scan_folder(&fs(), Path::new("/s"), false).unwrap();
        assert_eq!(
            names(&got),
            [PathBuf::from("/s/1.ARW"), PathBuf::from("/s/1.JPG")]
        );
    }

    #[test]
    fn req_open_1_recursive_skips_hidden_and_rejected() {
        let got = scan_folder(&fs(), Path::new("/s"), true).unwrap();
        assert_eq!(
            names(&got),
            [
                "/s/1.ARW",
                "/s/1.JPG",
                "/s/sub/2.ARW",
                "/s/sub/deeper/3.ARW"
            ]
            .map(PathBuf::from)
        );
        assert!(scan_folder(&fs(), Path::new("/nope"), false).is_err());
    }

    #[test]
    fn req_open_1_scan_only_reads_directories() {
        let fs = fs();
        scan_folder(&fs, Path::new("/s"), true).unwrap();
        assert!(
            fs.calls()
                .iter()
                .all(|c| matches!(c, crate::fs::FsCall::ReadDir(_)))
        );
    }
}
