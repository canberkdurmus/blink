# Progress

Current milestone: M3

Ticked IDs list their tests. `cargo xtask trace` requires every ticked ID and every ID of earlier
milestones to have a tagged, passing test.

## Next up

M2 closed (gate report `docs/reports/M2.md`, pre-release `v1.0.0-m2`). **M3 Ratings and sidecars**
starts with **FILE-4** (atomic sidecar writes through `Fs`, `blink-runtime`), which unblocks RATE-1/2/3
(rating, label and reject keys in `blink-core` reducer + UI) and OPEN-2's on-disk per-member sidecars and
CLI `rate`/`label` without `--dry-run` (ADR 0010). Then FILE-5..9 and PERF-7 (bench script for
rating key → durable sidecar). The gate also needs the exiftool oracle, crash injection and two
green nightly darktable interop runs in a row (SPEC §13).

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

- [x] OPEN-1 — `req_open_1_corpus_item_list_equals_filtered_directory_listing`, `req_open_1_non_recursive_lists_only_the_folder`, `req_open_1_open_folder_is_non_recursive_by_default_and_toggle_rescans`, `req_open_1_recursive_does_not_follow_directory_links`, `req_open_1_recursive_scan_terminates_on_a_real_symlink_loop`, `req_open_1_recursive_skips_hidden_and_rejected`, `req_open_1_scan_only_reads_directories` (#8) · GUI entry points (drop, dialog, `blink [folder]`): `req_open_1_dropping_a_folder_or_file_opens_its_folder`, `req_open_1_a_relative_file_argument_opens_the_current_folder`, `req_view_1_command_line_folder_opens_in_the_loupe_fit_with_the_embedded_preview` (#14, ADR 0013)
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

- [x] VIEW-1 — `req_view_1_a_load_in_flight_during_clear_never_fills_the_cache`, `req_view_1_arrow_keys_move_and_no_frame_shows_a_stale_item`, `req_view_1_a_running_prefetch_still_wanted_by_a_newer_show_is_not_dropped`, `req_view_1_command_line_folder_opens_in_the_loupe_fit_with_the_embedded_preview`, `req_view_1_first_item_shows_its_embedded_preview_and_neighbours_are_prefetched`, `req_view_1_fit_scales_the_image_into_the_viewport_and_centres_it`, `req_view_1_heif_and_unreadable_files_show_no_preview`, `req_view_1_loupe_screenshot_golden`, `req_view_1_navigation_moves_the_current_item_and_never_shows_stale_pixels`, `req_view_1_opening_shows_the_first_item_fit_with_its_embedded_preview` (#13, #14)
- [x] VIEW-4 — `req_view_4_full_resolution_is_needed_only_when_the_preview_would_be_upscaled`, `req_view_4_paired_jpeg_is_named_in_the_full_request_and_failure_clears_the_spinner`, `req_view_4_raw_decode_has_the_sensor_crop_size_on_every_decodable_corpus_file`, `req_view_4_zoom_100_on_low_res_raw_ends_with_raw_decode_at_sensor_size`, `req_view_4_zoom_at_cursor_keeps_the_point_under_the_cursor`, `req_view_4_zoom_on_low_res_arw_ends_with_raw_decode_at_sensor_size`, `req_view_4_zoom_on_low_res_raw_loads_the_raw_decode_and_swaps_it_in`, `req_view_4_zoom_on_low_res_raw_with_a_paired_jpeg_uses_the_jpeg`, `req_view_4_z_zooms_to_100_percent_with_every_pixel_and_req_view_9_keeps_it_on_the_next_item` (#13, #14)
- [x] VIEW-6 — `req_view_6_clipping_and_info_panel_toggle`, `req_view_6_clipping_mask_is_exact_on_a_synthetic_image`, `req_view_6_histograms_are_exact_and_labelled_with_the_source`, `req_view_6_j_toggles_the_clipping_overlay_and_i_shows_the_labelled_histogram`, `req_view_6_overlay_tiles_colour_clipped_pixels`, `req_view_6_thresholds_are_254_any_channel_and_1_all_channels` (#13, #14)
- [x] VIEW-9 — `req_view_9_pan_with_drag_moves_the_image_and_stays_inside`, `req_view_9_zoom_levels_are_fit_50_100_200_of_the_full_image`, `req_view_9_zoom_position_is_kept_on_the_next_item_and_toggleable` (#13, #14)
- [x] PERF-1 — `req_perf_1_launch_to_first_image_with_2000_raws` (#13, #14) · GUI headless bench (ADR 0014)
- [x] PERF-2 — `req_perf_2_next_item_prefetched_is_drawn_at_once` (#13, #14) · GUI headless bench (ADR 0014)
- [x] PERF-3 — `req_perf_3_held_arrow_key_never_shows_a_stale_item`, `req_perf_3_lead_is_the_first_item_not_passed_when_a_decode_ends`, `req_perf_3_lead_is_zero_while_decodes_keep_up`, `req_perf_3_lead_items_go_first_and_the_current_item_first_once_settled`, `req_perf_3_show_lists_the_items_ahead_in_the_travel_direction_nearest_first`, `req_perf_3_steady_steps_measure_the_interval_and_a_stop_settles` (#13, #14) · GUI headless bench (ADR 0014)
- [x] PERF-5 — `req_perf_5_decode_raw_requests_a_raw_decode_even_with_a_full_size_preview_or_jpeg`, `req_perf_5_full_raw_decode_45mp_for_100_percent_zoom` (#13, #14) · GUI headless bench (ADR 0014)

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
- 2026-11-20 · PERF-4 closed (#10), M1 gate passed · parallel JPEG decoder (ADR 0011), bench writes results.md (#11), v1.0.0-m1
- 2026-11-21 · VIEW-1, VIEW-4, VIEW-6, VIEW-9, PERF-1, PERF-2, PERF-3, PERF-5 closed (#14, #15), M2 gate passed · OPEN-1 relative-path fix, PERF-3 decode-ahead past the prefetch window, v1.0.0-m2
