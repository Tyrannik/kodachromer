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

/// True for the Leica M9 family (M9, M9-P, M9 Titanium) and the M-E (Typ 220),
/// which shares the M9's sensor and colour. Monochrom bodies don't count:
/// they have no colour filter array.
///
/// Accepts both the raw DNG tags ("Leica Camera AG" / "M9 Digital Camera")
/// and rawler's cleaned names ("Leica" / "M9").
pub fn is_leica_m9(make: &str, model: &str) -> bool {
    if !make.to_ascii_lowercase().contains("leica") {
        return false;
    }
    let model = model.trim().to_ascii_lowercase();
    let model = model.strip_prefix("leica ").unwrap_or(&model);
    let model = model.strip_suffix(" digital camera").unwrap_or(model).trim();
    matches!(model, "m9" | "m9-p" | "m9 titanium" | "m-e" | "m-e typ 220" | "m-e (typ 220)")
}

/// A user-facing warning when a file isn't from a Leica M9, or `None` if it is.
pub fn camera_warning(make: &str, model: &str) -> Option<String> {
    if is_leica_m9(make, model) {
        return None;
    }
    let (make, model) = (make.trim(), model.trim());
    let camera = match (make.is_empty(), model.is_empty()) {
        (true, true) => "an unknown camera".to_string(),
        // Models often repeat the make ("Canon EOS R5"); don't say it twice.
        (false, false) if model.to_ascii_lowercase().starts_with(&make.to_ascii_lowercase()) => model.to_string(),
        _ => format!("{make} {model}").trim().to_string(),
    };
    Some(format!(
        "Not a Leica M9 file ({camera}). Remjet's colour and film looks are tuned for the M9 sensor, so results may be off."
    ))
}

/// Check a file's camera from its metadata alone (much cheaper than [`load`]).
/// `Ok(None)` means it's an M9.
pub fn check_camera(path: &Path) -> Result<Option<String>> {
    let e = crate::exif::read(path)?;
    Ok(camera_warning(e.make.as_deref().unwrap_or(""), e.model.as_deref().unwrap_or("")))
}

impl RawInfo {
    /// See [`camera_warning`].
    pub fn camera_warning(&self) -> Option<String> {
        camera_warning(&self.make, &self.model)
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn m9_names_are_accepted() {
        // Raw DNG tags as the M9 writes them, and rawler's cleaned names.
        assert!(is_leica_m9("Leica Camera AG", "M9 Digital Camera"));
        assert!(is_leica_m9("Leica", "M9"));
        assert!(is_leica_m9("LEICA CAMERA AG ", " m9 digital camera "));
        assert!(is_leica_m9("Leica Camera AG", "M9-P Digital Camera"));
        assert!(is_leica_m9("Leica Camera AG", "M9 Titanium"));
        assert!(is_leica_m9("Leica Camera AG", "M-E Digital Camera"));
        assert!(is_leica_m9("Leica", "Leica M9"));
        assert_eq!(camera_warning("Leica", "M9"), None);
    }

    #[test]
    fn other_cameras_are_rejected() {
        assert!(!is_leica_m9("Leica Camera AG", "M Monochrom"));
        assert!(!is_leica_m9("Leica Camera AG", "M9 Monochrom"));
        assert!(!is_leica_m9("Leica", "M8"));
        assert!(!is_leica_m9("Leica", "M10"));
        assert!(!is_leica_m9("Leica Camera AG", "LEICA M (Typ 240)"));
        assert!(!is_leica_m9("Leica", "Q2"));
        assert!(!is_leica_m9("Canon", "EOS R5"));
        assert!(!is_leica_m9("Ricoh", "M9")); // right model name, wrong maker
        assert!(!is_leica_m9("", ""));
    }

    #[test]
    fn warning_names_the_camera() {
        let w = camera_warning("Canon", "EOS R5").expect("not an M9");
        assert!(w.contains("Not a Leica M9"), "{w}");
        assert!(w.contains("(Canon EOS R5)"), "{w}");

        // Don't repeat a make the model already includes.
        let w = camera_warning("Canon", "Canon EOS R5").expect("not an M9");
        assert!(w.contains("(Canon EOS R5)"), "{w}");

        let w = camera_warning("", "").expect("not an M9");
        assert!(w.contains("(an unknown camera)"), "{w}");

        let w = camera_warning("Leica", "M10").expect("not an M9");
        assert!(w.contains("(Leica M10)"), "{w}");
    }

    #[test]
    fn raw_info_uses_its_camera() {
        let info = |make: &str, model: &str| RawInfo {
            make: make.into(),
            model: model.into(),
            width: 0,
            height: 0,
            bits_per_sample: 0,
            black_levels: vec![],
            white_levels: vec![],
            as_shot_wb: [0.0; 4],
            color_matrix_illuminants: vec![],
            crop: None,
            has_linearization_table: false,
            is_float: false,
        };
        assert_eq!(info("Leica", "M9").camera_warning(), None);
        assert!(info("Nikon", "Z 6").camera_warning().is_some());
    }

    #[test]
    fn check_camera_fails_on_unreadable_files() {
        let dir = std::env::temp_dir().join(format!("remjet-camera-check-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("not-a-raw.dng");
        std::fs::write(&path, b"definitely not a DNG").unwrap();
        assert!(check_camera(&path).is_err());
        assert!(check_camera(&dir.join("missing.dng")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
