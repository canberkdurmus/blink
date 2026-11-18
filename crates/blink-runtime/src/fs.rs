//! The file-system boundary (SPEC §8.1). Every code path that touches files goes through [`Fs`],
//! so tests can use [`MemFs`], an in-memory file system that records every call.
//!
//! M1 needs read access only. Write operations (atomic sidecar writes, moves, trash) are added
//! with the IDs that need them (FILE-*); none of them will ever unlink a source file.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// What [`Fs::metadata`] and [`Fs::read_dir`] report about one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meta {
    pub is_dir: bool,
    pub len: u64,
    pub modified: Option<SystemTime>,
}

/// One entry of a directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub path: PathBuf,
    pub meta: Meta,
    /// The entry itself is a symlink (or, on Windows, a junction); `meta` describes its target.
    pub is_link: bool,
}

/// File-system access used by the runtime, the CLI and the UI.
pub trait Fs: Send + Sync {
    /// Read a whole file.
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;
    /// Metadata of a file or directory (follows symlinks).
    fn metadata(&self, path: &Path) -> io::Result<Meta>;
    /// Entries of a directory, in no particular order.
    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>>;
    /// Whether a path exists.
    fn exists(&self, path: &Path) -> bool {
        self.metadata(path).is_ok()
    }
}

/// The real file system.
#[derive(Debug, Default, Clone, Copy)]
pub struct RealFs;

fn meta_of(m: &std::fs::Metadata) -> Meta {
    Meta {
        is_dir: m.is_dir(),
        len: m.len(),
        modified: m.modified().ok(),
    }
}

impl Fs for RealFs {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn metadata(&self, path: &Path) -> io::Result<Meta> {
        std::fs::metadata(path).map(|m| meta_of(&m))
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(path)? {
            let e = e?;
            let path = e.path();
            // Windows: junctions are name-surrogate reparse points, which std reports as symlinks.
            let is_link = e.file_type().is_ok_and(|t| t.is_symlink());
            // Follow symlinks; skip entries that vanished or are dangling.
            if let Ok(m) = std::fs::metadata(&path) {
                out.push(DirEntry {
                    path,
                    meta: meta_of(&m),
                    is_link,
                });
            }
        }
        Ok(out)
    }
}

/// One call made against a [`MemFs`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsCall {
    Read(PathBuf),
    Metadata(PathBuf),
    ReadDir(PathBuf),
}

/// In-memory file system for tests. Directories are implicit (any prefix of a file path) or
/// explicit (added with [`MemFs::add_dir`]). Every call is recorded in [`MemFs::calls`].
#[derive(Debug, Default)]
pub struct MemFs {
    files: Mutex<BTreeMap<PathBuf, Vec<u8>>>,
    /// Directory → whether it is a symlinked directory.
    dirs: Mutex<BTreeMap<PathBuf, bool>>,
    mtimes: Mutex<BTreeMap<PathBuf, SystemTime>>,
    log: Mutex<Vec<FsCall>>,
}

impl MemFs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add (or replace) a file and all its parent directories.
    pub fn add_file(&self, path: impl Into<PathBuf>, bytes: impl Into<Vec<u8>>) {
        let path = path.into();
        if let Some(p) = path.parent() {
            self.add_dir(p);
        }
        lock(&self.files).insert(path, bytes.into());
    }

    /// Add an (empty) directory and its parents.
    pub fn add_dir(&self, path: impl AsRef<Path>) {
        let mut dirs = lock(&self.dirs);
        for a in path.as_ref().ancestors() {
            if !a.as_os_str().is_empty() {
                dirs.entry(a.to_path_buf()).or_insert(false);
            }
        }
    }

    /// Add a directory that is a symlink (or junction) to another directory.
    pub fn add_dir_link(&self, path: impl AsRef<Path>) {
        self.add_dir(&path);
        lock(&self.dirs).insert(path.as_ref().to_path_buf(), true);
    }

    /// Set a file's modification time.
    pub fn set_modified(&self, path: impl Into<PathBuf>, t: SystemTime) {
        lock(&self.mtimes).insert(path.into(), t);
    }

    fn modified(&self, path: &Path) -> Option<SystemTime> {
        lock(&self.mtimes).get(path).copied()
    }

    /// Every call made so far, in order.
    pub fn calls(&self) -> Vec<FsCall> {
        lock(&self.log).clone()
    }

    fn record(&self, c: FsCall) {
        lock(&self.log).push(c);
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn not_found(p: &Path) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, p.display().to_string())
}

impl Fs for MemFs {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.record(FsCall::Read(path.to_path_buf()));
        lock(&self.files)
            .get(path)
            .cloned()
            .ok_or_else(|| not_found(path))
    }

    fn metadata(&self, path: &Path) -> io::Result<Meta> {
        self.record(FsCall::Metadata(path.to_path_buf()));
        if let Some(len) = lock(&self.files).get(path).map(|b| b.len() as u64) {
            return Ok(Meta {
                is_dir: false,
                len,
                modified: self.modified(path),
            });
        }
        if lock(&self.dirs).contains_key(path) {
            return Ok(Meta {
                is_dir: true,
                len: 0,
                modified: None,
            });
        }
        Err(not_found(path))
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        self.record(FsCall::ReadDir(path.to_path_buf()));
        if !lock(&self.dirs).contains_key(path) {
            return Err(not_found(path));
        }
        let mut out = Vec::new();
        let files: Vec<(PathBuf, u64)> = lock(&self.files)
            .iter()
            .filter(|(p, _)| p.parent() == Some(path))
            .map(|(p, b)| (p.clone(), b.len() as u64))
            .collect();
        for (p, len) in files {
            let modified = self.modified(&p);
            out.push(DirEntry {
                path: p,
                meta: Meta {
                    is_dir: false,
                    len,
                    modified,
                },
                is_link: false,
            });
        }
        for (p, &is_link) in lock(&self.dirs).iter() {
            if p.parent() == Some(path) {
                out.push(DirEntry {
                    path: p.clone(),
                    meta: Meta {
                        is_dir: true,
                        len: 0,
                        modified: None,
                    },
                    is_link,
                });
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memfs_lists_files_and_dirs_and_logs_calls() {
        let fs = MemFs::new();
        fs.add_file("/a/x.ARW", b"raw".to_vec());
        fs.add_dir("/a/sub");
        let mut names: Vec<_> = fs
            .read_dir(Path::new("/a"))
            .unwrap()
            .into_iter()
            .map(|e| (e.path, e.meta.is_dir))
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                (PathBuf::from("/a/sub"), true),
                (PathBuf::from("/a/x.ARW"), false)
            ]
        );
        assert_eq!(fs.read(Path::new("/a/x.ARW")).unwrap(), b"raw");
        assert!(fs.read(Path::new("/a/nope")).is_err());
        assert_eq!(fs.calls().len(), 3);
    }

    #[test]
    fn realfs_reads_a_temp_dir() {
        let dir = std::env::temp_dir().join(format!("blink-fs-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f.txt"), b"hi").unwrap();
        let entries = RealFs.read_dir(&dir).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].meta.len, 2);
        assert_eq!(RealFs.read(&dir.join("f.txt")).unwrap(), b"hi");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
