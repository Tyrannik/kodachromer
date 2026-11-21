//! Raw development: DNG mosaic -> scene-linear image in the working space
//! (linear ProPhoto RGB, D50). No look or tone curve is applied here; this is
//! the neutral, colorimetric starting point for the film model.

use anyhow::{Context, Result, bail};
use rawler::imgop::sensor::Demosaic;
use rawler::imgop::sensor::bayer::ppg::PPGDemosaic;
use rawler::imgop::sensor::bayer::superpixel::Superpixel3Channel;
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

    fn clone_meta(&self) -> CameraImage {
        CameraImage { width: 0, height: 0, pixels: Vec::new(), wb: self.wb, cm_a: self.cm_a, cm_d65: self.cm_d65, baseline_ev: self.baseline_ev }
    }
}

/// Levels, white balance and demosaic. `wb = None` uses the as-shot balance.
pub fn demosaic(raw: &RawImage, wb: Option<[f32; 3]>, quality: Quality) -> Result<CameraImage> {
    let RawPhotometricInterpretation::Cfa(cfa_config) = &raw.photometric else {
        bail!("only Bayer sensors are supported");
    };
    let cfa = &cfa_config.cfa;
    if raw.cpp != 1 {
        bail!("expected one sample per pixel, got {}", raw.cpp);
    }
    let RawImageData::Integer(data) = &raw.data else {
        bail!("floating-point raw data is not supported");
    };

    let wb = wb.unwrap_or_else(|| as_shot_wb(raw));
    let black = raw.blacklevel.as_bayer_array();
    let white = raw.whitelevel.as_bayer_array();

    // 1. Black/white levels and white balance on the mosaic. Clipping every
    //    channel at 1.0 keeps blown highlights neutral instead of pink.
    let (w, h) = (raw.width, raw.height);
    let mut mosaic = vec![0f32; w * h];
    mosaic.par_chunks_exact_mut(w).enumerate().for_each(|(row, out)| {
        let src = &data[row * w..(row + 1) * w];
        for (col, (o, &v)) in out.iter_mut().zip(src).enumerate() {
            let pos = (row % 2) * 2 + col % 2;
            let lin = (v as f32 - black[pos]) / (white[pos] - black[pos]);
            *o = (lin.max(0.0) * wb[cfa.color_at(row, col)]).min(1.0);
        }
    });

    // 2. Demosaic straight into the default crop.
    let crop = raw.crop_area.unwrap_or(Rect::new(Point::new(0, 0), Dim2::new(w, h)));
    let pixels = PixF32::new_with(mosaic, w, h);
    let rgb = match quality {
        Quality::Full => PPGDemosaic::new().demosaic(&pixels, cfa, &cfa_config.colors, crop),
        Quality::Half => Superpixel3Channel::new().demosaic(&pixels, cfa, &cfa_config.colors, crop),
    };
    let dim = rgb.dim();

    let cm_d65 = color_matrix(raw, Illuminant::D65).context("DNG has no D65 colour matrix")?;
    let cm_a = color_matrix(raw, Illuminant::A).unwrap_or(cm_d65);
    Ok(CameraImage {
        width: dim.w,
        height: dim.h,
        pixels: rgb.into_inner(),
        wb,
        cm_a,
        cm_d65,
        baseline_ev: baseline_exposure(raw),
    })
}
