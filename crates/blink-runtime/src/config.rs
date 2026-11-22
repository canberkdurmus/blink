//! The user's `config.toml` in the config directory (PLAT-2): pick value (D-2, RATE-2),
//! auto-advance (RATE-1), `xmp:Label` names (SPEC §6) and the new-sidecar style (D-3).
//!
//! ```toml
//! pick_value = 1          # 1–5
//! auto_advance = false
//! sidecar_style = "adobe" # or "darktable"
//! rejects = "folder"      # apply rejects: "_rejected/" folder, or "trash" (FILE-1)
//! cache_mb = 2048       # decode cache cap in MiB (PERF-8), at least 1024
//! [[open_with]]          # FILE-3; replaces the default list when present
//! name = "ART"
//! command = ["ART", "{file}"]   # program, then arguments; {file} is the item's path
//! [labels]
//! red = "Red"
//! purple = "Purple"
//! ```
//!
//! A missing file means the defaults. A file that can't be used keeps the defaults for the
//! broken keys and reports why, so the GUI can show it in a toast.

use crate::fs::Fs;
use crate::sidecar::SidecarStyle;
use blink_core::external::{ExternalApp, TargetOs, default_apps};
use blink_core::state::{LabelColor, MarkPrefs};
use std::path::Path;

/// Smallest decode cache: the preview and full decode of a 100 MP RAW (~700 MB at 7 bytes
/// per pixel in the cache's accounting) still fit (PERF-8).
pub const MIN_CACHE_MB: i64 = 1024;
/// Largest decode cache accepted (1 TiB), so the byte count can't overflow.
pub const MAX_CACHE_MB: i64 = 1 << 20;

/// File name inside the config directory.
pub const CONFIG_FILE: &str = "config.toml";

/// Everything the config file sets.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub marks: MarkPrefs,
    pub sidecar_style: SidecarStyle,
    /// Decode cache cap in bytes (PERF-8); `None`: [`crate::loader::DEFAULT_CACHE_BYTES`].
    pub cache_bytes: Option<usize>,
    /// The "Open in" list (FILE-3); `None`: the defaults for this OS.
    pub open_with: Option<Vec<ExternalApp>>,
}

impl Config {
    /// The "Open in" list: configured, or the defaults for this OS.
    pub fn external_apps(&self) -> Vec<ExternalApp> {
        self.open_with
            .clone()
            .unwrap_or_else(|| default_apps(TargetOs::current()))
    }
}

fn parse_open_with(v: &toml::Value, errs: &mut Vec<String>) -> Option<Vec<ExternalApp>> {
    let Some(list) = v.as_array() else {
        errs.push("open_with must be a list of [[open_with]] tables".into());
        return None;
    };
    let mut apps = Vec::new();
    for (i, e) in list.iter().enumerate() {
        let n = i + 1;
        let Some(t) = e.as_table() else {
            errs.push(format!("open_with entry {n} must be a table"));
            continue;
        };
        if let Some(k) = t.keys().find(|k| *k != "name" && *k != "command") {
            errs.push(format!("unknown key open_with.{k} (entry {n})"));
        }
        let name = t.get("name").and_then(|v| v.as_str()).map(str::trim);
        let command: Option<Vec<String>> = t
            .get("command")
            .and_then(|v| v.as_array())
            .and_then(|a| a.iter().map(|x| x.as_str().map(str::to_string)).collect());
        match (name, command) {
            (Some(name), Some(cmd)) if !name.is_empty() => {
                match ExternalApp::from_command(name, &cmd) {
                    Some(app) => apps.push(app),
                    None => errs.push(format!("open_with entry {n}: command needs a program")),
                }
            }
            _ => errs.push(format!(
                "open_with entry {n} needs a name and a command list of strings"
            )),
        }
    }
    // Every entry broken: keep the defaults rather than an empty menu.
    (!apps.is_empty()).then_some(apps)
}

const LABEL_KEYS: [&str; 5] = ["red", "yellow", "green", "blue", "purple"];

impl Config {
    /// Parse `text`. Returns the config and one message per problem (unknown keys, wrong types,
    /// out-of-range values); every problem leaves that setting at its default.
    pub fn parse(text: &str) -> (Self, Vec<String>) {
        let mut c = Self::default();
        let mut errs = Vec::new();
        let table = match text.parse::<toml::Table>() {
            Ok(t) => t,
            Err(e) => return (c, vec![format!("{CONFIG_FILE}: {}", e.message())]),
        };
        for (k, v) in &table {
            match (k.as_str(), v) {
                ("pick_value", toml::Value::Integer(n)) if (1..=5).contains(n) => {
                    c.marks.pick_value = *n as i8
                }
                ("pick_value", _) => errs.push("pick_value must be a whole number 1–5".into()),
                ("auto_advance", toml::Value::Boolean(b)) => c.marks.auto_advance = *b,
                ("auto_advance", _) => errs.push("auto_advance must be true or false".into()),
                ("sidecar_style", toml::Value::String(s)) if s == "adobe" => {
                    c.sidecar_style = SidecarStyle::Adobe
                }
                ("sidecar_style", toml::Value::String(s)) if s == "darktable" => {
                    c.sidecar_style = SidecarStyle::Darktable
                }
                ("sidecar_style", _) => {
                    errs.push("sidecar_style must be \"adobe\" or \"darktable\"".into())
                }
                ("rejects", toml::Value::String(s)) if s == "folder" => {
                    c.marks.reject_to_trash = false
                }
                ("rejects", toml::Value::String(s)) if s == "trash" => {
                    c.marks.reject_to_trash = true
                }
                ("rejects", _) => errs.push("rejects must be \"folder\" or \"trash\"".into()),
                ("labels", toml::Value::Table(t)) => {
                    for (lk, lv) in t {
                        let slot = LABEL_KEYS.iter().position(|x| x == lk);
                        match (slot, lv) {
                            (Some(i), toml::Value::String(s)) if !s.trim().is_empty() => {
                                c.marks.label_names[LabelColor::ALL[i].index()] = s.clone()
                            }
                            (Some(_), _) => {
                                errs.push(format!("labels.{lk} must be a non-empty string"))
                            }
                            (None, _) => errs.push(format!("unknown key labels.{lk}")),
                        }
                    }
                }
                ("labels", _) => errs.push("labels must be a table".into()),
                ("cache_mb", toml::Value::Integer(n))
                    if (MIN_CACHE_MB..=MAX_CACHE_MB).contains(n) =>
                {
                    c.cache_bytes = Some(*n as usize * (1 << 20))
                }
                ("cache_mb", _) => errs.push(format!(
                    "cache_mb must be a whole number {MIN_CACHE_MB}–{MAX_CACHE_MB}"
                )),
                ("open_with", v) => c.open_with = parse_open_with(v, &mut errs),
                (other, _) => errs.push(format!("unknown key {other}")),
            }
        }
        // Two colours with one name could not be told apart when read back.
        for i in 0..5 {
            for j in 0..i {
                if c.marks.label_names[i].eq_ignore_ascii_case(&c.marks.label_names[j]) {
                    errs.push(format!(
                        "labels.{} and labels.{} have the same name",
                        LABEL_KEYS[j], LABEL_KEYS[i]
                    ));
                    c.marks.label_names = MarkPrefs::default().label_names;
                }
            }
        }
        let errs = errs
            .into_iter()
            .map(|e| format!("{CONFIG_FILE}: {e}"))
            .collect();
        (c, errs)
    }

    /// Read `config_dir/config.toml` through `fs`. Missing file: defaults, no message.
    pub fn load(fs: &dyn Fs, config_dir: &Path) -> (Self, Vec<String>) {
        let p = config_dir.join(CONFIG_FILE);
        match fs.read(&p) {
            Ok(b) => match String::from_utf8(b) {
                Ok(t) => Self::parse(&t),
                Err(_) => (Self::default(), vec![format!("{CONFIG_FILE}: not UTF-8")]),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Self::default(), vec![]),
            Err(e) => (Self::default(), vec![format!("{}: {e}", p.display())]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::MemFs;

    #[test]
    fn req_perf_8_cache_cap_defaults_to_2_gb_and_is_configurable() {
        assert_eq!(crate::loader::DEFAULT_CACHE_BYTES, 2 << 30);
        assert_eq!(Config::parse("").0.cache_bytes, None);
        let (c, errs) = Config::parse("cache_mb = 4096");
        assert_eq!((c.cache_bytes, errs.len()), (Some(4 << 30), 0));
        assert_eq!(
            Config::parse("cache_mb = 1024").0.cache_bytes,
            Some(1 << 30)
        );
        for bad in [
            "cache_mb = 1023",
            "cache_mb = \"2G\"",
            "cache_mb = 1.5",
            "cache_mb = -1",
        ] {
            let (c, errs) = Config::parse(bad);
            assert_eq!((c.cache_bytes, errs.len()), (None, 1), "{bad}");
        }
    }

    #[test]
    fn req_file_3_open_with_list_is_configurable_and_defaults_per_os() {
        let (c, errs) = Config::parse("");
        assert!(errs.is_empty());
        assert_eq!(c.external_apps(), default_apps(TargetOs::current()));
        let (c, errs) = Config::parse(
            r#"
[[open_with]]
name = "ART"
command = ["ART", "{file}"]
[[open_with]]
name = "Viewer"
command = ["/usr/bin/viewer", "--fullscreen"]
"#,
        );
        assert!(errs.is_empty(), "{errs:?}");
        let apps = c.external_apps();
        assert_eq!(
            apps.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            ["ART", "Viewer"]
        );
        assert_eq!(apps[1].programs, ["/usr/bin/viewer"]);
        assert_eq!(apps[1].args, ["--fullscreen"]);
        for bad in [
            "open_with = 3",
            "[[open_with]]\nname = \"x\"",
            "[[open_with]]\nname = \"x\"\ncommand = []",
            "[[open_with]]\nname = \"x\"\ncommand = [1]",
            "[[open_with]]\ncommand = [\"x\"]",
        ] {
            let (c, errs) = Config::parse(bad);
            assert_eq!(
                c.external_apps(),
                default_apps(TargetOs::current()),
                "{bad}"
            );
            assert_eq!(errs.len(), 1, "{bad}: {errs:?}");
        }
        // A broken entry is reported; the good ones are kept.
        let (c, errs) = Config::parse(
            "[[open_with]]\nname = \"a\"\ncommand = [\"a\"]\nextra = 1\n[[open_with]]\nname = \"b\"",
        );
        assert_eq!(c.external_apps().len(), 1);
        assert_eq!(errs.len(), 2, "{errs:?}");
    }

    #[test]
    fn req_rate_2_pick_value_is_configurable_1_to_5_and_defaults_to_1() {
        assert_eq!(Config::parse("").0.marks.pick_value, 1);
        for n in 1..=5 {
            let (c, errs) = Config::parse(&format!("pick_value = {n}"));
            assert_eq!((c.marks.pick_value, errs.len()), (n, 0));
        }
        for bad in [
            "pick_value = 0",
            "pick_value = 6",
            "pick_value = \"3\"",
            "pick_value = 2.5",
        ] {
            let (c, errs) = Config::parse(bad);
            assert_eq!(c.marks.pick_value, 1, "{bad}");
            assert_eq!(
                errs,
                ["config.toml: pick_value must be a whole number 1–5"],
                "{bad}"
            );
        }
    }

    #[test]
    fn req_rate_2_config_sets_auto_advance_labels_and_style_and_reports_problems() {
        let (c, errs) = Config::parse(
            "auto_advance = true\nsidecar_style = \"darktable\"\n[labels]\nred = \"Rot\"\npurple = \"Lila\"\n",
        );
        assert!(errs.is_empty(), "{errs:?}");
        assert!(c.marks.auto_advance);
        assert_eq!(c.sidecar_style, SidecarStyle::Darktable);
        assert_eq!(c.marks.label_name(LabelColor::Red), "Rot");
        assert_eq!(c.marks.label_name(LabelColor::Yellow), "Yellow");
        assert_eq!(c.marks.label_name(LabelColor::Purple), "Lila");
        let (c, errs) =
            Config::parse("colour = 1\nsidecar_style = \"lr\"\n[labels]\nred = \"Blue\"");
        assert_eq!(c, Config::default());
        assert_eq!(
            errs,
            [
                "config.toml: unknown key colour",
                "config.toml: sidecar_style must be \"adobe\" or \"darktable\"",
                "config.toml: labels.red and labels.blue have the same name",
            ]
        );
        assert_eq!(Config::parse("pick_value = ").1.len(), 1);
    }

    #[test]
    fn req_rate_2_config_is_read_from_the_config_dir_and_missing_means_defaults() {
        let fs = MemFs::new();
        assert_eq!(
            Config::load(&fs, Path::new("/cfg")),
            (Config::default(), vec![])
        );
        fs.add_file("/cfg/config.toml", b"pick_value = 3\n".to_vec());
        let (c, errs) = Config::load(&fs, Path::new("/cfg"));
        assert_eq!((c.marks.pick_value, errs.len()), (3, 0));
    }

    #[test]
    fn req_file_1_config_rejects_selects_the_folder_or_the_trash() {
        assert!(!Config::parse("").0.marks.reject_to_trash);
        let (c, e) = Config::parse("rejects = \"trash\"");
        assert!(c.marks.reject_to_trash && e.is_empty());
        let (c, e) = Config::parse("rejects = \"folder\"");
        assert!(!c.marks.reject_to_trash && e.is_empty());
        let (c, e) = Config::parse("rejects = \"bin\"");
        assert!(!c.marks.reject_to_trash);
        assert_eq!(e, ["config.toml: rejects must be \"folder\" or \"trash\""]);
    }
}
