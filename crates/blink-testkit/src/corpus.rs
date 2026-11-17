//! Load `corpus/manifest.toml` and locate the fetched files.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct Manifest {
    #[serde(rename = "file")]
    pub files: Vec<Entry>,
}

#[derive(Debug, Deserialize)]
pub struct Entry {
    pub path: String,
    pub sha256: String,
    pub camera: String,
    pub expected: Expected,
}

#[derive(Debug, Deserialize)]
pub struct Expected {
    pub preview_w: u32,
    pub preview_h: u32,
    pub badge: String,
    pub sensor_w: u32,
    pub sensor_h: u32,
    #[serde(default)]
    pub exif: BTreeMap<String, toml::Value>,
}

/// Workspace root.
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/blink-testkit has a workspace root")
        .to_path_buf()
}

/// Where `cargo xtask corpus fetch` puts the files (`$BLINK_CORPUS_DIR` or `corpus/cache`).
pub fn cache_dir() -> PathBuf {
    std::env::var_os("BLINK_CORPUS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root().join("corpus").join("cache"))
}

pub fn manifest() -> Manifest {
    let p = workspace_root().join("corpus").join("manifest.toml");
    let text = std::fs::read_to_string(&p).expect("read corpus/manifest.toml");
    toml::from_str(&text).expect("parse corpus/manifest.toml")
}

impl Entry {
    /// Absolute path of the cached file. Panics with a hint if the corpus wasn't fetched.
    pub fn file(&self) -> PathBuf {
        let p = cache_dir().join(&self.path);
        assert!(
            p.exists(),
            "{} missing: run `cargo xtask corpus fetch`",
            p.display()
        );
        p
    }

    pub fn bytes(&self) -> Vec<u8> {
        std::fs::read(self.file()).expect("read corpus file")
    }
}
