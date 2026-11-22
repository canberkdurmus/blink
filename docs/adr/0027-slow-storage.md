# 0027. PERF-9: the prefetch window adapts to measured reads; no I/O on the UI thread

Status: accepted (2026-11-22)

## Context

PERF-9: "Slow storage (USB/network): Prefetch window adapts to measured throughput; UI thread
never blocks on I/O". SPEC §8.3 fixes the window at 8 ahead and 2 behind. The SPEC doesn't say
which way it should adapt. The loader reads whole files. A read that has started can't be
cancelled, and cancellation is checked only between read and decode. So on a USB 2 disk
(about 30 MB/s, 50 MB RAWs, about 1.7 s per read), the full 10-item window keeps the device
busy for about 8 s with two workers. When the user moves on, the next item waits behind reads
of items they may never reach.

## Decision

- The loader measures every whole-file read through `Fs`. It smooths the time per read with
  the same 0.7/0.3 average as the PERF-3 pace.
- Throughput is `workers / read_secs` items per second. The measured time already includes
  contention between parallel reads. `Show` keeps the first
  `floor(HORIZON × workers / read_secs)` items of the window in priority order
  (`HORIZON` = 2 s). It always keeps at least the nearest item ahead and the nearest behind,
  and never more than the full window.
- Fast storage, or no read measured yet, keeps the full SPEC window. For example, NVMe reads a
  50 MB RAW in about 20 ms, which gives 400 items in the horizon.
- Reading this way is the simplest one that matches the acceptance column. The window shrinks
  to what the storage can deliver within the horizon. It doesn't grow beyond the SPEC's 8/2,
  because §8.3 fixes that size.
- The window adapts on the next `Show` after each read. No extra thread or timer is needed.
- The UI thread makes no `Fs` call. The last one was `open_path`, which tells a folder from a
  file with `metadata`. It now runs on a short-lived thread and sends `OpenFolder` back. A
  sequence number makes the latest open win.
- `SlowFs` is a test wrapper. It adds a fixed latency to every call and records which thread
  made each call.
- Tests:
  - `req_perf_9_slow_reads_shrink_the_prefetch_window_to_the_measured_throughput` checks
    which files are read at about 1 s per read.
  - `req_perf_9_the_ui_thread_never_touches_slow_storage` sends opening, navigation, marks,
    filters, undo, zoom and a rescan through the headless app at 30 ms per call. It asserts
    that no call came from the UI thread.

## Consequences

- On slow storage, browsing one item at a time still prefetches the next item first. A held
  key relies on the PERF-3 lead logic, which already uses the measured job latency.
- Config reading at launch happens before the event loop starts, not during interaction.
