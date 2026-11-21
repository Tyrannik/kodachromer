//! Colour science helpers: 3x3 matrices, chromatic adaptation and the DNG
//! camera-to-XYZ solve.
//!
//! The internal working space is **linear ProPhoto RGB (ROMM, D50)**: wide
//! enough that saturated camera colours survive until the film model, and it
//! shares the D50 white of the DNG profile connection space.

pub type Mat3 = [[f64; 3]; 3];
pub type Vec3 = [f64; 3];

pub const IDENTITY: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// CIE xy chromaticities of the standard whites.
pub const XY_D50: [f64; 2] = [0.34567, 0.35850];
pub const XY_D65: [f64; 2] = [0.31271, 0.32902];

/// CCTs of the two DNG calibration illuminants the M9 profile uses.
pub const CCT_A: f64 = 2856.0;
pub const CCT_D65: f64 = 6504.0;

const BRADFORD: Mat3 = [
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
];

/// XYZ (D50) -> linear ProPhoto RGB.
pub const XYZ_D50_TO_PROPHOTO: Mat3 = [
    [1.3459433, -0.2556075, -0.0511118],
    [-0.5445989, 1.5081673, 0.0205351],
    [0.0000000, 0.0000000, 1.2118128],
];

/// XYZ (D65) -> linear sRGB / Rec.709.
pub const XYZ_D65_TO_SRGB: Mat3 = [
    [3.2404542, -1.5371385, -0.4985314],
    [-0.9692660, 1.8760108, 0.0415560],
    [0.0556434, -0.2040259, 1.0572252],
];

/// XYZ (D65) -> linear Rec.2020.
pub const XYZ_D65_TO_REC2020: Mat3 = [
    [1.7166512, -0.3556708, -0.2533663],
    [-0.6666844, 1.6164812, 0.0157685],
    [0.0176399, -0.0427706, 0.9421031],
];

pub fn mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut out = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

pub fn mul_vec(a: &Mat3, v: &Vec3) -> Vec3 {
    [0, 1, 2].map(|i| a[i][0] * v[0] + a[i][1] * v[1] + a[i][2] * v[2])
}

pub fn diag(v: &Vec3) -> Mat3 {
    [[v[0], 0.0, 0.0], [0.0, v[1], 0.0], [0.0, 0.0, v[2]]]
}

pub fn inverse(m: &Mat3) -> Option<Mat3> {
    let c = |r1: usize, c1: usize, r2: usize, c2: usize| m[r1][c1] * m[r2][c2] - m[r1][c2] * m[r2][c1];
    let det = m[0][0] * c(1, 1, 2, 2) - m[0][1] * c(1, 0, 2, 2) + m[0][2] * c(1, 0, 2, 1);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = [
        [c(1, 1, 2, 2), -c(0, 1, 2, 2), c(0, 1, 1, 2)],
        [-c(1, 0, 2, 2), c(0, 0, 2, 2), -c(0, 0, 1, 2)],
        [c(1, 0, 2, 1), -c(0, 0, 2, 1), c(0, 0, 1, 1)],
    ];
    Some(inv.map(|row| row.map(|v| v / det)))
}

pub fn xy_to_xyz(xy: [f64; 2]) -> Vec3 {
    [xy[0] / xy[1], 1.0, (1.0 - xy[0] - xy[1]) / xy[1]]
}

pub fn xyz_to_xy(xyz: &Vec3) -> [f64; 2] {
    let s = xyz[0] + xyz[1] + xyz[2];
    [xyz[0] / s, xyz[1] / s]
}

/// Bradford chromatic adaptation matrix from one white to another (XYZ -> XYZ).
pub fn bradford(src: [f64; 2], dst: [f64; 2]) -> Mat3 {
    let s = mul_vec(&BRADFORD, &xy_to_xyz(src));
    let d = mul_vec(&BRADFORD, &xy_to_xyz(dst));
    let scale = diag(&[d[0] / s[0], d[1] / s[1], d[2] / s[2]]);
    mul(&inverse(&BRADFORD).expect("Bradford is invertible"), &mul(&scale, &BRADFORD))
}

/// Correlated colour temperature from xy (McCamy's approximation; accurate to
/// a few kelvin between 2800 K and 7000 K, which covers the profile range).
pub fn cct_from_xy(xy: [f64; 2]) -> f64 {
    let n = (xy[0] - 0.3320) / (0.1858 - xy[1]);
    449.0 * n.powi(3) + 3525.0 * n.powi(2) + 6823.3 * n + 5520.33
}

/// Build a camera -> working space (linear ProPhoto D50) matrix following the
/// DNG specification ("Mapping Camera Color Space to CIE XYZ Space"), for
/// camera data that has **already been white balanced** by `wb` multipliers.
///
/// `cm_a` / `cm_d65` are the DNG ColorMatrix1/2 (XYZ -> camera) for
/// illuminants A and D65. The two are interpolated by inverse CCT of the
/// white point, which is solved iteratively. A white-balanced neutral of
/// (1, 1, 1) maps to working-space (1, 1, 1).
pub fn camera_to_working(cm_a: &Mat3, cm_d65: &Mat3, wb: &Vec3) -> Option<(Mat3, f64)> {
    // Camera-native neutral: the raw values a white surface produces.
    let neutral = [1.0 / wb[0], 1.0 / wb[1], 1.0 / wb[2]];

    let mut xy = XY_D50;
    let mut cm = *cm_d65;
    let mut cct = CCT_D65;
    for _ in 0..10 {
        cct = cct_from_xy(xy).clamp(1500.0, 50000.0);
        let g = ((1.0 / cct - 1.0 / CCT_D65) / (1.0 / CCT_A - 1.0 / CCT_D65)).clamp(0.0, 1.0);
        cm = [0, 1, 2].map(|i| [0, 1, 2].map(|j| g * cm_a[i][j] + (1.0 - g) * cm_d65[i][j]));
        let next = xyz_to_xy(&mul_vec(&inverse(&cm)?, &neutral));
        let done = (next[0] - xy[0]).abs() + (next[1] - xy[1]).abs() < 1e-7;
        xy = next;
        if done {
            break;
        }
    }

    // raw = neutral ⊙ balanced, so camera_balanced -> XYZ(white) -> XYZ(D50).
    let cam_to_xyz = mul(&inverse(&cm)?, &diag(&neutral));
    let adapted = mul(&bradford(xy, XY_D50), &cam_to_xyz);
    let to_working = mul(&XYZ_D50_TO_PROPHOTO, &adapted);
    // Normalise so a balanced neutral of 1.0 is working-space white (1, 1, 1).
    let white = mul_vec(&to_working, &[1.0, 1.0, 1.0]);
    let y = white[1];
    Some((to_working.map(|row| row.map(|v| v / y)), cct))
}

/// Linear ProPhoto (D50) -> linear sRGB (D65).
pub fn working_to_srgb() -> Mat3 {
    let prophoto_to_xyz = inverse(&XYZ_D50_TO_PROPHOTO).expect("invertible");
    mul(&XYZ_D65_TO_SRGB, &mul(&bradford(XY_D50, XY_D65), &prophoto_to_xyz))
}

/// sRGB opto-electronic transfer function (linear -> encoded).
pub fn srgb_encode(v: f32) -> f32 {
    if v <= 0.0031308 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: &Mat3, b: &Mat3, tol: f64) -> bool {
        (0..3).all(|i| (0..3).all(|j| (a[i][j] - b[i][j]).abs() < tol))
    }

    #[test]
    fn inverse_round_trips() {
        let inv = inverse(&XYZ_D65_TO_SRGB).unwrap();
        assert!(close(&mul(&inv, &XYZ_D65_TO_SRGB), &IDENTITY, 1e-9));
    }

    #[test]
    fn bradford_d50_to_d65_matches_reference() {
        // Lindbloom's published Bradford D50 -> D65 matrix.
        let reference = [
            [0.9555766, -0.0230393, 0.0631636],
            [-0.0282895, 1.0099416, 0.0210077],
            [0.0122982, -0.0204830, 1.3299098],
        ];
        assert!(close(&bradford(XY_D50, XY_D65), &reference, 2e-4));
    }

    #[test]
    fn cct_of_standard_whites() {
        assert!((cct_from_xy(XY_D65) - 6504.0).abs() < 30.0);
        assert!((cct_from_xy([0.44757, 0.40745]) - 2856.0).abs() < 30.0);
    }

    #[test]
    fn working_white_maps_to_srgb_white() {
        let w = mul_vec(&working_to_srgb(), &[1.0, 1.0, 1.0]);
        assert!(w.iter().all(|c| (c - 1.0).abs() < 1e-3), "{w:?}");
    }

    #[test]
    fn balanced_neutral_maps_to_white() {
        // Leica M9 ColorMatrix1 (A) and ColorMatrix2 (D65), from Adobe DNG data.
        let cm_a = [[1.1219, -0.7055, 0.1129], [-0.1907, 1.0399, 0.1709], [0.0119, 0.0602, 0.5932]];
        let cm_d65 = [[0.6812, -0.2051, -0.0573], [-0.4285, 1.2271, 0.2138], [-0.0699, 0.1714, 0.6574]];
        for wb in [[2.08, 1.0, 1.18], [1.55, 1.0, 1.96]] {
            let (m, cct) = camera_to_working(&cm_a, &cm_d65, &wb).unwrap();
            let w = mul_vec(&m, &[1.0, 1.0, 1.0]);
            // Tolerance covers the 7-decimal rounding of the published ProPhoto matrix.
            assert!(w.iter().all(|c| (c - 1.0).abs() < 1e-4), "wb {wb:?} -> {w:?}");
            assert!((1500.0..50000.0).contains(&cct));
        }
    }
}
