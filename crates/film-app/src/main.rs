//! M9 Film — desktop app for developing Leica M9 DNGs with film looks.
//!
//! Layout: library (left), preview (centre), edit panel (right).
//! All decoding/rendering happens on worker threads; the UI only uploads
//! finished images as textures. Edits auto-save to a JSON sidecar per DNG.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use eframe::egui;
use film_core::develop::{self, CameraImage, Quality};
use film_core::edit::{self, Edit};
use film_core::{output, raw};
use image::RgbImage;
use rawler::decoders::Orientation;

const PREVIEW_EDGE: usize = 1800;
const THUMB_EDGE: usize = 240;
const JPEG_QUALITY: u8 = 92;

/// Film presets offered in the UI: (id, label). `None` = neutral render.
const PRESETS: &[(Option<&str>, &str)] = &[(Some("kodachrome64"), "Kodachrome 64"), (None, "None (neutral)")];

fn main() -> eframe::Result {
    let start_folder = std::env::args().nth(1).map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1500.0, 950.0]).with_title("M9 Film"),
        ..Default::default()
    };
    eframe::run_native("M9 Film", options, Box::new(move |cc| Ok(Box::new(FilmApp::new(cc, start_folder)))))
}

// ------------------------------------------------------------------ workers

struct PreviewJob {
    path: PathBuf,
    edit: Edit,
}

struct PreviewResult {
    path: PathBuf,
    edit: Edit,
    result: Result<(RgbImage, RgbImage), String>,
}

/// Renders previews. Keeps the demosaiced, downscaled camera image of the
/// current file cached, so slider changes only re-run the cheap stages.
fn spawn_preview_worker(ctx: egui::Context) -> (Sender<PreviewJob>, Receiver<PreviewResult>) {
    let (job_tx, job_rx) = channel::<PreviewJob>();
    let (res_tx, res_rx) = channel();
    thread::spawn(move || {
        let mut cache: Option<(PathBuf, CameraImage, f64, Orientation)> = None;
        while let Ok(mut job) = job_rx.recv() {
            // Only the newest request matters.
            while let Ok(newer) = job_rx.try_recv() {
                job = newer;
            }
            let result = (|| -> anyhow::Result<(RgbImage, RgbImage)> {
                if cache.as_ref().is_none_or(|c| c.0 != job.path) {
                    let raw_image = raw::load(&job.path)?;
                    let cam = develop::demosaic(&raw_image, None, Quality::Half)?;
                    let (small, f) = cam.downscale_to(PREVIEW_EDGE);
                    cache = Some((job.path.clone(), small, 2.0 * f as f64, raw_image.orientation));
                }
                let (_, cam, scale, orientation) = cache.as_ref().expect("filled above");
                let seed = edit::seed_for(&job.path);
                let after = edit::render(cam, &job.edit, *scale, seed)?;
                let neutral = Edit { preset: None, ..job.edit.clone() };
                let before = edit::render(cam, &neutral, *scale, seed)?;
                Ok((output::apply_orientation(after, *orientation), output::apply_orientation(before, *orientation)))
            })()
            .map_err(|e| format!("{e:#}"));
            if res_tx.send(PreviewResult { path: job.path, edit: job.edit, result }).is_err() {
                break;
            }
            ctx.request_repaint();
        }
    });
    (job_tx, res_rx)
}

/// Generates library thumbnails in the background; stops early if the
/// folder changes (generation counter).
fn spawn_thumbnails(
    ctx: egui::Context,
    files: Vec<PathBuf>,
    generation: Arc<AtomicU64>,
    tx: Sender<(u64, PathBuf, RgbImage)>,
) {
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

fn export_one(path: &Path, edit: &Edit, out_dir: &Path) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(out_dir)?;
    let img = edit::render_file(path, edit, Quality::Full, None)?;
    let out = out_dir.join(path.with_extension("jpg").file_name().unwrap_or_default());
    output::write_jpeg(&img, &out, JPEG_QUALITY)?;
    Ok(out)
}

// ---------------------------------------------------------------------- app

struct FilmApp {
    folder: Option<PathBuf>,
    files: Vec<PathBuf>,
    selected: Option<usize>,
    edited: HashSet<PathBuf>,

    edit: Edit,
    saved_edit: Edit,
    last_requested: Option<(PathBuf, Edit)>,

    preview_tx: Sender<PreviewJob>,
    preview_rx: Receiver<PreviewResult>,
    after_tex: Option<egui::TextureHandle>,
    before_tex: Option<egui::TextureHandle>,
    preview_path: Option<PathBuf>,
    rendering: bool,
    show_before: bool,

    thumbs: HashMap<PathBuf, egui::TextureHandle>,
    thumb_gen: Arc<AtomicU64>,
    thumb_tx: Sender<(u64, PathBuf, RgbImage)>,
    thumb_rx: Receiver<(u64, PathBuf, RgbImage)>,

    status_tx: Sender<String>,
    status_rx: Receiver<String>,
    status: String,
    exports_running: usize,
}

impl FilmApp {
    fn new(cc: &eframe::CreationContext<'_>, start_folder: Option<PathBuf>) -> Self {
        let (preview_tx, preview_rx) = spawn_preview_worker(cc.egui_ctx.clone());
        let (thumb_tx, thumb_rx) = channel();
        let (status_tx, status_rx) = channel();
        let mut app = Self {
            folder: None,
            files: Vec::new(),
            selected: None,
            edited: HashSet::new(),
            edit: Edit::default(),
            saved_edit: Edit::default(),
            last_requested: None,
            preview_tx,
            preview_rx,
            after_tex: None,
            before_tex: None,
            preview_path: None,
            rendering: false,
            show_before: false,
            thumbs: HashMap::new(),
            thumb_gen: Arc::new(AtomicU64::new(0)),
            thumb_tx,
            thumb_rx,
            status_tx,
            status_rx,
            status: "Open a folder of M9 DNGs to begin.".into(),
            exports_running: 0,
        };
        if let Some(folder) = start_folder {
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
        self.folder = Some(folder);
        self.selected = None;
        self.after_tex = None;
        self.before_tex = None;
        if !self.files.is_empty() {
            self.select(0);
        }
    }

    fn select(&mut self, index: usize) {
        if index >= self.files.len() || self.selected == Some(index) {
            return;
        }
        self.save_if_changed();
        self.selected = Some(index);
        let path = &self.files[index];
        self.edit = edit::load(path).unwrap_or_default();
        self.saved_edit = self.edit.clone();
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

    fn request_preview(&mut self) {
        let Some(path) = self.current_path().cloned() else { return };
        let key = (path.clone(), self.edit.clone());
        if self.last_requested.as_ref() == Some(&key) {
            return;
        }
        self.last_requested = Some(key);
        self.rendering = true;
        let _ = self.preview_tx.send(PreviewJob { path, edit: self.edit.clone() });
    }

    fn export_current(&mut self) {
        let (Some(path), Some(folder)) = (self.current_path().cloned(), self.folder.clone()) else { return };
        self.save_if_changed();
        let edit = self.edit.clone();
        let tx = self.status_tx.clone();
        let out_dir = folder.join("exports");
        self.exports_running += 1;
        self.status = format!("Exporting {}…", file_name(&path));
        thread::spawn(move || {
            let msg = match export_one(&path, &edit, &out_dir) {
                Ok(out) => format!("Exported {}", out.display()),
                Err(e) => format!("Export failed for {}: {e:#}", file_name(&path)),
            };
            let _ = tx.send(msg);
        });
    }

    fn poll_workers(&mut self, ctx: &egui::Context) {
        while let Ok(res) = self.preview_rx.try_recv() {
            let current = self.current_path() == Some(&res.path);
            match res.result {
                Ok((after, before)) if current => {
                    // A smaller copy of the preview becomes the library thumbnail.
                    let thumb = image::imageops::thumbnail(&after, THUMB_EDGE as u32, (THUMB_EDGE * after.height() as usize / after.width() as usize) as u32);
                    self.thumbs.insert(res.path.clone(), ctx.load_texture(thumb_name(&res.path), to_color(&thumb), egui::TextureOptions::LINEAR));
                    self.after_tex = Some(ctx.load_texture("after", to_color(&after), egui::TextureOptions::LINEAR));
                    self.before_tex = Some(ctx.load_texture("before", to_color(&before), egui::TextureOptions::LINEAR));
                    self.preview_path = Some(res.path);
                    if Some(&res.edit) == self.last_requested.as_ref().map(|k| &k.1) {
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
            // The preview worker's thumbnail for the selected file is fresher.
            if g == generation && !self.thumbs.contains_key(&path) {
                let tex = ctx.load_texture(thumb_name(&path), to_color(&img), egui::TextureOptions::LINEAR);
                self.thumbs.insert(path, tex);
            }
        }
        while let Ok(msg) = self.status_rx.try_recv() {
            self.exports_running = self.exports_running.saturating_sub(1);
            self.status = msg;
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let (left, right, before) = ctx.input(|i| {
            (i.key_pressed(egui::Key::ArrowLeft), i.key_pressed(egui::Key::ArrowRight), i.key_down(egui::Key::B))
        });
        if let Some(sel) = self.selected {
            if left && sel > 0 {
                self.select(sel - 1);
            }
            if right {
                self.select(sel + 1);
            }
        }
        self.show_before = before;
    }

    // ----------------------------------------------------------- panels

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("📂 Open folder…").clicked() {
                if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                    let ctx = ui.ctx().clone();
                    self.open_folder(&ctx, folder);
                }
            }
            ui.separator();
            if self.exports_running > 0 || self.rendering {
                ui.spinner();
            }
            ui.label(&self.status);
        });
    }

    fn library(&mut self, ui: &mut egui::Ui) {
        ui.heading("Library");
        ui.add_space(4.0);
        let mut clicked = None;
        egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
            for (i, path) in self.files.iter().enumerate() {
                let selected = self.selected == Some(i);
                let frame = egui::Frame::new()
                    .inner_margin(4.0)
                    .corner_radius(4.0)
                    .fill(if selected { ui.visuals().selection.bg_fill } else { egui::Color32::TRANSPARENT });
                let resp = frame
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
                    clicked = Some(i);
                }
                if selected && resp.ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::ArrowRight)) {
                    resp.scroll_to_me(None);
                }
            }
        });
        if let Some(i) = clicked {
            self.select(i);
        }
    }

    fn edit_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Edit");
        if self.selected.is_none() {
            ui.label("No image selected.");
            return;
        }
        ui.add_space(6.0);

        let current = PRESETS.iter().find(|(id, _)| id.as_deref() == self.edit.preset.as_deref()).map_or("?", |p| p.1);
        egui::ComboBox::from_label("Film").selected_text(current).show_ui(ui, |ui| {
            for (id, label) in PRESETS {
                ui.selectable_value(&mut self.edit.preset, id.map(str::to_string), *label);
            }
        });
        ui.add_space(8.0);

        ui.spacing_mut().slider_width = 160.0;
        let e = &mut self.edit;
        ui.add(egui::Slider::new(&mut e.exposure_ev, -2.0..=2.0).step_by(0.05).text("Exposure (EV)"));
        ui.add(egui::Slider::new(&mut e.warmth, -1.0..=1.0).step_by(0.01).text("Warmth"));
        ui.add(egui::Slider::new(&mut e.tint, -1.0..=1.0).step_by(0.01).text("Tint"));
        ui.add_enabled(e.preset.is_some(), egui::Slider::new(&mut e.grain, 0.0..=3.0).step_by(0.05).text("Grain"));
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            if ui.button("Reset").clicked() {
                self.edit = Edit::default();
            }
            let before = ui.add(egui::Button::new("Hold for before").sense(egui::Sense::click_and_drag()));
            if before.is_pointer_button_down_on() {
                self.show_before = true;
            }
        });
        ui.label(egui::RichText::new("Tip: hold B to compare, ←/→ to move between images.").small().weak());
        ui.add_space(16.0);
        ui.separator();

        ui.heading("Export");
        let enabled = self.folder.is_some();
        if ui.add_enabled(enabled, egui::Button::new("Export JPG")).clicked() {
            self.export_current();
        }
        if let Some(folder) = &self.folder {
            ui.label(egui::RichText::new(format!("Saves full resolution to {}", folder.join("exports").display())).small().weak());
        }
    }

    fn preview(&mut self, ui: &mut egui::Ui) {
        let tex = if self.show_before { self.before_tex.as_ref() } else { self.after_tex.as_ref() };
        let stale = self.preview_path.as_ref() != self.current_path();
        match tex {
            Some(tex) if !stale => {
                let avail = ui.available_size();
                let s = tex.size_vec2();
                let scale = (avail.x / s.x).min(avail.y / s.y);
                ui.centered_and_justified(|ui| {
                    ui.add(egui::Image::new((tex.id(), s * scale)));
                });
                if self.show_before {
                    ui.put(
                        egui::Rect::from_min_size(ui.max_rect().min + egui::vec2(10.0, 10.0), egui::vec2(140.0, 24.0)),
                        egui::Label::new(egui::RichText::new("BEFORE (neutral)").strong().background_color(egui::Color32::from_black_alpha(160))),
                    );
                }
            }
            _ if self.selected.is_some() => {
                ui.centered_and_justified(|ui| ui.spinner());
            }
            _ => {
                ui.centered_and_justified(|ui| ui.label("Open a folder of DNGs (📂 at the top left)."));
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
        egui::Panel::right("edit").resizable(false).exact_size(340.0).show(ui, |ui| self.edit_panel(ui));
        egui::CentralPanel::default().show(ui, |ui| self.preview(ui));

        // Save once the user lets go of a slider, and keep the preview current.
        if !ctx.input(|i| i.pointer.any_down()) {
            self.save_if_changed();
        }
        self.request_preview();
    }

    fn on_exit(&mut self) {
        self.save_if_changed();
    }
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
