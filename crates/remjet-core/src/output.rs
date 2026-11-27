//! Output: working space -> display-referred sRGB, orientation, JPEG.
//!
//! Every JPEG is tagged with an sRGB ICC profile (see `icc.rs`), so
//! colour-managed viewers show it as intended on wide-gamut displays.

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
    write_jpeg_with_meta(img, path, quality, None, None)
}

/// Encode a JPEG, optionally carrying EXIF (camera metadata from the source
/// DNG and the look's description) and an XMP packet.
pub fn write_jpeg_with_meta(img: &RgbImage, path: &Path, quality: u8, exif: Option<&crate::exif::ExifInfo>, xmp: Option<&str>) -> Result<()> {
    let jpeg = encode_jpeg(img, quality, exif, xmp).with_context(|| format!("encoding {}", path.display()))?;
    let file = File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut w = BufWriter::new(file);
    std::io::Write::write_all(&mut w, &jpeg).with_context(|| format!("writing {}", path.display()))
}

/// Encode a JPEG in memory: EXIF (APP1, if given), XMP (APP1, if given),
/// then the sRGB ICC profile (APP2).
pub fn encode_jpeg(img: &RgbImage, quality: u8, exif: Option<&crate::exif::ExifInfo>, xmp: Option<&str>) -> Result<Vec<u8>> {
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
    if let Some(packet) = xmp {
        enc.add_app_segment(1, crate::xmp::app1_payload(packet)).context("adding XMP")?;
    }
    enc.add_icc_profile(crate::icc::srgb_profile()).context("adding ICC profile")?;
    let (w, h) = (u16::try_from(img.width()).context("image too wide for JPEG")?, u16::try_from(img.height()).context("image too tall for JPEG")?);
    enc.encode(img.as_raw(), w, h, jpeg_encoder::ColorType::Rgb)?;
    Ok(jpeg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exif::ExifInfo;

    /// (marker, payload) of each segment before the scan data.
    fn segments(jpeg: &[u8]) -> Vec<(u8, &[u8])> {
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
        let mut out = Vec::new();
        let mut i = 2;
        while jpeg[i] == 0xFF && jpeg[i + 1] != 0xDA {
            let len = u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]) as usize;
            out.push((jpeg[i + 1], &jpeg[i + 4..i + 2 + len]));
            i += 2 + len;
        }
        out
    }

    fn sample() -> RgbImage {
        RgbImage::from_fn(48, 32, |x, y| image::Rgb([(x * 5) as u8, (y * 7) as u8, 128]))
    }

    #[test]
    fn jpeg_carries_exif_and_srgb_profile() {
        let info = ExifInfo { make: Some("Leica Camera AG".into()), model: Some("M9 Digital Camera".into()), iso: Some(160), ..Default::default() };
        let jpeg = encode_jpeg(&sample(), 92, Some(&info), None).unwrap();
        let segs = segments(&jpeg);
        let app1: Vec<_> = segs.iter().filter(|(m, _)| *m == 0xE1).collect();
        assert_eq!(app1.len(), 1);
        assert_eq!(&app1[0].1[..6], b"Exif\0\0");
        assert_eq!(&app1[0].1[6..], crate::exif::build_tiff(&info, 48, 32).as_slice());
        let icc: Vec<_> = segs.iter().filter(|(m, d)| *m == 0xE2 && d.starts_with(b"ICC_PROFILE\0")).collect();
        assert_eq!(icc.len(), 1);
        assert_eq!(&icc[0].1[12..14], &[1, 1], "single chunk, 1 of 1");
        assert_eq!(&icc[0].1[14..], crate::icc::srgb_profile());
        // EXIF comes before the profile.
        let pos = |m: u8| segs.iter().position(|s| s.0 == m).unwrap();
        assert!(pos(0xE1) < pos(0xE2));
    }

    #[test]
    fn jpeg_carries_xmp_after_exif() {
        let info = ExifInfo { description: Some("Remjet look: Portra 400, lab scan".into()), ..Default::default() };
        let xmp = crate::xmp::packet(&crate::edit::Edit::default(), Some("Kodachrome 64"), "Remjet look: Kodachrome 64, scan");
        let jpeg = encode_jpeg(&sample(), 92, Some(&info), Some(&xmp)).unwrap();
        let segs = segments(&jpeg);
        let app1: Vec<_> = segs.iter().filter(|(m, _)| *m == 0xE1).collect();
        assert_eq!(app1.len(), 2);
        assert!(app1[0].1.starts_with(b"Exif\0\0"));
        assert!(app1[1].1.starts_with(crate::xmp::APP1_HEADER));
        assert_eq!(&app1[1].1[crate::xmp::APP1_HEADER.len()..], xmp.as_bytes());
        // Still decodes, with the profile.
        let img = image::load_from_memory(&jpeg).unwrap();
        assert_eq!(img.width(), 48);
    }

    #[test]
    fn jpeg_without_exif_still_has_profile() {
        let jpeg = encode_jpeg(&sample(), 92, None, None).unwrap();
        let segs = segments(&jpeg);
        assert!(!segs.iter().any(|(m, _)| *m == 0xE1));
        assert!(segs.iter().any(|(m, d)| *m == 0xE2 && d.starts_with(b"ICC_PROFILE\0")));
    }

    #[test]
    fn decoders_see_the_profile_and_pixels() {
        use image::ImageDecoder;
        let jpeg = encode_jpeg(&sample(), 95, Some(&ExifInfo::default()), None).unwrap();
        let mut dec = image::codecs::jpeg::JpegDecoder::new(std::io::Cursor::new(&jpeg)).unwrap();
        assert_eq!(dec.icc_profile().unwrap().as_deref(), Some(crate::icc::srgb_profile()));
        assert!(dec.exif_metadata().unwrap().is_some());
        let img = image::load_from_memory(&jpeg).unwrap().to_rgb8();
        assert_eq!(img.dimensions(), (48, 32));
        let (a, b) = (img.get_pixel(20, 10), sample().get_pixel(20, 10).to_owned());
        assert!(a.0.iter().zip(b.0).all(|(x, y)| x.abs_diff(y) <= 4), "{a:?} vs {b:?}");
    }

    #[test]
    fn write_jpeg_writes_the_same_bytes() {
        let dir = std::env::temp_dir().join(format!("remjet-output-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.jpg");
        write_jpeg(&sample(), &path, 90).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), encode_jpeg(&sample(), 90, None, None).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
