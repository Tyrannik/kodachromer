//! Raw development: DNG mosaic -> scene-linear image in the working space
//! (linear ProPhoto RGB, D50). No look or tone curve is applied here; this is
//! the neutral, colorimetric starting point for the film model.

use anyhow::{Context, Result, bail};
use rawler::imgop::sensor::Demosaic;
use rawler::imgop::sensor::bayer::ppg::PPGDemosaic;
use rawler::imgop::sensor::bayer::superpixel::Superpixel3Channel;
use rawler::imgop::sensor::xtrans::bilinear::XTransBilinearDemosaic;
use rawler::imgop::sensor::xtrans::markesteijn::XTransMarkesteijnDemosaic;
use rawler::imgop::xyz::Illuminant;
use rawler::imgop::{Dim2, Point, Rect};
use rawler::pixarray::PixF32;
use rawler::rawimage::{RawImage, RawImageData, RawPhotometricInterpretation};
use rayon::prelude::*;

use crate::color::{self, Mat3};

const TAG_BASELINE_EXPOSURE: u16 = 50730;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Quality {
    /// Full resolution, PPG demosaic. For export.
    Full,
    /// Half resolution (one pixel per 2x2 Bayer quad). Fast, for previews.
    Half,
}

#[derive(Debug, Clone)]
pub struct DevelopParams {
    /// Exposure adjustment in stops, on top of the file's BaselineExposure.
    pub exposure_ev: f32,
    /// White balance multipliers (R, G, B). `None` uses the camera's as-shot WB.
    pub wb: Option<[f32; 3]>,
    pub quality: Quality,
}

impl Default for DevelopParams {
    fn default() -> Self {
        Self { exposure_ev: 0.0, wb: None, quality: Quality::Full }
    }
}

/// Scene-linear image in the working space. Values may exceed 1.0.
#[derive(Debug, Clone)]
pub struct LinearImage {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<[f32; 3]>,
    /// Estimated CCT of the white balance used, for display.
    pub cct: f64,
}

/// Camera as-shot white balance multipliers, normalised to green = 1.
pub fn as_shot_wb(raw: &RawImage) -> [f32; 3] {
    let c = raw.wb_coeffs;
    if c[..3].iter().any(|v| !v.is_finite() || *v <= 0.0) {
        return [1.0, 1.0, 1.0];
    }
    [c[0] / c[1], 1.0, c[2] / c[1]]
}

pub fn baseline_exposure(raw: &RawImage) -> f32 {
    raw.dng_tags.get(&TAG_BASELINE_EXPOSURE).map(|v| v.force_f32(0)).unwrap_or(0.0)
}

fn color_matrix(raw: &RawImage, illuminant: Illuminant) -> Option<Mat3> {
    let m = raw.color_matrix.get(&illuminant)?;
    (m.len() == 9).then(|| [0, 1, 2].map(|i| [0, 1, 2].map(|j| m[i * 3 + j] as f64)))
}

/// The two colour matrices the DNG interpolation needs: (A, D65).
///
/// Most files have both. Files without a D65 matrix use the closest daylight
/// one they have (D55, D50, D75 ...), and files with a single matrix use it
/// for both ends, which is what the DNG spec does for single-illuminant
/// profiles.
fn color_matrices(raw: &RawImage) -> Option<(Mat3, Mat3)> {
    use Illuminant::*;
    let daylight = [D65, D55, D75, D50, Daylight, FineWeather, CloudyWeather, Flash, Shade, C];
    let warm = [A, Tungsten, IsoStudioTungsten];
    let any = || {
        let mut keys: Vec<_> = raw.color_matrix.keys().copied().collect();
        keys.sort_by_key(|k| *k as u16);
        keys.into_iter().find_map(|k| color_matrix(raw, k))
    };
    let d65 = daylight.iter().find_map(|&i| color_matrix(raw, i)).or_else(any)?;
    let a = warm.iter().find_map(|&i| color_matrix(raw, i)).unwrap_or(d65);
    Some((a, d65))
}

/// Demosaiced, white-balanced camera RGB plus what's needed to finish the
/// colour transform. Cache this: re-running [`to_working`] with different
/// white-balance tweaks or exposure is cheap, demosaicing is not.
#[derive(Debug, Clone)]
pub struct CameraImage {
    pub width: usize,
    pub height: usize,
    /// Camera RGB after black/white levels and `wb`, clipped to [0, 1].
    pub pixels: Vec<[f32; 3]>,
    /// White balance multipliers already applied to `pixels`.
    pub wb: [f32; 3],
    /// Sensor pixel pitch in micrometres (of the full-resolution sensor, not
    /// of this possibly downscaled copy), for physically sized film effects.
    pub pixel_pitch_um: f64,
    cm_a: Mat3,
    cm_d65: Mat3,
    baseline_ev: f32,
}

pub fn develop(raw: &RawImage, params: &DevelopParams) -> Result<LinearImage> {
    let cam = demosaic(raw, params.wb, params.quality)?;
    to_working(&cam, [1.0; 3], params.exposure_ev)
}

/// Camera RGB -> working space. `wb_adjust` multiplies the applied white
/// balance (for warmth/tint sliders); the colour matrix is solved for the
/// combined white balance so the DNG profile interpolation stays correct.
pub fn to_working(cam: &CameraImage, wb_adjust: [f32; 3], exposure_ev: f32) -> Result<LinearImage> {
    let total = [0, 1, 2].map(|i| (cam.wb[i] * wb_adjust[i]) as f64);
    let (m, cct) = color::camera_to_working(&cam.cm_a, &cam.cm_d65, &total).context("singular colour matrix")?;
    let m = color::mul(&m, &color::diag(&wb_adjust.map(f64::from)));
    let gain = 2f64.powf((cam.baseline_ev + exposure_ev) as f64);
    let m: [[f32; 3]; 3] = m.map(|row| row.map(|v| (v * gain) as f32));
    let pixels = cam
        .pixels
        .par_iter()
        .map(|c| [0, 1, 2].map(|i| m[i][0] * c[0] + m[i][1] * c[1] + m[i][2] * c[2]))
        .collect();
    Ok(LinearImage { width: cam.width, height: cam.height, pixels, cct })
}

impl CameraImage {
    /// Box-downscale by an integer factor so the long edge is <= `max_edge`.
    /// Returns the image and the factor used.
    pub fn downscale_to(&self, max_edge: usize) -> (CameraImage, usize) {
        let f = self.width.max(self.height).div_ceil(max_edge.max(1)).max(1);
        if f == 1 {
            return (self.clone(), 1);
        }
        let (w, h) = (self.width / f, self.height / f);
        let n = (f * f) as f32;
        let pixels = (0..w * h)
            .into_par_iter()
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let mut acc = [0f32; 3];
                for dy in 0..f {
                    let row = (y * f + dy) * self.width;
                    for dx in 0..f {
                        let p = self.pixels[row + x * f + dx];
                        acc = [acc[0] + p[0], acc[1] + p[1], acc[2] + p[2]];
                    }
                }
                acc.map(|v| v / n)
            })
            .collect();
        (CameraImage { width: w, height: h, pixels, ..self.clone_meta() }, f)
    }

    /// Copy out a rectangle (clamped to the image bounds).
    pub fn crop(&self, x: usize, y: usize, w: usize, h: usize) -> CameraImage {
        let x = x.min(self.width.saturating_sub(1));
        let y = y.min(self.height.saturating_sub(1));
        let w = w.min(self.width - x).max(1);
        let h = h.min(self.height - y).max(1);
        let mut pixels = Vec::with_capacity(w * h);
        for row in y..y + h {
            pixels.extend_from_slice(&self.pixels[row * self.width + x..row * self.width + x + w]);
        }
        CameraImage { width: w, height: h, pixels, ..self.clone_meta() }
    }

    fn clone_meta(&self) -> CameraImage {
        CameraImage {
            width: 0,
            height: 0,
            pixels: Vec::new(),
            wb: self.wb,
            pixel_pitch_um: self.pixel_pitch_um,
            cm_a: self.cm_a,
            cm_d65: self.cm_d65,
            baseline_ev: self.baseline_ev,
        }
    }
}

/// Blur colour differences while keeping luminance detail intact.
fn chroma_denoise(pixels: &mut [[f32; 3]], w: usize, h: usize, sigma: f32) {
    let luma = |p: &[f32; 3]| (p[0] + 2.0 * p[1] + p[2]) * 0.25;
    let mut cr: Vec<f32> = pixels.par_iter().map(|p| p[0] - luma(p)).collect();
    let mut cb: Vec<f32> = pixels.par_iter().map(|p| p[2] - luma(p)).collect();
    crate::film::blur(&mut cr, w, h, sigma);
    crate::film::blur(&mut cb, w, h, sigma);
    pixels.par_iter_mut().zip(cr.par_iter().zip(cb.par_iter())).for_each(|(p, (r, b))| {
        let y = luma(p);
        // Solve for G so that luma is preserved: y = (r' + 2g' + b') / 4.
        let (rr, bb) = (y + r, y + b);
        let g = (4.0 * y - rr - bb) * 0.5;
        *p = [rr.max(0.0), g.max(0.0), bb.max(0.0)];
    });
}

/// Unsharp mask on luminance (scene-linear), `amount` 0..1+, radius in px.
pub fn sharpen(img: &mut LinearImage, amount: f32, sigma: f32) {
    if amount <= 0.0 || sigma < 0.3 {
        return;
    }
    let luma = |p: &[f32; 3]| (p[0] + 2.0 * p[1] + p[2]) * 0.25;
    let y: Vec<f32> = img.pixels.par_iter().map(luma).collect();
    let mut yb = y.clone();
    crate::film::blur(&mut yb, img.width, img.height, sigma);
    img.pixels.par_iter_mut().zip(y.par_iter().zip(yb.par_iter())).for_each(|(p, (&y, &b))| {
        if y > 1e-6 {
            // Sharpen in log space so the halo is proportional across tones.
            let f = ((y.ln() - b.max(1e-6).ln()) * amount).clamp(-0.7, 0.7).exp();
            *p = p.map(|c| c * f);
        }
    });
}

/// Chroma noise reduction strength (Gaussian sigma in full-resolution
/// pixels) by sensor type. Colour noise is a per-pixel effect, so this stays in
/// pixels rather than scaling with pitch. The M9's CCD was tuned at 1.5; the
/// X-Trans pattern spreads demosaic colour error over a 6x6 tile, so it gets a
/// little more.
const CHROMA_SIGMA_BAYER: f32 = 1.5;
const CHROMA_SIGMA_XTRANS: f32 = 2.0;

/// Levels, white balance and demosaic. `wb = None` uses the as-shot balance.
///
/// Handles Bayer and Fujifilm X-Trans mosaics, and linear (already
/// demosaiced) DNGs. Monochrome sensors and floating-point data are not
/// supported.
pub fn demosaic(raw: &RawImage, wb: Option<[f32; 3]>, quality: Quality) -> Result<CameraImage> {
    let RawImageData::Integer(data) = &raw.data else {
        bail!("floating-point (HDR) raw data is not supported");
    };
    let (cm_a, cm_d65) = color_matrices(raw).context("raw file has no colour matrix")?;
    let wb = wb.unwrap_or_else(|| as_shot_wb(raw));
    let (w, h) = (raw.width, raw.height);
    let crop = raw.crop_area.unwrap_or(Rect::new(Point::new(0, 0), Dim2::new(w, h)));

    let (mut pixels, dim, chroma_sigma) = match &raw.photometric {
        RawPhotometricInterpretation::Cfa(cfa_config) => {
            let cfa = &cfa_config.cfa;
            if raw.cpp != 1 {
                bail!("expected one sample per pixel, got {}", raw.cpp);
            }
            let xtrans = cfa.width == 6 && cfa.height == 6;
            let bayer = cfa.width == 2 && cfa.height == 2 && cfa.unique_colors() == 3;
            if !(bayer || xtrans) {
                bail!("unsupported colour filter pattern ({}x{}, {} colours)", cfa.width, cfa.height, cfa.unique_colors());
            }
            let black = raw.blacklevel.as_bayer_array();
            let white = raw.whitelevel.as_bayer_array();

            // 1. Black/white levels and white balance on the mosaic. Clipping
            //    every channel at 1.0 keeps blown highlights neutral, not pink.
            let mut mosaic = vec![0f32; w * h];
            mosaic.par_chunks_exact_mut(w).enumerate().for_each(|(row, out)| {
                let src = &data[row * w..(row + 1) * w];
                for (col, (o, &v)) in out.iter_mut().zip(src).enumerate() {
                    let pos = (row % 2) * 2 + col % 2;
                    let lin = (v as f32 - black[pos]) / (white[pos] - black[pos]);
                    *o = (lin.max(0.0) * wb[cfa.color_at(row, col).min(2)]).min(1.0);
                }
            });

            // 2. Demosaic straight into the default crop.
            let mosaic = PixF32::new_with(mosaic, w, h);
            let colors = &cfa_config.colors;
            let rgb = match (xtrans, quality) {
                (false, Quality::Full) => PPGDemosaic::new().demosaic(&mosaic, cfa, colors, crop),
                (false, Quality::Half) => Superpixel3Channel::new().demosaic(&mosaic, cfa, colors, crop),
                (true, Quality::Full) => XTransMarkesteijnDemosaic::new_pass_1().demosaic(&mosaic, cfa, colors, crop),
                (true, Quality::Half) => XTransBilinearDemosaic::new().demosaic(&mosaic, cfa, colors, crop),
            };
            let dim = rgb.dim();
            let (mut pixels, mut dim) = (rgb.into_inner(), (dim.w, dim.h));
            // X-Trans has no 2x2 superpixel: halve after a fast demosaic.
            if xtrans && quality == Quality::Half {
                (pixels, dim) = box_half(&pixels, dim.0, dim.1);
            }
            let sigma = if xtrans { CHROMA_SIGMA_XTRANS } else { CHROMA_SIGMA_BAYER };
            (pixels, dim, sigma)
        }
        RawPhotometricInterpretation::LinearRaw => {
            if raw.cpp != 3 {
                bail!("linear raw with {} samples per pixel is not supported", raw.cpp);
            }
            let per_channel = |v: Vec<f32>| -> [f32; 3] {
                match v.len() {
                    0 => [0.0; 3],
                    1 | 2 => [v[0]; 3],
                    _ => [v[0], v[1], v[2]],
                }
            };
            let black = per_channel(raw.blacklevel.as_vec());
            let white = per_channel(raw.whitelevel.as_vec());
            let (cx, cy, cw, ch) = (crop.p.x, crop.p.y, crop.d.w, crop.d.h);
            let mut out = vec![[0f32; 3]; cw * ch];
            out.par_chunks_exact_mut(cw).enumerate().for_each(|(r, row)| {
                let src = &data[((cy + r) * w + cx) * 3..((cy + r) * w + cx + cw) * 3];
                for (o, s) in row.iter_mut().zip(src.chunks_exact(3)) {
                    *o = [0, 1, 2].map(|c| (((s[c] as f32 - black[c]) / (white[c] - black[c])).max(0.0) * wb[c]).min(1.0));
                }
            });
            let (out, dim) = if quality == Quality::Half { box_half(&out, cw, ch) } else { (out, (cw, ch)) };
            // Already demosaiced (and usually denoised) by whatever wrote it.
            (out, dim, 0.0)
        }
        RawPhotometricInterpretation::BlackIsZero => bail!("monochrome sensors are not supported"),
    };

    // Film has no colour noise; digital sensors do. Smooth chroma only.
    let chroma_sigma = match quality {
        Quality::Full => chroma_sigma,
        Quality::Half => chroma_sigma * 0.5,
    };
    if chroma_sigma > 0.0 {
        chroma_denoise(&mut pixels, dim.0, dim.1, chroma_sigma);
    }
    Ok(CameraImage {
        width: dim.0,
        height: dim.1,
        pixels,
        wb,
        pixel_pitch_um: crate::sensor::geometry(raw).pixel_pitch_um,
        cm_a,
        cm_d65,
        baseline_ev: baseline_exposure(raw),
    })
}

/// 2x2 box downscale (odd edges dropped).
fn box_half(pixels: &[[f32; 3]], w: usize, h: usize) -> (Vec<[f32; 3]>, (usize, usize)) {
    let (hw, hh) = ((w / 2).max(1), (h / 2).max(1));
    if w < 2 || h < 2 {
        return (pixels.to_vec(), (w, h));
    }
    let out = (0..hw * hh)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % hw * 2, i / hw * 2);
            let [a, b, c, d] = [pixels[y * w + x], pixels[y * w + x + 1], pixels[(y + 1) * w + x], pixels[(y + 1) * w + x + 1]];
            [0, 1, 2].map(|k| (a[k] + b[k] + c[k] + d[k]) * 0.25)
        })
        .collect();
    (out, (hw, hh))
}
