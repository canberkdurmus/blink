//! The user config file `config.toml` in the config directory (PLAT-2), read through [`Fs`].
//!
//! ```toml
//! pick_value = 1          # RATE-2 / D-2: the star value a pick maps to, 1–5
//! sidecar_style = "adobe" # D-3: "adobe" (IMG.xmp) or "darktable" (IMG.ARW.xmp)
//! auto_advance = false    # RATE-1
//!
//! [labels]                # SPEC §6: xmp:Label values written for each colour
//! red = "Red"
//! ```
//!
//! A missing file means defaults. A broken file also means defaults, plus a message for the
//! toast area: Blink never refuses to start because of its config.

use crate::fs::Fs;
use crate::sidecar::SidecarStyle;
use blink_core::state::Settings;
use serde::Deserialize;
use std::path::Path;

/// File name inside the config directory.
pub const CONFIG_FILE: &str = "config.toml";

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    pick_value: Option<i64>,
    sidecar_style: Option<String>,
    auto_advance: Option<bool>,
    labels: Option<RawLabels>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLabels {
    red: Option<String>,
    yellow: Option<String>,
    green: Option<String>,
    blue: Option<String>,
    purple: Option<String>,
}

/// Everything the config file controls.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Config {
    pub settings: Settings,
    pub sidecar_style: SidecarStyle,
}

impl Config {
    /// Parse config text. `Err` names the problem; nothing is half-applied.
    pub fn parse(text: &str) -> Result<Self, String> {
        let raw: Raw = toml::from_str(text).map_err(|e| e.message().to_string())?;
        let mut c = Config::default();
        if let Some(p) = raw.pick_value {
            if !(1..=5).contains(&p) {
                return Err(format!("pick_value must be 1–5, not {p}"));
            }
            c.settings.pick_value = p as i8;
        }
        if let Some(s) = raw.sidecar_style {
            c.sidecar_style = match s.as_str() {
                "adobe" => SidecarStyle::Adobe,
                "darktable" => SidecarStyle::Darktable,
                _ => {
                    return Err(format!(
                        "sidecar_style must be \"adobe\" or \"darktable\", not {s:?}"
                    ));
                }
            };
        }
        if let Some(a) = raw.auto_advance {
            c.settings.auto_advance = a;
        }
        if let Some(l) = raw.labels {
            for (i, v) in [l.red, l.yellow, l.green, l.blue, l.purple]
                .into_iter()
                .enumerate()
            {
                if let Some(v) = v {
                    let v = v.trim().to_string();
                    if v.is_empty() {
                        return Err("label names must not be empty".into());
                    }
                    c.settings.label_names[i] = v;
                }
            }
        }
        Ok(c)
    }

    /// Load `config.toml` from `dir`. Returns the config and, if the file was unreadable or
    /// invalid, a message saying defaults are used.
    pub fn load(fs: &dyn Fs, dir: &Path) -> (Self, Option<String>) {
        let p = dir.join(CONFIG_FILE);
        let bytes = match fs.read(&p) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Self::default(), None),
            Err(e) => {
                return (
                    Self::default(),
                    Some(format!("{}: {e}; using defaults", p.display())),
                );
            }
        };
        let parsed = std::str::from_utf8(&bytes)
            .map_err(|e| e.to_string())
            .and_then(Self::parse);
        match parsed {
            Ok(c) => (c, None),
            Err(e) => (
                Self::default(),
                Some(format!("{}: {e}; using defaults", p.display())),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::MemFs;

    #[test]
    fn req_rate_2_pick_value_is_configurable_1_to_5_and_defaults_to_1() {
        assert_eq!(Config::parse("").unwrap().settings.pick_value, 1);
        for v in 1..=5 {
            assert_eq!(
                Config::parse(&format!("pick_value = {v}"))
                    .unwrap()
                    .settings
                    .pick_value,
                v as i8
            );
        }
        for bad in [
            "pick_value = 0",
            "pick_value = 6",
            "pick_value = -1",
            "pick_value = \"3\"",
        ] {
            assert!(Config::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn req_rate_2_config_file_sets_style_auto_advance_and_label_names() {
        let c = Config::parse(
            "pick_value = 3\nsidecar_style = \"darktable\"\nauto_advance = true\n[labels]\npurple = \"Violet\"\n",
        )
        .unwrap();
        assert_eq!(c.settings.pick_value, 3);
        assert_eq!(c.sidecar_style, SidecarStyle::Darktable);
        assert!(c.settings.auto_advance);
        assert_eq!(c.settings.label_names[4], "Violet");
        assert_eq!(c.settings.label_names[0], "Red");
        assert!(Config::parse("sidecar_style = \"lr\"").is_err());
        assert!(Config::parse("bogus = 1").is_err());
        assert!(Config::parse("[labels]\nred = \" \"").is_err());
    }

    #[test]
    fn req_rate_2_missing_or_broken_config_falls_back_to_defaults() {
        let fs = MemFs::new();
        assert_eq!(
            Config::load(&fs, Path::new("/c")),
            (Config::default(), None)
        );
        fs.add_file("/c/config.toml", b"pick_value = 9".to_vec());
        let (c, msg) = Config::load(&fs, Path::new("/c"));
        assert_eq!(c, Config::default());
        assert!(msg.unwrap().contains("pick_value must be 1–5"));
        fs.add_file("/c/config.toml", b"pick_value = 2".to_vec());
        assert_eq!(Config::load(&fs, Path::new("/c")).0.settings.pick_value, 2);
    }
}
