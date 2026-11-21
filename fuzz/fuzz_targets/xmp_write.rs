//! Fuzz the sidecar rewrite (FILE-5): never panic; whenever it accepts input, the output
//! parses and holds exactly the requested owned fields.
#![no_main]
use blink_xmp::{Edit, OwnedEdit, read_owned, write_owned};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (ctl, xml) = data.split_first().map_or((0, data), |(c, r)| (*c, r));
    let rating = match ctl % 3 {
        0 => Edit::Keep,
        1 => Edit::Remove,
        _ => Edit::Set((ctl / 3 % 7) as i8 - 1),
    };
    let label = match ctl / 21 % 3 {
        0 => Edit::Keep,
        1 => Edit::Remove,
        _ => Edit::Set("Red & <Blue>".to_string()),
    };
    let edit = OwnedEdit {
        rating: rating.clone(),
        label: label.clone(),
        metadata_date: "2026-11-21T09:05:00+00:00".into(),
    };
    let Ok(before) = read_owned(xml) else { return };
    if let Ok(out) = write_owned(Some(xml), &edit) {
        let got = read_owned(&out).expect("rewritten sidecar parses");
        let want_r = match rating { Edit::Keep => before.rating, Edit::Set(r) => Some(r), Edit::Remove => None };
        let want_l = match label { Edit::Keep => before.label, Edit::Set(l) => Some(l), Edit::Remove => None };
        assert_eq!(got.rating, want_r);
        assert_eq!(got.label, want_l);
    }
});
