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
}
