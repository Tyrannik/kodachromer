//! Non-destructive edits: the per-image settings (kept in the app's edit
//! store, keyed by file content), and the shared render path used by the app,
//! the CLI and batch export.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use image::RgbImage;
use serde::{Deserialize, Serialize};

use crate::develop::{self, CameraImage, Quality};
pub use crate::film::OutputMode;
use crate::film::{FilmModel, FilmParams, FilmPreset, GrainFrame};
use crate::{output, raw};

/// Sidecar names used beside DNGs by 0.1.0-alpha.1 (as Kodachromer) and
/// pre-release builds; found files are imported into the edit store and removed.
const SIDECAR_SUFFIX: &str = ".kodachromer.json";
const LEGACY_SIDECAR_SUFFIX: &str = ".m9film.json";
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
    /// Fit the camera's highlight range onto the film's, so the sensor's clip
    /// point lands on the film's white. Off renders the datasheet curve as is.
    pub fit_highlights: bool,
    /// Scanner colour correction for scanned negatives and Kodachrome: map
    /// the film's layer readings back to true colour, as a lab scanner does.
    /// Off shows the raw readings, which washes out saturated colours.
    pub scanner_colour: bool,
    /// Detail sharpening (like a scanner's), 0 = off.
    pub sharpen: f32,
    /// Halation glow around bright highlights, 0 = off .. 1 = strong.
    pub halation: f32,
    /// Scanned or projected slide.
    pub output: OutputMode,
    /// Push (+) or pull (-) processing in stops; 0 = normal. Only values
    /// the film's datasheet lists are used (others render as normal).
    pub push: i32,
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
            fit_highlights: true,
            scanner_colour: true,
            sharpen: 0.3,
            // Kodachrome had an anti-halation backing, so off by default.
            halation: 0.0,
            output: OutputMode::Scan,
            push: 0,
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

// ------------------------------------------------------------- edit store
//
// Edits live in the app's data folder (`%APPDATA%\Remjet\edits`), not
// beside the photos. Each DNG is identified by a fingerprint of its contents
// (file size + first 64 KiB, which hold the header and capture metadata), so
// edits follow a file when its folder is moved or renamed. Sidecar files
// written by earlier versions beside the DNG are imported and removed.

/// A stored edit, with the file's last known location for reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredEdit {
    file: String,
    edit: Edit,
}

pub fn store_dir() -> Option<PathBuf> {
    crate::paths::data_dir().map(|d| d.join("edits"))
}

fn with_suffix(dng: &Path, suffix: &str) -> PathBuf {
    let mut name = dng.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    dng.with_file_name(name)
}

/// Content fingerprint of a file (FNV-1a over its size and first 64 KiB),
/// cached by path + size + modification time.
pub fn fingerprint(dng: &Path) -> Option<String> {
    use std::collections::HashMap;
    use std::io::Read;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, (u64, std::time::SystemTime, String)>>> = OnceLock::new();
    let meta = std::fs::metadata(dng).ok()?;
    let (len, mtime) = (meta.len(), meta.modified().ok()?);
    let cache = CACHE.get_or_init(Default::default);
    if let Some((l, m, fp)) = cache.lock().ok()?.get(dng) {
        if *l == len && *m == mtime {
            return Some(fp.clone());
        }
    }
    let mut head = vec![0u8; 64 * 1024];
    let mut f = std::fs::File::open(dng).ok()?;
    let mut n = 0;
    while n < head.len() {
        match f.read(&mut head[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(_) => return None,
        }
    }
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in len.to_le_bytes().iter().chain(&head[..n]) {
        h = (h ^ *b as u64).wrapping_mul(0x100_0000_01b3);
    }
    let fp = format!("{h:016x}");
    cache.lock().ok()?.insert(dng.to_path_buf(), (len, mtime, fp.clone()));
    Some(fp)
}

fn store_file(dir: &Path, dng: &Path) -> Option<PathBuf> {
    fingerprint(dng).map(|fp| dir.join(format!("{fp}.json")))
}

/// Import a sidecar left beside the DNG by an earlier version, then remove it.
fn import_sidecar(dir: &Path, dng: &Path) -> Option<Edit> {
    for suffix in [SIDECAR_SUFFIX, LEGACY_SIDECAR_SUFFIX] {
        let side = with_suffix(dng, suffix);
        let Ok(text) = std::fs::read_to_string(&side) else { continue };
        let edit: Edit = serde_json::from_str(&text).ok()?;
        // Only delete the old file once the edit is safely in the store.
        if save_in(dir, dng, &edit).is_ok() {
            let _ = std::fs::remove_file(&side);
            let _ = std::fs::remove_file(with_suffix(dng, LEGACY_SIDECAR_SUFFIX));
        }
        return Some(edit);
    }
    None
}

pub fn load_in(dir: &Path, dng: &Path) -> Option<Edit> {
    if let Some(path) = store_file(dir, dng) {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(stored) = serde_json::from_str::<StoredEdit>(&text) {
                return Some(stored.edit);
            }
        }
    }
    import_sidecar(dir, dng)
}

pub fn save_in(dir: &Path, dng: &Path, edit: &Edit) -> Result<()> {
    let path = store_file(dir, dng).with_context(|| format!("reading {}", dng.display()))?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let stored = StoredEdit { file: dng.display().to_string(), edit: edit.clone() };
    std::fs::write(&path, serde_json::to_string_pretty(&stored)?).with_context(|| format!("writing {}", path.display()))
}

/// Forget a DNG's edit (it renders with defaults and no longer counts as edited).
pub fn remove_in(dir: &Path, dng: &Path) -> Result<()> {
    if let Some(path) = store_file(dir, dng).filter(|p| p.exists()) {
        std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    }
    Ok(())
}

pub fn remove(dng: &Path) -> Result<()> {
    match store_dir() {
        Some(dir) => remove_in(&dir, dng),
        None => Ok(()),
    }
}

pub fn has_edit_in(dir: &Path, dng: &Path) -> bool {
    store_file(dir, dng).is_some_and(|p| p.exists())
        || with_suffix(dng, SIDECAR_SUFFIX).exists()
        || with_suffix(dng, LEGACY_SIDECAR_SUFFIX).exists()
}

/// True if an old sidecar file still sits beside the DNG.
pub fn has_legacy_sidecar(dng: &Path) -> bool {
    with_suffix(dng, SIDECAR_SUFFIX).exists() || with_suffix(dng, LEGACY_SIDECAR_SUFFIX).exists()
}

/// The saved edit for a DNG, if it has one.
pub fn load(dng: &Path) -> Option<Edit> {
    load_in(&store_dir()?, dng)
}

pub fn save(dng: &Path, edit: &Edit) -> Result<()> {
    save_in(&store_dir().context("no app data folder (APPDATA is not set)")?, dng, edit)
}

/// True if the DNG has a saved edit.
pub fn has_edit(dng: &Path) -> bool {
    store_dir().is_some_and(|d| has_edit_in(&d, dng))
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
    render_framed(cam, edit, preset_override, pixel_scale, seed, GrainFrame { origin: (0, 0), stride: cam.width, height: cam.height })
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
    develop::sharpen(&mut linear, edit.sharpen, sharpen_sigma(pixel_scale));
    Ok(match film_model(cam, edit, preset_override, pixel_scale, seed)? {
        Some(model) => output::display_to_srgb8(&model.apply_framed(&linear, seed, frame)),
        None => output::to_srgb8(&linear),
    })
}

/// Sharpening radius in output pixels.
pub(crate) fn sharpen_sigma(pixel_scale: f64) -> f32 {
    (0.8 / pixel_scale) as f32
}

/// The film model an edit renders with (`None` for the neutral render).
pub(crate) fn film_model(
    cam: &CameraImage,
    edit: &Edit,
    preset_override: Option<&FilmPreset>,
    pixel_scale: f64,
    seed: u64,
) -> Result<Option<FilmModel>> {
    let Some(id) = &edit.preset else { return Ok(None) };
    let preset = match preset_override {
        Some(p) => p.clone(),
        None => crate::presets::load(id)?,
    };
    let params = FilmParams {
        grain: edit.grain as f64,
        grain_size: edit.grain_size as f64,
        highlights: edit.highlights as f64,
        sensor_white: edit.fit_highlights.then(|| cam.clip_white()),
        scanner_colour: edit.scanner_colour,
        halation: edit.halation as f64,
        push: if preset.supports_push(edit.push) { edit.push } else { 0 },
        output: edit.output,
        pixel_scale,
        sensor_pixel_um: cam.pixel_pitch_um,
        seed,
        ..Default::default()
    };
    Ok(Some(FilmModel::new(&preset, &params)?))
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
    let img = render_framed(&crop, edit, preset_override, 1.0, seed, GrainFrame { origin: (ax, ay), stride: cam.width, height: cam.height })?;
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
    /// Add the film to the file name (`L1002695_portra400.jpg`), so one
    /// shoot can be exported in several looks into the same folder.
    pub film_in_name: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self { quality: 92, max_edge: None, keep_exif: true, film_in_name: false }
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
    let out = out_dir.join(export_name(path, edit, opts));
    output::write_jpeg_with_exif(&img, &out, opts.quality.clamp(1, 100), exif.as_ref())?;
    Ok(out)
}

/// File name of a DNG's exported JPEG.
pub fn export_name(path: &Path, edit: &Edit, opts: &ExportOptions) -> String {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    match (&edit.preset, opts.film_in_name) {
        (Some(id), true) => format!("{stem}_{id}.jpg"),
        (None, true) => format!("{stem}_neutral.jpg"),
        _ => format!("{stem}.jpg"),
    }
}

/// `edit` with its film swapped for `preset`, keeping every adjustment.
/// The output and development are kept where the new film offers them, and
/// fall back to a scan at normal development where it doesn't.
pub fn with_film(edit: &Edit, preset: Option<&str>) -> Edit {
    let mut e = Edit { preset: preset.map(str::to_owned), ..edit.clone() };
    if let Some(p) = preset.and_then(|id| crate::presets::load(id).ok()) {
        let negative = p.is_negative() || p.is_monochrome();
        let ok = match e.output {
            OutputMode::Scan => true,
            OutputMode::Print => p.is_negative() && !p.is_monochrome() && p.print.is_some(),
            OutputMode::Projection => !negative,
        };
        if !ok {
            e.output = OutputMode::Scan;
        }
        if !p.supports_push(e.push) {
            e.push = 0;
        }
    }
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temp folder with a fake "DNG" of given content, and a store folder.
    fn setup(tag: &str, content: &[u8]) -> (PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("remjet-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let photos = root.join("photos");
        std::fs::create_dir_all(&photos).unwrap();
        let dng = photos.join("L1000001.DNG");
        std::fs::write(&dng, content).unwrap();
        (root.clone(), root.join("store"), dng)
    }

    #[test]
    fn edits_round_trip_through_the_store_not_the_photo_folder() {
        let (root, store, dng) = setup("store", b"fake dng one");
        assert!(load_in(&store, &dng).is_none());
        assert!(!has_edit_in(&store, &dng));
        let edit = Edit { exposure_ev: 0.5, warmth: -0.2, grain: 0.0, ..Default::default() };
        save_in(&store, &dng, &edit).unwrap();
        assert_eq!(load_in(&store, &dng), Some(edit));
        assert!(has_edit_in(&store, &dng));
        // Nothing was written beside the photo.
        assert_eq!(std::fs::read_dir(dng.parent().unwrap()).unwrap().count(), 1);
        remove_in(&store, &dng).unwrap();
        assert!(!has_edit_in(&store, &dng) && load_in(&store, &dng).is_none());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn edits_follow_a_moved_file() {
        let (root, store, dng) = setup("moved", b"fake dng two");
        let edit = Edit { exposure_ev: 1.25, ..Default::default() };
        save_in(&store, &dng, &edit).unwrap();
        let moved_dir = root.join("elsewhere");
        std::fs::create_dir_all(&moved_dir).unwrap();
        let moved = moved_dir.join("renamed.DNG");
        std::fs::rename(&dng, &moved).unwrap();
        assert_eq!(load_in(&store, &moved), Some(edit));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn old_sidecars_are_imported_and_removed() {
        for suffix in [SIDECAR_SUFFIX, LEGACY_SIDECAR_SUFFIX] {
            let (root, store, dng) = setup("legacy", suffix.as_bytes());
            let side = with_suffix(&dng, suffix);
            std::fs::write(&side, r#"{"exposure_ev": 2.0}"#).unwrap();
            assert!(has_edit_in(&store, &dng));
            assert_eq!(load_in(&store, &dng).unwrap().exposure_ev, 2.0);
            assert!(!side.exists(), "{suffix} sidecar should be removed after import");
            assert_eq!(load_in(&store, &dng).unwrap().exposure_ev, 2.0, "now served from the store");
            std::fs::remove_dir_all(&root).unwrap();
        }
    }

    #[test]
    fn missing_fields_take_defaults() {
        let e: Edit = serde_json::from_str(r#"{"exposure_ev": 1.0}"#).unwrap();
        assert_eq!(e.preset.as_deref(), Some("kodachrome64"));
        assert_eq!(e.exposure_ev, 1.0);
        assert_eq!(e.grain, 1.0);
    }

    #[test]
    fn export_names_can_carry_the_film() {
        let dng = Path::new("shoot/L1002695.DNG");
        let portra = Edit { preset: Some("portra400".into()), ..Default::default() };
        let plain = ExportOptions::default();
        let tagged = ExportOptions { film_in_name: true, ..Default::default() };
        assert_eq!(export_name(dng, &portra, &plain), "L1002695.jpg");
        assert_eq!(export_name(dng, &portra, &tagged), "L1002695_portra400.jpg");
        assert_eq!(export_name(dng, &Edit { preset: None, ..Default::default() }, &tagged), "L1002695_neutral.jpg");
    }

    #[test]
    fn swapping_film_keeps_adjustments_and_drops_what_the_film_lacks() {
        let slide = Edit { exposure_ev: 0.7, warmth: 0.2, output: OutputMode::Projection, push: 1, ..Default::default() };
        let neg = with_film(&slide, Some("portra400"));
        assert_eq!(neg.preset.as_deref(), Some("portra400"));
        assert_eq!((neg.exposure_ev, neg.warmth), (0.7, 0.2));
        assert_eq!(neg.output, OutputMode::Scan, "a negative can't be projected");
        assert_eq!(neg.push, 1, "Portra 400 has push 1");
        let k64 = with_film(&Edit { push: 1, ..neg }, Some("kodachrome64"));
        assert_eq!(k64.push, 0, "Kodachrome has no push");
        assert_eq!(with_film(&slide, None).preset, None);
    }

    #[test]
    fn neutral_wb_adjust_is_identity() {
        assert_eq!(Edit::default().wb_adjust(), [1.0, 1.0, 1.0]);
    }
}
