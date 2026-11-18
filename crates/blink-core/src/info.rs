//! The info panel (VIEW-7): which fields, in which order, formatted how. Pure: the runtime fills
//! [`InfoInput`] from the decoder's EXIF reader.

use crate::preview::PreviewSource;

/// Raw values for the info panel. `None`: the file doesn't say.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InfoInput {
    pub file_name: String,
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens: Option<String>,
    pub focal_length_mm: Option<f64>,
    pub exposure_time_s: Option<f64>,
    pub f_number: Option<f64>,
    pub iso: Option<u32>,
    pub exposure_compensation_ev: Option<f64>,
    /// EXIF `YYYY:MM:DD HH:MM:SS`.
    pub date_time_original: Option<String>,
    /// EXIF `±HH:MM`.
    pub offset_time_original: Option<String>,
    /// Sensor (recommended crop) dimensions.
    pub dimensions: Option<(u32, u32)>,
    pub preview_source: Option<PreviewSource>,
    pub preview_size: Option<(u32, u32)>,
}

/// One row of the panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoRow {
    pub label: &'static str,
    /// `—` when unknown.
    pub value: String,
}

const UNKNOWN: &str = "—";

/// Camera body: the model, prefixed by the make unless the model already names it
/// (`Canon` + `Canon EOS 20D` → `Canon EOS 20D`; `SONY` + `NEX-5N` → `SONY NEX-5N`).
pub fn body(make: Option<&str>, model: Option<&str>) -> Option<String> {
    match (make, model) {
        (Some(mk), Some(md)) => {
            let first = mk.split_whitespace().next().unwrap_or(mk);
            if md.to_lowercase().starts_with(&first.to_lowercase()) {
                Some(md.to_string())
            } else {
                Some(format!("{mk} {md}"))
            }
        }
        (None, Some(md)) => Some(md.to_string()),
        (Some(mk), None) => Some(mk.to_string()),
        (None, None) => None,
    }
}

fn trim_num(v: f64, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

/// `1/200 s`, `0.36 s`, `2.5 s`, `30 s`.
pub fn shutter(t: f64) -> String {
    if t <= 0.0 || !t.is_finite() {
        return UNKNOWN.into();
    }
    if t >= 1.0 {
        return format!("{} s", trim_num(t, 1));
    }
    let n = (1.0 / t).round();
    if n >= 2.0 && ((1.0 / n) - t).abs() / t < 0.03 {
        format!("1/{n:.0} s")
    } else {
        format!("{} s", trim_num(t, 2))
    }
}

/// `+0.7 EV`, `-1 EV`, `0 EV`.
pub fn exposure_compensation(ev: f64) -> String {
    let r = (ev * 10.0).round() / 10.0;
    if r == 0.0 {
        "0 EV".into()
    } else {
        format!("{}{} EV", if r > 0.0 { "+" } else { "" }, trim_num(r, 1))
    }
}

/// `2012:07:15 16:30:58` + `+01:00` → `2012-07-15 16:30:58 +01:00`.
pub fn capture_time(dto: &str, offset: Option<&str>) -> String {
    let mut s = dto.trim().to_string();
    if s.len() >= 10 && s.as_bytes()[4] == b':' && s.as_bytes()[7] == b':' {
        s.replace_range(4..5, "-");
        s.replace_range(7..8, "-");
    }
    match offset {
        Some(o) if !o.trim().is_empty() => format!("{s} {}", o.trim()),
        _ => s,
    }
}

fn dims(d: (u32, u32)) -> String {
    format!("{} × {}", d.0, d.1)
}

impl InfoInput {
    /// The rows of the info panel, in display order.
    pub fn rows(&self) -> Vec<InfoRow> {
        let or = |v: Option<String>| v.unwrap_or_else(|| UNKNOWN.into());
        let preview = match (self.preview_source, self.preview_size) {
            (Some(s), Some(d)) => Some(format!("{} · {}", s.badge(), dims(d))),
            (Some(s), None) => Some(s.badge().to_string()),
            _ => None,
        };
        vec![
            InfoRow {
                label: "File",
                value: self.file_name.clone(),
            },
            InfoRow {
                label: "Body",
                value: or(body(self.make.as_deref(), self.model.as_deref())),
            },
            InfoRow {
                label: "Lens",
                value: or(self.lens.clone()),
            },
            InfoRow {
                label: "Focal length",
                value: or(self
                    .focal_length_mm
                    .map(|f| format!("{} mm", trim_num(f, 1)))),
            },
            InfoRow {
                label: "Shutter",
                value: or(self.exposure_time_s.map(shutter)),
            },
            InfoRow {
                label: "Aperture",
                value: or(self.f_number.map(|f| format!("f/{}", trim_num(f, 1)))),
            },
            InfoRow {
                label: "ISO",
                value: or(self.iso.map(|i| i.to_string())),
            },
            InfoRow {
                label: "Exposure comp.",
                value: or(self.exposure_compensation_ev.map(exposure_compensation)),
            },
            InfoRow {
                label: "Captured",
                value: or(self
                    .date_time_original
                    .as_deref()
                    .map(|d| capture_time(d, self.offset_time_original.as_deref()))),
            },
            InfoRow {
                label: "Dimensions",
                value: or(self.dimensions.map(dims)),
            },
            InfoRow {
                label: "Preview",
                value: or(preview),
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn req_view_7_formats_every_field() {
        let i = InfoInput {
            file_name: "DSC0001.ARW".into(),
            make: Some("SONY".into()),
            model: Some("NEX-5N".into()),
            lens: Some("E 18-55mm F3.5-5.6 OSS".into()),
            focal_length_mm: Some(18.0),
            exposure_time_s: Some(0.005),
            f_number: Some(7.1),
            iso: Some(100),
            exposure_compensation_ev: Some(-2.0 / 3.0),
            date_time_original: Some("2012:07:15 16:30:58".into()),
            offset_time_original: Some("+01:00".into()),
            dimensions: Some((4912, 3264)),
            preview_source: Some(PreviewSource::EmbeddedLowRes),
            preview_size: Some((1616, 1080)),
        };
        let got: Vec<(&str, String)> = i.rows().into_iter().map(|r| (r.label, r.value)).collect();
        let want: Vec<(&str, String)> = [
            ("File", "DSC0001.ARW"),
            ("Body", "SONY NEX-5N"),
            ("Lens", "E 18-55mm F3.5-5.6 OSS"),
            ("Focal length", "18 mm"),
            ("Shutter", "1/200 s"),
            ("Aperture", "f/7.1"),
            ("ISO", "100"),
            ("Exposure comp.", "-0.7 EV"),
            ("Captured", "2012-07-15 16:30:58 +01:00"),
            ("Dimensions", "4912 × 3264"),
            ("Preview", "EMBEDDED · LOW-RES · 1616 × 1080"),
        ]
        .into_iter()
        .map(|(a, b)| (a, b.to_string()))
        .collect();
        assert_eq!(got, want);
    }

    #[test]
    fn req_view_7_unknown_values_show_a_dash() {
        let rows = InfoInput {
            file_name: "x.DNG".into(),
            ..Default::default()
        }
        .rows();
        assert_eq!(rows.len(), 11);
        assert!(rows[1..].iter().all(|r| r.value == "—"));
    }

    #[test]
    fn req_view_7_value_formatting_helpers() {
        assert_eq!(shutter(1.0 / 8000.0), "1/8000 s");
        assert_eq!(shutter(0.25), "1/4 s");
        assert_eq!(shutter(0.357_142_857), "0.36 s");
        assert_eq!(shutter(2.5), "2.5 s");
        assert_eq!(shutter(30.0), "30 s");
        assert_eq!(exposure_compensation(0.0), "0 EV");
        assert_eq!(exposure_compensation(1.0 / 3.0), "+0.3 EV");
        assert_eq!(exposure_compensation(-1.0), "-1 EV");
        assert_eq!(
            capture_time("2024:06:29 15:38:47", None),
            "2024-06-29 15:38:47"
        );
        assert_eq!(
            body(Some("Canon"), Some("Canon EOS R6")).unwrap(),
            "Canon EOS R6"
        );
        assert_eq!(
            body(Some("NIKON CORPORATION"), Some("NIKON Z 6")).unwrap(),
            "NIKON Z 6"
        );
        assert_eq!(
            body(Some("FUJIFILM"), Some("X-S10")).unwrap(),
            "FUJIFILM X-S10"
        );
    }
}
