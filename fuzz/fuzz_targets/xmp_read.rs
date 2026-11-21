//! Fuzz the XMP readers (RATE-4): never panic on any input.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = blink_xmp::read_owned(data);
    let _ = blink_xmp::jpeg_xmp_packet(data);
    let _ = blink_xmp::tiff_xmp_packet(data);
});
