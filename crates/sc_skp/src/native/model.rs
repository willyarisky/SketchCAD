//! Decoding `model.dat` — the geometry of a modern (zip-container) `.skp`.
//!
//! The framing is the [`tlv`](super::tlv) tree; this module is the vocabulary. SketchUp publishes
//! no schema, so the key meanings below were recovered by inspecting real files and then checked
//! against the one invariant the format hands you for free: every face stores its own plane, and
//! that plane must agree with the plane through the face's own loop. Both test models decode with
//! every face agreeing, which is what makes this trustworthy rather than plausible.
//!
//! ```text
//! 0x01f4  root
//!   0x01f6 → 0x1388                  the model's own entities
//!   0x01f9 → 0x1770 → 0x1771 → 0x157c*   component/group definitions
//!                       0x1388      the definition's entities
//!                       0x157e      its name
//!
//! 0x1388  an entity collection
//!   0x07d0 → 0x05dc → 0x05de         the collection's own id (what instances point at)
//!   0x1389 → 0x09c4*  vertex         0x05dc→0x05de id, 0x09c5 position (3×f64)
//!   0x138a → 0x0bb8*  edge           id, 0x0bb9 start vertex id, 0x0bba end vertex id
//!   0x138b → 0x0dac*  face           id, 0x0dad plane (normal + offset, 4×f64),
//!                     0x0dae → 0x1194* loop → 0x1195 → 0x0fa0*
//!                                     { 0x0fa1 edge id, 0x0fa2 direction }
//!   0x138d → 0x1d4c* → 0x1964  instance   0x1966 transform (13×f64), 0x1967 definition id
//! ```
//!
//! Entities refer to each other by id, not by position, so every lookup goes through a map and a
//! dangling reference drops the entity rather than corrupting the mesh.

use super::tlv::{self, Record};
use crate::{SkpDefinition, SkpEdge, SkpEntities, SkpFace, SkpInstance, SkpModel};
use std::collections::HashMap;

// Sections.
const ROOT_ENTITIES: u16 = 0x01f6;
const DEFINITIONS: u16 = 0x01f9;
const DEFS_OUTER: u16 = 0x1770;
const DEFS_INNER: u16 = 0x1771;
const DEFINITION: u16 = 0x157c;
const DEF_NAME: u16 = 0x157e;
const ENTITIES: u16 = 0x1388;

// Within an entity collection.
const ID_OUTER: u16 = 0x07d0;
const ID_MID: u16 = 0x05dc;
const ID: u16 = 0x05de;
const VERTICES: u16 = 0x1389;
const VERTEX: u16 = 0x09c4;
const VERTEX_POS: u16 = 0x09c5;
const EDGES: u16 = 0x138a;
const EDGE: u16 = 0x0bb8;
const EDGE_START: u16 = 0x0bb9;
const EDGE_END: u16 = 0x0bba;
const FACES: u16 = 0x138b;
const FACE: u16 = 0x0dac;
const FACE_PLANE: u16 = 0x0dad;
const FACE_LOOPS: u16 = 0x0dae;
const LOOP: u16 = 0x1194;
const LOOP_INNER: u16 = 0x1195;
const LOOP_EDGE: u16 = 0x0fa0;
const LOOP_EDGE_ID: u16 = 0x0fa1;
const LOOP_EDGE_DIR: u16 = 0x0fa2;
const INSTANCES: u16 = 0x138d;
const INSTANCE: u16 = 0x1d4c;
const INSTANCE_BODY: u16 = 0x1964;
const INSTANCE_XFORM: u16 = 0x1966;
const INSTANCE_DEF: u16 = 0x1967;

/// Entity ids are 16-bit and unique within a file.
type Id = u16;

fn child<'a>(rec: &Record<'a>, key: u16) -> Option<Record<'a>> {
    rec.children()?.into_iter().find(|c| c.key == key)
}

fn children<'a>(rec: &Record<'a>, key: u16) -> Vec<Record<'a>> {
    rec.children().unwrap_or_default().into_iter().filter(|c| c.key == key).collect()
}

/// Follow a chain of single children.
fn path<'a>(rec: &Record<'a>, keys: &[u16]) -> Option<Record<'a>> {
    keys.iter().try_fold(*rec, |cur, k| child(&cur, *k))
}

fn f64s(rec: &Record, n: usize) -> Option<Vec<f64>> {
    if rec.data.len() != n * 8 {
        return None;
    }
    Some(rec.data.as_chunks::<8>().0.iter().map(|c| f64::from_le_bytes(*c)).collect())
}

/// An entity's own id. Vertices carry it directly; edges, faces and collections wrap it once more.
fn entity_id(rec: &Record) -> Option<Id> {
    path(rec, &[ID_MID, ID]).or_else(|| path(rec, &[ID_OUTER, ID_MID, ID]))?.u16()
}

struct Collection {
    entities: SkpEntities,
    /// Instances here name their definition by that definition's collection id.
    pending: Vec<(Id, [f64; 16])>,
}

/// Decode one `0x1388` entity collection.
fn collection(ents: &Record) -> Collection {
    let mut verts = HashMap::new();
    if let Some(sec) = child(ents, VERTICES) {
        for v in children(&sec, VERTEX) {
            let (Some(id), Some(pos)) = (entity_id(&v), child(&v, VERTEX_POS).and_then(|p| f64s(&p, 3))) else { continue };
            verts.insert(id, [pos[0], pos[1], pos[2]]);
        }
    }

    let mut edges: HashMap<Id, (Id, Id)> = HashMap::new();
    if let Some(sec) = child(ents, EDGES) {
        for e in children(&sec, EDGE) {
            let (Some(id), Some(a), Some(b)) = (entity_id(&e), child(&e, EDGE_START).and_then(|c| c.u16()), child(&e, EDGE_END).and_then(|c| c.u16())) else { continue };
            edges.insert(id, (a, b));
        }
    }

    let mut out = SkpEntities::default();
    for (a, b) in edges.values() {
        if let (Some(pa), Some(pb)) = (verts.get(a), verts.get(b)) {
            out.edges.push(SkpEdge { a: *pa, b: *pb, soft: false, smooth: false, hidden: false, curve: None });
        }
    }

    if let Some(sec) = child(ents, FACES) {
        for f in children(&sec, FACE) {
            let Some(plane) = child(&f, FACE_PLANE).and_then(|p| f64s(&p, 4)) else { continue };
            let mut rings: Vec<Vec<[f64; 3]>> = Vec::new();
            for holder in children(&f, FACE_LOOPS) {
                for lp in children(&holder, LOOP) {
                    let Some(inner) = child(&lp, LOOP_INNER) else { continue };
                    let mut ring = Vec::new();
                    let mut whole = true;
                    for step in children(&inner, LOOP_EDGE) {
                        // Each step names an edge and which end the loop arrives at, so walking
                        // them in order gives the ring without having to sort anything.
                        let Some(eid) = child(&step, LOOP_EDGE_ID).and_then(|c| c.u16()) else { continue };
                        let forward = child(&step, LOOP_EDGE_DIR).is_some_and(|c| c.data.first() != Some(&0));
                        let Some((a, b)) = edges.get(&eid) else {
                            whole = false;
                            break;
                        };
                        match verts.get(if forward { b } else { a }) {
                            Some(p) => ring.push(*p),
                            None => {
                                whole = false;
                                break;
                            }
                        }
                    }
                    if whole && ring.len() >= 3 {
                        rings.push(ring);
                    }
                }
            }
            if rings.is_empty() {
                continue;
            }
            // The first loop is the outline; any others are holes.
            let outer = rings.remove(0);
            out.faces.push(SkpFace { outer, holes: rings, normal: [plane[0], plane[1], plane[2]], front: None, back: None, front_uv: None, back_uv: None });
        }
    }

    let mut pending = Vec::new();
    if let Some(sec) = child(ents, INSTANCES) {
        for i in children(&sec, INSTANCE) {
            let Some(body) = child(&i, INSTANCE_BODY) else { continue };
            let (Some(def), Some(m)) = (child(&body, INSTANCE_DEF).and_then(|c| c.u16()), child(&body, INSTANCE_XFORM).and_then(|c| f64s(&c, 13))) else { continue };
            // 13 doubles: three basis vectors, the origin, then a scale divisor.
            #[rustfmt::skip]
            let xform = [
                m[0], m[1], m[2], 0.0,
                m[3], m[4], m[5], 0.0,
                m[6], m[7], m[8], 0.0,
                m[9], m[10], m[11], if m[12].abs() > 1e-12 { m[12] } else { 1.0 },
            ];
            pending.push((def, xform));
        }
    }

    Collection { entities: out, pending }
}

/// Decode `model.dat` into the same description the SketchUp library produces, so everything
/// downstream — import, rendering, saving — is shared.
pub fn read(model_dat: &[u8], version: String) -> Result<SkpModel, String> {
    let root = tlv::root(model_dat).ok_or("model.dat is not a single well-formed record")?;

    // Definitions first; the model's own entities refer to them.
    let mut definitions: Vec<SkpDefinition> = Vec::new();
    let mut def_index: HashMap<Id, usize> = HashMap::new();
    let mut def_pending: Vec<Vec<(Id, [f64; 16])>> = Vec::new();
    if let Some(list) = path(&root, &[DEFINITIONS, DEFS_OUTER, DEFS_INNER]) {
        for d in children(&list, DEFINITION) {
            let Some(ents) = child(&d, ENTITIES) else { continue };
            let name = child(&d, DEF_NAME).and_then(|n| n.text()).unwrap_or("").to_string();
            let c = collection(&ents);
            // Instances name a definition by its collection id, so that is the key to index by.
            if let Some(id) = path(&ents, &[ID_OUTER, ID_MID, ID]).and_then(|r| r.u16()) {
                def_index.insert(id, definitions.len());
            }
            // SketchUp names groups "Group#n"; components get a real name.
            let group = name.starts_with("Group");
            definitions.push(SkpDefinition { name, description: String::new(), group, entities: c.entities });
            def_pending.push(c.pending);
        }
    }

    let root_ents = path(&root, &[ROOT_ENTITIES, ENTITIES]).ok_or("no model entities in model.dat")?;
    let top = collection(&root_ents);

    // Resolve instance → definition now that every definition has an index.
    let resolve = |pending: Vec<(Id, [f64; 16])>| -> Vec<SkpInstance> {
        pending
            .into_iter()
            .filter_map(|(def, transform)| {
                Some(SkpInstance { definition: *def_index.get(&def)?, transform, name: String::new(), hidden: false, locked: false, layer: None, material: None })
            })
            .collect()
    };
    let mut entities = top.entities;
    entities.instances = resolve(top.pending);
    for (d, pending) in definitions.iter_mut().zip(def_pending) {
        d.entities.instances = resolve(pending);
    }

    let mut warnings = Vec::new();
    // Said plainly rather than left to be noticed: this reader does not do materials yet.
    if !definitions.is_empty() || !entities.faces.is_empty() {
        warnings.push("materials, tags and scenes are not read without the SketchUp library".into());
    }
    Ok(SkpModel { version, units: None, camera: None, materials: Vec::new(), textures: Vec::new(), layers: Vec::new(), definitions, entities, warnings })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(key: u16, payload: &[u8]) -> Vec<u8> {
        let mut v = key.to_le_bytes().to_vec();
        v.extend((payload.len() as u32).to_le_bytes());
        v.extend(payload);
        v
    }
    fn id_rec(outer: bool, id: Id) -> Vec<u8> {
        let inner = rec(ID_MID, &rec(ID, &id.to_le_bytes()));
        if outer { rec(ID_OUTER, &inner) } else { inner }
    }
    fn pt(id: Id, p: [f64; 3]) -> Vec<u8> {
        let pos: Vec<u8> = p.iter().flat_map(|v| v.to_le_bytes()).collect();
        rec(VERTEX, &[id_rec(false, id), rec(VERTEX_POS, &pos)].concat())
    }
    fn edge(id: Id, a: Id, b: Id) -> Vec<u8> {
        rec(EDGE, &[id_rec(true, id), rec(EDGE_START, &a.to_le_bytes()), rec(EDGE_END, &b.to_le_bytes())].concat())
    }
    fn loop_step(eid: Id, forward: bool) -> Vec<u8> {
        rec(LOOP_EDGE, &[rec(LOOP_EDGE_ID, &eid.to_le_bytes()), rec(LOOP_EDGE_DIR, &[forward as u8])].concat())
    }

    /// A square built the way the format does it — loops naming edges, edges naming vertices —
    /// comes back as one face with its four corners in order.
    fn square_model() -> Vec<u8> {
        let verts = [pt(1, [0.0, 0.0, 0.0]), pt(2, [10.0, 0.0, 0.0]), pt(3, [10.0, 10.0, 0.0]), pt(4, [0.0, 10.0, 0.0])].concat();
        let edges = [edge(11, 1, 2), edge(12, 2, 3), edge(13, 3, 4), edge(14, 4, 1)].concat();
        let steps = [loop_step(11, true), loop_step(12, true), loop_step(13, true), loop_step(14, true)].concat();
        let lp = rec(LOOP, &rec(LOOP_INNER, &steps));
        let plane: Vec<u8> = [0.0f64, 0.0, 1.0, 0.0].iter().flat_map(|v| v.to_le_bytes()).collect();
        let face = rec(FACE, &[id_rec(true, 21), rec(FACE_PLANE, &plane), rec(FACE_LOOPS, &lp)].concat());
        let ents = rec(
            ENTITIES,
            &[rec(ID_OUTER, &rec(ID_MID, &rec(ID, &100u16.to_le_bytes()))), rec(VERTICES, &verts), rec(EDGES, &edges), rec(FACES, &face)].concat(),
        );
        rec(0x01f4, &rec(ROOT_ENTITIES, &ents))
    }

    #[test]
    fn decodes_a_square() {
        let m = read(&square_model(), "26.2.0".into()).unwrap();
        assert_eq!(m.entities.edges.len(), 4);
        assert_eq!(m.entities.faces.len(), 1);
        let f = &m.entities.faces[0];
        assert_eq!(f.normal, [0.0, 0.0, 1.0]);
        // Walking the loop forward lands on each edge's far end, so the ring starts at vertex 2.
        assert_eq!(f.outer, vec![[10.0, 0.0, 0.0], [10.0, 10.0, 0.0], [0.0, 10.0, 0.0], [0.0, 0.0, 0.0]]);
        assert!(f.holes.is_empty());
    }

    /// A loop naming an edge that is not in the file drops that face rather than producing a
    /// ring with a hole in it.
    #[test]
    fn a_dangling_edge_reference_drops_the_face() {
        let broken = String::from_utf8_lossy(&square_model()).len(); // keep the helper honest
        assert!(broken > 0);
        let verts = [pt(1, [0.0, 0.0, 0.0]), pt(2, [10.0, 0.0, 0.0]), pt(3, [10.0, 10.0, 0.0])].concat();
        let edges = [edge(11, 1, 2), edge(12, 2, 3)].concat();
        let steps = [loop_step(11, true), loop_step(12, true), loop_step(99, true)].concat();
        let lp = rec(LOOP, &rec(LOOP_INNER, &steps));
        let plane: Vec<u8> = [0.0f64, 0.0, 1.0, 0.0].iter().flat_map(|v| v.to_le_bytes()).collect();
        let face = rec(FACE, &[id_rec(true, 21), rec(FACE_PLANE, &plane), rec(FACE_LOOPS, &lp)].concat());
        let ents = rec(ENTITIES, &[rec(VERTICES, &verts), rec(EDGES, &edges), rec(FACES, &face)].concat());
        let m = read(&rec(0x01f4, &rec(ROOT_ENTITIES, &ents)), String::new()).unwrap();
        assert_eq!(m.entities.faces.len(), 0, "the face is dropped");
        assert_eq!(m.entities.edges.len(), 2, "its edges still come through");
    }

    #[test]
    fn rejects_rubbish() {
        assert!(read(b"not a skp at all", String::new()).is_err());
    }
}
