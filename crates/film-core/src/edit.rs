//! Non-destructive edits: the per-image settings saved in a JSON sidecar next
//! to each DNG (`L1001234.DNG.m9film.json`), and the shared render path used
//! by the app, the CLI and batch export.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use image::RgbImage;
use serde::{Deserialize, Serialize};

use crate::develop::{self, CameraImage, Quality};
use crate::film::{FilmModel, FilmParams, FilmPreset, GrainFrame};
use crate::{output, raw};

pub const SIDECAR_SUFFIX: &str = ".m9film.json";
const EDIT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Edit {
    pub version: u32,
    /// Film preset id, or `None` for the neutral (no film) render.
    pub preset: Option<String>,
    /// Exposure in stops.
    pub exposure_ev: f32,
    /// White balance warmth, -1 (cool) .. +1 (warm), relative to as shot.
    pub warmth: f32,
    /// White balance tint, -1 (green) .. +1 (magenta), relative to as shot.
    pub tint: f32,
    /// Grain multiplier (1 = preset default, 0 = off).
    pub grain: f32,
    /// Grain clump size multiplier (1 = preset default).
    pub grain_size: f32,
    /// Highlight shoulder, -1 (recover detail) .. +1 (brighter, blows sooner).
    pub highlights: f32,
    /// Detail sharpening (like a scanner's), 0 = off.
    pub sharpen: f32,
}

impl Default for Edit {
    fn default() -> Self {
        Self {
            version: EDIT_VERSION,
            preset: Some("kodachrome64".into()),
            exposure_ev: 0.0,
            warmth: 0.0,
            tint: 0.0,
            grain: 1.0,
            grain_size: 1.0,
            highlights: 0.0,
            sharpen: 0.3,
        }
    }
}

impl Edit {
    /// White-balance multipliers (R, G, B) for the warmth/tint sliders.
    /// Full slider travel is about +/-1500 K around daylight.
    pub fn wb_adjust(&self) -> [f32; 3] {
        let w = 2f32.powf(self.warmth * 0.35);
        let t = 2f32.powf(-self.tint * 0.25);
        [w, t, 1.0 / w]
    }
}

pub fn sidecar_path(dng: &Path) -> PathBuf {
    let mut name = dng.file_name().unwrap_or_default().to_os_string();
    name.push(SIDECAR_SUFFIX);
    dng.with_file_name(name)
}

/// The saved edit for a DNG, if it has one.
pub fn load(dng: &Path) -> Option<Edit> {
    let text = std::fs::read_to_string(sidecar_path(dng)).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save(dng: &Path, edit: &Edit) -> Result<()> {
    let path = sidecar_path(dng);
    let text = serde_json::to_string_pretty(edit)?;
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
}

/// Stable grain seed from the file name, so re-renders match exactly.
pub fn seed_for(path: &Path) -> u64 {
    path.file_name()
        .map(|n| {
            n.to_string_lossy()
                .bytes()
                .fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
        })
        .unwrap_or(0)
}

/// Render a cached camera image with an edit. Returns an un-rotated 8-bit
/// image. `pixel_scale` is the size of one output pixel in full-resolution
/// sensor pixels (keeps grain consistent between preview and export).
pub fn render(cam: &CameraImage, edit: &Edit, pixel_scale: f64, seed: u64) -> Result<RgbImage> {
    render_with(cam, edit, None, pixel_scale, seed)
}

/// Like [`render`], but `preset_override` (an unsaved, in-app tuned preset)
/// replaces the stored preset when given.
pub fn render_with(
    cam: &CameraImage,
    edit: &Edit,
    preset_override: Option<&FilmPreset>,
    pixel_scale: f64,
    seed: u64,
) -> Result<RgbImage> {
    render_framed(cam, edit, preset_override, pixel_scale, seed, GrainFrame { origin: (0, 0), stride: cam.width })
}

fn render_framed(
    cam: &CameraImage,
    edit: &Edit,
    preset_override: Option<&FilmPreset>,
    pixel_scale: f64,
    seed: u64,
    frame: GrainFrame,
) -> Result<RgbImage> {
    let mut linear = develop::to_working(cam, edit.wb_adjust(), edit.exposure_ev)?;
    develop::sharpen(&mut linear, edit.sharpen, (0.8 / pixel_scale) as f32);
    Ok(match &edit.preset {
        Some(id) => {
            let preset = match preset_override {
                Some(p) => p.clone(),
                None => crate::presets::load(id)?,
            };
            let params = FilmParams {
                grain: edit.grain as f64,
                grain_size: edit.grain_size as f64,
                highlights: edit.highlights as f64,
                pixel_scale,
                seed,
                ..Default::default()
            };
            output::display_to_srgb8(&FilmModel::new(&preset, &params)?.apply_framed(&linear, seed, frame))
        }
        None => output::to_srgb8(&linear),
    })
}

/// Render a rectangle (sensor coordinates, full resolution) of a full-frame
/// camera image. Grain and sharpening match a full render exactly: the crop is
/// rendered with an apron and grain is taken from absolute frame positions.
pub fn render_region(
    cam: &CameraImage,
    rect: (usize, usize, usize, usize),
    edit: &Edit,
    preset_override: Option<&FilmPreset>,
    seed: u64,
) -> Result<RgbImage> {
    const APRON: usize = 12;
    let (x, y, w, h) = rect;
    let (ax, ay) = (x.saturating_sub(APRON), y.saturating_sub(APRON));
    let (ax2, ay2) = ((x + w + APRON).min(cam.width), (y + h + APRON).min(cam.height));
    let crop = cam.crop(ax, ay, ax2 - ax, ay2 - ay);
    let img = render_framed(&crop, edit, preset_override, 1.0, seed, GrainFrame { origin: (ax, ay), stride: cam.width })?;
    let (w, h) = (w.min(crop.width - (x - ax)), h.min(crop.height - (y - ay)));
    Ok(image::imageops::crop_imm(&img, (x - ax) as u32, (y - ay) as u32, w as u32, h as u32).to_image())
}

/// Full pipeline for export: decode, develop, apply the edit, orient.
pub fn render_file(path: &Path, edit: &Edit, quality: Quality, max_edge: Option<usize>) -> Result<RgbImage> {
    let raw_image = raw::load(path)?;
    let mut cam = develop::demosaic(&raw_image, None, quality)?;
    let mut scale = if quality == Quality::Half { 2.0 } else { 1.0 };
    if let Some(max) = max_edge {
        let (small, f) = cam.downscale_to(max);
        cam = small;
        scale *= f as f64;
    }
    let img = render(&cam, edit, scale, seed_for(path))?;
    Ok(output::apply_orientation(img, raw_image.orientation))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportOptions {
    /// JPEG quality 1–100.
    pub quality: u8,
    /// Resize so the long edge is at most this many pixels (None = full size).
    pub max_edge: Option<u32>,
    /// Copy camera metadata (date, exposure, camera, lens) into the JPEG.
    pub keep_exif: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self { quality: 92, max_edge: None, keep_exif: true }
    }
}

/// Render and write one JPEG into `out_dir`, named after the DNG.
pub fn export_jpeg(path: &Path, edit: &Edit, out_dir: &Path, opts: &ExportOptions) -> Result<PathBuf> {
    std::fs::create_dir_all(out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    let mut img = render_file(path, edit, Quality::Full, None)?;
    if let Some(max) = opts.max_edge {
        let long = img.width().max(img.height());
        if max > 0 && long > max {
            let s = max as f64 / long as f64;
            let (w, h) = ((img.width() as f64 * s).round() as u32, (img.height() as f64 * s).round() as u32);
            img = image::imageops::resize(&img, w, h, image::imageops::FilterType::Lanczos3);
        }
    }
    let exif = if opts.keep_exif { crate::exif::read(path).ok() } else { None };
    let out = out_dir.join(path.with_extension("jpg").file_name().unwrap_or_default());
    output::write_jpeg_with_exif(&img, &out, opts.quality.clamp(1, 100), exif.as_ref())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_round_trip() {
        let dir = std::env::temp_dir().join(format!("m9film-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dng = dir.join("L1000001.DNG");
        assert!(load(&dng).is_none());
        let edit = Edit { exposure_ev: 0.5, warmth: -0.2, grain: 0.0, ..Default::default() };
        save(&dng, &edit).unwrap();
        assert_eq!(sidecar_path(&dng).file_name().unwrap(), "L1000001.DNG.m9film.json");
        assert_eq!(load(&dng), Some(edit));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_fields_take_defaults() {
        let e: Edit = serde_json::from_str(r#"{"exposure_ev": 1.0}"#).unwrap();
        assert_eq!(e.preset.as_deref(), Some("kodachrome64"));
        assert_eq!(e.exposure_ev, 1.0);
        assert_eq!(e.grain, 1.0);
    }

    #[test]
    fn neutral_wb_adjust_is_identity() {
        assert_eq!(Edit::default().wb_adjust(), [1.0, 1.0, 1.0]);
    }
}
