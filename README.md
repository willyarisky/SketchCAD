# SketchCAD

A SketchUp-style 3D modeler written in Rust: sticky edges and faces, push/pull, inference
snapping, the Measurements box, and SketchUp's layout and keyboard shortcuts.

> SketchCAD mirrors SketchUp's *workflow* — layout, tool behavior, shortcuts. Its icons, name and
> file format are its own; it ships no Trimble artwork or assets.

## Run

```sh
cargo run --release -p sc_app                  # empty model
cargo run --release -p sc_app -- --demo        # sample house
cargo run --release -p sc_app -- model.skcad   # open a file
```

Other flags: `--tool "Push/Pull"` (start with a tool), `--screenshot out.png` (save the window and quit),
`--no-extensions` (don't start Ruby).

## Release builds

`scripts/build-release.sh` builds every platform from a Mac into `dist/` (setup steps are at the top
of the script):

| Package | Contents | Runs on |
|---|---|---|
| `SketchCAD-<v>-macos-universal.zip` | `SketchCAD.app` (Apple Silicon + Intel), ad-hoc signed | macOS 11+ |
| `SketchCAD-<v>-windows-x86_64.zip` | `sketchcad.exe`, `sketchcad-skp.exe` | Windows 10+ (64-bit) |
| `SketchCAD-<v>-linux-x86_64.tar.gz` / `-linux-aarch64.tar.gz` | `sketchcad` | glibc 2.31+ (Ubuntu 20.04, Debian 11), X11 or Wayland, Vulkan or OpenGL |

The macOS app is not notarized: the first time, right-click it and choose **Open**. SketchUp file
support needs Trimble's SketchUp library on that machine (not available for Linux); extensions need Ruby.

## Test

```sh
cargo test
```

`sc_app`'s UI tests run the real app headlessly (wgpu + egui_kittest), drive tools with mouse and
keyboard events, and write screenshots to `target/screenshots/`.

## Workspace

| Crate | Purpose |
|---|---|
| `sc_math` | f64 geometry (inches internally, 0.001" tolerance), planes, rays, 2D polygons, triangulation, unit parsing/formatting |
| `sc_geom` | Geometry kernel: auto-splitting edges, automatic face creation via planar face tracing, holes, healing on erase, push/pull (stretch / extrude / carve), copy, picking |
| `sc_render` | wgpu viewport: sky/ground, shaded faces with front/back colors, screen-space thick edges, dashed lines, X-ray, back edges, reversed-Z depth, 4× MSAA |
| `sc_skp` | SketchUp `.skp`/`.skb` files: the `sketchcad-skp` converter (Trimble's SketchUp C API, loaded at run time) and the model description it exchanges with SketchCAD |
| `sc_app` | eframe/egui application: inference engine, tools, menus, toolbars, tray, Measurements box, `.skcad` files, undo/redo, SketchUp extensions |

## SketchUp files (.skp / .skb)

**File > Open** reads SketchUp models (`.skp`) and SketchUp backups (`.skb`, the same format).
**File > Save** writes back to the `.skp` you opened, and **Save As** offers SketchUp Model (`.skp`). Saving
over an existing `.skp` first keeps the previous file as a `.skb` backup next to it, as SketchUp does.

`.skp` is Trimble's proprietary format, so SketchCAD uses Trimble's own SketchUp C API library to read
and write it. It looks for, in order: `$SKETCHCAD_SKP_SDK`, an unpacked SketchUp SDK in `~/SketchUpSDK`
(or `~/Downloads/*SDK*`), then a SketchUp installation in `/Applications`. The library runs inside a
small converter, `sketchcad-skp`, which must match the library's processor type:

```sh
cargo build --release                                            # native converter (SDK 2021+)
rustup target add x86_64-apple-darwin                            # once
cargo build --release -p sc_skp --target x86_64-apple-darwin     # Intel-only libraries (SketchUp ≤ 2020), runs under Rosetta
```

What carries across: faces (with holes) and edges with soft/smooth/hidden, colors (front and back,
opacity), groups, components (shared definitions, names, descriptions), nesting, instance names,
hidden instances, tags and their visibility, units and the camera. Not yet: textures (shown as their
base color), scenes, styles, text, dimensions, images, section planes and guides — opening a file
lists anything it skipped.

The library's version limits what it can do: SketchUp 2017's library opens files up to SketchUp 2017
and saves in 2017 format; the current SketchUp SDK (free from Trimble's developer site) opens and saves
every version. If you distribute SketchCAD with `.skp` support, use the SDK under its license terms.

## SketchUp extensions

SketchCAD runs SketchUp Ruby extensions (`.rbz`). **Extensions > Extension Manager > Install Extension...**
installs one into the Plugins folder (`~/Library/Application Support/SketchCAD/Plugins` on macOS,
`%APPDATA%\SketchCAD\Plugins` on Windows, `~/.local/share/SketchCAD/Plugins` on Linux); every
top-level `.rb` there loads at start-up, as in SketchUp. Extensions can be turned off or uninstalled
from the same window, and **Extensions > Ruby Console** evaluates Ruby against the open model.

Try it with the example: `examples/extensions/box_maker.rbz` (source in `examples/extensions/box_maker/`).

How it works: extensions run in a separate Ruby process (so a broken extension can't crash the
modeler) against SketchCAD's implementation of the SketchUp Ruby API
(`crates/sc_app/src/plugins/ruby/`). Each `start_operation`/`commit_operation` is one undo step.
Ruby is found at `$SKETCHCAD_RUBY`, Homebrew's Ruby, rbenv, `/usr/bin/ruby` or `$PATH` (2.6+).

Supported: `Sketchup` (active_model, register_extension, require, status_text, send_action, read/write_default),
`SketchupExtension`, `Sketchup::Model` (entities, active_entities, selection, definitions, materials,
layers/tags, active_view, operations), `Entities` (add_line/edges/curve/face/circle/arc/ngon/group/instance,
add_cpoint/cline, add_faces_from_mesh, build, erase_entities, transform_entities, clear!), `Edge`, `Face`
(pushpull, followme, reverse!, material), `Vertex`, `Loop`, `Group`, `ComponentInstance`, `ComponentDefinition`,
`Selection`, `View`/`Camera`, `Geom` (Point3d, Vector3d, Transformation, BoundingBox, PolygonMesh, intersections),
`Length`/unit conversions, `UI` (menus and submenus, `UI::Command`, `UI::Toolbar` with PNG icons, messagebox,
inputbox, open/save panels, timers).

Not yet: encrypted `.rbe`/`.rbs` extensions and compiled (`.so`/`.bundle`) ones — those only run inside
SketchUp — plus `UI::HtmlDialog`/`WebDialog`, custom Ruby tools (`select_tool`), observers (accepted but
never called), textures, text/dimensions, and saving attribute dictionaries in `.skcad` files.

## Status against the plan

**Done**
- Phase 0: workspace, window, renderer, camera (orbit/pan/zoom-to-cursor, standard views, parallel/perspective)
- Kernel: lines, auto faces, splitting, holes, erase + heal, collinear merge, push/pull (stretch / extrude / carve, Ctrl new starting face), offset, Follow Me (mitered sweeps, lathes, holes), move/rotate/scale/mirror, copies and arrays (`3x`, `/3`) that keep materials
- Groups & components: definitions shared by instances, nested editing with the rest of the model faded, Make Group, Make Component (G), Explode, Make Unique (automatic for groups), instance names, hide, lock
- Tags (visibility), Outliner (tree, click to select, double-click to open), Components tray (place copies), Scenes (camera + style + tags, tabs, animated transitions, Page Up/Down)
- Inference: endpoint, midpoint, origin, on edge, on face, on axis, from-point alignment, guide points/lines, Shift lock, arrow-key axis locks — across groups and components too
- Tools: Select (click / Ctrl / Shift / double / triple click, window + crossing, groups), Eraser, Paint Bucket, Line, 2 Point Arc, Rectangle, Circle, Polygon, Push/Pull, Move, Rotate, Scale (grips, factor or length), Follow Me, Offset, Tape Measure, Orbit, Pan, Zoom, Zoom Extents
- Measurements box: type anywhere, units (`5'6"`, `300mm`, `1.2m`), `w,h`, `[x,y,z]`, `<x,y,z>`, `24s`, retype after drawing
- UI: menu bar, toolbars, Large Tool Set, Default Tray (Entity Info, Materials, Components, Styles, Tags, Scenes, Outliner, Instructor), right-click context menu, status bar, Model Info, shortcuts list, save-changes prompt
- Rendering: X-ray, Wireframe, Hidden Line, Shaded, Monochrome, Back Edges, smooth shading across smoothed edges, selection boxes for groups
- Files: `.skcad` v2 (definitions, tags, scenes; v1 files still open), undo/redo per editing context
- SketchUp files: open `.skp`/`.skb`, save `.skp` (with `.skb` backup) through Trimble's SketchUp C API
- Extensions: Extension Manager (install `.rbz`/`.rb`, enable/disable, uninstall), Ruby Console, SketchUp Ruby API subset, extension menus/toolbars/dialogs

**Next**
- Freehand, Rotated Rectangle, 3 Point Arc, Pie, Dimensions, Text, Protractor, Axes, 3D Text, Section Plane
- Intersect Faces, Solid Tools, Sandbox
- Textures, shadows, fog, sketchy edges, profiles
- Inference: parallel/perpendicular, tangent, intersection; spatial index (BVH) for large models
- Diff-based undo journal, autosave/recovery, OBJ/STL/glTF/DAE/DXF; `.skp` textures, scenes, styles, text and dimensions
- Extensions: HtmlDialog, custom Ruby tools, observers, attribute dictionaries in files; editable shortcuts, Preferences

## Shortcuts (SketchUp defaults)

Space Select · E Eraser · B Paint Bucket · L Line · A Arc · R Rectangle · C Circle · P Push/Pull ·
M Move · Q Rotate · S Scale · F Offset · T Tape Measure · O Orbit · H Pan · Z Zoom · Shift+Z Zoom Extents ·
K Back Edges · G Make Component · PgUp/PgDn Scenes · Ctrl/⌘+Z Undo · Ctrl+Y / ⌘⇧Z Redo · Ctrl/⌘+A Select All · Ctrl/⌘+T Select None ·
Delete Erase · Middle-drag Orbit · Shift+middle-drag Pan · Wheel Zoom · ↑ → ← lock Blue/Red/Green ·
Shift lock inference · Ctrl (⌥ on macOS) copy / new-face toggle · Esc cancel
