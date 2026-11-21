//! The user's `config.toml` in the config directory (PLAT-2): pick value (D-2, RATE-2),
//! auto-advance (RATE-1), `xmp:Label` names (SPEC §6) and the new-sidecar style (D-3).
//!
//! ```toml
//! pick_value = 1          # 1–5
//! auto_advance = false
//! sidecar_style = "adobe" # or "darktable"
//! [labels]
//! red = "Red"
//! purple = "Purple"
//! ```
//!
//! A missing file means the defaults. A file that can't be used keeps the defaults for the
//! broken keys and reports why, so the GUI can show it in a toast.

use crate::fs::Fs;
use crate::sidecar::SidecarStyle;
use blink_core::state::{LabelColor, MarkPrefs};
use std::path::Path;

/// File name inside the config directory.
pub const CONFIG_FILE: &str = "config.toml";

/// Everything the config file sets.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub marks: MarkPrefs,
    pub sidecar_style: SidecarStyle,
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
}
