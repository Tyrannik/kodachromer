//! Run the Android app's code path on the desktop, to check output, timing
//! and peak memory without a phone:
//!
//!   cargo run --release -p remjet-android --example android_render -- <dng> <film|""> <out dir> [full|half]

use std::path::Path;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [dng, film, out_dir, rest @ ..] = args.as_slice() else {
        anyhow::bail!("usage: android_render <dng> <film|\"\"> <out dir> [full|half]");
    };
    let full = rest.first().map(String::as_str) != Some("half");
    let (dng, out_dir) = (Path::new(dng), Path::new(out_dir));
    let name = dng.file_name().unwrap().to_string_lossy();
    let film = Some(film.as_str()).filter(|f| !f.is_empty());
    std::fs::create_dir_all(out_dir)?;

    let t = Instant::now();
    let jpeg = remjet_android::render_preview(dng, &name, film)?;
    std::fs::write(out_dir.join("preview.jpg"), jpeg)?;
    println!("first preview {} ms", t.elapsed().as_millis());
    let t = Instant::now();
    remjet_android::render_preview(dng, &name, Some("portra400"))?;
    println!("film switch preview {} ms", t.elapsed().as_millis());

    let t = Instant::now();
    remjet_android::export(dng, &name, film, &out_dir.join("export.jpg"), full, 92)?;
    println!("{} export {} ms", if full { "full" } else { "half" }, t.elapsed().as_millis());
    Ok(())
}
