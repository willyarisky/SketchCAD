//! Model side of the Ruby API: requests from the Ruby host, answered against the open document.
//!
//! Entities cross the process boundary as handle strings:
//! `E:<ctx>:<key>` edge, `F:<ctx>:<key>` face, `V:<ctx>:<key>` vertex, `G:<ctx>:<key>` group,
//! `C:<ctx>:<key>` component instance and `D:<key>` definition. `<ctx>` is `0` for the model root
//! or a definition key. Points and vectors are `[x, y, z]` in inches (SketchUp's internal unit);
//! transformations are 16 numbers, column-major, like `Geom::Transformation#to_a`.

use crate::document::{Definition, Document, Guide, Selection};
use sc_geom::{DefId, EdgeId, FaceId, Instance, InstanceId, Mesh, PushPullMode, VertexId};
use sc_math::geom2d::interior_point;
use sc_math::{DMat4, DVec2, DVec3, Plane};
use serde_json::{Map, Value, json};
use slotmap::{Key, KeyData};
use std::collections::HashSet;

type Ctx = Option<DefId>;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ent {
    Edge(Ctx, EdgeId),
    Face(Ctx, FaceId),
    Vertex(Ctx, VertexId),
    Inst(Ctx, InstanceId),
    Def(DefId),
}

fn key_str(k: impl Key) -> String {
    k.data().as_ffi().to_string()
}

fn key_from<K: Key>(s: &str) -> Result<K, String> {
    s.parse::<u64>().map(|n| KeyData::from_ffi(n).into()).map_err(|_| format!("bad entity key '{s}'"))
}

pub fn ctx_str(c: Ctx) -> String {
    c.map_or_else(|| "0".to_string(), key_str)
}

fn parse_ctx(doc: &Document, s: &str) -> Result<Ctx, String> {
    match s {
        "active" => Ok(doc.active_ctx()),
        "0" => Ok(None),
        _ => {
            let d: DefId = key_from(s)?;
            if doc.model.definitions.contains_key(d) { Ok(Some(d)) } else { Err("reference to deleted entities".into()) }
        }
    }
}

pub fn ent_str(doc: &Document, e: Ent) -> String {
    match e {
        Ent::Edge(c, k) => format!("E:{}:{}", ctx_str(c), key_str(k)),
        Ent::Face(c, k) => format!("F:{}:{}", ctx_str(c), key_str(k)),
        Ent::Vertex(c, k) => format!("V:{}:{}", ctx_str(c), key_str(k)),
        Ent::Inst(c, k) => {
            let group = doc.model.context_mesh(c).instances.get(k).and_then(|i| doc.model.definitions.get(i.def)).is_some_and(|d| d.is_group);
            format!("{}:{}:{}", if group { "G" } else { "C" }, ctx_str(c), key_str(k))
        }
        Ent::Def(d) => format!("D:{}", key_str(d)),
    }
}

/// Parse a handle and check the entity still exists.
pub fn parse_ent(doc: &Document, s: &str) -> Result<Ent, String> {
    let gone = || format!("reference to deleted entity ({s})");
    let mut it = s.split(':');
    let kind = it.next().unwrap_or_default();
    if kind == "D" {
        let d: DefId = key_from(it.next().unwrap_or_default())?;
        return if doc.model.definitions.contains_key(d) { Ok(Ent::Def(d)) } else { Err(gone()) };
    }
    let (Some(c), Some(k)) = (it.next(), it.next()) else { return Err(format!("bad entity handle '{s}'")) };
    let ctx = parse_ctx(doc, c).map_err(|_| gone())?;
    let m = doc.model.context_mesh(ctx);
    let e = match kind {
        "E" => Ent::Edge(ctx, key_from(k)?),
        "F" => Ent::Face(ctx, key_from(k)?),
        "V" => Ent::Vertex(ctx, key_from(k)?),
        "G" | "C" => Ent::Inst(ctx, key_from(k)?),
        _ => return Err(format!("bad entity handle '{s}'")),
    };
    let ok = match e {
        Ent::Edge(_, k) => m.edges.contains_key(k),
        Ent::Face(_, k) => m.faces.contains_key(k),
        Ent::Vertex(_, k) => m.vertices.contains_key(k),
        Ent::Inst(_, k) => m.instances.contains_key(k),
        Ent::Def(_) => true,
    };
    if ok { Ok(e) } else { Err(gone()) }
}

fn ent_ctx(e: Ent) -> Option<Ctx> {
    match e {
        Ent::Edge(c, _) | Ent::Face(c, _) | Ent::Vertex(c, _) | Ent::Inst(c, _) => Some(c),
        Ent::Def(_) => None,
    }
}

// ---------------------------------------------------------------------------
// Argument helpers

fn arg(a: &[Value], i: usize) -> &Value {
    a.get(i).unwrap_or(&Value::Null)
}

fn num(v: &Value) -> Result<f64, String> {
    v.as_f64().ok_or_else(|| format!("expected a number, got {v}"))
}

fn text(v: &Value) -> Result<&str, String> {
    v.as_str().ok_or_else(|| format!("expected a string, got {v}"))
}

pub fn point(v: &Value) -> Result<DVec3, String> {
    let a = v.as_array().filter(|a| a.len() >= 2).ok_or_else(|| format!("expected a point, got {v}"))?;
    Ok(DVec3::new(num(&a[0])?, num(&a[1])?, a.get(2).map(num).transpose()?.unwrap_or(0.0)))
}

fn points(v: &Value) -> Result<Vec<DVec3>, String> {
    v.as_array().ok_or_else(|| format!("expected a list of points, got {v}"))?.iter().map(point).collect()
}

fn matrix(v: &Value) -> Result<DMat4, String> {
    let a = v.as_array().filter(|a| a.len() == 16).ok_or("expected a transformation (16 numbers)")?;
    let mut m = [0.0; 16];
    for (i, x) in a.iter().enumerate() {
        m[i] = num(x)?;
    }
    Ok(DMat4::from_cols_array(&m))
}

fn pt_json(p: DVec3) -> Value {
    json!([p.x, p.y, p.z])
}

fn mat_json(m: &DMat4) -> Value {
    json!(m.to_cols_array().to_vec())
}

fn color_json(c: Option<sc_geom::Color>) -> Value {
    c.map_or(Value::Null, |c| json!(c.to_vec()))
}

fn color(v: &Value) -> Result<Option<sc_geom::Color>, String> {
    if v.is_null() {
        return Ok(None);
    }
    let a = v.as_array().filter(|a| a.len() >= 3).ok_or_else(|| format!("expected a color, got {v}"))?;
    let c = |i: usize| a.get(i).and_then(Value::as_f64).unwrap_or(255.0).clamp(0.0, 255.0) as u8;
    Ok(Some([c(0), c(1), c(2), c(3)]))
}

fn handles(doc: &Document, v: &Value) -> Result<Vec<Ent>, String> {
    v.as_array().ok_or("expected a list of entities")?.iter().map(|h| parse_ent(doc, text(h)?)).collect()
}

/// Edges, faces and instances of `ctx` among `ents` (others are ignored).
fn split(ents: &[Ent], ctx: Ctx) -> Selection {
    let mut s = Selection::default();
    for e in ents {
        match *e {
            Ent::Edge(c, k) if c == ctx => {
                s.edges.insert(k);
            }
            Ent::Face(c, k) if c == ctx => {
                s.faces.insert(k);
            }
            Ent::Inst(c, k) if c == ctx => {
                s.instances.insert(k);
            }
            _ => {}
        }
    }
    s
}

fn list_mesh(doc: &Document, ctx: Ctx) -> Vec<Value> {
    let m = doc.model.context_mesh(ctx);
    let faces = m.faces.keys().map(|k| Ent::Face(ctx, k));
    let edges = m.edges.keys().map(|k| Ent::Edge(ctx, k));
    let insts = m.instances.keys().map(|k| Ent::Inst(ctx, k));
    faces.chain(edges).chain(insts).map(|e| Value::String(ent_str(doc, e))).collect()
}

fn mesh_of(doc: &mut Document, ctx: Ctx) -> &mut Mesh {
    doc.model.context_mesh_mut(ctx)
}

/// The face created by drawing the closed polygon `pts` into `mesh`, if one was.
fn face_for_polygon(mesh: &Mesh, pts: &[DVec3]) -> Option<FaceId> {
    let plane = Plane::from_polygon(pts)?;
    let poly: Vec<DVec2> = pts.iter().map(|p| plane.to_2d(*p)).collect();
    let p = plane.to_3d(interior_point(&poly, &[])?);
    mesh.faces.iter().find(|(id, f)| f.plane.contains(p) && sc_math::parallel(f.plane.normal, plane.normal) && mesh.face_contains(*id, p)).map(|(id, _)| id)
}

/// Every place `def` is instanced: (context, instance).
fn instances_of(doc: &Document, def: DefId) -> Vec<(Ctx, InstanceId)> {
    let mut out = Vec::new();
    let mut scan = |ctx: Ctx, m: &Mesh| out.extend(m.instances.iter().filter(|(_, i)| i.def == def).map(|(id, _)| (ctx, id)));
    scan(None, &doc.model.mesh);
    for (d, def) in &doc.model.definitions {
        scan(Some(d), &def.mesh);
    }
    out
}

// ---------------------------------------------------------------------------

/// Viewport facts the API needs but the document does not hold.
pub struct ViewInfo {
    pub size: [f32; 2],
}

/// Answer one model request. `m` is the method name, `a` its positional arguments.
pub fn call(doc: &mut Document, view: &ViewInfo, m: &str, a: &[Value]) -> Result<Value, String> {
    let s = |e: Ent, doc: &Document| Value::String(ent_str(doc, e));
    Ok(match m {
        // ----- model
        "model.info" => {
            let title = doc.path.as_ref().and_then(|p| p.file_stem()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let path = doc.path.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
            json!({ "title": title, "path": path, "modified": doc.dirty, "active": ctx_str(doc.active_ctx()),
                    "unit": format!("{:?}", doc.model.units.unit) })
        }
        "model.active_path" => {
            let chain = doc.edit_chain();
            let mut ctx: Ctx = None;
            let mut out = Vec::new();
            for (inst, def) in chain {
                out.push(s(Ent::Inst(ctx, inst), doc));
                ctx = Some(def);
            }
            Value::Array(out)
        }
        "model.close_active" => Value::Bool(doc.exit()),
        "model.save" => {
            let path = match arg(a, 0).as_str() {
                Some(p) if !p.is_empty() => std::path::PathBuf::from(p),
                _ => doc.path.clone().ok_or("the model has never been saved; pass a path")?,
            };
            doc.save(&path)?;
            Value::Bool(true)
        }
        "model.bounds" => {
            let b = doc.model.bounds();
            if b.is_empty() { Value::Null } else { json!([pt_json(b.min), pt_json(b.max)]) }
        }

        // ----- entities collections
        "ents.list" => Value::Array(list_mesh(doc, parse_ctx(doc, text(arg(a, 0))?)?)),
        "ents.add_line" => {
            let ctx = parse_ctx(doc, text(arg(a, 0))?)?;
            let (p0, p1) = (point(arg(a, 1))?, point(arg(a, 2))?);
            let edges = mesh_of(doc, ctx).add_line(p0, p1);
            mesh_of(doc, ctx).revision += 1;
            // SketchUp returns the edge from p0; splitting may have produced several.
            let m = doc.model.context_mesh(ctx);
            let first = edges.iter().copied().min_by(|a, b| {
                let d = |e: &EdgeId| { let (x, y) = m.edge_points(*e); x.distance(p0).min(y.distance(p0)) };
                d(a).total_cmp(&d(b))
            });
            first.map_or(Value::Null, |e| s(Ent::Edge(ctx, e), doc))
        }
        "ents.add_edges" | "ents.add_curve" => {
            let ctx = parse_ctx(doc, text(arg(a, 0))?)?;
            let pts = points(arg(a, 1))?;
            let closed = arg(a, 2).as_bool().unwrap_or(false);
            let edges = mesh_of(doc, ctx).add_polyline(&pts, closed, m == "ents.add_curve");
            mesh_of(doc, ctx).revision += 1;
            Value::Array(edges.into_iter().map(|e| s(Ent::Edge(ctx, e), doc)).collect())
        }
        "ents.add_face" => {
            let ctx = parse_ctx(doc, text(arg(a, 0))?)?;
            let pts = points(arg(a, 1))?;
            if pts.len() < 3 {
                return Err("add_face needs at least 3 points".into());
            }
            let plane = Plane::from_polygon(&pts).ok_or("points are collinear")?;
            if pts.iter().any(|p| plane.signed_distance(*p).abs() > 1e-3) {
                return Err("points are not planar".into());
            }
            let mesh = mesh_of(doc, ctx);
            mesh.add_polyline(&pts, true, false);
            let Some(f) = face_for_polygon(mesh, &pts) else { return Ok(Value::Null) };
            // Faces follow the winding of the points, except on the ground where they face down.
            let on_ground = sc_math::parallel(plane.normal, DVec3::Z) && pts.iter().all(|p| p.z.abs() < 1e-3);
            if !on_ground && mesh.faces[f].plane.normal.dot(plane.normal) < 0.0 {
                mesh.reverse_face(f);
            }
            mesh.revision += 1;
            s(Ent::Face(ctx, f), doc)
        }
        "ents.add_group" => {
            let ctx = parse_ctx(doc, text(arg(a, 0))?)?;
            let ents = handles(doc, arg(a, 1))?;
            let sel = split(&ents, ctx);
            let edges: Vec<_> = sel.edges.into_iter().collect();
            let faces: Vec<_> = sel.faces.into_iter().collect();
            let insts: Vec<_> = sel.instances.into_iter().collect();
            let sub = mesh_of(doc, ctx).extract(&edges, &faces, &insts);
            let name = doc.model.unique_def_name("Group");
            let def = doc.model.definitions.insert(Definition { name, description: String::new(), is_group: true, mesh: sub });
            let mesh = mesh_of(doc, ctx);
            let id = mesh.instances.insert(Instance::new(def, DMat4::IDENTITY));
            mesh.revision += 1;
            s(Ent::Inst(ctx, id), doc)
        }
        "ents.add_instance" => {
            let ctx = parse_ctx(doc, text(arg(a, 0))?)?;
            let Ent::Def(def) = parse_ent(doc, text(arg(a, 1))?)? else { return Err("expected a ComponentDefinition".into()) };
            if Some(def) == ctx || def_contains(doc, def, ctx) {
                return Err("a component cannot contain itself".into());
            }
            let xf = matrix(arg(a, 2))?;
            let mesh = mesh_of(doc, ctx);
            let id = mesh.instances.insert(Instance::new(def, xf));
            mesh.revision += 1;
            s(Ent::Inst(ctx, id), doc)
        }
        "ents.erase" => {
            let ents = handles(doc, arg(a, 0))?;
            let ctxs: HashSet<Ctx> = ents.iter().filter_map(|e| ent_ctx(*e)).collect();
            for ctx in ctxs {
                let sel = split(&ents, ctx);
                let mesh = mesh_of(doc, ctx);
                mesh.erase_faces(&sel.faces.into_iter().collect::<Vec<_>>());
                mesh.erase_edges(&sel.edges.into_iter().filter(|e| mesh.edges.contains_key(*e)).collect::<Vec<_>>());
                for i in sel.instances {
                    mesh.instances.remove(i);
                }
                mesh.revision += 1;
            }
            for e in ents {
                if let Ent::Def(d) = e
                    && doc.model.instance_count(d) == 0 {
                        doc.model.definitions.remove(d);
                    }
            }
            Value::Null
        }
        "ents.transform" => {
            let ctx = parse_ctx(doc, text(arg(a, 0))?)?;
            let xf = matrix(arg(a, 1))?;
            let ents = handles(doc, arg(a, 2))?;
            let sel = split(&ents, ctx);
            let mesh = mesh_of(doc, ctx);
            let verts = sel.vertices(mesh);
            crate::tools::transform_selection(mesh, &sel, &verts, false, 1, |_| xf);
            Value::Null
        }
        "ents.clear" => {
            let ctx = parse_ctx(doc, text(arg(a, 0))?)?;
            let mesh = mesh_of(doc, ctx);
            let rev = mesh.revision;
            *mesh = Mesh::new();
            mesh.revision = rev + 1;
            Value::Null
        }
        "ents.add_guide" => {
            let ctx = parse_ctx(doc, text(arg(a, 0))?)?;
            let xf = if ctx == doc.active_ctx() { doc.active_xform() } else { DMat4::IDENTITY };
            let p = point(arg(a, 1))?;
            let g = match arg(a, 2) {
                Value::Null => Guide::Point { point: p },
                d => Guide::Line { point: p, dir: point(d)?.try_normalize().ok_or("zero-length guide direction")? },
            };
            doc.model.guides.push(g.transformed(&xf));
            mesh_of(doc, None).revision += 1;
            Value::Null
        }

        // ----- any entity
        "entity.valid" => Value::Bool(arg(a, 0).as_str().is_some_and(|h| parse_ent(doc, h).is_ok())),
        "entity.bounds" => {
            let b = match parse_ent(doc, text(arg(a, 0))?)? {
                Ent::Edge(c, e) => {
                    let (p, q) = doc.model.context_mesh(c).edge_points(e);
                    let mut b = sc_math::Aabb::EMPTY;
                    b.extend(p);
                    b.extend(q);
                    b
                }
                Ent::Face(c, f) => {
                    let mut b = sc_math::Aabb::EMPTY;
                    for p in doc.model.context_mesh(c).face_points(f) {
                        b.extend(p);
                    }
                    b
                }
                Ent::Vertex(c, v) => {
                    let mut b = sc_math::Aabb::EMPTY;
                    b.extend(doc.model.context_mesh(c).pos(v));
                    b
                }
                Ent::Inst(c, i) => doc.model.instance_bounds(&doc.model.context_mesh(c).instances[i]),
                Ent::Def(d) => doc.model.def_bounds(d),
            };
            if b.is_empty() { Value::Null } else { json!([pt_json(b.min), pt_json(b.max)]) }
        }

        // ----- edges
        "edge.info" => {
            let Ent::Edge(c, e) = parse_ent(doc, text(arg(a, 0))?)? else { return Err("expected an Edge".into()) };
            let mesh = doc.model.context_mesh(c);
            let ed = &mesh.edges[e];
            let faces: Vec<Value> = ed.faces.iter().map(|f| s(Ent::Face(c, *f), doc)).collect();
            let curve: Vec<Value> = ed.curve.map(|cv| mesh.curve_edges(cv).into_iter().map(|x| s(Ent::Edge(c, x), doc)).collect()).unwrap_or_default();
            json!({
                "start": [s(Ent::Vertex(c, ed.v[0]), doc), pt_json(mesh.pos(ed.v[0]))],
                "end": [s(Ent::Vertex(c, ed.v[1]), doc), pt_json(mesh.pos(ed.v[1]))],
                "length": mesh.edge_length(e), "soft": ed.soft, "smooth": ed.smooth, "hidden": ed.hidden,
                "faces": faces, "curve": curve,
            })
        }
        "edge.set" => {
            let Ent::Edge(c, e) = parse_ent(doc, text(arg(a, 0))?)? else { return Err("expected an Edge".into()) };
            let attrs = arg(a, 1).as_object().cloned().unwrap_or_default();
            let mesh = mesh_of(doc, c);
            let ed = &mut mesh.edges[e];
            let flag = |k: &str, cur: bool| attrs.get(k).and_then(Value::as_bool).unwrap_or(cur);
            (ed.soft, ed.smooth, ed.hidden) = (flag("soft", ed.soft), flag("smooth", ed.smooth), flag("hidden", ed.hidden));
            mesh.revision += 1;
            Value::Null
        }

        // ----- vertices
        "vertex.info" => {
            let Ent::Vertex(c, v) = parse_ent(doc, text(arg(a, 0))?)? else { return Err("expected a Vertex".into()) };
            let mesh = doc.model.context_mesh(c);
            let edges: Vec<Value> = mesh.vertex_edges(v).into_iter().map(|e| s(Ent::Edge(c, e), doc)).collect();
            let faces: Vec<Value> = mesh.faces.iter().filter(|(_, f)| f.vertex_ids().any(|x| x == v)).map(|(id, _)| s(Ent::Face(c, id), doc)).collect();
            json!({ "position": pt_json(mesh.pos(v)), "edges": edges, "faces": faces })
        }

        // ----- faces
        "face.info" => {
            let Ent::Face(c, f) = parse_ent(doc, text(arg(a, 0))?)? else { return Err("expected a Face".into()) };
            let mesh = doc.model.context_mesh(c);
            let face = &mesh.faces[f];
            let lp = |l: &Vec<VertexId>| -> Value {
                Value::Array(l.iter().map(|v| json!([s(Ent::Vertex(c, *v), doc), pt_json(mesh.pos(*v))])).collect())
            };
            let edges: Vec<Value> = face.loop_pairs().into_iter().filter_map(|(x, y)| mesh.edge_between(x, y)).map(|e| s(Ent::Edge(c, e), doc)).collect();
            let n = face.plane.normal;
            json!({
                "normal": pt_json(n), "plane": [n.x, n.y, n.z, -n.dot(face.plane.origin())],
                "area": mesh.face_area(f), "outer": lp(&face.outer),
                "holes": face.holes.iter().map(lp).collect::<Vec<_>>(), "edges": edges,
                "front": color_json(face.front), "back": color_json(face.back),
            })
        }
        "face.pushpull" => {
            let Ent::Face(c, f) = parse_ent(doc, text(arg(a, 0))?)? else { return Err("expected a Face".into()) };
            let dist = num(arg(a, 1))?;
            let mode = if arg(a, 2).as_bool().unwrap_or(false) { PushPullMode::NewStartingFace } else { PushPullMode::Auto };
            let mesh = mesh_of(doc, c);
            let out = mesh.push_pull(f, dist, mode);
            mesh.revision += 1;
            out.map_or(Value::Null, |nf| s(Ent::Face(c, nf), doc))
        }
        "face.reverse" => {
            let Ent::Face(c, f) = parse_ent(doc, text(arg(a, 0))?)? else { return Err("expected a Face".into()) };
            mesh_of(doc, c).reverse_face(f);
            mesh_of(doc, c).revision += 1;
            Value::Null
        }
        "face.set_material" => {
            let Ent::Face(c, f) = parse_ent(doc, text(arg(a, 0))?)? else { return Err("expected a Face".into()) };
            let col = color(arg(a, 1))?;
            let back = arg(a, 2).as_bool().unwrap_or(false);
            let mesh = mesh_of(doc, c);
            if back { mesh.faces[f].back = col } else { mesh.faces[f].front = col }
            mesh.revision += 1;
            Value::Null
        }
        "face.followme" => {
            let Ent::Face(c, f) = parse_ent(doc, text(arg(a, 0))?)? else { return Err("expected a Face".into()) };
            let path: Vec<EdgeId> = handles(doc, arg(a, 1))?
                .into_iter()
                .filter_map(|e| match e {
                    Ent::Edge(ec, k) if ec == c => Some(k),
                    _ => None,
                })
                .collect();
            let mesh = mesh_of(doc, c);
            let Some((pts, closed, soft)) = crate::tools::follow_me_path(mesh, f, &path) else { return Ok(Value::Bool(false)) };
            let made = mesh.follow_me(f, &pts, closed, &soft);
            mesh.revision += 1;
            Value::Bool(!made.is_empty())
        }

        // ----- groups and component instances
        "inst.info" => {
            let Ent::Inst(c, i) = parse_ent(doc, text(arg(a, 0))?)? else { return Err("expected a Group or ComponentInstance".into()) };
            let inst = &doc.model.context_mesh(c).instances[i];
            let tag = inst.tag.and_then(|t| doc.model.tags.get(t as usize)).map(|t| t.name.clone());
            json!({
                "definition": s(Ent::Def(inst.def), doc), "transformation": mat_json(&inst.xform),
                "name": inst.name, "hidden": inst.hidden, "locked": inst.locked, "layer": tag,
                "entities": ctx_str(Some(inst.def)),
            })
        }
        "inst.set" => {
            let Ent::Inst(c, i) = parse_ent(doc, text(arg(a, 0))?)? else { return Err("expected a Group or ComponentInstance".into()) };
            let attrs: Map<String, Value> = arg(a, 1).as_object().cloned().unwrap_or_default();
            let tag = match attrs.get("layer") {
                Some(Value::String(n)) => Some(Some(tag_index(doc, n))),
                Some(Value::Null) => Some(None),
                _ => None,
            };
            let xf = attrs.get("transformation").map(matrix).transpose()?;
            let mesh = mesh_of(doc, c);
            let inst = &mut mesh.instances[i];
            if let Some(n) = attrs.get("name").and_then(Value::as_str) {
                inst.name = n.to_string();
            }
            if let Some(h) = attrs.get("hidden").and_then(Value::as_bool) {
                inst.hidden = h;
            }
            if let Some(l) = attrs.get("locked").and_then(Value::as_bool) {
                inst.locked = l;
            }
            if let Some(t) = tag {
                inst.tag = t;
            }
            if let Some(xf) = xf {
                inst.xform = xf;
            }
            mesh.revision += 1;
            Value::Null
        }
        "inst.explode" => {
            let Ent::Inst(c, i) = parse_ent(doc, text(arg(a, 0))?)? else { return Err("expected a Group or ComponentInstance".into()) };
            let inst = mesh_of(doc, c).instances.remove(i).ok_or("deleted")?;
            let src = doc.model.definitions[inst.def].mesh.clone();
            let (edges, faces, insts) = mesh_of(doc, c).merge_mesh(&src, inst.xform);
            let mut out: Vec<Value> = faces.into_iter().map(|f| s(Ent::Face(c, f), doc)).collect();
            out.extend(edges.into_iter().map(|e| s(Ent::Edge(c, e), doc)));
            out.extend(insts.into_iter().map(|x| s(Ent::Inst(c, x), doc)));
            Value::Array(out)
        }
        "inst.make_unique" => {
            let Ent::Inst(c, i) = parse_ent(doc, text(arg(a, 0))?)? else { return Err("expected a Group or ComponentInstance".into()) };
            let def = doc.model.context_mesh(c).instances[i].def;
            if doc.model.instance_count(def) > 1 {
                let mut copy = doc.model.definitions[def].clone();
                copy.name = doc.model.unique_def_name(copy.name.split('#').next().unwrap_or("Component"));
                let nd = doc.model.definitions.insert(copy);
                let mesh = mesh_of(doc, c);
                mesh.instances[i].def = nd;
                mesh.revision += 1;
            }
            s(Ent::Inst(c, i), doc)
        }

        // ----- definitions
        "defs.list" => Value::Array(doc.model.definitions.keys().map(|d| s(Ent::Def(d), doc)).collect()),
        "defs.add" => {
            let base = arg(a, 0).as_str().filter(|n| !n.is_empty()).unwrap_or("Component");
            let name = if doc.model.definitions.values().any(|d| d.name == base) { doc.model.unique_def_name(base) } else { base.to_string() };
            let d = doc.model.definitions.insert(Definition { name, description: String::new(), is_group: false, mesh: Mesh::new() });
            s(Ent::Def(d), doc)
        }
        "def.info" => {
            let Ent::Def(d) = parse_ent(doc, text(arg(a, 0))?)? else { return Err("expected a ComponentDefinition".into()) };
            let def = &doc.model.definitions[d];
            let insts: Vec<Value> = instances_of(doc, d).into_iter().map(|(c, i)| s(Ent::Inst(c, i), doc)).collect();
            json!({ "name": def.name, "description": def.description, "group": def.is_group, "entities": ctx_str(Some(d)), "instances": insts })
        }
        "def.set" => {
            let Ent::Def(d) = parse_ent(doc, text(arg(a, 0))?)? else { return Err("expected a ComponentDefinition".into()) };
            let attrs = arg(a, 1).as_object().cloned().unwrap_or_default();
            let def = &mut doc.model.definitions[d];
            if let Some(n) = attrs.get("name").and_then(Value::as_str) {
                def.name = n.to_string();
            }
            if let Some(n) = attrs.get("description").and_then(Value::as_str) {
                def.description = n.to_string();
            }
            def.mesh.revision += 1;
            Value::Null
        }

        // ----- selection (always in the active context, like SketchUp)
        "sel.get" => {
            let ctx = doc.active_ctx();
            let sel = &doc.selection;
            let mut out: Vec<Value> = sel.faces.iter().map(|f| s(Ent::Face(ctx, *f), doc)).collect();
            out.extend(sel.edges.iter().map(|e| s(Ent::Edge(ctx, *e), doc)));
            out.extend(sel.instances.iter().map(|i| s(Ent::Inst(ctx, *i), doc)));
            Value::Array(out)
        }
        "sel.set" => {
            let ents = handles(doc, arg(a, 0))?;
            doc.selection = split(&ents, doc.active_ctx());
            Value::Null
        }

        // ----- tags (SketchUp's Ruby API still calls them layers)
        "tags.list" => Value::Array(doc.model.tags.iter().map(|t| json!({ "name": t.name, "visible": t.visible })).collect()),
        "tags.add" => {
            let name = text(arg(a, 0))?.to_string();
            Value::from(tag_index(doc, &name))
        }
        "tags.set_visible" => {
            let name = text(arg(a, 0))?;
            let vis = arg(a, 1).as_bool().unwrap_or(true);
            if let Some(t) = doc.model.tags.iter_mut().find(|t| t.name == name) {
                t.visible = vis;
            }
            Value::Null
        }

        // ----- view
        "view.size" => json!(view.size),
        "view.zoom_extents" => {
            let b = doc.model.bounds();
            let aspect = (view.size[0] / view.size[1].max(1.0)) as f64;
            doc.model.camera.zoom_extents(&b, aspect);
            Value::Null
        }
        "camera.get" => {
            let c = &doc.model.camera;
            json!({ "eye": pt_json(c.eye), "target": pt_json(c.target), "up": [0.0, 0.0, 1.0], "fov": c.fov_y, "perspective": c.perspective })
        }
        "camera.set" => {
            let attrs = arg(a, 0).as_object().cloned().unwrap_or_default();
            let c = &mut doc.model.camera;
            if let Some(v) = attrs.get("eye") {
                c.eye = point(v)?;
            }
            if let Some(v) = attrs.get("target") {
                c.target = point(v)?;
            }
            if let Some(v) = attrs.get("fov").and_then(Value::as_f64) {
                c.fov_y = v.clamp(1.0, 120.0);
            }
            if let Some(v) = attrs.get("perspective").and_then(Value::as_bool) {
                c.perspective = v;
            }
            Value::Null
        }
        _ => return Err(format!("SketchCAD does not implement '{m}' yet")),
    })
}

fn tag_index(doc: &mut Document, name: &str) -> u32 {
    if let Some(i) = doc.model.tags.iter().position(|t| t.name == name) {
        return i as u32;
    }
    doc.model.tags.push(crate::document::Tag { name: name.to_string(), visible: true, color: [160, 160, 160, 255] });
    (doc.model.tags.len() - 1) as u32
}

/// Does `def` (directly or through nesting) contain the context `ctx`'s definition?
fn def_contains(doc: &Document, def: DefId, ctx: Ctx) -> bool {
    let Some(target) = ctx else { return false };
    let mut stack = vec![def];
    let mut seen = HashSet::new();
    while let Some(d) = stack.pop() {
        if !seen.insert(d) {
            continue;
        }
        if d == target {
            return true;
        }
        if let Some(dd) = doc.model.definitions.get(d) {
            stack.extend(dd.mesh.instances.values().map(|i| i.def));
        }
    }
    false
}
