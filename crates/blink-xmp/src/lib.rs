//! Blink XMP: parse and write sidecars while preserving foreign content.
//!
//! This crate performs no I/O: bytes in, bytes out (SPEC §8.2).
#![forbid(unsafe_code)]

mod write;

pub use write::{Edit, OwnedEdit, iso8601_utc, new_sidecar, write_owned};

use quick_xml::NsReader;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::{Namespace, ResolveResult};

/// The XMP basic namespace URI (SPEC §6).
pub const NS_XMP: &str = "http://ns.adobe.com/xap/1.0/";
/// RDF namespace.
pub const NS_RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

/// The fields Blink owns that it reads back (SPEC §6). `None`: not present (or not valid).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OwnedFields {
    /// `xmp:Rating`: −1 rejected, 0 unrated, 1–5 stars.
    pub rating: Option<i8>,
    /// `xmp:Label`, e.g. `Red`.
    pub label: Option<String>,
}

/// Why an XMP packet could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmpError(pub String);

impl std::fmt::Display for XmpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "xmp: {}", self.0)
    }
}

impl std::error::Error for XmpError {}

/// Parse an `xmp:Rating` value. XMP allows reals; values outside −1…5 are ignored.
fn parse_rating(s: &str) -> Option<i8> {
    let v: f64 = s.trim().parse().ok()?;
    if !v.is_finite() {
        return None;
    }
    let r = v.round();
    (-1.0..=5.0).contains(&r).then_some(r as i8)
}

fn parse_label(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Rating,
    Label,
}

fn is_ns(r: &ResolveResult<'_>, uri: &str) -> bool {
    matches!(r, ResolveResult::Bound(Namespace(ns)) if *ns == uri)
}

fn field_of(local: &str) -> Option<Field> {
    match local {
        "Rating" => Some(Field::Rating),
        "Label" => Some(Field::Label),
        _ => None,
    }
}

impl OwnedFields {
    fn set(&mut self, f: Field, value: &str) {
        match f {
            Field::Rating if self.rating.is_none() => self.rating = parse_rating(value),
            Field::Label if self.label.is_none() => self.label = parse_label(value),
            _ => {}
        }
    }

    fn take_attributes(
        &mut self,
        reader: &NsReader<&[u8]>,
        e: &BytesStart<'_>,
    ) -> Result<(), XmpError> {
        for a in e.attributes().with_checks(false) {
            let a = a.map_err(|err| XmpError(err.to_string()))?;
            let (ns, local) = reader.resolver().resolve_attribute(a.key);
            if !is_ns(&ns, NS_XMP) {
                continue;
            }
            if let Some(f) = field_of(local.as_ref()) {
                let v = a
                    .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                    .map_err(|err| XmpError(err.to_string()))?;
                self.set(f, &v);
            }
        }
        Ok(())
    }
}

/// Read `xmp:Rating` and `xmp:Label` from an XMP packet or sidecar. Both forms are understood:
/// attributes on `rdf:Description` and child elements. Namespaces are matched by URI, so any
/// prefix works (old files use `xap:`). The first occurrence wins.
pub fn read_owned(xmp: &[u8]) -> Result<OwnedFields, XmpError> {
    let mut reader = NsReader::from_reader(xmp);
    let mut out = OwnedFields::default();
    // The owned field whose element text is being read, if any.
    let mut open: Option<Field> = None;
    let mut text = String::new();
    loop {
        let (ns, ev) = reader
            .read_resolved_event()
            .map_err(|e| XmpError(e.to_string()))?;
        let in_rdf = is_ns(&ns, NS_RDF);
        let in_xmp = is_ns(&ns, NS_XMP);
        match ev {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let local = e.local_name();
                if in_rdf && local.as_ref() == "Description" {
                    out.take_attributes(&reader, e)?;
                } else if in_xmp && matches!(ev, Event::Start(_)) {
                    open = field_of(local.as_ref());
                    text.clear();
                }
            }
            Event::Text(ref t) => {
                if open.is_some() {
                    text.push_str(&t.xml10_content());
                }
            }
            Event::CData(ref t) => {
                if open.is_some() {
                    text.push_str(&t.xml10_content());
                }
            }
            Event::GeneralRef(ref r) => {
                if open.is_some() {
                    let name = r.xml10_content();
                    let ch = match name.as_ref() {
                        "amp" => Some('&'),
                        "lt" => Some('<'),
                        "gt" => Some('>'),
                        "quot" => Some('"'),
                        "apos" => Some('\''),
                        n => n
                            .strip_prefix("#x")
                            .and_then(|h| u32::from_str_radix(h, 16).ok())
                            .or_else(|| n.strip_prefix('#').and_then(|d| d.parse().ok()))
                            .and_then(char::from_u32),
                    };
                    text.extend(ch);
                }
            }
            Event::End(_) => {
                if let Some(f) = open.take() {
                    out.set(f, &text);
                }
                text.clear();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

/// Extract the XMP packet embedded in a JPEG (APP1 `http://ns.adobe.com/xap/1.0/\0`).
pub fn jpeg_xmp_packet(jpeg: &[u8]) -> Option<&[u8]> {
    const SIG: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
    if jpeg.get(0..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2;
    loop {
        if *jpeg.get(i)? != 0xFF {
            return None;
        }
        let m = *jpeg.get(i + 1)?;
        if m == 0xFF {
            i += 1;
            continue;
        }
        if m == 0xDA || m == 0xD9 {
            return None;
        }
        let len = u16::from_be_bytes([*jpeg.get(i + 2)?, *jpeg.get(i + 3)?]) as usize;
        if len < 2 {
            return None;
        }
        let seg = jpeg.get(i + 4..i + 2 + len)?;
        if m == 0xE1 && seg.starts_with(SIG) {
            return Some(&seg[SIG.len()..]);
        }
        i += 2 + len;
    }
}

/// Extract the XMP packet of a TIFF-based file (DNG): IFD0 tag 700 (0x02BC).
pub fn tiff_xmp_packet(file: &[u8]) -> Option<&[u8]> {
    let le = match file.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |at: usize| -> Option<u16> {
        let b = file.get(at..at + 2)?;
        Some(if le {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        })
    };
    let u32_at = |at: usize| -> Option<u32> {
        let b = file.get(at..at + 4)?;
        let a = [b[0], b[1], b[2], b[3]];
        Some(if le {
            u32::from_le_bytes(a)
        } else {
            u32::from_be_bytes(a)
        })
    };
    if u16_at(2)? != 42 {
        return None;
    }
    let ifd = u32_at(4)? as usize;
    let n = u16_at(ifd)? as usize;
    for k in 0..n.min(4096) {
        let e = ifd + 2 + k * 12;
        if u16_at(e)? == 0x02BC {
            let count = u32_at(e + 4)? as usize;
            let off = if count <= 4 {
                e + 8
            } else {
                u32_at(e + 8)? as usize
            };
            return file.get(off..off.checked_add(count)?);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xmp_namespace_matches_spec() {
        assert_eq!(NS_XMP, "http://ns.adobe.com/xap/1.0/");
    }

    #[test]
    fn ratings_parse_and_range() {
        assert_eq!(parse_rating("3"), Some(3));
        assert_eq!(parse_rating(" -1 "), Some(-1));
        assert_eq!(parse_rating("4.0"), Some(4));
        assert_eq!(parse_rating("7"), None);
        assert_eq!(parse_rating("NaN"), None);
        assert_eq!(parse_rating("x"), None);
    }

    #[test]
    fn foreign_namespace_with_same_local_names_is_ignored() {
        let x = br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:MicrosoftPhoto="http://ns.microsoft.com/photo/1.0/" MicrosoftPhoto:Rating="75">
<MicrosoftPhoto:Label>Red</MicrosoftPhoto:Label></rdf:Description></rdf:RDF></x:xmpmeta>"#;
        assert_eq!(read_owned(x).unwrap(), OwnedFields::default());
    }

    #[test]
    fn malformed_xml_is_an_error_not_a_panic() {
        assert!(read_owned(b"<a><b></a>").is_err());
        let _ = read_owned(&[0xFF, 0xFE, 0x00]);
    }
}
