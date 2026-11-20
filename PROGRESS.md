# Progress

Current milestone: M1

Ticked IDs list their tests. `cargo xtask trace` requires every ticked ID and every ID of earlier
milestones to have a tagged, passing test.

## Next up

M1 gate in progress: PERF-4 closed (#10), full tier 87/87. Remaining: `cargo xtask report M1`, tag
`v0.1.0-m1`, then M2 starting with **VIEW-1** (GUI entry points per ADR 0010).

## M0 Foundations (infrastructure)

- [x] Workspace and crate stubs, empty egui window
- [x] xtask: verify, trace, corpus fetch, bench, report, guard
- [x] PROGRESS.md, BLOCKED.md, HUMAN_TODO.md, ADR 0001, docs/ideas.md
- [x] Workflows: ci.yml (6 targets), nightly.yml, weekly.yml, release.yml
- [x] `.claude/` settings, hooks, agents, skill
- [x] CI green on all 6 targets (run 44182943822)
- [x] `v0.0.1-m0` pre-release built on all targets
- [x] Gate report docs/reports/M0.md

## M1 Headless core

- [x] OPEN-1 — `req_open_1_corpus_item_list_equals_filtered_directory_listing`, `req_open_1_non_recursive_lists_only_the_folder`, `req_open_1_open_folder_is_non_recursive_by_default_and_toggle_rescans`, `req_open_1_recursive_does_not_follow_directory_links`, `req_open_1_recursive_scan_terminates_on_a_real_symlink_loop`, `req_open_1_recursive_skips_hidden_and_rejected`, `req_open_1_scan_only_reads_directories` (#8) · GUI entry points (drop, dialog, `blink [folder]`) land with VIEW-1 in M2 (ADR 0010)
- [x] OPEN-2 — `req_open_2_label_applies_to_every_pair_member_and_only_the_label_changes`, `req_open_2_n_pairs_show_as_n_items`, `req_open_2_pairing_rules`, `req_open_2_pairs_only_within_the_same_folder`, `req_open_2_rating_applies_to_every_pair_member` (#8, #9) · on-disk sidecar per member completes with FILE-4 in M3 (ADR 0010)
- [x] OPEN-4 — `req_open_4_classifies_by_extension_case_insensitively`, `req_open_4_doctor_lists_format_support`, `req_open_4_every_corpus_file_is_a_supported_raw_format` (#6, #9)
- [x] VIEW-2 — `req_view_2_largest_of_several_embedded_previews_is_chosen`, `req_view_2_largest_preview_chosen_per_camera`, `req_view_2_low_res_threshold_is_half_the_sensor_long_edge` (#6)
- [x] VIEW-3 — `req_view_3_badge_matches_corpus_and_preview_decodes`, `req_view_3_badge_texts` (#6)
- [x] VIEW-7 — `req_view_7_exif_matches_frozen_exiftool_values`, `req_view_7_formats_every_field`, `req_view_7_unknown_values_show_a_dash`, `req_view_7_value_formatting_helpers` (#6, #9)
- [x] VIEW-8 — `req_view_8_all_eight_orientations_display_upright`, `req_view_8_display_only_input_bytes_unchanged`, `req_view_8_orientation_of_raw_container_applies_to_embedded_preview` (#6)
- [x] VIEW-10 — `req_view_10_adobe_rgb_preview_converts_within_delta_e_2`, `req_view_10_srgb_tagged_preview_is_not_converted` (#6)
- [x] RATE-4 — `req_rate_4_embedded_xmp_in_jpeg_and_dng`, `req_rate_4_embedded_xmp_used_when_no_sidecar_exists`, `req_rate_4_label_with_entities`, `req_rate_4_late_marks_load_does_not_override_user_rating`, `req_rate_4_namespace_matched_by_uri_not_prefix`, `req_rate_4_newest_sidecar_wins_when_both_styles_exist`, `req_rate_4_pair_uses_companion_sidecar_when_primary_has_none`, `req_rate_4_reads_bridge_element_form`, `req_rate_4_reads_darktable_sidecar_including_reject`, `req_rate_4_reads_digikam_sidecar_and_ignores_foreign_ratings`, `req_rate_4_reads_lightroom_sidecar`, `req_rate_4_sidecars_from_each_app_are_read_on_open` (#7, #8)
- [x] CLI-1 — `req_cli_1_export_golden`, `req_cli_1_filter_expressions`, `req_cli_1_list_and_filter_golden`, `req_cli_1_list_on_the_corpus_golden`, `req_cli_1_rate_and_label_plan_writes_for_every_pair_member_without_writing`, `req_cli_1_scan_golden` (#9) · `rate`/`label` are `--dry-run` until M3 (ADR 0010)
- [x] CLI-2 — `req_cli_2_doctor_golden_for_synthetic_files`, `req_cli_2_doctor_on_every_corpus_file_matches_manifest_and_golden` (#9)
- [x] PLAT-2 — `req_plat_2_portable_marker_puts_config_and_cache_next_to_binary`, `req_plat_2_without_marker_uses_platform_standard_dirs`, `req_plat_2_marker_must_be_a_file_next_to_the_binary` (#5)
- [x] PERF-4 — `req_perf_4_embedded_preview_extract_and_decode_6mb_jpeg`, `req_perf_4_matches_zune_jpeg_within_rounding`, `req_perf_4_output_is_identical_for_any_thread_and_chunk_count`, `req_perf_4_unhandled_layouts_are_left_to_zune_jpeg`, `req_perf_4_corrupt_or_truncated_data_never_panics`, `req_perf_4_parallel_decoder_matches_zune_on_corpus_previews` (#10) · parallel in-file decode (ADR 0011)

## M2 Loupe and speed

- [ ] VIEW-1
- [ ] VIEW-4
- [ ] VIEW-6
- [ ] VIEW-9
- [ ] PERF-1
- [ ] PERF-2
- [ ] PERF-3
- [ ] PERF-5

## M3 Ratings and sidecars

- [ ] RATE-1
- [ ] RATE-2
- [ ] RATE-3
- [ ] FILE-4
- [ ] FILE-5
- [ ] FILE-6
- [ ] FILE-7
- [ ] FILE-8
- [ ] FILE-9
- [ ] PERF-7

## M4 Grid, filters, files

- [ ] VIEW-5
- [ ] FILT-1
- [ ] FILT-2
- [ ] FILE-1
- [ ] FILE-2
- [ ] FILE-3
- [ ] OPEN-3
- [ ] PERF-6
- [ ] PERF-8
- [ ] PERF-9

## M5 Release candidate

- [ ] KEY-1
- [ ] SESS-1
- [ ] A11Y-1
- [ ] PLAT-1
- [ ] Packaging (SPEC §9)
- [ ] README
- [ ] User guide

## Session log

- 2026-11-17 · M0 closed · workspace, xtask, 4 workflows green on 6 targets, .claude setup, v0.0.1-m0, ADRs 0001–0005
- 2026-11-19 · OPEN-1, OPEN-2, OPEN-4, VIEW-2, VIEW-3, VIEW-7, VIEW-8, VIEW-10, RATE-4, CLI-1, CLI-2 closed (#6–#9) · PERF-4 blocked (serial Huffman floor ~55 ms); parallel decoder prototype under way
