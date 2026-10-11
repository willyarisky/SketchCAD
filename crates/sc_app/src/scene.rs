//! Walking the context tree: render scenes and flattened geometry for inference.

use crate::document::Document;
use sc_geom::{DefId, EdgeId, FaceId, InstanceId, Mesh};
use sc_math::{Aabb, DMat4, DVec3};
use sc_render::{RenderScene, SceneOptions, flags, scene_rgba};

/// Highlights requested by the active tool, on top of the selection.
#[derive(Default)]
pub struct Highlights<'a> {
    pub edges: &'a [EdgeId],
    pub faces: &'a [FaceId],
    pub instances: &'a [InstanceId],
}

/// Visit every context on the edit path, outermost first: (context, its world transform, the
/// instance on the path inside it or `None` for the active context).
fn walk_path(doc: &Document, mut f: impl FnMut(Option<DefId>, &DMat4, Option<InstanceId>)) {
    let m = &doc.model;
    let mut ctx: Option<DefId> = None;
    let mut xf = DMat4::IDENTITY;
    for level in 0..=doc.edit_path.len() {
        let next = doc.edit_path.get(level).copied();
        f(ctx, &xf, next);
        let Some(n) = next else { break };
        let Some(i) = m.context_mesh(ctx).instances.get(n) else { break };
        xf *= i.xform;
        ctx = Some(i.def);
    }
}

/// Everything except the active context's own geometry, flattened into the active context's
/// local coordinates. Instances inside the active context are included.
pub fn flatten_outside_active(doc: &Document) -> Mesh {
    let m = &doc.model;
    let inv = doc.active_xform().inverse();
    let mut out = Mesh::new();
    walk_path(doc, |ctx, xf, next| {
        let mesh = m.context_mesh(ctx);
        let local = inv * *xf;
        if next.is_some() {
            out.append_raw(mesh, &local);
        }
        for (id, inst) in &mesh.instances {
            if Some(id) != next && m.instance_visible(inst) {
                m.flatten_into(inst.def, &(local * inst.xform), &mut out);
            }
        }
    });
    out
}

fn add_definition(doc: &Document, def: DefId, xf: &DMat4, opts: &SceneOptions, extra_flags: u32, s: &mut RenderScene) {
    let m = &doc.model;
    let Some(d) = m.definitions.get(def) else { return };
    s.add_mesh(&d.mesh, xf, opts, extra_flags, |_| false, |_| false);
    for i in d.mesh.instances.values() {
        if m.instance_visible(i) {
            add_definition(doc, i.def, &(*xf * i.xform), opts, extra_flags, s);
        }
    }
}

/// Corners of a definition's bounding box transformed to world space.
pub fn box_corners(b: &Aabb, xf: &DMat4) -> [DVec3; 8] {
    std::array::from_fn(|i| {
        let p = DVec3::new(
            if i & 1 == 0 { b.min.x } else { b.max.x },
            if i & 2 == 0 { b.min.y } else { b.max.y },
            if i & 4 == 0 { b.min.z } else { b.max.z },
        );
        xf.transform_point3(p)
    })
}

/// The whole model as a render scene. Geometry outside the open group/component is faded and its
/// bounds are drawn dashed; selected groups/components get a blue bounding box.
pub fn build_scene(doc: &Document, opts: &SceneOptions, hl: &Highlights) -> RenderScene {
    let m = &doc.model;
    let sel = &doc.selection;
    let mut s = RenderScene::empty(opts.style);
    let blue = scene_rgba([0, 0, 255, 255]);
    let gray = scene_rgba([90, 90, 90, 255]);
    walk_path(doc, |ctx, xf, next| {
        let mesh = m.context_mesh(ctx);
        let active = next.is_none();
        let fade = if active { 0 } else { flags::FADED };
        if active {
            s.add_mesh(mesh, xf, opts, 0, |e| sel.edges.contains(&e) || hl.edges.contains(&e), |f| sel.faces.contains(&f) || hl.faces.contains(&f));
        } else {
            s.add_mesh(mesh, xf, opts, fade, |_| false, |_| false);
        }
        for (id, inst) in &mesh.instances {
            if !m.instance_visible(inst) {
                continue;
            }
            let wx = *xf * inst.xform;
            let selected = active && (sel.instances.contains(&id) || hl.instances.contains(&id));
            let is_open = Some(id) == next;
            if is_open {
                // The open context is drawn by the next level; outline its bounds.
                s.push_box(&box_corners(&m.def_bounds(inst.def), &wx), gray, 4.0);
                continue;
            }
            add_definition(doc, inst.def, &wx, opts, fade, &mut s);
            if selected {
                s.push_box(&box_corners(&m.def_bounds(inst.def), &wx), blue, 0.0);
            }
        }
    });
    if opts.show_axes {
        s.push_axes();
    }
    s
}
