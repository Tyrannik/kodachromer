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

pub fn develop(raw: &RawImage, params: &DevelopParams) -> Result<LinearImage> {
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

    let wb = params.wb.unwrap_or_else(|| as_shot_wb(raw));
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
    let rgb = match params.quality {
        Quality::Full => PPGDemosaic::new().demosaic(&pixels, cfa, &cfa_config.colors, crop),
        Quality::Half => Superpixel3Channel::new().demosaic(&pixels, cfa, &cfa_config.colors, crop),
    };
    let dim = rgb.dim();

    // 3. Camera RGB -> working space, plus exposure.
    let cm_d65 = color_matrix(raw, Illuminant::D65).context("DNG has no D65 colour matrix")?;
    let cm_a = color_matrix(raw, Illuminant::A).unwrap_or(cm_d65);
    let wb64 = wb.map(f64::from);
    let (m, cct) = color::camera_to_working(&cm_a, &cm_d65, &wb64).context("singular colour matrix")?;
    let gain = 2f64.powf((baseline_exposure(raw) + params.exposure_ev) as f64);
    let m: [[f32; 3]; 3] = m.map(|row| row.map(|v| (v * gain) as f32));

    let mut pixels = rgb.into_inner();
    pixels.par_iter_mut().for_each(|p| {
        let c = *p;
        *p = [0, 1, 2].map(|i| m[i][0] * c[0] + m[i][1] * c[1] + m[i][2] * c[2]);
    });

    Ok(LinearImage { width: dim.w, height: dim.h, pixels, cct })
}
