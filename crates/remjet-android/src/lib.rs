//! JNI bindings for the Android app (`android/`).
//!
//! The app hands us plain file paths: it copies the picked DNG into its cache
//! folder first, because the core decodes from a path and Android's document
//! picker only gives out `content://` URIs. Everything here is synchronous;
//! the app calls it from a background thread.
//!
//! The Kotlin side is `io.github.tyrannik.remjet.Native`.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result, anyhow};
use jni::JNIEnv;
use jni::objects::{JClass, JObject, JString};
use jni::sys::{jboolean, jbyteArray, jint, jstring};
use remjet_core::develop::{self, CameraImage, Quality};
use remjet_core::edit::{self, Edit};
use rawler::decoders::Orientation;
use remjet_core::{exif, output, presets, raw};

/// Long edge of the on-screen preview. Big enough for a phone screen, small
/// enough that changing film re-renders in about a second on an old phone.
pub const PREVIEW_EDGE: usize = 1600;

/// The decoded preview-size image of the open photo, so switching film only
/// re-runs the film model, not the raw decode.
struct Preview {
    path: PathBuf,
    cam: CameraImage,
    scale: f64,
    orientation: Orientation,
    seed: u64,
}

static PREVIEW: Mutex<Option<Preview>> = Mutex::new(None);

/// The edit used on Android for now: the desktop defaults with `film`.
/// `None` or an empty id is the neutral (no film) look.
pub fn edit_for(film: Option<&str>) -> Edit {
    edit::with_film(&Edit::default(), film.filter(|f| !f.is_empty()))
}

/// Decode `path` at half size and shrink it for the screen (cached).
fn load_preview(path: &Path, seed_name: &str) -> Result<()> {
    let mut slot = PREVIEW.lock().map_err(|_| anyhow!("preview lock poisoned"))?;
    if slot.as_ref().is_some_and(|p| p.path == path) {
        return Ok(());
    }
    *slot = None; // free the old photo before decoding the next one
    let raw_image = raw::load(path)?;
    let half = develop::demosaic(&raw_image, None, Quality::Half)?;
    let (cam, f) = half.downscale_to(PREVIEW_EDGE);
    *slot = Some(Preview {
        path: path.to_path_buf(),
        cam,
        scale: 2.0 * f as f64,
        orientation: raw_image.orientation,
        seed: edit::seed_for(Path::new(seed_name)),
    });
    Ok(())
}

/// Preview JPEG bytes of `path` with `film`. `name` is the photo's original
/// file name (the cached copy has another), used for the grain seed so the
/// preview's grain matches the export.
pub fn render_preview(path: &Path, name: &str, film: Option<&str>) -> Result<Vec<u8>> {
    load_preview(path, name)?;
    let slot = PREVIEW.lock().map_err(|_| anyhow!("preview lock poisoned"))?;
    let p = slot.as_ref().context("no photo loaded")?;
    let img = edit::render(&p.cam, &edit_for(film), p.scale, p.seed)?;
    let img = output::apply_orientation(img, p.orientation);
    output::encode_jpeg(&img, 88, None)
}

/// Render `path` with `film` and write a JPEG to `out`. `full` picks full
/// resolution (18 MP on an M9) over half (4.5 MP, about a quarter of the
/// memory and time). Camera metadata is copied across.
pub fn export(path: &Path, name: &str, film: Option<&str>, out: &Path, full: bool, quality: u8) -> Result<()> {
    // Drop the preview cache: a full-size render needs every megabyte an
    // old phone has. It is rebuilt on the next preview.
    if full {
        if let Ok(mut slot) = PREVIEW.lock() {
            *slot = None;
        }
    }
    let quality_mode = if full { Quality::Full } else { Quality::Half };
    let raw_image = raw::load(path)?;
    let cam = develop::demosaic(&raw_image, None, quality_mode)?;
    let orientation = raw_image.orientation;
    drop(raw_image); // the raw samples aren't needed once demosaiced
    let scale = if full { 1.0 } else { 2.0 };
    let img = edit::render(&cam, &edit_for(film), scale, edit::seed_for(Path::new(name)))?;
    drop(cam);
    let img = output::apply_orientation(img, orientation);
    let meta = exif::read(path).ok();
    output::write_jpeg_with_exif(&img, out, quality.clamp(1, 100), meta.as_ref())
}

/// Film list as "id\tname" lines, built-ins first.
pub fn film_list() -> String {
    presets::list().into_iter().map(|(id, name)| format!("{id}\t{name}")).collect::<Vec<_>>().join("\n")
}

/// A warning if the photo isn't from an M9 (colours may be off), else "".
pub fn camera_warning(path: &Path) -> Result<String> {
    Ok(raw::check_camera(path)?.unwrap_or_default())
}

// ------------------------------------------------------------------ JNI glue

/// Run `f`, turning errors and panics into a Java `RuntimeException`.
fn guard<T>(env: &mut JNIEnv, fallback: T, f: impl FnOnce(&mut JNIEnv) -> Result<T>) -> T {
    let result = catch_unwind(AssertUnwindSafe(|| f(env)));
    let message = match result {
        Ok(Ok(v)) => return v,
        Ok(Err(e)) => format!("{e:#}"),
        Err(panic) => {
            let what = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown error".into());
            format!("internal error: {what}")
        }
    };
    if !env.exception_check().unwrap_or(false) {
        let _ = env.throw_new("java/lang/RuntimeException", message);
    }
    fallback
}

fn string(env: &mut JNIEnv, s: &JString) -> Result<String> {
    Ok(env.get_string(s)?.into())
}

fn opt_string(env: &mut JNIEnv, s: &JString) -> Result<Option<String>> {
    if s.is_null() { Ok(None) } else { string(env, s).map(Some) }
}

fn new_string(env: &mut JNIEnv, s: &str) -> Result<jstring> {
    Ok(env.new_string(s)?.into_raw())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_tyrannik_remjet_Native_filmList(mut env: JNIEnv, _class: JClass) -> jstring {
    guard(&mut env, JObject::null().into_raw(), |env| new_string(env, &film_list()))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_tyrannik_remjet_Native_cameraWarning(
    mut env: JNIEnv,
    _class: JClass,
    path: JString,
) -> jstring {
    guard(&mut env, JObject::null().into_raw(), |env| {
        let path = string(env, &path)?;
        let warning = camera_warning(Path::new(&path))?;
        new_string(env, &warning)
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_tyrannik_remjet_Native_renderPreview(
    mut env: JNIEnv,
    _class: JClass,
    path: JString,
    name: JString,
    film: JString,
) -> jbyteArray {
    guard(&mut env, JObject::null().into_raw(), |env| {
        let (path, name, film) = (string(env, &path)?, string(env, &name)?, opt_string(env, &film)?);
        let jpeg = render_preview(Path::new(&path), &name, film.as_deref())?;
        Ok(env.byte_array_from_slice(&jpeg)?.into_raw())
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_tyrannik_remjet_Native_export(
    mut env: JNIEnv,
    _class: JClass,
    path: JString,
    name: JString,
    film: JString,
    out: JString,
    full: jboolean,
    quality: jint,
) {
    guard(&mut env, (), |env| {
        let (path, name, film, out) =
            (string(env, &path)?, string(env, &name)?, opt_string(env, &film)?, string(env, &out)?);
        export(Path::new(&path), &name, film.as_deref(), Path::new(&out), full != 0, quality.clamp(1, 100) as u8)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn film_list_has_builtins() {
        let list = film_list();
        assert!(list.lines().any(|l| l.starts_with("kodachrome64\t")));
        assert!(list.lines().all(|l| l.split('\t').count() == 2));
    }

    #[test]
    fn empty_film_is_neutral() {
        assert_eq!(edit_for(Some("")).preset, None);
        assert_eq!(edit_for(None).preset, None);
        assert_eq!(edit_for(Some("portra400")).preset.as_deref(), Some("portra400"));
    }
}
