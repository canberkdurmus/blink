//! M1 gate on the real corpus (full tier): `blink-cli doctor` and `list` (CLI-1, CLI-2).

mod full_corpus {
    use blink_testkit::corpus::{cache_dir, manifest};
    use blink_testkit::golden::{check, mask_timings};
    use std::path::{Path, PathBuf};
    use std::process::Command;

    fn cli(args: &[&str]) -> (bool, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_blink-cli"))
            .args(args)
            .output()
            .expect("run blink-cli");
        (
            out.status.success(),
            String::from_utf8(out.stdout).expect("utf-8 output"),
        )
    }

    fn snapshot(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/snapshots/corpus")
            .join(name)
    }

    #[test]
    fn req_cli_2_doctor_on_every_corpus_file_matches_manifest_and_golden() {
        let m = manifest();
        assert!(!m.files.is_empty());
        for e in &m.files {
            let file = e.file();
            let (ok, out) = cli(&["doctor", file.to_str().unwrap()]);
            assert!(ok, "doctor failed on {}", e.path);
            let x = &e.expected;
            // Frozen exiftool-derived values (SPEC §11.4) …
            // (The preview type varies: JPEG, or uncompressed RGB as in the Leica M8.)
            let size = format!(" {} × {}", x.preview_w, x.preview_h);
            assert!(
                out.lines()
                    .any(|l| l.starts_with("chosen preview: ") && l.ends_with(&size)),
                "{}: chosen preview isn't{size}\n{out}",
                e.path
            );
            for want in [
                format!("sensor: {} × {}", x.sensor_w, x.sensor_h),
                format!("badge: {}", x.badge),
            ] {
                assert!(
                    out.lines().any(|l| l == want),
                    "{}: missing `{want}`\n{out}",
                    e.path
                );
            }
            info_rows_match_exiftool(e, &out);
            // … and the whole report as a golden, timings masked.
            check(
                &snapshot(&format!("doctor/{}.txt", e.path.replace('/', "__"))),
                &mask_timings(&out),
            );
        }
    }

    /// VIEW-7: the formatted info panel rows agree with the frozen `exiftool -j` values.
    fn info_rows_match_exiftool(e: &blink_testkit::corpus::Entry, out: &str) {
        use toml::Value;
        let want = &e.expected.exif;
        let row = |label: &str| -> String {
            let prefix = format!("  {label}: ");
            out.lines()
                .find_map(|l| l.strip_prefix(&prefix))
                .unwrap_or_else(|| panic!("{}: no `{label}` row\n{out}", e.path))
                .to_string()
        };
        let num = |k: &str| {
            want.get(k).map(|v| match v {
                Value::Integer(i) => *i as f64,
                Value::Float(f) => *f,
                other => panic!("{k}: {other:?}"),
            })
        };
        let s = |k: &str| want.get(k).and_then(Value::as_str);
        let near = |label: &str, got: f64, want: f64, tol: f64| {
            assert!(
                (got - want).abs() <= tol,
                "{}: {label} shows {got}, exiftool {want}",
                e.path
            );
        };
        let parse = |label: &str, v: &str| -> f64 {
            v.parse()
                .unwrap_or_else(|_| panic!("{}: {label} `{v}` isn't a number", e.path))
        };
        let file_name = e.path.rsplit('/').next().unwrap();
        assert_eq!(row("File"), file_name);
        match s("model") {
            Some(m) => assert!(row("Body").contains(m.trim()), "{}: body", e.path),
            None => assert_eq!(row("Body"), "—"),
        }
        assert_eq!(row("Lens"), s("lens").unwrap_or("—"), "{}: lens", e.path);
        match num("iso") {
            Some(iso) => near("ISO", parse("ISO", &row("ISO")), iso, 0.0),
            None => assert_eq!(row("ISO"), "—"),
        }
        match num("focal_length") {
            Some(f) => {
                let r = row("Focal length");
                near(
                    "focal length",
                    parse("focal length", r.trim_end_matches(" mm")),
                    f,
                    0.051,
                );
            }
            None => assert_eq!(row("Focal length"), "—"),
        }
        match num("f_number") {
            Some(f) => {
                let r = row("Aperture");
                let v = r.strip_prefix("f/").expect("f/ prefix");
                near("aperture", parse("aperture", v), f, 0.051);
            }
            None => assert_eq!(row("Aperture"), "—"),
        }
        match num("exposure_time") {
            Some(t) => {
                let r = row("Shutter");
                let v = r.strip_suffix(" s").expect("seconds");
                let got = match v.strip_prefix("1/") {
                    Some(d) => 1.0 / parse("shutter", d),
                    None => parse("shutter", v),
                };
                // 1/N display rounds N (e.g. 1/3 s for 0.3 s), so allow 12 %.
                near("shutter", got, t, t * 0.12);
            }
            None => assert_eq!(row("Shutter"), "—"),
        }
        match num("exposure_compensation") {
            Some(ev) => {
                let r = row("Exposure comp.");
                let v = r.strip_suffix(" EV").expect("EV");
                near(
                    "exposure comp.",
                    parse("ev", v.trim_start_matches('+')),
                    ev,
                    0.051,
                );
            }
            None => assert_eq!(row("Exposure comp."), "—"),
        }
        match s("date_time_original") {
            Some(d) => {
                let (date, time) = d.split_once(' ').expect("date time");
                let mut want_row = format!("{} {time}", date.replace(':', "-"));
                if let Some(o) = s("offset_time_original") {
                    want_row = format!("{want_row} {o}");
                }
                assert_eq!(row("Captured"), want_row, "{}: capture time", e.path);
            }
            None => assert_eq!(row("Captured"), "—"),
        }
        assert_eq!(
            row("Dimensions"),
            format!("{} × {}", e.expected.sensor_w, e.expected.sensor_h),
            "{}: dimensions",
            e.path
        );
        assert_eq!(
            row("Preview"),
            format!(
                "{} · {} × {}",
                e.expected.badge, e.expected.preview_w, e.expected.preview_h
            ),
            "{}: preview",
            e.path
        );
    }

    #[test]
    fn req_cli_1_list_on_the_corpus_golden() {
        let root = cache_dir();
        let (ok, out) = cli(&["list", "-r", root.to_str().unwrap()]);
        assert!(ok);
        assert_eq!(out.lines().count(), manifest().files.len());
        check(&snapshot("list.txt"), &out);
    }
}
