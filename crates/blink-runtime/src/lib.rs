//! Blink runtime: effect executor, worker pools, prefetch, caches, watcher, atomic writes.
//!
//! All file-system access goes through the `Fs` trait (added in M1+).

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
