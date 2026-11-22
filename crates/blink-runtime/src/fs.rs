//! The file-system boundary (SPEC §8.1). Every code path that touches files goes through [`Fs`],
//! so tests can use [`MemFs`], an in-memory file system that records every call.
//!
//! Writes are the primitives of an atomic sidecar write (FILE-6, [`crate::atomic`]): create a new
//! file, fsync it, rename it over the target, fsync the directory. `remove_file` exists only for
//! Blink's own temp files and for sidecars an undo takes back; it is never called on a source file
//! (FILE-1, FILE-7).
//!
//! [`MemFs`] can fail any write step and simulate a power loss, for crash injection (FILE-6).

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
    /// Read at most the first `len` bytes of a file (metadata scans, FILT-1).
    fn read_prefix(&self, path: &Path, len: usize) -> io::Result<Vec<u8>> {
        let mut b = self.read(path)?;
        b.truncate(len);
        Ok(b)
    }
    /// Metadata of a file or directory (follows symlinks).
    fn metadata(&self, path: &Path) -> io::Result<Meta>;
    /// Entries of a directory, in no particular order.
    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>>;
    /// Whether a path exists.
    fn exists(&self, path: &Path) -> bool {
        self.metadata(path).is_ok()
    }
    // Writes. The defaults make a read-only file system (test wrappers that only read).

    /// Create `path` (it must not exist) holding `bytes`. Not durable until [`Fs::sync_file`].
    fn write_new(&self, path: &Path, _bytes: &[u8]) -> io::Result<()> {
        Err(read_only(path))
    }
    /// Flush a file's data to stable storage.
    fn sync_file(&self, path: &Path) -> io::Result<()> {
        Err(read_only(path))
    }
    /// Rename `from` to `to`, atomically replacing `to` if it exists.
    fn rename(&self, from: &Path, _to: &Path) -> io::Result<()> {
        Err(read_only(from))
    }
    /// Make the directory's entries (a rename, a removal) durable.
    fn sync_dir(&self, dir: &Path) -> io::Result<()> {
        Err(read_only(dir))
    }
    /// Remove a file Blink created: a temp file, or a sidecar that an undo takes back. Both
    /// file systems refuse anything else ([`crate::atomic::may_remove`]).
    fn remove_file(&self, path: &Path) -> io::Result<()> {
        Err(read_only(path))
    }
    /// Create one directory (its parent must exist).
    fn create_dir(&self, path: &Path) -> io::Result<()> {
        Err(read_only(path))
    }
    /// Move a file to `to` on the same volume, failing with `AlreadyExists` if `to` exists: a
    /// move never replaces a file (FILE-1, FILE-2).
    fn move_file(&self, from: &Path, _to: &Path) -> io::Result<()> {
        Err(read_only(from))
    }
    /// Move a file to the operating system's trash (FILE-1).
    fn trash(&self, path: &Path) -> io::Result<()> {
        Err(read_only(path))
    }
    /// Copy a file to `to`, durable when it returns, failing with `AlreadyExists` if `to` exists
    /// (FILE-2). A failed copy leaves nothing at `to`.
    fn copy_file(&self, from: &Path, _to: &Path) -> io::Result<()> {
        Err(read_only(from))
    }
}

fn refuse_unlink(p: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("refusing to unlink a source file: {}", p.display()),
    )
}

fn read_only(p: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        format!("read-only file system: {}", p.display()),
    )
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

    fn read_prefix(&self, path: &Path, len: usize) -> io::Result<Vec<u8>> {
        use std::io::Read;
        let mut b = Vec::with_capacity(len.min(1 << 20));
        std::fs::File::open(path)?
            .take(len as u64)
            .read_to_end(&mut b)?;
        Ok(b)
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

    fn write_new(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        f.write_all(bytes)
    }

    fn sync_file(&self, path: &Path) -> io::Result<()> {
        // Windows needs write access for FlushFileBuffers.
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)?
            .sync_all()
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        // Unix: rename(2), atomic replace. Windows: std uses FileRenameInfoEx with
        // FILE_RENAME_FLAG_REPLACE_IF_EXISTS (MoveFileExW with MOVEFILE_REPLACE_EXISTING on old
        // systems), an atomic replace like ReplaceFileW (ADR 0015).
        std::fs::rename(from, to)
    }

    fn sync_dir(&self, dir: &Path) -> io::Result<()> {
        #[cfg(unix)]
        {
            std::fs::File::open(dir)?.sync_all()
        }
        // NTFS journals renames; a directory handle can't be flushed without extra privileges.
        #[cfg(not(unix))]
        {
            let _ = dir;
            Ok(())
        }
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        if !crate::atomic::may_remove(path) {
            return Err(refuse_unlink(path));
        }
        std::fs::remove_file(path)
    }

    fn create_dir(&self, path: &Path) -> io::Result<()> {
        std::fs::create_dir(path)
    }

    fn move_file(&self, from: &Path, to: &Path) -> io::Result<()> {
        // Check, then rename: another program creating `to` in between would be replaced. Blink
        // never does; ADR 0021 accepts the window (no portable no-replace rename in std).
        if std::fs::symlink_metadata(to).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                to.display().to_string(),
            ));
        }
        std::fs::rename(from, to)
    }

    fn copy_file(&self, from: &Path, to: &Path) -> io::Result<()> {
        if std::fs::symlink_metadata(to).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                to.display().to_string(),
            ));
        }
        // Into a temp file next to `to`, made durable, then moved into place: a crash or a full
        // disk never leaves a partial copy under the real name.
        let tmp = crate::atomic::temp_path(to);
        let copied = (|| {
            let mut src = std::fs::File::open(from)?;
            let mut dst = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)?;
            std::io::copy(&mut src, &mut dst)?;
            dst.sync_all()
        })();
        let placed = copied.and_then(|()| self.move_file(&tmp, to));
        if placed.is_err() {
            let _ = self.remove_file(&tmp);
        }
        placed
    }

    fn trash(&self, path: &Path) -> io::Result<()> {
        trash_moves_without_copying(path)?;
        #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
        let mut ctx = trash::TrashContext::default();
        #[cfg(target_os = "macos")]
        {
            // NSFileManager: no Finder automation prompt, no sound (ADR 0021).
            use trash::macos::{DeleteMethod, TrashContextExtMacos};
            ctx.set_delete_method(DeleteMethod::NsFileManager);
        }
        ctx.delete(path)
            .map_err(|e| io::Error::other(e.to_string()))
    }
}

/// Refuse to trash a file unless the OS trash takes it with a rename (ADR 0021). Linux: the
/// `trash` crate falls back to the home trash on volumes without a usable top-directory trash,
/// and moves across volumes by copy + unlink; so the file must be on the home trash's volume.
/// Windows: a volume without a Recycle Bin (most removable drives) may delete for good, so its
/// root must hold `$Recycle.Bin`. macOS: NSFileManager fails rather than deletes.
fn trash_moves_without_copying(path: &Path) -> io::Result<()> {
    let refuse = |why: &str| -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "{}: {why}; set rejects = \"folder\" in config.toml",
                path.display()
            ),
        ))
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        use std::os::unix::fs::MetadataExt;
        let home_trash = std::env::var_os("XDG_DATA_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".local/share")))
            .map(|d| d.join("Trash"));
        let dev = |p: &Path| {
            p.ancestors()
                .find_map(|a| std::fs::metadata(a).ok())
                .map(|m| m.dev())
        };
        let file_dev = std::fs::metadata(path)?.dev();
        if home_trash.as_deref().and_then(dev) != Some(file_dev) {
            return refuse(
                "not on the same volume as the trash (it would be copied, then deleted)",
            );
        }
    }
    #[cfg(windows)]
    {
        let root: PathBuf = path
            .components()
            .take_while(|c| {
                matches!(
                    c,
                    std::path::Component::Prefix(_) | std::path::Component::RootDir
                )
            })
            .collect();
        if !root.join("$Recycle.Bin").exists() {
            return refuse("this drive has no Recycle Bin (the file could be deleted for good)");
        }
    }
    let _ = refuse;
    Ok(())
}

/// One call made against a [`MemFs`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsCall {
    Read(PathBuf),
    /// `read_prefix(path, len)`.
    ReadPrefix(PathBuf, usize),
    Metadata(PathBuf),
    ReadDir(PathBuf),
    WriteNew(PathBuf),
    SyncFile(PathBuf),
    Rename(PathBuf, PathBuf),
    SyncDir(PathBuf),
    RemoveFile(PathBuf),
    CreateDir(PathBuf),
    MoveFile(PathBuf, PathBuf),
    CopyFile(PathBuf, PathBuf),
    Trash(PathBuf),
}

impl FsCall {
    /// Whether the call changes the file system.
    pub fn is_write(&self) -> bool {
        !matches!(
            self,
            Self::Read(_) | Self::ReadPrefix(..) | Self::Metadata(_) | Self::ReadDir(_)
        )
    }

    /// The paths a write call changes (empty for reads and syncs).
    pub fn changed_paths(&self) -> Vec<&Path> {
        match self {
            Self::WriteNew(p) | Self::RemoveFile(p) | Self::CreateDir(p) | Self::Trash(p) => {
                vec![p]
            }
            Self::Rename(a, b) | Self::MoveFile(a, b) => vec![a, b],
            Self::CopyFile(_, b) => vec![b],
            _ => vec![],
        }
    }
}

/// A directory change not yet made durable by `sync_dir`; undone by [`MemFs::power_loss`].
#[derive(Debug, Clone)]
enum Pending {
    /// `path` was created.
    Created(PathBuf),
    /// `from` was renamed over `to`; `to_before` is what `to` held.
    Renamed {
        from: PathBuf,
        to: PathBuf,
        to_before: Option<Vec<u8>>,
        to_before_unsynced: bool,
    },
    /// `path` was removed; it held `bytes`.
    Removed { path: PathBuf, bytes: Vec<u8> },
}

impl Pending {
    fn dir(&self) -> Option<&Path> {
        match self {
            Self::Created(p) | Self::Removed { path: p, .. } => p.parent(),
            Self::Renamed { to, .. } => to.parent(),
        }
    }
}

/// Crash-injection state of a [`MemFs`].
#[derive(Debug, Default)]
struct Faults {
    /// Number of write calls made so far.
    writes: usize,
    /// The write call (0-based) that fails; every later write fails too (the process died).
    fail_at: Option<usize>,
    /// Files whose data was written but not fsynced.
    unsynced: std::collections::BTreeSet<PathBuf>,
    /// Directory changes not yet fsynced, oldest first.
    pending: Vec<Pending>,
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
    faults: Mutex<Faults>,
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

    /// Make the `n`-th write call from now (0-based) fail, and every write after it: the
    /// process "dies" at that step. A failing `write_new` leaves a torn (half-written) file.
    pub fn fail_at_write(&self, n: usize) {
        let mut f = lock(&self.faults);
        f.fail_at = Some(f.writes + n);
    }

    /// Stop failing writes: the fault was transient (a full disk freed up), not a crash.
    pub fn clear_fault(&self) {
        lock(&self.faults).fail_at = None;
    }

    /// Number of write calls made so far.
    pub fn write_count(&self) -> usize {
        lock(&self.faults).writes
    }

    /// Simulate a power loss: directory changes since the last `sync_dir` of their directory are
    /// undone (newest first), then data never fsynced is lost (the file is left empty).
    /// Clears the injected fault, so the file system works again ("reboot").
    pub fn power_loss(&self) {
        let mut f = lock(&self.faults);
        let mut files = lock(&self.files);
        while let Some(p) = f.pending.pop() {
            match p {
                Pending::Created(path) => {
                    files.remove(&path);
                    f.unsynced.remove(&path);
                }
                Pending::Renamed {
                    from,
                    to,
                    to_before,
                    to_before_unsynced,
                } => {
                    if let Some(b) = files.remove(&to) {
                        files.insert(from.clone(), b);
                    }
                    if f.unsynced.remove(&to) {
                        f.unsynced.insert(from);
                    }
                    if let Some(b) = to_before {
                        files.insert(to.clone(), b);
                        if to_before_unsynced {
                            f.unsynced.insert(to);
                        }
                    }
                }
                Pending::Removed { path, bytes } => {
                    files.insert(path, bytes);
                }
            }
        }
        for p in std::mem::take(&mut f.unsynced) {
            if let Some(b) = files.get_mut(&p) {
                b.clear();
            }
        }
        f.fail_at = None;
    }

    /// Paths of all files, sorted.
    pub fn file_paths(&self) -> Vec<PathBuf> {
        lock(&self.files).keys().cloned().collect()
    }

    /// Contents of a file without recording a call.
    pub fn peek(&self, path: impl AsRef<Path>) -> Option<Vec<u8>> {
        lock(&self.files).get(path.as_ref()).cloned()
    }

    /// Count a write call; `Err` if it is the injected fault or comes after it.
    fn write_step(&self, c: FsCall) -> io::Result<()> {
        self.record(c);
        let mut f = lock(&self.faults);
        let n = f.writes;
        f.writes += 1;
        match f.fail_at {
            Some(at) if n >= at => Err(io::Error::other(format!("injected failure at write {n}"))),
            _ => Ok(()),
        }
    }

    fn dir_exists(&self, path: &Path) -> bool {
        path.parent()
            .is_some_and(|d| d.as_os_str().is_empty() || lock(&self.dirs).contains_key(d))
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

    fn read_prefix(&self, path: &Path, len: usize) -> io::Result<Vec<u8>> {
        self.record(FsCall::ReadPrefix(path.to_path_buf(), len));
        lock(&self.files)
            .get(path)
            .map(|b| b[..b.len().min(len)].to_vec())
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

    fn write_new(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let torn = self.write_step(FsCall::WriteNew(path.to_path_buf()));
        if lock(&self.files).contains_key(path) || lock(&self.dirs).contains_key(path) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                path.display().to_string(),
            ));
        }
        if !self.dir_exists(path) {
            return Err(not_found(path));
        }
        let data = match &torn {
            Ok(()) => bytes.to_vec(),
            Err(_) => bytes[..bytes.len() / 2].to_vec(),
        };
        lock(&self.files).insert(path.to_path_buf(), data);
        let mut f = lock(&self.faults);
        f.unsynced.insert(path.to_path_buf());
        f.pending.push(Pending::Created(path.to_path_buf()));
        torn
    }

    fn sync_file(&self, path: &Path) -> io::Result<()> {
        self.write_step(FsCall::SyncFile(path.to_path_buf()))?;
        if !lock(&self.files).contains_key(path) {
            return Err(not_found(path));
        }
        lock(&self.faults).unsynced.remove(path);
        Ok(())
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.write_step(FsCall::Rename(from.to_path_buf(), to.to_path_buf()))?;
        if lock(&self.dirs).contains_key(to) || !self.dir_exists(to) {
            return Err(io::Error::other(format!(
                "cannot rename to {}",
                to.display()
            )));
        }
        let mut files = lock(&self.files);
        let bytes = files.remove(from).ok_or_else(|| not_found(from))?;
        let to_before = files.insert(to.to_path_buf(), bytes);
        drop(files);
        let mut f = lock(&self.faults);
        let to_before_unsynced = f.unsynced.remove(to);
        if f.unsynced.remove(from) {
            f.unsynced.insert(to.to_path_buf());
        }
        f.pending.push(Pending::Renamed {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
            to_before,
            to_before_unsynced,
        });
        drop(f);
        let mut m = lock(&self.mtimes);
        match m.remove(from) {
            Some(t) => m.insert(to.to_path_buf(), t),
            None => m.remove(to),
        };
        Ok(())
    }

    fn sync_dir(&self, dir: &Path) -> io::Result<()> {
        self.write_step(FsCall::SyncDir(dir.to_path_buf()))?;
        lock(&self.faults).pending.retain(|p| p.dir() != Some(dir));
        Ok(())
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        // Every test runs on this file system: unlinking a source file fails the test (FILE-1).
        assert!(
            crate::atomic::may_remove(path),
            "FILE-1 violation: unlink of a source file {}",
            path.display()
        );
        self.write_step(FsCall::RemoveFile(path.to_path_buf()))?;
        let bytes = lock(&self.files)
            .remove(path)
            .ok_or_else(|| not_found(path))?;
        lock(&self.mtimes).remove(path);
        let mut f = lock(&self.faults);
        f.unsynced.remove(path);
        f.pending.push(Pending::Removed {
            path: path.to_path_buf(),
            bytes,
        });
        Ok(())
    }

    fn create_dir(&self, path: &Path) -> io::Result<()> {
        self.write_step(FsCall::CreateDir(path.to_path_buf()))?;
        if lock(&self.dirs).contains_key(path) || lock(&self.files).contains_key(path) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                path.display().to_string(),
            ));
        }
        if !self.dir_exists(path) {
            return Err(not_found(path));
        }
        lock(&self.dirs).insert(path.to_path_buf(), false);
        Ok(())
    }

    fn move_file(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.write_step(FsCall::MoveFile(from.to_path_buf(), to.to_path_buf()))?;
        if lock(&self.files).contains_key(to) || lock(&self.dirs).contains_key(to) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                to.display().to_string(),
            ));
        }
        if !self.dir_exists(to) {
            return Err(not_found(to));
        }
        let bytes = lock(&self.files)
            .remove(from)
            .ok_or_else(|| not_found(from))?;
        lock(&self.files).insert(to.to_path_buf(), bytes);
        let mut m = lock(&self.mtimes);
        if let Some(t) = m.remove(from) {
            m.insert(to.to_path_buf(), t);
        }
        Ok(())
    }

    fn copy_file(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.write_step(FsCall::CopyFile(from.to_path_buf(), to.to_path_buf()))?;
        if lock(&self.files).contains_key(to) || lock(&self.dirs).contains_key(to) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                to.display().to_string(),
            ));
        }
        if !self.dir_exists(to) {
            return Err(not_found(to));
        }
        let bytes = lock(&self.files)
            .get(from)
            .cloned()
            .ok_or_else(|| not_found(from))?;
        lock(&self.files).insert(to.to_path_buf(), bytes);
        Ok(())
    }

    fn trash(&self, path: &Path) -> io::Result<()> {
        self.write_step(FsCall::Trash(path.to_path_buf()))?;
        lock(&self.files)
            .remove(path)
            .map(|_| ())
            .ok_or_else(|| not_found(path))
    }
}

impl MemFs {
    /// Files moved to the trash so far, in order (they are gone from the file system).
    pub fn trashed(&self) -> Vec<PathBuf> {
        self.calls()
            .into_iter()
            .filter_map(|c| match c {
                FsCall::Trash(p) => Some(p),
                _ => None,
            })
            .collect()
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
