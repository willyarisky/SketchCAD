//! Opening and saving SketchUp files (`.skp`, and `.skb` backups) through `sc_skp`.

use crate::document::{Definition, Document, ModelData, Tag};
use sc_geom::{Color, DefId, Instance, Mesh, MeshBuilder};
use sc_math::units::LengthUnit;
use sc_math::{DMat4, DVec3};
use sc_skp::{SkpCamera, SkpDefinition, SkpEdge, SkpEntities, SkpFace, SkpInstance, SkpLayer, SkpMaterial, SkpModel};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Is `path` a SketchUp file?
pub fn is_skp(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("skp") || e.eq_ignore_ascii_case("skb"))
}

fn v3(p: [f64; 3]) -> DVec3 {
    DVec3::from_array(p)
}

/// SketchUp model → SketchCAD model.
pub fn import(src: &SkpModel) -> ModelData {
    let mut model = ModelData::default();
    let color = |i: Option<usize>| -> Option<Color> { i.and_then(|i| src.materials.get(i)).map(|m| m.color) };

    // Definitions first, so instances can refer to any of them.
    let ids: Vec<DefId> = src
        .definitions
        .iter()
        .map(|d| model.definitions.insert(Definition { name: d.name.clone(), description: d.description.clone(), is_group: d.group, mesh: Mesh::new() }))
        .collect();
    // Instances painted with a material show it on the faces inside that have none: bake that into
    // a painted copy of the definition (shared by every instance painted the same).
    let mut painted: HashMap<(usize, usize), DefId> = HashMap::new();

    fn build(ents: &SkpEntities, color: &dyn Fn(Option<usize>) -> Option<Color>, inst_def: &mut dyn FnMut(&SkpInstance) -> Option<DefId>, tags: &[u32]) -> Mesh {
        let mut b = MeshBuilder::new();
        for e in &ents.edges {
            b.edge(v3(e.a), v3(e.b), e.soft, e.smooth, e.hidden, e.curve);
        }
        for f in &ents.faces {
            let outer: Vec<DVec3> = f.outer.iter().map(|p| v3(*p)).collect();
            let holes: Vec<Vec<DVec3>> = f.holes.iter().map(|h| h.iter().map(|p| v3(*p)).collect()).collect();
            b.face(&outer, &holes, v3(f.normal), color(f.front), color(f.back));
        }
        let mut mesh = b.finish();
        for i in &ents.instances {
            let Some(def) = inst_def(i) else { continue };
            let mut inst = Instance::new(def, DMat4::from_cols_array(&i.transform));
            inst.name = i.name.clone();
            inst.hidden = i.hidden;
            inst.locked = i.locked;
            inst.tag = i.layer.and_then(|l| tags.get(l).copied());
            mesh.instances.insert(inst);
        }
        mesh
    }

    model.tags = src.layers.iter().map(|l| Tag { name: l.name.clone(), visible: l.visible, color: [160, 160, 160, 255] }).collect();
    let tags: Vec<u32> = (0..src.layers.len() as u32).collect();

    let mut meshes: Vec<Mesh> = Vec::with_capacity(src.definitions.len());
    let mut pending_paint: Vec<(usize, usize, DefId)> = Vec::new();
    let root;
    {
        let mut inst_def = |i: &SkpInstance| -> Option<DefId> {
            let base = *ids.get(i.definition)?;
            let Some(m) = i.material else { return Some(base) };
            let has_default = src.definitions[i.definition].entities.faces.iter().any(|f| f.front.is_none() || f.back.is_none());
            if !has_default {
                return Some(base);
            }
            Some(*painted.entry((i.definition, m)).or_insert_with(|| {
                let d = &src.definitions[i.definition];
                let id = model.definitions.insert(Definition { name: d.name.clone(), description: d.description.clone(), is_group: d.group, mesh: Mesh::new() });
                pending_paint.push((i.definition, m, id));
                id
            }))
        };
        for d in &src.definitions {
            meshes.push(build(&d.entities, &color, &mut inst_def, &tags));
        }
        root = build(&src.entities, &color, &mut inst_def, &tags);
    }
    model.mesh = root;
    for (src_idx, mat, id) in pending_paint {
        let mut mesh = meshes[src_idx].clone();
        let c = color(Some(mat));
        for f in mesh.faces.values_mut() {
            f.front = f.front.or(c);
            f.back = f.back.or(c);
        }
        model.definitions[id].mesh = mesh;
    }
    for (id, mesh) in ids.iter().zip(meshes) {
        model.definitions[*id].mesh = mesh;
    }
    // Group definitions painted per instance replaced the originals; drop originals nobody uses.
    let unused: Vec<DefId> = ids.iter().copied().filter(|d| model.definitions[*d].is_group && model.instance_count(*d) == 0).collect();
    for d in unused {
        model.definitions.remove(d);
    }

    model.units.unit = match src.units.as_deref() {
        Some("Feet") => LengthUnit::Feet,
        Some("Millimeters") => LengthUnit::Millimeters,
        Some("Centimeters") => LengthUnit::Centimeters,
        Some("Meters") => LengthUnit::Meters,
        Some("Inches") => LengthUnit::Inches,
        _ => LengthUnit::Architectural,
    };
    if let Some(c) = &src.camera {
        let cam = &mut model.camera;
        cam.eye = v3(c.eye);
        cam.target = v3(c.target);
        cam.perspective = c.perspective;
        if c.fov > 1.0 && c.fov < 120.0 {
            cam.fov_y = c.fov;
        }
        if cam.eye.distance(cam.target) < 1e-6 {
            cam.target = cam.eye + DVec3::Y;
        }
    }
    model
}

/// SketchCAD model → SketchUp model.
pub fn export(model: &ModelData) -> SkpModel {
    let mut materials: Vec<SkpMaterial> = Vec::new();
    let mut mat_index: HashMap<Color, usize> = HashMap::new();
    let mut mat = |c: Option<Color>| -> Option<usize> {
        let c = c?;
        Some(*mat_index.entry(c).or_insert_with(|| {
            let name = crate::ui::tray::MATERIALS.iter().find(|(_, mc)| *mc == c).map(|(n, _)| n.to_string()).unwrap_or_else(|| format!("Color_{:02X}{:02X}{:02X}", c[0], c[1], c[2]));
            materials.push(SkpMaterial { name, color: c });
            materials.len() - 1
        }))
    };
    let def_index: HashMap<DefId, usize> = model.definitions.keys().enumerate().map(|(i, d)| (d, i)).collect();

    let mut entities = |mesh: &Mesh| -> SkpEntities {
        let p = |v| mesh.pos(v).to_array();
        let faces = mesh
            .faces
            .values()
            .map(|f| SkpFace {
                outer: f.outer.iter().map(|v| p(*v)).collect(),
                holes: f.holes.iter().map(|h| h.iter().map(|v| p(*v)).collect()).collect(),
                normal: f.plane.normal.to_array(),
                front: mat(f.front),
                back: mat(f.back),
            })
            .collect();
        let edges = mesh
            .edges
            .values()
            .map(|e| SkpEdge { a: p(e.v[0]), b: p(e.v[1]), soft: e.soft, smooth: e.smooth, hidden: e.hidden, curve: e.curve.map(u64::from) })
            .collect();
        let instances = mesh
            .instances
            .values()
            .filter_map(|i| {
                Some(SkpInstance {
                    definition: *def_index.get(&i.def)?,
                    transform: i.xform.to_cols_array(),
                    name: i.name.clone(),
                    hidden: i.hidden,
                    locked: i.locked,
                    layer: i.tag.map(|t| t as usize).filter(|t| *t < model.tags.len()),
                    material: None,
                })
            })
            .collect();
        SkpEntities { faces, edges, instances }
    };

    let definitions = model.definitions.values().map(|d| SkpDefinition { name: d.name.clone(), description: d.description.clone(), group: d.is_group, entities: entities(&d.mesh) }).collect();
    let root = entities(&model.mesh);
    let c = &model.camera;
    SkpModel {
        version: String::new(),
        units: Some(format!("{:?}", model.units.unit)),
        camera: Some(SkpCamera { eye: c.eye.to_array(), target: c.target.to_array(), up: [0.0, 0.0, 1.0], perspective: c.perspective, fov: c.fov_y }),
        materials,
        layers: model.tags.iter().map(|t| SkpLayer { name: t.name.clone(), visible: t.visible }).collect(),
        definitions,
        entities: root,
        warnings: Vec::new(),
    }
}

fn sdk() -> Result<sc_skp::locate::Sdk, String> {
    sc_skp::locate::find_sdk().ok_or_else(|| {
        "SketchUp files need Trimble's SketchUp C API library, which was not found.\n\n\
         Install SketchUp, or download the free SketchUp SDK from Trimble's developer site and unpack it \
         into ~/SketchUpSDK (or set SKETCHCAD_SKP_SDK to its SketchUpAPI.framework)."
            .to_string()
    })
}

/// A document read from a SketchUp file, with notes about what could not be brought across.
pub type OpenResult = Result<(Document, Vec<String>), String>;

/// Read a `.skp`/`.skb` into a document (runs the converter; may take a moment).
pub fn open(path: &Path) -> OpenResult {
    let sdk = sdk()?;
    let src = sc_skp::locate::read(&sdk, path)?;
    let model = import(&src);
    let doc = Document { model, path: Some(path.to_path_buf()), ..Default::default() };
    Ok((doc, src.warnings))
}

/// Write the document as a `.skp`/`.skb`. Saving over an existing `.skp` first keeps the old file
/// as a `.skb` backup next to it, as SketchUp does. Returns the backup path, if one was made.
pub fn save(model: &ModelData, path: &Path) -> Result<Option<PathBuf>, String> {
    let sdk = sdk()?;
    let mut backup = None;
    if path.is_file() && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("skp")) {
        let skb = path.with_extension("skb");
        std::fs::copy(path, &skb).map_err(|e| format!("Could not make the backup {}: {e}", skb.display()))?;
        backup = Some(skb);
    }
    // Write next to the target, then move into place, so a failure never leaves half a file.
    let tmp = path.with_extension("skp~tmp.skp");
    let r = sc_skp::locate::write(&sdk, &export(model), &tmp).and_then(|_| std::fs::rename(&tmp, path).map_err(|e| e.to_string()));
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r.map(|_| backup)
}

impl crate::app::SketchCadApp {
    /// Finish a background `.skp` open, showing a progress dialog while it runs.
    pub fn poll_skp_open(&mut self, ctx: &egui::Context) {
        let Some((path, rx)) = &self.skp_open else { return };
        let path = path.clone();
        match rx.try_recv() {
            Ok(result) => {
                self.skp_open = None;
                match result {
                    Ok((doc, warnings)) => {
                        self.with_tool_cx(|t, cx| t.cancel(cx));
                        self.doc = doc;
                        self.inference.reset();
                        self.pending_zoom_extents = self.doc.model.camera.eye.distance(self.doc.model.camera.target) < 1e-3;
                        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        if warnings.is_empty() {
                            self.flash(format!("Opened {name}"), false);
                        } else {
                            let list: Vec<String> = warnings.iter().map(|w| format!("• {w}")).collect();
                            self.error_dialog = Some(format!("Opened {name}. Some things in it are not supported by SketchCAD yet:\n\n{}", list.join("\n")));
                        }
                    }
                    Err(e) => self.error_dialog = Some(format!("Could not open {}:\n{e}", path.display())),
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
                egui::Modal::new(egui::Id::new("skp_open")).show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("Opening {}...", path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()));
                    });
                });
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.skp_open = None;
                self.error_dialog = Some(format!("Could not open {}: the converter stopped unexpectedly", path.display()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn available() -> bool {
        match sc_skp::locate::find_sdk().map(|s| sc_skp::locate::find_converter(&s)) {
            Some(Ok(_)) => true,
            Some(Err(e)) => {
                eprintln!("skipping: {e}");
                false
            }
            None => {
                eprintln!("skipping: no SketchUp library on this machine");
                false
            }
        }
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("skcad-skp-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.join(name)
    }

    fn total_faces(m: &ModelData) -> usize {
        m.mesh.faces.len() + m.definitions.values().map(|d| d.mesh.faces.len()).sum::<usize>()
    }

    /// The demo house (a group and three shared components, colored faces) survives a round trip
    /// through a real .skp file: geometry, orientation, components, names and colors.
    #[test]
    fn round_trip_through_skp() {
        if !available() {
            return;
        }
        let mut model = ModelData::default();
        let mut house = crate::demo::house();
        let red = [178, 34, 34, 255];
        let roof = house.faces.iter().find(|(_, f)| f.plane.normal.z > 0.3).map(|(id, _)| id).unwrap();
        house.faces[roof].front = Some(red);
        let h = model.definitions.insert(Definition { name: "House".into(), description: String::new(), is_group: true, mesh: house });
        let b = model.definitions.insert(Definition { name: "Bush".into(), description: "Shrub".into(), is_group: false, mesh: crate::demo::bush() });
        model.mesh.instances.insert(Instance::new(h, DMat4::IDENTITY));
        for x in [-50.0, 260.0] {
            let mut i = Instance::new(b, DMat4::from_translation(DVec3::new(x, -60.0, 0.0)));
            i.name = format!("bush at {x}");
            model.mesh.instances.insert(i);
        }
        model.tags.push(Tag { name: "Plants".into(), visible: false, color: [0; 4] });
        model.mesh.instances.values_mut().find(|i| i.def == b).unwrap().tag = Some(0);
        model.units.unit = LengthUnit::Millimeters;

        let path = temp("house.skp");
        save(&model, &path).unwrap();
        assert!(sc_skp::file_version(&path).is_some(), "a real SketchUp file header");
        let (doc, warnings) = open(&path).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let back = &doc.model;

        assert_eq!(total_faces(back), total_faces(&model));
        assert_eq!(back.mesh.instances.len(), 3);
        let bush = back.definitions.values().find(|d| d.name == "Bush").expect("component kept");
        assert!(!bush.is_group && bush.description == "Shrub");
        assert_eq!(back.instance_count(back.definitions.iter().find(|(_, d)| d.name == "Bush").unwrap().0), 2, "both bushes share one definition");
        assert!(back.mesh.instances.values().any(|i| i.name == "bush at 260"));
        let house_def = back.definitions.values().find(|d| d.is_group).expect("group kept");
        assert_eq!(house_def.mesh.faces.len(), model.definitions[h].mesh.faces.len());
        house_def.mesh.validate().unwrap();
        // Same bounds, and every face still faces outward the same way.
        let (b0, b1) = (model.definitions[h].mesh.bounds(), house_def.mesh.bounds());
        assert!(b0.min.distance(b1.min) < 1e-6 && b0.max.distance(b1.max) < 1e-6);
        let normals = |m: &Mesh| {
            let mut v: Vec<[i64; 3]> = m.faces.values().map(|f| (f.plane.normal * 1000.0).round().to_array().map(|x| x as i64)).collect();
            v.sort();
            v
        };
        assert_eq!(normals(&house_def.mesh), normals(&model.definitions[h].mesh));
        assert!(house_def.mesh.faces.values().any(|f| f.front == Some(red)), "painted roof");
        assert!(bush.mesh.faces.values().all(|f| f.front == Some([96, 150, 60, 255])));
        assert_eq!(back.tags.len(), 1);
        assert!(!back.tags[0].visible);
        assert_eq!(back.units.unit, LengthUnit::Millimeters);

        // Saving over it keeps the previous file as a .skb backup, which opens too.
        let backup = save(&back.clone(), &path).unwrap().expect("backup made");
        assert_eq!(backup, path.with_extension("skb"));
        let (from_skb, _) = open(&backup).unwrap();
        assert_eq!(from_skb.model.mesh.instances.len(), 3);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    /// One of SketchUp's own sample models opens with its components and materials.
    #[test]
    fn opens_sketchup_sample_model() {
        let sample = Path::new("/Applications/SketchUp 2017/SketchUp.app/Contents/Resources/Content/Components/Components Sampler/Car Sedan.skp");
        if !sample.is_file() || !available() {
            return;
        }
        let (doc, _) = open(sample).unwrap();
        let m = &doc.model;
        assert!(m.definitions.len() >= 5, "nested components: {}", m.definitions.len());
        assert!(total_faces(m) > 100);
        for d in m.definitions.values() {
            d.mesh.validate().unwrap();
        }
        assert!(m.definitions.values().any(|d| d.mesh.faces.values().any(|f| f.front.is_some())), "materials");
        assert!(!m.bounds().is_empty());
    }
}
