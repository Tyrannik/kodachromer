//! M9 Film — desktop app for developing Leica M9 DNGs with film looks.
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
use film_core::develop::{self, CameraImage, Quality};
use film_core::edit::{self, Edit, ExportOptions};
use film_core::film::FilmPreset;
use film_core::{output, presets, raw};
use image::RgbImage;
use rawler::decoders::Orientation;
use serde::{Deserialize, Serialize};

const PREVIEW_EDGE: usize = 1800;
const THUMB_EDGE: usize = 240;

/// Export size choices: (max long edge, label).
const SIZES: &[(Option<u32>, &str)] =
    &[(None, "Full size"), (Some(4000), "4000 px"), (Some(3000), "3000 px"), (Some(2048), "2048 px"), (Some(1600), "1600 px")];

fn main() -> eframe::Result {
    let start_folder = std::env::args().nth(1).map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1500.0, 950.0]).with_title("M9 Film"),
        ..Default::default()
    };
    eframe::run_native("M9 Film", options, Box::new(move |cc| Ok(Box::new(FilmApp::new(cc, start_folder)))))
}

// ------------------------------------------------------------- app settings

/// App-wide preferences, stored in %APPDATA%\M9Film\settings.json.
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
        std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("M9Film").join("settings.json"))
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

/// A 100% view request: centre in displayed-image coordinates (0..1) and the
/// size of the viewport in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ZoomReq {
    center: [f32; 2],
    view_px: [u32; 2],
}

struct PreviewJob {
    path: PathBuf,
    edit: Edit,
    zoom: Option<ZoomReq>,
    /// Unsaved tuned preset to preview instead of the stored one.
    preset: Option<FilmPreset>,
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
    edit: Edit,
    zoom: Option<ZoomReq>,
    preset: Option<FilmPreset>,
    /// (after, before, full-res oriented size for zoom maths)
    result: Result<(RgbImage, RgbImage, [usize; 2]), String>,
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

fn swaps_axes(o: Orientation) -> bool {
    matches!(o, Orientation::Rotate90 | Orientation::Rotate270 | Orientation::Transpose | Orientation::Transverse)
}

/// Renders previews. Caches the downscaled image for fit view and, once
/// zoom is used, the full-resolution demosaic, so slider changes only re-run
/// the cheap colour and film stages.
fn spawn_preview_worker(ctx: egui::Context) -> (Sender<PreviewJob>, Receiver<PreviewResult>) {
    let (job_tx, job_rx) = channel::<PreviewJob>();
    let (res_tx, res_rx) = channel();
    thread::spawn(move || {
        let mut fit: Option<(PathBuf, CameraImage, f64, Orientation)> = None;
        let mut full: Option<(PathBuf, CameraImage, Orientation)> = None;
        while let Ok(mut job) = job_rx.recv() {
            while let Ok(newer) = job_rx.try_recv() {
                job = newer; // only the newest request matters
            }
            let seed = edit::seed_for(&job.path);
            let neutral = Edit { preset: None, ..job.edit.clone() };
            let result = (|| -> anyhow::Result<(RgbImage, RgbImage, [usize; 2])> {
                match job.zoom {
                    None => {
                        if fit.as_ref().is_none_or(|c| c.0 != job.path) {
                            let raw_image = raw::load(&job.path)?;
                            let cam = develop::demosaic(&raw_image, None, Quality::Half)?;
                            let (small, f) = cam.downscale_to(PREVIEW_EDGE);
                            fit = Some((job.path.clone(), small, 2.0 * f as f64, raw_image.orientation));
                        }
                        let (_, cam, scale, o) = fit.as_ref().expect("filled above");
                        let after = output::apply_orientation(edit::render_with(cam, &job.edit, job.preset.as_ref(), *scale, seed)?, *o);
                        let before = output::apply_orientation(edit::render(cam, &neutral, *scale, seed)?, *o);
                        let size = [after.width() as usize, after.height() as usize];
                        Ok((after, before, size))
                    }
                    Some(z) => {
                        if full.as_ref().is_none_or(|c| c.0 != job.path) {
                            full = None; // free the previous image first
                            let raw_image = raw::load(&job.path)?;
                            let cam = develop::demosaic(&raw_image, None, Quality::Full)?;
                            full = Some((job.path.clone(), cam, raw_image.orientation));
                        }
                        let (_, cam, o) = full.as_ref().expect("filled above");
                        let (s, t) = display_to_sensor(*o, z.center[0], z.center[1]);
                        let (vw, vh) = if swaps_axes(*o) { (z.view_px[1], z.view_px[0]) } else { (z.view_px[0], z.view_px[1]) };
                        let (vw, vh) = ((vw as usize).min(cam.width), (vh as usize).min(cam.height));
                        let x = ((s * cam.width as f32) as isize - vw as isize / 2).clamp(0, (cam.width - vw) as isize) as usize;
                        let y = ((t * cam.height as f32) as isize - vh as isize / 2).clamp(0, (cam.height - vh) as isize) as usize;
                        let crop = cam.crop(x, y, vw, vh);
                        let after = output::apply_orientation(edit::render_with(&crop, &job.edit, job.preset.as_ref(), 1.0, seed)?, *o);
                        let before = output::apply_orientation(edit::render(&crop, &neutral, 1.0, seed)?, *o);
                        let size = if swaps_axes(*o) { [cam.height, cam.width] } else { [cam.width, cam.height] };
                        Ok((after, before, size))
                    }
                }
            })()
            .map_err(|e| format!("{e:#}"));
            if res_tx.send(PreviewResult { path: job.path, edit: job.edit, zoom: job.zoom, preset: job.preset, result }).is_err() {
                break;
            }
            ctx.request_repaint();
        }
    });
    (job_tx, res_rx)
}

// --------------------------------------------------------------- thumbnails

/// Renders library thumbnails in the background; stops if the folder changes.
fn spawn_thumbnails(ctx: egui::Context, files: Vec<PathBuf>, generation: Arc<AtomicU64>, tx: Sender<(u64, PathBuf, RgbImage)>) {
    let my_gen = generation.load(Ordering::SeqCst);
    thread::spawn(move || {
        for path in files {
            if generation.load(Ordering::SeqCst) != my_gen {
                return;
            }
            let e = edit::load(&path).unwrap_or_default();
            if let Ok(img) = edit::render_file(&path, &e, Quality::Half, Some(THUMB_EDGE)) {
                let _ = tx.send((my_gen, path, img));
                ctx.request_repaint();
            }
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
    last_requested: Option<(PathBuf, Edit, Option<ZoomReq>, Option<FilmPreset>)>,

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
    zoom: Option<ZoomReq>,
    zoom_after: Option<egui::TextureHandle>,
    zoom_before: Option<egui::TextureHandle>,
    /// Upright full-resolution size of the current image (known after first zoom).
    full_size: Option<[usize; 2]>,
    rendering: bool,
    show_before: bool,

    thumbs: HashMap<PathBuf, egui::TextureHandle>,
    thumb_gen: Arc<AtomicU64>,
    thumb_tx: Sender<(u64, PathBuf, RgbImage)>,
    thumb_rx: Receiver<(u64, PathBuf, RgbImage)>,

    batch: Option<Batch>,
    status: String,
}

impl FilmApp {
    fn new(cc: &eframe::CreationContext<'_>, start_folder: Option<PathBuf>) -> Self {
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
            zoom_after: None,
            zoom_before: None,
            full_size: None,
            rendering: false,
            show_before: false,
            thumbs: HashMap::new(),
            thumb_gen: Arc::new(AtomicU64::new(0)),
            thumb_tx,
            thumb_rx,
            batch: None,
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
                    .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("dng")))
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        self.edited = files.iter().filter(|p| edit::sidecar_path(p).exists()).cloned().collect();
        self.status = format!("{} DNGs in {}", files.len(), folder.display());
        self.thumbs.clear();
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
        self.zoom_after = None;
        self.zoom_before = None;
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

    fn save_if_changed(&mut self) {
        let Some(path) = self.current_path().cloned() else { return };
        if self.edit == self.saved_edit {
            return;
        }
        match edit::save(&path, &self.edit) {
            Ok(()) => {
                self.saved_edit = self.edit.clone();
                self.edited.insert(path);
            }
            Err(e) => self.status = format!("Could not save edit: {e:#}"),
        }
    }

    fn copy_settings(&mut self) {
        self.clipboard = Some(self.edit.clone());
        self.status = "Settings copied.".into();
    }

    fn paste_settings(&mut self, ctx: &egui::Context) {
        let Some(clip) = self.clipboard.clone() else { return };
        self.save_if_changed();
        let targets: Vec<PathBuf> = self.selection.iter().filter_map(|&i| self.files.get(i).cloned()).collect();
        let mut failed = 0;
        for path in &targets {
            match edit::save(path, &clip) {
                Ok(()) => {
                    self.edited.insert(path.clone());
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
        self.status = match failed {
            0 => format!("Pasted settings to {} image(s).", targets.len()),
            n => format!("Pasted to {}, {n} failed to save.", targets.len() - n),
        };
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
        let key = (path.clone(), self.edit.clone(), self.zoom, preset.clone());
        if self.last_requested.as_ref() == Some(&key) {
            return;
        }
        self.last_requested = Some(key);
        self.rendering = true;
        let _ = self.preview_tx.send(PreviewJob { path, edit: self.edit.clone(), zoom: self.zoom, preset });
    }

    fn poll_workers(&mut self, ctx: &egui::Context) {
        while let Ok(res) = self.preview_rx.try_recv() {
            let current = self.current_path() == Some(&res.path);
            let latest = self.last_requested.as_ref().is_some_and(|k| k.1 == res.edit && k.2 == res.zoom && k.3 == res.preset);
            match res.result {
                Ok((after, before, size)) if current => {
                    let opts = egui::TextureOptions::LINEAR;
                    if res.zoom.is_some() {
                        self.full_size = Some(size);
                        self.zoom_after = Some(ctx.load_texture("zoom-after", to_color(&after), opts));
                        self.zoom_before = Some(ctx.load_texture("zoom-before", to_color(&before), opts));
                    } else {
                        let h = (THUMB_EDGE * after.height() as usize / after.width() as usize) as u32;
                        let thumb = image::imageops::thumbnail(&after, THUMB_EDGE as u32, h);
                        self.thumbs.insert(res.path.clone(), ctx.load_texture(thumb_name(&res.path), to_color(&thumb), opts));
                        self.fit_after = Some(ctx.load_texture("after", to_color(&after), opts));
                        self.fit_before = Some(ctx.load_texture("before", to_color(&before), opts));
                        self.fit_path = Some(res.path.clone());
                    }
                    if latest {
                        self.rendering = false;
                    }
                }
                Err(e) if current => {
                    self.status = format!("Could not render {}: {e}", file_name(&res.path));
                    self.rendering = false;
                }
                _ => {}
            }
        }
        let generation = self.thumb_gen.load(Ordering::SeqCst);
        while let Ok((g, path, img)) = self.thumb_rx.try_recv() {
            if g == generation {
                let tex = ctx.load_texture(thumb_name(&path), to_color(&img), egui::TextureOptions::LINEAR);
                self.thumbs.insert(path, tex);
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

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let (left, right, before, esc, z, all, copy, paste) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowRight),
                i.key_down(egui::Key::B),
                i.key_pressed(egui::Key::Escape),
                i.key_pressed(egui::Key::Z),
                i.modifiers.command && i.key_pressed(egui::Key::A),
                i.events.iter().any(|e| matches!(e, egui::Event::Copy)),
                i.events.iter().any(|e| matches!(e, egui::Event::Paste(_))),
            )
        });
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
            self.zoom = match self.zoom {
                Some(_) => None,
                None => Some(ZoomReq { center: [0.5, 0.5], view_px: [1, 1] }),
            };
        }
        if all {
            self.selection = (0..self.files.len()).collect();
        }
        if copy {
            self.copy_settings();
        }
        if paste {
            self.paste_settings(ctx);
        }
        self.show_before = before;
    }

    // ----------------------------------------------------------- panels

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("📂 Open folder…").clicked() {
                let mut dialog = rfd::FileDialog::new();
                if let Some(f) = &self.folder {
                    dialog = dialog.set_directory(f);
                }
                if let Some(folder) = dialog.pick_folder() {
                    let ctx = ui.ctx().clone();
                    self.open_folder(&ctx, folder);
                }
            }
            if ui.button("🖼 Reference…").on_hover_text("Show a reference image (e.g. a real K64 scan) beside the preview").clicked() {
                let ctx = ui.ctx().clone();
                self.pick_reference(&ctx);
            }
            if self.reference.is_some() && ui.button("✕ Hide reference").clicked() {
                self.reference = None;
            }
            ui.separator();
            if self.rendering {
                ui.spinner();
            }
            ui.label(&self.status);
        });
    }

    fn library(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Library");
            if self.selection.len() > 1 {
                ui.label(egui::RichText::new(format!("{} selected", self.selection.len())).weak());
            }
        });
        ui.label(egui::RichText::new("Ctrl/Shift-click to select several").small().weak());
        ui.add_space(4.0);
        let mut clicked = None;
        let nav = ui.input(|i| i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::ArrowRight));
        egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
            for (i, path) in self.files.iter().enumerate() {
                let focused = self.selected == Some(i);
                let in_sel = self.selection.contains(&i);
                let fill = if focused {
                    ui.visuals().selection.bg_fill
                } else if in_sel {
                    ui.visuals().selection.bg_fill.gamma_multiply(0.45)
                } else {
                    egui::Color32::TRANSPARENT
                };
                let resp = egui::Frame::new()
                    .inner_margin(4.0)
                    .corner_radius(4.0)
                    .fill(fill)
                    .show(ui, |ui| {
                        let width = ui.available_width().min(THUMB_EDGE as f32);
                        match self.thumbs.get(path) {
                            Some(tex) => {
                                let s = tex.size_vec2();
                                ui.add(egui::Image::new((tex.id(), egui::vec2(width, width * s.y / s.x))));
                            }
                            None => {
                                ui.add_sized([width, width * 0.66], egui::Spinner::new());
                            }
                        }
                        let mark = if self.edited.contains(path) { " ●" } else { "" };
                        ui.label(format!("{}{mark}", file_name(path)));
                    })
                    .response
                    .interact(egui::Sense::click());
                if resp.clicked() {
                    clicked = Some((i, ui.input(|inp| inp.modifiers)));
                }
                if focused && nav {
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
        ui.heading("Edit");
        if self.selected.is_none() {
            ui.label("No image selected.");
            return;
        }
        ui.add_space(6.0);

        let current = match &self.edit.preset {
            None => "None (neutral)".to_string(),
            Some(id) => self.preset_list.iter().find(|(i, _)| i == id).map_or(format!("{id} (missing)"), |p| p.1.clone()),
        };
        egui::ComboBox::from_label("Film").selected_text(current).show_ui(ui, |ui| {
            for (id, name) in &self.preset_list {
                ui.selectable_value(&mut self.edit.preset, Some(id.clone()), name);
            }
            ui.selectable_value(&mut self.edit.preset, None, "None (neutral)");
        });
        ui.add_space(8.0);

        ui.spacing_mut().slider_width = 160.0;
        let d = Edit::default();
        let film = self.edit.preset.is_some();
        let e = &mut self.edit;
        slider(ui, true, &mut e.exposure_ev, -2.0..=2.0, 0.05, "Exposure (EV)", d.exposure_ev);
        slider(ui, true, &mut e.warmth, -1.0..=1.0, 0.01, "Warmth", d.warmth);
        slider(ui, true, &mut e.tint, -1.0..=1.0, 0.01, "Tint", d.tint);
        slider(ui, film, &mut e.grain, 0.0..=4.0, 0.05, "Grain", d.grain);
        slider(ui, film, &mut e.grain_size, 0.5..=4.0, 0.05, "Grain size", d.grain_size);
        ui.label(egui::RichText::new("Double-click a slider to reset it.").small().weak());
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            if ui.button("Reset all").clicked() {
                self.edit = Edit::default();
            }
            let before = ui.add(egui::Button::new("Hold for before").sense(egui::Sense::click_and_drag()));
            if before.is_pointer_button_down_on() {
                self.show_before = true;
            }
            let zoom_label = if self.zoom.is_some() { "Fit" } else { "100%" };
            if ui.button(zoom_label).clicked() {
                self.zoom = match self.zoom {
                    Some(_) => None,
                    None => Some(ZoomReq { center: [0.5, 0.5], view_px: [1, 1] }),
                };
            }
        });
        ui.label(
            egui::RichText::new("Click the image to zoom to 100% there, drag to pan, Esc to fit. Hold B to compare, ←/→ to move.")
                .small()
                .weak(),
        );

        ui.add_space(10.0);
        ui.separator();
        self.tune_section(ui);

        ui.add_space(10.0);
        ui.separator();
        ui.heading("Settings");
        ui.horizontal(|ui| {
            if ui.button("Copy").on_hover_text("Ctrl+C").clicked() {
                self.copy_settings();
            }
            let n = self.selection.len();
            let paste = ui
                .add_enabled(self.clipboard.is_some(), egui::Button::new(format!("Paste to {n} selected")))
                .on_hover_text("Ctrl+V. Select images with Ctrl/Shift-click or Ctrl+A.");
            if paste.clicked() {
                self.paste_settings(&ctx);
            }
        });

        ui.add_space(10.0);
        ui.separator();
        self.export_section(ui);
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
        let title = if dirty { format!("Tune look: {} •", tune.working.name) } else { format!("Tune look: {}", tune.working.name) };
        let mut action = None;
        egui::CollapsingHeader::new(egui::RichText::new(title).heading()).id_salt("tune").show(ui, |ui| {
            ui.label(egui::RichText::new("Changes the film itself, for every image that uses it. Double-click a slider for the Kodak-data value.").small().weak());
            let w = &mut tune.working;
            let r = tune.reference.clone().unwrap_or_else(|| tune.saved.clone());
            // Base brightness shown as EV relative to the reference grey.
            let before = -(w.exposure.grey_linear / r.exposure.grey_linear).log2() as f32;
            let mut bright = before;
            slider(ui, true, &mut bright, -1.0..=1.0, 0.05, "Brightness (EV)", 0.0);
            if (bright - before).abs() > 1e-6 {
                let grey = r.exposure.grey_linear * 2f64.powf(-bright as f64);
                w.exposure.grey_linear = (grey * 1e5).round() / 1e5;
            }
            slider64(ui, &mut w.look.contrast, 0.7..=1.4, "Contrast", r.look.contrast);
            slider64(ui, &mut w.look.interimage, 0.0..=0.6, "Saturation", r.look.interimage);
            slider64(ui, &mut w.scan.gamma, 0.8..=1.6, "Shadow lift", r.scan.gamma);
            slider64(ui, &mut w.scan.black_density, 2.4..=3.8, "Black depth", r.scan.black_density);
            ui.label(egui::RichText::new("Scanner colour").small());
            slider64(ui, &mut w.scan.white_balance[0], 0.85..=1.15, "Red", r.scan.white_balance[0]);
            slider64(ui, &mut w.scan.white_balance[1], 0.85..=1.15, "Green", r.scan.white_balance[1]);
            slider64(ui, &mut w.scan.white_balance[2], 0.85..=1.15, "Blue", r.scan.white_balance[2]);
            ui.label(egui::RichText::new("Grain").small());
            slider64(ui, &mut w.grain.amount, 0.0..=1.2, "Strength", r.grain.amount);
            let mut colour = 1.0 - w.grain.correlation;
            slider64(ui, &mut colour, 0.0..=1.0, "Colour grain", 1.0 - r.grain.correlation);
            w.grain.correlation = 1.0 - colour;

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
                ui.add(egui::TextEdit::singleline(&mut self.new_preset_name).hint_text("New film name").desired_width(170.0));
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
        ui.heading("Export JPG");
        let opts = &mut self.settings.export;
        let size_label = SIZES.iter().find(|(s, _)| *s == opts.max_edge).map_or("Custom", |s| s.1);
        egui::ComboBox::from_label("Size").selected_text(size_label).show_ui(ui, |ui| {
            for (s, label) in SIZES {
                ui.selectable_value(&mut opts.max_edge, *s, *label);
            }
        });
        ui.add(egui::Slider::new(&mut opts.quality, 60..=100).text("Quality"));
        ui.checkbox(&mut opts.keep_exif, "Keep camera metadata (EXIF)");

        ui.horizontal(|ui| {
            let dest = match (&self.settings.export_dir, &self.folder) {
                (Some(d), _) => d.display().to_string(),
                (None, Some(f)) => f.join("exports").display().to_string(),
                _ => "—".into(),
            };
            ui.label(egui::RichText::new(format!("To: {dest}")).small());
        });
        ui.horizontal(|ui| {
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
        ui.add_space(6.0);

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
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(!current.is_empty(), egui::Button::new("This image")).clicked() {
                job = Some(current);
            }
            if ui.add_enabled(selected.len() > 1, egui::Button::new(format!("Selected ({})", selected.len()))).clicked() {
                job = Some(selected);
            }
            if ui.add_enabled(!edited.is_empty(), egui::Button::new(format!("Edited ({})", edited.len()))).clicked() {
                job = Some(edited);
            }
            if ui.add_enabled(!all.is_empty(), egui::Button::new(format!("All ({})", all.len()))).clicked() {
                job = Some(all);
            }
        });
        if let Some(files) = job {
            self.start_export(&ctx, files);
        }
    }

    fn preview(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let ppp = ui.ctx().pixels_per_point();
        if self.selected.is_none() {
            ui.centered_and_justified(|ui| ui.label("Open a folder of DNGs (📂 at the top left)."));
            return;
        }

        if let Some(z) = &mut self.zoom {
            // Keep the requested crop matched to the viewport in physical pixels.
            z.view_px = [(rect.width() * ppp) as u32, (rect.height() * ppp) as u32];
            let resp = ui.interact(rect, ui.scope_id().with("zoom"), egui::Sense::click_and_drag());
            if resp.dragged() {
                if let Some([fw, fh]) = self.full_size {
                    let d = resp.drag_delta() * ppp;
                    z.center[0] = (z.center[0] - d.x / fw as f32).clamp(0.0, 1.0);
                    z.center[1] = (z.center[1] - d.y / fh as f32).clamp(0.0, 1.0);
                }
            } else if resp.clicked() {
                self.zoom = None;
            }
            let tex = if self.show_before { self.zoom_before.as_ref() } else { self.zoom_after.as_ref() };
            match tex {
                Some(tex) => {
                    let size = tex.size_vec2() / ppp;
                    ui.put(egui::Rect::from_center_size(rect.center(), size), egui::Image::new((tex.id(), size)));
                }
                None => {
                    ui.put(egui::Rect::from_center_size(rect.center(), egui::vec2(40.0, 40.0)), egui::Spinner::new());
                }
            }
            badge(ui, rect, if self.show_before { "100% · BEFORE" } else { "100%" });
            return;
        }

        let tex = if self.show_before { self.fit_before.as_ref() } else { self.fit_after.as_ref() };
        let stale = self.fit_path.as_ref() != self.current_path();
        match tex {
            Some(tex) if !stale => {
                let s = tex.size_vec2();
                let scale = (rect.width() / s.x).min(rect.height() / s.y);
                let img_rect = egui::Rect::from_center_size(rect.center(), s * scale);
                let resp = ui.put(img_rect, egui::Image::new((tex.id(), s * scale)).sense(egui::Sense::click()));
                if resp.clicked() {
                    if let Some(p) = resp.interact_pointer_pos() {
                        let u = ((p.x - img_rect.min.x) / img_rect.width()).clamp(0.0, 1.0);
                        let v = ((p.y - img_rect.min.y) / img_rect.height()).clamp(0.0, 1.0);
                        self.zoom = Some(ZoomReq { center: [u, v], view_px: [1, 1] });
                        self.zoom_after = None;
                        self.zoom_before = None;
                    }
                }
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ZoomIn);
                }
                if self.show_before {
                    badge(ui, rect, "BEFORE (neutral)");
                }
            }
            _ => {
                ui.centered_and_justified(|ui| ui.spinner());
            }
        }
    }
}

impl eframe::App for FilmApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_workers(&ctx);
        self.handle_keys(&ctx);

        egui::Panel::top("top").show(ui, |ui| self.top_bar(ui));
        egui::Panel::left("library").resizable(true).default_size(260.0).min_size(160.0).show(ui, |ui| self.library(ui));
        egui::Panel::right("edit").resizable(false).exact_size(360.0).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.edit_panel(ui));
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
        egui::CentralPanel::default().show(ui, |ui| self.preview(ui));

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

fn slider64(ui: &mut egui::Ui, value: &mut f64, range: std::ops::RangeInclusive<f64>, text: &str, default: f64) {
    let mut v = *value as f32;
    let (lo, hi) = (*range.start() as f32, *range.end() as f32);
    slider(ui, true, &mut v, lo..=hi, 0.01, text, default as f32);
    if (v - *value as f32).abs() > 1e-6 {
        // Round so saved presets read cleanly (0.53, not 0.5299999713897705).
        *value = (v as f64 * 1000.0).round() / 1000.0;
    }
}

/// Slider that resets to `default` on double-click.
fn slider(ui: &mut egui::Ui, enabled: bool, value: &mut f32, range: std::ops::RangeInclusive<f32>, step: f64, text: &str, default: f32) {
    let resp = ui.add_enabled(enabled, egui::Slider::new(value, range).step_by(step).text(text));
    // egui sliders only sense drags, so detect the double-click from raw input.
    let (double, pos) = ui.input(|i| (i.pointer.button_double_clicked(egui::PointerButton::Primary), i.pointer.interact_pos()));
    if enabled && double && pos.is_some_and(|p| resp.rect.contains(p)) {
        *value = default;
    }
}

fn badge(ui: &mut egui::Ui, rect: egui::Rect, text: &str) {
    ui.put(
        egui::Rect::from_min_size(rect.min + egui::vec2(10.0, 10.0), egui::vec2(160.0, 24.0)),
        egui::Label::new(egui::RichText::new(text).strong().background_color(egui::Color32::from_black_alpha(160))),
    );
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
