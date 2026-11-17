//! Fast-tier tests on synthetic fixtures (SPEC §11.3).

use blink_core::formats::FileKind;
use blink_core::preview::PreviewSource;
use blink_decode::color::SourceSpace;
use blink_decode::{analyze, decode_embedded, decode_jpeg_file, default_backend, image};
use blink_testkit::fixtures::*;

fn close(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 24)
}

#[test]
fn req_view_8_all_eight_orientations_display_upright() {
    for o in 1..=8u16 {
        let p = decode_jpeg_file(&orientation_jpeg(o), PreviewSource::Jpeg).unwrap();
        assert_eq!(p.orientation, o);
        assert_eq!(
            (p.image.width, p.image.height),
            (ORIENT_W, ORIENT_H),
            "o={o}"
        );
        let at = |x, y| p.image.pixel(x, y);
        let (qx, qy) = (ORIENT_W / 4, ORIENT_H / 4);
        for (pt, want) in [
            ((qx, qy), RED),
            ((3 * qx, qy), GREEN),
            ((qx, 3 * qy), BLUE),
            ((3 * qx, 3 * qy), WHITE),
        ] {
            let got = at(pt.0, pt.1);
            assert!(close(got, want), "o={o} at {pt:?}: {got:?} != {want:?}");
        }
    }
}

#[test]
fn req_view_8_orientation_of_raw_container_applies_to_embedded_preview() {
    // Container IFD0 says orientation 6; the embedded preview is stored unrotated.
    let stored = jpeg(
        &quadrants(32, 64, [GREEN, WHITE, RED, BLUE]),
        32,
        64,
        95,
        None,
    );
    let mut file = exif_tiff(Some(6), None, None);
    file.extend([0u8; 16]);
    file.extend(&stored);
    let a = analyze(&file, FileKind::Raw, &default_backend());
    let p = decode_embedded(&file, &a).unwrap();
    assert_eq!((p.image.width, p.image.height), (64, 32));
    assert!(close(p.image.pixel(16, 8), RED));
    assert!(close(p.image.pixel(48, 24), WHITE));
}

#[test]
fn req_view_8_display_only_input_bytes_unchanged() {
    let f = orientation_jpeg(6);
    let copy = f.clone();
    let _ = decode_jpeg_file(&f, PreviewSource::Jpeg).unwrap();
    assert_eq!(f, copy);
}

#[test]
fn req_view_2_largest_of_several_embedded_previews_is_chosen() {
    let small = jpeg(
        &quadrants(160, 120, [RED, RED, RED, RED]),
        160,
        120,
        80,
        None,
    );
    let big = jpeg(
        &quadrants(640, 424, [BLUE, BLUE, BLUE, BLUE]),
        640,
        424,
        80,
        None,
    );
    let mid = jpeg(&quadrants(320, 240, [GREEN; 4]), 320, 240, 80, None);
    let file = tiff_with_jpegs(&[small, big, mid]);
    let a = analyze(&file, FileKind::Raw, &default_backend());
    assert_eq!(a.embedded.len(), 3, "{:?}", a.embedded);
    let c = a.chosen.unwrap();
    assert_eq!((c.width, c.height), (640, 424));
    for e in &a.embedded {
        assert!(c.width >= e.width && c.height >= e.height);
    }
    let p = decode_embedded(&file, &a).unwrap();
    assert!(close(p.image.pixel(10, 10), BLUE));
}

// --- VIEW-10: Adobe RGB → sRGB, checked against an independent reference ---

fn adobe_to_srgb_reference(p: [u8; 3]) -> [f64; 3] {
    let lin = |v: u8| (f64::from(v) / 255.0).powf(563.0 / 256.0);
    let (r, g, b) = (lin(p[0]), lin(p[1]), lin(p[2]));
    let x = 0.576_730_9 * r + 0.185_554_0 * g + 0.188_185_2 * b;
    let y = 0.297_376_9 * r + 0.627_349_1 * g + 0.075_274_1 * b;
    let z = 0.027_034_3 * r + 0.070_687_2 * g + 0.991_108_5 * b;
    let sr = 3.240_454_2 * x - 1.537_138_5 * y - 0.498_531_4 * z;
    let sg = -0.969_266_0 * x + 1.876_010_8 * y + 0.041_556_0 * z;
    let sb = 0.055_643_4 * x - 0.204_025_9 * y + 1.057_225_2 * z;
    let enc = |v: f64| {
        let v = v.clamp(0.0, 1.0);
        let e = if v <= 0.003_130_8 {
            12.92 * v
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        };
        e * 255.0
    };
    [enc(sr), enc(sg), enc(sb)]
}

fn srgb_to_lab(p: [f64; 3]) -> [f64; 3] {
    let lin = |v: f64| {
        let v = v / 255.0;
        if v <= 0.040_45 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (lin(p[0]), lin(p[1]), lin(p[2]));
    let x = (0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b) / 0.950_47;
    let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175_0 * b;
    let z = (0.019_333_9 * r + 0.119_192_0 * g + 0.950_304_1 * b) / 1.088_83;
    let f = |t: f64| {
        if t > 216.0 / 24389.0 {
            t.cbrt()
        } else {
            (24389.0 / 27.0 * t + 16.0) / 116.0
        }
    };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// CIEDE2000 (Sharma, Wu, Dalal 2005).
fn delta_e_2000(a: [f64; 3], b: [f64; 3]) -> f64 {
    use std::f64::consts::PI;
    let (l1, a1, b1) = (a[0], a[1], a[2]);
    let (l2, a2, b2) = (b[0], b[1], b[2]);
    let c1 = a1.hypot(b1);
    let c2 = a2.hypot(b2);
    let cm = (c1 + c2) / 2.0;
    let g = 0.5 * (1.0 - (cm.powi(7) / (cm.powi(7) + 25f64.powi(7))).sqrt());
    let (a1p, a2p) = ((1.0 + g) * a1, (1.0 + g) * a2);
    let (c1p, c2p) = (a1p.hypot(b1), a2p.hypot(b2));
    let hp = |b: f64, a: f64| {
        if b == 0.0 && a == 0.0 {
            0.0
        } else {
            let h = b.atan2(a).to_degrees();
            if h < 0.0 { h + 360.0 } else { h }
        }
    };
    let (h1p, h2p) = (hp(b1, a1p), hp(b2, a2p));
    let dl = l2 - l1;
    let dc = c2p - c1p;
    let dh = if c1p * c2p == 0.0 {
        0.0
    } else if (h2p - h1p).abs() <= 180.0 {
        h2p - h1p
    } else if h2p - h1p > 180.0 {
        h2p - h1p - 360.0
    } else {
        h2p - h1p + 360.0
    };
    let dhh = 2.0 * (c1p * c2p).sqrt() * (dh.to_radians() / 2.0).sin();
    let lm = (l1 + l2) / 2.0;
    let cmp = (c1p + c2p) / 2.0;
    let hm = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        (h1p + h2p) / 2.0
    } else if h1p + h2p < 360.0 {
        (h1p + h2p + 360.0) / 2.0
    } else {
        (h1p + h2p - 360.0) / 2.0
    };
    let t = 1.0 - 0.17 * (hm - 30.0).to_radians().cos()
        + 0.24 * (2.0 * hm).to_radians().cos()
        + 0.32 * (3.0 * hm + 6.0).to_radians().cos()
        - 0.20 * (4.0 * hm - 63.0).to_radians().cos();
    let dtheta = 30.0 * (-((hm - 275.0) / 25.0).powi(2)).exp();
    let rc = 2.0 * (cmp.powi(7) / (cmp.powi(7) + 25f64.powi(7))).sqrt();
    let sl = 1.0 + 0.015 * (lm - 50.0).powi(2) / (20.0 + (lm - 50.0).powi(2)).sqrt();
    let sc = 1.0 + 0.045 * cmp;
    let sh = 1.0 + 0.015 * cmp * t;
    let rt = -(2.0 * dtheta * PI / 180.0).sin() * rc;
    ((dl / sl).powi(2) + (dc / sc).powi(2) + (dhh / sh).powi(2) + rt * (dc / sc) * (dhh / sh))
        .sqrt()
}

#[test]
fn delta_e_2000_matches_published_pair() {
    // Sharma et al. test data, pair 1: ΔE00 = 2.0425.
    let d = delta_e_2000([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485]);
    assert!((d - 2.0425).abs() < 1e-3, "{d}");
}

#[test]
fn req_view_10_adobe_rgb_preview_converts_within_delta_e_2() {
    let file = adobe_rgb_jpeg();
    let raw = image::decode_jpeg(&file).unwrap().image; // un-converted pixels
    let p = decode_jpeg_file(&file, PreviewSource::Jpeg).unwrap();
    assert_eq!(p.space, SourceSpace::AdobeRgb);
    let mut worst = 0f64;
    for y in 0..raw.height {
        for x in 0..raw.width {
            let reference = srgb_to_lab(adobe_to_srgb_reference(raw.pixel(x, y)));
            let got = p.image.pixel(x, y).map(f64::from);
            worst = worst.max(delta_e_2000(reference, srgb_to_lab(got)));
        }
    }
    assert!(worst < 2.0, "worst ΔE2000 = {worst}");
    // The conversion did something: a saturated patch moved.
    assert_ne!(raw.pixel(8, 8), p.image.pixel(8, 8));
}

#[test]
fn req_view_10_srgb_tagged_preview_is_not_converted() {
    let tiff = exif_tiff(Some(1), Some(1), Some("R98"));
    let px = quadrants(32, 32, [[200, 60, 50]; 4]);
    let file = jpeg(&px, 32, 32, 95, Some(&tiff));
    let raw = image::decode_jpeg(&file).unwrap().image;
    let p = decode_jpeg_file(&file, PreviewSource::Jpeg).unwrap();
    assert_eq!(p.space, SourceSpace::Srgb);
    assert_eq!(p.image, raw);
}
