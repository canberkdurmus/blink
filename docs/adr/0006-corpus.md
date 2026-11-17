# 0006. Test corpus selection and expected-value bootstrap

Date: 2026-11-17 · Status: accepted

## Context

SPEC §11.3 asks for a redistributable corpus from raw.pixls.us covering Sony (an old body with a
~1616×1080 preview and an A1-class body with two previews), Canon CR2 and CR3, Nikon NEF incl. a Z
body, Fujifilm X-Trans, OM/Olympus, Panasonic, Pentax and Leica/phone DNG. §11.4 says expected
values are bootstrapped from exiftool and then frozen.

## Decision

- 12 files, all **CC0-1.0** (license column of raw.pixls.us' repository listing), smallest suitable
  sample per brand: Sony NEX-5N and ILCE-1M2 (A1 II: 1616×1080 preview + 8640×5760 JpgFromRaw),
  Canon EOS 20D (CR2) and EOS R6 (CR3), Nikon Z 6 and Z6_3 (HE* compression that rawler 0.8.0 cannot
  decode — kept on purpose as the "preview works, RAW decode unsupported" case), Fujifilm X-S10
  (X-Trans IV), Olympus E-M10, Panasonic DMC-LX7, Pentax K10D, Leica M8 (DNG with an uncompressed
  320×240 preview) and Apple iPhone 6s Plus (phone DNG). ~210 MB in total, cached in CI.
- Expected values in `corpus/manifest.toml`:
  - `exif.*` from `exiftool -j -n` (13.25), **standard EXIF tags only** (Make, Model, LensModel
    0xA434, FocalLength, ExposureTime, FNumber, ISO, ExposureCompensation, DateTimeOriginal,
    OffsetTimeOriginal, Orientation). Absent tags are omitted, meaning "Blink shows nothing".
    Maker-note and composite values (e.g. `LensID` from exiftool's lens databases) are out of scope:
    reproducing exiftool's lens tables is not a culling feature.
  - `preview_w/h`: the largest embedded image, measured by extracting every embedded image with
    exiftool (`-b -PreviewImage/-JpgFromRaw/-OtherImage/-ThumbnailImage`) and the uncompressed IFD0
    image of the Leica DNG.
  - `sensor_w/h`: the camera's recommended crop as reported by rawler 0.8.0 (equals EXIF
    ExifImageWidth/Height or RAF RawImageCroppedSize where the file has them). For files rawler cannot
    decode, the raw IFD's ImageWidth/Height.
  - `badge`: SPEC §3 rule, LOW-RES when the preview's long edge < 50 % of the sensor's long edge.
    DMC-LX7 (1920 / 3776 = 50.8 %) sits just above the threshold on purpose.

## Alternatives

- Larger "typical" files (24–45 MP for every brand): more realistic perf numbers, but ~1 GB of cache.
  Perf tests use the 45 MP-class A1 II file plus generated JPEGs instead.
- Non-CC0 files (CC-BY-SA): allowed by SPEC if redistributable, but CC0 covers every brand needed.

## Consequences

Changing any expected value needs a `golden-update:` trailer. Adding files is a manifest change too.
