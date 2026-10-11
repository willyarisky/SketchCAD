// SketchCAD viewport shaders. Reversed-Z depth (1 = near, 0 = far).

struct Globals {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    // xy: viewport size in px, z: 1 = perspective, w: face alpha (x-ray)
    viewport: vec4<f32>,
    // xyz: camera forward, w: device pixels per UI point (scales line widths and dashes)
    forward: vec4<f32>,
    // xyz: light direction (towards light), w: unused
    light: vec4<f32>,
    sky_top: vec4<f32>,
    sky_horizon: vec4<f32>,
    ground: vec4<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;

// ---------------------------------------------------------------------------
// Background: sky gradient + ground, evaluated per pixel from the view ray.

struct BgOut {
    @builtin(position) pos: vec4<f32>,
};

@vertex
fn vs_bg(@builtin(vertex_index) i: u32) -> BgOut {
    var p = array<vec2<f32>, 3>(vec2(-1.0, -3.0), vec2(3.0, 1.0), vec2(-1.0, 1.0));
    var o: BgOut;
    o.pos = vec4(p[i], 0.0, 1.0);
    return o;
}

fn unproject(ndc: vec2<f32>, z: f32) -> vec3<f32> {
    let v = g.inv_view_proj * vec4(ndc, z, 1.0);
    return v.xyz / v.w;
}

@fragment
fn fs_bg(in: BgOut) -> @location(0) vec4<f32> {
    let ndc = vec2(in.pos.x / g.viewport.x * 2.0 - 1.0, 1.0 - in.pos.y / g.viewport.y * 2.0);
    let dir = normalize(unproject(ndc, 0.5) - unproject(ndc, 1.0));
    if (dir.z >= 0.0) {
        let t = pow(clamp(dir.z, 0.0, 1.0), 0.6);
        return vec4(mix(g.sky_horizon.rgb, g.sky_top.rgb, t), 1.0);
    }
    return vec4(g.ground.rgb, 1.0);
}

// ---------------------------------------------------------------------------
// Faces

struct FaceIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) front: vec4<f32>,
    @location(3) back: vec4<f32>,
    @location(4) flags: u32,
};

struct FaceOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) front: vec4<f32>,
    @location(3) back: vec4<f32>,
    @location(4) @interpolate(flat) flags: u32,
};

@vertex
fn vs_face(in: FaceIn) -> FaceOut {
    var o: FaceOut;
    o.pos = g.view_proj * vec4(in.pos, 1.0);
    o.world = in.pos;
    o.normal = in.normal;
    o.front = in.front;
    o.back = in.back;
    o.flags = in.flags;
    return o;
}

const FLAG_SELECTED: u32 = 1u;
const FLAG_UNLIT: u32 = 2u;
const FLAG_FADED: u32 = 4u;

@fragment
fn fs_face(in: FaceOut) -> @location(0) vec4<f32> {
    var to_eye = -g.forward.xyz;
    if (g.viewport.z > 0.5) {
        to_eye = normalize(g.eye.xyz - in.world);
    }
    let n = normalize(in.normal);
    let facing = dot(n, to_eye) >= 0.0;
    var base = in.back;
    var ne = -n;
    if (facing) {
        base = in.front;
        ne = n;
    }
    var shade = 1.0;
    if ((in.flags & FLAG_UNLIT) == 0u) {
        // SketchUp-like soft lighting: strong ambient, gentle directional term.
        let l = normalize(g.light.xyz);
        shade = 0.62 + 0.38 * max(dot(ne, l), 0.0);
    }
    var c = vec4(base.rgb * shade, base.a * g.viewport.w);
    if ((in.flags & FLAG_SELECTED) != 0u) {
        // Selection stipple: a sparse grid of blue dots.
        let px = vec2<i32>(in.pos.xy);
        if ((px.x % 3) == 0 && (px.y % 3) == 0) {
            c = vec4(0.0, 0.0, 1.0, 1.0);
        }
    }
    if ((in.flags & FLAG_FADED) != 0u) {
        c = vec4(mix(c.rgb, vec3(0.92), 0.6), c.a);
    }
    return c;
}

// ---------------------------------------------------------------------------
// Lines: screen-space quads expanded from instanced segments.

struct LineIn {
    @location(0) a: vec3<f32>,
    @location(1) b: vec3<f32>,
    @location(2) color: vec4<f32>,
    // x: width px, y: dash length px (0 = solid), z: depth bias factor
    @location(3) style: vec4<f32>,
};

struct LineOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) @interpolate(linear) along: f32,
    @location(2) dash: f32,
};

@vertex
fn vs_line(@builtin(vertex_index) vi: u32, in: LineIn) -> LineOut {
    var o: LineOut;
    var pa = g.view_proj * vec4(in.a, 1.0);
    var pb = g.view_proj * vec4(in.b, 1.0);
    // Clip against the camera plane so segments crossing it don't explode.
    let eps = 1e-4;
    if (pa.w < eps && pb.w < eps) {
        o.pos = vec4(2.0, 2.0, 2.0, 1.0);
        return o;
    }
    if (pa.w < eps) {
        pa = mix(pa, pb, (eps - pa.w) / (pb.w - pa.w));
    }
    if (pb.w < eps) {
        pb = mix(pb, pa, (eps - pb.w) / (pa.w - pb.w));
    }
    let half_vp = g.viewport.xy * 0.5;
    let sa = pa.xy / pa.w * half_vp;
    let sb = pb.xy / pb.w * half_vp;
    var dir = sb - sa;
    let len = length(dir);
    if (len < 1e-6) {
        dir = vec2(1.0, 0.0);
    } else {
        dir = dir / len;
    }
    let nrm = vec2(-dir.y, dir.x);
    // Corner table: (end, side)
    var corners = array<vec2<f32>, 6>(
        vec2(0.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
        vec2(0.0, -1.0), vec2(1.0, 1.0), vec2(0.0, 1.0),
    );
    let c = corners[vi];
    let hw = in.style.x * 0.5 * g.forward.w;
    var p = pa;
    var cap = -1.0;
    if (c.x > 0.5) {
        p = pb;
        cap = 1.0;
    }
    let off_px = nrm * c.y * hw + dir * cap * hw * 0.5;
    p = vec4(p.xy + off_px / half_vp * p.w, p.z, p.w);
    // Reversed-Z bias: pull lines slightly toward the camera so they win over coplanar faces.
    p.z = p.z * in.style.z;
    o.pos = p;
    o.color = in.color;
    o.along = c.x * len;
    o.dash = in.style.y * g.forward.w;
    return o;
}

@fragment
fn fs_line(in: LineOut) -> @location(0) vec4<f32> {
    if (in.dash > 0.0 && fract(in.along / (in.dash * 2.0)) > 0.5) {
        discard;
    }
    return in.color;
}
