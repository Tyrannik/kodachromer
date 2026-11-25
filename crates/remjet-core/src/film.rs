//! Film emulation: scene-linear working space -> developed film -> scanned
//! (or projected, or printed) display-linear sRGB.
//!
//! Two engines share the exposure, grain, halation and scanner stages:
//! - the *matrix* model below (Kodachrome 64), with a hand-set sensitivity
//!   matrix and a dye cross-talk matrix;
//! - the *spectral* model in [`crate::spectral`] (Ektachrome E100, Portra
//!   400), derived wholly from a datasheet's spectral data.
//!
//! Matrix pipeline per pixel:
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
use serde::{Deserialize, Serialize};

use crate::color::{self, Mat3};
use crate::develop::LinearImage;
use crate::spectral::{self, Datasheet, PrintStock, Process};

// ---------------------------------------------------------------- preset data

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FilmPreset {
    pub id: String,
    pub name: String,
    pub version: u32,
    /// Spectral model process. Absent: the matrix model (`curves`, `dyes`,
    /// `look.sensitivity` and `exposure.grey_density` are then required).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<Process>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curves: Option<Curves>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dyes: Option<Dyes>,
    pub exposure: Exposure,
    pub look: Look,
    pub scan: Scan,
    pub grain: Grain,
    #[serde(default)]
    pub halation: Halation,
    #[serde(default)]
    pub projection: Projection,
    /// Measured data for the spectral model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datasheet: Option<Datasheet>,
    /// Print paper for a colour negative.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub print: Option<PrintStock>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Curves {
    pub log_exposure_r: Vec<f64>,
    pub density_r: Vec<f64>,
    pub log_exposure_g: Vec<f64>,
    pub density_g: Vec<f64>,
    pub log_exposure_b: Vec<f64>,
    pub density_b: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dyes {
    pub matrix: Mat3,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Exposure {
    /// Scene-linear camera value treated as an 18% grey card.
    pub grey_linear: f64,
    /// Matrix model: green density the grey card produces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grey_density: Option<f64>,
    /// Spectral model: log10 exposure (lux-seconds) of the grey card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_h_ref: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Look {
    /// Matrix model only: layer exposure from linear Rec.2020. The spectral
    /// model derives this from the datasheet's sensitivity curves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sensitivity: Option<Mat3>,
    pub interimage: f64,
    /// Scales log exposure around mid-grey: >1 steeper (more contrast).
    #[serde(default = "one")]
    pub contrast: f64,
    /// Veiling flare added to every layer's exposure, as a fraction of the
    /// mid-grey exposure. Physically real (lens + film) and it keeps
    /// near-black sensor noise from being blown up by the logarithmic film
    /// response into coloured speckles.
    #[serde(default = "default_flare")]
    pub flare: f64,
}

fn default_flare() -> f64 {
    0.004
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Grain {
    pub rms_granularity: f64,
    pub correlation: f64,
    pub blur_px: f64,
    pub amount: f64,
}

/// Halation: light that passes through the emulsion, reflects off the film
/// base and re-exposes the bottom (red-sensitive) layer, giving a warm glow
/// around strong highlights. Its strength is set per image.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Halation {
    /// Spread of the glow (Gaussian sigma) in pixels at full M9 resolution
    /// (6.9 µm pixels); other sensors get the same spread in micrometres.
    pub radius_px: f64,
    /// Highlights brighter than this many stops over mid-grey produce glow.
    pub threshold_stops: f64,
    /// How much of the reflected light reaches the red, green and blue layers.
    pub tint: [f64; 3],
}

impl Default for Halation {
    fn default() -> Self {
        Self { radius_px: 40.0, threshold_stops: 1.75, tint: [1.0, 0.22, 0.04] }
    }
}

pub const KODACHROME_64: &str = include_str!("../../../presets/kodachrome64.toml");
pub const EKTACHROME_E100: &str = include_str!("../../../presets/ektachrome-e100.toml");
pub const PORTRA_400: &str = include_str!("../../../presets/portra400.toml");

impl FilmPreset {
    pub fn from_toml(text: &str) -> Result<Self> {
        let p: Self = toml::from_str(text).context("parsing film preset")?;
        if let Some(process) = p.process {
            let ds = p.datasheet.as_ref().context("a spectral preset needs [datasheet]")?;
            ds.validate(process, p.print.as_ref())?;
            p.exposure.log_h_ref.context("a spectral preset needs exposure.log_h_ref")?;
            return Ok(p);
        }
        let c = p.curves.as_ref().context("preset needs [curves] (or a process and [datasheet])")?;
        let dyes = p.dyes.as_ref().context("preset needs [dyes]")?;
        p.look.sensitivity.context("preset needs look.sensitivity")?;
        p.exposure.grey_density.context("preset needs exposure.grey_density")?;
        for (x, y, ch) in [
            (&c.log_exposure_r, &c.density_r, "r"),
            (&c.log_exposure_g, &c.density_g, "g"),
            (&c.log_exposure_b, &c.density_b, "b"),
        ] {
            if x.len() != y.len() || x.len() < 2 {
                bail!("curve {ch}: log_exposure and density must have the same length (>= 2)");
            }
            if x.windows(2).any(|w| w[1] <= w[0]) {
                bail!("curve {ch}: log_exposure must be strictly increasing");
            }
        }
        color::inverse(&dyes.matrix).context("dye matrix is singular")?;
        Ok(p)
    }

    pub fn builtin(id: &str) -> Result<Self> {
        match id {
            "kodachrome64" => Self::from_toml(KODACHROME_64),
            "ektachrome-e100" => Self::from_toml(EKTACHROME_E100),
            "portra400" => Self::from_toml(PORTRA_400),
            _ => bail!("unknown built-in preset '{id}' (available: kodachrome64, ektachrome-e100, portra400)"),
        }
    }

    pub fn to_toml(&self) -> Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// True for colour negatives, which are always viewed as a print.
    pub fn is_negative(&self) -> bool {
        self.process == Some(Process::Negative)
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
    /// Multiplier on grain clump size (1 = preset). Larger = coarser grain
    /// that stays visible in downsized images.
    pub grain_size: f64,
    /// Seed so grain is stable across re-renders of the same image.
    pub seed: u64,
    /// Size of one output pixel relative to a full-resolution sensor pixel
    /// (2.0 for half-size previews), so grain looks the same at any size.
    pub pixel_scale: f64,
    /// Pixel pitch of the camera's sensor in micrometres. Grain, sharpness
    /// and halation are physical sizes on the film and are converted to
    /// pixels with this. Defaults to the M9 the looks were tuned on.
    pub sensor_pixel_um: f64,
    /// Highlight shoulder, -1 (hold back / recover) .. +1 (open up).
    pub highlights: f64,
    /// Halation strength, 0 (off) .. 1 (strong).
    pub halation: f64,
    /// Scanned or projected slide.
    pub output: OutputMode,
}

impl Default for FilmParams {
    fn default() -> Self {
        Self {
            exposure_ev: 0.0,
            grain: 1.0,
            grain_size: 1.0,
            seed: 0,
            pixel_scale: 1.0,
            sensor_pixel_um: M9_PIXEL_UM,
            highlights: 0.0,
            halation: 0.0,
            output: OutputMode::Scan,
        }
    }
}

/// Where an image sits inside the full frame, so grain is generated from
/// absolute frame coordinates: a crop rendered on its own gets exactly the
/// same grain as the same pixels in a full render.
#[derive(Debug, Clone, Copy)]
pub struct GrainFrame {
    pub origin: (usize, usize),
    /// Width of the full frame (the noise row stride).
    pub stride: usize,
    /// Height of the full frame (for effects that depend on frame position,
    /// such as projector light falloff).
    pub height: usize,
}

/// How the finished slide is viewed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputMode {
    /// Read by a film scanner: open shadows, scanner black point and balance.
    #[default]
    Scan,
    /// Projected onto a screen: the slide's full density range, deep blacks
    /// limited only by screen/room flare, lamp colour and corner falloff.
    Projection,
}

/// Viewing conditions for [`OutputMode::Projection`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Projection {
    /// Screen and room flare as a fraction of the brightest white; sets how
    /// black the deepest shadows can get (0.003 ≈ 330:1 on-screen contrast).
    pub flare: f64,
    /// Colour of the projected light after the eye's adaptation (R, G, B):
    /// tungsten lamps stay a little warm.
    pub white_balance: [f64; 3],
    /// Light falloff from the centre to the corners of the frame, in stops.
    pub falloff_stops: f64,
}

impl Default for Projection {
    fn default() -> Self {
        Self { flare: 0.003, white_balance: [1.04, 1.0, 0.90], falloff_stops: 0.35 }
    }
}

/// Highlight knee: one stop above mid-grey, in log10 exposure.
const HIGHLIGHT_KNEE: f32 = 0.30;

use crate::sensor::M9_PIXEL_UM;
/// Aperture used for the datasheet granularity measurement.
const GRANULARITY_APERTURE_UM: f64 = 48.0;

pub struct FilmModel {
    to_layers: [[f32; 3]; 3],
    log_offset: f32,
    grey_log_e: f32,
    contrast: f32,
    /// Flare pedestal added to layer exposures (input units).
    flare: f32,
    /// Slope of log exposure above the highlight knee (1 = unchanged).
    highlight_slope: f32,
    interimage: f32,
    engine: Engine,
    /// Per-layer spread from the datasheet MTF (sigma px, k, wide sigma px).
    spread: Option<[(f32, f32, f32); 3]>,
    /// Per channel (offset, scale) applied to integral density by the scanner.
    balance: [(f32, f32); 3],
    t_white: f32,
    t_black: f32,
    inv_gamma: f32,
    scan_wb: [f32; 3],
    grain_sigma: f32,
    /// Halation: gain, blur sigma in output pixels, threshold (input units), tint.
    hal_gain: f32,
    hal_sigma: f32,
    hal_threshold: f32,
    hal_tint: [f32; 3],
    output: OutputMode,
    proj_flare: f32,
    proj_wb: [f32; 3],
    proj_falloff: f32,
    grain_corr: f32,
    grain_blur: f32,
}

const LOG_E_MIN: f64 = -4.0;
const LOG_E_MAX: f64 = 2.0;

/// How developed film is computed from per-layer log exposure.
enum Engine {
    /// Analytical density per layer, then the dye cross-talk matrix; the
    /// state is the integral (scanner) density itself.
    Matrix { analytical: [Lut; 3], dyes: [[f32; 3]; 3] },
    /// Datasheet-derived: the state is dye amounts (reversal) or Status M
    /// density (negative); [`spectral::Core::view`] turns it into colour.
    Spectral { develop: [Lut; 3], core: std::sync::Arc<spectral::Core> },
}

impl Engine {
    /// Developed state for per-layer log exposures. Grain is added to this.
    fn develop(&self, log_e: [f32; 3]) -> [f32; 3] {
        match self {
            Self::Matrix { analytical, dyes: d } => {
                let a = [0, 1, 2].map(|i| analytical[i].eval(log_e[i]));
                [0, 1, 2].map(|i| d[i][0] * a[0] + d[i][1] * a[1] + d[i][2] * a[2])
            }
            Self::Spectral { develop, .. } => [0, 1, 2].map(|i| develop[i].eval(log_e[i])),
        }
    }

    /// Scanner-channel density (R, G, B) of a developed state.
    fn finish(&self, s: [f32; 3]) -> [f32; 3] {
        match self {
            Self::Matrix { .. } => s,
            Self::Spectral { core, .. } => core.view(s).map(|v| -v.max(1e-6).log10()),
        }
    }
}

impl FilmModel {
    pub fn new(p: &FilmPreset, params: &FilmParams) -> Result<Self> {
        let ev = params.exposure_ev * 2f64.log10();
        let mut output = params.output;
        let mut spread = None;
        let (engine, to_layers, grey_log_e, log_offset) = if let Some(process) = p.process {
            let ds = p.datasheet.as_ref().context("a spectral preset needs [datasheet]")?;
            let log_h_ref = p.exposure.log_h_ref.context("a spectral preset needs exposure.log_h_ref")?;
            let core = spectral::Core::get(process, ds, p.print.as_ref(), log_h_ref)?;
            let develop = [0, 1, 2].map(|ch| Lut::from_fn(LOG_E_MIN, LOG_E_MAX, |x| core.develop.eval(ch, x)));
            if process == Process::Negative {
                // A print is looked at, not projected.
                output = OutputMode::Scan;
            }
            // Film sharpness is a size on the film: convert with this camera's pitch.
            let px = sensor_pitch_um(params) * params.pixel_scale.max(0.1);
            spread = core.spread.map(|s| s.map(|l| ((l.sigma_um / px) as f32, l.k as f32, (l.wide_um / px) as f32)));
            // The grey card lands on the datasheet's reference exposure.
            let offset = log_h_ref - p.exposure.grey_linear.log10() + ev;
            let to_layers = core.to_layers;
            (Engine::Spectral { develop, core }, to_layers, log_h_ref, offset)
        } else {
            let c = p.curves.as_ref().context("preset needs [curves]")?;
            let dyes = p.dyes.as_ref().context("preset needs [dyes]")?;
            let integral = |ch: usize, x: f64| match ch {
                0 => interp(&c.log_exposure_r, &c.density_r, x),
                1 => interp(&c.log_exposure_g, &c.density_g, x),
                _ => interp(&c.log_exposure_b, &c.density_b, x),
            };

            // Analytical dye curves: a(logE) = A^-1 * D_integral(logE) for neutrals.
            let a_inv = color::inverse(&dyes.matrix).context("dye matrix is singular")?;
            let analytical = [0, 1, 2].map(|dye| {
                Lut::from_fn(LOG_E_MIN, LOG_E_MAX, |x| {
                    let d = [integral(0, x), integral(1, x), integral(2, x)];
                    color::mul_vec(&a_inv, &d)[dye]
                })
            });

            // Exposure: grey_linear lands where green reaches grey_density.
            let grey_density = p.exposure.grey_density.context("preset needs exposure.grey_density")?;
            let ge = &c.log_exposure_g;
            let grey_log_e = find_log_e(|x| integral(1, x), grey_density, ge[0], ge[ge.len() - 1]);
            let log_offset = grey_log_e - p.exposure.grey_linear.log10() + ev;

            // Working (ProPhoto D50) -> linear Rec.2020 (D65) -> layer exposures.
            let to_rec2020 = color::mul(
                &color::XYZ_D65_TO_REC2020,
                &color::mul(
                    &color::bradford(color::XY_D50, color::XY_D65),
                    &color::inverse(&color::XYZ_D50_TO_PROPHOTO).expect("invertible"),
                ),
            );
            let sensitivity = p.look.sensitivity.context("preset needs look.sensitivity")?;
            let to_layers = color::mul(&sensitivity, &to_rec2020);
            let engine = Engine::Matrix { analytical, dyes: dyes.matrix.map(|r| r.map(|v| v as f32)) };
            (engine, to_layers, grey_log_e, log_offset)
        };

        // Scanner: base (Dmin) is white; grey-balance channels at the grey density.
        let read = |x: f64| engine.finish(engine.develop([x as f32; 3])).map(|v| v as f64);
        let dmin = read(LOG_E_MAX);
        let d_grey = read(grey_log_e);
        let balance = [0, 1, 2].map(|ch| {
            let full = (d_grey[1] - dmin[1]) / (d_grey[ch] - dmin[ch]);
            let scale = 1.0 + p.scan.neutral_balance * (full - 1.0);
            // D' = dmin_g + (D - dmin_ch) * scale  ==  D * scale + offset
            ((dmin[1] - dmin[ch] * scale) as f32, scale as f32)
        });

        // Grain: per-pixel density std at full resolution, then reduced for
        // downscaled output by how much box-averaging f x f pixels shrinks
        // noise whose correlation area is ~4*pi*blur^2 (clumpier grain
        // survives downscaling better, as it does visually).
        //
        // Pixel sizes in presets are at M9 resolution; `k` sensor pixels
        // cover the same distance on the film as one M9 pixel. The film's
        // grain field is fixed in micrometres, so per-pixel std follows
        // Selwyn's law (std x sqrt(area) constant) over whichever is larger,
        // the pixel or the grain's own correlation length: finer pixels show
        // more noise per pixel until they resolve the clumps themselves.
        let pixel_um = sensor_pitch_um(params);
        let k = M9_PIXEL_UM / pixel_um;
        let blur_m9 = p.grain.blur_px * params.grain_size.clamp(0.25, 6.0);
        let full_blur = blur_m9 * k;
        let clump_um = (4.0 * std::f64::consts::PI).sqrt() * blur_m9 * M9_PIXEL_UM;
        let sigma_full = p.grain.rms_granularity / 1000.0 * (GRANULARITY_APERTURE_UM / M9_PIXEL_UM)
            * (M9_PIXEL_UM.max(clump_um) / pixel_um.max(clump_um))
            * p.grain.amount
            * params.grain;
        let f = params.pixel_scale.max(0.1);
        let corr_len = (4.0 * std::f64::consts::PI * full_blur * full_blur).max(1.0).sqrt();
        let grain_sigma = if f <= 1.0 { sigma_full } else { sigma_full * (corr_len / f).min(1.0) };

        Ok(Self {
            to_layers: to_layers.map(|r| r.map(|v| v as f32)),
            log_offset: log_offset as f32,
            grey_log_e: grey_log_e as f32,
            contrast: p.look.contrast.clamp(0.3, 3.0) as f32,
            // Expressed relative to the exposure that lands on mid-grey.
            flare: (p.look.flare.max(0.0) * 10f64.powf(grey_log_e - log_offset)) as f32,
            // -1 compresses everything above the knee to ~40% (about 2.5x more
            // scene range before white); +1 expands it to blow out sooner.
            highlight_slope: 2f32.powf(params.highlights.clamp(-1.0, 1.0) as f32 * 1.3),
            interimage: p.look.interimage as f32,
            engine,
            spread,
            balance,
            t_white: 10f32.powf(-dmin[1] as f32),
            t_black: 10f32.powf(-p.scan.black_density as f32),
            inv_gamma: (1.0 / p.scan.gamma.max(0.1)) as f32,
            scan_wb: p.scan.white_balance.map(|v| v as f32),
            grain_sigma: grain_sigma as f32,
            hal_gain: (params.halation.clamp(0.0, 2.0) * HALATION_GAIN) as f32,
            hal_sigma: (p.halation.radius_px.max(0.5) * k / f) as f32,
            hal_threshold: (10f64.powf(grey_log_e - log_offset) * 2f64.powf(p.halation.threshold_stops)) as f32,
            hal_tint: p.halation.tint.map(|v| v as f32),
            output,
            proj_flare: p.projection.flare.max(0.0) as f32,
            proj_wb: p.projection.white_balance.map(|v| v as f32),
            proj_falloff: p.projection.falloff_stops.max(0.0) as f32,
            grain_corr: p.grain.correlation.clamp(0.0, 1.0) as f32,
            grain_blur: (full_blur / f) as f32,
        })
    }

    /// Integral (scanner-channel) densities for one working-space pixel.
    pub fn densities(&self, rgb: [f32; 3]) -> [f32; 3] {
        self.engine.finish(self.state_from_exposure(self.layer_exposure(rgb)))
    }

    /// Linear exposure of the red-, green- and blue-sensitive layers.
    fn layer_exposure(&self, rgb: [f32; 3]) -> [f32; 3] {
        let m = &self.to_layers;
        [0, 1, 2].map(|i| m[i][0] * rgb[0] + m[i][1] * rgb[1] + m[i][2] * rgb[2])
    }

    /// Developed film (before grain) for linear layer exposures.
    fn state_from_exposure(&self, exposure: [f32; 3]) -> [f32; 3] {
        let log_e = [0, 1, 2].map(|i| {
            let e = exposure[i];
            // Flare pedestal, then a floor well below the film's maximum
            // density (the curve is flat there, so nothing visible changes).
            let le = (e.max(0.0) + self.flare).log10() + self.log_offset;
            let rel = (le - self.grey_log_e).max(-3.0) * self.contrast;
            // Highlight shoulder: compress (or expand) exposure above the knee.
            let rel = if rel > HIGHLIGHT_KNEE { HIGHLIGHT_KNEE + (rel - HIGHLIGHT_KNEE) * self.highlight_slope } else { rel };
            self.grey_log_e + rel
        });
        let mean = (log_e[0] + log_e[1] + log_e[2]) / 3.0;
        // Interimage push, limited so one starved layer can't swing the others.
        self.engine.develop([0, 1, 2].map(|i| log_e[i] + self.interimage * (log_e[i] - mean).clamp(-1.0, 1.0)))
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

    /// Projection: integral density -> light on the screen, display-linear.
    /// White is the lamp through clear film; the deepest black is set only by
    /// screen/room flare. `light` is the lamp's relative brightness at this
    /// point of the frame (corner falloff).
    pub fn project(&self, d: [f32; 3], light: f32) -> [f32; 3] {
        [0, 1, 2].map(|i| {
            let (off, scale) = self.balance[i];
            let t = 10f32.powf(-(d[i] * scale + off)) / self.t_white;
            ((t * light + self.proj_flare) / (1.0 + self.proj_flare) * self.proj_wb[i]).clamp(0.0, 1.0)
        })
    }

    /// Relative lamp brightness at a frame position (0..1 each axis):
    /// a smooth radial falloff reaching `-falloff_stops` in the corners.
    fn projector_light(&self, u: f32, v: f32) -> f32 {
        let (x, y) = (2.0 * u - 1.0, 2.0 * v - 1.0);
        2f32.powf(-self.proj_falloff * (x * x + y * y) * 0.5)
    }

    /// Run the full model over an image. Output pixels are display-linear sRGB.
    pub fn apply(&self, img: &LinearImage, seed: u64) -> LinearImage {
        self.apply_framed(img, seed, GrainFrame { origin: (0, 0), stride: img.width, height: img.height })
    }

    /// As [`apply`], for an image that is a crop of a larger frame.
    pub fn apply_framed(&self, img: &LinearImage, seed: u64, frame: GrainFrame) -> LinearImage {
        let mut exposure: Vec<[f32; 3]> = img.pixels.par_iter().map(|&p| self.layer_exposure(p)).collect();
        if let Some(spread) = &self.spread {
            apply_spread(&mut exposure, img.width, img.height, spread);
        }
        if self.hal_gain > 0.0 {
            self.add_halation(&mut exposure, img.width, img.height);
        }
        let mut dens: Vec<[f32; 3]> = exposure.par_iter().map(|&e| self.state_from_exposure(e)).collect();
        drop(exposure);
        if self.grain_sigma > 0.0 {
            add_grain(&mut dens, img.width, img.height, self, seed, frame);
        }
        if matches!(self.engine, Engine::Spectral { .. }) {
            dens.par_iter_mut().for_each(|d| *d = self.engine.finish(*d));
        }
        let pixels = match self.output {
            OutputMode::Scan => dens.par_iter().map(|&d| self.scan(d)).collect(),
            OutputMode::Projection => {
                let (fw, fh) = (frame.stride.max(1) as f32, frame.height.max(1) as f32);
                dens.par_iter()
                    .enumerate()
                    .map(|(i, &d)| {
                        let (x, y) = (i % img.width, i / img.width);
                        let u = (frame.origin.0 + x) as f32 / fw;
                        let v = (frame.origin.1 + y) as f32 / fh;
                        self.project(d, self.projector_light(u, v))
                    })
                    .collect()
            }
        };
        LinearImage { width: img.width, height: img.height, pixels, cct: img.cct }
    }

    /// Add halation to layer exposures: light above the threshold is spread
    /// by a wide Gaussian (computed on a reduced grid, since the glow is
    /// smooth) and added back, mostly to the red layer.
    fn add_halation(&self, exposure: &mut [[f32; 3]], w: usize, h: usize) {
        let t = self.hal_threshold;
        // Long wavelengths penetrate the emulsion best, so weight towards red.
        let source = |e: &[f32; 3]| (0.6 * e[0] + 0.3 * e[1] + 0.1 * e[2] - t).max(0.0);
        let d = ((self.hal_sigma / 3.0).floor() as usize).max(1);
        let (sw, sh) = (w.div_ceil(d), h.div_ceil(d));
        let mut small: Vec<f32> = (0..sw * sh)
            .into_par_iter()
            .map(|i| {
                let (sx, sy) = (i % sw, i / sw);
                let (mut sum, mut n) = (0.0f32, 0u32);
                for y in sy * d..((sy + 1) * d).min(h) {
                    for x in sx * d..((sx + 1) * d).min(w) {
                        sum += source(&exposure[y * w + x]);
                        n += 1;
                    }
                }
                sum / n.max(1) as f32
            })
            .collect();
        if small.iter().all(|v| *v == 0.0) {
            return;
        }
        blur(&mut small, sw, sh, self.hal_sigma / d as f32);
        let gain = self.hal_tint.map(|c| c * self.hal_gain);
        exposure.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            // Bilinear sample of the blurred grid at this pixel's centre.
            let fy = ((y as f32 + 0.5) / d as f32 - 0.5).clamp(0.0, (sh - 1) as f32);
            let (y0, ty) = (fy.floor() as usize, fy.fract());
            let y1 = (y0 + 1).min(sh - 1);
            for (x, e) in row.iter_mut().enumerate() {
                let fx = ((x as f32 + 0.5) / d as f32 - 0.5).clamp(0.0, (sw - 1) as f32);
                let (x0, tx) = (fx.floor() as usize, fx.fract());
                let x1 = (x0 + 1).min(sw - 1);
                let top = small[y0 * sw + x0] * (1.0 - tx) + small[y0 * sw + x1] * tx;
                let bottom = small[y1 * sw + x0] * (1.0 - tx) + small[y1 * sw + x1] * tx;
                let glow = top * (1.0 - ty) + bottom * ty;
                for c in 0..3 {
                    e[c] += gain[c] * glow;
                }
            }
        });
    }
}

/// Film sharpness: blur each layer's exposure by its fitted spread, with the
/// adjacency term `(1 + k) * G(sigma) - k * G(wide)` where the MTF rises
/// above 100%.
fn apply_spread(exposure: &mut [[f32; 3]], w: usize, h: usize, spread: &[(f32, f32, f32); 3]) {
    for (c, &(sigma, k, wide)) in spread.iter().enumerate() {
        let plane: Vec<f32> = exposure.par_iter().map(|e| e[c]).collect();
        let mut sharp = plane.clone();
        blur(&mut sharp, w, h, sigma);
        if k > 0.0 {
            let broad = smooth_blur(&plane, w, h, wide);
            sharp.par_iter_mut().zip(broad.par_iter()).for_each(|(s, b)| *s = (1.0 + k) * *s - k * b);
        }
        exposure.par_iter_mut().zip(sharp.par_iter()).for_each(|(e, v)| e[c] = v.max(0.0));
    }
}

/// Gaussian blur for wide sigmas, computed on a grid reduced by about
/// sigma / 3 and sampled back bilinearly (the result is smooth anyway).
fn smooth_blur(plane: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    let d = ((sigma / 3.0).floor() as usize).max(1);
    if d == 1 {
        let mut out = plane.to_vec();
        blur(&mut out, w, h, sigma);
        return out;
    }
    let (sw, sh) = (w.div_ceil(d), h.div_ceil(d));
    let mut small: Vec<f32> = (0..sw * sh)
        .into_par_iter()
        .map(|i| {
            let (sx, sy) = (i % sw, i / sw);
            let (mut sum, mut n) = (0.0f32, 0u32);
            for y in sy * d..((sy + 1) * d).min(h) {
                for x in sx * d..((sx + 1) * d).min(w) {
                    sum += plane[y * w + x];
                    n += 1;
                }
            }
            sum / n.max(1) as f32
        })
        .collect();
    blur(&mut small, sw, sh, sigma / d as f32);
    let mut out = vec![0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let fy = ((y as f32 + 0.5) / d as f32 - 0.5).clamp(0.0, (sh - 1) as f32);
        let (y0, ty) = (fy.floor() as usize, fy.fract());
        let y1 = (y0 + 1).min(sh - 1);
        for (x, o) in row.iter_mut().enumerate() {
            let fx = ((x as f32 + 0.5) / d as f32 - 0.5).clamp(0.0, (sw - 1) as f32);
            let (x0, tx) = (fx.floor() as usize, fx.fract());
            let x1 = (x0 + 1).min(sw - 1);
            let top = small[y0 * sw + x0] * (1.0 - tx) + small[y0 * sw + x1] * tx;
            let bottom = small[y1 * sw + x0] * (1.0 - tx) + small[y1 * sw + x1] * tx;
            *o = top * (1.0 - ty) + bottom * ty;
        }
    });
    out
}

/// Halation exposure gain at strength 1.0.
const HALATION_GAIN: f64 = 0.8;

/// The camera's pixel pitch, falling back to the M9 for nonsense values.
fn sensor_pitch_um(params: &FilmParams) -> f64 {
    if params.sensor_pixel_um.is_finite() && params.sensor_pixel_um > 0.0 { params.sensor_pixel_um } else { M9_PIXEL_UM }
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

/// Approximately normal noise (unit variance) from one hash: the sum of four
/// 16-bit uniforms (Irwin–Hall). Much cheaper than Box–Muller and
/// indistinguishable once blurred into grain.
fn gaussian(seed: u64, i: u64) -> f32 {
    let h = splitmix(seed ^ i.wrapping_mul(0xD6E8_FEB8_6659_FD93));
    let sum = (h & 0xFFFF) + ((h >> 16) & 0xFFFF) + ((h >> 32) & 0xFFFF) + (h >> 48);
    (sum as f32 / 65535.0 - 2.0) * 3f32.sqrt()
}

/// One grain field for an image placed at `frame` within the full frame.
fn noise_field(seed: u64, w: usize, h: usize, frame: GrainFrame) -> Vec<f32> {
    (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let gi = (frame.origin.1 + y) * frame.stride + frame.origin.0 + x;
            gaussian(seed, gi as u64)
        })
        .collect()
}

/// Blurred, unit-variance grain with a clumpy (dye-cloud) character: large
/// excursions are emphasised so grain reads as irregular clusters rather
/// than even digital noise.
fn grain_field(seed: u64, w: usize, h: usize, blur_sigma: f32, frame: GrainFrame) -> Vec<f32> {
    let mut f = noise_field(seed, w, h, frame);
    let gain = blur(&mut f, w, h, blur_sigma);
    // v * (a + b|v|) has std sqrt(a^2 + 2ab*E|v|^3 + 3b^2) for unit normal v.
    let (a, b) = (0.6f32, 0.4f32);
    let norm = 1.0 / (a * a + 2.0 * a * b * 1.5958 + 3.0 * b * b).sqrt();
    f.par_iter_mut().for_each(|v| {
        let n = *v / gain;
        *v = n * (a + b * n.abs()) * norm;
    });
    f
}

/// Separable Gaussian blur of a single-channel field, returning the factor by
/// which it reduced the standard deviation of white noise.
pub(crate) fn blur(field: &mut [f32], w: usize, h: usize, sigma: f32) -> f32 {
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

fn add_grain(dens: &mut [[f32; 3]], w: usize, h: usize, model: &FilmModel, seed: u64, frame: GrainFrame) {
    let shared_w = model.grain_corr.sqrt();
    let own_w = (1.0 - model.grain_corr).sqrt();
    let sigma = model.grain_sigma;
    // Granularity is specified at density 1.0 and grows with density. Work
    // one field at a time to keep memory to a single extra plane.
    let scale = |d: f32| sigma * d.max(0.05).sqrt();
    let shared = grain_field(splitmix(seed.wrapping_add(3)), w, h, model.grain_blur, frame);
    dens.par_iter_mut().zip(shared.par_iter()).for_each(|(d, n)| {
        let base = [scale(d[0]), scale(d[1]), scale(d[2])];
        for c in 0..3 {
            d[c] += base[c] * shared_w * n;
        }
    });
    drop(shared);
    if own_w > 0.0 {
        for c in 0..3 {
            let own = grain_field(splitmix(seed.wrapping_add(c as u64)), w, h, model.grain_blur, frame);
            dens.par_iter_mut().zip(own.par_iter()).for_each(|(d, n)| d[c] += scale(d[c]) * own_w * n);
        }
    }
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
    fn m9_pitch_is_the_default() {
        let p = FilmPreset::builtin("kodachrome64").unwrap();
        let a = FilmModel::new(&p, &FilmParams::default()).unwrap();
        let b = FilmModel::new(&p, &FilmParams { sensor_pixel_um: M9_PIXEL_UM, ..Default::default() }).unwrap();
        assert_eq!((a.grain_sigma, a.grain_blur, a.hal_sigma), (b.grain_sigma, b.grain_blur, b.hal_sigma));
        // Nonsense pitches fall back to the M9 rather than breaking the render.
        for bad in [0.0, -3.0, f64::NAN] {
            let c = FilmModel::new(&p, &FilmParams { sensor_pixel_um: bad, ..Default::default() }).unwrap();
            assert_eq!((a.grain_sigma, a.grain_blur, a.hal_sigma), (c.grain_sigma, c.grain_blur, c.hal_sigma));
        }
    }

    #[test]
    fn film_effects_keep_their_physical_size_on_other_sensors() {
        let p = FilmPreset::builtin("kodachrome64").unwrap();
        let m9 = FilmModel::new(&p, &FilmParams::default()).unwrap();
        // A 45 MP full-frame sensor: 4.39 µm pixels, ~1.57x finer than the M9.
        let pitch = 36_000.0 / 8192.0;
        let k = (M9_PIXEL_UM / pitch) as f32;
        let fine = FilmModel::new(&p, &FilmParams { sensor_pixel_um: pitch, ..Default::default() }).unwrap();
        // Grain clumps and halation spread cover the same micrometres, so
        // more pixels.
        assert!((fine.grain_blur / m9.grain_blur - k).abs() < 1e-4);
        assert!((fine.hal_sigma / m9.hal_sigma - k).abs() < 1e-4);
        // Kodachrome's clumps (~27 µm) are bigger than both pixels, so the
        // per-pixel grain strength is unchanged.
        assert!((fine.grain_sigma - m9.grain_sigma).abs() < 1e-6);

        // Pixels coarser than the clumps average grain away (Selwyn's law).
        let coarse = FilmModel::new(&p, &FilmParams { sensor_pixel_um: 54.0, ..Default::default() }).unwrap();
        assert!(coarse.grain_sigma < m9.grain_sigma * 0.6, "{} vs {}", coarse.grain_sigma, m9.grain_sigma);
    }

    #[test]
    fn datasheet_sharpness_keeps_its_physical_size_on_other_sensors() {
        let pitch = 36_000.0 / 8192.0;
        let k = (M9_PIXEL_UM / pitch) as f32;
        for id in ["ektachrome-e100", "portra400"] {
            let p = FilmPreset::builtin(id).unwrap();
            let m9 = FilmModel::new(&p, &FilmParams::default()).unwrap();
            let fine = FilmModel::new(&p, &FilmParams { sensor_pixel_um: pitch, ..Default::default() }).unwrap();
            let (a, b) = (m9.spread.unwrap(), fine.spread.unwrap());
            for c in 0..3 {
                assert!((b[c].0 / a[c].0 - k).abs() < 1e-4 && (b[c].2 / a[c].2 - k).abs() < 1e-4, "{id}: {a:?} vs {b:?}");
                assert_eq!(a[c].1, b[c].1);
            }
            assert!((fine.hal_sigma / m9.hal_sigma - k).abs() < 1e-4);
            assert!((fine.grain_blur / m9.grain_blur - k).abs() < 1e-4);
        }
    }

    #[test]
    fn grain_looks_the_same_size_on_a_finer_sensor() {
        // Render a flat grey on the M9 and on a sensor with half the pitch,
        // then compare grain over the same physical area: the fine render
        // box-averaged 2x2 should have about the M9's grain std.
        let p = FilmPreset::builtin("kodachrome64").unwrap();
        let std_of = |pixels: &[[f32; 3]]| {
            let g: Vec<f32> = pixels.iter().map(|p| p[1]).collect();
            let mean = g.iter().sum::<f32>() / g.len() as f32;
            (g.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / g.len() as f32).sqrt()
        };
        let flat = |n: usize| LinearImage { width: n, height: n, pixels: vec![[0.18; 3]; n * n], cct: 5000.0 };
        let m9 = FilmModel::new(&p, &FilmParams { halation: 0.0, ..Default::default() }).unwrap().apply(&flat(256), 7);
        let fine = FilmModel::new(&p, &FilmParams { halation: 0.0, sensor_pixel_um: M9_PIXEL_UM / 2.0, ..Default::default() })
            .unwrap()
            .apply(&flat(512), 7);
        let binned: Vec<[f32; 3]> = (0..256 * 256)
            .map(|i| {
                let (x, y) = (i % 256 * 2, i / 256 * 2);
                let px = |x: usize, y: usize| fine.pixels[y * 512 + x];
                let s = [px(x, y), px(x + 1, y), px(x, y + 1), px(x + 1, y + 1)];
                [0, 1, 2].map(|c| s.iter().map(|p| p[c]).sum::<f32>() / 4.0)
            })
            .collect();
        let (a, b) = (std_of(&m9.pixels), std_of(&binned));
        assert!(a > 0.0);
        assert!((b / a - 1.0).abs() < 0.1, "M9 grain std {a}, finer sensor binned {b}");
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
        assert!((d[1] - p.exposure.grey_density.unwrap() as f32).abs() < 0.03, "{d:?}");
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

    #[test]
    fn cropped_render_has_identical_grain() {
        let (_, m) = model(1.0);
        let (w, h) = (64, 48);
        let pixels: Vec<[f32; 3]> = (0..w * h).map(|i| [0.05 + (i % w) as f32 * 0.003, 0.1, 0.08]).collect();
        let full = m.apply(&LinearImage { width: w, height: h, pixels: pixels.clone(), cct: 5000.0 }, 3);
        // Crop with an apron wider than the grain blur, compare the interior.
        let (x0, y0, cw, ch) = (10, 8, 40, 30);
        let crop: Vec<[f32; 3]> = (0..ch).flat_map(|y| pixels[(y0 + y) * w + x0..(y0 + y) * w + x0 + cw].to_vec()).collect();
        let part = m.apply_framed(&LinearImage { width: cw, height: ch, pixels: crop, cct: 5000.0 }, 3, GrainFrame { origin: (x0, y0), stride: w, height: h });
        let apron = 6;
        for y in apron..ch - apron {
            for x in apron..cw - apron {
                let a = part.pixels[y * cw + x];
                let b = full.pixels[(y0 + y) * w + x0 + x];
                assert!((0..3).all(|c| (a[c] - b[c]).abs() < 1e-5), "mismatch at ({x},{y})");
            }
        }
    }

    /// Regression: in a dark saturated red, the green channel is tiny and
    /// noisy. Noise there must not be blown up into bright speckles.
    #[test]
    fn near_black_channel_noise_is_not_amplified() {
        let (_, m) = model(0.0);
        let outs: Vec<[f32; 3]> = [0.0, 0.0001, 0.0003, 0.0006, -0.0002]
            .iter()
            .map(|&g| m.scan(m.densities([0.03, g, 0.0004])))
            .collect();
        for c in 0..3 {
            let lo = outs.iter().map(|o| o[c]).fold(f32::MAX, f32::min);
            let hi = outs.iter().map(|o| o[c]).fold(f32::MIN, f32::max);
            assert!(hi - lo < 0.02, "channel {c} swings {lo}..{hi}: {outs:?}");
        }
    }

    #[test]
    fn projection_has_deeper_shadows_and_corner_falloff() {
        let p = FilmPreset::builtin("kodachrome64").unwrap();
        let grey = p.exposure.grey_linear as f32;
        let scan = FilmModel::new(&p, &FilmParams { grain: 0.0, ..Default::default() }).unwrap();
        let proj = FilmModel::new(&p, &FilmParams { grain: 0.0, output: OutputMode::Projection, ..Default::default() }).unwrap();
        // Two stops under grey: the projector shows denser shadows than the
        // scanner (no shadow lift), but blacks stay above zero (flare).
        let dark = [grey * 0.25; 3];
        let (s, pr) = (scan.scan(scan.densities(dark)), proj.project(proj.densities(dark), 1.0));
        assert!(pr[1] < s[1], "projection shadows should be deeper: scan {s:?} vs projection {pr:?}");
        let black = proj.project([5.0; 3], 1.0);
        assert!(black.iter().all(|&c| c > 0.0 && c < 0.01), "flare-limited black: {black:?}");
        // Corners are dimmer than the centre.
        assert!(proj.projector_light(0.0, 0.0) < proj.projector_light(0.5, 0.5));
        assert!((proj.projector_light(0.5, 0.5) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn halation_glows_red_around_highlights_only() {
        let p = FilmPreset::builtin("kodachrome64").unwrap();
        // Larger than the glow's reach (3 sigma = 120 px) from centre to corner.
        let (w, h) = (400, 300);
        let grey = p.exposure.grey_linear as f32;
        // Dark background with one very bright square in the middle.
        let pixels: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                if (190..210).contains(&x) && (140..160).contains(&y) { [grey * 8.0; 3] } else { [grey * 0.25; 3] }
            })
            .collect();
        let img = LinearImage { width: w, height: h, pixels, cct: 5000.0 };
        let render = |hal: f64| {
            let m = FilmModel::new(&p, &FilmParams { grain: 0.0, halation: hal, ..Default::default() }).unwrap();
            m.apply(&img, 0)
        };
        let (off, on) = (render(0.0), render(1.0));
        let near = 150 * w + 215; // just right of the bright square
        let far = 5 * w + 5; // far corner
        let (a, b) = (off.pixels[near], on.pixels[near]);
        assert!(b[0] - a[0] > 0.01, "red should glow near the highlight: {a:?} -> {b:?}");
        assert!(b[0] - a[0] > (b[2] - a[2]) * 2.0, "glow should be red-dominant: {a:?} -> {b:?}");
        assert_eq!(off.pixels[far], on.pixels[far], "no glow far from highlights");
    }

    #[test]
    fn highlights_control_holds_back_and_opens_up() {
        let p = FilmPreset::builtin("kodachrome64").unwrap();
        let bright = (p.exposure.grey_linear * 6.0) as f32; // ~2.6 stops over grey
        let out = |h: f64| {
            let m = FilmModel::new(&p, &FilmParams { grain: 0.0, highlights: h, ..Default::default() }).unwrap();
            m.scan(m.densities([bright; 3]))[1]
        };
        assert!(out(-1.0) < out(0.0) - 0.05, "recovery should darken highlights");
        assert!(out(1.0) > out(0.0), "positive should brighten highlights");
        // Mid-grey is untouched.
        let grey = p.exposure.grey_linear as f32;
        let m0 = FilmModel::new(&p, &FilmParams { grain: 0.0, ..Default::default() }).unwrap();
        let m1 = FilmModel::new(&p, &FilmParams { grain: 0.0, highlights: -1.0, ..Default::default() }).unwrap();
        assert_eq!(m0.scan(m0.densities([grey; 3])), m1.scan(m1.densities([grey; 3])));
    }
}
