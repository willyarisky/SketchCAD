//! SketchUp `.skp` / `.skb` support.
//!
//! `.skp` is SketchUp's proprietary format; the only reliable reader/writer is Trimble's SketchUp C
//! API library (`SketchUpAPI`), from the SketchUp SDK or a SketchUp installation. SketchCAD never
//! links it: the `sketchcad-skp` converter loads it at run time and translates between `.skp` and
//! [`SkpModel`], a plain description of the geometry. `.skb` is SketchUp's backup copy of a `.skp`
//! — the same format under another extension.

pub mod locate;

use serde::{Deserialize, Serialize};

/// Everything SketchCAD exchanges with a `.skp` file. Lengths are inches, as in SketchUp.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SkpModel {
    /// SketchUp version that saved the file, e.g. "17.3.116" (reading only).
    #[serde(default)]
    pub version: String,
    /// "Inches", "Feet", "Millimeters", "Centimeters" or "Meters".
    #[serde(default)]
    pub units: Option<String>,
    #[serde(default)]
    pub camera: Option<SkpCamera>,
    #[serde(default)]
    pub materials: Vec<SkpMaterial>,
    /// Images used by textured materials.
    #[serde(default)]
    pub textures: Vec<SkpTexture>,
    /// Layers/tags other than "Layer0" (untagged).
    #[serde(default)]
    pub layers: Vec<SkpLayer>,
    #[serde(default)]
    pub definitions: Vec<SkpDefinition>,
    pub entities: SkpEntities,
    /// Things in the file SketchCAD could not bring across (textures, scenes, ...).
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkpCamera {
    pub eye: [f64; 3],
    pub target: [f64; 3],
    pub up: [f64; 3],
    pub perspective: bool,
    /// Vertical field of view, degrees.
    pub fov: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkpMaterial {
    pub name: String,
    /// sRGB + alpha. For a textured material this is its average colour, used wherever the
    /// texture cannot be shown (Monochrome style, thumbnails).
    pub color: [u8; 4],
    /// Index into [`SkpModel::textures`].
    #[serde(default)]
    pub texture: Option<usize>,
}

/// An image used by a textured material.
///
/// The encoded bytes do not travel in the JSON: the converter writes each image to a file beside
/// it (`file`) and [`locate::read`] loads them into `data`, so a megabyte of PNG never has to be
/// escaped into a JSON string.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SkpTexture {
    /// The image's name inside the `.skp`, e.g. "shingles.jpg".
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Model-space size of one tile, in inches (SketchUp's s/t scale).
    pub size_inches: [f64; 2],
    /// File holding the encoded image, beside the JSON. Relative to the JSON's folder.
    #[serde(default)]
    pub file: String,
    /// The encoded image (PNG/JPEG/...), filled in once `file` has been read.
    #[serde(skip)]
    pub data: Vec<u8>,
}

/// How a texture lies on a face: an affine map from model space to texture space, so
/// `u = dot(p, u[0..3]) + u[3]` and likewise for `v`.
///
/// SketchUp stores texture placement per face, and for an undistorted texture the mapping is
/// exactly affine. Keeping it as a projection rather than per-vertex UVs means it survives
/// splitting and push/pull the way SketchUp's own does.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SkpUv {
    pub u: [f64; 4],
    pub v: [f64; 4],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkpLayer {
    pub name: String,
    pub visible: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SkpDefinition {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Groups are definitions with a single owner.
    #[serde(default)]
    pub group: bool,
    pub entities: SkpEntities,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SkpEntities {
    #[serde(default)]
    pub faces: Vec<SkpFace>,
    /// Every edge, including those bounding faces (they carry the soft/smooth/hidden flags).
    #[serde(default)]
    pub edges: Vec<SkpEdge>,
    #[serde(default)]
    pub instances: Vec<SkpInstance>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkpFace {
    pub outer: Vec<[f64; 3]>,
    #[serde(default)]
    pub holes: Vec<Vec<[f64; 3]>>,
    pub normal: [f64; 3],
    /// Indices into [`SkpModel::materials`].
    #[serde(default)]
    pub front: Option<usize>,
    #[serde(default)]
    pub back: Option<usize>,
    /// Where the front/back material's texture sits on this face.
    #[serde(default)]
    pub front_uv: Option<SkpUv>,
    #[serde(default)]
    pub back_uv: Option<SkpUv>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkpEdge {
    pub a: [f64; 3],
    pub b: [f64; 3],
    #[serde(default)]
    pub soft: bool,
    #[serde(default)]
    pub smooth: bool,
    #[serde(default)]
    pub hidden: bool,
    /// Edges sharing a value came from one arc/circle.
    #[serde(default)]
    pub curve: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkpInstance {
    /// Index into [`SkpModel::definitions`].
    pub definition: usize,
    /// Column-major 4x4, like SketchUp's `SUTransformation`.
    pub transform: [f64; 16],
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub locked: bool,
    /// Index into [`SkpModel::layers`]; `None` is Layer0 / Untagged.
    #[serde(default)]
    pub layer: Option<usize>,
    /// Material painted on the instance: faces inside without their own material show it.
    #[serde(default)]
    pub material: Option<usize>,
}

/// The SketchUp version that saved a `.skp`/`.skb`, read from its header (e.g. "17.3.116").
pub fn file_version(path: &std::path::Path) -> Option<String> {
    use std::io::Read;
    let mut head = [0u8; 160];
    let n = std::fs::File::open(path).ok()?.read(&mut head).ok()?;
    // The header is UTF-16LE text: "SketchUp Model" then "{major.minor.build}".
    let text: String = head[..n].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).filter_map(|u| char::from_u32(u as u32)).collect();
    let start = text.find('{')? + 1;
    let end = start + text[start..].find('}')?;
    Some(text[start..end].to_string())
}

/// "17.3.116" -> "SketchUp 2017"; "8.0.1" -> "SketchUp 8".
pub fn version_name(v: &str) -> String {
    match v.split('.').next().and_then(|m| m.parse::<u32>().ok()) {
        Some(m) if m >= 13 => format!("SketchUp 20{m:02}"),
        Some(m) => format!("SketchUp {m}"),
        None => "an unknown SketchUp version".into(),
    }
}
