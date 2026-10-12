//! Reading `.skp` / `.skb` files directly, without Trimble's library.
//!
//! SketchUp has used two unrelated containers:
//!
//! * **Newer files** (seen from SketchUp 2021 on) are a zip. `model.dat` holds the geometry,
//!   `materials/<name>/material.xml` describes each material next to its texture image, and
//!   `meta/model_thumbnail.png` is the preview SketchUp shows in a file browser.
//! * **Older files** are one MFC `CArchive` object stream — the class names (`CVersionMap`,
//!   `CArcCurve`) sit in plain sight near the front, each followed by that class's own private
//!   layout.
//!
//! Both start with the same UTF-16 header naming the version that wrote them, which is how
//! [`crate::file_version`] works and why it works on every file regardless of container.
//!
//! **What this module does and does not do.** It reads the container: version, materials,
//! textures and the thumbnail. It does *not* read geometry. Geometry lives in `model.dat` as a
//! [`tlv`] tree whose keys are raw numbers with no names attached — the framing is understood
//! (see [`tlv`]) but the vocabulary is not, and working it out is an open task. Until then
//! geometry still comes from the SketchUp library via [`crate::locate`].

pub mod model;
pub mod tlv;

mod material;
pub use material::NativeMaterial;

use crate::SkpTexture;
use std::io::{Cursor, Read};
use std::path::Path;

/// Which container a file uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// A zip of `model.dat`, `materials/`, `styles/` and `meta/`.
    Zip,
    /// One MFC `CArchive` object stream.
    Legacy,
}

/// What we can read from a `.skp` without the SketchUp library.
#[derive(Debug, Default)]
pub struct NativeSkp {
    /// The version that wrote the file, e.g. "26.2.0".
    pub version: String,
    pub materials: Vec<NativeMaterial>,
    pub textures: Vec<SkpTexture>,
    /// `meta/model_thumbnail.png`, the preview SketchUp stores for file browsers.
    pub thumbnail: Option<Vec<u8>>,
    /// The raw geometry stream, kept for [`tlv`] inspection. Not decoded.
    pub model_dat: Option<Vec<u8>>,
}

impl NativeSkp {
    /// Human-readable name of the writing version, e.g. "SketchUp 2026".
    pub fn version_name(&self) -> String {
        crate::version_name(&self.version)
    }
}

/// Which container `bytes` uses. Decided by looking for the zip, not by version number, so a
/// file from a version we have never seen is still placed correctly.
pub fn format(bytes: &[u8]) -> Format {
    if zip_start(bytes).is_some() { Format::Zip } else { Format::Legacy }
}

/// Where the zip begins. Newer files put a short header in front of it, so the archive does not
/// start at byte 0 and has to be found.
fn zip_start(bytes: &[u8]) -> Option<usize> {
    // Only the header can precede it; scanning further would risk hitting the bytes of a
    // compressed entry that happen to spell the same signature.
    const MAX_HEADER: usize = 1024;
    let limit = bytes.len().min(MAX_HEADER);
    bytes[..limit].windows(4).position(|w| w == b"PK\x03\x04")
}

/// Read everything we can from a `.skp` / `.skb` without the SketchUp library.
///
/// Legacy files yield their version and nothing more — see the module note.
pub fn read(path: &Path) -> Result<NativeSkp, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    read_bytes(&bytes)
}

/// Read a `.skp` / `.skb` whole — geometry included — with no SketchUp library involved.
///
/// Only the modern zip container can be read this way; older files still need the library, and
/// say so rather than coming back empty.
pub fn read_model(path: &Path) -> Result<crate::SkpModel, String> {
    let f = read(path)?;
    let Some(dat) = &f.model_dat else {
        return Err(format!("{} was written by {}, which stores its geometry in the older format SketchCAD cannot read on its own yet.", path.display(), f.version_name()));
    };
    let mut m = model::read(dat, f.version.clone())?;
    // Materials come from the container, which is already understood; only their assignment to
    // faces is still undecoded, so they arrive as a palette rather than painted on.
    m.materials = f.materials.iter().map(|mat| crate::SkpMaterial { name: mat.name.clone(), color: mat.color, texture: mat.texture }).collect();
    m.textures = f.textures;
    Ok(m)
}

pub fn read_bytes(bytes: &[u8]) -> Result<NativeSkp, String> {
    let version = crate::file_version_from(bytes).unwrap_or_default();
    let Some(start) = zip_start(bytes) else {
        // An MFC stream. The container is a chain of class names each followed by that class's
        // own layout, so there is nothing to offer yet beyond the version.
        return Ok(NativeSkp { version, ..Default::default() });
    };
    let mut zip = zip::ZipArchive::new(Cursor::new(&bytes[start..])).map_err(|e| format!("Not a readable SketchUp archive: {e}"))?;

    let names: Vec<String> = zip.file_names().map(str::to_string).collect();
    let read_entry = |zip: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str| -> Option<Vec<u8>> {
        let mut f = zip.by_name(name).ok()?;
        let mut v = Vec::new();
        f.read_to_end(&mut v).ok()?;
        Some(v)
    };

    let thumbnail = read_entry(&mut zip, "meta/model_thumbnail.png");
    let model_dat = read_entry(&mut zip, "model.dat");

    // materials/<folder>/material.xml, with the texture image beside it.
    let mut materials = Vec::new();
    let mut textures: Vec<SkpTexture> = Vec::new();
    let mut xmls: Vec<&String> = names.iter().filter(|n| n.starts_with("materials/") && n.ends_with("/material.xml")).collect();
    xmls.sort();
    for xml_name in xmls {
        let Some(raw) = read_entry(&mut zip, xml_name) else { continue };
        let Ok(text) = String::from_utf8(raw) else { continue };
        let Some(mut m) = material::parse(&text) else { continue };
        if let Some(file) = m.texture_file.clone() {
            let dir = xml_name.trim_end_matches("material.xml");
            // The recorded file name and the stored entry can differ in case (".JPG").
            let entry = names.iter().find(|n| n.starts_with(dir) && n.len() > dir.len() && n[dir.len()..].eq_ignore_ascii_case(&file));
            match entry.cloned().and_then(|e| read_entry(&mut zip, &e)) {
                Some(data) => {
                    m.texture = Some(textures.len());
                    textures.push(SkpTexture { name: file, width: 0, height: 0, size_inches: m.texture_scale, file: String::new(), data });
                }
                // The image is missing, so the material falls back to its colour.
                None => m.texture_file = None,
            }
        }
        materials.push(m);
    }

    Ok(NativeSkp { version, materials, textures, thumbnail, model_dat })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file with no zip in it is reported as the legacy container, not mis-parsed.
    #[test]
    fn legacy_files_are_recognised() {
        let mut bytes = vec![0xff, 0xfe, 0xff, 0x0e];
        for c in "SketchUp Model".encode_utf16() {
            bytes.extend(c.to_le_bytes());
        }
        bytes.extend([0xff, 0xfe, 0xff, 0x0a]);
        for c in "{17.3.116}".encode_utf16() {
            bytes.extend(c.to_le_bytes());
        }
        bytes.extend(b"\xff\xff\x00\x00\x0bCVersionMap");
        assert_eq!(format(&bytes), Format::Legacy);
        let f = read_bytes(&bytes).unwrap();
        assert_eq!(f.version, "17.3.116");
        assert_eq!(f.version_name(), "SketchUp 2017");
        assert!(f.materials.is_empty() && f.model_dat.is_none());
    }

    /// A zip behind a header is found and read, including a material and its image.
    #[test]
    fn reads_materials_and_textures_from_a_zip() {
        let xml = r#"<?xml version="1.0"?>
<materialDocument><mat:material name="[Brick]" type="2" colorRed="10" colorGreen="20" colorBlue="30" trans="1" useTrans="0" hasTexture="1">
<mat:texture textureFilename="brick.jpg" xScale="24" yScale="36" avgColor="0"/></mat:material></materialDocument>"#;
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            use std::io::Write;
            for (name, body) in [("materials/[Brick]/material.xml", xml.as_bytes()), ("materials/[Brick]/brick.jpg", b"JPEGDATA".as_slice()), ("model.dat", b"\x64\x00\x02\x00\x00\x00hi".as_slice()), ("meta/model_thumbnail.png", b"PNGDATA".as_slice())] {
                w.start_file(name, opts).unwrap();
                w.write_all(body).unwrap();
            }
            w.finish().unwrap();
        }
        // Put the usual header in front, as real files do.
        let mut file = vec![0xff, 0xfe, 0xff, 0x0e];
        for c in "SketchUp Model".encode_utf16() {
            file.extend(c.to_le_bytes());
        }
        file.extend([0xff, 0xfe, 0xff, 0x08]);
        for c in "{26.2.0}".encode_utf16() {
            file.extend(c.to_le_bytes());
        }
        file.extend([0x56, 0x46, 0x46, 0x08]);
        file.extend(&buf);

        assert_eq!(format(&file), Format::Zip);
        let f = read_bytes(&file).unwrap();
        assert_eq!(f.version, "26.2.0");
        assert_eq!(f.version_name(), "SketchUp 2026");
        assert_eq!(f.thumbnail.as_deref(), Some(b"PNGDATA".as_slice()));
        assert_eq!(f.materials.len(), 1);
        let m = &f.materials[0];
        assert_eq!(m.name, "[Brick]");
        assert_eq!(m.color, [10, 20, 30, 255]);
        assert_eq!(m.texture, Some(0));
        assert_eq!(f.textures[0].data, b"JPEGDATA");
        assert_eq!(f.textures[0].size_inches, [24.0, 36.0], "xScale/yScale are the tile size in inches");
        // The geometry stream is handed back whole for inspection.
        let dat = f.model_dat.unwrap();
        assert_eq!(tlv::root(&dat).unwrap().text(), Some("hi"));
    }
}
