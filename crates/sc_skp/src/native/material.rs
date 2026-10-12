//! `materials/<name>/material.xml` — SketchUp's own description of a material.
//!
//! The schema is plain and stable:
//!
//! ```xml
//! <mat:material name="[Roofing Metal]" colorRed="53" colorGreen="49" colorBlue="51"
//!               trans="0" useTrans="0" hasTexture="1">
//!   <mat:texture textureFilename="Roofing.jpg" xScale="24" yScale="24" avgColor="4281878181"/>
//! </mat:material>
//! ```
//!
//! `xScale` / `yScale` are the model-space size of one tile in inches — the same quantity
//! [`crate::SkpTexture::size_inches`] carries, so it crosses over unchanged.

use quick_xml::events::Event;

/// A material read straight out of the file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NativeMaterial {
    pub name: String,
    /// sRGB + alpha.
    pub color: [u8; 4],
    /// Image file named by the material, if it has one.
    pub texture_file: Option<String>,
    /// Size of one tile in inches.
    pub texture_scale: [f64; 2],
    /// Index into [`super::NativeSkp::textures`], once the image has been found.
    pub texture: Option<usize>,
}

/// Parse one `material.xml`. `None` if it holds no material element.
pub fn parse(xml: &str) -> Option<NativeMaterial> {
    let mut r = quick_xml::Reader::from_str(xml);
    r.config_mut().trim_text(true);
    let mut m = NativeMaterial { color: [255, 255, 255, 255], texture_scale: [1.0, 1.0], ..Default::default() };
    let mut found = false;
    // Transparency only applies when the material says to use it.
    let (mut trans, mut use_trans) = (1.0f64, false);

    loop {
        let ev = r.read_event().ok()?;
        let e = match &ev {
            Event::Start(e) | Event::Empty(e) => e,
            Event::Eof => break,
            _ => continue,
        };
        // Tag names arrive namespace-qualified ("mat:material"); match on the local part.
        let name = e.name();
        let local = name.as_ref().rsplit(|b| *b == b':').next().unwrap_or_default().to_vec();
        let attr = |key: &str| -> Option<String> {
            let raw = e.attributes().flatten().find(|a| a.key.as_ref() == key.as_bytes())?;
            quick_xml::escape::unescape(&String::from_utf8_lossy(&raw.value)).ok().map(|v| v.into_owned())
        };
        match local.as_slice() {
            b"material" => {
                found = true;
                m.name = attr("name").unwrap_or_default();
                let ch = |k: &str| attr(k).and_then(|v| v.parse::<i64>().ok()).unwrap_or(255).clamp(0, 255) as u8;
                m.color = [ch("colorRed"), ch("colorGreen"), ch("colorBlue"), 255];
                trans = attr("trans").and_then(|v| v.parse().ok()).unwrap_or(1.0);
                use_trans = attr("useTrans").is_some_and(|v| v != "0");
            }
            b"texture" => {
                m.texture_file = attr("textureFilename").filter(|f| !f.is_empty());
                let sc = |k: &str| attr(k).and_then(|v| v.parse::<f64>().ok()).filter(|v| *v > 0.0).unwrap_or(1.0);
                m.texture_scale = [sc("xScale"), sc("yScale")];
            }
            _ => {}
        }
    }
    if !found {
        return None;
    }
    if use_trans {
        m.color[3] = (trans.clamp(0.0, 1.0) * 255.0).round() as u8;
    }
    Some(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXTURED: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no" ?>
<materialDocument xmlns:mat="http://sketchup.google.com/schemas/sketchup/1.0/material">
  <mat:material name="[Roofing Metal Standing Seam Red]1" type="2" colorRed="53" colorGreen="49" colorBlue="51" trans="0" useTrans="0" hasTexture="1">
    <mat:texture textureFilename="Roofing_Metal_Standing_Seam_Red.jpg" xScale="24" yScale="36" avgColor="4281878181">
      <mat:images><mat:image id="1" path="./Roofing.jpg"/></mat:images>
    </mat:texture>
  </mat:material>
</materialDocument>"#;

    #[test]
    fn reads_a_textured_material() {
        let m = parse(TEXTURED).expect("a material");
        assert_eq!(m.name, "[Roofing Metal Standing Seam Red]1");
        assert_eq!(m.color, [53, 49, 51, 255]);
        assert_eq!(m.texture_file.as_deref(), Some("Roofing_Metal_Standing_Seam_Red.jpg"));
        assert_eq!(m.texture_scale, [24.0, 36.0]);
    }

    /// `trans` is ignored unless `useTrans` says to apply it — SketchUp writes trans="0" on
    /// plenty of fully opaque materials, and honouring it blindly makes them invisible.
    #[test]
    fn transparency_needs_use_trans() {
        assert_eq!(parse(TEXTURED).unwrap().color[3], 255, "trans=0 with useTrans=0 stays opaque");
        let faded = TEXTURED.replace(r#"trans="0" useTrans="0""#, r#"trans="0.5" useTrans="1""#);
        assert_eq!(parse(&faded).unwrap().color[3], 128);
    }

    #[test]
    fn reads_a_plain_colour_material() {
        let xml = r#"<materialDocument><mat:material name="[Color M00]" colorRed="255" colorGreen="128" colorBlue="0" hasTexture="0"/></materialDocument>"#;
        let m = parse(xml).unwrap();
        assert_eq!(m.color, [255, 128, 0, 255]);
        assert!(m.texture_file.is_none());
    }

    /// Names are XML-escaped in the file and must come back as written.
    #[test]
    fn unescapes_names() {
        let xml = r#"<materialDocument><mat:material name="Black &amp; White" colorRed="0" colorGreen="0" colorBlue="0"/></materialDocument>"#;
        assert_eq!(parse(xml).unwrap().name, "Black & White");
    }

    #[test]
    fn rejects_documents_without_a_material() {
        assert!(parse("<materialDocument></materialDocument>").is_none());
        assert!(parse("not xml at all <<<").is_none());
    }
}
