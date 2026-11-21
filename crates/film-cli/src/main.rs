//! Command-line front end. For now: `film-cli info <files or folders...>`
//! prints what the decoder sees in each DNG.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Result, bail};
use film_core::raw;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.split_first() {
        Some((cmd, paths)) if cmd == "info" && !paths.is_empty() => info(paths),
        _ => bail!("usage: film-cli info <file.dng | folder>..."),
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
                    "{}: {} {} {}x{} {}bit black={:?} white={:?} wb={:?} matrices={:?} crop={:?} lintable={} float={} ({} ms)",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    i.make, i.model, i.width, i.height, i.bits_per_sample,
                    i.black_levels, i.white_levels, i.as_shot_wb, i.color_matrix_illuminants,
                    i.crop, i.has_linearization_table, i.is_float,
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
