//! RATE-4: read ratings and labels written by other apps.

use blink_testkit::sidecars;
use blink_xmp::{OwnedFields, jpeg_xmp_packet, read_owned, tiff_xmp_packet};

fn f(rating: i8, label: Option<&str>) -> OwnedFields {
    OwnedFields {
        rating: Some(rating),
        label: label.map(str::to_string),
    }
}

#[test]
fn req_rate_4_reads_lightroom_sidecar() {
    assert_eq!(
        read_owned(sidecars::lightroom(4, Some("Red")).as_bytes()).unwrap(),
        f(4, Some("Red"))
    );
    assert_eq!(
        read_owned(sidecars::lightroom(0, None).as_bytes()).unwrap(),
        f(0, None)
    );
}

#[test]
fn req_rate_4_reads_darktable_sidecar_including_reject() {
    assert_eq!(
        read_owned(sidecars::darktable(2).as_bytes()).unwrap(),
        f(2, None)
    );
    assert_eq!(
        read_owned(sidecars::darktable(-1).as_bytes()).unwrap(),
        f(-1, None)
    );
}

#[test]
fn req_rate_4_reads_bridge_element_form() {
    assert_eq!(
        read_owned(sidecars::bridge(5, Some("Green")).as_bytes()).unwrap(),
        f(5, Some("Green"))
    );
}

#[test]
fn req_rate_4_reads_digikam_sidecar_and_ignores_foreign_ratings() {
    assert_eq!(
        read_owned(sidecars::digikam(3, Some("Purple")).as_bytes()).unwrap(),
        f(3, Some("Purple"))
    );
}

#[test]
fn req_rate_4_namespace_matched_by_uri_not_prefix() {
    assert_eq!(
        read_owned(sidecars::xap_prefix(1).as_bytes()).unwrap(),
        f(1, None)
    );
}

#[test]
fn req_rate_4_embedded_xmp_in_jpeg_and_dng() {
    let packet = sidecars::bridge(2, Some("Blue"));
    // JPEG: SOI, APP1 with the XMP signature, EOI.
    let mut seg = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
    seg.extend_from_slice(packet.as_bytes());
    let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
    jpeg.extend(((seg.len() + 2) as u16).to_be_bytes());
    jpeg.extend(&seg);
    jpeg.extend([0xFF, 0xD9]);
    let p = jpeg_xmp_packet(&jpeg).unwrap();
    assert_eq!(read_owned(p).unwrap(), f(2, Some("Blue")));

    // DNG: IFD0 with tag 700 (XMP, type BYTE) pointing at the packet.
    let mut dng = b"II*\0".to_vec();
    dng.extend(8u32.to_le_bytes());
    dng.extend(1u16.to_le_bytes());
    dng.extend(0x02BCu16.to_le_bytes());
    dng.extend(1u16.to_le_bytes());
    dng.extend((packet.len() as u32).to_le_bytes());
    dng.extend(26u32.to_le_bytes());
    dng.extend(0u32.to_le_bytes());
    assert_eq!(dng.len(), 26);
    dng.extend(packet.as_bytes());
    assert_eq!(
        read_owned(tiff_xmp_packet(&dng).unwrap()).unwrap(),
        f(2, Some("Blue"))
    );
    assert!(jpeg_xmp_packet(b"not a jpeg").is_none());
    assert!(tiff_xmp_packet(b"II*\0\x08\0\0\0").is_none());
}

#[test]
fn req_rate_4_label_with_entities() {
    let x = sidecars::bridge(1, Some("Client &amp; Co"));
    assert_eq!(read_owned(x.as_bytes()).unwrap(), f(1, Some("Client & Co")));
}
