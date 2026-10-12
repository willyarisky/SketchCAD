//! End-to-end UI tests: the real app, rendered headlessly with wgpu, driven by mouse/keyboard events.
//! Screenshots land in `target/screenshots/` for visual review.

use crate::app::{LaunchOptions, SketchCadApp};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use sc_math::DVec3;

fn harness(opts: LaunchOptions) -> Harness<'static, SketchCadApp> {
    let mut h = Harness::builder().with_size([1440.0, 900.0]).with_theme(egui::Theme::Light).wgpu().build_eframe(|cc| SketchCadApp::new(cc, opts));
    h.run_steps(4);
    h
}

fn screen_of(h: &Harness<SketchCadApp>, p: DVec3) -> Pos2 {
    let app = h.state();
    let s = app.doc.model.camera.project(p, app.viewport.size).expect("point behind camera");
    pos2(app.viewport.rect.min.x + s.x as f32, app.viewport.rect.min.y + s.y as f32)
}

fn move_to(h: &mut Harness<SketchCadApp>, p: Pos2) {
    h.event(Event::PointerMoved(p));
    h.step();
}

fn click(h: &mut Harness<SketchCadApp>, p: Pos2) {
    move_to(h, p);
    h.event(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.event(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.step();
    // Let the double-click window pass so consecutive clicks stay single clicks.
    std::thread::sleep(std::time::Duration::from_millis(350));
}

fn type_vcb(h: &mut Harness<SketchCadApp>, text: &str) {
    h.event(Event::Text(text.to_string()));
    h.step();
    h.key_press(Key::Enter);
    h.step();
}

fn press(h: &mut Harness<SketchCadApp>, key: Key) {
    h.key_press(key);
    h.step();
}

fn save(h: &mut Harness<SketchCadApp>, name: &str) {
    let img = h.render().expect("render");
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/screenshots");
    std::fs::create_dir_all(&dir).unwrap();
    img.save(dir.join(format!("{name}.png"))).unwrap();
}

const MM: f64 = 1.0 / 25.4;

#[test]
fn demo_house_renders() {
    let mut h = harness(LaunchOptions { demo: true, ..Default::default() });
    h.run_steps(4);
    let app = h.state();
    assert_eq!(app.doc.model.mesh.instances.len(), 4, "house group + 3 bushes");
    save(&mut h, "demo_house");
}

#[test]
fn draw_rectangle_push_pull_and_undo() {
    let mut h = harness(LaunchOptions::default());

    // Rectangle tool (R): click two corners, then type exact dimensions.
    press(&mut h, Key::R);
    assert_eq!(h.state().tool.id(), crate::tools::ToolId::Rectangle);
    let a = screen_of(&h, DVec3::new(20.0, 20.0, 0.0));
    let b = screen_of(&h, DVec3::new(120.0, 90.0, 0.0));
    click(&mut h, a);
    click(&mut h, b);
    assert_eq!(h.state().doc.model.mesh.faces.len(), 1, "rectangle should make one face");
    type_vcb(&mut h, "2000,1500");
    let face = h.state().doc.model.mesh.faces.keys().next().unwrap();
    let area = h.state().doc.model.mesh.face_area(face);
    assert!((area - 2000.0 * MM * 1500.0 * MM).abs() < 1e-6, "retyped dimensions, area {area}");

    // Push/Pull (P): click the face, drag a little, type the exact height.
    press(&mut h, Key::P);
    let c = screen_of(&h, DVec3::new(20.0 + 1000.0 * MM, 20.0 + 750.0 * MM, 0.0));
    click(&mut h, c);
    move_to(&mut h, c - egui::vec2(0.0, 60.0));
    type_vcb(&mut h, "1200");
    let m = &h.state().doc.model.mesh;
    assert_eq!(m.faces.len(), 6, "box");
    assert!((m.bounds().max.z - 1200.0 * MM).abs() < 1e-6, "height {}", m.bounds().max.z / MM);
    m.validate().unwrap();

    // Hover a top corner with the Line tool to show the inference marker, and screenshot.
    press(&mut h, Key::L);
    let corner = screen_of(&h, DVec3::new(20.0, 20.0, 1200.0 * MM));
    move_to(&mut h, corner + egui::vec2(2.0, 1.0));
    move_to(&mut h, corner + egui::vec2(1.0, 1.0));
    assert_eq!(h.state().inference.last.map(|i| i.kind), Some(crate::inference::Kind::Endpoint));
    save(&mut h, "box_endpoint_inference");

    // Undo twice (Cmd/Ctrl+Z): back to the bare rectangle, then nothing.
    press(&mut h, Key::Escape);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.step();
    assert_eq!(h.state().doc.model.mesh.faces.len(), 1);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.step();
    assert_eq!(h.state().doc.model.mesh.faces.len(), 0);
}

#[test]
fn line_tool_measurements_box_and_axis_lock() {
    let mut h = harness(LaunchOptions::default());
    press(&mut h, Key::L);
    let start = screen_of(&h, DVec3::new(30.0, 30.0, 0.0));
    click(&mut h, start);
    // Point roughly along the green axis, lock it with the left arrow, type a length.
    let pt = screen_of(&h, DVec3::new(34.0, 90.0, 0.0));
    move_to(&mut h, pt);
    press(&mut h, Key::ArrowLeft);
    let pt = screen_of(&h, DVec3::new(36.0, 95.0, 0.0));
    move_to(&mut h, pt);
    type_vcb(&mut h, "500");
    let m = &h.state().doc.model.mesh;
    assert_eq!(m.edges.len(), 1);
    let e = m.edges.keys().next().unwrap();
    let (p, q) = m.edge_points(e);
    assert!((p.distance(q) - 500.0 * MM).abs() < 1e-6, "length {}", p.distance(q) / MM);
    assert!(sc_math::parallel(q - p, DVec3::Y), "locked to green axis: {:?}", q - p);
    save(&mut h, "line_axis_lock");
}

#[test]
fn circle_on_face_and_select() {
    let mut h = harness(LaunchOptions::default());
    // Draw a rectangle by typing absolute coordinates isn't supported for rectangles, so click.
    press(&mut h, Key::R);
    let pt = screen_of(&h, DVec3::new(0.0, 0.0, 0.0));
    click(&mut h, pt);
    let pt = screen_of(&h, DVec3::new(100.0, 100.0, 0.0));
    click(&mut h, pt);
    type_vcb(&mut h, "3000,3000");
    press(&mut h, Key::C);
    let center = screen_of(&h, DVec3::new(1500.0 * MM, 1500.0 * MM, 0.0));
    click(&mut h, center);
    move_to(&mut h, center + egui::vec2(40.0, 0.0));
    type_vcb(&mut h, "600");
    let m = &h.state().doc.model.mesh;
    assert_eq!(m.faces.len(), 2, "circle splits the face");
    // Select tool (Space): click the disc, Entity Info shows one face.
    press(&mut h, Key::Space);
    click(&mut h, center);
    assert_eq!(h.state().doc.selection.faces.len(), 1);
    h.run_steps(2);
    save(&mut h, "circle_selected");
}

#[test]
fn make_component_copy_and_edit_shared_definition() {
    let mut h = harness(LaunchOptions::default());
    // A 1000 mm cube.
    press(&mut h, Key::R);
    let pt = screen_of(&h, DVec3::new(0.0, 0.0, 0.0));
    click(&mut h, pt);
    let pt = screen_of(&h, DVec3::new(40.0, 40.0, 0.0));
    click(&mut h, pt);
    type_vcb(&mut h, "1000,1000");
    press(&mut h, Key::P);
    let c = screen_of(&h, DVec3::new(500.0 * MM, 500.0 * MM, 0.0));
    click(&mut h, c);
    move_to(&mut h, c - egui::vec2(0.0, 40.0));
    type_vcb(&mut h, "1000");
    assert_eq!(h.state().doc.model.mesh.faces.len(), 6);

    // Select all, G, Enter in the dialog: one component instance remains in the model.
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.step();
    press(&mut h, Key::G);
    h.run_steps(2);
    assert!(h.state().component_dialog.is_some(), "Make Component dialog opens");
    press(&mut h, Key::Enter);
    h.run_steps(2);
    let root = &h.state().doc.model.mesh;
    assert_eq!((root.faces.len(), root.edges.len(), root.instances.len()), (0, 0, 1));
    let def = root.instances.values().next().unwrap().def;
    assert!(!h.state().doc.model.definitions[def].is_group);

    // Move a copy 2000 mm along red: Space to deselect first, then M, toggle copy, pick the cube.
    press(&mut h, Key::Escape);
    press(&mut h, Key::M);
    let copy_mod = if cfg!(target_os = "macos") { Modifiers::ALT } else { Modifiers::CTRL };
    h.event(Event::ModifiersChanged(copy_mod));
    h.step();
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.step();
    let start = screen_of(&h, DVec3::new(500.0 * MM, 0.0, 500.0 * MM));
    click(&mut h, start);
    let pt = screen_of(&h, DVec3::new(2500.0 * MM, 0.0, 500.0 * MM));
    move_to(&mut h, pt);
    type_vcb(&mut h, "2000");
    let root = &h.state().doc.model.mesh;
    assert_eq!(root.instances.len(), 2, "copied instance");
    assert_eq!(h.state().doc.model.instance_count(def), 2, "copy shares the definition");

    // Double-click the copy to open it, then push/pull its top face up 500 mm.
    press(&mut h, Key::Space);
    let inside = screen_of(&h, DVec3::new(2500.0 * MM, 0.0, 500.0 * MM));
    click(&mut h, inside);
    h.event(Event::PointerButton { pos: inside, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.event(Event::PointerButton { pos: inside, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.step();
    assert_eq!(h.state().doc.edit_path.len(), 1, "double-click opens the component");
    press(&mut h, Key::P);
    let top = screen_of(&h, DVec3::new(2500.0 * MM, 500.0 * MM, 1000.0 * MM));
    click(&mut h, top);
    move_to(&mut h, top - egui::vec2(0.0, 30.0));
    type_vcb(&mut h, "500");
    h.run_steps(2);
    save(&mut h, "component_edit");
    let b = h.state().doc.model.def_bounds(def);
    assert!((b.max.z - 1500.0 * MM).abs() < 1e-6, "definition grew to 1500 mm: {}", b.max.z / MM);
    // Both instances changed: model bounds reach 1500 mm everywhere.
    assert!((h.state().doc.model.bounds().max.z - 1500.0 * MM).abs() < 1e-6);

    // Close the component and undo the push/pull.
    press(&mut h, Key::Space);
    let empty = h.state().viewport.rect.min + egui::vec2(30.0, 30.0);
    click(&mut h, empty);
    assert!(h.state().doc.edit_path.is_empty(), "clicking outside closes the component");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.step();
    let b = h.state().doc.model.def_bounds(def);
    assert!((b.max.z - 1000.0 * MM).abs() < 1e-6, "undo restored the definition");
}

#[test]
fn tags_scenes_and_outliner() {
    let mut h = harness(LaunchOptions { demo: true, ..Default::default() });
    h.run_steps(3);

    // Scene 1: the start-up view.
    h.state_mut().add_scene();
    let cam1 = h.state().doc.model.camera;

    // A "Planting" tag on the bushes, then hidden: model bounds shrink to the house.
    {
        let app = h.state_mut();
        app.doc.model.tags.push(crate::document::Tag { name: "Planting".into(), visible: true, color: [90, 180, 90, 255] });
        for i in app.doc.model.mesh.instances.values_mut() {
            if app.doc.model.definitions[i.def].name == "Bush" {
                i.tag = Some(0);
            }
        }
        app.doc.model.mesh.revision += 1;
    }
    let with_bushes = h.state().doc.model.bounds();
    h.state_mut().doc.model.tags[0].visible = false;
    let without = h.state().doc.model.bounds();
    assert!(without.min.x > with_bushes.min.x, "hidden tag excluded from bounds");

    // Scene 2: top view with the tag hidden.
    h.state_mut().run(&egui::Context::default(), crate::commands::Command::StandardView(sc_render::StandardView::Top));
    h.state_mut().add_scene();
    assert_eq!(h.state().doc.model.scenes.len(), 2);

    // Back to scene 1: tags restore immediately, the camera animates there.
    h.state_mut().go_to_scene(0);
    assert!(h.state().doc.model.tags[0].visible);
    std::thread::sleep(std::time::Duration::from_millis(900));
    h.run_steps(3);
    let cam = h.state().doc.model.camera;
    assert!(cam.eye.distance(cam1.eye) < 1e-6, "camera reached scene 1");

    // Expand tray panels and screenshot them.
    for label in ["Outliner", "Tags", "Scenes"] {
        h.get_by_label(label).click();
        h.run_steps(2);
    }
    h.run_steps(2);
    save(&mut h, "tags_scenes_outliner");
}

#[test]
fn follow_me_and_scale_tools() {
    let mut h = harness(LaunchOptions::default());
    // Path: a 1000 mm circle on the ground. Profile: a 200 mm square standing on it, facing the path.
    {
        let m = &mut h.state_mut().doc.model.mesh;
        let n = 24;
        let r = 1000.0 * MM;
        let pts: Vec<DVec3> = (0..n).map(|i| {
            let a = i as f64 / n as f64 * std::f64::consts::TAU;
            DVec3::new(r * a.cos(), r * a.sin(), 0.0)
        }).collect();
        m.add_polyline(&pts, true, true);
        let (x0, x1, z0, z1) = (900.0 * MM, 1100.0 * MM, 100.0 * MM, 300.0 * MM);
        m.add_polyline(&[DVec3::new(x0, 0.0, z0), DVec3::new(x1, 0.0, z0), DVec3::new(x1, 0.0, z1), DVec3::new(x0, 0.0, z1)], true, false);
    }
    // The circle also bounds a disc face; select only the circle's edges as the path.
    let edges: Vec<_> = h.state().doc.model.mesh.edges.iter().filter(|(_, e)| e.curve.is_some()).map(|(id, _)| id).collect();
    assert_eq!(edges.len(), 24);
    h.state_mut().doc.selection.edges.extend(edges);
    h.state_mut().set_tool(crate::tools::ToolId::FollowMe);
    h.state_mut().run(&egui::Context::default(), crate::commands::Command::ZoomExtents);
    h.run_steps(2);
    let profile = screen_of(&h, DVec3::new(1000.0 * MM, 0.0, 200.0 * MM));
    click(&mut h, profile);
    let m = &h.state().doc.model.mesh;
    m.validate().unwrap();
    assert_eq!(m.faces.len(), 1 + 24 * 4, "ground disc + lathed ring");
    h.run_steps(2);
    save(&mut h, "follow_me_ring");

    // Scale: select everything, grab the top corner grip, type 2.
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.step();
    press(&mut h, Key::S);
    let b = h.state().doc.model.bounds();
    let corner = screen_of(&h, b.max);
    move_to(&mut h, corner + egui::vec2(1.0, 0.0));
    move_to(&mut h, corner);
    h.event(Event::PointerButton { pos: corner, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    move_to(&mut h, corner + egui::vec2(20.0, -10.0));
    type_vcb(&mut h, "2");
    let b2 = h.state().doc.model.bounds();
    assert!((b2.min - b.min).length() < 1e-6, "scaled about the opposite corner");
    assert!(((b2.max - b2.min) - (b.max - b.min) * 2.0).length() < 1e-6, "doubled");
    h.state().doc.model.mesh.validate().unwrap();
}

/// Step the app until `cond` holds (Ruby runs in its own process, so this takes real time).
fn step_until(h: &mut Harness<SketchCadApp>, what: &str, cond: impl Fn(&SketchCadApp) -> bool) {
    let t0 = std::time::Instant::now();
    while !cond(h.state()) {
        assert!(t0.elapsed().as_secs() < 30, "timed out waiting for {what}; console: {:#?}", h.state().plugins.console);
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.run_steps(3);
}

#[test]
fn install_and_run_sketchup_extension() {
    use crate::plugins::{HostState, manager::Dirs};
    if crate::plugins::host::find_ruby().is_none() {
        eprintln!("skipping: no Ruby interpreter");
        return;
    }
    let mut h = harness(LaunchOptions::default());
    let data = std::env::temp_dir().join(format!("skcad-ui-ext-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    {
        let app = h.state_mut();
        app.plugins = crate::plugins::Plugins::new(Dirs::new(data.clone()));
        app.plugins.start();
        app.plugins.install(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/extensions/box_maker.rbz"));
    }
    step_until(&mut h, "extension loaded", |a| a.plugins.state == HostState::Ready && !a.plugins.busy() && !a.plugins.menus.is_empty());

    // Extensions > Box Maker > Create Box... opens the extension's input box; OK draws the box.
    h.get_by_label("Extensions").click();
    h.run_steps(2);
    h.get_by_label("Box Maker ⏵").hover();
    h.run_steps(2);
    save(&mut h, "extension_menu");
    h.get_by_label("Create Box...").click();
    step_until(&mut h, "input box", |a| a.plugins.dialog.is_some());
    save(&mut h, "extension_inputbox");
    h.get_by_label("OK").click();
    step_until(&mut h, "box", |a| !a.plugins.busy() && !a.doc.model.mesh.instances.is_empty());
    let app = h.state();
    let inst = app.doc.model.mesh.instances.values().next().unwrap();
    let b = app.doc.model.def_bounds(inst.def);
    assert_eq!(((b.max.x - b.min.x) / MM).round(), 1000.0);
    assert_eq!(app.doc.history.undo_name(), Some("Create Box"));

    // Extension Manager and Ruby Console.
    h.state_mut().plugins.show_manager = true;
    h.state_mut().plugins.show_console = true;
    h.state_mut().plugins.enqueue(crate::plugins::Job::Eval("Sketchup.active_model.selection.first.definition.entities.grep(Sketchup::Face).length".into()));
    let ctx = h.ctx.clone();
    h.state_mut().run(&ctx, crate::commands::Command::ZoomExtents);
    step_until(&mut h, "console", |a| !a.plugins.busy());
    assert!(h.state().plugins.console.iter().any(|(_, l)| l == "6"), "{:#?}", h.state().plugins.console);
    save(&mut h, "extension_manager");

    h.state_mut().plugins.stop();
    std::fs::remove_dir_all(data).ok();
}

#[test]
fn open_sketchup_file() {
    let sample = std::path::Path::new("/Applications/SketchUp 2017/SketchUp.app/Contents/Resources/Content/Components/Components Sampler/Car Sedan.skp");
    let ready = sc_skp::locate::find_sdk().is_some_and(|s| sc_skp::locate::find_converter(&s).is_ok());
    if !sample.is_file() || !ready {
        eprintln!("skipping: no SketchUp library/converter or sample");
        return;
    }
    let mut h = harness(LaunchOptions::default());
    h.state_mut().open_path(sample);
    step_until(&mut h, "skp open", |a| a.skp_open.is_none());
    assert!(h.state().error_dialog.is_none(), "{:?}", h.state().error_dialog);
    assert!(h.state().doc.model.definitions.len() >= 5);
    let ctx = h.ctx.clone();
    h.state_mut().run(&ctx, crate::commands::Command::StandardView(sc_render::StandardView::Iso));
    h.state_mut().run(&ctx, crate::commands::Command::ZoomExtents);
    h.run_steps(4);
    save(&mut h, "skp_car_sedan");
}

/// A textured face really reaches the screen: a red/white checker laid on the ground shows both
/// its colours in the viewport, and switching to the plain Shaded style hides the pattern.
#[test]
fn textured_face_renders() {
    let mut h = harness(LaunchOptions::default());
    {
        let app = h.state_mut();
        let png = {
            let img = image::RgbaImage::from_fn(16, 16, |x, y| if (x / 8 + y / 8) % 2 == 0 { image::Rgba([230, 20, 20, 255]) } else { image::Rgba([255, 255, 255, 255]) });
            let mut out = std::io::Cursor::new(Vec::new());
            img.write_to(&mut out, image::ImageFormat::Png).unwrap();
            out.into_inner()
        };
        let m = &mut app.doc.model;
        let tex = m.textures.insert(sc_geom::Texture { name: "check.png".into(), width: 16, height: 16, size: [40.0, 40.0], data: png });
        let mut b = sc_geom::MeshBuilder::new();
        let sq = [DVec3::ZERO, DVec3::new(400.0, 0.0, 0.0), DVec3::new(400.0, 400.0, 0.0), DVec3::new(0.0, 400.0, 0.0)];
        let f = b.face(&sq, &[], DVec3::Z, None, None).unwrap();
        let uv = sc_geom::UvMap { u: sc_math::DVec4::new(1.0 / 40.0, 0.0, 0.0, 0.0), v: sc_math::DVec4::new(0.0, 1.0 / 40.0, 0.0, 0.0) };
        b.face_mut(f).front_tex = Some(sc_geom::FaceTexture { tex, uv });
        m.mesh = b.finish();
        app.pending_zoom_extents = true;
    }
    h.run_steps(6);

    // Count how much of the viewport is the texture's red, which no other part of the UI uses.
    let reds = |h: &mut Harness<SketchCadApp>| {
        let img = h.render().expect("render");
        img.pixels().filter(|p| p.0[0] > 170 && p.0[1] < 110 && p.0[2] < 110).count()
    };
    let textured = reds(&mut h);
    assert!(textured > 2000, "the checker pattern should cover a good part of the view, got {textured} red pixels");
    save(&mut h, "textured_face");

    h.state_mut().doc.model.style.face_style = sc_render::FaceStyle::Shaded;
    h.run_steps(4);
    let plain = reds(&mut h);
    assert!(plain * 10 < textured, "Shaded style should drop the texture: {plain} vs {textured}");
}

/// The Measurements box resizes by dragging the grip beside it, as SketchUp's does: left to
/// widen, right to narrow, and it refuses to shrink away or eat the whole status bar.
#[test]
fn measurements_box_resizes_by_dragging() {
    let mut h = harness(LaunchOptions::default());
    let start = h.state().vcb_width;
    assert_eq!(start, crate::ui::VCB_DEFAULT_WIDTH);

    let grip = |h: &Harness<SketchCadApp>| h.state().vcb_grip_rect.center();
    let drag = |h: &mut Harness<SketchCadApp>, from: Pos2, dx: f32| {
        h.event(Event::PointerMoved(from));
        h.step();
        h.event(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
        h.step();
        // egui reads a drag from pointer motion while the button is held.
        for i in 1..=4 {
            h.event(Event::PointerMoved(from + egui::vec2(dx * i as f32 / 4.0, 0.0)));
            h.step();
        }
        h.event(Event::PointerButton { pos: from + egui::vec2(dx, 0.0), button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
        h.step();
    };

    let g = grip(&h);
    drag(&mut h, g, -80.0);
    let wider = h.state().vcb_width;
    assert!(wider > start + 50.0, "dragging left should widen: {start} -> {wider}");
    save(&mut h, "measurements_widened");

    let g = grip(&h);
    drag(&mut h, g, 60.0);
    let narrower = h.state().vcb_width;
    assert!(narrower < wider - 30.0, "dragging right should narrow: {wider} -> {narrower}");

    // Yanking far right stops at the minimum rather than collapsing the box.
    let g = grip(&h);
    drag(&mut h, g, 4000.0);
    assert_eq!(h.state().vcb_width, 60.0, "clamped to the minimum width");
    save(&mut h, "measurements_resized");
}

/// Over the viewport the tool draws its own cursor, so the OS one is hidden; elsewhere the
/// ordinary pointer comes back. Orbit used to borrow the grab hand and read as Pan.
#[test]
fn tool_draws_its_own_cursor() {
    let mut h = harness(LaunchOptions { demo: true, ..Default::default() });
    h.run_steps(3);
    // Read it off the finished frame: ctx.output() is reset once the pass has been consumed.
    let cursor = |h: &Harness<SketchCadApp>| h.output().platform_output.cursor_icon;

    for (key, tool) in [(Key::O, crate::tools::ToolId::Orbit), (Key::L, crate::tools::ToolId::Line), (Key::P, crate::tools::ToolId::PushPull)] {
        press(&mut h, key);
        assert_eq!(h.state().tool.id(), tool);
        let c = h.state().viewport.rect.center();
        move_to(&mut h, c);
        h.run_steps(2);
        assert_eq!(cursor(&h), egui::CursorIcon::None, "{tool:?} should hide the OS cursor and draw its own");
    }

    // Off the viewport — over the tray on the right — the normal cursor returns.
    let off = { let r = h.state().viewport.rect; pos2(r.max.x + 60.0, r.min.y + 40.0) };
    move_to(&mut h, off);
    h.run_steps(2);
    assert_ne!(cursor(&h), egui::CursorIcon::None, "outside the viewport the OS cursor is visible again");
}

/// Picking a patterned material and painting a face puts a real tiling texture on it: the model
/// gains the image once however many faces use it, and the face gets a placement at tile scale.
#[test]
fn paint_bucket_applies_a_patterned_material() {
    let mut h = harness(LaunchOptions::default());
    {
        // A plain square on the ground, directly in the active context so the bucket can hit it.
        let mut b = sc_geom::MeshBuilder::new();
        let sq = [DVec3::ZERO, DVec3::new(200.0, 0.0, 0.0), DVec3::new(200.0, 200.0, 0.0), DVec3::new(0.0, 200.0, 0.0)];
        b.face(&sq, &[], DVec3::Z, None, None).unwrap();
        h.state_mut().doc.model.mesh = b.finish();
        h.state_mut().pending_zoom_extents = true;
    }
    h.run_steps(5);
    let brick = crate::materials::MATERIALS.iter().find(|m| m.name == "Brick").unwrap();
    h.state_mut().pick_material(brick);
    {
        let app = h.state();
        assert_eq!(app.material, brick.color, "the colour follows the material");
        let (_, tile) = app.material_tex.expect("a patterned material carries a texture");
        assert_eq!(tile, brick.tile);
        assert_eq!(app.doc.model.textures.len(), 1, "the tile is placed in the model");
    }
    // Picking it again must not store a second copy.
    h.state_mut().pick_material(brick);
    assert_eq!(h.state().doc.model.textures.len(), 1, "one copy per material");

    press(&mut h, Key::B);
    let face = screen_of(&h, DVec3::new(100.0, 100.0, 0.0));
    click(&mut h, face);
    h.run_steps(3);

    let painted: Vec<_> = {
        let m = &h.state().doc.model;
        std::iter::once(&m.mesh).chain(m.definitions.values().map(|d| &d.mesh)).flat_map(|x| x.faces.values()).filter_map(|f| f.front_tex.or(f.back_tex)).collect()
    };
    assert!(!painted.is_empty(), "the face was painted with the texture");
    let uv = painted[0].uv;
    // A 32in tile means one repeat every 32 inches, whichever way the face is turned.
    assert!((uv.u.truncate().length() - 1.0 / 32.0).abs() < 1e-9, "u axis is one tile per 32in");
    assert!((uv.v.truncate().length() - 1.0 / 32.0).abs() < 1e-9, "v axis is one tile per 32in");
    assert!(uv.u.truncate().dot(uv.v.truncate()).abs() < 1e-9, "the two axes are perpendicular");

    // A hand-picked colour goes back to being a plain colour.
    h.state_mut().material_tex = None;
    assert!(h.state().material_tex.is_none());
    save(&mut h, "material_bucket");
}
