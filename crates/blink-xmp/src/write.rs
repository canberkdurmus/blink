//! Rewrite the owned fields of a sidecar and keep everything else (FILE-5, FILE-9, SPEC §6).
//!
//! The rewrite is a splice over the original bytes: the packet is parsed once to find where the
//! owned fields (`xmp:Rating`, `xmp:Label`, `xmp:MetadataDate`) live, and only those spans change.
//! Every other byte (declarations, processing instructions, comments, foreign namespaces,
//! elements, attributes, whitespace, a BOM, `xpacket` padding) is copied unchanged. Only an
//! `rdf:Description` start tag whose owned attribute changes is rebuilt, which may change the
//! whitespace between its attributes.

use crate::{NS_RDF, NS_XMP, XmpError};
use quick_xml::NsReader;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::{Namespace, QName, ResolveResult};

/// What to do with one owned field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit<T> {
    /// Leave the field as it is (absent stays absent).
    Keep,
    /// Set the field, adding it if absent.
    Set(T),
    /// Remove the field if present.
    Remove,
}

/// The change Blink makes to one sidecar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedEdit {
    /// `xmp:Rating` (−1…5).
    pub rating: Edit<i8>,
    /// `xmp:Label`.
    pub label: Edit<String>,
    /// `xmp:MetadataDate`, ISO 8601 with offset (FILE-9). Always written.
    pub metadata_date: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Field {
    Rating,
    Label,
    MetadataDate,
}

const FIELDS: [Field; 3] = [Field::Rating, Field::Label, Field::MetadataDate];

impl Field {
    fn of(local: &str) -> Option<Self> {
        match local {
            "Rating" => Some(Self::Rating),
            "Label" => Some(Self::Label),
            "MetadataDate" => Some(Self::MetadataDate),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Rating => "Rating",
            Self::Label => "Label",
            Self::MetadataDate => "MetadataDate",
        }
    }
}

impl OwnedEdit {
    /// `Some(value)`: set; `None`: remove; not listed: keep.
    fn wanted(&self, f: Field) -> Option<Option<String>> {
        let e = match f {
            Field::Rating => match &self.rating {
                Edit::Keep => return None,
                Edit::Set(r) => Edit::Set(r.to_string()),
                Edit::Remove => Edit::Remove,
            },
            Field::Label => self.label.clone(),
            Field::MetadataDate => Edit::Set(self.metadata_date.clone()),
        };
        match e {
            Edit::Keep => None,
            Edit::Set(v) => Some(Some(v)),
            Edit::Remove => Some(None),
        }
    }
}

fn escape(v: &str) -> String {
    let mut s = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '&' => s.push_str("&amp;"),
            '<' => s.push_str("&lt;"),
            '>' => s.push_str("&gt;"),
            '"' => s.push_str("&quot;"),
            c => s.push(c),
        }
    }
    s
}

/// A minimal, valid new sidecar (SPEC §6): `x:xmpmeta` → `rdf:RDF` → one `rdf:Description` with
/// the owned fields as attributes.
pub fn new_sidecar(edit: &OwnedEdit) -> Vec<u8> {
    let mut attrs = String::new();
    for f in FIELDS {
        if let Some(Some(v)) = edit.wanted(f) {
            attrs.push_str(&format!("\n    xmp:{}=\"{}\"", f.name(), escape(&v)));
        }
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Blink {}\">\n \
<rdf:RDF xmlns:rdf=\"{NS_RDF}\">\n  \
<rdf:Description rdf:about=\"\"\n    xmlns:xmp=\"{NS_XMP}\"{attrs}/>\n \
</rdf:RDF>\n</x:xmpmeta>\n",
        env!("CARGO_PKG_VERSION")
    )
    .into_bytes()
}

/// An `rdf:Description` start (or empty) tag.
struct Desc {
    start: usize,
    end: usize,
    name: String,
    empty: bool,
    /// Raw `key="value"` pairs in order, with the owned field each one is (if any).
    attrs: Vec<(String, String, Option<Field>)>,
    /// Whitespace before the first attribute, reused as the separator when rebuilding.
    sep: String,
    /// Prefix bound to the XMP namespace here, or a prefix that is free to bind.
    xmp_prefix: Result<String, String>,
}

/// An owned field in element form: `<xmp:Rating>3</xmp:Rating>` or `<xmp:Label/>`.
struct Elem {
    field: Field,
    /// Whole element.
    start: usize,
    end: usize,
    /// Content between the tags (`None` for an empty element).
    inner: Option<(usize, usize)>,
    name: String,
}

#[derive(Clone, Copy)]
enum Open {
    Desc,
    Owned(usize),
    Other,
}

fn is_ns(r: &ResolveResult<'_>, uri: &str) -> bool {
    matches!(r, ResolveResult::Bound(Namespace(ns)) if *ns == uri)
}

fn xmp_prefix(reader: &NsReader<&[u8]>, src: &[u8]) -> Result<String, String> {
    for p in ["xmp", "xap"] {
        let q = format!("{p}:x");
        if is_ns(&reader.resolver().resolve_attribute(QName(&q)).0, NS_XMP) {
            return Ok(p.to_string());
        }
    }
    // Pick a prefix that is unbound here and used nowhere in the file, so declaring it can
    // neither shadow a foreign binding nor give an undeclared foreign use a meaning.
    for p in ["xmp", "xmpBlink", "xmpBlink2"] {
        let q = format!("{p}:x");
        let used = src.windows(p.len() + 1).any(|w| w == format!("{p}:").as_bytes());
        if !used
            && matches!(
                reader.resolver().resolve_attribute(QName(&q)).0,
                ResolveResult::Unknown(_) | ResolveResult::Unbound
            )
        {
            return Err(p.to_string());
        }
    }
    Err("xmpBlink3".to_string())
}

/// Return `existing` with the owned fields changed as `edit` says and everything else kept.
/// `None` or blank input yields [`new_sidecar`]. Input that is not well-formed XML, or has no
/// `rdf:RDF`, is an error: Blink never overwrites a sidecar it cannot parse.
pub fn write_owned(existing: Option<&[u8]>, edit: &OwnedEdit) -> Result<Vec<u8>, XmpError> {
    let Some(src) = existing.filter(|b| {
        !b.iter()
            .all(|c| c.is_ascii_whitespace() || *c == 0 || matches!(c, 0xEF | 0xBB | 0xBF))
    }) else {
        return Ok(new_sidecar(edit));
    };
    // quick-xml skips a leading BOM and reports positions after it: parse past the BOM and
    // shift every position back into `src`.
    let bom = if src.starts_with(&[0xEF, 0xBB, 0xBF]) { 3 } else { 0 };
    let mut reader = NsReader::from_reader(&src[bom..]);
    reader.config_mut().check_end_names = true;
    let mut descs: Vec<Desc> = Vec::new();
    let mut elems: Vec<Elem> = Vec::new();
    let mut stack: Vec<Open> = Vec::new();
    // Where to insert a Description if none exists: after the `rdf:RDF` start tag.
    let mut rdf: Option<(usize, usize, String, bool)> = None;
    let mut root_seen = false;
    loop {
        let p0 = reader.buffer_position() as usize + bom;
        let (ns, ev) = reader
            .read_resolved_event()
            .map_err(|e| XmpError(e.to_string()))?;
        let in_rdf = is_ns(&ns, NS_RDF);
        let in_xmp = is_ns(&ns, NS_XMP);
        let p1 = reader.buffer_position() as usize + bom;
        match ev {
            Event::Start(ref e) | Event::Empty(ref e) => {
                root_seen = true;
                let empty = matches!(ev, Event::Empty(_));
                let local = e.local_name();
                let local = local.as_ref();
                let parent_is_desc = matches!(stack.last(), Some(Open::Desc));
                let kind = if in_rdf && local == "Description" {
                    descs.push(desc_of(&reader, src, e, p0, p1, empty)?);
                    Open::Desc
                } else if in_xmp
                    && parent_is_desc
                    && let Some(field) = Field::of(local)
                {
                    elems.push(Elem {
                        field,
                        start: p0,
                        end: p1,
                        inner: (!empty).then_some((p1, p1)),
                        name: e.name().as_ref().to_string(),
                    });
                    Open::Owned(elems.len() - 1)
                } else {
                    if in_rdf && local == "RDF" && rdf.is_none() {
                        rdf = Some((p0, p1, e.name().as_ref().to_string(), empty));
                    }
                    Open::Other
                };
                if !empty {
                    stack.push(kind);
                }
            }
            Event::End(_) => {
                if let Some(Open::Owned(i)) = stack.pop() {
                    let el = &mut elems[i];
                    el.end = p1;
                    if let Some((a, _)) = el.inner {
                        el.inner = Some((a, p0));
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !root_seen || !stack.is_empty() {
        return Err(XmpError("not a complete XML document".into()));
    }

    // Edits as (start, end, replacement), applied back to front.
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for el in &elems {
        let Some(want) = edit.wanted(el.field) else {
            continue;
        };
        match (want, el.inner) {
            (None, _) => edits.push((el.start, el.end, String::new())),
            (Some(v), Some((a, b))) => edits.push((a, b, escape(&v))),
            (Some(v), None) => edits.push((
                el.start,
                el.end,
                format!("<{n}>{}</{n}>", escape(&v), n = el.name),
            )),
        }
    }
    // Fields to add: wanted, but neither an attribute nor an element anywhere.
    let present = |f: Field| {
        elems.iter().any(|e| e.field == f)
            || descs
                .iter()
                .any(|d| d.attrs.iter().any(|(_, _, a)| *a == Some(f)))
    };
    let mut add: Vec<(Field, String)> = FIELDS
        .iter()
        .filter_map(|&f| match edit.wanted(f) {
            Some(Some(v)) if !present(f) => Some((f, v)),
            _ => None,
        })
        .collect();
    for (i, d) in descs.iter().enumerate() {
        let touches = d
            .attrs
            .iter()
            .any(|(_, _, f)| f.is_some_and(|f| edit.wanted(f).is_some()));
        let extra = if i == 0 {
            std::mem::take(&mut add)
        } else {
            Vec::new()
        };
        if !touches && extra.is_empty() {
            continue;
        }
        edits.push((d.start, d.end, rebuild(d, edit, &extra)));
    }
    if !add.is_empty() {
        // No rdf:Description at all: add one inside rdf:RDF.
        let (s, e, name, empty) = rdf.ok_or_else(|| XmpError("no rdf:RDF element".into()))?;
        let prefix = name.rsplit_once(':').map(|(p, _)| p).unwrap_or("rdf");
        let mut desc = format!("<{prefix}:Description {prefix}:about=\"\" xmlns:xmp=\"{NS_XMP}\"");
        for (f, v) in &add {
            desc.push_str(&format!(" xmp:{}=\"{}\"", f.name(), escape(v)));
        }
        desc.push_str("/>");
        if empty {
            let open = std::str::from_utf8(&src[s..e])
                .map_err(|e| XmpError(e.to_string()))?
                .trim_end_matches('>')
                .trim_end_matches('/')
                .to_string();
            edits.push((s, e, format!("{open}>{desc}</{name}>")));
        } else {
            edits.push((e, e, desc));
        }
    }
    edits.sort_by_key(|(s, e, _)| (*s, *e));
    let mut out = Vec::with_capacity(src.len() + 128);
    let mut at = 0;
    for (s, e, r) in edits {
        if s < at {
            return Err(XmpError("overlapping edits".into()));
        }
        out.extend_from_slice(&src[at..s]);
        out.extend_from_slice(r.as_bytes());
        at = e;
    }
    out.extend_from_slice(&src[at..]);
    Ok(out)
}

fn desc_of(
    reader: &NsReader<&[u8]>,
    src: &[u8],
    e: &BytesStart<'_>,
    start: usize,
    end: usize,
    empty: bool,
) -> Result<Desc, XmpError> {
    let mut attrs = Vec::new();
    for a in e.attributes().with_checks(false) {
        let a = a.map_err(|err| XmpError(err.to_string()))?;
        let (ns, local) = reader.resolver().resolve_attribute(a.key);
        let field = if is_ns(&ns, NS_XMP) {
            Field::of(local.as_ref())
        } else {
            None
        };
        attrs.push((
            a.key.as_ref().to_string(),
            a.value.replace('"', "&quot;"),
            field,
        ));
    }
    let raw = e.attributes_raw();
    let ws = raw.len() - raw.trim_start().len();
    let sep = if ws == 0 {
        " ".to_string()
    } else {
        raw[..ws].to_string()
    };
    Ok(Desc {
        start,
        end,
        name: e.name().as_ref().to_string(),
        empty,
        attrs,
        sep,
        xmp_prefix: xmp_prefix(reader, src),
    })
}

fn rebuild(d: &Desc, edit: &OwnedEdit, add: &[(Field, String)]) -> String {
    let mut s = format!("<{}", d.name);
    for (k, v, f) in &d.attrs {
        let v = match f.and_then(|f| edit.wanted(f)) {
            None => v.clone(),
            Some(None) => continue,
            Some(Some(new)) => escape(&new),
        };
        s.push_str(&format!("{}{k}=\"{v}\"", d.sep));
    }
    if !add.is_empty() {
        let p = match &d.xmp_prefix {
            Ok(p) => p.clone(),
            Err(p) => {
                s.push_str(&format!("{}xmlns:{p}=\"{NS_XMP}\"", d.sep));
                p.clone()
            }
        };
        for (f, v) in add {
            s.push_str(&format!("{}{p}:{}=\"{}\"", d.sep, f.name(), escape(v)));
        }
    }
    s.push_str(if d.empty { "/>" } else { ">" });
    s
}

/// Format seconds since the Unix epoch as ISO 8601 in UTC with an explicit offset,
/// e.g. `2026-11-21T09:05:00+00:00` (FILE-9).
pub fn iso8601_utc(unix_secs: i64) -> String {
    let days = unix_secs.div_euclid(86_400);
    let sod = unix_secs.rem_euclid(86_400);
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}+00:00",
        sod / 3600,
        sod % 3600 / 60,
        sod % 60
    )
}
