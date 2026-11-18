//! Preview sources and the loupe badge (SPEC §3, VIEW-2, VIEW-3).

/// Where the displayed pixels came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PreviewSource {
    /// Largest embedded preview, but its long edge is < 50 % of the sensor's.
    EmbeddedLowRes,
    /// Largest embedded preview.
    Embedded,
    /// The same-stem JPEG paired with a RAW.
    PairedJpeg,
    /// A real decode of the RAW sensor data.
    RawDecode,
    /// A standalone JPEG item, decoded in full (ADR 0008).
    Jpeg,
    /// Nothing displayable (e.g. standalone HEIF in v1, or a file without previews).
    Unavailable,
}

impl PreviewSource {
    /// The text of the preview-source badge in the loupe (VIEW-3).
    pub fn badge(self) -> &'static str {
        match self {
            Self::EmbeddedLowRes => "EMBEDDED · LOW-RES",
            Self::Embedded => "EMBEDDED",
            Self::PairedJpeg => "PAIRED JPEG",
            Self::RawDecode => "RAW",
            Self::Jpeg => "JPEG",
            Self::Unavailable => "NO PREVIEW",
        }
    }

    /// Source for an embedded preview with the given long edges (SPEC §3). An unknown sensor
    /// size can't prove the preview is low-res, so it counts as a normal embedded preview.
    pub fn for_embedded(preview_long_edge: u32, sensor_long_edge: Option<u32>) -> Self {
        match sensor_long_edge {
            Some(s) if is_low_res(preview_long_edge, s) => Self::EmbeddedLowRes,
            _ => Self::Embedded,
        }
    }
}

/// LOW-RES rule of SPEC §3: the preview's long edge is < 50 % of the sensor's long edge.
pub fn is_low_res(preview_long_edge: u32, sensor_long_edge: u32) -> bool {
    u64::from(preview_long_edge) * 2 < u64::from(sensor_long_edge)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn req_view_3_badge_texts() {
        use PreviewSource::*;
        let got: Vec<_> = [EmbeddedLowRes, Embedded, PairedJpeg, RawDecode, Unavailable]
            .iter()
            .map(|s| s.badge())
            .collect();
        assert_eq!(
            got,
            [
                "EMBEDDED · LOW-RES",
                "EMBEDDED",
                "PAIRED JPEG",
                "RAW",
                "NO PREVIEW"
            ]
        );
    }

    #[test]
    fn req_view_2_low_res_threshold_is_half_the_sensor_long_edge() {
        assert!(is_low_res(1616, 4912));
        assert!(!is_low_res(1920, 3776)); // 50.8 %
        assert!(!is_low_res(2000, 4000)); // exactly 50 % is not "< 50 %"
        assert!(is_low_res(1999, 4000));
        assert_eq!(
            PreviewSource::for_embedded(1616, Some(4912)),
            PreviewSource::EmbeddedLowRes
        );
        assert_eq!(
            PreviewSource::for_embedded(1616, None),
            PreviewSource::Embedded
        );
    }
}
