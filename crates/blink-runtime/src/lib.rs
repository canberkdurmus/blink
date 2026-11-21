//! Blink runtime: effect executor, worker pools, prefetch, caches, watcher, atomic writes.
//!
//! All file-system access goes through the [`fs::Fs`] trait.

pub mod atomic;
pub mod config;
pub mod fs;
pub mod inspect;
pub mod loader;
pub mod marks;
pub mod paths;
pub mod scan;
pub mod sidecar;

/// Version string shared by the GUI and CLI.
pub fn version() -> &'static str {
    blink_core::VERSION
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_matches_core() {
        assert_eq!(super::version(), blink_core::VERSION);
    }
}
