//! Where Blink keeps its configuration and caches (PLAT-2).
//!
//! Standard mode uses the platform locations from the `directories` crate for the app ID
//! `io.github.canberkdurmus.Blink` (SPEC D-7). Portable mode is on when a file named
//! `blink-portable` sits next to the binary; config and caches then live in `blink-data/` next to
//! the binary, so the app leaves no trace elsewhere.

use crate::fs::Fs;
use std::path::{Path, PathBuf};

/// File whose presence next to the executable turns on portable mode.
pub const PORTABLE_MARKER: &str = "blink-portable";
/// Directory next to the executable that holds config and caches in portable mode.
pub const PORTABLE_DATA_DIR: &str = "blink-data";

/// Resolved locations for config and caches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub portable: bool,
}

impl AppPaths {
    /// Resolve paths for the executable at `exe`. Checks for the portable marker through `fs`.
    /// Returns `None` only when not portable and the OS reports no home directory.
    pub fn resolve(fs: &dyn Fs, exe: &Path) -> Option<Self> {
        if let Some(dir) = exe.parent()
            && fs
                .metadata(&dir.join(PORTABLE_MARKER))
                .is_ok_and(|m| !m.is_dir)
        {
            return Some(Self::portable(dir));
        }
        Self::standard()
    }

    /// Resolve paths for the running executable.
    pub fn for_current_exe(fs: &dyn Fs) -> Option<Self> {
        let exe = std::env::current_exe().ok()?;
        Self::resolve(fs, &exe)
    }

    /// Portable layout rooted at the executable's directory.
    pub fn portable(exe_dir: &Path) -> Self {
        let data = exe_dir.join(PORTABLE_DATA_DIR);
        Self {
            config_dir: data.join("config"),
            cache_dir: data.join("cache"),
            portable: true,
        }
    }

    /// Platform-standard layout.
    pub fn standard() -> Option<Self> {
        let d = directories::ProjectDirs::from("io.github", "canberkdurmus", "Blink")?;
        Some(Self {
            config_dir: d.config_dir().to_path_buf(),
            cache_dir: d.cache_dir().to_path_buf(),
            portable: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::MemFs;

    fn exe() -> PathBuf {
        PathBuf::from("/opt/blink/bin").join(if cfg!(windows) { "blink.exe" } else { "blink" })
    }

    #[test]
    fn req_plat_2_portable_marker_puts_config_and_cache_next_to_binary() {
        let fs = MemFs::new();
        fs.add_file(
            PathBuf::from("/opt/blink/bin").join(PORTABLE_MARKER),
            b"".to_vec(),
        );
        let p = AppPaths::resolve(&fs, &exe()).unwrap();
        assert!(p.portable);
        let data = PathBuf::from("/opt/blink/bin").join(PORTABLE_DATA_DIR);
        assert_eq!(p.config_dir, data.join("config"));
        assert_eq!(p.cache_dir, data.join("cache"));
    }

    #[test]
    fn req_plat_2_without_marker_uses_platform_standard_dirs() {
        let fs = MemFs::new();
        fs.add_file(exe(), b"".to_vec());
        let p = AppPaths::resolve(&fs, &exe()).unwrap();
        assert!(!p.portable);
        assert_eq!(Some(p.clone()), AppPaths::standard());
        assert_ne!(p.config_dir, p.cache_dir);
        assert!(!p.config_dir.starts_with("/opt/blink"));
        let cfg = p.config_dir.to_string_lossy().into_owned();
        let cache = p.cache_dir.to_string_lossy().into_owned();
        if cfg!(target_os = "linux") {
            assert!(cfg.ends_with("/blink"), "{cfg}");
            assert!(cache.ends_with("/blink"), "{cache}");
            assert_ne!(cfg, cache);
        } else if cfg!(target_os = "macos") {
            assert!(cfg.contains("Library/Application Support/io.github.canberkdurmus.Blink"));
            assert!(cache.contains("Library/Caches/io.github.canberkdurmus.Blink"));
        } else if cfg!(windows) {
            assert!(cfg.contains("canberkdurmus\\Blink"), "{cfg}");
            assert!(cache.contains("canberkdurmus\\Blink"), "{cache}");
        }
    }

    #[test]
    fn req_plat_2_marker_must_be_a_file_next_to_the_binary() {
        let fs = MemFs::new();
        // A directory with the marker name does not count, nor does a marker elsewhere.
        fs.add_dir(PathBuf::from("/opt/blink/bin").join(PORTABLE_MARKER));
        fs.add_file(
            PathBuf::from("/opt/blink").join(PORTABLE_MARKER),
            b"".to_vec(),
        );
        assert!(!AppPaths::resolve(&fs, &exe()).unwrap().portable);
    }
}
