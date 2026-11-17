//! Blink XMP: parse and write sidecars while preserving foreign content.
//!
//! This crate performs no I/O: bytes in, bytes out (SPEC §8.2).
#![forbid(unsafe_code)]

/// The XMP basic namespace URI (SPEC §6).
pub const NS_XMP: &str = "http://ns.adobe.com/xap/1.0/";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xmp_namespace_matches_spec() {
        assert_eq!(NS_XMP, "http://ns.adobe.com/xap/1.0/");
    }
}
