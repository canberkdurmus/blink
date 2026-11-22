//! Launch an external app on the current item (FILE-3). Command construction is in
//! [`blink_core::external`]; this resolves program candidates through [`Fs`] and spawns.

use crate::fs::Fs;
use blink_core::external::{ExternalApp, is_path};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// A fully resolved command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

/// Starts a process and doesn't wait for it.
pub trait Launcher: Send + Sync {
    fn launch(&self, cmd: &Launch) -> std::io::Result<()>;
}

/// Spawns with [`std::process::Command`]: no shell, standard streams detached.
pub struct ProcessLauncher;

impl Launcher for ProcessLauncher {
    fn launch(&self, cmd: &Launch) -> std::io::Result<()> {
        use std::process::{Command, Stdio};
        let mut child = Command::new(&cmd.program)
            .args(&cmd.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        // Reap it when it exits so it never lingers as a zombie.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
}

fn is_sep(c: char) -> bool {
    c == '/' || (cfg!(windows) && c == '\\')
}

/// `*` matches any text (possibly empty); everything else literally, ASCII case-insensitively
/// (install folders differ in case between versions).
fn glob_match(pattern: &str, name: &str) -> bool {
    let (p, n) = (pattern.to_ascii_lowercase(), name.to_ascii_lowercase());
    let parts: Vec<&str> = p.split('*').collect();
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if parts.len() == 1 {
        return p == n;
    }
    if !n.starts_with(first) || n.len() < first.len() + last.len() || !n.ends_with(last) {
        return false;
    }
    let mut rest = &n[first.len()..n.len() - last.len()];
    for mid in &parts[1..parts.len() - 1] {
        match rest.find(mid) {
            Some(i) => rest = &rest[i + mid.len()..],
            None => return false,
        }
    }
    true
}

/// Order with digit runs compared as numbers, so `5.11` sorts above `5.9`.
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    fn chunks(s: &str) -> Vec<(bool, &str)> {
        let mut out = Vec::new();
        let mut start = 0;
        let b = s.as_bytes();
        for i in 1..=b.len() {
            if i == b.len() || b[i].is_ascii_digit() != b[start].is_ascii_digit() {
                out.push((b[start].is_ascii_digit(), &s[start..i]));
                start = i;
            }
        }
        out
    }
    let (ca, cb) = (chunks(a), chunks(b));
    for (x, y) in ca.iter().zip(&cb) {
        let o = match (x, y) {
            ((true, x), (true, y)) => {
                let (x, y) = (x.trim_start_matches('0'), y.trim_start_matches('0'));
                x.len().cmp(&y.len()).then_with(|| x.cmp(y))
            }
            ((_, x), (_, y)) => x.cmp(y),
        };
        if o.is_ne() {
            return o;
        }
    }
    ca.len().cmp(&cb.len()).then_with(|| a.cmp(b))
}

/// The existing file a program path pattern names, or `None`.
pub fn resolve_pattern(fs: &dyn Fs, pattern: &str) -> Option<PathBuf> {
    let Some(star) = pattern.find('*') else {
        let p = PathBuf::from(pattern);
        return fs.metadata(&p).is_ok_and(|m| !m.is_dir).then_some(p);
    };
    let start = pattern[..star].rfind(is_sep).map_or(0, |i| i + 1);
    let end = pattern[star..]
        .find(is_sep)
        .map_or(pattern.len(), |i| star + i);
    let (dir, comp, rest) = (&pattern[..start], &pattern[start..end], &pattern[end..]);
    let mut names: Vec<String> = fs
        .read_dir(Path::new(if dir.is_empty() { "." } else { dir }))
        .ok()?
        .into_iter()
        .filter_map(|e| e.path.file_name().map(|n| n.to_string_lossy().into_owned()))
        .filter(|n| glob_match(comp, n))
        .collect();
    // Highest version folder first.
    names.sort_by(|a, b| natural_cmp(b, a));
    names
        .iter()
        .find_map(|n| resolve_pattern(fs, &format!("{dir}{n}{rest}")))
}

/// The command for opening `file` with `app`: the first program candidate that exists (bare
/// names are left to `PATH`). `None` when only missing paths are listed.
pub fn command(fs: &dyn Fs, app: &ExternalApp, file: &Path) -> Option<Launch> {
    let program = app.programs.iter().find_map(|p| {
        if is_path(p) {
            resolve_pattern(fs, p)
        } else {
            Some(PathBuf::from(p))
        }
    })?;
    Some(Launch {
        program,
        args: app.args_for(file),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::MemFs;
    use blink_core::external::{TargetOs, default_apps};

    #[test]
    fn req_file_3_version_folders_compare_numerically() {
        use std::cmp::Ordering::*;
        assert_eq!(natural_cmp("5.11", "5.9"), Greater);
        assert_eq!(natural_cmp("GIMP 3", "GIMP 2"), Greater);
        assert_eq!(natural_cmp("1.0", "1.0"), Equal);
        assert_eq!(natural_cmp("", "a"), Less);
        assert_eq!(natural_cmp("v010", "v9"), Greater);
    }

    #[test]
    fn req_file_3_glob_matches_version_folders() {
        assert!(glob_match("*", "5.11"));
        assert!(glob_match("GIMP *", "gimp 2"));
        assert!(glob_match("gimp-*.exe", "gimp-2.10.exe"));
        assert!(glob_match("a*b*c", "aXbYc"));
        assert!(!glob_match("a*b*c", "aXcYb"));
        assert!(!glob_match("ab*ba", "aba"));
        assert!(!glob_match("x", "y"));
    }

    #[test]
    fn req_file_3_first_existing_candidate_wins_and_the_newest_version_folder() {
        let fs = MemFs::new();
        fs.add_file("/opt/RawTherapee/5.9/rawtherapee", b"x".to_vec());
        fs.add_file("/opt/RawTherapee/5.11/rawtherapee", b"x".to_vec());
        fs.add_dir("/opt/RawTherapee/notes");
        let app = ExternalApp {
            name: "RawTherapee".into(),
            programs: vec![
                "/missing/rt".into(),
                "/opt/RawTherapee/*/rawtherapee".into(),
                "rawtherapee".into(),
            ],
            args: vec![],
        };
        let c = command(&fs, &app, Path::new("/p/a.CR3")).unwrap();
        assert_eq!(
            c.program,
            PathBuf::from("/opt/RawTherapee/5.11/rawtherapee")
        );
        assert_eq!(c.args, vec![OsString::from("/p/a.CR3")]);
        // Resolving only reads.
        assert!(fs.calls().iter().all(|c| !c.is_write()));
    }

    #[test]
    fn req_file_3_bare_names_fall_back_to_path_and_missing_paths_give_none() {
        let fs = MemFs::new();
        let app = &default_apps(TargetOs::Windows)[0];
        let c = command(&fs, app, Path::new("/p/a.NEF")).unwrap();
        assert_eq!(c.program, PathBuf::from("darktable"));
        let only_paths = ExternalApp {
            name: "x".into(),
            programs: vec!["/nope/x".into()],
            args: vec![],
        };
        assert_eq!(command(&fs, &only_paths, Path::new("/p/a.NEF")), None);
    }
}
