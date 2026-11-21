//! Output: working space -> display-referred sRGB, orientation, JPEG.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use anyhow::{Context, Result};
use image::RgbImage;
use rawler::decoders::Orientation;
use rayon::prelude::*;

use crate::color;
use crate::develop::LinearImage;

/// Convert a working-space image to 8-bit sRGB. Out-of-gamut and >1.0 values
/// are clipped per channel (a neutral, look-free transform).
pub fn to_srgb8(img: &LinearImage) -> RgbImage {
    let m = color::working_to_srgb().map(|row| row.map(|v| v as f32));
    let mut buf = vec![0u8; img.width * img.height * 3];
    buf.par_chunks_exact_mut(3).zip(img.pixels.par_iter()).for_each(|(out, p)| {
        for i in 0..3 {
            let lin = m[i][0] * p[0] + m[i][1] * p[1] + m[i][2] * p[2];
            out[i] = (color::srgb_encode(lin.clamp(0.0, 1.0)) * 255.0 + 0.5) as u8;
        }
    });
    RgbImage::from_raw(img.width as u32, img.height as u32, buf).expect("buffer size matches")
}

/// Encode an image whose pixels are already display-linear sRGB (e.g. the
/// film model's scanner output) to 8-bit.
pub fn display_to_srgb8(img: &LinearImage) -> RgbImage {
    let mut buf = vec![0u8; img.width * img.height * 3];
    buf.par_chunks_exact_mut(3).zip(img.pixels.par_iter()).for_each(|(out, p)| {
        for i in 0..3 {
            out[i] = (color::srgb_encode(p[i].clamp(0.0, 1.0)) * 255.0 + 0.5) as u8;
        }
    });
    RgbImage::from_raw(img.width as u32, img.height as u32, buf).expect("buffer size matches")
}

/// Rotate/flip so the image displays upright.
pub fn apply_orientation(img: RgbImage, orientation: Orientation) -> RgbImage {
    use image::imageops::{flip_horizontal, flip_vertical, rotate90, rotate180, rotate270};
    match orientation {
        Orientation::Normal | Orientation::Unknown => img,
        Orientation::HorizontalFlip => flip_horizontal(&img),
        Orientation::Rotate180 => rotate180(&img),
        Orientation::VerticalFlip => flip_vertical(&img),
        Orientation::Transpose => flip_horizontal(&rotate90(&img)),
        Orientation::Rotate90 => rotate90(&img),
        Orientation::Transverse => flip_horizontal(&rotate270(&img)),
        Orientation::Rotate270 => rotate270(&img),
    }
}

pub fn write_jpeg(img: &RgbImage, path: &Path, quality: u8) -> Result<()> {
    write_jpeg_with_exif(img, path, quality, None)
}

/// Encode a JPEG, optionally carrying camera metadata from the source DNG.
pub fn write_jpeg_with_exif(img: &RgbImage, path: &Path, quality: u8, exif: Option<&crate::exif::ExifInfo>) -> Result<()> {
    // Full-resolution colour (4:4:4) and optimised Huffman tables: noticeably
    // cleaner fine colour detail and grain than the default 4:2:0.
    let mut jpeg = Vec::new();
    let mut enc = jpeg_encoder::Encoder::new(&mut jpeg, quality);
    enc.set_sampling_factor(jpeg_encoder::SamplingFactor::R_4_4_4);
    enc.set_optimized_huffman_tables(true);
    if let Some(info) = exif {
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend(crate::exif::build_tiff(info, img.width(), img.height()));
        enc.add_app_segment(1, app1).context("adding EXIF")?;
    }
    let (w, h) = (u16::try_from(img.width()).context("image too wide for JPEG")?, u16::try_from(img.height()).context("image too tall for JPEG")?);
    enc.encode(img.as_raw(), w, h, jpeg_encoder::ColorType::Rgb).with_context(|| format!("encoding {}", path.display()))?;
    let file = File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut w = BufWriter::new(file);
    std::io::Write::write_all(&mut w, &jpeg).with_context(|| format!("writing {}", path.display()))
}
