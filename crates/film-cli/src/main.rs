//! Command-line front end.
//!
//!   film-cli info   <files or folders...>
//!   film-cli render <files or folders...> --out <folder> [--half] [--ev <stops>]

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use film_core::develop::{self, DevelopParams, Quality};
use film_core::{output, raw};

const USAGE: &str = "usage:\n  film-cli info <file.dng | folder>...\n  film-cli render <file.dng | folder>... --out <folder> [--half] [--ev <stops>]";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.split_first() {
        Some((cmd, rest)) if cmd == "info" && !rest.is_empty() => info(rest),
        Some((cmd, rest)) if cmd == "render" => render(rest),
        _ => bail!(USAGE),
    }
}

fn collect_dngs(paths: &[String]) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for p in paths.iter().map(Path::new) {
        if p.is_dir() {
            for entry in std::fs::read_dir(p)? {
                let path = entry?.path();
                if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("dng")) {
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
    let mut failures = 0;
    for path in &files {
        let start = Instant::now();
        match raw::load(path) {
            Ok(image) => {
                let i = raw::info(&image);
                println!(
                    "{}: {} {} {}x{} {}bit black={:?} white={:?} wb={:?} baseline_ev={} orient={:?} matrices={:?} crop={:?} lintable={} ({} ms)",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    i.make, i.model, i.width, i.height, i.bits_per_sample,
                    i.black_levels, i.white_levels, develop::as_shot_wb(&image),
                    develop::baseline_exposure(&image), image.orientation,
                    i.color_matrix_illuminants, i.crop, i.has_linearization_table,
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
    Ok(())
}

fn render(args: &[String]) -> Result<()> {
    let mut inputs = Vec::new();
    let mut out_dir = None;
    let mut params = DevelopParams::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out_dir = Some(PathBuf::from(it.next().context("--out needs a folder")?)),
            "--half" => params.quality = Quality::Half,
            "--ev" => params.exposure_ev = it.next().context("--ev needs a value")?.parse()?,
            _ => inputs.push(a.clone()),
        }
    }
    let out_dir = out_dir.context(USAGE)?;
    if inputs.is_empty() {
        bail!(USAGE);
    }
    std::fs::create_dir_all(&out_dir)?;

    let files = collect_dngs(&inputs)?;
    for path in &files {
        let start = Instant::now();
        let raw_image = raw::load(path)?;
        let linear = develop::develop(&raw_image, &params)?;
        let img = output::apply_orientation(output::to_srgb8(&linear), raw_image.orientation);
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
    Ok(())
}
