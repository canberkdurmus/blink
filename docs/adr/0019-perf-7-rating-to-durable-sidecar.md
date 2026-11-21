# 0019. PERF-7: measuring "rating key → sidecar durable on disk"

Date: 2026-11-22 · Status: accepted

## Context

PERF-7's budget is 250 ms from a rating key to the sidecar being durable on disk. Writes are
debounced (ADR 0015 §5: 100 ms after the first queued change) and run on the sidecar I/O thread.

## Decision

- Script `perf/scripts/perf-7-rate-durable.json`, scenario `rate_durable`, 21 runs, through the
  same headless bench as PERF-1/2/3/5 (ADR 0014) on `RealFs`. Each run presses a star key
  (`1`…`5` in turn) on the next item and stops the clock when the sidecar service reports that
  write done. The service reports it after the whole atomic write: temp file, fsync, rename and
  directory fsync (FILE-6). So the number includes the debounce and the durability steps.
  `BlinkApp::sidecar_writes_done` counts completed writes.
- Every run creates a new sidecar. That is the slower case, because there is no file to read and
  splice first, but it pays the same fsyncs. The test checks every sidecar's rating afterwards,
  and that the RAW is byte-identical (FILE-7).
- The folder (`target/perf-fixtures/rate-21`, 21 hard links to the PERF-1 file) belongs to
  PERF-7 alone and is rebuilt on every run. That way no sidecar lands in the shared PERF-1/2/3
  folder.
- The bench now sends a key release after every press. Without one, egui reports a second
  press of the same key as a repeat, and mark keys ignore repeats (ADR 0016).
- The Linux CI baseline is taken from the first CI run of the bench (`golden-update:`), the
  same way as ADR 0014.
