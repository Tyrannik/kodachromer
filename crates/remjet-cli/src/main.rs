//! Command-line front end.
//!
//!   remjet-cli info   <files or folders...>
//!   remjet-cli render <files or folders...> --out <folder> [--half] [--ev <stops>]
//!                   [--preset kodachrome64 | ektachrome-e100 | portra400 | ektar100 | gold200 | ultramax400 | portra800 | trix400 | <preset.toml>] [--grain <amount>] [--no-fit] [--no-scanner-colour] [--print] [--push <stops>]

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use remjet_core::develop::{self, DevelopParams, Quality};
use remjet_core::edit::ExportOptions;
use remjet_core::film::{FilmModel, FilmParams, FilmPreset};
use remjet_core::{output, raw};

const USAGE: &str = "usage:\n  remjet-cli info <raw file | folder>...\n  remjet-cli render <raw file | folder>... --out <folder> [--half] [--ev <stops>]\n                  [--preset kodachrome64 | ektachrome-e100 | portra400 | ektar100 | gold200 | ultramax400 | portra800 | trix400 | <preset.toml>]\n                  [--grain <amount>] [--no-fit] [--no-scanner-colour] [--print] [--push <stops>]\n  remjet-cli export <raw file | folder>... [--out <folder>] [--max-edge <px>] [--quality <1-100>]\n                  [--no-exif] [--edited-only]\n      (uses each file's saved app edit; default output is <folder>/exports)";

/// Batch export using the app's sidecar edits.
fn export(args: &[String]) -> Result<()> {
    let mut inputs = Vec::new();
    let mut out_dir: Option<PathBuf> = None;
    let mut opts = ExportOptions::default();
    let mut edited_only = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out_dir = Some(PathBuf::from(it.next().context("--out needs a folder")?)),
            "--max-edge" => opts.max_edge = Some(it.next().context("--max-edge needs a value")?.parse()?),
            "--quality" => opts.quality = it.next().context("--quality needs a value")?.parse()?,
            "--no-exif" => opts.keep_exif = false,
            "--edited-only" => edited_only = true,
            _ => inputs.push(a.clone()),
        }
    }
    if inputs.is_empty() {
        bail!(USAGE);
    }
    let files = collect_dngs(&inputs)?;
    let (mut ok, mut failed, mut skipped, mut not_m9) = (0, 0, 0, 0);
    for path in &files {
        let saved = remjet_core::edit::load(path);
        if edited_only && saved.is_none() {
            skipped += 1;
            continue;
        }
        if let Ok(Some(w)) = raw::check_camera(path) {
            not_m9 += 1;
            warn_camera(path, &w);
        }
        let dir = out_dir.clone().unwrap_or_else(|| path.parent().unwrap_or(Path::new(".")).join("exports"));
        let start = Instant::now();
        match remjet_core::edit::export_jpeg(path, &saved.unwrap_or_default(), &dir, &opts) {
            Ok(out) => {
                ok += 1;
                println!("{} -> {} ({} ms)", path.display(), out.display(), start.elapsed().as_millis());
            }
            Err(e) => {
                failed += 1;
                println!("{}: FAILED: {e:#}", path.display());
            }
        }
    }
    println!("{ok} exported, {failed} failed, {skipped} skipped");
    print_not_m9(not_m9);
    if failed > 0 {
        bail!("{failed} export(s) failed");
    }
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.split_first() {
        Some((cmd, rest)) if cmd == "info" && !rest.is_empty() => info(rest),
        Some((cmd, rest)) if cmd == "render" => render(rest),
        Some((cmd, rest)) if cmd == "export" => export(rest),
        _ => bail!(USAGE),
    }
}

fn collect_dngs(paths: &[String]) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for p in paths.iter().map(Path::new) {
        if p.is_dir() {
            for entry in std::fs::read_dir(p)? {
                let path = entry?.path();
                if raw::is_raw_file(&path) {
                    out.push(path);
                }
            }
        } else {
            out.push(p.to_path_buf());
        }
    }
    out.sort();
    Ok(out)
}

fn info(paths: &[String]) -> Result<()> {
    let files = collect_dngs(paths)?;
    let (mut failures, mut not_m9) = (0, 0);
    for path in &files {
        let start = Instant::now();
        match raw::load(path) {
            Ok(image) => {
                let i = raw::info(&image);
                if let Some(w) = i.camera_warning() {
                    not_m9 += 1;
                    warn_camera(path, &w);
                }
                println!(
                    "{}: {} {} {}x{} {}bit black={:?} white={:?} wb={:?} baseline_ev={} orient={:?} matrices={:?} crop={:?} lintable={} sensor=[{}] ({} ms)",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    i.make, i.model, i.width, i.height, i.bits_per_sample,
                    i.black_levels, i.white_levels, develop::as_shot_wb(&image),
                    develop::baseline_exposure(&image), image.orientation,
                    i.color_matrix_illuminants, i.crop, i.has_linearization_table, i.sensor,
                    start.elapsed().as_millis()
                );
            }
            Err(e) => {
                failures += 1;
                println!("{}: FAILED: {e:#}", path.display());
            }
        }
    }
    println!("{} files, {} failed", files.len(), failures);
    print_not_m9(not_m9);
    Ok(())
}

/// Warn (on stderr, so piped output stays clean) that a file isn't from an M9.
fn warn_camera(path: &Path, warning: &str) {
    eprintln!("warning: {}: {warning}", path.file_name().unwrap_or_default().to_string_lossy());
}

fn print_not_m9(count: usize) {
    if count > 0 {
        eprintln!("warning: {count} file(s) are not from a Leica M9; colours may be off");
    }
}

/// Stable grain seed from the file name, so re-renders match.
fn seed_for(path: &Path) -> u64 {
    path.file_name()
        .map(|n| n.to_string_lossy().bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3)))
        .unwrap_or(0)
}

fn render(args: &[String]) -> Result<()> {
    let mut inputs = Vec::new();
    let mut out_dir = None;
    let mut params = DevelopParams::default();
    let mut preset: Option<FilmPreset> = None;
    let mut film_params = FilmParams::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out_dir = Some(PathBuf::from(it.next().context("--out needs a folder")?)),
            "--half" => params.quality = Quality::Half,
            // With a preset, exposure is applied as film exposure (log domain).
            "--ev" => film_params.exposure_ev = it.next().context("--ev needs a value")?.parse()?,
            "--no-fit" => film_params.sensor_white = None,
            "--no-scanner-colour" => film_params.scanner_colour = false,
            "--print" => film_params.output = remjet_core::film::OutputMode::Print,
            "--push" => {
                film_params.push = it.next().context("--push needs a number of stops (negative to pull)")?.parse()?
            }
            "--grain" => film_params.grain = it.next().context("--grain needs a value")?.parse()?,
            "--preset" => {
                let p = it.next().context("--preset needs a name or .toml path")?;
                preset = Some(if p.ends_with(".toml") {
                    FilmPreset::from_toml(&std::fs::read_to_string(p).with_context(|| format!("reading {p}"))?)?
                } else {
                    remjet_core::presets::load(p)?
                });
            }
            _ => inputs.push(a.clone()),
        }
    }
    match &preset {
        Some(p) if !p.supports_push(film_params.push) => {
            let have: Vec<String> = p.developments().iter().map(|d| format!("{:+} ({})", d.0, d.1)).collect();
            let have = if have.is_empty() { "none".to_string() } else { have.join(", ") };
            bail!("{} has no push/pull of {:+} stops; it has: {have}", p.name, film_params.push);
        }
        None if film_params.push != 0 => bail!("--push needs a --preset"),
        _ => {}
    }
    if preset.is_none() {
        params.exposure_ev = film_params.exposure_ev as f32;
    }
    if params.quality == Quality::Half {
        film_params.pixel_scale = 2.0;
    }
    let out_dir = out_dir.context(USAGE)?;
    if inputs.is_empty() {
        bail!(USAGE);
    }
    std::fs::create_dir_all(&out_dir)?;

    let files = collect_dngs(&inputs)?;
    let mut not_m9 = 0;
    for path in &files {
        let start = Instant::now();
        let raw_image = raw::load(path)?;
        if let Some(w) = raw::info(&raw_image).camera_warning() {
            not_m9 += 1;
            warn_camera(path, &w);
        }
        let linear = develop::develop(&raw_image, &params)?;
        let encoded = match &preset {
            Some(p) => {
                let sensor = remjet_core::sensor::geometry(&raw_image);
                let sensor_white = film_params.sensor_white.map(|_| 2f64.powf(develop::baseline_exposure(&raw_image) as f64));
                let model = FilmModel::new(p, &FilmParams { sensor_pixel_um: sensor.pixel_pitch_um, sensor_white, ..film_params.clone() })?;
                let seed = seed_for(path);
                output::display_to_srgb8(&model.apply(&linear, seed))
            }
            None => output::to_srgb8(&linear),
        };
        let img = output::apply_orientation(encoded, raw_image.orientation);
        let out = out_dir.join(path.with_extension("jpg").file_name().unwrap());
        output::write_jpeg(&img, &out, 92)?;
        println!(
            "{} -> {} ({}x{}, ~{:.0} K, {} ms)",
            path.file_name().unwrap().to_string_lossy(),
            out.display(),
            img.width(),
            img.height(),
            linear.cct,
            start.elapsed().as_millis()
        );
    }
    print_not_m9(not_m9);
    Ok(())
}
