# 0009. XMP reading: quick-xml, namespace matching, sidecar precedence

Date: 2026-11-18 · Status: accepted

## Context

RATE-4 says Blink reads existing ratings and labels from sidecars on open, and otherwise from the
XMP embedded in DNG/JPEG, without writing anything. Sidecars come from Lightroom (attribute form),
Bridge (element form inside an `xpacket`), darktable (attributes, labels in its own namespace) and
digiKam (attributes, plus Microsoft/digiKam rating fields). SPEC §8.4 fixes `quick-xml`.

## Decision

- **`quick-xml` `=0.42.0`** in `blink-xmp`. License MIT, actively maintained, pure Rust, builds on
  all 6 targets, no network. Exact pin because the event API changes between minors.
  ADR 0002 already covers it for xtask.
- Fields are matched by **namespace URI**, not by prefix, so `xap:Rating` in old files counts and
  `MicrosoftPhoto:Rating` doesn't. Both forms are read: attributes on `rdf:Description` and child
  elements. The first occurrence wins. Ratings are XMP reals rounded to −1…5; other values count as
  absent. darktable's `darktable:colorlabels` is not `xmp:Label` and is not read (SPEC §6 owns
  `xmp:Label` only).
- Embedded XMP: JPEG APP1 with the `http://ns.adobe.com/xap/1.0/\0` signature, and DNG/TIFF IFD0
  tag 700.
- **Precedence on open (runtime):** for each member file, the darktable-style sidecar
  (`IMG.ARW.xmp`) and the Adobe-style one (`IMG.xmp`) are both considered, and the one with the newer
  modification time wins (Adobe-style on a tie, matching D-3). If neither exists, the embedded XMP
  of a DNG/JPEG is used, for display only. An item's marks come from its primary file first, then
  from its companions.

## Consequences

The same crate gets the event-preserving writer in M3 (FILE-5).
