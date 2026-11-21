//! Carry camera metadata from the DNG into exported JPEGs.
//!
//! Reads the parsed EXIF from the DNG via rawler, writes a minimal, standard
//! EXIF block (TIFF little-endian, IFD0 + Exif sub-IFD) and inserts it as an
//! APP1 segment. Orientation is always written as 1 because exports are
//! already rotated upright.

use std::path::Path;

use anyhow::{Context, Result};
use rawler::decoders::RawDecodeParams;
use rawler::rawsource::RawSource;

/// The subset of metadata we carry over.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExifInfo {
    pub make: Option<String>,
    pub model: Option<String>,
    pub artist: Option<String>,
    pub copyright: Option<String>,
    pub date_time_original: Option<String>,
    pub create_date: Option<String>,
    pub exposure_time: Option<(u32, u32)>,
    pub fnumber: Option<(u32, u32)>,
    pub iso: Option<u16>,
    pub exposure_bias: Option<(i32, i32)>,
    pub focal_length: Option<(u32, u32)>,
    pub lens_make: Option<String>,
    pub lens_model: Option<String>,
    pub serial_number: Option<String>,
}

pub fn read(path: &Path) -> Result<ExifInfo> {
    let src = RawSource::new(path).with_context(|| format!("opening {}", path.display()))?;
    let decoder = rawler::get_decoder(&src)?;
    let md = decoder.raw_metadata(&src, &RawDecodeParams::default())?;
    let e = md.exif;
    let nonempty = |s: Option<String>| s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    Ok(ExifInfo {
        make: nonempty(Some(md.make)),
        model: nonempty(Some(md.model)),
        artist: nonempty(e.artist),
        copyright: nonempty(e.copyright),
        date_time_original: nonempty(e.date_time_original),
        create_date: nonempty(e.create_date),
        exposure_time: e.exposure_time.map(|r| (r.n, r.d)),
        fnumber: e.fnumber.map(|r| (r.n, r.d)),
        iso: e.iso_speed_ratings,
        exposure_bias: e.exposure_bias.map(|r| (r.n, r.d)),
        focal_length: e.focal_length.map(|r| (r.n, r.d)),
        lens_make: nonempty(e.lens_make),
        lens_model: nonempty(e.lens_model),
        serial_number: nonempty(e.serial_number),
    })
}

// ------------------------------------------------------------ TIFF writer

const ASCII: u16 = 2;
const SHORT: u16 = 3;
const LONG: u16 = 4;
const RATIONAL: u16 = 5;
const SRATIONAL: u16 = 10;

struct Entry {
    tag: u16,
    typ: u16,
    count: u32,
    data: Vec<u8>,
}

#[derive(Default)]
struct Ifd {
    entries: Vec<Entry>,
}

impl Ifd {
    fn ascii(&mut self, tag: u16, s: &Option<String>) {
        if let Some(s) = s {
            let mut data = s.as_bytes().to_vec();
            data.push(0);
            self.entries.push(Entry { tag, typ: ASCII, count: data.len() as u32, data });
        }
    }
    fn short(&mut self, tag: u16, v: u16) {
        self.entries.push(Entry { tag, typ: SHORT, count: 1, data: v.to_le_bytes().to_vec() });
    }
    fn long(&mut self, tag: u16, v: u32) {
        self.entries.push(Entry { tag, typ: LONG, count: 1, data: v.to_le_bytes().to_vec() });
    }
    fn rational(&mut self, tag: u16, v: Option<(u32, u32)>) {
        if let Some((n, d)) = v.filter(|r| r.1 != 0) {
            let data = [n.to_le_bytes(), d.to_le_bytes()].concat();
            self.entries.push(Entry { tag, typ: RATIONAL, count: 1, data });
        }
    }
    fn srational(&mut self, tag: u16, v: Option<(i32, i32)>) {
        if let Some((n, d)) = v.filter(|r| r.1 != 0) {
            let data = [n.to_le_bytes(), d.to_le_bytes()].concat();
            self.entries.push(Entry { tag, typ: SRATIONAL, count: 1, data });
        }
    }

    /// Bytes this IFD occupies (directory + out-of-line data).
    fn size(&self) -> usize {
        2 + 12 * self.entries.len() + 4 + self.entries.iter().filter(|e| e.data.len() > 4).map(|e| e.data.len().next_multiple_of(2)).sum::<usize>()
    }

    /// Serialise at `offset` (relative to the TIFF header).
    fn write(mut self, offset: usize, out: &mut Vec<u8>) {
        self.entries.sort_by_key(|e| e.tag);
        let mut data_offset = offset + 2 + 12 * self.entries.len() + 4;
        let mut data_area = Vec::new();
        out.extend((self.entries.len() as u16).to_le_bytes());
        for e in &self.entries {
            out.extend(e.tag.to_le_bytes());
            out.extend(e.typ.to_le_bytes());
            out.extend(e.count.to_le_bytes());
            if e.data.len() <= 4 {
                let mut v = e.data.clone();
                v.resize(4, 0);
                out.extend(v);
            } else {
                out.extend((data_offset as u32).to_le_bytes());
                data_area.extend(&e.data);
                if e.data.len() % 2 == 1 {
                    data_area.push(0);
                }
                data_offset += e.data.len().next_multiple_of(2);
            }
        }
        out.extend(0u32.to_le_bytes()); // no next IFD
        out.extend(data_area);
    }
}

/// Build the TIFF-structured EXIF payload (without the APP1 header).
pub fn build_tiff(info: &ExifInfo, width: u32, height: u32) -> Vec<u8> {
    let mut ifd0 = Ifd::default();
    ifd0.ascii(0x010F, &info.make);
    ifd0.ascii(0x0110, &info.model);
    ifd0.short(0x0112, 1); // orientation: pixels are already upright
    ifd0.ascii(0x0131, &Some(format!("Kodachromer {}", env!("CARGO_PKG_VERSION"))));
    ifd0.ascii(0x0132, &info.date_time_original);
    ifd0.ascii(0x013B, &info.artist);
    ifd0.ascii(0x8298, &info.copyright);

    let mut exif = Ifd::default();
    exif.rational(0x829A, info.exposure_time);
    exif.rational(0x829D, info.fnumber);
    if let Some(iso) = info.iso {
        exif.short(0x8827, iso);
    }
    exif.entries.push(Entry { tag: 0x9000, typ: 7, count: 4, data: b"0232".to_vec() }); // ExifVersion
    exif.ascii(0x9003, &info.date_time_original);
    exif.ascii(0x9004, &info.create_date.clone().or_else(|| info.date_time_original.clone()));
    exif.srational(0x9204, info.exposure_bias);
    exif.rational(0x920A, info.focal_length);
    exif.short(0xA001, 1); // ColorSpace: sRGB
    exif.long(0xA002, width);
    exif.long(0xA003, height);
    exif.ascii(0xA431, &info.serial_number);
    exif.ascii(0xA433, &info.lens_make);
    exif.ascii(0xA434, &info.lens_model);

    // IFD0 gets a pointer to the Exif IFD; its size includes that entry.
    ifd0.long(0x8769, 0);
    let exif_offset = 8 + ifd0.size();
    ifd0.entries.last_mut().expect("just pushed").data = (exif_offset as u32).to_le_bytes().to_vec();

    let mut out = b"II*\0".to_vec();
    out.extend(8u32.to_le_bytes());
    ifd0.write(8, &mut out);
    exif.write(exif_offset, &mut out);
    out
}

/// Insert an EXIF APP1 segment into an encoded JPEG (after SOI/APP0).
pub fn insert_into_jpeg(jpeg: &[u8], tiff: &[u8]) -> Result<Vec<u8>> {
    anyhow::ensure!(jpeg.starts_with(&[0xFF, 0xD8]), "not a JPEG");
    let payload_len = 2 + 6 + tiff.len();
    anyhow::ensure!(payload_len <= u16::MAX as usize, "EXIF block too large");
    let mut app1 = vec![0xFF, 0xE1];
    app1.extend((payload_len as u16).to_be_bytes());
    app1.extend(b"Exif\0\0");
    app1.extend(tiff);

    // Keep a JFIF APP0 first if present (some readers expect it there).
    let mut insert_at = 2;
    if jpeg.len() > 6 && jpeg[2] == 0xFF && jpeg[3] == 0xE0 {
        insert_at = 4 + u16::from_be_bytes([jpeg[4], jpeg[5]]) as usize;
    }
    let mut out = Vec::with_capacity(jpeg.len() + app1.len());
    out.extend(&jpeg[..insert_at]);
    out.extend(app1);
    out.extend(&jpeg[insert_at..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_u16(b: &[u8], o: usize) -> u16 {
        u16::from_le_bytes([b[o], b[o + 1]])
    }
    fn read_u32(b: &[u8], o: usize) -> u32 {
        u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
    }

    /// Find a tag in the IFD at `off`, returning (type, count, value/offset).
    fn find(b: &[u8], off: usize, tag: u16) -> Option<(u16, u32, u32)> {
        let n = read_u16(b, off) as usize;
        (0..n).map(|i| off + 2 + 12 * i).find(|&e| read_u16(b, e) == tag).map(|e| (read_u16(b, e + 2), read_u32(b, e + 4), read_u32(b, e + 8)))
    }

    #[test]
    fn tiff_structure_is_valid() {
        let info = ExifInfo {
            make: Some("Leica Camera AG".into()),
            model: Some("M9 Digital Camera".into()),
            date_time_original: Some("2013:05:01 12:34:56".into()),
            exposure_time: Some((1, 250)),
            fnumber: Some((56, 10)),
            iso: Some(160),
            ..Default::default()
        };
        let t = build_tiff(&info, 5212, 3468);
        assert_eq!(&t[..4], b"II*\0");
        let ifd0 = read_u32(&t, 4) as usize;
        // Make string resolves through its offset.
        let (typ, count, at) = find(&t, ifd0, 0x010F).unwrap();
        assert_eq!(typ, ASCII);
        assert_eq!(&t[at as usize..at as usize + count as usize - 1], b"Leica Camera AG");
        // Orientation is upright.
        assert_eq!(find(&t, ifd0, 0x0112).unwrap().2 & 0xFFFF, 1);
        // Exif IFD pointer lands on a directory containing the exposure time.
        let exif = find(&t, ifd0, 0x8769).unwrap().2 as usize;
        let (typ, _, at) = find(&t, exif, 0x829A).unwrap();
        assert_eq!(typ, RATIONAL);
        assert_eq!((read_u32(&t, at as usize), read_u32(&t, at as usize + 4)), (1, 250));
        assert_eq!(find(&t, exif, 0x8827).unwrap().2 & 0xFFFF, 160);
        assert_eq!(find(&t, exif, 0xA002).unwrap().2, 5212);
    }

    #[test]
    fn app1_inserted_after_app0() {
        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0xAA, 0xBB, 0xFF, 0xDA, 0x00];
        let out = insert_into_jpeg(&jpeg, b"II*\0\x08\0\0\0").unwrap();
        assert_eq!(&out[..8], &jpeg[..8]);
        assert_eq!(&out[8..10], &[0xFF, 0xE1]);
        assert_eq!(&out[12..18], b"Exif\0\0");
        assert!(out.ends_with(&[0xFF, 0xDA, 0x00]));
    }
}
