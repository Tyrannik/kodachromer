//! Raw file loading and inspection.

use std::path::Path;

use anyhow::{Context, Result};
use rawler::rawimage::{RawImage, RawImageData};

/// DNG tag holding the lookup table the M9 uses for its "compressed" (8-bit) mode.
const TAG_LINEARIZATION_TABLE: u16 = 50712;

/// Summary of a raw file, used to sanity-check that decoding works.
#[derive(Debug, Clone)]
pub struct RawInfo {
    pub make: String,
    pub model: String,
    pub width: usize,
    pub height: usize,
    pub bits_per_sample: usize,
    pub black_levels: Vec<f32>,
    pub white_levels: Vec<f32>,
    pub as_shot_wb: [f32; 4],
    pub color_matrix_illuminants: Vec<String>,
    pub crop: Option<(usize, usize, usize, usize)>,
    pub has_linearization_table: bool,
    pub is_float: bool,
}

/// Decode a raw file fully (rawler applies any linearisation table here).
pub fn load(path: &Path) -> Result<RawImage> {
    rawler::decode_file(path).with_context(|| format!("decoding {}", path.display()))
}

pub fn info(image: &RawImage) -> RawInfo {
    let mut illuminants: Vec<String> = image.color_matrix.keys().map(|k| format!("{k:?}")).collect();
    illuminants.sort();
    RawInfo {
        make: image.clean_make.clone(),
        model: image.clean_model.clone(),
        width: image.width,
        height: image.height,
        bits_per_sample: image.bps,
        black_levels: image.blacklevel.as_vec(),
        white_levels: image.whitelevel.as_vec(),
        as_shot_wb: image.wb_coeffs,
        color_matrix_illuminants: illuminants,
        crop: image.crop_area.map(|r| (r.p.x, r.p.y, r.d.w, r.d.h)),
        has_linearization_table: image.dng_tags.contains_key(&TAG_LINEARIZATION_TABLE),
        is_float: matches!(image.data, RawImageData::Float(_)),
    }
}
