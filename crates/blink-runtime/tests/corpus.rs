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
}
