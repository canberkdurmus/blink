//! FILE-7 (and FILE-1 apply rejects + undo): a full GUI session on real files (the whole corpus plus synthetic pairs and foreign
//! sidecars, copied to a temp folder) never changes a byte of any RAW/JPEG/HEIF file.

mod full_safety {
    use blink_core::state::{Action, Nav};
    use blink_core::view::Zoom;
    use blink_testkit::{corpus, fixtures, sidecars};
    use blink_ui::{BlinkApp, Options};
    use egui_kittest::Harness;
    use std::collections::BTreeMap;
    use std::hash::{Hash, Hasher};
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    fn digest(p: &Path) -> (u64, u64) {
        let b = std::fs::read(p).unwrap();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        b.hash(&mut h);
        (b.len() as u64, h.finish())
    }

    /// Every non-sidecar file under `dir` with its length and hash.
    fn sources(dir: &Path) -> BTreeMap<PathBuf, (u64, u64)> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("xmp")))
            .map(|p| (p.clone(), digest(&p)))
            .collect()
    }

    fn settle(h: &mut Harness<'static, BlinkApp>) {
        let t = Instant::now();
        loop {
            h.step();
            let s = h.state();
            let cur = s.state().current_item().map(|i| i.primary.path.clone());
            let shown = s.displayed().map(|d| Some(&d.path) == cur.as_ref()) == Some(true);
            let failed = s.state().loupe.shown.is_some();
            if (shown || failed) && s.loader_idle() && !s.state().loupe.pending_full {
                h.step();
                return;
            }
            assert!(t.elapsed() < Duration::from_secs(120), "never settled");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn wait(h: &mut Harness<'static, BlinkApp>, done: impl Fn(&BlinkApp) -> bool) {
        let t = Instant::now();
        while !done(h.state()) {
            assert!(t.elapsed() < Duration::from_secs(60), "timed out");
            h.step();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn req_file_7_a_full_session_leaves_every_source_file_byte_identical() {
        let dir = std::env::temp_dir().join(format!("blink-file7-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for e in corpus::manifest().files {
            let name = e.path.replace(['/', '\\'], "_");
            std::fs::copy(e.file(), dir.join(name)).unwrap();
        }
        // Synthetic pairs, a standalone JPEG and HEIF, sidecars in every app's style.
        let preview = fixtures::orientation_jpeg(1);
        let raw = fixtures::tiff_with_jpegs(std::slice::from_ref(&preview));
        for (name, bytes) in [
            ("pair.ARW", raw.clone()),
            ("pair.JPG", preview.clone()),
            ("pair.HEIC", b"heif".to_vec()),
            ("solo.JPG", fixtures::orientation_jpeg(6)),
            ("solo.HEIC", b"heif".to_vec()),
            ("lr.ARW", raw.clone()),
            ("dt.ARW", raw.clone()),
            ("br.ARW", raw),
        ] {
            std::fs::write(dir.join(name), bytes).unwrap();
        }
        std::fs::write(dir.join("lr.xmp"), sidecars::lightroom(2, Some("Red"))).unwrap();
        std::fs::write(dir.join("dt.ARW.xmp"), sidecars::darktable(1)).unwrap();
        std::fs::write(dir.join("br.xmp"), sidecars::bridge(3, None)).unwrap();
        let before = sources(&dir);

        let app = BlinkApp::new(
            &egui::Context::default(),
            Some(dir.clone()),
            Options {
                workers: 2,
                ..Options::default()
            },
        );
        let mut h = Harness::builder()
            .with_size([800.0, 500.0])
            .build_ui_state(|ui, app: &mut BlinkApp| app.show(ui), app);
        settle(&mut h);
        let n = h.state().state().items.len();
        assert!(n >= before.len() / 2, "{n} items");
        for i in 0..n {
            let a = h.state_mut();
            a.dispatch(Action::SetRating {
                item: i,
                rating: (i % 7) as i8 - 1,
            });
            if i % 2 == 0 {
                a.dispatch(Action::SetLabel {
                    item: i,
                    label: Some(["Red", "Green", "Purple"][i % 3].into()),
                });
            }
            if i < 3 {
                // Zoom: paired JPEG or a real RAW decode of the file.
                a.dispatch(Action::SetZoom {
                    zoom: Zoom::P100,
                    cursor: None,
                });
                settle(&mut h);
                h.state_mut().dispatch(Action::SetZoom {
                    zoom: Zoom::Fit,
                    cursor: None,
                });
            }
            h.state_mut().dispatch(Action::Navigate(Nav::Next));
            settle(&mut h);
        }
        // FILE-1 on the real file system: apply rejects, check the moved bytes, undo.
        let rejected: usize = (0..n).filter(|i| i % 7 == 0).count();
        h.state_mut().dispatch(Action::ApplyRejects);
        h.state_mut().dispatch(Action::Confirm);
        wait(&mut h, |a| a.state().items.len() == n - rejected);
        let moved = dir.join("_rejected");
        let mut in_rejected = 0;
        for e in std::fs::read_dir(&moved).unwrap() {
            let p = e.unwrap().path();
            if p.extension().is_some_and(|x| x == "xmp") {
                continue;
            }
            assert_eq!(
                before.get(&dir.join(p.file_name().unwrap())),
                Some(&digest(&p)),
                "{} moved intact",
                p.display()
            );
            in_rejected += 1;
        }
        assert!(in_rejected >= rejected);
        h.state_mut().dispatch(Action::Undo);
        wait(&mut h, |a| {
            a.state().items.len() == n && !a.state().file_op_running
        });
        assert_eq!(std::fs::read_dir(&moved).unwrap().count(), 0);
        std::fs::remove_dir(&moved).unwrap();
        h.state().flush_sidecars();
        drop(h);

        let after = sources(&dir);
        assert_eq!(
            before.keys().collect::<Vec<_>>(),
            after.keys().collect::<Vec<_>>(),
            "files appeared or vanished"
        );
        for (p, d) in &before {
            assert_eq!(after.get(p), Some(d), "{} changed", p.display());
        }
        // The session did write: every item has a sidecar now.
        let xmps = std::fs::read_dir(&dir)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|x| x == "xmp")
            })
            .count();
        assert!(xmps >= n, "{xmps} sidecars for {n} items");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
