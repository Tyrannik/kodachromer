//! Physical sensor geometry: how big one pixel is on the image plane.
//!
//! Film grain, film sharpness and halation are physical: they have a size in
//! micrometres on the film, not in pixels. To render them consistently on any
//! camera, each file's pixel pitch is worked out from its own metadata. The
//! film frame is taken to be the same size as the sensor, as if the same lens
//! had exposed a piece of film cut to that format.
//!
//! The Leica M9 keeps the exact pitch Remjet was tuned with, so M9 output does
//! not change.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;

use rawler::formats::tiff::Value;
use rawler::formats::tiff::reader::{GenericTiffReader, TiffReader};
use rawler::rawimage::RawImage;
use rawler::rawsource::RawSource;

/// Pixel pitch the film looks were tuned at: the M9's 5212 px across 36 mm.
pub const M9_PIXEL_UM: f64 = 36_000.0 / 5212.0;

/// Diagonal of a 36 x 24 mm full frame.
const FULL_FRAME_DIAGONAL_MM: f64 = 43.266_615;

// EXIF tags read from the file. Stored in `RawImage::dng_tags` by
// [`stash_exif_hints`] so geometry can be worked out from the decoded image
// alone; their numbers don't collide with any DNG tag (50706 and up).
const TAG_FOCAL_LENGTH: u16 = 0x920a;
const TAG_FOCAL_LENGTH_35MM: u16 = 0xa405;
const TAG_FOCAL_PLANE_X_RES: u16 = 0xa20e;
const TAG_FOCAL_PLANE_UNIT: u16 = 0xa210;
// TIFF/EP spellings of the focal-plane tags, used by some raw formats.
const TAG_FOCAL_PLANE_X_RES_EP: u16 = 0x920e;
const TAG_FOCAL_PLANE_UNIT_EP: u16 = 0x9210;

/// Where a pixel pitch came from, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PitchSource {
    /// A Leica M9-family body: the pitch the looks were tuned at.
    LeicaM9,
    /// Crop factor from the 35 mm-equivalent focal length.
    CropFactor,
    /// EXIF focal-plane resolution.
    FocalPlane,
    /// Nothing usable in the file: assumed full frame.
    AssumedFullFrame,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SensorGeometry {
    /// Distance between pixel centres on the sensor, in micrometres.
    pub pixel_pitch_um: f64,
    /// Width and height of the image area (default crop), in millimetres.
    pub format_mm: (f64, f64),
    pub source: PitchSource,
}

impl SensorGeometry {
    /// Number of sensor pixels that cover the same distance as one M9 pixel.
    /// Pixel-sized look parameters tuned on the M9 are multiplied by this.
    pub fn m9_scale(&self) -> f64 {
        M9_PIXEL_UM / self.pixel_pitch_um
    }

    /// True when the pitch was guessed rather than read from the file.
    pub fn is_assumed(&self) -> bool {
        self.source == PitchSource::AssumedFullFrame
    }
}

impl fmt::Display for SensorGeometry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let how = match self.source {
            PitchSource::LeicaM9 => "Leica M9",
            PitchSource::CropFactor => "from crop factor",
            PitchSource::FocalPlane => "from focal-plane resolution",
            PitchSource::AssumedFullFrame => "assumed full frame",
        };
        write!(f, "{:.2} µm pixels, {:.1} x {:.1} mm ({how})", self.pixel_pitch_um, self.format_mm.0, self.format_mm.1)
    }
}

/// What the file says about its geometry. Every field is optional.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SensorHints {
    pub m9_family: bool,
    pub focal_length_mm: Option<f64>,
    pub focal_length_35mm: Option<f64>,
    /// Pixels per millimetre on the focal plane.
    pub focal_plane_px_per_mm: Option<f64>,
}

/// Work out the pixel pitch for an image area of `width` x `height` pixels.
pub fn geometry_from_hints(hints: &SensorHints, width: usize, height: usize) -> SensorGeometry {
    let (w, h) = (width.max(1) as f64, height.max(1) as f64);
    let make = |pitch_um: f64, source| SensorGeometry { pixel_pitch_um: pitch_um, format_mm: (w * pitch_um / 1000.0, h * pitch_um / 1000.0), source };
    // Sensors from about 1/2.3" phones to 54 mm medium format.
    let plausible = |pitch_um: f64| {
        let width_mm = w.max(h) * pitch_um / 1000.0;
        (0.8..=16.0).contains(&pitch_um) && (5.0..=60.0).contains(&width_mm)
    };

    if hints.m9_family {
        return make(M9_PIXEL_UM, PitchSource::LeicaM9);
    }
    if let (Some(f), Some(f35)) = (hints.focal_length_mm, hints.focal_length_35mm)
        && f > 0.0
        && f35 > 0.0
    {
        let diagonal_mm = FULL_FRAME_DIAGONAL_MM * f / f35;
        let pitch = diagonal_mm * 1000.0 / w.hypot(h);
        if plausible(pitch) {
            return make(pitch, PitchSource::CropFactor);
        }
    }
    if let Some(px_per_mm) = hints.focal_plane_px_per_mm
        && px_per_mm > 0.0
    {
        let pitch = 1000.0 / px_per_mm;
        if plausible(pitch) {
            return make(pitch, PitchSource::FocalPlane);
        }
    }
    // Full frame: 36 mm across the long edge.
    make(36_000.0 / w.max(h), PitchSource::AssumedFullFrame)
}

/// Geometry of a decoded raw file's default crop. Files opened with
/// [`crate::raw::load`] carry the EXIF hints needed; others fall back to a
/// camera-name check and then to full frame.
pub fn geometry(raw: &RawImage) -> SensorGeometry {
    let (w, h) = raw.crop_area.map(|r| (r.d.w, r.d.h)).unwrap_or((raw.width, raw.height));
    geometry_from_hints(&hints_from_raw(raw), w, h)
}

/// Bodies that share the M9's 18 MP Kodak sensor.
pub fn is_m9_family(make: &str, model: &str) -> bool {
    let make = make.to_ascii_lowercase();
    let model = model.to_ascii_lowercase();
    if !make.contains("leica") {
        return false;
    }
    let model = model.trim_start_matches("leica").trim();
    model.starts_with("m9") || model.starts_with("m-e") || (model.starts_with("m monochrom") && !model.contains("typ"))
}

fn hints_from_raw(raw: &RawImage) -> SensorHints {
    SensorHints {
        m9_family: is_m9_family(&raw.make, &raw.model) || is_m9_family(&raw.clean_make, &raw.clean_model),
        ..hints_from_tags(&raw.dng_tags)
    }
}

fn hints_from_tags(tags: &HashMap<u16, Value>) -> SensorHints {
    let get = |tag: u16| tags.get(&tag).and_then(|v| value_f64(v, 0));
    let unit_mm = |tag: u16| match tags.get(&tag).and_then(|v| value_f64(v, 0)).map(|u| u as u32) {
        Some(3) => Some(10.0),
        Some(4) => Some(1.0),
        Some(5) => Some(0.001),
        // 2 (inch) is the default when the unit is missing.
        Some(2) | None => Some(25.4),
        _ => None,
    };
    let focal_plane = [(TAG_FOCAL_PLANE_X_RES, TAG_FOCAL_PLANE_UNIT), (TAG_FOCAL_PLANE_X_RES_EP, TAG_FOCAL_PLANE_UNIT_EP)]
        .into_iter()
        .find_map(|(res, unit)| Some(get(res)? / unit_mm(unit)?));
    SensorHints {
        m9_family: false,
        focal_length_mm: get(TAG_FOCAL_LENGTH),
        focal_length_35mm: get(TAG_FOCAL_LENGTH_35MM),
        focal_plane_px_per_mm: focal_plane,
    }
}

fn value_f64(v: &Value, idx: usize) -> Option<f64> {
    let x = match v {
        Value::Rational(r) => r.get(idx).filter(|r| r.d != 0).map(|r| r.n as f64 / r.d as f64),
        Value::SRational(r) => r.get(idx).filter(|r| r.d != 0).map(|r| r.n as f64 / r.d as f64),
        Value::Short(s) => s.get(idx).map(|&s| s as f64),
        Value::Long(l) => l.get(idx).map(|&l| l as f64),
        Value::Float(f) => f.get(idx).map(|&f| f as f64),
        Value::Double(d) => d.get(idx).copied(),
        _ => None,
    }?;
    x.is_finite().then_some(x)
}

/// Copy the EXIF tags geometry needs from a TIFF-based raw file (DNG, CR2,
/// NEF, ARW, PEF, ORF, RW2 ...) into `raw.dng_tags`. Best effort: other
/// containers, or files without the tags, are left alone.
pub fn stash_exif_hints(path: &Path, raw: &mut RawImage) {
    let Ok(src) = RawSource::new(path) else { return };
    let Ok(tiff) = GenericTiffReader::new(&mut src.reader(), 0, 0, None, &[]) else { return };
    for tag in [
        TAG_FOCAL_LENGTH,
        TAG_FOCAL_LENGTH_35MM,
        TAG_FOCAL_PLANE_X_RES,
        TAG_FOCAL_PLANE_UNIT,
        TAG_FOCAL_PLANE_X_RES_EP,
        TAG_FOCAL_PLANE_UNIT_EP,
    ] {
        if raw.dng_tags.contains_key(&tag) {
            continue;
        }
        if let Some(entry) = tiff.find_first_ifd_with_tag(tag).and_then(|ifd| ifd.get_entry(tag)) {
            raw.dng_tags.insert(tag, entry.value.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const M9_CROP: (usize, usize) = (5212, 3472);

    #[test]
    fn m9_keeps_its_tuned_pitch() {
        // Even with other hints present, the M9 uses the exact tuning pitch.
        let hints = SensorHints { m9_family: true, focal_length_mm: Some(35.0), focal_length_35mm: Some(50.0), focal_plane_px_per_mm: Some(100.0) };
        let g = geometry_from_hints(&hints, M9_CROP.0, M9_CROP.1);
        assert_eq!(g.pixel_pitch_um, M9_PIXEL_UM);
        assert_eq!(g.m9_scale(), 1.0);
        assert_eq!(g.source, PitchSource::LeicaM9);
    }

    #[test]
    fn m9_family_names() {
        for (make, model) in [
            ("Leica Camera AG", "M9 Digital Camera"),
            ("Leica", "M9"),
            ("Leica", "M9-P"),
            ("Leica", "M9 Titanium"),
            ("Leica", "M-E"),
            ("Leica Camera AG", "M Monochrom"),
        ] {
            assert!(is_m9_family(make, model), "{make} {model}");
        }
        for (make, model) in [("Leica", "M10"), ("Leica", "M (Typ 240)"), ("Leica", "M Monochrom (Typ 246)"), ("Canon", "EOS R5"), ("", "M9")] {
            assert!(!is_m9_family(make, model), "{make} {model}");
        }
    }

    #[test]
    fn crop_factor_gives_aps_c_pitch() {
        // Fujifilm X-T4: 6240 x 4160 on 23.5 x 15.6 mm, 23 mm lens = 35 mm equiv.
        let hints = SensorHints { focal_length_mm: Some(23.0), focal_length_35mm: Some(35.0), ..Default::default() };
        let g = geometry_from_hints(&hints, 6240, 4160);
        assert_eq!(g.source, PitchSource::CropFactor);
        assert!((g.pixel_pitch_um - 3.76).abs() < 0.1, "{g}");
        assert!((g.format_mm.0 - 23.5).abs() < 0.6, "{g}");
    }

    #[test]
    fn focal_plane_resolution_is_used_without_crop_factor() {
        // Sony A7R IV: 9504 px across 35.7 mm = 3.76 µm.
        let hints = SensorHints { focal_plane_px_per_mm: Some(1000.0 / 3.76), ..Default::default() };
        let g = geometry_from_hints(&hints, 9504, 6336);
        assert_eq!(g.source, PitchSource::FocalPlane);
        assert!((g.pixel_pitch_um - 3.76).abs() < 1e-9);
    }

    #[test]
    fn implausible_hints_fall_back_to_full_frame() {
        // A focal-plane resolution written for a thumbnail, and a bogus focal length.
        let hints = SensorHints { focal_length_mm: Some(50.0), focal_length_35mm: Some(1.0), focal_plane_px_per_mm: Some(5.0), ..Default::default() };
        let g = geometry_from_hints(&hints, 6000, 4000);
        assert!(g.is_assumed());
        assert!((g.format_mm.0 - 36.0).abs() < 1e-9);
        assert!((g.pixel_pitch_um - 6.0).abs() < 1e-9);
    }

    #[test]
    fn portrait_crop_uses_long_edge_for_fallback() {
        let g = geometry_from_hints(&SensorHints::default(), 4000, 6000);
        assert!((g.format_mm.1 - 36.0).abs() < 1e-9);
    }

    #[test]
    fn exif_values_parse() {
        use rawler::formats::tiff::Rational;
        let mut tags = HashMap::new();
        tags.insert(TAG_FOCAL_LENGTH, Value::Rational(vec![Rational::new(230, 10)]));
        tags.insert(TAG_FOCAL_LENGTH_35MM, Value::Short(vec![35]));
        tags.insert(TAG_FOCAL_PLANE_X_RES, Value::Rational(vec![Rational::new(6765, 1)]));
        tags.insert(TAG_FOCAL_PLANE_UNIT, Value::Short(vec![2]));
        let h = hints_from_tags(&tags);
        assert_eq!(h.focal_length_mm, Some(23.0));
        assert_eq!(h.focal_length_35mm, Some(35.0));
        assert!((h.focal_plane_px_per_mm.unwrap() - 6765.0 / 25.4).abs() < 1e-9);
        // Centimetres, and the TIFF/EP spelling.
        let mut tags = HashMap::new();
        tags.insert(TAG_FOCAL_PLANE_X_RES_EP, Value::Rational(vec![Rational::new(2660, 1)]));
        tags.insert(TAG_FOCAL_PLANE_UNIT_EP, Value::Short(vec![3]));
        assert!((hints_from_tags(&tags).focal_plane_px_per_mm.unwrap() - 266.0).abs() < 1e-9);
        assert_eq!(hints_from_tags(&HashMap::new()), SensorHints::default());
    }
}
