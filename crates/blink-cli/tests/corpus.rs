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
            // … and the whole report as a golden, timings masked.
            check(
                &snapshot(&format!("doctor/{}.txt", e.path.replace('/', "__"))),
                &mask_timings(&out),
            );
        }
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
