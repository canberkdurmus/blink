//! FILE-5 (preserve foreign content), FILE-9 (`xmp:MetadataDate`), RATE-1 (XMP round trip).

use blink_testkit::sidecars;
use blink_xmp::{Edit, NS_XMP, OwnedEdit, OwnedFields, iso8601_utc, read_owned, write_owned};
use proptest::prelude::*;
use quick_xml::NsReader;
use quick_xml::events::Event;
use quick_xml::name::{Namespace, ResolveResult};
use std::collections::BTreeSet;

const DATE: &str = "2026-11-21T09:05:00+00:00";

fn edit(rating: Edit<i8>, label: Edit<String>) -> OwnedEdit {
    OwnedEdit {
        rating,
        label,
        metadata_date: DATE.into(),
    }
}

/// A namespace-resolved XML tree. Owned fields (`xmp:Rating|Label|MetadataDate` as attributes or
/// elements) are dropped, as are namespace declarations of the XMP namespace and whitespace-only
/// text, so two trees are equal iff the foreign content is semantically identical.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    Elem {
        ns: String,
        local: String,
        attrs: BTreeSet<(String, String, String)>,
        children: Vec<Node>,
    },
    Text(String),
    Other(String),
}

fn uri(r: &ResolveResult<'_>) -> String {
    match r {
        ResolveResult::Bound(Namespace(ns)) => ns.to_string(),
        _ => String::new(),
    }
}

fn is_owned(ns: &str, local: &str) -> bool {
    ns == NS_XMP && matches!(local, "Rating" | "Label" | "MetadataDate")
}

/// Canonical tree plus the set of declared namespace URIs.
fn canonical(xml: &[u8]) -> (Vec<Node>, BTreeSet<String>) {
    let mut r = NsReader::from_reader(xml);
    let mut stack: Vec<Node> = vec![Node::Elem {
        ns: String::new(),
        local: "#doc".into(),
        attrs: BTreeSet::new(),
        children: vec![],
    }];
    let mut nss = BTreeSet::new();
    let mut text = String::new();
    let flush = |text: &mut String, stack: &mut Vec<Node>| {
        let t = text.trim().to_string();
        text.clear();
        if !t.is_empty()
            && let Some(Node::Elem { children, .. }) = stack.last_mut()
        {
            children.push(Node::Text(t));
        }
    };
    loop {
        let (ns, ev) = r.read_resolved_event().expect("well-formed output");
        let ns = uri(&ns);
        match ev {
            Event::Start(ref e) | Event::Empty(ref e) => {
                flush(&mut text, &mut stack);
                let mut attrs = BTreeSet::new();
                for a in e.attributes() {
                    let a = a.unwrap();
                    if let Some(decl) = a.key.as_namespace_binding() {
                        let _ = decl;
                        let v = a.value.to_string();
                        if v != NS_XMP {
                            nss.insert(v);
                        }
                        continue;
                    }
                    let (ans, local) = r.resolver().resolve_attribute(a.key);
                    let ans = uri(&ans);
                    let local = local.as_ref().to_string();
                    if is_owned(&ans, &local) {
                        continue;
                    }
                    let v = a
                        .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .unwrap()
                        .to_string();
                    attrs.insert((ans, local, v));
                }
                let node = Node::Elem {
                    ns: ns.clone(),
                    local: e.local_name().as_ref().to_string(),
                    attrs,
                    children: vec![],
                };
                if matches!(ev, Event::Empty(_)) {
                    if !is_owned(&ns, e.local_name().as_ref())
                        && let Some(Node::Elem { children, .. }) = stack.last_mut()
                    {
                        children.push(node);
                    }
                } else {
                    stack.push(node);
                }
            }
            Event::End(_) => {
                flush(&mut text, &mut stack);
                let node = stack.pop().unwrap();
                let owned = matches!(&node, Node::Elem { ns, local, .. } if is_owned(ns, local));
                if !owned && let Some(Node::Elem { children, .. }) = stack.last_mut() {
                    children.push(node);
                }
            }
            Event::Text(t) => text.push_str(&t.xml10_content()),
            Event::CData(t) => text.push_str(&t.xml10_content()),
            Event::GeneralRef(g) => {
                text.push('&');
                text.push_str(&g.xml10_content());
                text.push(';');
            }
            Event::Comment(c) => {
                flush(&mut text, &mut stack);
                if let Some(Node::Elem { children, .. }) = stack.last_mut() {
                    children.push(Node::Other(format!("comment:{}", &*c)));
                }
            }
            Event::PI(p) => {
                flush(&mut text, &mut stack);
                if let Some(Node::Elem { children, .. }) = stack.last_mut() {
                    children.push(Node::Other(format!("pi:{}", p.content())));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    // Text inside owned elements was dropped with them; strip it from comparison too.
    let Node::Elem { children, .. } = stack.pop().unwrap() else {
        unreachable!()
    };
    (children, nss)
}

fn assert_foreign_preserved(before: &[u8], after: &[u8]) {
    let (a, na) = canonical(before);
    let (b, nb) = canonical(after);
    assert_eq!(
        a,
        b,
        "foreign content changed:\n{}\n---\n{}",
        s(before),
        s(after)
    );
    assert_eq!(na, nb, "namespaces changed");
}

fn s(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn fields(rating: Option<i8>, label: Option<&str>) -> OwnedFields {
    OwnedFields {
        rating,
        label: label.map(str::to_string),
    }
}

/// The element-form `xmp:MetadataDate` value, or the attribute value.
fn metadata_date(xml: &[u8]) -> Option<String> {
    let t = s(xml);
    if let Some(i) = t.find("MetadataDate=\"") {
        let rest = &t[i + 14..];
        return Some(rest[..rest.find('"')?].to_string());
    }
    let i = t.find("MetadataDate>")?;
    let rest = &t[i + 13..];
    Some(rest[..rest.find('<')?].to_string())
}

#[test]
fn req_file_5_app_sidecars_keep_every_foreign_field() {
    let inputs = [
        sidecars::lightroom(4, Some("Red")),
        sidecars::darktable(2),
        sidecars::bridge(5, Some("Green")),
        sidecars::digikam(3, Some("Purple")),
        sidecars::xap_prefix(1),
    ];
    let edits = [
        edit(Edit::Set(1), Edit::Keep),
        edit(Edit::Keep, Edit::Set("Blue".into())),
        edit(Edit::Set(-1), Edit::Remove),
        edit(Edit::Remove, Edit::Set("Yellow & <b>".into())),
    ];
    for x in &inputs {
        for e in &edits {
            let out = write_owned(Some(x.as_bytes()), e).unwrap();
            assert_foreign_preserved(x.as_bytes(), &out);
            let got = read_owned(&out).unwrap();
            let before = read_owned(x.as_bytes()).unwrap();
            let want_r = match e.rating {
                Edit::Keep => before.rating,
                Edit::Set(r) => Some(r),
                Edit::Remove => None,
            };
            let want_l = match &e.label {
                Edit::Keep => before.label.clone(),
                Edit::Set(l) => Some(l.clone()),
                Edit::Remove => None,
            };
            assert_eq!(
                got,
                OwnedFields {
                    rating: want_r,
                    label: want_l
                },
                "{}",
                s(&out)
            );
        }
    }
}

#[test]
fn req_file_5_bytes_outside_owned_fields_are_untouched() {
    // Bridge: element form. Only the text of the changed elements differs.
    let x = sidecars::bridge(5, Some("Green"));
    let out = s(&write_owned(Some(x.as_bytes()), &edit(Edit::Set(2), Edit::Keep)).unwrap());
    let want = x
        .replace("<xmp:Rating>5</xmp:Rating>", "<xmp:Rating>2</xmp:Rating>")
        .replace(
            "2024-05-01T10:00:00+02:00</xmp:MetadataDate>",
            &format!("{DATE}</xmp:MetadataDate>"),
        );
    assert_eq!(out, want);
    assert!(out.starts_with("<?xpacket begin=\"\u{feff}\""));
    assert!(out.ends_with("<?xpacket end=\"w\"?>"));
}

#[test]
fn req_file_5_keeps_attribute_or_element_form_per_field() {
    let lr = sidecars::lightroom(4, Some("Red"));
    let out = s(&write_owned(
        Some(lr.as_bytes()),
        &edit(Edit::Set(5), Edit::Set("Blue".into())),
    )
    .unwrap());
    assert!(
        out.contains("xmp:Rating=\"5\"") && out.contains("xmp:Label=\"Blue\""),
        "{out}"
    );
    let br = sidecars::bridge(4, Some("Red"));
    let out = s(&write_owned(
        Some(br.as_bytes()),
        &edit(Edit::Set(5), Edit::Set("Blue".into())),
    )
    .unwrap());
    assert!(
        out.contains("<xmp:Rating>5</xmp:Rating>") && out.contains("<xmp:Label>Blue</xmp:Label>"),
        "{out}"
    );
    // A field that wasn't there is added as an attribute, with the file's own prefix.
    let old = sidecars::xap_prefix(1);
    let out = s(&write_owned(
        Some(old.as_bytes()),
        &edit(Edit::Keep, Edit::Set("Red".into())),
    )
    .unwrap());
    assert!(
        out.contains("xap:Label=\"Red\"") && !out.contains("xmlns:xmp="),
        "{out}"
    );
    assert_eq!(
        read_owned(out.as_bytes()).unwrap(),
        fields(Some(1), Some("Red"))
    );
}

#[test]
fn req_file_5_unparseable_sidecars_are_refused_not_overwritten() {
    for bad in [
        &b"<x:xmpmeta><rdf:RDF>"[..],
        b"<a></b>",
        b"not xml at all <",
        b"<x/>",
    ] {
        assert!(
            write_owned(Some(bad), &edit(Edit::Set(1), Edit::Keep)).is_err(),
            "{}",
            s(bad)
        );
    }
}

#[test]
fn req_file_5_a_foreign_xmp_prefix_is_not_shadowed() {
    // `xmp:` is bound to another namespace on an ancestor: Blink must not rebind it.
    let x = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" xmlns:xmp="http://example.com/not-xmp/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmp:Rating="9"><xmp:Thing>v</xmp:Thing></rdf:Description></rdf:RDF></x:xmpmeta>"#;
    let out = write_owned(Some(x.as_bytes()), &edit(Edit::Set(3), Edit::Keep)).unwrap();
    assert_foreign_preserved(x.as_bytes(), &out);
    assert_eq!(
        read_owned(&out).unwrap(),
        fields(Some(3), None),
        "{}",
        s(&out)
    );
}

#[test]
fn req_file_9_metadata_date_is_written_on_every_change() {
    assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00+00:00");
    assert_eq!(iso8601_utc(1_795_251_900), "2026-11-21T09:05:00+00:00");
    assert_eq!(iso8601_utc(951_782_400), "2000-02-29T00:00:00+00:00");
    assert_eq!(iso8601_utc(-1), "1969-12-31T23:59:59+00:00");
    for x in [
        sidecars::lightroom(4, None),
        sidecars::darktable(1),
        sidecars::bridge(2, None),
    ] {
        let out = write_owned(Some(x.as_bytes()), &edit(Edit::Set(3), Edit::Keep)).unwrap();
        assert_eq!(metadata_date(&out).as_deref(), Some(DATE), "{}", s(&out));
        assert_eq!(
            s(&out).matches("MetadataDate").count(),
            if x.contains("<xmp:MetadataDate>") {
                2
            } else {
                1
            }
        );
    }
    let new = write_owned(None, &edit(Edit::Set(3), Edit::Keep)).unwrap();
    assert_eq!(metadata_date(&new).as_deref(), Some(DATE));
}

#[test]
fn req_rate_1_req_file_5_new_sidecar_is_minimal_and_round_trips() {
    for r in -1..=5i8 {
        for l in [
            None,
            Some("Red"),
            Some("Yellow"),
            Some("Green"),
            Some("Blue"),
            Some("Purple"),
        ] {
            let e = edit(
                Edit::Set(r),
                l.map_or(Edit::Keep, |l| Edit::Set(l.to_string())),
            );
            let out = write_owned(None, &e).unwrap();
            assert_eq!(read_owned(&out).unwrap(), fields(Some(r), l));
            let t = s(&out);
            assert_eq!(t.matches("<rdf:Description").count(), 1);
            assert!(t.contains("<x:xmpmeta") && t.contains("<rdf:RDF"));
            // Round trip through a second edit keeps it parseable and minimal.
            let again = write_owned(Some(&out), &edit(Edit::Keep, Edit::Remove)).unwrap();
            assert_eq!(read_owned(&again).unwrap(), fields(Some(r), None));
        }
    }
    // Blank files count as absent.
    assert_eq!(
        read_owned(&write_owned(Some(b"  \n"), &edit(Edit::Set(2), Edit::Keep)).unwrap()).unwrap(),
        fields(Some(2), None)
    );
}

#[test]
fn req_file_5_rdf_without_description_gets_one() {
    for x in [
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"/></x:xmpmeta>"#,
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><!-- keep --><r:RDF xmlns:r="http://www.w3.org/1999/02/22-rdf-syntax-ns#"></r:RDF></x:xmpmeta>"#,
    ] {
        let out = write_owned(
            Some(x.as_bytes()),
            &edit(Edit::Set(4), Edit::Set("Red".into())),
        )
        .unwrap();
        assert_eq!(
            read_owned(&out).unwrap(),
            fields(Some(4), Some("Red")),
            "{}",
            s(&out)
        );
    }
}

// ---- Property test with generated foreign XMP (FILE-5 acceptance) ----

const PREFIXES: [(&str, &str); 6] = [
    ("dc", "http://purl.org/dc/elements/1.1/"),
    ("crs", "http://ns.adobe.com/camera-raw-settings/1.0/"),
    ("darktable", "http://darktable.sf.net/"),
    ("foo", "http://example.com/foo/"),
    // The XMP namespace itself, with foreign (non-owned) properties.
    ("xmp", NS_XMP),
    ("xap", NS_XMP),
];

fn value() -> impl Strategy<Value = String> {
    prop::string::string_regex("[a-zA-Z0-9 &<>\"'.:/-]{0,12}").unwrap()
}

fn esc(v: &str) -> String {
    v.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn local() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("CreatorTool".to_string()),
        Just("ModifyDate".to_string()),
        Just("history".to_string()),
        Just("subject".to_string()),
        prop::string::string_regex("[A-Z][a-z]{1,6}").unwrap(),
    ]
}

/// Prefix index, local name and escaped value of one foreign attribute (never an owned field).
fn foreign_attr() -> impl Strategy<Value = (usize, String, String)> {
    (0..PREFIXES.len(), local(), value()).prop_map(|(p, l, v)| {
        let l = if matches!(l.as_str(), "Rating" | "Label" | "Metadatadate") {
            format!("{l}X")
        } else {
            l
        };
        (p, l, esc(&v))
    })
}

fn foreign_elem() -> impl Strategy<Value = String> {
    let leaf = (foreign_attr(), value())
        .prop_map(|((p, l, _), v)| format!("<{0}:{1}>{2}</{0}:{1}>", PREFIXES[p].0, l, esc(&v)));
    leaf.prop_recursive(3, 12, 3, |inner| {
        prop_oneof![
            (foreign_attr(), prop::collection::vec(inner.clone(), 0..3)).prop_map(
                |((p, l, _), kids)| {
                    format!(
                        "<{0}:{1}><rdf:Seq>{2}</rdf:Seq></{0}:{1}>",
                        PREFIXES[p].0,
                        l,
                        kids.iter()
                            .map(|k| format!("<rdf:li>{k}</rdf:li>"))
                            .collect::<String>()
                    )
                }
            ),
            (foreign_attr(), prop::collection::vec(inner, 0..3), value()).prop_map(
                |((p, l, a), kids, c)| format!(
                    "<{0}:{1} foo:attr=\"{2}\"><!-- {3} -->\n {4}</{0}:{1}>",
                    PREFIXES[p].0,
                    l,
                    a,
                    c.replace('-', "_"),
                    kids.concat()
                )
            ),
        ]
    })
}

/// Where an owned field lives in the generated sidecar.
#[derive(Debug, Clone, Copy)]
enum Form {
    Absent,
    Attr,
    Elem,
}

fn form() -> impl Strategy<Value = Form> {
    prop_oneof![Just(Form::Absent), Just(Form::Attr), Just(Form::Elem)]
}

prop_compose! {
    fn sidecar()(
        xpacket in any::<bool>(),
        decl in any::<bool>(),
        attrs in prop::collection::vec(foreign_attr(), 0..6),
        elems in prop::collection::vec(foreign_elem(), 0..5),
        second in prop::option::of(prop::collection::vec(foreign_elem(), 1..3)),
        rating in (form(), -1..=5i8),
        label in (form(), prop_oneof![Just("Red"), Just("Blue"), Just("x&y")]),
        date in form(),
        self_closing in any::<bool>(),
        xmp_prefix in prop_oneof![Just("xmp"), Just("xap")],
    ) -> String {
        let mut owned_attrs = String::new();
        let mut owned_elems = String::new();
        for (f, name, v) in [
            (rating.0, "Rating", rating.1.to_string()),
            (label.0, "Label", esc(label.1)),
            (date, "MetadataDate", "2020-01-01T00:00:00Z".to_string()),
        ] {
            match f {
                Form::Absent => {}
                Form::Attr => owned_attrs.push_str(&format!("\n   {xmp_prefix}:{name}=\"{v}\"")),
                Form::Elem => owned_elems.push_str(&format!("\n   <{xmp_prefix}:{name}>{v}</{xmp_prefix}:{name}>")),
            }
        }
        // Unique attribute names per element.
        let mut seen = BTreeSet::new();
        let fattrs: String = attrs
            .iter()
            .filter(|(p, l, _)| seen.insert((PREFIXES[*p].1, l.clone())))
            .map(|(p, l, v)| format!("\n   {}:{}=\"{}\"", PREFIXES[*p].0, l, v))
            .collect();
        let nsdecl: String = PREFIXES
            .iter()
            .filter(|(p, _)| *p != "xmp" && *p != "xap")
            .map(|(p, u)| format!("\n    xmlns:{p}=\"{u}\""))
            .collect();
        let body = format!("{owned_elems}{}", elems.concat());
        let desc = if self_closing && body.is_empty() {
            format!("<rdf:Description rdf:about=\"\"{nsdecl}\n    xmlns:xmp=\"{NS_XMP}\" xmlns:xap=\"{NS_XMP}\"{fattrs}{owned_attrs}/>")
        } else {
            format!("<rdf:Description rdf:about=\"\"{nsdecl}\n    xmlns:xmp=\"{NS_XMP}\" xmlns:xap=\"{NS_XMP}\"{fattrs}{owned_attrs}>{body}\n  </rdf:Description>")
        };
        let second = second
            .map(|e| format!("\n  <rdf:Description rdf:about=\"\"{nsdecl}>{}</rdf:Description>", e.concat()))
            .unwrap_or_default();
        let mut x = format!(
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"gen\">\n <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  {desc}{second}\n </rdf:RDF>\n</x:xmpmeta>\n"
        );
        if decl {
            x = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{x}");
        }
        if xpacket {
            x = format!("<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n{x}{}<?xpacket end=\"w\"?>", " ".repeat(40));
        }
        x
    }
}

fn edit_strategy() -> impl Strategy<Value = OwnedEdit> {
    (
        prop_oneof![
            Just(Edit::Keep),
            Just(Edit::Remove),
            (-1..=5i8).prop_map(Edit::Set)
        ],
        prop_oneof![
            Just(Edit::Keep),
            Just(Edit::Remove),
            value()
                .prop_filter("non-blank", |v| !v.trim().is_empty())
                .prop_map(|v| Edit::Set(v.trim().to_string()))
        ],
    )
        .prop_map(|(rating, label)| edit(rating, label))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    #[test]
    fn req_file_5_generated_foreign_xmp_is_preserved(x in sidecar(), e in edit_strategy()) {
        let before = read_owned(x.as_bytes()).unwrap();
        let out = write_owned(Some(x.as_bytes()), &e).unwrap();
        assert_foreign_preserved(x.as_bytes(), &out);
        let got = read_owned(&out).unwrap();
        let want_r = match e.rating { Edit::Keep => before.rating, Edit::Set(r) => Some(r), Edit::Remove => None };
        let want_l = match &e.label { Edit::Keep => before.label.clone(), Edit::Set(l) => Some(l.clone()), Edit::Remove => None };
        prop_assert_eq!(got, OwnedFields { rating: want_r, label: want_l });
        let date = metadata_date(&out);
        prop_assert_eq!(date.as_deref(), Some(DATE));
        // Rewriting is stable: a second identical edit changes nothing.
        prop_assert_eq!(write_owned(Some(&out), &e).unwrap(), out);
    }
}

#[test]
fn req_file_5_a_leading_bom_is_kept_and_positions_stay_right() {
    // Found by the xmp_write fuzz target: a BOM shifted every splice by three bytes.
    for x in [
        format!("\u{feff}{}", sidecars::lightroom(4, Some("Red"))),
        format!("\u{feff}{}", sidecars::bridge(2, None)),
        "\u{feff}<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\"><xmp:Label>Red</xmp:Label></rdf:Description></rdf:RDF></x:xmpmeta>".to_string(),
    ] {
        let out = write_owned(Some(x.as_bytes()), &edit(Edit::Set(0), Edit::Keep)).unwrap();
        assert!(out.starts_with("\u{feff}".as_bytes()));
        assert_foreign_preserved(x.as_bytes(), &out);
        assert_eq!(read_owned(&out).unwrap().rating, Some(0), "{}", s(&out));
    }
}

#[test]
fn req_file_5_an_unbound_xmp_prefix_is_declared_as_xmp() {
    // `xmp:` used but never declared: it is free, so Blink binds `xmp` on the Description.
    let x = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about=""/></rdf:RDF></x:xmpmeta>"#;
    let out = s(&write_owned(Some(x.as_bytes()), &edit(Edit::Set(2), Edit::Keep)).unwrap());
    assert!(out.contains(&format!("xmlns:xmp=\"{NS_XMP}\"")) && out.contains("xmp:Rating=\"2\""), "{out}");
}
