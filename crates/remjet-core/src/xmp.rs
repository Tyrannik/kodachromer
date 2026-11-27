//! Record the look an export was rendered with, as XMP.
//!
//! The packet carries a readable description (`dc:description`, which
//! Lightroom, Bridge and most photo viewers show as the caption) and every
//! setting of the edit under Remjet's own namespace, so the look can be read
//! back exactly. It goes into the JPEG as a standard XMP APP1 segment.

use crate::edit::Edit;

/// Namespace for Remjet's own properties.
pub const NS: &str = "https://github.com/Tyrannik/remjet/ns/1.0/";

/// APP1 header that marks a segment as XMP.
pub const APP1_HEADER: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

/// Build the XMP packet for an export rendered with `edit`. `film_name` is
/// the preset's display name, and `description` the one-line summary.
pub fn packet(edit: &Edit, film_name: Option<&str>, description: &str) -> String {
    let software = format!("Remjet {}", env!("CARGO_PKG_VERSION"));
    let output = match edit.output {
        crate::film::OutputMode::Scan => "scan",
        crate::film::OutputMode::Print => "print",
        crate::film::OutputMode::Projection => "projection",
    };
    let attrs = [
        ("remjet:Film", edit.preset.clone().unwrap_or_else(|| "neutral".into())),
        ("remjet:FilmName", film_name.unwrap_or("No film (neutral)").to_owned()),
        ("remjet:Output", output.into()),
        ("remjet:Push", edit.push.to_string()),
        ("remjet:Exposure", num(edit.exposure_ev)),
        ("remjet:Warmth", num(edit.warmth)),
        ("remjet:Tint", num(edit.tint)),
        ("remjet:Highlights", num(edit.highlights)),
        ("remjet:FitHighlights", edit.fit_highlights.to_string()),
        ("remjet:ScannerColour", edit.scanner_colour.to_string()),
        ("remjet:Grain", num(edit.grain)),
        ("remjet:GrainSize", num(edit.grain_size)),
        ("remjet:Sharpen", num(edit.sharpen)),
        ("remjet:Halation", num(edit.halation)),
        ("remjet:EditVersion", edit.version.to_string()),
    ];
    let mut s = String::new();
    s.push_str("<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n");
    s.push_str("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n");
    s.push_str(" <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n");
    s.push_str("  <rdf:Description rdf:about=\"\"\n");
    s.push_str("    xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n");
    s.push_str("    xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"\n");
    s.push_str(&format!("    xmlns:remjet=\"{NS}\"\n"));
    s.push_str(&format!("    xmp:CreatorTool=\"{}\"", escape(&software)));
    for (k, v) in attrs {
        s.push_str(&format!("\n    {k}=\"{}\"", escape(&v)));
    }
    s.push_str(">\n");
    s.push_str("   <dc:description>\n    <rdf:Alt>\n");
    s.push_str(&format!("     <rdf:li xml:lang=\"x-default\">{}</rdf:li>\n", escape(description)));
    s.push_str("    </rdf:Alt>\n   </dc:description>\n");
    s.push_str("  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n");
    s.push_str("<?xpacket end=\"w\"?>");
    s
}

/// The APP1 payload (header + packet).
pub fn app1_payload(packet: &str) -> Vec<u8> {
    [APP1_HEADER, packet.as_bytes()].concat()
}

fn num(v: f32) -> String {
    // Round away float noise from the sliders (0.30000001 -> 0.3).
    let r = (v as f64 * 1000.0).round() / 1000.0;
    if r == 0.0 { "0".into() } else { format!("{r}") }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c if (c as u32) < 0x20 && c != '\n' && c != '\t' => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_records_every_setting() {
        let edit = Edit { preset: Some("portra400".into()), exposure_ev: 0.5, warmth: -0.2, push: 1, ..Default::default() };
        let p = packet(&edit, Some("Portra 400"), "Portra 400 \"lab\" scan & more");
        assert!(p.starts_with("<?xpacket begin="));
        assert!(p.ends_with("<?xpacket end=\"w\"?>"));
        for needle in [
            "remjet:Film=\"portra400\"",
            "remjet:FilmName=\"Portra 400\"",
            "remjet:Output=\"scan\"",
            "remjet:Push=\"1\"",
            "remjet:Exposure=\"0.5\"",
            "remjet:Warmth=\"-0.2\"",
            "remjet:Sharpen=\"0.3\"",
            "remjet:FitHighlights=\"true\"",
            "Portra 400 &quot;lab&quot; scan &amp; more",
        ] {
            assert!(p.contains(needle), "missing {needle} in\n{p}");
        }
    }

    #[test]
    fn neutral_render_is_labelled() {
        let edit = Edit { preset: None, ..Default::default() };
        let p = packet(&edit, None, "No film");
        assert!(p.contains("remjet:Film=\"neutral\""));
    }
}
