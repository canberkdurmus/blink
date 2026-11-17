//! Supported file kinds (OPEN-4, D-4, D-5). Pure: the RAW extension list comes from the
//! decode backend.

use std::collections::BTreeSet;

/// What a file is, from Blink's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FileKind {
    /// A proprietary RAW (ARW, CR3, NEF, …).
    Raw,
    /// Adobe DNG.
    Dng,
    Jpeg,
    /// HEIF/HEIC: listed and used for rating propagation, not decoded in v1 (D-4, D-5).
    Heif,
}

impl FileKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Raw => "RAW",
            Self::Dng => "DNG",
            Self::Jpeg => "JPEG",
            Self::Heif => "HEIF",
        }
    }

    /// RAW and DNG files are the primary member of a pair.
    pub fn is_raw_like(self) -> bool {
        matches!(self, Self::Raw | Self::Dng)
    }
}

pub const JPEG_EXTENSIONS: &[&str] = &["jpg", "jpeg", "jpe"];
pub const HEIF_EXTENSIONS: &[&str] = &["heic", "heif", "hif"];
pub const DNG_EXTENSIONS: &[&str] = &["dng"];

/// The set of extensions Blink opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Formats {
    raw: BTreeSet<String>,
}

impl Formats {
    /// `raw_extensions`: what the RAW backend can decode (any case). DNG, JPEG and HEIF are
    /// always supported. Video formats are never listed (SPEC §1 non-goals).
    pub fn new<I, S>(raw_extensions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let raw = raw_extensions
            .into_iter()
            .map(|s| s.as_ref().to_ascii_lowercase())
            .filter(|e| !DNG_EXTENSIONS.contains(&e.as_str()) && e != "crm")
            .collect();
        Self { raw }
    }

    /// Classify a file name by its extension (case-insensitive). Hidden files (`.name`) and
    /// AppleDouble files (`._name`) are never items.
    pub fn classify(&self, file_name: &str) -> Option<FileKind> {
        if file_name.starts_with('.') {
            return None;
        }
        let (_, ext) = file_name.rsplit_once('.')?;
        let ext = ext.to_ascii_lowercase();
        if DNG_EXTENSIONS.contains(&ext.as_str()) {
            Some(FileKind::Dng)
        } else if JPEG_EXTENSIONS.contains(&ext.as_str()) {
            Some(FileKind::Jpeg)
        } else if HEIF_EXTENSIONS.contains(&ext.as_str()) {
            Some(FileKind::Heif)
        } else if self.raw.contains(&ext) {
            Some(FileKind::Raw)
        } else {
            None
        }
    }

    /// Sorted RAW extensions (lower case).
    pub fn raw_extensions(&self) -> impl Iterator<Item = &str> {
        self.raw.iter().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn req_open_4_classifies_by_extension_case_insensitively() {
        let f = Formats::new(["ARW", "CR3", "DNG", "CRM"]);
        assert_eq!(f.classify("a.ARW"), Some(FileKind::Raw));
        assert_eq!(f.classify("a.cr3"), Some(FileKind::Raw));
        assert_eq!(f.classify("a.DNG"), Some(FileKind::Dng));
        assert_eq!(f.classify("a.JPG"), Some(FileKind::Jpeg));
        assert_eq!(f.classify("a.jpeg"), Some(FileKind::Jpeg));
        assert_eq!(f.classify("a.HEIC"), Some(FileKind::Heif));
        assert_eq!(f.classify("a.hif"), Some(FileKind::Heif));
        assert_eq!(f.classify("a.crm"), None, "video is a non-goal");
        assert_eq!(f.classify("a.xmp"), None);
        assert_eq!(f.classify("a.NEF"), None, "not in the backend list");
        assert_eq!(f.classify("._a.ARW"), None);
        assert_eq!(f.classify("noext"), None);
        assert_eq!(f.raw_extensions().collect::<Vec<_>>(), ["arw", "cr3"]);
    }
}
