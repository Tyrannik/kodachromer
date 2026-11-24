//! A compact sRGB ICC profile, built in code, for embedding in exports.
//!
//! Exports are already encoded as sRGB, so tagging them changes no pixels; it
//! just tells colour-managed viewers (and wide-gamut displays) what the numbers
//! mean instead of leaving them to guess. The profile is ICC v2.1 (the most
//! widely understood version) with the IEC 61966-2.1 primaries, a D50 media
//! white with a Bradford `chad` tag, and the piecewise sRGB curve sampled at
//! 1024 points and shared by all three channels. Built from `color.rs` rather
//! than shipped as a binary so there is no third-party file to license.

use std::sync::OnceLock;

use crate::color::{self, Mat3};

/// ICC profile connection space illuminant (D50), as the spec rounds it.
const PCS_D50: [f64; 3] = [0.9642, 1.0, 0.8249];

/// Number of samples in the tone curve; same as the IEC reference profile.
const CURVE_POINTS: usize = 1024;

const DESCRIPTION: &str = "sRGB IEC61966-2.1 (Remjet)";
const COPYRIGHT: &str = "No copyright, use freely";

/// The profile bytes, built once.
pub fn srgb_profile() -> &'static [u8] {
    static PROFILE: OnceLock<Vec<u8>> = OnceLock::new();
    PROFILE.get_or_init(build_srgb_profile)
}

/// sRGB electro-optical transfer function (encoded -> linear).
fn srgb_decode(v: f64) -> f64 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

/// Encode as s15Fixed16Number.
fn s15f16(v: f64) -> [u8; 4] {
    ((v * 65536.0).round() as i32).to_be_bytes()
}

fn xyz_type(xyz: [f64; 3]) -> Vec<u8> {
    let mut t = b"XYZ \0\0\0\0".to_vec();
    for c in xyz {
        t.extend(s15f16(c));
    }
    t
}

/// sRGB -> XYZ, chromatically adapted to D50: the columns are rXYZ, gXYZ, bXYZ.
fn srgb_to_xyz_d50() -> Mat3 {
    let srgb_to_xyz_d65 = color::inverse(&color::XYZ_D65_TO_SRGB).expect("invertible");
    color::mul(&chad(), &srgb_to_xyz_d65)
}

/// Bradford D65 -> D50, the adaptation the colorants went through.
fn chad() -> Mat3 {
    color::bradford(color::XY_D65, color::XY_D50)
}

fn build_srgb_profile() -> Vec<u8> {
    let m = srgb_to_xyz_d50();
    let column = |j: usize| [m[0][j], m[1][j], m[2][j]];

    // textDescriptionType: ASCII part, then empty Unicode and ScriptCode parts.
    let mut desc = b"desc\0\0\0\0".to_vec();
    desc.extend((DESCRIPTION.len() as u32 + 1).to_be_bytes());
    desc.extend(DESCRIPTION.as_bytes());
    desc.push(0);
    desc.extend([0u8; 4 + 4 + 2 + 1 + 67]);

    let mut cprt = b"text\0\0\0\0".to_vec();
    cprt.extend(COPYRIGHT.as_bytes());
    cprt.push(0);

    let mut curv = b"curv\0\0\0\0".to_vec();
    curv.extend((CURVE_POINTS as u32).to_be_bytes());
    for i in 0..CURVE_POINTS {
        let y = srgb_decode(i as f64 / (CURVE_POINTS - 1) as f64);
        curv.extend(((y * 65535.0).round() as u16).to_be_bytes());
    }

    let mut sf32 = b"sf32\0\0\0\0".to_vec();
    for row in chad() {
        for v in row {
            sf32.extend(s15f16(v));
        }
    }

    // (signature, data index); the three TRC tags share one curve.
    let data = [desc, cprt, xyz_type(PCS_D50), sf32, xyz_type(column(0)), xyz_type(column(1)), xyz_type(column(2)), curv];
    let tags: [(&[u8; 4], usize); 10] =
        [(b"desc", 0), (b"cprt", 1), (b"wtpt", 2), (b"chad", 3), (b"rXYZ", 4), (b"gXYZ", 5), (b"bXYZ", 6), (b"rTRC", 7), (b"gTRC", 7), (b"bTRC", 7)];

    // Lay out tag data after the header and tag table, 4-byte aligned.
    let mut offset = 128 + 4 + 12 * tags.len();
    let mut placed = Vec::with_capacity(data.len());
    for d in &data {
        placed.push(offset);
        offset = (offset + d.len()).next_multiple_of(4);
    }
    let total = offset;

    let mut p = Vec::with_capacity(total);
    p.extend((total as u32).to_be_bytes());
    p.extend([0u8; 4]); // preferred CMM: none
    p.extend([0x02, 0x10, 0x00, 0x00]); // version 2.1
    p.extend(b"mntrRGB XYZ ");
    // Creation date: fixed so the profile (and exports) are reproducible.
    for v in [2026u16, 1, 1, 0, 0, 0] {
        p.extend(v.to_be_bytes());
    }
    p.extend(b"acsp");
    p.extend([0u8; 4 + 4 + 4 + 4 + 8]); // platform, flags, manufacturer, model, attributes
    p.extend(0u32.to_be_bytes()); // rendering intent: perceptual
    for c in PCS_D50 {
        p.extend(s15f16(c));
    }
    p.extend([0u8; 4 + 16 + 28]); // creator, profile ID, reserved
    debug_assert_eq!(p.len(), 128);

    p.extend((tags.len() as u32).to_be_bytes());
    for (sig, i) in tags {
        p.extend(sig);
        p.extend((placed[i] as u32).to_be_bytes());
        p.extend((data[i].len() as u32).to_be_bytes());
    }
    for (d, at) in data.iter().zip(&placed) {
        p.resize(*at, 0);
        p.extend(d);
    }
    p.resize(total, 0);
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u32_at(b: &[u8], o: usize) -> u32 {
        u32::from_be_bytes(b[o..o + 4].try_into().unwrap())
    }

    /// Look up a tag's data in the tag table.
    fn tag<'a>(p: &'a [u8], sig: &[u8; 4]) -> &'a [u8] {
        let n = u32_at(p, 128) as usize;
        let e = (0..n).map(|i| 132 + 12 * i).find(|&e| &p[e..e + 4] == sig).expect("tag present");
        let (off, len) = (u32_at(p, e + 4) as usize, u32_at(p, e + 8) as usize);
        &p[off..off + len]
    }

    fn xyz(t: &[u8]) -> [f64; 3] {
        assert_eq!(&t[..4], b"XYZ ");
        [0, 1, 2].map(|i| i32::from_be_bytes(t[8 + 4 * i..12 + 4 * i].try_into().unwrap()) as f64 / 65536.0)
    }

    #[test]
    fn header_is_well_formed() {
        let p = srgb_profile();
        assert_eq!(u32_at(p, 0) as usize, p.len());
        assert_eq!(p.len() % 4, 0);
        assert_eq!(&p[8..12], &[0x02, 0x10, 0, 0]);
        assert_eq!(&p[12..24], b"mntrRGB XYZ ");
        assert_eq!(&p[36..40], b"acsp");
        // Small enough for a single JPEG APP2 segment.
        assert!(p.len() < 65_000, "{}", p.len());
    }

    #[test]
    fn tags_are_in_bounds_and_aligned() {
        let p = srgb_profile();
        let n = u32_at(p, 128) as usize;
        for i in 0..n {
            let e = 132 + 12 * i;
            let (off, len) = (u32_at(p, e + 4) as usize, u32_at(p, e + 8) as usize);
            assert_eq!(off % 4, 0);
            assert!(off >= 132 + 12 * n && off + len <= p.len());
        }
    }

    #[test]
    fn colorants_match_iec_reference() {
        // rXYZ/gXYZ/bXYZ of the IEC 61966-2.1 reference sRGB profile.
        let p = srgb_profile();
        let reference = [
            (b"rXYZ", [0.4360747, 0.2225045, 0.0139322]),
            (b"gXYZ", [0.3850649, 0.7168786, 0.0971045]),
            (b"bXYZ", [0.1430804, 0.0606169, 0.7141733]),
        ];
        for (sig, want) in reference {
            let got = xyz(tag(p, sig));
            assert!(got.iter().zip(want).all(|(g, w)| (g - w).abs() < 2e-4), "{sig:?}: {got:?}");
        }
        // White point is D50, and the colorants sum to (near) it.
        let w = xyz(tag(p, b"wtpt"));
        assert!(w.iter().zip(PCS_D50).all(|(g, x)| (g - x).abs() < 1e-4));
    }

    #[test]
    fn curve_is_the_srgb_eotf() {
        let c = tag(srgb_profile(), b"rTRC");
        assert_eq!(&c[..4], b"curv");
        assert_eq!(u32_at(c, 8) as usize, CURVE_POINTS);
        let at = |i: usize| u16::from_be_bytes([c[12 + 2 * i], c[13 + 2 * i]]);
        assert_eq!(at(0), 0);
        assert_eq!(at(CURVE_POINTS - 1), 65535);
        // Encoded 0.5 is ~21.4% linear.
        let mid = at(CURVE_POINTS / 2) as f64 / 65535.0;
        assert!((mid - srgb_decode((CURVE_POINTS / 2) as f64 / 1023.0)).abs() < 1e-4);
        assert!((0.20..0.23).contains(&mid));
        // All channels share the same curve.
        assert_eq!(tag(srgb_profile(), b"gTRC"), c);
        assert_eq!(tag(srgb_profile(), b"bTRC"), c);
    }

    #[test]
    fn colour_engine_treats_it_as_srgb() {
        // An independent CMS must parse the profile and map it onto its own
        // sRGB as (almost exactly) the identity.
        use moxcms::{ColorProfile, Layout, TransformOptions};
        let ours = ColorProfile::new_from_slice(srgb_profile()).expect("moxcms parses the profile");
        let t = ours
            .create_transform_8bit(Layout::Rgb, &ColorProfile::new_srgb(), Layout::Rgb, TransformOptions::default())
            .expect("transform");
        let mut src = Vec::new();
        for r in (0..=255u16).step_by(17) {
            for g in (0..=255u16).step_by(51) {
                for b in (0..=255u16).step_by(85) {
                    src.extend([r as u8, g as u8, b as u8]);
                }
            }
        }
        let mut dst = vec![0u8; src.len()];
        t.transform(&src, &mut dst).unwrap();
        let worst = src.iter().zip(&dst).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
        assert!(worst <= 1, "max channel difference {worst}");
    }
}
