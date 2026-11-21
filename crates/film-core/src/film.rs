//! Reversal-film emulation: scene-linear working space -> slide densities ->
//! scanned, display-linear sRGB.
//!
//! Pipeline per pixel:
//! 1. Working space (linear ProPhoto D50) -> linear Rec.2020 -> layer
//!    exposures via the preset's sensitivity matrix.
//! 2. log10 exposure, interimage coupling.
//! 3. Each dye layer's *analytical* density from its own exposure. Analytical
//!    curves are derived from the measured *integral* (Status A) curves by
//!    removing dye cross-absorption, so a neutral exposure reproduces the
//!    datasheet curves exactly.
//! 4. Integral density = dye matrix x analytical densities, plus grain.
//! 5. Scanner: grey balance, transmittance, white = film base, black =
//!    `scan.black_density`.

use anyhow::{Context, Result, bail};
use rayon::prelude::*;
use serde::Deserialize;

use crate::color::{self, Mat3};
use crate::develop::LinearImage;

// ---------------------------------------------------------------- preset data

#[derive(Debug, Clone, Deserialize)]
pub struct FilmPreset {
    pub id: String,
    pub name: String,
    pub version: u32,
    pub curves: Curves,
    pub dyes: Dyes,
    pub exposure: Exposure,
    pub look: Look,
    pub scan: Scan,
    pub grain: Grain,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Curves {
    pub log_exposure_r: Vec<f64>,
    pub density_r: Vec<f64>,
    pub log_exposure_g: Vec<f64>,
    pub density_g: Vec<f64>,
    pub log_exposure_b: Vec<f64>,
    pub density_b: Vec<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Dyes {
    pub matrix: Mat3,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Exposure {
    pub grey_linear: f64,
    pub grey_density: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Look {
    pub sensitivity: Mat3,
    pub interimage: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Scan {
    pub black_density: f64,
    pub neutral_balance: f64,
    /// Scanner software gamma on top of the transmittance read (>1 opens shadows).
    #[serde(default = "one")]
    pub gamma: f64,
    /// Scanner colour balance multipliers on the linear scan (R, G, B).
    #[serde(default = "unity3")]
    pub white_balance: [f64; 3],
}

fn one() -> f64 {
    1.0
}

fn unity3() -> [f64; 3] {
    [1.0; 3]
}

#[derive(Debug, Clone, Deserialize)]
pub struct Grain {
    pub rms_granularity: f64,
    pub correlation: f64,
    pub blur_px: f64,
    pub amount: f64,
}

pub const KODACHROME_64: &str = include_str!("../../../presets/kodachrome64.toml");

impl FilmPreset {
    pub fn from_toml(text: &str) -> Result<Self> {
        let p: Self = toml::from_str(text).context("parsing film preset")?;
        for (x, y, ch) in [
            (&p.curves.log_exposure_r, &p.curves.density_r, "r"),
            (&p.curves.log_exposure_g, &p.curves.density_g, "g"),
            (&p.curves.log_exposure_b, &p.curves.density_b, "b"),
        ] {
            if x.len() != y.len() || x.len() < 2 {
                bail!("curve {ch}: log_exposure and density must have the same length (>= 2)");
            }
            if x.windows(2).any(|w| w[1] <= w[0]) {
                bail!("curve {ch}: log_exposure must be strictly increasing");
            }
        }
        color::inverse(&p.dyes.matrix).context("dye matrix is singular")?;
        Ok(p)
    }

    pub fn builtin(id: &str) -> Result<Self> {
        match id {
            "kodachrome64" => Self::from_toml(KODACHROME_64),
            _ => bail!("unknown preset '{id}' (available: kodachrome64)"),
        }
    }
}

// ------------------------------------------------------------ compiled model

/// Uniformly sampled curve for fast lookup, clamped at both ends.
#[derive(Debug, Clone)]
struct Lut {
    x0: f64,
    step: f64,
    y: Vec<f32>,
}

impl Lut {
    const N: usize = 2048;

    fn from_fn(x0: f64, x1: f64, f: impl Fn(f64) -> f64) -> Self {
        let step = (x1 - x0) / (Self::N - 1) as f64;
        let y = (0..Self::N).map(|i| f(x0 + i as f64 * step) as f32).collect();
        Self { x0, step, y }
    }

    fn eval(&self, x: f32) -> f32 {
        let t = ((x as f64 - self.x0) / self.step).clamp(0.0, (Self::N - 1) as f64);
        let i = (t as usize).min(Self::N - 2);
        let f = (t - i as f64) as f32;
        self.y[i] + (self.y[i + 1] - self.y[i]) * f
    }
}

/// Piecewise-linear interpolation through measured points, clamped.
fn interp(xs: &[f64], ys: &[f64], x: f64) -> f64 {
    if x <= xs[0] {
        return ys[0];
    }
    if x >= xs[xs.len() - 1] {
        return ys[ys.len() - 1];
    }
    let i = xs.partition_point(|&v| v <= x) - 1;
    let t = (x - xs[i]) / (xs[i + 1] - xs[i]);
    ys[i] + t * (ys[i + 1] - ys[i])
}

/// Per-render settings layered on top of a preset (the app's sliders).
#[derive(Debug, Clone)]
pub struct FilmParams {
    /// Extra exposure in stops (slide films are unforgiving: +/-0.5 matters).
    pub exposure_ev: f64,
    /// Multiplier on the preset's grain amount. 0 disables grain.
    pub grain: f64,
    /// Seed so grain is stable across re-renders of the same image.
    pub seed: u64,
    /// Size of one output pixel relative to a full-resolution M9 pixel
    /// (2.0 for half-size previews), so grain looks the same at any size.
    pub pixel_scale: f64,
}

impl Default for FilmParams {
    fn default() -> Self {
        Self { exposure_ev: 0.0, grain: 1.0, seed: 0, pixel_scale: 1.0 }
    }
}

/// M9 sensor pixel pitch on a 36 mm wide frame, in micrometres.
const M9_PIXEL_UM: f64 = 36_000.0 / 5212.0;
/// Aperture used for the datasheet granularity measurement.
const GRANULARITY_APERTURE_UM: f64 = 48.0;

pub struct FilmModel {
    to_layers: [[f32; 3]; 3],
    log_offset: f32,
    interimage: f32,
    analytical: [Lut; 3],
    dyes: [[f32; 3]; 3],
    /// Per channel (offset, scale) applied to integral density by the scanner.
    balance: [(f32, f32); 3],
    t_white: f32,
    t_black: f32,
    inv_gamma: f32,
    scan_wb: [f32; 3],
    grain_sigma: f32,
    grain_corr: f32,
    grain_blur: f32,
}

const LOG_E_MIN: f64 = -4.0;
const LOG_E_MAX: f64 = 2.0;

impl FilmModel {
    pub fn new(p: &FilmPreset, params: &FilmParams) -> Result<Self> {
        let c = &p.curves;
        let integral = |ch: usize, x: f64| match ch {
            0 => interp(&c.log_exposure_r, &c.density_r, x),
            1 => interp(&c.log_exposure_g, &c.density_g, x),
            _ => interp(&c.log_exposure_b, &c.density_b, x),
        };

        // Analytical dye curves: a(logE) = A^-1 * D_integral(logE) for neutrals.
        let a_inv = color::inverse(&p.dyes.matrix).context("dye matrix is singular")?;
        let analytical = [0, 1, 2].map(|dye| {
            Lut::from_fn(LOG_E_MIN, LOG_E_MAX, |x| {
                let d = [integral(0, x), integral(1, x), integral(2, x)];
                color::mul_vec(&a_inv, &d)[dye]
            })
        });

        // Exposure: grey_linear lands where green reaches grey_density.
        let ge = &c.log_exposure_g;
        let grey_log_e = find_log_e(|x| integral(1, x), p.exposure.grey_density, ge[0], ge[ge.len() - 1]);
        let log_offset = grey_log_e - p.exposure.grey_linear.log10() + params.exposure_ev * 2f64.log10();

        // Working (ProPhoto D50) -> linear Rec.2020 (D65) -> layer exposures.
        let to_rec2020 = color::mul(
            &color::XYZ_D65_TO_REC2020,
            &color::mul(
                &color::bradford(color::XY_D50, color::XY_D65),
                &color::inverse(&color::XYZ_D50_TO_PROPHOTO).expect("invertible"),
            ),
        );
        let to_layers = color::mul(&p.look.sensitivity, &to_rec2020);

        // Scanner: base (Dmin) is white; grey-balance channels at the grey density.
        let dmin = [0, 1, 2].map(|ch| integral(ch, LOG_E_MAX));
        let d_grey = [0, 1, 2].map(|ch| integral(ch, grey_log_e));
        let balance = [0, 1, 2].map(|ch| {
            let full = (d_grey[1] - dmin[1]) / (d_grey[ch] - dmin[ch]);
            let scale = 1.0 + p.scan.neutral_balance * (full - 1.0);
            // D' = dmin_g + (D - dmin_ch) * scale  ==  D * scale + offset
            ((dmin[1] - dmin[ch] * scale) as f32, scale as f32)
        });

        let px_um = M9_PIXEL_UM * params.pixel_scale.max(0.1);
        let grain_sigma = p.grain.rms_granularity / 1000.0 * (GRANULARITY_APERTURE_UM / px_um)
            * p.grain.amount
            * params.grain;

        Ok(Self {
            to_layers: to_layers.map(|r| r.map(|v| v as f32)),
            log_offset: log_offset as f32,
            interimage: p.look.interimage as f32,
            analytical,
            dyes: p.dyes.matrix.map(|r| r.map(|v| v as f32)),
            balance,
            t_white: 10f32.powf(-dmin[1] as f32),
            t_black: 10f32.powf(-p.scan.black_density as f32),
            inv_gamma: (1.0 / p.scan.gamma.max(0.1)) as f32,
            scan_wb: p.scan.white_balance.map(|v| v as f32),
            grain_sigma: grain_sigma as f32,
            grain_corr: p.grain.correlation.clamp(0.0, 1.0) as f32,
            grain_blur: (p.grain.blur_px / params.pixel_scale.max(0.1)) as f32,
        })
    }

    /// Integral (scanner-channel) densities for one working-space pixel.
    pub fn densities(&self, rgb: [f32; 3]) -> [f32; 3] {
        let m = &self.to_layers;
        let log_e = [0, 1, 2].map(|i| {
            let e = m[i][0] * rgb[0] + m[i][1] * rgb[1] + m[i][2] * rgb[2];
            e.max(1e-7).log10() + self.log_offset
        });
        let mean = (log_e[0] + log_e[1] + log_e[2]) / 3.0;
        let a = [0, 1, 2].map(|i| self.analytical[i].eval(log_e[i] + self.interimage * (log_e[i] - mean)));
        let d = &self.dyes;
        [0, 1, 2].map(|i| d[i][0] * a[0] + d[i][1] * a[1] + d[i][2] * a[2])
    }

    /// Scanner: integral density -> display-linear value in [0, 1].
    pub fn scan(&self, d: [f32; 3]) -> [f32; 3] {
        [0, 1, 2].map(|i| {
            let (off, scale) = self.balance[i];
            let t = 10f32.powf(-(d[i] * scale + off));
            let v = ((t - self.t_black) / (self.t_white - self.t_black)).clamp(0.0, 1.0);
            (v.powf(self.inv_gamma) * self.scan_wb[i]).clamp(0.0, 1.0)
        })
    }

    /// Run the full model over an image. Output pixels are display-linear sRGB.
    pub fn apply(&self, img: &LinearImage, seed: u64) -> LinearImage {
        let mut dens: Vec<[f32; 3]> = img.pixels.par_iter().map(|&p| self.densities(p)).collect();
        if self.grain_sigma > 0.0 {
            add_grain(&mut dens, img.width, img.height, self, seed);
        }
        let pixels = dens.par_iter().map(|&d| self.scan(d)).collect();
        LinearImage { width: img.width, height: img.height, pixels, cct: img.cct }
    }
}

/// Solve f(x) = target for a monotonically decreasing f on [lo, hi].
fn find_log_e(f: impl Fn(f64) -> f64, target: f64, mut lo: f64, mut hi: f64) -> f64 {
    for _ in 0..60 {
        let mid = 0.5 * (lo + hi);
        if f(mid) > target { lo = mid } else { hi = mid }
    }
    0.5 * (lo + hi)
}

// --------------------------------------------------------------------- grain

/// SplitMix64: tiny, fast, good-enough hash for per-pixel noise.
fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn gaussian(seed: u64, i: u64) -> f32 {
    let h1 = splitmix(seed ^ i.wrapping_mul(0xD6E8_FEB8_6659_FD93));
    let h2 = splitmix(h1);
    let u1 = ((h1 >> 11) as f64 + 0.5) / (1u64 << 53) as f64;
    let u2 = (h2 >> 11) as f64 / (1u64 << 53) as f64;
    ((-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()) as f32
}

/// Separable Gaussian blur of a single-channel field, returning the factor by
/// which it reduced the standard deviation of white noise.
fn blur(field: &mut [f32], w: usize, h: usize, sigma: f32) -> f32 {
    if sigma < 0.05 {
        return 1.0;
    }
    let r = (sigma * 3.0).ceil() as isize;
    let k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let sum: f32 = k.iter().sum();
    let k: Vec<f32> = k.iter().map(|v| v / sum).collect();
    let mut tmp = vec![0f32; field.len()];
    tmp.par_chunks_exact_mut(w).zip(field.par_chunks_exact(w)).for_each(|(out, row)| {
        for x in 0..w {
            out[x] = k.iter().enumerate().map(|(j, kv)| {
                let xx = (x as isize + j as isize - r).clamp(0, w as isize - 1) as usize;
                kv * row[xx]
            }).sum();
        }
    });
    field.par_chunks_exact_mut(w).enumerate().for_each(|(y, out)| {
        for x in 0..w {
            out[x] = k.iter().enumerate().map(|(j, kv)| {
                let yy = (y as isize + j as isize - r).clamp(0, h as isize - 1) as usize;
                kv * tmp[yy * w + x]
            }).sum();
        }
    });
    // Std of blurred white noise = sqrt(sum k^2) per axis.
    let s: f32 = k.iter().map(|v| v * v).sum::<f32>().sqrt();
    s * s
}

fn add_grain(dens: &mut [[f32; 3]], w: usize, h: usize, model: &FilmModel, seed: u64) {
    let n = w * h;
    let fields: Vec<Vec<f32>> = (0..4u64)
        .into_par_iter()
        .map(|c| {
            let s = splitmix(seed.wrapping_add(c));
            let mut f: Vec<f32> = (0..n as u64).into_par_iter().map(|i| gaussian(s, i)).collect();
            let gain = blur(&mut f, w, h, model.grain_blur);
            f.par_iter_mut().for_each(|v| *v /= gain);
            f
        })
        .collect();
    let shared = model.grain_corr.sqrt();
    let own = (1.0 - model.grain_corr).sqrt();
    let sigma = model.grain_sigma;
    dens.par_iter_mut().enumerate().for_each(|(i, d)| {
        for c in 0..3 {
            // Granularity is specified at density 1.0 and grows with density.
            let s = sigma * d[c].max(0.05).sqrt();
            d[c] += s * (shared * fields[3][i] + own * fields[c][i]);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(grain: f64) -> (FilmPreset, FilmModel) {
        let p = FilmPreset::builtin("kodachrome64").unwrap();
        let m = FilmModel::new(&p, &FilmParams { grain, ..Default::default() }).unwrap();
        (p, m)
    }

    #[test]
    fn preset_parses() {
        let (p, _) = model(1.0);
        assert_eq!(p.id, "kodachrome64");
    }

    #[test]
    fn neutral_exposure_reproduces_datasheet_curves() {
        let (p, m) = model(0.0);
        // Neutral input: the layer exposures are equal, so integral densities
        // must equal the measured curves at that log exposure.
        let grey = p.exposure.grey_linear as f32;
        let d = m.densities([grey, grey, grey]);
        assert!((d[1] - p.exposure.grey_density as f32).abs() < 0.03, "{d:?}");
    }

    #[test]
    fn grey_scans_neutral_and_brightness_is_monotonic() {
        let (mut p, _) = model(0.0);
        p.scan.white_balance = [1.0; 3]; // test the film/scanner balance itself
        let m = FilmModel::new(&p, &FilmParams { grain: 0.0, ..Default::default() }).unwrap();
        let grey = p.exposure.grey_linear as f32;
        let out = m.scan(m.densities([grey, grey, grey]));
        let spread = out.iter().cloned().fold(f32::MIN, f32::max) - out.iter().cloned().fold(f32::MAX, f32::min);
        assert!(spread < 0.01, "grey not neutral: {out:?}");

        let mut last = -1.0;
        for i in 0..40 {
            let v = 0.001 * 1.25f32.powi(i);
            let o = m.scan(m.densities([v, v, v]));
            assert!(o[1] >= last, "not monotonic at {v}");
            last = o[1];
        }
        assert!(last > 0.95, "highlights should reach near white, got {last}");
    }

    /// Diagnostic: `cargo test -p film-core print_ramps -- --nocapture --ignored`
    #[test]
    #[ignore]
    fn print_ramps() {
        let (p, m) = model(0.0);
        let g = p.exposure.grey_linear as f32;
        println!("neutral ramp (stops vs grey -> sRGB 0-255):");
        for s in -6..=4 {
            let v = g * 2f32.powi(s);
            let o = m.scan(m.densities([v, v, v]));
            let e = o.map(|c| (crate::color::srgb_encode(c) * 255.0).round() as i32);
            println!("  {s:+} stops: {e:?}");
        }
        // Linear ProPhoto values of a few typical scene colours at grey level.
        for (name, c) in [
            ("blue sky", [0.06, 0.10, 0.30]),
            ("foliage", [0.06, 0.10, 0.04]),
            ("skin", [0.30, 0.20, 0.15]),
            ("red", [0.25, 0.06, 0.04]),
            ("yellow", [0.45, 0.40, 0.05]),
        ] {
            let o = m.scan(m.densities(c));
            println!("  {name}: {:?}", o.map(|c| (crate::color::srgb_encode(c) * 255.0).round() as i32));
        }
    }

    #[test]
    fn grain_is_deterministic() {
        let (_, m) = model(1.0);
        let img = LinearImage { width: 16, height: 16, pixels: vec![[0.1, 0.1, 0.1]; 256], cct: 5000.0 };
        assert_eq!(m.apply(&img, 7).pixels, m.apply(&img, 7).pixels);
        assert_ne!(m.apply(&img, 7).pixels, m.apply(&img, 8).pixels);
    }
}
