//! Times the app's fit-preview render (the work behind every slider move).
//! `cargo run --release -p remjet-core --features gpu --example preview_bench -- FILE.DNG...`

use std::time::Instant;

use remjet_core::develop::{self, Quality};
use remjet_core::edit::{self, Edit, OutputMode};
use remjet_core::raw;

const PREVIEW_EDGE: usize = 1800;

fn main() -> anyhow::Result<()> {
    let files: Vec<String> = std::env::args().skip(1).collect();
    let films = ["kodachrome64", "ektachrome-e100", "portra400", "trix400"];
    for f in &files {
        let path = std::path::Path::new(f);
        let t = Instant::now();
        let raw_image = raw::load(path)?;
        let cam = develop::demosaic(&raw_image, None, Quality::Half)?;
        let (small, fct) = cam.downscale_to(PREVIEW_EDGE);
        let scale = 2.0 * fct as f64;
        println!("{f}: open {:.0} ms ({}x{})", t.elapsed().as_secs_f64() * 1e3, small.width, small.height);
        let seed = edit::seed_for(path);
        for film in films {
            stages(&small, film, scale)?;
        }
        for film in films {
            for (label, e) in [
                ("default", Edit { preset: Some(film.into()), ..Default::default() }),
                ("halation", Edit { preset: Some(film.into()), halation: Some(0.6), ..Default::default() }),
                ("print", Edit { preset: Some(film.into()), output: OutputMode::Print, ..Default::default() }),
            ] {
                let mut times = Vec::new();
                for i in 0..5 {
                    let e = Edit { exposure_ev: i as f32 * 0.1, ..e.clone() };
                    let t = Instant::now();
                    let _ = edit::render(&small, &e, scale, seed)?;
                    times.push(t.elapsed().as_secs_f64() * 1e3);
                }
                times.sort_by(f64::total_cmp);
                println!("  {film:16} {label:9} median {:6.1} ms", times[2]);
            }
        }
        #[cfg(feature = "gpu")]
        if let Ok(gpu) = remjet_core::gpu::Gpu::with_options(true) {
            println!("  GPU {}", gpu.name);
            let img = gpu.upload(&small)?;
            for film in films {
                let e = Edit { preset: Some(film.into()), ..Default::default() };
                let _ = gpu.render(&img, &e, None, scale, seed)?;
                let mut times = Vec::new();
                for i in 0..5 {
                    let e = Edit { exposure_ev: i as f32 * 0.1, ..e.clone() };
                    let t = Instant::now();
                    let _ = gpu.render(&img, &e, None, scale, seed)?;
                    times.push(t.elapsed().as_secs_f64() * 1e3);
                }
                times.sort_by(f64::total_cmp);
                println!("  GPU {film:16} median {:6.1} ms", times[2]);
            }
        }
        let t = Instant::now();
        let _ = edit::render(&small, &Edit { preset: None, ..Default::default() }, scale, seed)?;
        println!("  neutral                    {:6.1} ms", t.elapsed().as_secs_f64() * 1e3);
    }
    Ok(())
}

/// Time of each CPU stage for one film.
fn stages(small: &develop::CameraImage, film: &str, scale: f64) -> anyhow::Result<()> {
    use remjet_core::film::{FilmModel, FilmParams};
    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1e3;
    let e = Edit { preset: Some(film.into()), ..Default::default() };
    let t = Instant::now();
    let mut lin = develop::to_working(small, e.wb_adjust(), e.exposure_ev)?;
    let a = ms(t);
    let t = Instant::now();
    develop::sharpen(&mut lin, e.sharpen, (0.8 / scale) as f32);
    let b = ms(t);
    let t = Instant::now();
    let p = remjet_core::presets::load(film)?;
    let c = ms(t);
    let t = Instant::now();
    let params = FilmParams { pixel_scale: scale, sensor_white: Some(small.clip_white()), ..Default::default() };
    let m = FilmModel::new(&p, &params)?;
    let d = ms(t);
    let t = Instant::now();
    let out = m.apply(&lin, 1);
    let f = ms(t);
    let t = Instant::now();
    let _ = remjet_core::output::display_to_srgb8(&out);
    let g = ms(t);
    let t = Instant::now();
    let _ = remjet_core::film::FilmModel::new(&p, &FilmParams { grain: 0.0, ..params.clone() })?.apply(&lin, 1);
    let h = ms(t);
    println!("    {film}: working {a:.1} sharpen {b:.1} preset {c:.1} model {d:.1} apply {f:.1} (no grain {h:.1}) srgb8 {g:.1}");
    Ok(())
}
