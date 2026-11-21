//! Output: working space -> display-referred sRGB, orientation, JPEG.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use anyhow::{Context, Result};
use image::RgbImage;
use image::codecs::jpeg::JpegEncoder;
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
    let file = File::create(path).with_context(|| format!("creating {}", path.display()))?;
    JpegEncoder::new_with_quality(BufWriter::new(file), quality)
        .encode_image(img)
        .with_context(|| format!("encoding {}", path.display()))
}
