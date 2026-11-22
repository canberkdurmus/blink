//! Blink core: state, actions, the pure reducer, filters, sort, keymap resolution and undo.
//!
//! This crate performs no I/O (SPEC §8.2).
#![forbid(unsafe_code)]

pub mod filter;
pub mod formats;
pub mod info;
pub mod item;
pub mod preview;
pub mod sort;
pub mod state;
pub mod view;

/// Application name as shown to users (SPEC D-7).
pub const APP_NAME: &str = "Blink";

/// Version of the Blink workspace.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_name_is_blink() {
        assert_eq!(APP_NAME, "Blink");
        assert!(!VERSION.is_empty());
    }
}
