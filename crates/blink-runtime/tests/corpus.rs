//! OPEN-1 on the real corpus (full tier).

mod full_corpus {
    use blink_core::formats::Formats;
    use blink_core::item::build_items;
    use blink_decode::raw::RawBackend;
    use blink_runtime::fs::RealFs;
    use blink_runtime::scan::scan_folder;
    use blink_testkit::corpus::{cache_dir, manifest};
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    #[test]
    fn req_open_1_corpus_item_list_equals_filtered_directory_listing() {
        let formats = Formats::new(blink_decode::default_backend().extensions());
        let root = cache_dir();
        // Recursive open of the corpus root: one item per manifest file.
        let files = scan_folder(&RealFs, &root, true).unwrap();
        let items = build_items(&files, &formats);
        let got: BTreeSet<PathBuf> = items.iter().map(|i| i.primary.path.clone()).collect();
        let want: BTreeSet<PathBuf> = manifest()
            .files
            .iter()
            .map(|e| root.join(&e.path))
            .collect();
        assert_eq!(got, want);
        // Each brand folder, non-recursively: items == std::fs listing filtered by extension.
        for dir in std::fs::read_dir(&root).unwrap().flatten() {
            if !dir.path().is_dir() {
                continue;
            }
            let files = scan_folder(&RealFs, &dir.path(), false).unwrap();
            let got: BTreeSet<PathBuf> = build_items(&files, &formats)
                .iter()
                .flat_map(|i| i.members().map(|m| m.path.clone()).collect::<Vec<_>>())
                .collect();
            let want: BTreeSet<PathBuf> = std::fs::read_dir(dir.path())
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file())
                .filter(|p| {
                    formats
                        .classify(&p.file_name().unwrap().to_string_lossy())
                        .is_some()
                })
                .collect();
            assert_eq!(got, want, "{}", dir.path().display());
            assert!(!got.is_empty());
        }
    }

    /// SPEC VIEW-4 acceptance: zoom on a LOW-RES ARW ends with source `RawDecode` and pixel
    /// dimensions equal to the sensor's active area.
    #[test]
    fn req_view_4_zoom_on_low_res_arw_ends_with_raw_decode_at_sensor_size() {
        use blink_core::preview::PreviewSource;
        use blink_core::state::{Action, State, reduce};
        use blink_runtime::loader::Loader;
        use std::sync::Arc;
        use std::time::{Duration, Instant};

        let e = manifest()
            .files
            .into_iter()
            .find(|e| e.path == "sony/NEX-5N.ARW")
            .unwrap();
        assert_eq!(e.expected.badge, "EMBEDDED · LOW-RES");
        let backend = blink_decode::default_backend();
        let formats = Formats::new(backend.extensions());
        let loader: Loader<()> = Loader::new(
            Arc::new(RealFs),
            Arc::new(backend),
            2,
            1 << 30,
            |_| (),
            || {},
        );
        let folder = e.file().parent().unwrap().to_path_buf();
        let mut state = State::new(formats);
        let mut queue = vec![
            Action::OpenFolder(folder.clone()),
            Action::FolderScanned {
                folder: folder.clone(),
                recursive: false,
                files: scan_folder(&RealFs, &folder, false).unwrap(),
            },
        ];
        let t = Instant::now();
        let mut zoomed = false;
        loop {
            for a in std::mem::take(&mut queue) {
                let (s, fx) = reduce(&state, a);
                state = s;
                for f in &fx {
                    if let Some(a) = loader.execute(f) {
                        queue.push(a);
                    }
                }
            }
            if !queue.is_empty() {
                continue;
            }
            let on_file = state.current_item().map(|i| i.primary.path.clone()) == Some(e.file());
            if !on_file {
                queue.push(Action::Navigate(blink_core::state::Nav::Next));
                continue;
            }
            match &state.loupe.shown {
                Some(x) if !zoomed => {
                    assert_eq!(x.source, PreviewSource::EmbeddedLowRes);
                    zoomed = true;
                    queue.push(Action::ToggleZoom100 { cursor: None });
                    continue;
                }
                Some(x) if zoomed && !state.loupe.pending_full => {
                    assert_eq!(x.source, PreviewSource::RawDecode);
                    assert_eq!(x.size, (e.expected.sensor_w, e.expected.sensor_h));
                    let l = loader.best(&e.file()).unwrap();
                    assert_eq!(
                        (l.preview.image.width, l.preview.image.height),
                        (e.expected.sensor_w, e.expected.sensor_h)
                    );
                    break;
                }
                _ => {}
            }
            assert!(t.elapsed() < Duration::from_secs(60), "{:?}", state.loupe);
            if let Some(a) = loader.wait(Duration::from_millis(100)) {
                queue.push(a);
            }
        }
    }

    /// SPEC FILT-1 acceptance: filter results equal the expected sets on the corpus. Expected
    /// sets come from the frozen EXIF values in `corpus/manifest.toml`, evaluated here directly.
    #[test]
    fn req_filt_1_req_filt_2_filter_results_and_capture_order_on_the_corpus() {
        use blink_core::filter::{Filter, ItemMeta};
        use blink_core::sort::SortKey;
        use blink_core::state::{Action, State, reduce};
        use std::collections::BTreeMap;

        let formats = Formats::new(blink_decode::default_backend().extensions());
        let root = cache_dir();
        let m = manifest();
        let mut s = State::new(formats.clone());
        s = reduce(&s, Action::OpenFolder(root.clone())).0;
        s = reduce(&s, Action::SetRecursive(true)).0;
        let files = scan_folder(&RealFs, &root, true).unwrap();
        s = reduce(
            &s,
            Action::FolderScanned {
                folder: root.clone(),
                recursive: true,
                files,
            },
        )
        .0;
        let meta = blink_runtime::meta::load_meta(&RealFs, &s.items);
        assert_eq!(meta.len(), m.files.len(), "every corpus file has metadata");
        s = reduce(&s, Action::MetaLoaded(meta)).0;

        // Expected metadata straight from the manifest.
        let want: BTreeMap<PathBuf, ItemMeta> = m
            .files
            .iter()
            .map(|e| {
                let x = &e.expected.exif;
                let st = |k: &str| x.get(k).and_then(|v| v.as_str()).map(String::from);
                let num = |k: &str| {
                    x.get(k)
                        .and_then(|v| v.as_float().or(v.as_integer().map(|i| i as f64)))
                };
                (
                    root.join(&e.path),
                    ItemMeta {
                        body: blink_core::info::body(st("make").as_deref(), st("model").as_deref()),
                        lens: st("lens"),
                        focal_mm: num("focal_length"),
                        iso: num("iso").map(|v| v as u32),
                        captured: st("date_time_original"),
                    },
                )
            })
            .collect();
        assert_eq!(s.meta, want);

        let rel = |s: &State| -> BTreeSet<String> {
            s.visible_items()
                .map(|i| {
                    i.primary
                        .path
                        .strip_prefix(&root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/")
                })
                .collect()
        };
        let expect = |pred: &dyn Fn(&ItemMeta) -> bool| -> BTreeSet<String> {
            m.files
                .iter()
                .filter(|e| pred(&want[&root.join(&e.path)]))
                .map(|e| e.path.clone())
                .collect()
        };
        let in_range = |v: Option<f64>, lo: f64, hi: f64| v.is_some_and(|v| v >= lo && v <= hi);
        type Pred = Box<dyn Fn(&ItemMeta) -> bool>;
        let cases: Vec<(&str, Pred)> = vec![
            (
                "body=SONY NEX-5N",
                Box::new(|x| x.body.as_deref() == Some("SONY NEX-5N")),
            ),
            (
                "body=canon eos r6",
                Box::new(|x| x.body.as_deref() == Some("Canon EOS R6")),
            ),
            (
                "lens=NIKKOR Z 24-70mm f/4 S",
                Box::new(|x| x.lens.as_deref() == Some("NIKKOR Z 24-70mm f/4 S")),
            ),
            (
                "focal=24..70",
                Box::new(move |x| in_range(x.focal_mm, 24.0, 70.0)),
            ),
            (
                "focal=..10",
                Box::new(move |x| in_range(x.focal_mm, 0.0, 10.0)),
            ),
            ("iso=..100", Box::new(|x| x.iso.is_some_and(|i| i <= 100))),
            (
                "iso=160..800",
                Box::new(|x| x.iso.is_some_and(|i| (160..=800).contains(&i))),
            ),
            (
                "date=2017-01-01..2017-12-31",
                Box::new(|x| {
                    x.captured
                        .as_deref()
                        .is_some_and(|c| c.starts_with("2017:"))
                }),
            ),
            (
                "date=2020-11-28",
                Box::new(|x| {
                    x.captured
                        .as_deref()
                        .is_some_and(|c| c.starts_with("2020:11:28"))
                }),
            ),
            ("kind=dng", Box::new(|_| false)),
            (
                "iso=100,focal=..60",
                Box::new(move |x| x.iso == Some(100) && in_range(x.focal_mm, 0.0, 60.0)),
            ),
        ];
        for (expr, pred) in &cases {
            let got = rel(&reduce(&s, Action::SetFilter(Filter::parse(expr).unwrap())).0);
            let mut want_set = expect(pred.as_ref());
            if *expr == "kind=dng" {
                want_set = m
                    .files
                    .iter()
                    .filter(|e| e.path.to_ascii_lowercase().ends_with(".dng"))
                    .map(|e| e.path.clone())
                    .collect();
            }
            assert_eq!(got, want_set, "{expr}");
        }
        // Spot checks, so an empty expectation can't pass silently.
        let count =
            |e: &str| rel(&reduce(&s, Action::SetFilter(Filter::parse(e).unwrap())).0).len();
        assert_eq!(count("body=SONY NEX-5N"), 1);
        assert_eq!(count("kind=dng"), 2);
        assert_eq!(count("date=2017-01-01..2017-12-31"), 3);
        assert_eq!(count("iso=..100"), 7);

        // FILT-2: the default capture-time order equals the manifest dates sorted.
        assert_eq!(s.sort, SortKey::CaptureTime);
        let got: Vec<PathBuf> = s.visible_items().map(|i| i.primary.path.clone()).collect();
        let mut by_date: Vec<(&str, PathBuf)> = m
            .files
            .iter()
            .map(|e| {
                (
                    e.expected.exif["date_time_original"].as_str().unwrap(),
                    root.join(&e.path),
                )
            })
            .collect();
        by_date.sort();
        assert_eq!(got, by_date.into_iter().map(|(_, p)| p).collect::<Vec<_>>());
    }
}
