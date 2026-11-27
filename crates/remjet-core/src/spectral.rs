//! Spectral film model, built directly from a Kodak datasheet's published
//! measurements instead of hand-tuned matrices.
//!
//! What is derived from what:
//! - **Layer exposure.** The datasheet's spectral-sensitivity curves, lit by
//!   CIE daylight (D55), are integrated against a set of synthetic scene
//!   reflectances. A 3x3 matrix from the working space (ProPhoto D50) to the
//!   three layers' relative exposures is fitted to those by weighted least
//!   squares, with a white surface giving equal exposure in every layer.
//! - **Dye amounts.** For a reversal film, the Status A characteristic
//!   curves are inverted through the datasheet's spectral dye densities: at
//!   each log exposure, the dye amounts whose combined spectrum reads as the
//!   published Status A densities are solved for (ISO 5-3 densitometer
//!   responses). A neutral exposure therefore reproduces the curves exactly.
//! - **Colour negative + print.** The negative's Status M curves are
//!   converted to *printing* densities with gains computed from the
//!   datasheet's mid-scale neutral and minimum density spectra, weighed by the
//!   print paper's spectral sensitivity. Printing densities expose the paper
//!   through its own characteristic curves and dyes, with printer lights
//!   solved so the reference grey prints neutral.
//! - **Black-and-white negative.** One panchromatic sensitivity and one
//!   diffuse visual density curve, shared by all three channels so the
//!   image stays exactly neutral; the negative is viewed through the same
//!   lab scanner as the colour negatives (there is no paper data).
//! - **Viewing.** Film transmittance (or print reflectance) spectra are lit
//!   by 5000 K daylight (D50, the datasheets' viewing condition) and
//!   integrated with the CIE 1931 observer. White is the film base or paper.
//! - **Sharpness.** Each layer's modulation-transfer curve is fitted with a
//!   Gaussian spread minus a wider adjacency term (which gives the >100%
//!   response that development adjacency effects produce), in micrometres
//!   on the film.

use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::cie::{self, StatusBand};
use crate::color::{self, Mat3, Vec3};

/// Wavelength grid: 380..=730 nm in 5 nm steps.
pub const WL0: f64 = 380.0;
pub const WL_STEP: f64 = 5.0;
pub const NW: usize = 71;

pub type Spectrum = [f64; NW];

fn wavelength(i: usize) -> f64 {
    WL0 + WL_STEP * i as f64
}

/// Taking illuminant: daylight-balanced films are rated in CIE D55.
const TAKING_CCT: f64 = 5500.0;
/// Viewing illuminant: the datasheets' 5000 K (D50).
const VIEWING_CCT: f64 = 5000.0;

// ---------------------------------------------------------------- preset data

/// Uniformly sampled datasheet curve: `values[i]` at `start + i * step`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Series {
    pub start: f64,
    pub step: f64,
    pub values: Vec<f64>,
}

impl Series {
    fn end(&self) -> f64 {
        self.start + self.step * (self.values.len() - 1) as f64
    }

    /// Linear interpolation, holding the end values outside the range.
    pub fn at(&self, x: f64) -> f64 {
        let t = ((x - self.start) / self.step).clamp(0.0, (self.values.len() - 1) as f64);
        let i = (t as usize).min(self.values.len() - 2);
        let f = t - i as f64;
        self.values[i] + (self.values[i + 1] - self.values[i]) * f
    }

    /// Resampled to the wavelength grid, ends held.
    fn spectrum(&self) -> Spectrum {
        std::array::from_fn(|i| self.at(wavelength(i)))
    }

    /// Log10 sensitivity resampled to linear, zero outside the plotted range
    /// (Kodak stops the curves where the sensitivity becomes negligible).
    fn linear_sensitivity(&self) -> Spectrum {
        std::array::from_fn(|i| {
            let w = wavelength(i);
            if w < self.start - 1e-9 || w > self.end() + 1e-9 { 0.0 } else { 10f64.powf(self.at(w)) }
        })
    }

    fn validate(&self, what: &str) -> Result<()> {
        if self.values.len() < 2 || self.step.is_nan() || self.step <= 0.0 || self.values.iter().any(|v| !v.is_finite()) {
            bail!("{what}: needs a positive step and at least two finite values");
        }
        Ok(())
    }
}

/// Per-layer curves. A black-and-white film gives one `neutral` curve in
/// the TOML, used for all three channels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "RgbSeriesToml")]
pub struct RgbSeries {
    pub red: Series,
    pub green: Series,
    pub blue: Series,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RgbSeriesToml {
    Rgb { red: Series, green: Series, blue: Series },
    Neutral { neutral: Series },
}

impl From<RgbSeriesToml> for RgbSeries {
    fn from(t: RgbSeriesToml) -> Self {
        match t {
            RgbSeriesToml::Rgb { red, green, blue } => Self { red, green, blue },
            RgbSeriesToml::Neutral { neutral } => Self { red: neutral.clone(), green: neutral.clone(), blue: neutral },
        }
    }
}

impl RgbSeries {
    fn get(&self) -> [&Series; 3] {
        [&self.red, &self.green, &self.blue]
    }
}

/// Spectral dye densities of the cyan, magenta and yellow dyes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DyeSet {
    pub cyan: Series,
    pub magenta: Series,
    pub yellow: Series,
    /// The datasheet's neutral (for checking the digitisation; unused).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub neutral: Option<Series>,
}

impl DyeSet {
    fn get(&self) -> [&Series; 3] {
        [&self.cyan, &self.magenta, &self.yellow]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Densitometry {
    StatusA,
    StatusM,
    /// Black-and-white films: diffuse visual density (one curve).
    DiffuseVisual,
}

impl Densitometry {
    /// Densitometer bands of a colour densitometry (validated before use).
    fn bands(self) -> &'static [StatusBand; 3] {
        match self {
            Self::StatusA => &cie::STATUS_A,
            Self::StatusM | Self::DiffuseVisual => &cie::STATUS_M,
        }
    }
}

/// Spectral diffuse density of a processed negative (colour negatives).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NegativeDensity {
    pub midscale_neutral: Series,
    pub minimum: Series,
}

/// Modulation transfer per layer, in percent, at `frequencies` (cycles/mm).
/// A black-and-white film gives one `neutral` list in the TOML.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "MtfToml")]
pub struct Mtf {
    pub frequencies: Vec<f64>,
    pub red: Vec<f64>,
    pub green: Vec<f64>,
    pub blue: Vec<f64>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum MtfToml {
    Rgb { frequencies: Vec<f64>, red: Vec<f64>, green: Vec<f64>, blue: Vec<f64> },
    Neutral { frequencies: Vec<f64>, neutral: Vec<f64> },
}

impl From<MtfToml> for Mtf {
    fn from(t: MtfToml) -> Self {
        match t {
            MtfToml::Rgb { frequencies, red, green, blue } => Self { frequencies, red, green, blue },
            MtfToml::Neutral { frequencies, neutral } => {
                Self { frequencies, red: neutral.clone(), green: neutral.clone(), blue: neutral }
            }
        }
    }
}

/// Measured data for the taking film.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Datasheet {
    /// How `curves` were measured: Status A (slides) or Status M (negatives).
    pub densitometry: Densitometry,
    /// Characteristic curves: density vs log10 exposure (lux-seconds).
    pub curves: RgbSeries,
    /// Spectral sensitivity of the red-, green- and blue-sensitive layers:
    /// log10 sensitivity vs wavelength (nm).
    pub sensitivity: RgbSeries,
    /// Spectral dye densities (needed for reversal films).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dyes: Option<DyeSet>,
    /// Spectral density of the processed negative (needed for negatives).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub negative_density: Option<NegativeDensity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtf: Option<Mtf>,
    /// Push and pull processing the film supports (beyond normal, push 0).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub development: Vec<Development>,
    /// Curves the lab scanner is set up for (normal development), when
    /// `curves` are a pushed or pulled development. Set at render time.
    #[serde(skip)]
    pub scan_reference: Option<RgbSeries>,
}

/// One push or pull process: the film is rated `push` stops faster (pull
/// when negative) and developed to `curves` (measured, from a datasheet) or
/// to curves derived from normal development by `model`. With neither the
/// film is developed normally (the datasheet says its latitude covers it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Development {
    pub push: i32,
    /// Shown in the app, e.g. "Push 2 (EI 1600)".
    pub label: String,
    /// Where the data comes from, e.g. "F-4017: D-76, large tank, 10 min".
    #[serde(default)]
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curves: Option<RgbSeries>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<DevelopmentModel>,
}

/// A push or pull modelled from the film's normal curves, for films Kodak
/// published no push curves for. Each channel's curve becomes
/// `pivot + contrast * (D(log_h + shift) - pivot)`: steeper or flatter about
/// `pivot`, and `shift` log exposure faster. Below the pivot the curve eases
/// back to the film's minimum density, and the maximum is capped at the
/// normal maximum less `max_density_drop`. Values are red,
/// green, blue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DevelopmentModel {
    pub contrast: [f64; 3],
    pub shift: [f64; 3],
    /// Density held fixed. Defaults to each curve's minimum (a negative's fog).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pivot: Option<f64>,
    /// How much lower the maximum density gets (a pushed slide's thinner
    /// blacks). Without it a negative's density is not capped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_density_drop: Option<f64>,
}

impl DevelopmentModel {
    fn apply(&self, curves: &RgbSeries) -> RgbSeries {
        let [r, g, b] = [0, 1, 2].map(|ch| {
            let c = curves.get()[ch];
            let lo = c.values.iter().copied().fold(f64::MAX, f64::min);
            let hi = c.values.iter().copied().fold(f64::MIN, f64::max);
            let cap = self.max_density_drop.map_or(f64::MAX, |d| hi - d);
            let pivot = self.pivot.unwrap_or(lo);
            // Widen the grid by the shift so both ends still reach the
            // normal curve's end densities (a pulled slide's base stays clear).
            let s = self.shift[ch];
            let extra = (s.abs() / c.step).ceil() as usize;
            let start = if s > 0.0 { c.start - c.step * extra as f64 } else { c.start };
            let values = (0..c.values.len() + extra)
                .map(|k| {
                    let x = start + c.step * k as f64;
                    let d = c.at(x + self.shift[ch]);
                    let mut y = pivot + self.contrast[ch] * (d - pivot);
                    if d < pivot && pivot > lo {
                        // Below the pivot, ease back to the film's minimum
                        // density so the clear base stays clear (monotonic
                        // for any contrast between 0 and 2).
                        let w = (pivot - d) / (pivot - lo);
                        y = w * d + (1.0 - w) * y;
                    }
                    y.min(cap).max(lo)
                })
                .collect();
            Series { start, step: c.step, values }
        });
        RgbSeries { red: r, green: g, blue: b }
    }
}

impl Datasheet {
    /// The datasheet as developed with `push` (0 = normal): its curves
    /// replaced by that development's, and the stops it is rated faster by.
    /// `None` if the film has no such development.
    pub fn developed(&self, push: i32) -> Option<Self> {
        if push == 0 {
            return Some(self.clone());
        }
        let d = self.development.iter().find(|d| d.push == push)?;
        let mut out = self.clone();
        let curves = d.curves.clone().or_else(|| d.model.as_ref().map(|m| m.apply(&self.curves)));
        if let Some(c) = curves {
            out.scan_reference = Some(self.curves.clone());
            out.curves = c;
        }
        out.development.clear();
        Some(out)
    }
}

/// The paper a colour negative is printed on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrintStock {
    pub name: String,
    pub densitometry: Densitometry,
    /// Visual reflection density the reference grey prints at (lab aim).
    pub grey_density: f64,
    pub curves: RgbSeries,
    pub sensitivity: RgbSeries,
    pub dyes: DyeSet,
}

/// Film development process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Process {
    /// Slide film (E-6): the developed film itself is the image.
    Reversal,
    /// Colour negative (C-41), printed on paper.
    Negative,
    /// Black-and-white negative, scanned.
    Monochrome,
}

impl Datasheet {
    pub fn validate(&self, process: Process, print: Option<&PrintStock>) -> Result<()> {
        for (s, n) in self.curves.get().into_iter().zip(["red", "green", "blue"]) {
            s.validate(&format!("datasheet curve {n}"))?;
        }
        for (s, n) in self.sensitivity.get().into_iter().zip(["red", "green", "blue"]) {
            s.validate(&format!("datasheet sensitivity {n}"))?;
        }
        if let Some(m) = &self.mtf {
            let n = m.frequencies.len();
            if n < 2 || m.red.len() != n || m.green.len() != n || m.blue.len() != n {
                bail!("datasheet mtf: each layer needs one value per frequency");
            }
        }
        for d in &self.development {
            if d.push == 0 || self.development.iter().filter(|e| e.push == d.push).count() > 1 {
                bail!("development '{}': push must be non-zero and appear once", d.label);
            }
            if d.curves.is_some() && d.model.is_some() {
                bail!("development '{}': give measured curves or a model, not both", d.label);
            }
            if let Some(m) = &d.model {
                if m.contrast.iter().chain(&m.shift).any(|v| !v.is_finite()) || m.contrast.iter().any(|&a| a <= 0.0) {
                    bail!("development '{}': model needs positive contrast and finite shift", d.label);
                }
            }
            if let Some(c) = &d.curves {
                for (s, n) in c.get().into_iter().zip(["red", "green", "blue"]) {
                    s.validate(&format!("development '{}' curve {n}", d.label))?;
                }
            }
        }
        let mono = self.densitometry == Densitometry::DiffuseVisual;
        if mono != (process == Process::Monochrome) {
            bail!("diffuse visual densitometry is for black-and-white (monochrome) films only");
        }
        match process {
            Process::Monochrome => {
                let c = self.curves.get();
                if c[1] != c[0] || c[2] != c[0] {
                    bail!("a black-and-white film has one characteristic curve");
                }
            }
            Process::Reversal => {
                let d = self.dyes.as_ref().context("a reversal film needs [datasheet.dyes]")?;
                for s in d.get() {
                    s.validate("datasheet dye")?;
                }
            }
            Process::Negative => {
                let nd = self.negative_density.as_ref().context("a negative needs [datasheet.negative_density]")?;
                nd.midscale_neutral.validate("midscale_neutral")?;
                nd.minimum.validate("minimum")?;
                let p = print.context("a negative needs a [print] paper")?;
                for s in p.curves.get().into_iter().chain(p.sensitivity.get()).chain(p.dyes.get()) {
                    s.validate("print data")?;
                }
            }
        }
        Ok(())
    }
}

// ------------------------------------------------------------------ spectra

/// CIE daylight illuminant at a nominal temperature (D50 = 5000, D55 = 5500).
pub fn daylight(nominal_cct: f64) -> Spectrum {
    // CIE defines D-series by the revised radiation constant: 5000 -> 5003 K.
    let t = nominal_cct * 1.4388 / 1.4380;
    let xd = if t <= 7000.0 {
        -4.6070e9 / t.powi(3) + 2.9678e6 / t.powi(2) + 0.09911e3 / t + 0.244063
    } else {
        -2.0064e9 / t.powi(3) + 1.9018e6 / t.powi(2) + 0.24748e3 / t + 0.237040
    };
    let yd = -3.0 * xd * xd + 2.870 * xd - 0.275;
    let m = 0.0241 + 0.2562 * xd - 0.7341 * yd;
    let m1 = (-1.3515 - 1.7703 * xd + 5.9114 * yd) / m;
    let m2 = (0.0300 - 31.4424 * xd + 30.0717 * yd) / m;
    std::array::from_fn(|i| cie::DAYLIGHT_S0[i] + m1 * cie::DAYLIGHT_S1[i] + m2 * cie::DAYLIGHT_S2[i])
}

/// Linear densitometer weights (spectral products) for one ISO 5-3 band.
fn status_weights(b: &StatusBand) -> Spectrum {
    let last = b.wl0 + 10.0 * (b.values.len() - 1) as f64;
    std::array::from_fn(|i| {
        let w = wavelength(i);
        let v = if w < b.wl0 {
            b.values[0] - b.slope_lo * (b.wl0 - w)
        } else if w > last {
            b.values[b.values.len() - 1] + b.slope_hi * (w - last)
        } else {
            let t = (w - b.wl0) / 10.0;
            let k = (t as usize).min(b.values.len() - 2);
            b.values[k] + (b.values[k + 1] - b.values[k]) * (t - k as f64)
        };
        10f64.powf(v - 5.0)
    })
}

/// XYZ of a spectral transmittance/reflectance under an illuminant, with the
/// perfect white at Y = 1.
fn xyz_of(t: &Spectrum, illum: &Spectrum) -> Vec3 {
    let k: f64 = (0..NW).map(|i| illum[i] * cie::CMF[i][1]).sum();
    [0, 1, 2].map(|c| (0..NW).map(|i| t[i] * illum[i] * cie::CMF[i][c]).sum::<f64>() / k)
}

/// Density of a spectral density curve as read through weights `w`.
fn weighted_density(d: &Spectrum, w: &Spectrum) -> f64 {
    let num: f64 = (0..NW).map(|i| w[i] * 10f64.powf(-d[i])).sum();
    -(num / w.iter().sum::<f64>()).log10()
}

// --------------------------------------------------------- sensitivity fit

/// Synthetic scene reflectances: Gaussian bumps and smooth edges across the
/// visible range, plus a flat grey. Broad enough to constrain a 3x3 fit
/// without biasing it towards any one test chart.
fn training_set() -> Vec<Spectrum> {
    let mut out = Vec::new();
    for c in (400..=700).step_by(20) {
        for width in [15.0, 30.0, 60.0] {
            out.push(std::array::from_fn(|i| {
                0.05 + 0.85 * (-0.5 * ((wavelength(i) - c as f64) / width).powi(2)).exp()
            }));
        }
    }
    for e in (420..=680).step_by(20) {
        let s = |i: usize| 1.0 / (1.0 + (-(wavelength(i) - e as f64) / 12.0).exp());
        out.push(std::array::from_fn(|i| 0.05 + 0.85 * s(i)));
        out.push(std::array::from_fn(|i| 0.05 + 0.85 * (1.0 - s(i))));
    }
    out.push([0.9; NW]);
    out
}

/// Fit the matrix from working-space ProPhoto RGB (D50) to the relative
/// exposure of each layer, from the layers' spectral sensitivities.
pub fn fit_sensitivity(sens: &RgbSeries) -> Result<Mat3> {
    let illum = daylight(TAKING_CCT);
    let s = sens.get().map(|s| s.linear_sensitivity());
    let norm = s.map(|sc| (0..NW).map(|i| illum[i] * sc[i]).sum::<f64>());
    if norm.iter().any(|n| n.is_nan() || *n <= 0.0) {
        bail!("spectral sensitivity is zero over 380–730 nm");
    }
    let white = xyz_of(&[1.0; NW], &illum);
    let adapt = color::bradford(color::xyz_to_xy(&white), color::XY_D50);
    let to_rgb = color::mul(&color::XYZ_D50_TO_PROPHOTO, &adapt);
    // Weighted normal equations, one row of the matrix per layer.
    let mut ata = [[0.0; 3]; 3];
    let mut atb = [[0.0; 3]; 3]; // [layer][rgb]
    for t in training_set() {
        let e = [0, 1, 2].map(|c| (0..NW).map(|i| t[i] * illum[i] * s[c][i]).sum::<f64>() / norm[c]);
        let rgb = color::mul_vec(&to_rgb, &xyz_of(&t, &illum));
        // Relative weighting: errors matter in proportion to exposure (log response).
        let w2 = (1.0 / (e[0] + e[1] + e[2]).max(1e-3)).powi(2);
        for a in 0..3 {
            for b in 0..3 {
                ata[a][b] += w2 * rgb[a] * rgb[b];
            }
            for c in 0..3 {
                atb[c][a] += w2 * rgb[a] * e[c];
            }
        }
    }
    let inv = color::inverse(&ata).context("sensitivity fit is singular")?;
    let mut m = [0, 1, 2].map(|c| color::mul_vec(&inv, &atb[c]));
    // White (1, 1, 1) exposes every layer equally.
    for row in &mut m {
        let sum: f64 = row.iter().sum();
        row.iter_mut().for_each(|v| *v /= sum);
    }
    Ok(m)
}

// ------------------------------------------------------------- dye physics

/// Three dyes (plus an optional fixed base) whose amounts set a spectrum.
struct DyeStack {
    dyes: [Spectrum; 3],
    base: Spectrum,
}

impl DyeStack {
    fn density(&self, a: &Vec3) -> Spectrum {
        std::array::from_fn(|i| self.base[i] + a[0] * self.dyes[0][i] + a[1] * self.dyes[1][i] + a[2] * self.dyes[2][i])
    }

    /// Densitometer reading of the stack and its Jacobian w.r.t. dye amounts.
    fn read(&self, a: &Vec3, w: &[Spectrum; 3]) -> (Vec3, Mat3) {
        let t = self.density(a).map(|d| 10f64.powf(-d));
        let mut d = [0.0; 3];
        let mut j = [[0.0; 3]; 3];
        for c in 0..3 {
            let s: f64 = (0..NW).map(|i| w[c][i] * t[i]).sum();
            let total: f64 = w[c].iter().sum();
            d[c] = -(s / total).log10();
            for k in 0..3 {
                j[c][k] = (0..NW).map(|i| w[c][i] * t[i] * self.dyes[k][i]).sum::<f64>() / s;
            }
        }
        (d, j)
    }

    /// Dye amounts that read as `target` through `w` (Newton's method).
    fn solve(&self, target: &Vec3, w: &[Spectrum; 3], guess: Vec3) -> Vec3 {
        let mut a = guess;
        for _ in 0..50 {
            let (d, j) = self.read(&a, w);
            let r = [0, 1, 2].map(|c| target[c] - d[c]);
            if r.iter().all(|v| v.abs() < 1e-9) {
                break;
            }
            let Some(ji) = color::inverse(&j) else { break };
            let step = color::mul_vec(&ji, &r);
            a = [0, 1, 2].map(|c| a[c] + step[c]);
        }
        a
    }
}

/// A function of log exposure sampled on a uniform grid, per channel.
#[derive(Debug, Clone)]
pub struct Table {
    pub x0: f64,
    pub step: f64,
    pub y: Vec<Vec3>,
}

impl Table {
    pub fn x1(&self) -> f64 {
        self.x0 + self.step * (self.y.len() - 1) as f64
    }

    pub fn eval(&self, ch: usize, x: f64) -> f64 {
        let t = ((x - self.x0) / self.step).clamp(0.0, (self.y.len() - 1) as f64);
        let i = (t as usize).min(self.y.len() - 2);
        let f = t - i as f64;
        self.y[i][ch] + (self.y[i + 1][ch] - self.y[i][ch]) * f
    }

    fn max(&self) -> f64 {
        self.y.iter().flatten().cloned().fold(f64::MIN, f64::max)
    }

    fn min(&self) -> f64 {
        self.y.iter().flatten().cloned().fold(f64::MAX, f64::min)
    }
}

/// Grid covering all three curves at the finest of their steps.
fn curve_grid(c: &RgbSeries) -> (f64, f64, usize) {
    let s = c.get();
    let x0 = s.iter().map(|s| s.start).fold(f64::MAX, f64::min);
    let x1 = s.iter().map(|s| s.end()).fold(f64::MIN, f64::max);
    let step = s.iter().map(|s| s.step).fold(f64::MAX, f64::min);
    (x0, step, ((x1 - x0) / step).round() as usize + 1)
}

/// Per-channel densities of a set of curves on their common grid.
fn curves_table(curves: &RgbSeries) -> Table {
    let (x0, step, n) = curve_grid(curves);
    let c = curves.get();
    Table { x0, step, y: (0..n).map(|k| [0, 1, 2].map(|ch| c[ch].at(x0 + step * k as f64))).collect() }
}

/// Dye amounts per log exposure for neutral exposures: the datasheet curves
/// inverted through the dyes' spectra and the densitometer's response.
fn analytical_table(curves: &RgbSeries, stack: &DyeStack, w: &[Spectrum; 3]) -> Table {
    let (x0, step, n) = curve_grid(curves);
    let c = curves.get();
    let mut guess = [1.0; 3];
    let y = (0..n)
        .map(|k| {
            let x = x0 + step * k as f64;
            let target = [0, 1, 2].map(|ch| c[ch].at(x));
            guess = stack.solve(&target, w, guess);
            guess
        })
        .collect();
    Table { x0, step, y }
}

// ------------------------------------------------------------------ viewing

/// Dye amounts -> display-linear sRGB, relative to a white (film base or
/// paper), through a 3D table of log XYZ under the viewing illuminant.
#[derive(Debug, Clone)]
pub struct Viewer {
    pub(crate) lo: f32,
    pub(crate) scale: f32,
    pub(crate) log_xyz: Vec<[f32; 3]>,
    /// XYZ (D50) -> linear sRGB (D65), divided by the white's sRGB.
    pub(crate) to_rgb: [[f32; 3]; 3],
    /// Visual density of the white (base + minimum dye).
    pub white_density: f64,
}

impl Viewer {
    pub(crate) const N: usize = 41;

    fn new(stack: &DyeStack, lo: f64, hi: f64, white: &Vec3) -> Self {
        let illum = daylight(VIEWING_CCT);
        let n = Self::N;
        let step = (hi - lo) / (n - 1) as f64;
        let mut log_xyz = vec![[0f32; 3]; n * n * n];
        for (idx, out) in log_xyz.iter_mut().enumerate() {
            let a = [idx / (n * n), (idx / n) % n, idx % n].map(|k| lo + step * k as f64);
            let t = stack.density(&a).map(|d| 10f64.powf(-d));
            *out = xyz_of(&t, &illum).map(|v| v.max(1e-12).log10() as f32);
        }
        let white_t = stack.density(white).map(|d| 10f64.powf(-d));
        let white_xyz = xyz_of(&white_t, &illum);
        let wp = color::xyz_to_xy(&xyz_of(&[1.0; NW], &illum));
        let m = color::mul(&color::XYZ_D65_TO_SRGB, &color::bradford(wp, color::XY_D65));
        let white_rgb = color::mul_vec(&m, &white_xyz);
        let to_rgb = [0, 1, 2].map(|r| [0, 1, 2].map(|c| (m[r][c] / white_rgb[r]) as f32));
        Self { lo: lo as f32, scale: (1.0 / step) as f32, log_xyz, to_rgb, white_density: -white_xyz[1].log10() }
    }

    /// Display-linear RGB (white = 1) for dye amounts `a`.
    pub fn rgb(&self, a: [f32; 3]) -> [f32; 3] {
        let n = Self::N;
        let top = (n - 1) as f32;
        let mut i0 = [0usize; 3];
        let mut f = [0f32; 3];
        for c in 0..3 {
            let t = ((a[c] - self.lo) * self.scale).clamp(0.0, top);
            i0[c] = (t as usize).min(n - 2);
            f[c] = t - i0[c] as f32;
        }
        let mut acc = [0f32; 3];
        for corner in 0..8 {
            let d = [(corner >> 2) & 1, (corner >> 1) & 1, corner & 1];
            let w: f32 = (0..3).map(|c| if d[c] == 1 { f[c] } else { 1.0 - f[c] }).product();
            if w == 0.0 {
                continue;
            }
            let v = &self.log_xyz[((i0[0] + d[0]) * n + i0[1] + d[1]) * n + i0[2] + d[2]];
            for c in 0..3 {
                acc[c] += w * v[c];
            }
        }
        let xyz = acc.map(|v| 10f32.powf(v));
        let m = &self.to_rgb;
        [0, 1, 2].map(|r| m[r][0] * xyz[0] + m[r][1] * xyz[1] + m[r][2] * xyz[2])
    }
}

// --------------------------------------------------------------------- MTF

/// Layer spread fitted to an MTF curve: a Gaussian of `sigma_um`, plus an
/// adjacency term `(1 + k) * G(sigma) - k * G(sigma_wide)` that lifts mid
/// frequencies above 100% the way development adjacency effects do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spread {
    pub sigma_um: f64,
    pub k: f64,
    pub wide_um: f64,
}

impl Spread {
    pub fn response(&self, f_per_mm: f64) -> f64 {
        let g = |s_um: f64| (-2.0 * std::f64::consts::PI.powi(2) * (s_um * 1e-3).powi(2) * f_per_mm * f_per_mm).exp();
        (1.0 + self.k) * g(self.sigma_um) - self.k * g(self.wide_um)
    }
}

/// Least-squares fit of a [`Spread`] to measured response (percent).
pub fn fit_spread(freq: &[f64], percent: &[f64]) -> Spread {
    let err = |s: &Spread| -> f64 {
        freq.iter().zip(percent).map(|(f, p)| (s.response(*f) - p / 100.0).powi(2)).sum()
    };
    let mut best = (f64::MAX, Spread { sigma_um: 1.0, k: 0.0, wide_um: 2.0 });
    let consider = |best: &mut (f64, Spread), s: Spread| {
        let e = err(&s);
        if e < best.0 {
            *best = (e, s);
        }
    };
    // Coarse grid, then a finer one around the best point.
    for si in 0..80 {
        let sigma_um = 0.25 + 0.25 * si as f64;
        for ki in 0..=30 {
            let k = 0.02 * ki as f64;
            for ri in 0..=24 {
                let wide_um = sigma_um * (1.5 + 0.5 * ri as f64);
                consider(&mut best, Spread { sigma_um, k, wide_um });
                if k == 0.0 {
                    break;
                }
            }
        }
    }
    let c = best.1;
    for si in -10..=10 {
        for ki in -10..=10 {
            for ri in -10..=10 {
                let sigma_um = (c.sigma_um + 0.025 * si as f64).max(0.05);
                let k = (c.k + 0.002 * ki as f64).max(0.0);
                let wide_um = (c.wide_um * (1.0 + 0.02 * ri as f64)).max(sigma_um * 1.01);
                consider(&mut best, Spread { sigma_um, k, wide_um });
            }
        }
    }
    best.1
}

// ------------------------------------------------------------ compiled core

/// A colour negative's printing stage.
#[derive(Debug, Clone)]
pub struct Printer {
    /// Status M -> printing density gain per channel (R, G, B).
    pub gain: Vec3,
    /// Status M minimum density of the negative.
    pub dmin: Vec3,
    /// Printer lights: paper log exposure = lights - printing density.
    pub lights: Vec3,
    /// Paper dye amounts vs paper log exposure (neutral exposures).
    pub paper: Table,
}

impl Printer {
    pub fn paper_log_h(&self, d: [f64; 3]) -> Vec3 {
        [0, 1, 2].map(|c| self.lights[c] - self.gain[c] * (d[c] - self.dmin[c]))
    }
}

/// A lab scanner reading a colour negative, the way a minilab scanner
/// (Frontier, Noritsu) does: it measures the negative's printing densities
/// above the orange mask, grey-balances each channel at the reference grey,
/// inverts, and applies its own gentle tone curve. The film's own toe and
/// shoulder are kept (densities are inverted linearly, not through the
/// film's curve); the scanner curve is softer than paper, with a long
/// highlight shoulder, so the negative's highlight latitude survives.
#[derive(Debug, Clone)]
pub struct LabScan {
    pub(crate) gain: Vec3,
    pub(crate) dmin: Vec3,
    /// Printing density (above base) of the reference grey, per channel.
    pub(crate) p_ref: Vec3,
    /// Printing density per log10 exposure at the reference grey.
    pub(crate) slope: Vec3,
    /// Scanner tone curve: output density = dmax / (1 + exp(k (rel - c))).
    pub(crate) k: f64,
    pub(crate) c: f64,
}

/// Scanner tone curve: deepest output density, density the reference grey
/// lands at (about an 18-20% grey), and mid-tone contrast in output density
/// per log10 exposure. Softer than a print (paper runs at about 1.5-1.7).
pub(crate) const LAB_SCAN_DMAX: f64 = 2.4;
const LAB_SCAN_GREY: f64 = 0.70;
const LAB_SCAN_GAMMA: f64 = 1.15;

impl LabScan {
    /// `reference` is the development the scanner is set up for (normal
    /// processing) and `normal_log_h_ref` its grey: the scanner keeps that
    /// contrast, so a push or pull shows its own. Grey is still balanced on
    /// the film actually scanned, as a scanner's auto-density does.
    fn new(develop: &Table, reference: &Table, gain: Vec3, dmin: Vec3, log_h_ref: f64, normal_log_h_ref: f64) -> Self {
        let p = |t: &Table, c: usize, x: f64| gain[c] * (t.eval(c, x) - dmin[c]);
        let p_ref = [0, 1, 2].map(|c| p(develop, c, log_h_ref));
        let x = normal_log_h_ref;
        let slope = [0, 1, 2].map(|c| ((p(reference, c, x + 0.15) - p(reference, c, x - 0.15)) / 0.3).max(0.05));
        // Logistic through (0, LAB_SCAN_GREY) with slope -LAB_SCAN_GAMMA there.
        let e0 = LAB_SCAN_DMAX / LAB_SCAN_GREY - 1.0;
        let k = LAB_SCAN_GAMMA * (1.0 + e0) * (1.0 + e0) / (LAB_SCAN_DMAX * e0);
        let c = -e0.ln() / k;
        Self { gain, dmin, p_ref, slope, k, c }
    }

    /// Display-linear scanner RGB (white = 1) for Status M densities.
    pub fn rgb(&self, state: [f32; 3]) -> [f32; 3] {
        [0, 1, 2].map(|c| {
            let p = self.gain[c] * (state[c] as f64 - self.dmin[c]);
            let rel = (p - self.p_ref[c]) / self.slope[c];
            let d = LAB_SCAN_DMAX / (1.0 + (self.k * (rel - self.c)).exp());
            10f64.powf(-d) as f32
        })
    }
}

/// Everything derived from a datasheet, computed once per preset.
#[derive(Debug)]
pub struct Core {
    pub process: Process,
    /// ProPhoto (D50) -> relative layer exposure; white -> (1, 1, 1).
    pub to_layers: Mat3,
    /// Per log exposure: dye amounts (reversal) or Status M density (negative).
    pub develop: Table,
    pub printer: Option<Printer>,
    /// Negatives: the lab-scanner alternative to printing.
    pub lab_scan: Option<LabScan>,
    pub viewer: Viewer,
    /// Fitted layer spreads (R, G, B), if the datasheet has MTF data.
    pub spread: Option<[Spread; 3]>,
}

#[derive(PartialEq)]
struct Key {
    process: Process,
    datasheet: Datasheet,
    print: Option<PrintStock>,
    log_h_ref: u64,
    push: i32,
}

static CACHE: Mutex<Vec<(Key, Arc<Core>)>> = Mutex::new(Vec::new());

impl Core {
    /// Build (or fetch from cache) the model for a datasheet. `log_h_ref` is
    /// the log exposure of the reference grey at the film's normal rating,
    /// used to aim the print. `push` picks one of the datasheet's push/pull
    /// developments (0 = normal); the film is then rated `push` stops faster.
    pub fn get(
        process: Process,
        ds: &Datasheet,
        print: Option<&PrintStock>,
        log_h_ref: f64,
        push: i32,
    ) -> Result<Arc<Core>> {
        let key = Key { process, datasheet: ds.clone(), print: print.cloned(), log_h_ref: log_h_ref.to_bits(), push };
        if let Some((_, c)) = CACHE.lock().expect("cache").iter().find(|(k, _)| *k == key) {
            return Ok(c.clone());
        }
        let core = Arc::new(Self::build(process, ds, print, log_h_ref, push)?);
        let mut cache = CACHE.lock().expect("cache");
        // Room for every built-in film plus a few push/pull variants.
        if cache.len() >= 32 {
            cache.remove(0);
        }
        cache.push((key, core.clone()));
        Ok(core)
    }

    fn build(process: Process, ds: &Datasheet, print: Option<&PrintStock>, log_h_ref: f64, push: i32) -> Result<Self> {
        ds.validate(process, print)?;
        let normal_log_h_ref = log_h_ref;
        let developed =
            ds.developed(push).with_context(|| format!("this film has no push/pull development of {push:+} stops"))?;
        let ds = &developed;
        // Rated `push` stops faster: the grey card gets that much less exposure.
        let log_h_ref = normal_log_h_ref - push as f64 * 2f64.log10();
        // The development the lab scanner is set up for.
        let reference = ds.scan_reference.as_ref().unwrap_or(&ds.curves);
        let to_layers = fit_sensitivity(&ds.sensitivity)?;
        let spread = ds.mtf.as_ref().map(|m| {
            [&m.red, &m.green, &m.blue].map(|v| fit_spread(&m.frequencies, v))
        });
        match process {
            Process::Monochrome => {
                // One silver image: the same visual density in every channel.
                let (x0, step, n) = curve_grid(&ds.curves);
                let curve = &ds.curves.green;
                let develop = Table { x0, step, y: (0..n).map(|k| [curve.at(x0 + step * k as f64); 3]).collect() };
                let dmin = [curve.values[0]; 3];
                let scan_ref = curves_table(reference);
                let lab_scan = LabScan::new(&develop, &scan_ref, [1.0; 3], dmin, log_h_ref, normal_log_h_ref);
                // Silver is close to spectrally neutral: viewing the negative
                // itself (unused by the app) is its density read flat.
                let flat = [1.0 / 3.0; NW];
                let stack = DyeStack { dyes: [flat; 3], base: [0.0; NW] };
                let viewer = Viewer::new(&stack, 0.0, develop.max() + 0.3, &dmin);
                Ok(Self { process, to_layers, develop, printer: None, lab_scan: Some(lab_scan), viewer, spread })
            }
            Process::Reversal => {
                let d = ds.dyes.as_ref().expect("validated");
                let stack = DyeStack { dyes: d.get().map(|s| s.spectrum()), base: [0.0; NW] };
                let w = ds.densitometry.bands().each_ref().map(status_weights);
                let develop = analytical_table(&ds.curves, &stack, &w);
                // White: the clear film (base + minimum dye), at the top of the curves.
                let white = develop.y[develop.y.len() - 1];
                let viewer = Viewer::new(&stack, develop.min().min(0.0), develop.max() + 0.3, &white);
                Ok(Self { process, to_layers, develop, printer: None, lab_scan: None, viewer, spread })
            }
            Process::Negative => {
                let p = print.expect("validated");
                let nd = ds.negative_density.as_ref().expect("validated");
                // Negative: per-layer Status M density, as published.
                let (x0, step, n) = curve_grid(&ds.curves);
                let c = ds.curves.get();
                let develop = Table {
                    x0,
                    step,
                    y: (0..n).map(|k| [0, 1, 2].map(|ch| c[ch].at(x0 + step * k as f64))).collect(),
                };
                let dmin = [0, 1, 2].map(|ch| c[ch].values[0]);

                // Printing density / Status M gains from the negative's spectra.
                let paper_sens = p.sensitivity.get().map(|s| s.linear_sensitivity());
                let status = ds.densitometry.bands().each_ref().map(status_weights);
                let (neu, min) = (nd.midscale_neutral.spectrum(), nd.minimum.spectrum());
                let gain = [0, 1, 2].map(|ch| {
                    let printing = weighted_density(&neu, &paper_sens[ch]) - weighted_density(&min, &paper_sens[ch]);
                    let measured = weighted_density(&neu, &status[ch]) - weighted_density(&min, &status[ch]);
                    printing / measured
                });

                // Paper: flat base at its lowest Dmin; dyes from the curves.
                let pc = p.curves.get();
                let base = [0, 1, 2].map(|ch| pc[ch].values[0]).into_iter().fold(f64::MAX, f64::min);
                let stack = DyeStack { dyes: p.dyes.get().map(|s| s.spectrum()), base: [base; NW] };
                let pw = p.densitometry.bands().each_ref().map(status_weights);
                let mut guess = [0.0; 3];
                let (px0, pstep, pn) = curve_grid(&p.curves);
                let paper = Table {
                    x0: px0,
                    step: pstep,
                    y: (0..pn)
                        .map(|k| {
                            let x = px0 + pstep * k as f64;
                            guess = stack.solve(&[0, 1, 2].map(|ch| pc[ch].at(x)), &pw, guess);
                            guess
                        })
                        .collect(),
                };
                let white = paper.y[0];
                let viewer = Viewer::new(&stack, paper.min().min(0.0), paper.max() + 0.3, &white);

                // Printer lights: the reference grey prints neutral (relative to
                // paper white) at the paper's aim density.
                let d_ref = [0, 1, 2].map(|ch| develop.eval(ch, log_h_ref));
                let target = (p.grey_density - viewer.white_density).max(0.05);
                let mut printer = Printer { gain, dmin, lights: [0.0; 3], paper };
                let display_density = |pr: &Printer| -> Vec3 {
                    let lh = pr.paper_log_h(d_ref);
                    let a = [0, 1, 2].map(|ch| pr.paper.eval(ch, lh[ch]) as f32);
                    viewer.rgb(a).map(|v| -(v.max(1e-6) as f64).log10())
                };
                // Start where the grey lands mid-curve, then Newton with a
                // numerical Jacobian.
                let mid = 0.5 * (printer.paper.x0 + printer.paper.x1());
                printer.lights = [0, 1, 2].map(|c| mid + gain[c] * (d_ref[c] - dmin[c]));
                for _ in 0..40 {
                    let d0 = display_density(&printer);
                    let r = d0.map(|v| target - v);
                    if r.iter().all(|v| v.abs() < 1e-5) {
                        break;
                    }
                    let mut j = [[0.0; 3]; 3];
                    for k in 0..3 {
                        let mut q = printer.clone();
                        q.lights[k] += 1e-3;
                        let d1 = display_density(&q);
                        for c in 0..3 {
                            j[c][k] = (d1[c] - d0[c]) / 1e-3;
                        }
                    }
                    let Some(ji) = color::inverse(&j) else { bail!("printer-light solve is singular") };
                    let s = color::mul_vec(&ji, &r);
                    printer.lights = [0, 1, 2].map(|c| printer.lights[c] + s[c].clamp(-0.5, 0.5));
                }
                let scan_ref = curves_table(reference);
                let lab_scan = LabScan::new(&develop, &scan_ref, gain, dmin, log_h_ref, normal_log_h_ref);
                Ok(Self {
                    process,
                    to_layers,
                    develop,
                    printer: Some(printer),
                    lab_scan: Some(lab_scan),
                    viewer,
                    spread,
                })
            }
        }
    }

    /// Display-linear RGB (white = film base or paper) of a developed state:
    /// dye amounts for a reversal film, Status M densities for a negative.
    pub fn view(&self, state: [f32; 3]) -> [f32; 3] {
        match &self.printer {
            Some(pr) => {
                let lh = [0, 1, 2].map(|c| pr.lights[c] - pr.gain[c] * (state[c] as f64 - pr.dmin[c]));
                self.viewer.rgb([0, 1, 2].map(|c| pr.paper.eval(c, lh[c]) as f32))
            }
            None => self.viewer.rgb(state),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daylight_white_points_match_cie() {
        let d50 = color::xyz_to_xy(&xyz_of(&[1.0; NW], &daylight(5000.0)));
        let d65 = color::xyz_to_xy(&xyz_of(&[1.0; NW], &daylight(6500.0)));
        assert!((d50[0] - color::XY_D50[0]).abs() < 5e-4 && (d50[1] - color::XY_D50[1]).abs() < 5e-4, "{d50:?}");
        assert!((d65[0] - color::XY_D65[0]).abs() < 5e-4 && (d65[1] - color::XY_D65[1]).abs() < 5e-4, "{d65:?}");
    }

    #[test]
    fn status_responses_peak_at_one() {
        for b in cie::STATUS_A.iter().chain(&cie::STATUS_M) {
            let w = status_weights(b);
            let peak = w.iter().cloned().fold(0.0, f64::max);
            assert!((peak - 1.0).abs() < 1e-9);
        }
    }

    #[test]
    fn spread_fit_reproduces_a_gaussian() {
        let s = Spread { sigma_um: 5.0, k: 0.0, wide_um: 10.0 };
        let f = [5.0, 10.0, 20.0, 40.0, 80.0];
        let p: Vec<f64> = f.iter().map(|f| s.response(*f) * 100.0).collect();
        let fit = fit_spread(&f, &p);
        for f in f {
            assert!((fit.response(f) - s.response(f)).abs() < 0.01);
        }
    }
}

#[cfg(test)]
mod datasheet_tests {
    use super::*;
    use crate::film::{FilmModel, FilmParams, FilmPreset};

    fn core(id: &str) -> (FilmPreset, Arc<Core>) {
        let p = FilmPreset::builtin(id).unwrap();
        let c = Core::get(
            p.process.unwrap(),
            p.datasheet.as_ref().unwrap(),
            p.print.as_ref(),
            p.exposure.log_h_ref.unwrap(),
            0,
        )
        .unwrap();
        (p, c)
    }

    /// Diagnostic: `cargo test -p remjet-core spectral_report -- --nocapture --ignored`
    #[test]
    #[ignore]
    fn spectral_report() {
        for id in ["ektachrome-e100", "ektachrome-e100vs", "ektachrome-e200", "portra160", "portra400", "ektar100", "gold200", "ultramax400", "portra800"] {
            let (p, c) = core(id);
            println!("== {id}");
            println!("to_layers {:?}", c.to_layers.map(|r| r.map(|v| (v * 1000.0).round() / 1000.0)));
            println!("spread {:?}", c.spread);
            if let Some(pr) = &c.printer {
                println!("gain {:?} lights {:?} dmin {:?}", pr.gain, pr.lights, pr.dmin);
            }
            println!("white density {}", c.viewer.white_density);
            let mut q = p.clone();
            q.scan.neutral_balance = 0.0;
            q.scan.gamma = 1.0;
            for (label, preset) in [("raw", &q), ("preset", &p)] {
                let m = FilmModel::new(preset, &FilmParams { grain: 0.0, ..Default::default() }).unwrap();
                println!(" {label} ramp:");
                for s in -6..=5 {
                    let v = p.exposure.grey_linear as f32 * 2f32.powi(s);
                    let o = m.scan(m.densities([v; 3]));
                    println!("  {s:+}: {:?}", o.map(|c| (crate::color::srgb_encode(c) * 255.0).round() as i32));
                }
            }
        }
    }
    fn status(d: Densitometry) -> [Spectrum; 3] {
        d.bands().each_ref().map(status_weights)
    }

    #[test]
    fn reversal_neutrals_reproduce_status_a_curves() {
        let (p, c) = core("ektachrome-e100");
        let ds = p.datasheet.unwrap();
        let stack = DyeStack { dyes: ds.dyes.as_ref().unwrap().get().map(|s| s.spectrum()), base: [0.0; NW] };
        let w = status(ds.densitometry);
        for (k, a) in c.develop.y.iter().enumerate() {
            let x = c.develop.x0 + c.develop.step * k as f64;
            let (d, _) = stack.read(a, &w);
            for (ch, s) in ds.curves.get().into_iter().enumerate() {
                assert!((d[ch] - s.at(x)).abs() < 1e-4, "log H {x}: {d:?}");
            }
        }
    }

    #[test]
    fn e100_dyes_form_the_published_visual_neutral() {
        let (p, _) = core("ektachrome-e100");
        let dyes = p.datasheet.unwrap().dyes.unwrap();
        let neutral = dyes.neutral.as_ref().unwrap().spectrum();
        let parts = dyes.get().map(|s| s.spectrum());
        for i in (0..NW).filter(|&i| (405.0..=700.0).contains(&wavelength(i))) {
            let sum = parts[0][i] + parts[1][i] + parts[2][i];
            assert!((sum - neutral[i]).abs() < 0.03, "{} nm: {sum} vs {}", wavelength(i), neutral[i]);
        }
        // Kodak normalises the dyes to a visual neutral density of 1.0 at 5000 K.
        let t = std::array::from_fn(|i| 10f64.powf(-(parts[0][i] + parts[1][i] + parts[2][i])));
        let visual = -xyz_of(&t, &daylight(5000.0))[1].log10();
        assert!((visual - 1.0).abs() < 0.05, "visual density {visual}");
    }

    #[test]
    fn sensitivity_fit_is_layer_dominant_and_white_balanced() {
        for id in ["ektachrome-e100", "ektachrome-e100vs", "ektachrome-e200", "portra160", "portra400", "ektar100", "gold200", "ultramax400", "portra800"] {
            let (_, c) = core(id);
            for (i, row) in c.to_layers.iter().enumerate() {
                assert!((row.iter().sum::<f64>() - 1.0).abs() < 1e-9);
                assert!(row[i] > 0.9 && row.iter().enumerate().all(|(j, v)| j == i || v.abs() < 0.4), "{id}: {row:?}");
            }
        }
    }

    #[test]
    fn paper_neutrals_reproduce_status_a_curves() {
        let (p, c) = core("portra400");
        let pr = p.print.unwrap();
        let printer = c.printer.as_ref().unwrap();
        let pc = pr.curves.get();
        let base = pc.iter().map(|s| s.values[0]).fold(f64::MAX, f64::min);
        let stack = DyeStack { dyes: pr.dyes.get().map(|s| s.spectrum()), base: [base; NW] };
        let w = status(pr.densitometry);
        for (k, a) in printer.paper.y.iter().enumerate() {
            let x = printer.paper.x0 + printer.paper.step * k as f64;
            let (d, _) = stack.read(a, &w);
            for ch in 0..3 {
                assert!((d[ch] - pc[ch].at(x)).abs() < 1e-4, "log H {x}: {d:?}");
            }
        }
    }

    #[test]
    fn portra_grey_prints_neutral_at_the_aim_density() {
        let (p, c) = core("portra400");
        let output = crate::film::OutputMode::Print;
        let m = FilmModel::new(&p, &FilmParams { grain: 0.0, output, ..Default::default() }).unwrap();
        let g = p.exposure.grey_linear as f32;
        let d = m.densities([g; 3]);
        let aim = (p.print.unwrap().grey_density - c.viewer.white_density) as f32;
        // Flare adds a touch of exposure, so allow a little below the aim.
        assert!(d.iter().all(|v| (v - aim).abs() < 0.02), "{d:?} vs {aim}");
        let spread = d.iter().cloned().fold(f32::MIN, f32::max) - d.iter().cloned().fold(f32::MAX, f32::min);
        assert!(spread < 0.003, "grey not neutral: {d:?}");
        // Printing gains come from the negative's spectra, not from Status M.
        let gain = c.printer.as_ref().unwrap().gain;
        assert!(gain[0] > 1.05 && gain[2] < 0.97, "{gain:?}");
    }

    #[test]
    fn every_negative_scans_and_prints_its_grey_neutral() {
        use crate::film::OutputMode;
        for id in ["portra160", "portra400", "ektar100", "gold200", "ultramax400", "portra800"] {
            let (p, c) = core(id);
            let g = p.exposure.grey_linear as f32;
            let m = FilmModel::new(&p, &FilmParams { grain: 0.0, output: OutputMode::Print, ..Default::default() }).unwrap();
            let d = m.densities([g; 3]);
            let aim = (p.print.as_ref().unwrap().grey_density - c.viewer.white_density) as f32;
            assert!(d.iter().all(|v| (v - aim).abs() < 0.02), "{id}: {d:?} vs {aim}");
            let spread = d.iter().cloned().fold(f32::MIN, f32::max) - d.iter().cloned().fold(f32::MAX, f32::min);
            assert!(spread < 0.003, "{id}: printed grey not neutral: {d:?}");
            let m = FilmModel::new(&p, &FilmParams { grain: 0.0, output: OutputMode::Scan, ..Default::default() }).unwrap();
            let o = m.scan(m.densities([g; 3]));
            let spread = o.iter().cloned().fold(f32::MIN, f32::max) - o.iter().cloned().fold(f32::MAX, f32::min);
            assert!(spread < 0.01, "{id}: scanned grey not neutral: {o:?}");
        }
    }

    #[test]
    fn ramps_are_monotonic_from_black_to_white() {
        for id in [
            "ektachrome-e100",
            "ektachrome-e100vs",
            "ektachrome-e200",
            "portra160",
            "portra400",
            "ektar100",
            "gold200",
            "ultramax400",
            "portra800",
            "trix400",
            "tmax100",
            "tmax400",
        ] {
            let (p, _) = core(id);
            let m = FilmModel::new(&p, &FilmParams { grain: 0.0, ..Default::default() }).unwrap();
            let g = p.exposure.grey_linear as f32;
            let mut last = [-1.0f32; 3];
            for i in 0..60 {
                let v = g * 2f32.powf(-8.0 + 0.2 * i as f32);
                let o = m.scan(m.densities([v; 3]));
                assert!((0..3).all(|c| o[c] >= last[c] - 1e-4), "{id}: not monotonic at {v}: {last:?} -> {o:?}");
                last = o;
            }
            assert!(last.iter().all(|&c| c > 0.97), "{id}: highlights should reach white: {last:?}");
            let black = m.scan(m.densities([g / 256.0; 3]));
            assert!(black.iter().all(|&c| c < 0.03), "{id}: deep shadows should be near black: {black:?}");
        }
    }

    #[test]
    fn negatives_are_scanned_or_printed_never_projected() {
        use crate::film::OutputMode;
        let p = FilmPreset::builtin("portra400").unwrap();
        let g = p.exposure.grey_linear as f32;
        let img = crate::develop::LinearImage { width: 8, height: 8, pixels: vec![[g * 4.0; 3]; 64], cct: 5000.0 };
        let render = |output| {
            let m = FilmModel::new(&p, &FilmParams { grain: 0.0, output, ..Default::default() }).unwrap();
            m.apply(&img, 0).pixels
        };
        assert_eq!(render(OutputMode::Scan), render(OutputMode::Projection));
        assert_ne!(render(OutputMode::Scan), render(OutputMode::Print), "lab scan and print should differ");
    }

    #[test]
    fn lab_scan_keeps_more_highlight_separation_than_the_print() {
        use crate::film::OutputMode;
        let p = FilmPreset::builtin("portra400").unwrap();
        let g = p.exposure.grey_linear as f32;
        let tone = |output, stops: f32| {
            let m = FilmModel::new(&p, &FilmParams { grain: 0.0, output, ..Default::default() }).unwrap();
            m.scan(m.densities([g * 2f32.powf(stops); 3]))
        };
        for output in [OutputMode::Scan, OutputMode::Print] {
            let grey = tone(output, 0.0);
            let spread = grey.iter().cloned().fold(f32::MIN, f32::max) - grey.iter().cloned().fold(f32::MAX, f32::min);
            assert!(spread < 0.01, "{output:?}: grey not neutral: {grey:?}");
        }
        // The camera's top stop and a half (+1.5 .. +3 stops over grey), with
        // the default highlight fit.
        let range = |o| tone(o, 3.0)[1] - tone(o, 1.5)[1];
        assert!(range(OutputMode::Scan) > 1.5 * range(OutputMode::Print), "{} vs {}", range(OutputMode::Scan), range(OutputMode::Print));
    }

    #[test]
    fn black_and_white_films_are_exactly_neutral_and_reproduce_their_curve() {
        for id in ["trix400", "tmax100", "tmax400"] {
            black_and_white_is_neutral(id);
        }
    }

    fn black_and_white_is_neutral(id: &str) {
        let (p, c) = core(id);
        let ds = p.datasheet.as_ref().unwrap();
        // One curve in the TOML, the same in every channel.
        assert_eq!(ds.curves.red, ds.curves.blue);
        for (k, d) in c.develop.y.iter().enumerate() {
            let x = c.develop.x0 + c.develop.step * k as f64;
            assert!(d.iter().all(|v| (v - ds.curves.green.at(x)).abs() < 1e-9));
        }
        let m = FilmModel::new(&p, &FilmParams { grain: 0.0, halation: 1.0, ..Default::default() }).unwrap();
        let g = p.exposure.grey_linear as f32;
        // Coloured light still gives a neutral (grey) result.
        for rgb in [[g, g, g], [g * 3.0, g * 0.2, g * 0.1], [g * 0.1, g * 0.5, g * 4.0], [1.0, 0.5, 0.05]] {
            let o = m.scan(m.densities(rgb));
            assert!((o[0] - o[1]).abs() < 1e-5 && (o[2] - o[1]).abs() < 1e-5, "{id}: {rgb:?} -> {o:?}");
        }
        // Panchromatic: strong red light still exposes the film.
        let red = m.scan(m.densities([g * 2.0, g * 0.05, g * 0.05]))[1];
        assert!(red > 0.02, "{id}: {red}");
        // The grey card lands mid-scale, not at the ends.
        let grey = m.scan(m.densities([g; 3]))[1];
        assert!((0.1..0.4).contains(&grey), "{id}: {grey}");
    }

    #[test]
    fn every_push_and_pull_renders_with_grey_held_and_contrast_following_the_push() {
        for id in [
            "ektachrome-e100",
            "ektachrome-e100vs",
            "ektachrome-e200",
            "portra160",
            "portra400",
            "ektar100",
            "gold200",
            "ultramax400",
            "portra800",
            "trix400",
            "tmax100",
            "tmax400",
        ] {
            let p = FilmPreset::builtin(id).unwrap();
            let pushes = p.developments();
            assert!(pushes.len() >= 2, "{id}: expected pull and push options, got {pushes:?}");
            let g = p.exposure.grey_linear as f32;
            // Contrast of the scan about grey (one stop either side), green.
            let look = |push: i32| {
                let m = FilmModel::new(&p, &FilmParams { grain: 0.0, push, ..Default::default() }).unwrap();
                let at = |stops: f32| m.scan(m.densities([g * 2f32.powf(stops); 3]));
                let mut last = [-1.0f32; 3];
                for i in 0..60 {
                    let o = at(-8.0 + 0.2 * i as f32);
                    // (A capped slide black can dip by a hair as the dye
                    // layers flatten at slightly different exposures: up
                    // to 0.001 of white on E100VS, whose red maximum is
                    // lower than its green and blue.)
                    assert!(
                        (0..3).all(|c| o[c] >= last[c] - 1.2e-3),
                        "{id} push {push}: not monotonic: {last:?} -> {o:?}"
                    );
                    last = o;
                }
                (at(0.0), at(1.0)[1] - at(-1.0)[1])
            };
            let (grey0, contrast0) = look(0);
            for &(push, _) in &pushes {
                let (grey, contrast) = look(push);
                // The lab scanner's auto-density (or, for a slide, the push
                // rating) puts grey back where it was.
                assert!(
                    (0..3).all(|c| (grey[c] - grey0[c]).abs() < 0.03),
                    "{id} push {push}: grey {grey:?} vs {grey0:?}"
                );
                // Pulls are softer, longer pushes contrastier. (Tri-X push 1
                // is normal development, merely underexposed.)
                if push < 0 {
                    assert!(contrast < contrast0 - 0.005, "{id} push {push}: contrast {contrast} vs {contrast0}");
                } else if push >= 2 {
                    assert!(contrast > contrast0 + 0.005, "{id} push {push}: contrast {contrast} vs {contrast0}");
                }
            }
        }
    }

    #[test]
    fn modelled_development_steepens_about_the_pivot_and_moves_faster() {
        let line = Series { start: -2.0, step: 0.5, values: vec![0.2, 0.2, 0.7, 1.2, 1.7] };
        let curves = RgbSeries { red: line.clone(), green: line.clone(), blue: line };
        let m = DevelopmentModel { contrast: [1.2; 3], shift: [0.5; 3], pivot: None, max_density_drop: Some(0.3) };
        let out = m.apply(&curves);
        // Fog held; one step faster; 1.2x the density above fog; capped at 1.7 - 0.3.
        assert_eq!((out.green.start, out.green.values.len()), (-2.5, 6));
        let want = [0.2, 0.2, 0.8, 1.4, 1.4, 1.4];
        for (v, w) in out.green.values.iter().zip(want) {
            assert!((v - w).abs() < 1e-9, "{:?}", out.green.values);
        }
    }

    #[test]
    fn trix_push_uses_kodaks_measured_curves() {
        let p = FilmPreset::builtin("trix400").unwrap();
        let ds = p.datasheet.as_ref().unwrap();
        let d2 = ds.developed(2).unwrap();
        let measured = ds.development.iter().find(|d| d.push == 2).unwrap().curves.as_ref().unwrap();
        assert_eq!(&d2.curves, measured);
        assert_eq!(d2.scan_reference.as_ref(), Some(&ds.curves));
        // Push 1 is normal development, rated one stop faster.
        assert_eq!(ds.developed(1).unwrap().curves, ds.curves);
        assert!(ds.developed(5).is_none());
        assert!(p.developments().iter().all(|d| !d.1.contains("modelled")));
        // Portra 800's pushes are Kodak's measured curves too.
        let p800 = FilmPreset::builtin("portra800").unwrap();
        assert_eq!(p800.developments().iter().map(|d| d.0).collect::<Vec<_>>(), [1, 2]);
        assert!(p800.developments().iter().all(|d| !d.1.contains("modelled")));
        let e100 = FilmPreset::builtin("ektachrome-e100").unwrap();
        assert!(e100.developments().iter().all(|d| d.1.ends_with("· modelled")));
        // T-Max 400's pull and pushes are measured; T-Max 100 has no curve
        // shorter than normal, so only its pull is modelled.
        let t400 = FilmPreset::builtin("tmax400").unwrap();
        assert_eq!(t400.developments().iter().map(|d| d.0).collect::<Vec<_>>(), [-1, 1, 2]);
        assert!(t400.developments().iter().all(|d| !d.1.contains("modelled")));
        let t100 = FilmPreset::builtin("tmax100").unwrap();
        let modelled: Vec<i32> = t100.developments().iter().filter(|d| d.1.contains("modelled")).map(|d| d.0).collect();
        assert_eq!(modelled, [-1]);
        assert!(FilmPreset::builtin("kodachrome64").unwrap().developments().is_empty());
    }
}
