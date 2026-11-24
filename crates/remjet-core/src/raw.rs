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
    pub sensor: crate::sensor::SensorGeometry,
}

/// File extensions of raw formats Remjet opens (all decoded by rawler).
/// Bayer and X-Trans sensors work; Sigma Foveon (X3F) and monochrome bodies
/// are listed by rawler but are rejected when developed.
pub const RAW_EXTENSIONS: &[&str] = &[
    "dng", "3fr", "arw", "cr2", "cr3", "crw", "dcr", "erf", "fff", "iiq", "kdc", "mef", "mos", "mrw", "nef", "nrw",
    "orf", "ori", "pef", "raf", "rw2", "rwl", "srw",
];

/// True if `path` has a raw file extension Remjet can open.
pub fn is_raw_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| RAW_EXTENSIONS.iter().any(|r| r.eq_ignore_ascii_case(e)))
}

/// Decode a raw file fully (rawler applies any linearisation table here).
/// Also picks up the EXIF tags [`crate::sensor`] needs for the pixel pitch.
pub fn load(path: &Path) -> Result<RawImage> {
    let mut image = rawler::decode_file(path).with_context(|| format!("decoding {}", path.display()))?;
    crate::sensor::stash_exif_hints(path, &mut image);
    Ok(image)
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
        sensor: crate::sensor::geometry(image),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_extensions() {
        for name in ["a.dng", "a.DNG", "b.CR3", "c.nef", "d.RAF", "e.arw", "f.rw2"] {
            assert!(is_raw_file(Path::new(name)), "{name}");
        }
        for name in ["a.jpg", "a.tif", "a.json", "dng", "a"] {
            assert!(!is_raw_file(Path::new(name)), "{name}");
        }
    }
}
