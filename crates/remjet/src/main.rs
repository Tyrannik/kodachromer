//! Remjet — desktop app for developing Leica M9 DNGs with film looks.
//!
//! Layout: library (left), preview (centre), edit + export panel (right).
//! All decoding/rendering happens on worker threads; the UI only uploads
//! finished images as textures. Edits auto-save to a JSON sidecar per DNG.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;

use eframe::egui;
use remjet_core::develop::{self, CameraImage, Quality};
use remjet_core::edit::{self, Edit, ExportOptions, OutputMode};
use remjet_core::film::FilmPreset;
use remjet_core::{output, presets, raw};
use image::RgbImage;
use rawler::decoders::Orientation;
use serde::{Deserialize, Serialize};

mod theme;

const PREVIEW_EDGE: usize = 1800;
const THUMB_EDGE: usize = 240;

/// Export size choices: (max long edge, label).
const SIZES: &[(Option<u32>, &str)] =
    &[(None, "Full size"), (Some(4000), "4000 px"), (Some(3000), "3000 px"), (Some(2048), "2048 px"), (Some(1600), "1600 px")];

const VERSION: &str = env!("CARGO_PKG_VERSION");
const REPO_URL: &str = env!("CARGO_PKG_REPOSITORY");

fn main() -> eframe::Result {
    let start_folder = std::env::args().nth(1).map(PathBuf::from);
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1500.0, 950.0])
        .with_title(format!("Remjet {VERSION}"));
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/icon-256.png")) {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native("Remjet", options, Box::new(move |cc| Ok(Box::new(FilmApp::new(cc, start_folder)))))
}

// ------------------------------------------------------------- app settings

/// App-wide preferences, stored in %APPDATA%\Remjet\settings.json.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    last_folder: Option<PathBuf>,
    export: ExportOptions,
    /// Custom export folder; None = "<image folder>\exports".
    export_dir: Option<PathBuf>,
}

impl Settings {
    fn path() -> Option<PathBuf> {
        remjet_core::paths::data_dir().map(|d| d.join("settings.json"))
    }
    fn load() -> Self {
        Self::path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }
    fn save(&self) {
        if let Some(p) = Self::path() {
            let _ = std::fs::create_dir_all(p.parent().expect("has parent"));
            let _ = std::fs::write(p, serde_json::to_string_pretty(self).unwrap_or_default());
        }
    }
}

// ------------------------------------------------------------------ preview

/// Zoomed view: `level` is physical screen pixels per full-resolution image
/// pixel (1.0 = 100%); `center` is in upright image coordinates (0..1).
#[derive(Debug, Clone, Copy, PartialEq)]
struct View {
    level: f32,
    center: [f32; 2],
}

const MAX_ZOOM: f32 = 8.0;

/// What a preview job should produce.
#[derive(Debug, Clone, PartialEq)]
enum Mode {
    /// Downscaled whole-image preview (plus neutral "before" and thumbnail).
    Fit,
    /// Full-resolution render for zooming. `region` (upright 0..1 x0,y0,x1,y1)
    /// is rendered first as a quick patch; `before` asks for the neutral render.
    Detail { region: [f32; 4], before: bool },
}

/// Identifies a preview request; the detail region is excluded so panning
/// never triggers a re-render.
#[derive(Debug, Clone, PartialEq)]
enum ModeKey {
    Fit,
    Detail { before: bool },
}

type RenderKey = (Edit, Option<FilmPreset>);

struct PreviewJob {
    path: PathBuf,
    edit: Edit,
    /// Unsaved tuned preset to preview instead of the stored one.
    preset: Option<FilmPreset>,
    mode: Mode,
}

enum PreviewOut {
    Fit {
        after: egui::ColorImage,
        before: egui::ColorImage,
        thumb: egui::ColorImage,
        full_size: [usize; 2],
        /// Set when the file isn't from a Leica M9.
        camera_warning: Option<String>,
    },
    Patch { image: egui::ColorImage, region: [f32; 4] },
    Full { image: egui::ColorImage, before: bool },
    /// Nothing new to send (result already delivered earlier).
    Done,
}

/// Live tuning of a film preset's look (applies to every image using it).
struct Tune {
    id: String,
    working: FilmPreset,
    saved: FilmPreset,
    /// Kodak-data defaults for double-click reset (built-in presets only).
    reference: Option<FilmPreset>,
}

struct PreviewResult {
    path: PathBuf,
    key: RenderKey,
    out: Result<PreviewOut, String>,
}

/// Map a point on the upright (displayed) image to sensor coordinates,
/// both normalised to 0..1. Mirrors `output::apply_orientation`.
fn display_to_sensor(o: Orientation, u: f32, v: f32) -> (f32, f32) {
    match o {
        Orientation::Normal | Orientation::Unknown => (u, v),
        Orientation::HorizontalFlip => (1.0 - u, v),
        Orientation::Rotate180 => (1.0 - u, 1.0 - v),
        Orientation::VerticalFlip => (u, 1.0 - v),
        Orientation::Rotate90 => (v, 1.0 - u),
        Orientation::Rotate270 => (1.0 - v, u),
        Orientation::Transpose => (v, u),
        Orientation::Transverse => (1.0 - v, 1.0 - u),
    }
}

/// Inverse of [`display_to_sensor`].
fn sensor_to_display(o: Orientation, s: f32, t: f32) -> (f32, f32) {
    match o {
        Orientation::Normal | Orientation::Unknown => (s, t),
        Orientation::HorizontalFlip => (1.0 - s, t),
        Orientation::Rotate180 => (1.0 - s, 1.0 - t),
        Orientation::VerticalFlip => (s, 1.0 - t),
        Orientation::Rotate90 => (1.0 - t, s),
        Orientation::Rotate270 => (t, 1.0 - s),
        Orientation::Transpose => (t, s),
        Orientation::Transverse => (1.0 - t, 1.0 - s),
    }
}

fn swaps_axes(o: Orientation) -> bool {
    matches!(o, Orientation::Rotate90 | Orientation::Rotate270 | Orientation::Transpose | Orientation::Transverse)
}

/// Bounding box of an axis-aligned rect after mapping its corners.
fn map_rect(r: [f32; 4], f: impl Fn(f32, f32) -> (f32, f32)) -> [f32; 4] {
    let (a, b) = (f(r[0], r[1]), f(r[2], r[3]));
    [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)]
}

/// The preview worker's downscaled copy of the current image.
struct FitCache {
    path: PathBuf,
    cam: CameraImage,
    /// Pixel scale of `cam` relative to the full-resolution frame.
    scale: f64,
    orientation: Orientation,
    full_size: [usize; 2],
    camera_warning: Option<String>,
}

/// Renders previews on a background thread.
///
/// * Fit: from a cached half-size, downscaled image (fast for slider moves).
/// * Detail: from a cached full-resolution demosaic. First a quick patch of
///   the visible area, then — unless a newer request has arrived — the whole
///   frame, which the UI keeps as a texture so panning and zooming are free.
fn spawn_preview_worker(ctx: egui::Context) -> (Sender<PreviewJob>, Receiver<PreviewResult>) {
    let (job_tx, job_rx) = channel::<PreviewJob>();
    let (res_tx, res_rx) = channel::<PreviewResult>();
    thread::spawn(move || {
        let mut fit: Option<FitCache> = None;
        let mut full: Option<(PathBuf, CameraImage, Orientation)> = None;
        // Last full renders delivered, so repeat requests cost nothing.
        let mut full_after: Option<(PathBuf, RenderKey)> = None;
        let mut full_before: Option<(PathBuf, Edit)> = None;
        let mut next: Option<PreviewJob> = None;
        loop {
            let mut job = match next.take() {
                Some(j) => j,
                None => match job_rx.recv() {
                    Ok(j) => j,
                    Err(_) => break,
                },
            };
            while let Ok(newer) = job_rx.try_recv() {
                job = newer; // only the newest request matters
            }
            let key: RenderKey = (job.edit.clone(), job.preset.clone());
            let seed = edit::seed_for(&job.path);
            let send = |out: anyhow::Result<PreviewOut>| {
                let ok = res_tx.send(PreviewResult { path: job.path.clone(), key: key.clone(), out: out.map_err(|e| format!("{e:#}")) }).is_ok();
                ctx.request_repaint();
                ok
            };
            let alive = match &job.mode {
                Mode::Fit => send((|| {
                    if fit.as_ref().is_none_or(|c| c.path != job.path) {
                        let raw_image = raw::load(&job.path)?;
                        let cam = develop::demosaic(&raw_image, None, Quality::Half)?;
                        let (w, h) = raw_image.crop_area.map_or((raw_image.width, raw_image.height), |r| (r.d.w, r.d.h));
                        let full_size = if swaps_axes(raw_image.orientation) { [h, w] } else { [w, h] };
                        let (small, f) = cam.downscale_to(PREVIEW_EDGE);
                        fit = Some(FitCache {
                            path: job.path.clone(),
                            cam: small,
                            scale: 2.0 * f as f64,
                            orientation: raw_image.orientation,
                            full_size,
                            camera_warning: raw::info(&raw_image).camera_warning(),
                        });
                    }
                    let FitCache { cam, scale, orientation: o, full_size, camera_warning: warning, .. } = fit.as_ref().expect("filled above");
                    let after = output::apply_orientation(edit::render_with(cam, &job.edit, job.preset.as_ref(), *scale, seed)?, *o);
                    let neutral = Edit { preset: None, ..job.edit.clone() };
                    let before = output::apply_orientation(edit::render(cam, &neutral, *scale, seed)?, *o);
                    let th = (THUMB_EDGE * after.height() as usize / after.width() as usize) as u32;
                    let thumb = image::imageops::thumbnail(&after, THUMB_EDGE as u32, th);
                    Ok(PreviewOut::Fit {
                        after: to_color(&after),
                        before: to_color(&before),
                        thumb: to_color(&thumb),
                        full_size: *full_size,
                        camera_warning: warning.clone(),
                    })
                })()),
                Mode::Detail { region, before } => {
                    let mut alive = true;
                    let result = (|| -> anyhow::Result<()> {
                        if full.as_ref().is_none_or(|c| c.0 != job.path) {
                            full = None; // free the previous image first
                            let raw_image = raw::load(&job.path)?;
                            let cam = develop::demosaic(&raw_image, None, Quality::Full)?;
                            full = Some((job.path.clone(), cam, raw_image.orientation));
                        }
                        let (_, cam, o) = full.as_ref().expect("filled above");
                        if *before {
                            let neutral = Edit { preset: None, ..job.edit.clone() };
                            if full_before.as_ref().is_some_and(|(p, e)| *p == job.path && *e == neutral) {
                                alive = send(Ok(PreviewOut::Done));
                                return Ok(());
                            }
                            let img = output::apply_orientation(edit::render(cam, &neutral, 1.0, seed)?, *o);
                            alive = send(Ok(PreviewOut::Full { image: to_color(&img), before: true }));
                            full_before = Some((job.path.clone(), neutral));
                            return Ok(());
                        }
                        if full_after.as_ref().is_some_and(|(p, k)| *p == job.path && *k == key) {
                            alive = send(Ok(PreviewOut::Done));
                            return Ok(());
                        }
                        // Quick patch of what's on screen (skipped when most of
                        // the frame is visible anyway).
                        let s = map_rect(*region, |u, v| display_to_sensor(*o, u, v));
                        let (x0, y0) = ((s[0] * cam.width as f32) as usize, (s[1] * cam.height as f32) as usize);
                        let (x1, y1) = ((s[2] * cam.width as f32).ceil() as usize, (s[3] * cam.height as f32).ceil() as usize);
                        let (x1, y1) = (x1.min(cam.width), y1.min(cam.height));
                        if x1 > x0 && y1 > y0 && (x1 - x0) * (y1 - y0) < cam.width * cam.height * 2 / 5 {
                            let patch = edit::render_region(cam, (x0, y0, x1 - x0, y1 - y0), &job.edit, job.preset.as_ref(), seed)?;
                            let patch = output::apply_orientation(patch, *o);
                            let rendered = [
                                x0 as f32 / cam.width as f32,
                                y0 as f32 / cam.height as f32,
                                x1 as f32 / cam.width as f32,
                                y1 as f32 / cam.height as f32,
                            ];
                            let region = map_rect(rendered, |s, t| sensor_to_display(*o, s, t));
                            alive = send(Ok(PreviewOut::Patch { image: to_color(&patch), region }));
                            // A newer request (e.g. the slider moved on) wins over the full render.
                            if let Ok(newer) = job_rx.try_recv() {
                                next = Some(newer);
                                return Ok(());
                            }
                        }
                        let img = output::apply_orientation(edit::render_with(cam, &job.edit, job.preset.as_ref(), 1.0, seed)?, *o);
                        alive = send(Ok(PreviewOut::Full { image: to_color(&img), before: false }));
                        full_after = Some((job.path.clone(), key.clone()));
                        Ok(())
                    })();
                    if let Err(e) = result {
                        alive = send(Err(e));
                    }
                    alive
                }
            };
            if !alive {
                break;
            }
        }
    });
    (job_tx, res_rx)
}

// --------------------------------------------------------------- thumbnails

/// What the thumbnail worker reports for a file.
enum ThumbMsg {
    /// Camera check: a warning if the file isn't from a Leica M9.
    Camera(Option<String>),
    Image(RgbImage),
}

/// Renders library thumbnails in the background; stops if the folder changes.
/// Each file's camera is checked first (from metadata, so it's quick).
fn spawn_thumbnails(ctx: egui::Context, files: Vec<PathBuf>, generation: Arc<AtomicU64>, tx: Sender<(u64, PathBuf, ThumbMsg)>) {
    let my_gen = generation.load(Ordering::SeqCst);
    thread::spawn(move || {
        for path in files {
            if generation.load(Ordering::SeqCst) != my_gen {
                return;
            }
            if let Ok(warning) = raw::check_camera(&path) {
                let _ = tx.send((my_gen, path.clone(), ThumbMsg::Camera(warning)));
            }
            let e = edit::load(&path).unwrap_or_default();
            if let Ok(img) = edit::render_file(&path, &e, Quality::Half, Some(THUMB_EDGE)) {
                let _ = tx.send((my_gen, path, ThumbMsg::Image(img)));
            }
            ctx.request_repaint();
        }
    });
}

// -------------------------------------------------------------------- batch

/// A running export job, shared with its worker thread.
struct Batch {
    total: usize,
    done: Arc<AtomicUsize>,
    cancel: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    failures: Arc<Mutex<Vec<String>>>,
    out_dir_label: String,
}

fn spawn_batch(ctx: egui::Context, files: Vec<PathBuf>, out_dir: Option<PathBuf>, opts: ExportOptions) -> Batch {
    let done = Arc::new(AtomicUsize::new(0));
    let cancel = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let failures = Arc::new(Mutex::new(Vec::new()));
    let out_dir_label = match (&out_dir, files.first()) {
        (Some(d), _) => d.display().to_string(),
        (None, Some(f)) => f.parent().unwrap_or(Path::new(".")).join("exports").display().to_string(),
        _ => String::new(),
    };
    let batch = Batch { total: files.len(), done: done.clone(), cancel: cancel.clone(), finished: finished.clone(), failures: failures.clone(), out_dir_label };
    thread::spawn(move || {
        for path in files {
            if cancel.load(Ordering::SeqCst) {
                break;
            }
            let dir = out_dir.clone().unwrap_or_else(|| path.parent().unwrap_or(Path::new(".")).join("exports"));
            let e = edit::load(&path).unwrap_or_default();
            if let Err(err) = edit::export_jpeg(&path, &e, &dir, &opts) {
                failures.lock().expect("not poisoned").push(format!("{}: {err:#}", file_name(&path)));
            }
            done.fetch_add(1, Ordering::SeqCst);
            ctx.request_repaint();
        }
        finished.store(true, Ordering::SeqCst);
        ctx.request_repaint();
    });
    batch
}

// ---------------------------------------------------------------------- app

#[derive(Default)]
struct History {
    undo: Vec<Edit>,
    redo: Vec<Edit>,
}

/// Keyboard shortcuts read this frame.
struct Keys {
    left: bool,
    right: bool,
    before: bool,
    esc: bool,
    zoom: bool,
    all: bool,
    copy: bool,
    paste: bool,
    paste_text: Option<String>,
    undo: bool,
    redo: bool,
}

/// Settings as placed on the system clipboard (tagged so random text
/// on the clipboard is never mistaken for settings).
#[derive(Serialize, Deserialize)]
struct ClipboardSettings {
    /// Also reads settings copied by Kodachromer (the app's former name).
    #[serde(alias = "kodachromer_settings")]
    remjet_settings: Edit,
}

/// Human-readable summary of what differs between two edits.
fn describe_change(a: &Edit, b: &Edit) -> String {
    let mut names = Vec::new();
    let mut check = |changed: bool, name: &'static str| {
        if changed {
            names.push(name);
        }
    };
    check(a.preset != b.preset, "film");
    check(a.output != b.output, "output mode");
    check(a.push != b.push, "development");
    check(a.exposure_ev != b.exposure_ev, "exposure");
    check(a.warmth != b.warmth, "warmth");
    check(a.tint != b.tint, "tint");
    check(a.highlights != b.highlights, "highlights");
    check(a.fit_highlights != b.fit_highlights, "highlight fit");
    check(a.scanner_colour != b.scanner_colour, "scanner colour");
    check(a.halation != b.halation, "halation");
    check(a.sharpen != b.sharpen, "sharpen");
    check(a.grain != b.grain, "grain");
    check(a.grain_size != b.grain_size, "grain size");
    match names.len() {
        0 => "no change".into(),
        1..=3 => names.join(", "),
        n => format!("{n} settings"),
    }
}

struct FilmApp {
    settings: Settings,
    folder: Option<PathBuf>,
    files: Vec<PathBuf>,
    /// The image shown in the editor.
    selected: Option<usize>,
    /// Multi-selection for paste / export (always contains `selected`).
    selection: BTreeSet<usize>,
    edited: HashSet<PathBuf>,
    clipboard: Option<Edit>,

    edit: Edit,
    saved_edit: Edit,
    last_requested: Option<(PathBuf, RenderKey, ModeKey)>,

    /// (id, name) of available film presets.
    preset_list: Vec<(String, String)>,
    tune: Option<Tune>,
    new_preset_name: String,
    /// Reference image shown beside the preview: (file name, texture).
    reference: Option<(String, egui::TextureHandle)>,

    preview_tx: Sender<PreviewJob>,
    preview_rx: Receiver<PreviewResult>,
    fit_after: Option<egui::TextureHandle>,
    fit_before: Option<egui::TextureHandle>,
    fit_path: Option<PathBuf>,
    zoom: Option<View>,
    /// Full-resolution render (and the edit it shows), for zooming/panning on the GPU.
    full_tex: Option<(egui::TextureHandle, RenderKey)>,
    full_before_tex: Option<egui::TextureHandle>,
    /// Quick render of the visible area for the newest edit: (texture, upright rect, key).
    patch: Option<(egui::TextureHandle, [f32; 4], RenderKey)>,
    /// Upright full-resolution size of the current image.
    full_size: Option<[usize; 2]>,
    /// Visible part of the image in zoom view (upright 0..1), for patch requests.
    visible: [f32; 4],
    rendering: bool,
    show_before: bool,

    thumbs: HashMap<PathBuf, egui::TextureHandle>,
    thumb_gen: Arc<AtomicU64>,
    thumb_tx: Sender<(u64, PathBuf, ThumbMsg)>,
    thumb_rx: Receiver<(u64, PathBuf, ThumbMsg)>,
    /// Files that aren't from a Leica M9, with the warning to show.
    camera_warnings: HashMap<PathBuf, String>,

    batch: Option<Batch>,
    status: String,
    show_about: bool,
    /// Per-image undo/redo history for this session.
    history: HashMap<PathBuf, History>,
    /// Short confirmation shown over the preview: (message, shown at).
    toast: Option<(String, std::time::Instant)>,
}

impl FilmApp {
    fn new(cc: &eframe::CreationContext<'_>, start_folder: Option<PathBuf>) -> Self {
        // wgpu's default reaction to a validation error is to panic. Some window
        // managers / tools briefly report nonsense window sizes during a resize
        // (e.g. a negative height), which fails surface configuration; log it
        // and carry on — the next valid resize reconfigures the surface.
        if let Some(rs) = &cc.wgpu_render_state {
            rs.device.on_uncaptured_error(Arc::new(|err| eprintln!("GPU error ignored: {err}")));
        }
        theme::install(&cc.egui_ctx);
        let (preview_tx, preview_rx) = spawn_preview_worker(cc.egui_ctx.clone());
        let (thumb_tx, thumb_rx) = channel();
        let settings = Settings::load();
        let mut app = Self {
            folder: None,
            files: Vec::new(),
            selected: None,
            selection: BTreeSet::new(),
            edited: HashSet::new(),
            clipboard: None,
            edit: Edit::default(),
            saved_edit: Edit::default(),
            last_requested: None,
            preset_list: presets::list(),
            tune: None,
            new_preset_name: String::new(),
            reference: None,
            preview_tx,
            preview_rx,
            fit_after: None,
            fit_before: None,
            fit_path: None,
            zoom: None,
            full_tex: None,
            full_before_tex: None,
            patch: None,
            full_size: None,
            visible: [0.0, 0.0, 1.0, 1.0],
            rendering: false,
            show_before: false,
            thumbs: HashMap::new(),
            thumb_gen: Arc::new(AtomicU64::new(0)),
            thumb_tx,
            thumb_rx,
            camera_warnings: HashMap::new(),
            batch: None,
            show_about: false,
            history: HashMap::new(),
            toast: None,
            status: "Open a folder of M9 DNGs to begin.".into(),
            settings,
        };
        let folder = start_folder.or_else(|| app.settings.last_folder.clone().filter(|p| p.is_dir()));
        if let Some(folder) = folder {
            app.open_folder(&cc.egui_ctx, folder);
        }
        app
    }

    fn open_folder(&mut self, ctx: &egui::Context, folder: PathBuf) {
        self.save_if_changed();
        let mut files: Vec<PathBuf> = std::fs::read_dir(&folder)
            .map(|rd| {
                rd.filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| raw::is_raw_file(p))
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        self.edited = files.iter().filter(|p| edit::has_edit(p)).cloned().collect();
        // Loading imports any old sidecar files beside the DNGs into the edit
        // store (and removes them), keeping the photo folder clean.
        let imported = self.edited.iter().filter(|p| edit::has_legacy_sidecar(p)).filter(|p| edit::load(p).is_some()).count();
        if imported > 0 {
            self.show_toast(format!("Moved {imported} edit file(s) out of the photo folder into Remjet's edit store"));
        }
        self.status = format!("{} raw files in {}", files.len(), folder.display());
        self.thumbs.clear();
        self.camera_warnings.clear();
        self.thumb_gen.fetch_add(1, Ordering::SeqCst);
        spawn_thumbnails(ctx.clone(), files.clone(), self.thumb_gen.clone(), self.thumb_tx.clone());
        self.files = files;
        self.settings.last_folder = Some(folder.clone());
        self.settings.save();
        self.folder = Some(folder);
        self.selected = None;
        self.selection.clear();
        self.fit_after = None;
        self.fit_before = None;
        if !self.files.is_empty() {
            self.focus(0);
        }
    }

    /// Make `index` the image being edited (and the only selected one).
    fn focus(&mut self, index: usize) {
        if index >= self.files.len() {
            return;
        }
        self.selection = BTreeSet::from([index]);
        if self.selected == Some(index) {
            return;
        }
        self.save_if_changed();
        self.selected = Some(index);
        self.zoom = None;
        self.full_tex = None;
        self.full_before_tex = None;
        self.patch = None;
        self.full_size = None;
        self.edit = edit::load(&self.files[index]).unwrap_or_default();
        self.saved_edit = self.edit.clone();
    }

    fn click_in_library(&mut self, index: usize, mods: egui::Modifiers) {
        if mods.command {
            if !self.selection.remove(&index) {
                self.selection.insert(index);
            }
            if self.selection.is_empty() {
                self.focus(index);
            }
        } else if mods.shift {
            let anchor = self.selected.unwrap_or(index);
            self.selection = (anchor.min(index)..=anchor.max(index)).collect();
        } else {
            self.focus(index);
        }
    }

    fn current_path(&self) -> Option<&PathBuf> {
        self.selected.and_then(|i| self.files.get(i))
    }

    /// Commit the current edit: save it and record the previous state as an
    /// undo step (one step per slider release / button press).
    fn save_if_changed(&mut self) {
        let Some(path) = self.current_path().cloned() else { return };
        if self.edit == self.saved_edit {
            return;
        }
        match self.persist(&path, &self.edit.clone()) {
            Ok(()) => {
                let h = self.history.entry(path).or_default();
                h.undo.push(self.saved_edit.clone());
                h.redo.clear();
                self.saved_edit = self.edit.clone();
            }
            Err(e) => self.status = format!("Could not save edit: {e:#}"),
        }
    }

    /// Store an edit; an edit equal to the defaults is removed instead, so the
    /// image counts as unedited again.
    fn persist(&mut self, path: &Path, e: &Edit) -> anyhow::Result<()> {
        if *e == Edit::default() {
            edit::remove(path)?;
            self.edited.remove(path);
        } else {
            edit::save(path, e)?;
            self.edited.insert(path.to_path_buf());
        }
        Ok(())
    }

    fn can_undo(&self) -> bool {
        self.current_path().and_then(|p| self.history.get(p)).is_some_and(|h| !h.undo.is_empty())
    }

    fn can_redo(&self) -> bool {
        self.current_path().and_then(|p| self.history.get(p)).is_some_and(|h| !h.redo.is_empty())
    }

    /// Step the current image's history back (`redo = false`) or forward.
    fn step_history(&mut self, redo: bool) {
        self.save_if_changed();
        let Some(path) = self.current_path().cloned() else { return };
        let Some(h) = self.history.get_mut(&path) else { return };
        let (from, to) = if redo { (&mut h.redo, &mut h.undo) } else { (&mut h.undo, &mut h.redo) };
        let Some(target) = from.pop() else {
            self.show_toast(if redo { "Nothing to redo".into() } else { "Nothing to undo".into() });
            return;
        };
        to.push(self.saved_edit.clone());
        let what = describe_change(&self.saved_edit, &target);
        if let Err(e) = self.persist(&path, &target) {
            self.status = format!("Could not save edit: {e:#}");
        }
        self.edit = target.clone();
        self.saved_edit = target;
        self.show_toast(format!("{} {what}", if redo { "Redo:" } else { "Undo:" }));
    }

    fn copy_settings(&mut self, ctx: &egui::Context) {
        self.save_if_changed();
        self.clipboard = Some(self.edit.clone());
        // Also on the system clipboard, so settings survive a restart.
        if let Ok(json) = serde_json::to_string(&ClipboardSettings { remjet_settings: self.edit.clone() }) {
            ctx.copy_text(json);
        }
        let name = self.current_path().map(|p| file_name(p)).unwrap_or_default();
        self.show_toast(format!("Copied settings from {name} — select images and press Ctrl+V to paste"));
    }

    fn paste_settings(&mut self, ctx: &egui::Context, from_text: Option<&str>) {
        // Prefer settings pasted from the system clipboard (e.g. copied in an
        // earlier session); fall back to the in-app clipboard.
        let parsed = from_text.and_then(|t| serde_json::from_str::<ClipboardSettings>(t.trim()).ok()).map(|c| c.remjet_settings);
        let Some(clip) = parsed.or_else(|| self.clipboard.clone()) else {
            self.show_toast("Nothing to paste — copy an image's settings first (Ctrl+C)".into());
            return;
        };
        self.save_if_changed();
        let targets: Vec<PathBuf> = self.selection.iter().filter_map(|&i| self.files.get(i).cloned()).collect();
        let mut failed = 0;
        for path in &targets {
            let previous = edit::load(path).unwrap_or_default();
            if previous == clip {
                continue;
            }
            match self.persist(path, &clip) {
                Ok(()) => {
                    let h = self.history.entry(path.clone()).or_default();
                    h.undo.push(previous);
                    h.redo.clear();
                }
                Err(_) => failed += 1,
            }
        }
        if self.selected.is_some_and(|i| self.selection.contains(&i)) {
            self.edit = clip.clone();
            self.saved_edit = clip;
        }
        // Refresh the pasted thumbnails.
        spawn_thumbnails(ctx.clone(), targets.clone(), self.thumb_gen.clone(), self.thumb_tx.clone());
        let n = targets.len();
        self.show_toast(match failed {
            0 if n == 1 => format!("Pasted settings to {} (Ctrl+Z to undo)", file_name(&targets[0])),
            0 => format!("Pasted settings to {n} images (Ctrl+Z undoes it per image)"),
            f => format!("Pasted to {} of {n} images; {f} could not be saved", n - f),
        });
    }

    fn show_toast(&mut self, message: String) {
        self.toast = Some((message, std::time::Instant::now()));
    }

    fn start_export(&mut self, ctx: &egui::Context, files: Vec<PathBuf>) {
        if files.is_empty() || self.batch.is_some() {
            return;
        }
        self.save_if_changed();
        self.settings.save();
        self.batch = Some(spawn_batch(ctx.clone(), files, self.settings.export_dir.clone(), self.settings.export.clone()));
    }

    fn request_preview(&mut self) {
        let Some(path) = self.current_path().cloned() else { return };
        // Preview unsaved tuning only while it applies to this image's film.
        let preset = self
            .tune
            .as_ref()
            .filter(|t| Some(&t.id) == self.edit.preset.as_ref() && t.working != t.saved)
            .map(|t| t.working.clone());
        let render_key: RenderKey = (self.edit.clone(), preset.clone());
        let mode_key = match self.zoom {
            None => ModeKey::Fit,
            Some(_) => ModeKey::Detail { before: self.show_before },
        };
        let key = (path.clone(), render_key, mode_key.clone());
        if self.last_requested.as_ref() == Some(&key) {
            return;
        }
        self.last_requested = Some(key);
        self.rendering = true;
        let mode = match mode_key {
            ModeKey::Fit => Mode::Fit,
            ModeKey::Detail { before } => Mode::Detail { region: self.visible, before },
        };
        let _ = self.preview_tx.send(PreviewJob { path, edit: self.edit.clone(), preset, mode });
    }

    /// The render key the screen should currently show.
    fn current_key(&self) -> Option<&RenderKey> {
        self.last_requested.as_ref().map(|k| &k.1)
    }

    fn poll_workers(&mut self, ctx: &egui::Context) {
        while let Ok(res) = self.preview_rx.try_recv() {
            if self.current_path() != Some(&res.path) {
                continue;
            }
            let latest = self.current_key() == Some(&res.key);
            let lin = egui::TextureOptions::LINEAR;
            // Full-res: crisp pixels when magnified, mipmapped when zoomed out.
            let full_opts = egui::TextureOptions {
                magnification: egui::TextureFilter::Nearest,
                minification: egui::TextureFilter::Linear,
                mipmap_mode: Some(egui::TextureFilter::Linear),
                ..lin
            };
            match res.out {
                Ok(PreviewOut::Fit { after, before, thumb, full_size, camera_warning }) => {
                    self.set_camera_warning(&res.path, camera_warning);
                    self.thumbs.insert(res.path.clone(), ctx.load_texture(thumb_name(&res.path), thumb, lin));
                    self.fit_after = Some(ctx.load_texture("after", after, lin));
                    self.fit_before = Some(ctx.load_texture("before", before, lin));
                    self.fit_path = Some(res.path.clone());
                    self.full_size = Some(full_size);
                    if latest {
                        self.rendering = false;
                    }
                }
                Ok(PreviewOut::Patch { image, region }) if latest => {
                    self.patch = Some((ctx.load_texture("patch", image, full_opts), region, res.key));
                }
                Ok(PreviewOut::Full { image, before: false }) => {
                    self.full_tex = Some((ctx.load_texture("full", image, full_opts), res.key.clone()));
                    if self.patch.as_ref().is_some_and(|p| p.2 == res.key) {
                        self.patch = None;
                    }
                    if latest {
                        self.rendering = false;
                    }
                }
                Ok(PreviewOut::Full { image, before: true }) => {
                    self.full_before_tex = Some(ctx.load_texture("full-before", image, full_opts));
                    self.rendering = false;
                }
                Ok(PreviewOut::Done) => {
                    if latest {
                        self.rendering = false;
                    }
                }
                Ok(PreviewOut::Patch { .. }) => {}
                Err(e) => {
                    self.status = format!("Could not render {}: {e}", file_name(&res.path));
                    self.rendering = false;
                }
            }
        }
        let generation = self.thumb_gen.load(Ordering::SeqCst);
        while let Ok((g, path, msg)) = self.thumb_rx.try_recv() {
            if g != generation {
                continue;
            }
            match msg {
                ThumbMsg::Camera(warning) => self.set_camera_warning(&path, warning),
                ThumbMsg::Image(img) => {
                    let tex = ctx.load_texture(thumb_name(&path), to_color(&img), egui::TextureOptions::LINEAR);
                    self.thumbs.insert(path, tex);
                }
            }
        }
        if let Some(b) = &self.batch {
            if b.finished.load(Ordering::SeqCst) {
                let failures = b.failures.lock().expect("not poisoned").clone();
                let done = b.done.load(Ordering::SeqCst);
                let cancelled = b.cancel.load(Ordering::SeqCst);
                self.status = match (cancelled, failures.len()) {
                    (true, _) => format!("Export cancelled after {done} of {}.", b.total),
                    (false, 0) => format!("Exported {done} JPG(s) to {}", b.out_dir_label),
                    (false, n) => format!("Exported {} of {}; {n} failed: {}", done - n, b.total, failures.join("; ")),
                };
                self.batch = None;
            }
        }
    }

    fn set_camera_warning(&mut self, path: &Path, warning: Option<String>) {
        match warning {
            Some(w) => {
                self.camera_warnings.insert(path.to_path_buf(), w);
            }
            None => {
                self.camera_warnings.remove(path);
            }
        }
    }

    /// Fit <-> 100% at the centre of what's shown.
    fn toggle_100(&mut self) {
        self.zoom = match self.zoom {
            Some(_) => None,
            None => Some(View { level: 1.0, center: [0.5, 0.5] }),
        };
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let keys = ctx.input(|i| {
            let cmd = i.modifiers.command;
            let pressed = |k| i.key_pressed(k);
            // Windows turns Ctrl+C / Ctrl+V into clipboard events, and may or
            // may not also report the key itself: accept either.
            let paste_text = i.events.iter().find_map(|e| match e {
                egui::Event::Paste(t) => Some(t.clone()),
                _ => None,
            });
            Keys {
                left: pressed(egui::Key::ArrowLeft),
                right: pressed(egui::Key::ArrowRight),
                before: i.key_down(egui::Key::B),
                esc: pressed(egui::Key::Escape),
                zoom: !cmd && pressed(egui::Key::Z),
                all: cmd && pressed(egui::Key::A),
                copy: i.events.iter().any(|e| matches!(e, egui::Event::Copy)) || (cmd && pressed(egui::Key::C)),
                paste: paste_text.is_some() || (cmd && pressed(egui::Key::V)),
                paste_text,
                undo: cmd && !i.modifiers.shift && pressed(egui::Key::Z),
                redo: cmd && (pressed(egui::Key::Y) || (i.modifiers.shift && pressed(egui::Key::Z))),
            }
        });
        let Keys { left, right, before, esc, zoom: z, all, copy, paste, paste_text, undo, redo } = keys;
        if undo {
            self.step_history(false);
        }
        if redo {
            self.step_history(true);
        }
        if let Some(sel) = self.selected {
            if left && sel > 0 {
                self.focus(sel - 1);
            }
            if right {
                self.focus(sel + 1);
            }
        }
        if esc {
            self.zoom = None;
        }
        if z {
            self.toggle_100();
        }
        if all {
            self.selection = (0..self.files.len()).collect();
            self.show_toast(format!("Selected all {} images", self.files.len()));
        }
        if copy {
            self.copy_settings(ctx);
        }
        if paste {
            self.paste_settings(ctx, paste_text.as_deref());
        }
        self.show_before = before;
    }

    // ----------------------------------------------------------- panels

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Open folder…").clicked() {
                let mut dialog = rfd::FileDialog::new();
                if let Some(f) = &self.folder {
                    dialog = dialog.set_directory(f);
                }
                if let Some(folder) = dialog.pick_folder() {
                    let ctx = ui.ctx().clone();
                    self.open_folder(&ctx, folder);
                }
            }
            if ui.button("Reference…").on_hover_text("Show a reference image (e.g. a real K64 scan) beside the preview").clicked() {
                let ctx = ui.ctx().clone();
                self.pick_reference(&ctx);
            }
            if self.reference.is_some() && ui.button("Hide reference").clicked() {
                self.reference = None;
            }
            ui.separator();
            if self.rendering {
                ui.spinner();
            }
            ui.label(egui::RichText::new(&self.status).color(theme::TEXT_WEAK));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("About").clicked() {
                    self.show_about = !self.show_about;
                }
                ui.label(egui::RichText::new(format!("Remjet {VERSION}")).small().color(theme::TEXT_WEAK));
            });
        });
        let mut open = self.show_about;
        egui::Window::new("About Remjet").open(&mut open).collapsible(false).resizable(false).default_width(420.0).show(ui.ctx(), |ui| {
            ui.heading(format!("Remjet {VERSION}"));
            ui.label("Film emulation for Leica M9 DNGs, starting with Kodachrome 64 (scanned-slide look).");
            ui.label(egui::RichText::new("Alpha software: expect rough edges. Your DNGs are never modified, and nothing is written into your photo folders: edits are kept in Remjet's own data folder.").small());
            ui.add_space(6.0);
            ui.hyperlink_to("Project page, issues and source", REPO_URL);
            ui.add_space(6.0);
            ui.label(egui::RichText::new("Licensed under the GNU GPL v3 or later. Raw decoding by rawler (LGPL-2.1).").small());
            ui.label(
                egui::RichText::new(
                    "Kodachrome and Kodak are trademarks of Eastman Kodak Company. Remjet is an independent project, \
                     not affiliated with or endorsed by Kodak or Leica. Film curves are digitised from Kodak's published \
                     technical data (E-55) for emulation purposes.",
                )
                .small()
                .weak(),
            );
        });
        self.show_about = open;
    }

    fn library(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new().inner_margin(egui::Margin { left: 14, right: 14, top: 10, bottom: 6 }).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Library");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let count = if self.selection.len() > 1 {
                        format!("{} of {} selected", self.selection.len(), self.files.len())
                    } else {
                        format!("{} frames", self.files.len())
                    };
                    ui.label(egui::RichText::new(count).small().color(theme::TEXT_WEAK));
                });
            });
            theme::hint(ui, "Ctrl- or Shift-click to select several");
            if !self.camera_warnings.is_empty() {
                let n = self.camera_warnings.len();
                ui.label(egui::RichText::new(format!("⚠ {n} not from a Leica M9")).small().color(theme::WARN)).on_hover_text(
                    "Remjet's film looks were tuned on Leica M9 files. Files from other cameras still open, with grain and halation scaled to their sensor, but colours may differ.",
                );
            }
        });
        let mut clicked = None;
        let nav = ui.input(|i| i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::ArrowRight));
        egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for (i, path) in self.files.iter().enumerate() {
                let resp = film_frame(
                    ui,
                    i,
                    path,
                    self.thumbs.get(path),
                    FrameState {
                        focused: self.selected == Some(i),
                        selected: self.selection.contains(&i),
                        edited: self.edited.contains(path),
                        warning: self.camera_warnings.get(path).map(String::as_str),
                    },
                );
                if resp.clicked() {
                    clicked = Some((i, ui.input(|inp| inp.modifiers)));
                }
                if self.selected == Some(i) && nav {
                    resp.scroll_to_me(None);
                }
            }
        });
        if let Some((i, mods)) = clicked {
            self.click_in_library(i, mods);
        }
    }

    fn edit_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        theme::section(ui, "Film");
        if self.selected.is_none() {
            theme::hint(ui, "Choose a frame in the library to edit it.");
            return;
        }

        let current = match &self.edit.preset {
            None => "None (neutral)".to_string(),
            Some(id) => self.preset_list.iter().find(|(i, _)| i == id).map_or(format!("{id} (missing)"), |p| p.1.clone()),
        };
        row(ui, "Stock", |ui| {
            egui::ComboBox::from_id_salt("film").width(ui.available_width()).selected_text(current).show_ui(ui, |ui| {
                for (id, name) in &self.preset_list {
                    ui.selectable_value(&mut self.edit.preset, Some(id.clone()), name);
                }
                ui.selectable_value(&mut self.edit.preset, None, "None (neutral)");
            });
        });
        // Colour negatives are lab-scanned or printed on their paper; slides
        // are scanned or projected.
        let loaded = self.edit.preset.as_deref().and_then(|id| presets::load(id).ok());
        let mono = loaded.as_ref().is_some_and(|p| p.is_monochrome());
        let developments = loaded.as_ref().map(|p| p.developments()).unwrap_or_default();
        let print = loaded.filter(|p| p.is_negative()).and_then(|p| p.print);
        ui.add_enabled_ui(self.edit.preset.is_some(), |ui| {
            let negative = print.is_some() || mono;
            let paper = print.as_ref().map(|p| p.name.clone()).unwrap_or_default();
            let label = |m: OutputMode| match (negative, m) {
                (true, OutputMode::Print) if !mono => format!("Optical print on {paper}"),
                (true, _) => "Lab scan of the negative".to_string(),
                (false, OutputMode::Projection) => "Projected slide".to_string(),
                (false, _) => "Scanned slide".to_string(),
            };
            let info = if mono {
                OUTPUT_INFO_MONO
            } else if negative {
                OUTPUT_INFO_NEGATIVE
            } else {
                OUTPUT_INFO
            };
            row_info(ui, "Output", Some(info), |ui| {
                egui::ComboBox::from_id_salt("output").width(ui.available_width()).selected_text(label(self.edit.output)).show_ui(ui, |ui| {
                    if mono {
                        // Black-and-white: Kodak publishes no paper data to print on.
                        ui.selectable_value(&mut self.edit.output, OutputMode::Scan, label(OutputMode::Scan))
                            .on_hover_text("As a lab scanner reads the negative: soft, with the negative's long highlight latitude.");
                    } else if negative {
                        ui.selectable_value(&mut self.edit.output, OutputMode::Scan, label(OutputMode::Scan))
                            .on_hover_text("As a minilab scanner reads the negative: soft, with the negative's long highlight latitude.");
                        ui.selectable_value(&mut self.edit.output, OutputMode::Print, label(OutputMode::Print))
                            .on_hover_text("Printed through the paper's own curves and dyes from Kodak's data: richer colour, shorter highlights.");
                    } else {
                        ui.selectable_value(&mut self.edit.output, OutputMode::Scan, label(OutputMode::Scan))
                            .on_hover_text("As a film scanner reads the slide: open shadows, scanner balance.");
                        ui.selectable_value(&mut self.edit.output, OutputMode::Projection, label(OutputMode::Projection))
                            .on_hover_text("As the slide looks projected: full density range, deep blacks, warm lamp, soft corner falloff.");
                    }
                });
            });
        });
        // Push and pull: only the processes the film's datasheet lists.
        if !developments.is_empty() {
            if !developments.iter().any(|d| d.0 == self.edit.push) {
                self.edit.push = 0;
            }
            let name =
                |push: i32| developments.iter().find(|d| d.0 == push).map_or("Normal".to_string(), |d| d.1.clone());
            row_info(ui, "Development", Some(DEVELOPMENT_INFO), |ui| {
                egui::ComboBox::from_id_salt("development").width(ui.available_width()).selected_text(name(self.edit.push)).show_ui(ui, |ui| {
                    let mut all: Vec<i32> = developments.iter().map(|d| d.0).chain([0]).collect();
                    all.sort();
                    for push in all {
                        ui.selectable_value(&mut self.edit.push, push, name(push));
                    }
                });
            });
        }

        theme::section(ui, "Adjust");
        let d = Edit::default();
        let film = self.edit.preset.is_some();
        let e = &mut self.edit;
        slider(ui, true, &mut e.exposure_ev, -2.0..=2.0, 0.05, "Exposure", d.exposure_ev, None);
        slider(ui, true, &mut e.warmth, -1.0..=1.0, 0.01, "Warmth", d.warmth, None);
        slider(ui, true, &mut e.tint, -1.0..=1.0, 0.01, "Tint", d.tint, None);
        slider(ui, film, &mut e.highlights, -1.0..=1.0, 0.01, "Highlights", d.highlights, Some("How the film's highlights roll off. Left keeps more detail in bright areas; right makes them brighter, so they blow out sooner."));
        ui.horizontal(|ui| {
            ui.add_space(LABEL_WIDTH + ui.spacing().item_spacing.x);
            ui.add_enabled(film, egui::Checkbox::new(&mut e.fit_highlights, "Fit camera highlights to film"));
            theme::info(
                ui,
                "A camera records about three stops above mid-grey before it clips, while the film's curve runs further. \
                 On: the camera's brightest tone is placed on the film's white, so the top stop keeps its detail and whites look white. \
                 Off: the Kodak datasheet curve exactly as published, which can leave the brightest tones a dull light grey.",
            );
        });
        ui.horizontal(|ui| {
            ui.add_space(LABEL_WIDTH + ui.spacing().item_spacing.x);
            ui.add_enabled(film, egui::Checkbox::new(&mut e.scanner_colour, "Scanner colour"));
            theme::info(
                ui,
                "Film layers see colour through broad, overlapping sensitivities, so a scanner's raw red, green and blue readings are washed out. \
                 On: the readings are calibrated back to true colour, as a lab scanner (Frontier, Noritsu) does. Applies to scanned negatives and Kodachrome. \
                 Off: the raw readings, with flatter colour, most visibly in bright saturated colours such as yellows.",
            );
        });
        slider(ui, film, &mut e.halation, 0.0..=1.0, 0.01, "Halation", d.halation, Some("The red-orange glow film shows around bright lights, from light that passes through the film, reflects off its back and exposes it again. 0 is off."));
        slider(ui, true, &mut e.sharpen, 0.0..=1.0, 0.01, "Sharpen", d.sharpen, Some("Detail sharpening like a film scanner's. It works on brightness only, so it doesn't add colour fringes."));
        slider(ui, film, &mut e.grain, 0.0..=4.0, 0.05, "Grain", d.grain, None);
        slider(ui, film, &mut e.grain_size, 0.5..=4.0, 0.05, "Grain size", d.grain_size, Some("Size of the grain clumps, relative to the film's own. Grain is sized on the film, so it looks the same at any export size."));
        theme::hint(ui, "Double-click a slider to reset it.");
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.add_enabled(self.can_undo(), egui::Button::new("Undo")).on_hover_text("Ctrl+Z").clicked() {
                self.step_history(false);
            }
            if ui.add_enabled(self.can_redo(), egui::Button::new("Redo")).on_hover_text("Ctrl+Y or Ctrl+Shift+Z").clicked() {
                self.step_history(true);
            }
            if ui.button("Reset all").on_hover_text("Back to the film's defaults; the image no longer counts as edited").clicked() {
                self.edit = Edit::default();
                self.save_if_changed();
                self.show_toast("Reset all settings (Ctrl+Z to undo)".into());
            }
        });

        ui.add_space(6.0);
        self.tune_section(ui);

        theme::section(ui, "Copy settings");
        ui.horizontal(|ui| {
            if ui.button("Copy").on_hover_text("Ctrl+C").clicked() {
                self.copy_settings(&ctx);
            }
            let n = self.selection.len();
            let target = if n == 1 { "Paste to this frame".to_string() } else { format!("Paste to {n} selected") };
            let paste = ui
                .add_enabled(self.clipboard.is_some(), egui::Button::new(target))
                .on_hover_text("Ctrl+V. Select frames with Ctrl/Shift-click or Ctrl+A.");
            if paste.clicked() {
                self.paste_settings(&ctx, None);
            }
        });

        self.export_section(ui);
        ui.add_space(14.0);
    }

    /// The strip under the preview: compare and zoom, with their shortcuts.
    fn view_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            let enabled = self.selected.is_some();
            ui.add_enabled_ui(enabled, |ui| {
                let before = ui
                    .add(egui::Button::new("Hold for before").selected(self.show_before).sense(egui::Sense::click_and_drag()))
                    .on_hover_text("Or hold B: shows the frame without the film look");
                if before.is_pointer_button_down_on() {
                    self.show_before = true;
                }
                let fit = self.zoom.is_none();
                if ui.add(egui::Button::new("Fit").selected(fit)).on_hover_text("Esc").clicked() && !fit {
                    self.zoom = None;
                }
                if ui.add(egui::Button::new("100%").selected(self.zoom.is_some_and(|z| (z.level - 1.0).abs() < 0.01))).on_hover_text("Z").clicked() {
                    self.zoom = Some(View { level: 1.0, center: self.zoom.map_or([0.5, 0.5], |z| z.center) });
                }
            });
            if let Some(z) = self.zoom {
                ui.label(egui::RichText::new(format!("{:.0}%", z.level * 100.0)).color(theme::TEXT_WEAK));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new("Click the photo for 100%. Ctrl+scroll zooms, drag pans, ←/→ change frame.").small().color(theme::TEXT_WEAK));
            });
        });
    }

    /// Live tuning of the current film preset. Changes preview immediately and
    /// apply to every image using that film once saved.
    fn tune_section(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.edit.preset.clone() else { return };
        if self.tune.as_ref().is_none_or(|t| t.id != id) {
            match presets::load(&id) {
                Ok(p) => {
                    let reference = presets::is_builtin(&id).then(|| FilmPreset::builtin(&id).ok()).flatten();
                    self.tune = Some(Tune { id: id.clone(), working: p.clone(), saved: p, reference });
                }
                Err(e) => {
                    ui.label(format!("Cannot load film '{id}': {e:#}"));
                    return;
                }
            }
        }
        let ctx = ui.ctx().clone();
        let tune = self.tune.as_mut().expect("loaded above");
        let dirty = tune.working != tune.saved;
        let title = if dirty { format!("Tune {} (unsaved)", tune.working.name) } else { format!("Tune {}", tune.working.name) };
        let mut action = None;
        egui::CollapsingHeader::new(egui::RichText::new(title).font(egui::FontId::new(19.0, theme::condensed()))).id_salt("tune").show_unindented(ui, |ui| {
            // Unindented, so the tuning sliders share the Adjust label column.
            theme::hint(ui, "Changes the film itself, for every image that uses it. Double-click a slider for the Kodak-data value.");
            let w = &mut tune.working;
            let r = tune.reference.clone().unwrap_or_else(|| tune.saved.clone());
            // Base brightness shown as EV relative to the reference grey.
            let before = -(w.exposure.grey_linear / r.exposure.grey_linear).log2() as f32;
            let mut bright = before;
            slider(ui, true, &mut bright, -1.0..=1.0, 0.01, "Brightness", 0.0, Some("Which camera brightness the film treats as a mid-grey card, in stops from the Kodak value. Unlike Exposure, it changes the film for every image."));
            if (bright - before).abs() > 1e-6 {
                let grey = r.exposure.grey_linear * 2f64.powf(-bright as f64);
                w.exposure.grey_linear = (grey * 1e5).round() / 1e5;
            }
            slider64(ui, &mut w.look.contrast, 0.7..=1.4, "Contrast", r.look.contrast, Some("Steepness of the film's tone curve around mid-grey. Above 1 is more contrasty than Kodak's data, below 1 flatter."));
            let mono = w.is_monochrome();
            if !mono {
                slider64(ui, &mut w.look.interimage, 0.0..=0.6, "Saturation", r.look.interimage, Some("How strongly the film's colour layers hold each other back (interimage effect). This is what gives slide film its dense, saturated colour. 0 turns it off."));
            }
            slider64(ui, &mut w.scan.gamma, 0.8..=1.6, "Shadow lift", r.scan.gamma, Some("The scanner software's gamma. Above 1 opens up shadows the way scanning software does; 1 is the film's raw transmittance."));
            slider64(ui, &mut w.scan.black_density, 2.4..=3.8, "Black depth", r.scan.black_density, Some("The film density the scanner reads as pure black. Higher keeps detail in deep shadows but greyer blacks; lower crushes them to black sooner."));
            let negative = w.is_negative();
            let (colour_title, colour_info) = if mono {
                ("Toning", "Tints the black-and-white image, per channel. Above 1 adds that colour; all three at 1 is neutral grey.")
            } else if negative {
                ("Print / scan colour", "Colour balance of the scan or the printing light, per channel. Above 1 adds that colour.")
            } else {
                ("Scanner colour", "Colour balance the scanner applies, per channel. Above 1 adds that colour to the scan. Only affects Scanned slide output.")
            };
            subhead_info(ui, colour_title, colour_info);
            slider64(ui, &mut w.scan.white_balance[0], 0.85..=1.15, "Red", r.scan.white_balance[0], None);
            slider64(ui, &mut w.scan.white_balance[1], 0.85..=1.15, "Green", r.scan.white_balance[1], None);
            slider64(ui, &mut w.scan.white_balance[2], 0.85..=1.15, "Blue", r.scan.white_balance[2], None);
            subhead(ui, "Grain");
            slider64(ui, &mut w.grain.amount, 0.0..=1.2, "Strength", r.grain.amount, None);
            if !mono {
                let mut colour = 1.0 - w.grain.correlation;
                slider64(ui, &mut colour, 0.0..=1.0, "Colour grain", 1.0 - r.grain.correlation, Some("How much each colour layer's grain differs from the others. 0 is grain in brightness only; higher adds coloured speckle."));
                w.grain.correlation = 1.0 - colour;
            }
            if !negative && !mono {
                subhead_info(ui, "Projection", "How the slide looks on a screen. Only affects Projected slide output.");
                slider64(ui, &mut w.projection.flare, 0.0..=0.02, "Screen black", r.projection.flare, Some("Light from the room and screen that lands on the darkest parts of the picture, as a fraction of white. It sets how black a projected slide can get: 0.003 is about 330:1."));
                slider64(ui, &mut w.projection.falloff_stops, 0.0..=1.5, "Corner falloff", r.projection.falloff_stops, Some("How much darker the projector makes the corners than the centre, in stops."));
                slider64(ui, &mut w.projection.white_balance[0], 0.85..=1.15, "Lamp red", r.projection.white_balance[0], Some("Colour of the projector lamp. Tungsten lamps stay a little warm even after your eyes adapt."));
                slider64(ui, &mut w.projection.white_balance[2], 0.75..=1.15, "Lamp blue", r.projection.white_balance[2], None);
            }

            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(dirty, egui::Button::new("Save")).on_hover_text("Update this film for all images").clicked() {
                    action = Some(TuneAction::Save);
                }
                if ui.add_enabled(dirty, egui::Button::new("Discard")).clicked() {
                    action = Some(TuneAction::Discard);
                }
                if tune.reference.is_some() && presets::is_overridden(&tune.id) && ui.button("Restore Kodak defaults").clicked() {
                    action = Some(TuneAction::Restore);
                }
                if !presets::is_builtin(&tune.id) && ui.button("Delete film").clicked() {
                    action = Some(TuneAction::Delete);
                }
            });
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.new_preset_name).hint_text("New film name").desired_width(ui.available_width() - 100.0));
                if ui.add_enabled(!self.new_preset_name.trim().is_empty(), egui::Button::new("Save as new")).clicked() {
                    action = Some(TuneAction::SaveAs);
                }
            });
        });
        if let Some(a) = action {
            self.apply_tune_action(&ctx, a);
        }
    }

    fn apply_tune_action(&mut self, ctx: &egui::Context, action: TuneAction) {
        let Some(tune) = self.tune.as_mut() else { return };
        let result: anyhow::Result<String> = (|| match action {
            TuneAction::Save => {
                presets::save_user(&tune.working)?;
                tune.saved = tune.working.clone();
                Ok(format!("Saved film '{}'.", tune.working.name))
            }
            TuneAction::Discard => {
                tune.working = tune.saved.clone();
                Ok("Discarded changes.".into())
            }
            TuneAction::Restore => {
                presets::delete_user(&tune.id)?;
                let p = FilmPreset::builtin(&tune.id)?;
                tune.working = p.clone();
                tune.saved = p;
                Ok("Restored the Kodak-data defaults.".into())
            }
            TuneAction::SaveAs => {
                let name = self.new_preset_name.trim().to_string();
                let mut p = tune.working.clone();
                p.id = presets::new_id(&name);
                p.name = name;
                presets::save_user(&p)?;
                // Restore the original film and switch this image to the new one.
                tune.working = tune.saved.clone();
                self.edit.preset = Some(p.id.clone());
                self.new_preset_name.clear();
                Ok(format!("Created film '{}'.", p.name))
            }
            TuneAction::Delete => {
                presets::delete_user(&tune.id)?;
                self.edit.preset = presets::BUILTINS.first().map(|(id, _)| id.to_string());
                Ok("Deleted film.".into())
            }
        })();
        match result {
            Ok(msg) => self.status = msg,
            Err(e) => self.status = format!("Could not update film: {e:#}"),
        }
        if !matches!(action, TuneAction::Discard) {
            self.preset_list = presets::list();
            self.tune = None; // reload on next frame
            // Saved films change every image that uses them: refresh thumbnails.
            spawn_thumbnails(ctx.clone(), self.files.clone(), self.thumb_gen.clone(), self.thumb_tx.clone());
        }
        self.last_requested = None;
    }

    fn pick_reference(&mut self, ctx: &egui::Context) {
        let mut dialog = rfd::FileDialog::new().add_filter("Images", &["jpg", "jpeg", "png"]);
        if let Some(dir) = self.folder.as_ref().and_then(|f| f.parent()).map(|p| p.join("references")).filter(|p| p.is_dir()) {
            dialog = dialog.set_directory(dir);
        }
        let Some(path) = dialog.pick_file() else { return };
        match image::open(&path) {
            Ok(img) => {
                let img = img.to_rgb8();
                let tex = ctx.load_texture("reference", to_color(&img), egui::TextureOptions::LINEAR);
                self.reference = Some((file_name(&path), tex));
            }
            Err(e) => self.status = format!("Could not open reference: {e}"),
        }
    }

    fn export_section(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        theme::section(ui, "Export JPG");
        let opts = &mut self.settings.export;
        let size_label = SIZES.iter().find(|(s, _)| *s == opts.max_edge).map_or("Custom", |s| s.1);
        row(ui, "Size", |ui| {
            egui::ComboBox::from_id_salt("size").width(ui.available_width()).selected_text(size_label).show_ui(ui, |ui| {
                for (s, label) in SIZES {
                    ui.selectable_value(&mut opts.max_edge, *s, *label);
                }
            });
        });
        let mut q = opts.quality as f32;
        slider(ui, true, &mut q, 60.0..=100.0, 1.0, "Quality", 92.0, None);
        opts.quality = q.round() as _;
        ui.horizontal(|ui| {
            ui.add_space(LABEL_WIDTH + ui.spacing().item_spacing.x);
            ui.checkbox(&mut opts.keep_exif, "Keep camera metadata (EXIF)");
            theme::info(ui, "Copies the date, camera, lens and exposure settings from the DNG into the JPG. Off writes no metadata.");
        });

        let dest = match (&self.settings.export_dir, &self.folder) {
            (Some(d), _) => d.display().to_string(),
            (None, Some(f)) => f.join("exports").display().to_string(),
            _ => "No folder open".into(),
        };
        row(ui, "Folder", |ui| {
            ui.add(egui::Label::new(egui::RichText::new(&dest).small().color(theme::TEXT_WEAK)).truncate()).on_hover_text(&dest);
        });
        ui.horizontal(|ui| {
            ui.add_space(LABEL_WIDTH + ui.spacing().item_spacing.x);
            if ui.button("Change folder…").clicked() {
                if let Some(d) = rfd::FileDialog::new().pick_folder() {
                    self.settings.export_dir = Some(d);
                    self.settings.save();
                }
            }
            if self.settings.export_dir.is_some() && ui.button("Use default").clicked() {
                self.settings.export_dir = None;
                self.settings.save();
            }
        });
        ui.add_space(4.0);

        if let Some(b) = &self.batch {
            let done = b.done.load(Ordering::SeqCst);
            ui.add(egui::ProgressBar::new(done as f32 / b.total.max(1) as f32).text(format!("Exporting {done} / {}", b.total)));
            if ui.button("Cancel").clicked() {
                b.cancel.store(true, Ordering::SeqCst);
            }
            return;
        }

        let current: Vec<PathBuf> = self.current_path().cloned().into_iter().collect();
        let selected: Vec<PathBuf> = self.selection.iter().filter_map(|&i| self.files.get(i).cloned()).collect();
        let edited: Vec<PathBuf> = self.files.iter().filter(|p| self.edited.contains(*p)).cloned().collect();
        let all = self.files.clone();
        let mut job = None;
        // Two by two, equal widths: the export targets read as one set of choices.
        let gap = ui.spacing().item_spacing.x;
        let w = (ui.available_width() - gap) / 2.0;
        let h = ui.spacing().interact_size.y + 4.0;
        let mut target = |ui: &mut egui::Ui, files: Vec<PathBuf>, enabled: bool, label: String| {
            if ui.add_enabled(enabled, egui::Button::new(label).min_size(egui::vec2(w, h))).clicked() {
                job = Some(files);
            }
        };
        ui.horizontal(|ui| {
            target(ui, current.clone(), !current.is_empty(), "This frame".into());
            target(ui, selected.clone(), selected.len() > 1, format!("Selected ({})", selected.len()));
        });
        ui.horizontal(|ui| {
            target(ui, edited.clone(), !edited.is_empty(), format!("Edited ({})", edited.len()));
            target(ui, all.clone(), !all.is_empty(), format!("Whole folder ({})", all.len()));
        });
        if let Some(files) = job {
            self.start_export(&ctx, files);
        }
    }

    fn preview(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let ppp = ui.ctx().pixels_per_point();
        if self.selected.is_none() {
            ui.centered_and_justified(|ui| {
                ui.label(egui::RichText::new("Open a folder of DNGs to start: Open folder…, top left.").color(theme::TEXT_WEAK));
            });
            return;
        }

        let stale = self.fit_path.as_ref() != self.current_path();
        let (Some([fw, fh]), false) = (self.full_size, stale) else {
            ui.centered_and_justified(|ui| ui.spinner());
            return;
        };
        let (fw, fh) = (fw as f32, fh as f32);
        let fit_level = (rect.width() * ppp / fw).min(rect.height() * ppp / fh);

        let resp = ui.interact(rect, ui.scope_id().with("preview"), egui::Sense::click_and_drag());
        let pointer = resp.hover_pos();
        // Our own click test (press + release in the preview with little
        // movement): egui's drag-sensing click detection misses fast clicks.
        let click_pos = ui.input(|i| i.pointer.interact_pos());
        let clicked = ui.input(|i| i.pointer.primary_clicked()) && click_pos.is_some_and(|p| rect.contains(p)) && resp.hovered();
        let zoom_delta = if resp.hovered() { ui.input(|i| i.zoom_delta()) } else { 1.0 };

        // Current view (fit is just the lowest zoom level, centred).
        let mut view = self.zoom.unwrap_or(View { level: fit_level, center: [0.5, 0.5] });
        let size = |level: f32| egui::vec2(fw * level / ppp, fh * level / ppp);

        // Ctrl+scroll / pinch: zoom around the cursor.
        if (zoom_delta - 1.0).abs() > 1e-4 {
            let p = pointer.unwrap_or(rect.center());
            let s0 = size(view.level);
            let u = view.center[0] + (p.x - rect.center().x) / s0.x;
            let v = view.center[1] + (p.y - rect.center().y) / s0.y;
            let new_level = (view.level * zoom_delta).clamp(fit_level, MAX_ZOOM.max(fit_level));
            let s1 = size(new_level);
            view = View { level: new_level, center: [u - (p.x - rect.center().x) / s1.x, v - (p.y - rect.center().y) / s1.y] };
            self.zoom = (new_level > fit_level * 1.001).then_some(view);
        }

        if self.zoom.is_some() {
            if resp.dragged() {
                let s = size(view.level);
                view.center[0] -= resp.drag_delta().x / s.x;
                view.center[1] -= resp.drag_delta().y / s.y;
            } else if clicked {
                self.zoom = None;
                return;
            }
        } else if clicked {
            // Click in fit view: 100% at that point.
            if let Some(p) = click_pos {
                let s = size(fit_level);
                let u = 0.5 + (p.x - rect.center().x) / s.x;
                let v = 0.5 + (p.y - rect.center().y) / s.y;
                view = View { level: 1.0f32.max(fit_level), center: [u, v] };
                self.zoom = Some(view);
            }
        }

        // Keep the image covering the viewport where it can.
        let s = size(view.level);
        for (c, (vw, iw)) in view.center.iter_mut().zip([(rect.width(), s.x), (rect.height(), s.y)]) {
            let half = vw / 2.0 / iw;
            *c = if half >= 0.5 { 0.5 } else { c.clamp(half, 1.0 - half) };
        }
        if self.zoom.is_some() {
            self.zoom = Some(view);
        }
        let img_rect = egui::Rect::from_min_size(rect.center() - egui::vec2(view.center[0] * s.x, view.center[1] * s.y), s);
        let at = |r: [f32; 4]| egui::Rect::from_min_max(img_rect.lerp_inside(egui::vec2(r[0], r[1])), img_rect.lerp_inside(egui::vec2(r[2], r[3])));
        self.visible = {
            let a = (rect.min - img_rect.min) / s;
            let b = (rect.max - img_rect.min) / s;
            [a.x.clamp(0.0, 1.0), a.y.clamp(0.0, 1.0), b.x.clamp(0.0, 1.0), b.y.clamp(0.0, 1.0)]
        };

        let painter = ui.painter_at(rect);
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        let white = egui::Color32::WHITE;
        let zoomed = self.zoom.is_some();
        if self.show_before {
            let tex = if zoomed { self.full_before_tex.as_ref().or(self.fit_before.as_ref()) } else { self.fit_before.as_ref() };
            if let Some(t) = tex {
                painter.image(t.id(), img_rect, uv, white);
            }
        } else {
            // Best available: full-res render (possibly of an older edit), else
            // the fit preview stretched; then the fresh patch on top.
            let current = self.current_key().cloned();
            let full_current = self.full_tex.as_ref().is_some_and(|f| Some(&f.1) == current.as_ref());
            match (&self.full_tex, zoomed) {
                (Some((t, _)), true) => {
                    painter.image(t.id(), img_rect, uv, white);
                }
                _ => {
                    if let Some(t) = &self.fit_after {
                        painter.image(t.id(), img_rect, uv, white);
                    }
                }
            }
            if zoomed && !full_current {
                if let Some((t, r, k)) = &self.patch {
                    if Some(k) == current.as_ref() {
                        painter.image(t.id(), at(*r), uv, white);
                    }
                }
            }
        }
        if zoomed {
            if self.show_before {
                badge(ui, rect, "Before");
            }
        } else {
            if self.show_before {
                badge(ui, rect, "Before");
            }
            if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ZoomIn);
            }
        }
        if zoomed && resp.hovered() {
            ui.ctx().set_cursor_icon(if resp.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
        }
        if let Some(w) = self.current_path().and_then(|p| self.camera_warnings.get(p)) {
            camera_banner(ui, rect, w);
        }
    }
}

impl eframe::App for FilmApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_workers(&ctx);
        self.handle_keys(&ctx);

        let line = egui::Stroke::new(1.0, theme::LINE);
        let bar = egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::symmetric(10, 6)).stroke(line);
        egui::Panel::top("top").frame(bar).show(ui, |ui| self.top_bar(ui));
        egui::Panel::left("library")
            .frame(egui::Frame::new().fill(theme::PANEL).stroke(line))
            .resizable(true)
            .default_size(280.0)
            .min_size(180.0)
            .show(ui, |ui| self.library(ui));
        egui::Panel::right("edit")
            .frame(egui::Frame::new().fill(theme::PANEL).stroke(line).inner_margin(egui::Margin { left: 16, right: 6, top: 0, bottom: 0 }))
            .resizable(false)
            .exact_size(360.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.set_width(ui.available_width() - 10.0);
                    self.edit_panel(ui)
                });
            });
        if let Some((name, tex)) = &self.reference {
            let (name, tex) = (name.clone(), tex.clone());
            let avail = ui.available_width();
            egui::Panel::right("reference").resizable(true).default_size(avail * 0.4).show(ui, |ui| {
                ui.label(egui::RichText::new(format!("Reference: {name}")).small().weak());
                let rect = ui.available_rect_before_wrap();
                let s = tex.size_vec2();
                let scale = (rect.width() / s.x).min(rect.height() / s.y);
                ui.put(egui::Rect::from_center_size(rect.center(), s * scale), egui::Image::new((tex.id(), s * scale)));
            });
        }
        egui::Panel::bottom("view").frame(egui::Frame::new().fill(theme::SURROUND).inner_margin(egui::Margin::symmetric(16, 8))).show(ui, |ui| {
            self.view_bar(ui)
        });
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::SURROUND).inner_margin(egui::Margin { left: 16, right: 16, top: 16, bottom: 4 }))
            .show(ui, |ui| self.preview(ui));
        draw_toast(&ctx, &mut self.toast);

        // Save once the user lets go of a slider, and keep the preview current.
        if !ctx.input(|i| i.pointer.any_down()) {
            self.save_if_changed();
        }
        self.request_preview();
    }

    fn on_exit(&mut self) {
        self.save_if_changed();
        self.settings.save();
    }
}

#[derive(Debug, Clone, Copy)]
enum TuneAction {
    Save,
    Discard,
    Restore,
    SaveAs,
    Delete,
}

/// Width of the label column in the edit panel.
const LABEL_WIDTH: f32 = 104.0;

/// A labelled row in the edit panel: label on the left, control filling the rest.
fn row<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    row_info(ui, label, None, add)
}

/// A labelled row with an optional (i) explanation beside the label.
fn row_info<R>(ui: &mut egui::Ui, label: &str, info: Option<&str>, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.horizontal(|ui| {
        let h = ui.spacing().interact_size.y;
        ui.allocate_ui_with_layout(egui::vec2(LABEL_WIDTH, h), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.set_min_width(LABEL_WIDTH);
            ui.set_max_width(LABEL_WIDTH);
            ui.label(egui::RichText::new(label).color(theme::TEXT_WEAK));
            // The (i) sits at the column's right edge, so every row keeps
            // the same slider start and the icons line up.
            if let Some(text) = info {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(6.0);
                    theme::info(ui, text);
                });
            }
        });
        add(ui)
    })
    .inner
}

/// A small heading for a group of sliders in the tuning section.
fn subhead(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(text).font(egui::FontId::new(15.0, theme::condensed())).color(theme::TEXT_WEAK));
}

fn subhead_info(ui: &mut egui::Ui, text: &str, info: &str) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        ui.label(egui::RichText::new(text).font(egui::FontId::new(15.0, theme::condensed())).color(theme::TEXT_WEAK));
        theme::info(ui, info);
    });
}

const DEVELOPMENT_INFO: &str = "Push or pull processing: the film is rated faster (push) or slower (pull) and developed to match. \
    Pushes get contrastier and lose shadow detail; pulls get softer. Exposure follows the rating. \
    Tri-X and Portra 800 use Kodak's measured development curves. Kodak published no push curves for the other colour \
    films, so theirs are marked \"modelled\": the negatives are fitted to Portra 800's measured pushes, and E100 is an \
    estimate at Kodak's push rating. Kodak advises against pushing Kodachrome 64, so it has none.";

const OUTPUT_INFO_NEGATIVE: &str = "How the negative is turned into a picture. \
    Lab scan: as a minilab scanner reads it, soft and with the negative's long highlight latitude. \
    Optical print: printed on Kodak paper through its own curves and dyes, with richer colour and shorter highlights.";

const OUTPUT_INFO_MONO: &str = "Black-and-white negatives are shown as a lab scan. Kodak publishes no paper data to print them on.";

const OUTPUT_INFO: &str = "How the slide is turned into a picture. \
    Scanned slide: as a film scanner reads it, with open shadows and the scanner's colour balance. \
    Projected slide: as it looks on a screen, with the film's full density range, deep blacks, a warm lamp and darker corners.";

/// Tuning slider. `range` sets the span; the slider is centred on `default`
/// (the Kodak-data value), so an untouched film shows every handle in the
/// middle. Values that can't go negative keep their floor at zero.
fn slider64(ui: &mut egui::Ui, value: &mut f64, range: std::ops::RangeInclusive<f64>, text: &str, default: f64, info: Option<&str>) {
    let (lo, hi) = (*range.start(), *range.end());
    let half = (hi - lo) / 2.0;
    let floor = if lo >= 0.0 { 0.0 } else { f64::NEG_INFINITY };
    let round = |x: f64| (x * 1000.0).round() / 1000.0;
    // Always include the current value: the slider clamps whatever it shows.
    let lo = round((default - half).max(floor)).min(*value) as f32;
    let hi = round(default + half).max(*value) as f32;
    let mut v = *value as f32;
    // Fine steps: egui snaps the value to the step every frame, and a coarser
    // step would quietly change a preset (e.g. a 0.003 flare) just by showing it.
    slider(ui, true, &mut v, lo..=hi, 0.001, text, default as f32, info);
    if (v - *value as f32).abs() > 1e-6 {
        // Round so saved presets read cleanly (0.53, not 0.5299999713897705).
        *value = (v as f64 * 1000.0).round() / 1000.0;
    }
}

/// Labelled slider that resets to `default` on double-click.
#[allow(clippy::too_many_arguments)]
fn slider(ui: &mut egui::Ui, enabled: bool, value: &mut f32, range: std::ops::RangeInclusive<f32>, step: f64, text: &str, default: f32, info: Option<&str>) {
    let resp = ui
        .add_enabled_ui(enabled, |ui| {
            row_info(ui, text, info, |ui| {
                // One width for every value box, wide enough for "-1.000", so
                // rails and boxes line up whatever the value's length.
                const VALUE_BOX: f32 = 58.0;
                ui.spacing_mut().interact_size.x = VALUE_BOX;
                ui.spacing_mut().slider_width = (ui.available_width() - VALUE_BOX - ui.spacing().item_spacing.x).max(60.0);
                let decimals = if step >= 1.0 { 0 } else if step < 0.01 { 3 } else { 2 };
                ui.add(egui::Slider::new(value, range).step_by(step).fixed_decimals(decimals))
            })
        })
        .inner;
    // egui sliders only sense drags, so detect the double-click from raw input.
    let (double, pos) = ui.input(|i| (i.pointer.button_double_clicked(egui::PointerButton::Primary), i.pointer.interact_pos()));
    if enabled && double && pos.is_some_and(|p| resp.rect.contains(p)) {
        *value = default;
    }
}

/// Draw the toast (if any) near the bottom of the window, fading out.
/// How a library frame should be marked.
struct FrameState<'a> {
    focused: bool,
    selected: bool,
    edited: bool,
    warning: Option<&'a str>,
}

/// One frame of the library's film strip: sprocket holes down both rebates,
/// the thumbnail as the exposed frame, and the frame number and file name
/// printed along the edge below it.
fn film_frame(ui: &mut egui::Ui, index: usize, path: &Path, thumb: Option<&egui::TextureHandle>, st: FrameState) -> egui::Response {
    const REBATE: f32 = 22.0;
    const PITCH: f32 = 14.0; // sprocket hole spacing
    const PRINT: f32 = 26.0; // edge-print band under each frame
    let width = ui.available_width();
    let image_w = (width - 2.0 * REBATE).max(40.0);
    // Whole number of hole pitches per frame, so the holes run on unbroken.
    let height = ((image_w * 2.0 / 3.0 + PRINT + 6.0) / PITCH).ceil() * PITCH;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::FILM_BASE);
    for x in [rect.left() + REBATE / 2.0, rect.right() - REBATE / 2.0] {
        let mut y = rect.top() + PITCH / 2.0;
        while y < rect.bottom() {
            painter.rect_filled(egui::Rect::from_center_size(egui::pos2(x, y), egui::vec2(8.0, 9.0)), 2.0, theme::SURROUND);
            y += PITCH;
        }
    }

    let frame = egui::Rect::from_min_size(rect.left_top() + egui::vec2(REBATE, 6.0), egui::vec2(image_w, image_w * 2.0 / 3.0));
    match thumb {
        Some(tex) => {
            let s = tex.size_vec2();
            let scale = (frame.width() / s.x).min(frame.height() / s.y);
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            painter.image(tex.id(), egui::Rect::from_center_size(frame.center(), s * scale), uv, egui::Color32::WHITE);
        }
        None => egui::Spinner::new().size(18.0).color(theme::TEXT_WEAK).paint_at(ui, egui::Rect::from_center_size(frame.center(), egui::vec2(18.0, 18.0))),
    }
    let marker = if st.focused {
        Some(egui::Stroke::new(2.0, theme::EDGE_PRINT))
    } else if st.selected {
        Some(egui::Stroke::new(1.5, theme::EDGE_PRINT.gamma_multiply(0.6)))
    } else if resp.hovered() {
        Some(egui::Stroke::new(1.0, egui::Color32::from_gray(150)))
    } else {
        None
    };
    if let Some(stroke) = marker {
        painter.rect_stroke(frame.expand(2.0), 1.0, stroke, egui::StrokeKind::Outside);
    }

    // Edge print.
    let y = frame.bottom() + PRINT / 2.0 + 1.0;
    let number = painter.text(
        egui::pos2(frame.left(), y),
        egui::Align2::LEFT_CENTER,
        format!("▸{}", index + 1),
        egui::FontId::new(14.0, theme::condensed()),
        if st.focused || st.selected { theme::EDGE_PRINT } else { theme::EDGE_PRINT.gamma_multiply(0.65) },
    );
    painter.text(
        egui::pos2(number.right() + 8.0, y),
        egui::Align2::LEFT_CENTER,
        file_name(path),
        egui::FontId::proportional(12.5),
        if st.focused { theme::TEXT } else { theme::TEXT_WEAK },
    );
    let mut right = frame.right();
    if st.edited {
        painter.circle_filled(egui::pos2(right - 3.0, y), 3.0, theme::EDGE_PRINT);
        right -= 12.0;
    }
    if st.warning.is_some() {
        painter.text(egui::pos2(right, y), egui::Align2::RIGHT_CENTER, "⚠ not M9", egui::FontId::proportional(11.5), theme::WARN);
    }

    let resp = if st.edited { resp.on_hover_text("Edited") } else { resp };
    match st.warning {
        Some(w) => resp.on_hover_text(w),
        None => resp,
    }
}

fn draw_toast(ctx: &egui::Context, toast: &mut Option<(String, std::time::Instant)>) {
    const SHOW: f32 = 2.6;
    const FADE: f32 = 0.5;
    let Some((msg, at)) = toast.as_ref() else { return };
    let age = at.elapsed().as_secs_f32();
    if age > SHOW + FADE {
        *toast = None;
        return;
    }
    let alpha = if age < SHOW { 1.0 } else { 1.0 - (age - SHOW) / FADE };
    egui::Area::new(egui::Id::unique("remjet-toast"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -40.0))
        .interactable(false)
        .show(ctx, |ui| {
            ui.set_opacity(alpha);
            egui::Frame::new()
                .fill(egui::Color32::from_gray(230))
                .corner_radius(3.0)
                .inner_margin(egui::Margin::symmetric(16, 9))
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(msg).size(15.0).color(egui::Color32::from_gray(20)));
                });
        });
    ctx.request_repaint_after(std::time::Duration::from_millis(30));
}

fn badge(ui: &mut egui::Ui, rect: egui::Rect, text: &str) {
    let area = egui::Rect::from_min_size(rect.min + egui::vec2(12.0, 12.0), egui::vec2(rect.width() - 24.0, 30.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::left_to_right(egui::Align::Min)));
    egui::Frame::new()
        .fill(egui::Color32::from_black_alpha(190))
        .corner_radius(3.0)
        .inner_margin(egui::Margin::symmetric(10, 4))
        .show(&mut child, |ui| {
            ui.label(egui::RichText::new(text).font(egui::FontId::new(16.0, theme::condensed())).color(theme::TEXT));
        });
}

/// Persistent warning across the top of the preview (below the badge).
fn camera_banner(ui: &mut egui::Ui, rect: egui::Rect, warning: &str) {
    let area = egui::Rect::from_min_max(rect.left_top() + egui::vec2(12.0, 48.0), egui::pos2(rect.right() - 12.0, rect.bottom()));
    let mut banner = ui.new_child(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::top_down(egui::Align::Min)));
    egui::Frame::new()
        .fill(egui::Color32::from_black_alpha(200))
        .corner_radius(3.0)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(&mut banner, |ui| {
            ui.label(egui::RichText::new(format!("⚠ {warning}")).color(theme::WARN));
        });
}

fn to_color(img: &RgbImage) -> egui::ColorImage {
    egui::ColorImage::from_rgb([img.width() as usize, img.height() as usize], img.as_raw())
}

fn file_name(p: &Path) -> String {
    p.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

fn thumb_name(p: &Path) -> String {
    format!("thumb:{}", p.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rotating an image and mapping its corners must agree with
    /// `output::apply_orientation`.
    #[test]
    fn sensor_to_display_inverts_display_to_sensor() {
        for o in [
            Orientation::Normal,
            Orientation::HorizontalFlip,
            Orientation::Rotate180,
            Orientation::VerticalFlip,
            Orientation::Transpose,
            Orientation::Rotate90,
            Orientation::Transverse,
            Orientation::Rotate270,
        ] {
            let (s, t) = display_to_sensor(o, 0.2, 0.7);
            let (u, v) = sensor_to_display(o, s, t);
            assert!((u - 0.2).abs() < 1e-6 && (v - 0.7).abs() < 1e-6, "{o:?}");
        }
    }

    #[test]
    fn display_to_sensor_matches_apply_orientation() {
        let (w, h) = (4u32, 3u32);
        let mut img = RgbImage::new(w, h);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = image::Rgb([x as u8, y as u8, 0]);
        }
        for o in [
            Orientation::Normal,
            Orientation::HorizontalFlip,
            Orientation::Rotate180,
            Orientation::VerticalFlip,
            Orientation::Transpose,
            Orientation::Rotate90,
            Orientation::Transverse,
            Orientation::Rotate270,
        ] {
            let out = output::apply_orientation(img.clone(), o);
            for (dx, dy, p) in out.enumerate_pixels() {
                let u = (dx as f32 + 0.5) / out.width() as f32;
                let v = (dy as f32 + 0.5) / out.height() as f32;
                let (s, t) = display_to_sensor(o, u, v);
                let (sx, sy) = ((s * w as f32) as u8, (t * h as f32) as u8);
                assert_eq!([sx, sy], [p[0], p[1]], "{o:?} at display ({dx},{dy})");
            }
        }
    }
}
