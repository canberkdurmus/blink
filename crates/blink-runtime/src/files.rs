//! File operations (FILE-1): apply rejects to `_rejected/` or the OS trash, and the exact moves
//! of their undo and redo. Every change goes through [`Fs`]. A file is moved or trashed, never
//! unlinked, and a move never replaces an existing file (ADR 0021).

use crate::fs::Fs;
use crate::sidecar::belongs_to;
use blink_core::state::FileOp;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Folder that apply rejects moves items into, next to them (FILE-1). Scans skip it (OPEN-1).
pub const REJECTED_DIR: &str = "_rejected";

/// What a [`FileOp`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Moves made, `(from, to)`, in order.
    pub moves: Vec<(PathBuf, PathBuf)>,
    /// Files moved to the OS trash.
    pub trashed: Vec<PathBuf>,
    /// One message per failure.
    pub errors: Vec<String>,
}

fn dir_of(p: &Path) -> PathBuf {
    p.parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .to_path_buf()
}

fn name_of(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The files of one item: each member, then every sidecar in its folder that belongs to a
/// member (FILE-4 names, any case).
pub fn item_files(fs: &dyn Fs, members: &[(PathBuf, bool)]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = members.iter().map(|(p, _)| p.clone()).collect();
    let Some((first, _)) = members.first() else {
        return out;
    };
    let mut sidecars: Vec<PathBuf> = fs
        .read_dir(&dir_of(first))
        .unwrap_or_default()
        .into_iter()
        .filter(|e| !e.meta.is_dir && crate::atomic::is_sidecar_path(&e.path))
        .map(|e| e.path)
        .filter(|sc| members.iter().any(|(m, own)| belongs_to(sc, m, *own)))
        .collect();
    sidecars.sort();
    out.extend(sidecars);
    out
}

/// `name` with `-n` after its first `stem_len` bytes (`IMG_1.ARW.xmp` → `IMG_1-2.ARW.xmp`).
fn suffixed(name: &str, stem_len: usize, n: usize) -> String {
    match name.get(..stem_len) {
        Some(stem) if n > 0 => format!("{stem}-{n}{}", &name[stem_len..]),
        _ if n > 0 => format!("{n}-{name}"),
        _ => name.to_string(),
    }
}

fn sync_dirs(fs: &dyn Fs, dirs: &BTreeSet<PathBuf>, errors: &mut Vec<String>) {
    for d in dirs {
        if let Err(e) = fs.sync_dir(d) {
            errors.push(format!("{}: {e}", d.display()));
        }
    }
}

/// Move one item's `files` into `dest`, all under one name suffix that is free for every file
/// (so a pair and its sidecars keep matching names). On a failure, the files already moved go
/// back and the error is reported.
fn move_item(fs: &dyn Fs, files: &[PathBuf], dest: &Path, out: &mut Outcome) {
    let Some(primary) = files.first() else {
        return;
    };
    let stem_len = primary
        .file_stem()
        .map(|s| s.to_string_lossy().len())
        .unwrap_or(0);
    let target = |n: usize, f: &Path| dest.join(suffixed(&name_of(f), stem_len, n));
    let Some(n) = (0..10_000).find(|&n| files.iter().all(|f| !fs.exists(&target(n, f)))) else {
        out.errors.push(format!(
            "{}: no free name in {}",
            primary.display(),
            dest.display()
        ));
        return;
    };
    let mut done = Vec::new();
    for f in files {
        let to = target(n, f);
        match fs.move_file(f, &to) {
            Ok(()) => done.push((f.clone(), to)),
            Err(e) => {
                out.errors.push(format!("{}: {e}", f.display()));
                for (from, to) in done.iter().rev() {
                    if let Err(e) = fs.move_file(to, from) {
                        out.errors.push(format!(
                            "{} could not go back to {}: {e}",
                            to.display(),
                            from.display()
                        ));
                    }
                }
                return;
            }
        }
    }
    out.moves.extend(done);
}

/// Run a file operation.
pub fn run(fs: &dyn Fs, op: &FileOp) -> Outcome {
    let mut out = Outcome::default();
    let mut dirs = BTreeSet::new();
    match op {
        FileOp::Rejects { items, to_trash } => {
            for members in items {
                let files = item_files(fs, members);
                if *to_trash {
                    for f in files {
                        match fs.trash(&f) {
                            Ok(()) => out.trashed.push(f),
                            Err(e) => out.errors.push(format!("{}: {e}", f.display())),
                        }
                    }
                    continue;
                }
                let Some(primary) = files.first() else {
                    continue;
                };
                let folder = dir_of(primary);
                let dest = folder.join(REJECTED_DIR);
                if !fs.metadata(&dest).is_ok_and(|m| m.is_dir) {
                    if let Err(e) = fs.create_dir(&dest) {
                        out.errors.push(format!("{}: {e}", dest.display()));
                        continue;
                    }
                    dirs.insert(folder.clone());
                }
                let before = out.moves.len();
                move_item(fs, &files, &dest, &mut out);
                if out.moves.len() > before {
                    dirs.insert(folder);
                    dirs.insert(dest);
                }
            }
        }
        FileOp::Moves(moves) => {
            for (from, to) in moves {
                let parent = dir_of(to);
                if !fs.metadata(&parent).is_ok_and(|m| m.is_dir)
                    && let Err(e) = fs.create_dir(&parent)
                {
                    out.errors.push(format!("{}: {e}", parent.display()));
                    continue;
                }
                match fs.move_file(from, to) {
                    Ok(()) => {
                        dirs.insert(dir_of(from));
                        dirs.insert(parent);
                        out.moves.push((from.clone(), to.clone()));
                    }
                    Err(e) => out.errors.push(format!("{}: {e}", from.display())),
                }
            }
        }
    }
    // Make the moves durable.
    sync_dirs(fs, &dirs, &mut out.errors);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::{FsCall, MemFs};
    use std::collections::BTreeMap;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    /// Every file's bytes, by path.
    fn snapshot(fs: &MemFs) -> BTreeMap<PathBuf, Vec<u8>> {
        fs.file_paths()
            .into_iter()
            .map(|f| {
                let b = fs.peek(&f).unwrap();
                (f, b)
            })
            .collect()
    }

    fn no_unlink(fs: &MemFs) {
        for c in fs.calls() {
            assert!(!matches!(c, FsCall::RemoveFile(_)), "unlink: {c:?}");
        }
    }

    fn folder() -> MemFs {
        let fs = MemFs::new();
        for (f, b) in [
            ("/f/a.ARW", "raw a"),
            ("/f/a.JPG", "jpg a"),
            ("/f/a.xmp", "xmp a"),
            ("/f/a.JPG.xmp", "xmp a.jpg"),
            ("/f/b.ARW", "raw b"),
            ("/f/b.xmp", "xmp b"),
            ("/f/c.ARW", "raw c"),
            ("/f/c.ARW.XMP", "xmp c"),
            ("/f/_rejected/c.ARW", "older c"),
            ("/f/e.ARW", "raw e"),
            ("/f/e.CR3", "raw e cr3"),
            ("/f/e.xmp", "shared"),
        ] {
            fs.add_file(f, b.as_bytes().to_vec());
        }
        fs
    }

    #[test]
    fn req_file_1_rejects_move_every_member_and_sidecar_to_rejected_without_unlinking() {
        let fs = folder();
        let before = snapshot(&fs);
        let op = FileOp::Rejects {
            items: vec![
                vec![(p("/f/a.ARW"), true), (p("/f/a.JPG"), false)],
                vec![(p("/f/c.ARW"), true)],
                // Unpaired same-stem file: `e.xmp` is shared, so it stays.
                vec![(p("/f/e.ARW"), false)],
            ],
            to_trash: false,
        };
        let out = run(&fs, &op);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert_eq!(
            out.moves,
            [
                (p("/f/a.ARW"), p("/f/_rejected/a.ARW")),
                (p("/f/a.JPG"), p("/f/_rejected/a.JPG")),
                (p("/f/a.JPG.xmp"), p("/f/_rejected/a.JPG.xmp")),
                (p("/f/a.xmp"), p("/f/_rejected/a.xmp")),
                // A name taken in `_rejected/`: the whole item gets one free suffix.
                (p("/f/c.ARW"), p("/f/_rejected/c-1.ARW")),
                (p("/f/c.ARW.XMP"), p("/f/_rejected/c-1.ARW.XMP")),
                (p("/f/e.ARW"), p("/f/_rejected/e.ARW")),
            ]
        );
        no_unlink(&fs);
        // Same bytes, new places; nothing else changed.
        let after = snapshot(&fs);
        for (from, to) in &out.moves {
            assert_eq!(after.get(to), before.get(from), "{}", to.display());
            assert!(!after.contains_key(from));
        }
        for keep in [
            "/f/b.ARW",
            "/f/b.xmp",
            "/f/e.CR3",
            "/f/e.xmp",
            "/f/_rejected/c.ARW",
        ] {
            assert_eq!(after.get(&p(keep)), before.get(&p(keep)), "{keep}");
        }
        assert_eq!(after.len(), before.len());
        // The moves are made durable.
        let calls = fs.calls();
        assert!(calls.contains(&FsCall::SyncDir(p("/f/_rejected"))));
        assert!(calls.contains(&FsCall::SyncDir(p("/f"))));
        // Undo: the reversed moves put every file back byte for byte.
        let back: Vec<_> = out
            .moves
            .iter()
            .rev()
            .map(|(a, b)| (b.clone(), a.clone()))
            .collect();
        let undo = run(&fs, &FileOp::Moves(back));
        assert!(undo.errors.is_empty());
        assert_eq!(snapshot(&fs), before);
        no_unlink(&fs);
    }

    #[test]
    fn req_file_1_rejects_create_the_folder_and_trash_mode_uses_the_os_trash() {
        let fs = MemFs::new();
        fs.add_file("/f/x.ARW", b"x".to_vec());
        fs.add_file("/f/x.xmp", b"s".to_vec());
        let items = vec![vec![(p("/f/x.ARW"), true)]];
        let out = run(
            &fs,
            &FileOp::Rejects {
                items: items.clone(),
                to_trash: true,
            },
        );
        assert_eq!(out.trashed, [p("/f/x.ARW"), p("/f/x.xmp")]);
        assert!(out.moves.is_empty() && out.errors.is_empty());
        assert_eq!(fs.trashed(), out.trashed);
        no_unlink(&fs);
        // Folder mode creates `_rejected/` when missing.
        fs.add_file("/f/x.ARW", b"x".to_vec());
        let out = run(
            &fs,
            &FileOp::Rejects {
                items,
                to_trash: false,
            },
        );
        assert!(fs.calls().contains(&FsCall::CreateDir(p("/f/_rejected"))));
        assert_eq!(out.moves, [(p("/f/x.ARW"), p("/f/_rejected/x.ARW"))]);
    }

    #[test]
    fn req_file_1_a_failed_move_loses_no_file_and_is_reported() {
        let fs = folder();
        let before: BTreeSet<Vec<u8>> = snapshot(&fs).into_values().collect();
        // The second move fails, and so does every write after it (the rollback too).
        fs.fail_at_write(1);
        let out = run(
            &fs,
            &FileOp::Rejects {
                items: vec![vec![(p("/f/a.ARW"), true), (p("/f/a.JPG"), false)]],
                to_trash: false,
            },
        );
        assert!(!out.errors.is_empty());
        assert!(out.moves.is_empty());
        fs.clear_fault();
        let after: BTreeSet<Vec<u8>> = snapshot(&fs).into_values().collect();
        assert_eq!(after, before, "every file's bytes still exist somewhere");
        no_unlink(&fs);
    }

    #[test]
    #[should_panic(expected = "FILE-1 violation")]
    fn req_file_1_the_mock_fs_fails_any_test_that_unlinks_a_source_file() {
        let fs = MemFs::new();
        fs.add_file("/f/a.ARW", b"raw".to_vec());
        let _ = fs.remove_file(Path::new("/f/a.ARW"));
    }

    #[test]
    fn req_file_1_the_real_fs_refuses_to_unlink_a_source_file() {
        let dir = std::env::temp_dir().join(format!("blink-file1-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("a.ARW");
        std::fs::write(&raw, b"raw").unwrap();
        let e = crate::fs::RealFs.remove_file(&raw).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(std::fs::read(&raw).unwrap(), b"raw");
        // A no-replace move refuses an existing target.
        let other = dir.join("b.ARW");
        std::fs::write(&other, b"other").unwrap();
        let e = crate::fs::RealFs.move_file(&raw, &other).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&other).unwrap(), b"other");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
