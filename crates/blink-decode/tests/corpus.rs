//! Corpus goldens (full tier, SPEC §11.1): run `cargo xtask corpus fetch` first.

mod full_corpus {
    use blink_core::formats::{FileKind, Formats};
    use blink_decode::raw::RawBackend;
    use blink_decode::{analyze, decode_embedded, default_backend};
    use blink_testkit::corpus::{Entry, manifest};

    fn kind(e: &Entry) -> FileKind {
        let f = Formats::new(default_backend().extensions());
        f.classify(&e.path)
            .unwrap_or_else(|| panic!("{} not supported", e.path))
    }

    #[test]
    fn req_open_4_every_corpus_file_is_a_supported_raw_format() {
        let f = Formats::new(default_backend().extensions());
        for e in manifest().files {
            let k = f.classify(&e.path);
            assert!(
                matches!(k, Some(FileKind::Raw | FileKind::Dng)),
                "{}: {k:?}",
                e.path
            );
        }
        // Major brands' extensions are all known to the backend.
        for ext in [
            "arw", "cr2", "cr3", "nef", "raf", "orf", "rw2", "pef", "srw", "rwl",
        ] {
            assert!(
                f.raw_extensions().any(|x| x == ext),
                "{ext} missing from {:?}",
                f.raw_extensions().collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn req_view_2_largest_preview_chosen_per_camera() {
        for e in manifest().files {
            let bytes = e.bytes();
            let a = analyze(&bytes, kind(&e), &default_backend());
            let c = a.chosen.unwrap_or_else(|| panic!("{}: no preview", e.path));
            assert_eq!(
                (c.width, c.height),
                (e.expected.preview_w, e.expected.preview_h),
                "{}: embedded {:?}",
                e.path,
                a.embedded
            );
            for other in &a.embedded {
                assert!(
                    c.width >= other.width && c.height >= other.height,
                    "{}: {other:?} beats {c:?}",
                    e.path
                );
            }
            assert_eq!(
                a.sensor,
                Some((e.expected.sensor_w, e.expected.sensor_h)),
                "{}: sensor",
                e.path
            );
        }
    }

    #[test]
    fn req_view_3_badge_matches_corpus_and_preview_decodes() {
        for e in manifest().files {
            let bytes = e.bytes();
            let a = analyze(&bytes, kind(&e), &default_backend());
            assert_eq!(a.default_source().badge(), e.expected.badge, "{}", e.path);
            let p = decode_embedded(&bytes, &a)
                .unwrap_or_else(|err| panic!("{}: preview decode failed: {err}", e.path));
            assert_eq!(p.source, a.default_source());
            let (w, h) = (e.expected.preview_w, e.expected.preview_h);
            let dims = (p.image.width, p.image.height);
            assert!(dims == (w, h) || dims == (h, w), "{}: {dims:?}", e.path);
        }
    }

    fn exif_matches(e: &Entry, exif: &blink_decode::exif::ExifInfo) {
        use toml::Value;
        let want = &e.expected.exif;
        let num = |k: &str| {
            want.get(k).map(|v| match v {
                Value::Integer(i) => *i as f64,
                Value::Float(f) => *f,
                other => panic!("{k}: {other:?}"),
            })
        };
        let s = |k: &str| want.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let close = |a: Option<f64>, b: Option<f64>, k: &str| match (a, b) {
            (Some(a), Some(b)) => assert!(
                (a - b).abs() <= 1e-6 * b.abs().max(1.0),
                "{}: {k} {a} != {b}",
                e.path
            ),
            (a, b) => assert_eq!(a, b, "{}: {k}", e.path),
        };
        assert_eq!(exif.make, s("make"), "{}", e.path);
        assert_eq!(exif.model, s("model"), "{}", e.path);
        assert_eq!(exif.lens, s("lens"), "{}", e.path);
        assert_eq!(
            exif.date_time_original,
            s("date_time_original"),
            "{}",
            e.path
        );
        assert_eq!(
            exif.offset_time_original,
            s("offset_time_original"),
            "{}",
            e.path
        );
        close(exif.focal_length, num("focal_length"), "focal_length");
        close(exif.exposure_time, num("exposure_time"), "exposure_time");
        close(exif.f_number, num("f_number"), "f_number");
        close(exif.iso.map(f64::from), num("iso"), "iso");
        close(
            exif.exposure_compensation,
            num("exposure_compensation"),
            "ev",
        );
        close(
            exif.orientation.map(f64::from),
            num("orientation"),
            "orientation",
        );
    }

    #[test]
    fn req_view_7_exif_matches_frozen_exiftool_values() {
        for e in manifest().files {
            let bytes = e.bytes();
            let a = analyze(&bytes, kind(&e), &default_backend());
            exif_matches(&e, &a.exif);
        }
    }
}
