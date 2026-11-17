//! Blink decode: embedded preview extraction, RAW and JPEG decoding, orientation,
//! colour conversion, histogram and clipping. Read-only access to source files.

/// Crate marker so dependants can confirm linkage.
pub fn backend_name() -> &'static str {
    "none"
}

#[cfg(test)]
mod tests {
    #[test]
    fn backend_name_is_set() {
        assert!(!super::backend_name().is_empty());
    }
}
