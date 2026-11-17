//! `cargo xtask corpus fetch`: download the corpus listed in `corpus/manifest.toml` into the
//! git-ignored cache (`corpus/cache/`, or `$BLINK_CORPUS_DIR`) and verify every SHA-256.

use crate::{Result, root};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct Manifest {
    #[serde(default, rename = "file")]
    pub files: Vec<Entry>,
}

#[derive(Debug, Deserialize)]
pub struct Entry {
    pub path: String,
    pub sha256: String,
    pub url: String,
    pub license: String,
    pub camera: String,
}

pub fn cache_dir() -> PathBuf {
    std::env::var_os("BLINK_CORPUS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root().join("corpus").join("cache"))
}

pub fn load_manifest() -> Result<Manifest> {
    let p = root().join("corpus").join("manifest.toml");
    let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    toml::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn file_matches(path: &Path, sha: &str) -> bool {
    std::fs::read(path).is_ok_and(|b| sha256_hex(&b).eq_ignore_ascii_case(sha))
}

pub fn fetch() -> Result {
    let manifest = load_manifest()?;
    let dir = cache_dir();
    let mut downloaded = 0;
    for e in &manifest.files {
        if e.license.trim().is_empty() {
            return Err(format!("{}: no license recorded", e.path));
        }
        let dest = dir.join(&e.path);
        if file_matches(&dest, &e.sha256) {
            continue;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        eprintln!("fetching {} ({})", e.path, e.camera);
        let tmp = dest.with_extension("part");
        let tmp_s = tmp.to_string_lossy().into_owned();
        // curl ships with every CI image (including Windows 10+), so no HTTP crate is needed.
        crate::run_cmd("curl", &["-fsSL", "--retry", "3", "-o", &tmp_s, &e.url])?;
        let body = std::fs::read(&tmp).map_err(|err| err.to_string())?;
        let got = sha256_hex(&body);
        if !got.eq_ignore_ascii_case(&e.sha256) {
            return Err(format!(
                "{}: sha256 mismatch: expected {}, got {got}",
                e.path, e.sha256
            ));
        }
        std::fs::rename(&tmp, &dest).map_err(|err| err.to_string())?;
        downloaded += 1;
    }
    println!(
        "corpus: {} files verified in {} ({downloaded} downloaded)",
        manifest.files.len(),
        dir.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn manifest_parses() {
        load_manifest().unwrap();
    }
}
