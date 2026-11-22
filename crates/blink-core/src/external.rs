//! Open the current item in an external app (FILE-3): the configurable app list, its per-OS
//! defaults, and the command line for a file. No I/O: the runtime resolves wildcard program
//! paths and spawns the process.
//!
//! The file is always one argument of its own and no shell is involved, so no file name can
//! inject a command.

use std::ffi::OsString;
use std::path::Path;

/// Placeholder argument replaced by the file's path.
pub const FILE: &str = "{file}";

/// The operating system a command is built for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetOs {
    Linux,
    MacOs,
    Windows,
}

impl TargetOs {
    /// The OS this binary was built for (other Unixes use the Linux defaults).
    pub fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }
}

/// One entry of the "Open in" list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalApp {
    /// Shown in the menu.
    pub name: String,
    /// Program candidates, tried in order; the first that exists is run. A bare name is looked
    /// up on `PATH`. A `*` inside a path component matches any text there (version folders);
    /// the highest-sorting match wins.
    pub programs: Vec<String>,
    /// Arguments; an argument equal to [`FILE`] becomes the file's path. Without one, the path
    /// is appended as the last argument.
    pub args: Vec<String>,
}

impl ExternalApp {
    /// An app from a config `command` array: program first, then arguments.
    pub fn from_command(name: &str, command: &[String]) -> Option<Self> {
        let (program, args) = command.split_first()?;
        if program.trim().is_empty() {
            return None;
        }
        Some(Self {
            name: name.to_string(),
            programs: vec![program.clone()],
            args: args.to_vec(),
        })
    }

    /// Arguments for opening `file`: [`FILE`] replaced, or the path appended.
    pub fn args_for(&self, file: &Path) -> Vec<OsString> {
        let mut out: Vec<OsString> = Vec::with_capacity(self.args.len() + 1);
        let mut placed = false;
        for a in &self.args {
            if a == FILE {
                out.push(file.as_os_str().to_os_string());
                placed = true;
            } else {
                out.push(a.into());
            }
        }
        if !placed {
            out.push(file.as_os_str().to_os_string());
        }
        out
    }
}

fn app(name: &str, programs: &[&str], args: &[&str]) -> ExternalApp {
    ExternalApp {
        name: name.into(),
        programs: programs.iter().map(|s| s.to_string()).collect(),
        args: args.iter().map(|s| s.to_string()).collect(),
    }
}

/// The default list (SPEC FILE-3): darktable, RawTherapee, GIMP, the system default.
pub fn default_apps(os: TargetOs) -> Vec<ExternalApp> {
    match os {
        TargetOs::Linux => vec![
            app("darktable", &["darktable"], &[]),
            app("RawTherapee", &["rawtherapee"], &[]),
            app("GIMP", &["gimp"], &[]),
            app("System default", &["xdg-open"], &[]),
        ],
        TargetOs::MacOs => vec![
            app("darktable", &["open"], &["-a", "darktable", FILE]),
            app("RawTherapee", &["open"], &["-a", "RawTherapee", FILE]),
            app("GIMP", &["open"], &["-a", "GIMP", FILE]),
            app("System default", &["open"], &[FILE]),
        ],
        TargetOs::Windows => vec![
            app(
                "darktable",
                &[r"C:\Program Files\darktable\bin\darktable.exe", "darktable"],
                &[],
            ),
            app(
                "RawTherapee",
                &[
                    r"C:\Program Files\RawTherapee\*\rawtherapee.exe",
                    "rawtherapee",
                ],
                &[],
            ),
            app(
                "GIMP",
                &[
                    r"C:\Program Files\GIMP 3\bin\gimp-3.exe",
                    r"C:\Program Files\GIMP 3\bin\gimp-3.0.exe",
                    r"C:\Program Files\GIMP 2\bin\gimp-2.10.exe",
                    "gimp",
                ],
                &[],
            ),
            // Explorer opens a file with its associated app, without a shell parsing the name.
            app("System default", &["explorer.exe"], &[]),
        ],
    }
}

/// A program candidate is a path (to check for) rather than a name looked up on `PATH`.
pub fn is_path(program: &str) -> bool {
    program.contains('/') || program.contains('\\')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn os(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    fn first_command(os_: TargetOs, name: &str, file: &Path) -> (String, Vec<OsString>) {
        let a = default_apps(os_)
            .into_iter()
            .find(|a| a.name == name)
            .unwrap_or_else(|| panic!("no default {name}"));
        (a.programs[0].clone(), a.args_for(file))
    }

    #[test]
    fn req_file_3_defaults_are_darktable_rawtherapee_gimp_and_the_system_default_on_every_os() {
        for o in [TargetOs::Linux, TargetOs::MacOs, TargetOs::Windows] {
            let names: Vec<_> = default_apps(o).into_iter().map(|a| a.name).collect();
            assert_eq!(
                names,
                ["darktable", "RawTherapee", "GIMP", "System default"],
                "{o:?}"
            );
        }
    }

    #[test]
    fn req_file_3_linux_commands() {
        let f = PathBuf::from("/photos/a b/IMG_0001.CR3");
        for (name, prog) in [
            ("darktable", "darktable"),
            ("RawTherapee", "rawtherapee"),
            ("GIMP", "gimp"),
            ("System default", "xdg-open"),
        ] {
            assert_eq!(
                first_command(TargetOs::Linux, name, &f),
                (prog.to_string(), os(&["/photos/a b/IMG_0001.CR3"]))
            );
        }
    }

    #[test]
    fn req_file_3_macos_commands() {
        let f = PathBuf::from("/Users/me/Pictures/IMG_0001.ARW");
        for (name, app) in [
            ("darktable", "darktable"),
            ("RawTherapee", "RawTherapee"),
            ("GIMP", "GIMP"),
        ] {
            assert_eq!(
                first_command(TargetOs::MacOs, name, &f),
                (
                    "open".to_string(),
                    os(&["-a", app, "/Users/me/Pictures/IMG_0001.ARW"])
                )
            );
        }
        assert_eq!(
            first_command(TargetOs::MacOs, "System default", &f),
            ("open".to_string(), os(&["/Users/me/Pictures/IMG_0001.ARW"]))
        );
    }

    #[test]
    fn req_file_3_windows_commands() {
        let f = PathBuf::from(r"D:\Shoot & Co\IMG_0001.NEF");
        let apps = default_apps(TargetOs::Windows);
        assert_eq!(
            apps[0].programs,
            [r"C:\Program Files\darktable\bin\darktable.exe", "darktable"]
        );
        assert_eq!(
            apps[1].programs[0],
            r"C:\Program Files\RawTherapee\*\rawtherapee.exe"
        );
        assert!(apps[2].programs.iter().all(|p| p.contains("gimp")));
        assert_eq!(apps[3].programs, ["explorer.exe"]);
        // The name, `&` and all, is one argument: no `cmd /c` that could split it.
        for a in &apps {
            assert_eq!(a.args_for(&f), os(&[r"D:\Shoot & Co\IMG_0001.NEF"]));
            assert!(a.programs.iter().all(|p| !p.contains("cmd")));
        }
    }

    #[test]
    fn req_file_3_configured_command_places_the_file_where_asked() {
        let cmd: Vec<String> = ["my-editor", "--open", "{file}", "--new-window"]
            .map(String::from)
            .to_vec();
        let a = ExternalApp::from_command("Mine", &cmd).unwrap();
        assert_eq!(a.programs, ["my-editor"]);
        assert_eq!(
            a.args_for(Path::new("/x/y.dng")),
            os(&["--open", "/x/y.dng", "--new-window"])
        );
        let b = ExternalApp::from_command("Mine", &["ed".to_string()]).unwrap();
        assert_eq!(b.args_for(Path::new("/x/y.dng")), os(&["/x/y.dng"]));
        assert_eq!(ExternalApp::from_command("Empty", &[]), None);
        assert_eq!(ExternalApp::from_command("Blank", &[" ".into()]), None);
    }

    #[test]
    fn req_file_3_program_paths_are_told_from_names() {
        assert!(is_path(r"C:\Program Files\x.exe"));
        assert!(is_path("/usr/bin/gimp"));
        assert!(!is_path("gimp"));
    }
}
