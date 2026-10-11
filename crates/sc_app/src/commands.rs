//! Application commands and the keyboard shortcut map (SketchUp defaults).

use crate::tools::ToolId;
use egui::{Key, KeyboardShortcut, Modifiers};
use sc_render::{FaceStyle, StandardView};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    Tool(ToolId),
    New,
    Open,
    Save,
    SaveAs,
    Quit,
    Undo,
    Redo,
    Delete,
    SelectAll,
    SelectNone,
    Hide,
    UnhideAll,
    ReverseFaces,
    MakeGroup,
    MakeComponent,
    Explode,
    EditComponent,
    CloseComponent,
    MakeUnique,
    AddScene,
    NextScene,
    PreviousScene,
    DeleteGuides,
    ZoomExtents,
    StandardView(StandardView),
    FaceStyle(FaceStyle),
    ToggleBackEdges,
    ToggleXRay,
    ToggleAxes,
    ToggleGuides,
    ModelInfo,
    Preferences,
    Shortcuts,
    About,
}

impl Command {
    pub fn label(self) -> String {
        match self {
            Command::Tool(t) => t.name().to_string(),
            Command::New => "New".into(),
            Command::Open => "Open...".into(),
            Command::Save => "Save".into(),
            Command::SaveAs => "Save As...".into(),
            Command::Quit => "Exit".into(),
            Command::Undo => "Undo".into(),
            Command::Redo => "Redo".into(),
            Command::Delete => "Delete".into(),
            Command::SelectAll => "Select All".into(),
            Command::SelectNone => "Select None".into(),
            Command::Hide => "Hide".into(),
            Command::UnhideAll => "Unhide All".into(),
            Command::ReverseFaces => "Reverse Faces".into(),
            Command::MakeGroup => "Make Group".into(),
            Command::MakeComponent => "Make Component...".into(),
            Command::Explode => "Explode".into(),
            Command::EditComponent => "Edit Group/Component".into(),
            Command::CloseComponent => "Close Group/Component".into(),
            Command::MakeUnique => "Make Unique".into(),
            Command::AddScene => "Add Scene".into(),
            Command::NextScene => "Next Scene".into(),
            Command::PreviousScene => "Previous Scene".into(),
            Command::DeleteGuides => "Delete Guides".into(),
            Command::ZoomExtents => "Zoom Extents".into(),
            Command::StandardView(v) => format!("{v:?}"),
            Command::FaceStyle(s) => match s {
                FaceStyle::XRay => "X-ray".into(),
                FaceStyle::Wireframe => "Wireframe".into(),
                FaceStyle::HiddenLine => "Hidden Line".into(),
                FaceStyle::Shaded => "Shaded".into(),
                FaceStyle::ShadedWithTextures => "Shaded With Textures".into(),
                FaceStyle::Monochrome => "Monochrome".into(),
            },
            Command::ToggleBackEdges => "Back Edges".into(),
            Command::ToggleXRay => "X-ray".into(),
            Command::ToggleAxes => "Axes".into(),
            Command::ToggleGuides => "Guides".into(),
            Command::ModelInfo => "Model Info".into(),
            Command::Preferences => "Preferences".into(),
            Command::Shortcuts => "Keyboard Shortcuts".into(),
            Command::About => "About SketchCAD".into(),
        }
    }
}

pub struct Keymap {
    pub bindings: Vec<(KeyboardShortcut, Command)>,
}

impl Default for Keymap {
    fn default() -> Self {
        let none = Modifiers::NONE;
        let cmd = Modifiers::COMMAND; // Ctrl on Windows/Linux, ⌘ on macOS
        let shift = Modifiers::SHIFT;
        let cmd_shift = Modifiers { shift: true, ..Modifiers::COMMAND };
        let k = |m: Modifiers, key: Key, c: Command| (KeyboardShortcut::new(m, key), c);
        let t = Command::Tool;
        let mut bindings = vec![
            k(none, Key::Space, t(ToolId::Select)),
            k(none, Key::E, t(ToolId::Eraser)),
            k(none, Key::B, t(ToolId::PaintBucket)),
            k(none, Key::L, t(ToolId::Line)),
            k(none, Key::A, t(ToolId::Arc)),
            k(none, Key::R, t(ToolId::Rectangle)),
            k(none, Key::C, t(ToolId::Circle)),
            k(none, Key::P, t(ToolId::PushPull)),
            k(none, Key::M, t(ToolId::Move)),
            k(none, Key::Q, t(ToolId::Rotate)),
            k(none, Key::S, t(ToolId::Scale)),
            k(none, Key::F, t(ToolId::Offset)),
            k(none, Key::T, t(ToolId::TapeMeasure)),
            k(none, Key::O, t(ToolId::Orbit)),
            k(none, Key::H, t(ToolId::Pan)),
            k(none, Key::Z, t(ToolId::Zoom)),
            k(shift, Key::Z, Command::ZoomExtents),
            k(none, Key::K, Command::ToggleBackEdges),
            k(none, Key::G, Command::MakeComponent),
            k(none, Key::PageDown, Command::NextScene),
            k(none, Key::PageUp, Command::PreviousScene),
            k(cmd, Key::N, Command::New),
            k(cmd, Key::O, Command::Open),
            k(cmd, Key::S, Command::Save),
            k(cmd_shift, Key::S, Command::SaveAs),
            k(cmd, Key::Z, Command::Undo),
            k(cmd, Key::Y, Command::Redo),
            k(cmd_shift, Key::Z, Command::Redo),
            k(cmd, Key::A, Command::SelectAll),
            k(cmd, Key::T, Command::SelectNone),
            k(none, Key::Delete, Command::Delete),
            k(cmd, Key::Q, Command::Quit),
        ];
        if cfg!(target_os = "macos") {
            bindings.push(k(none, Key::Backspace, Command::Delete));
            bindings.push(k(cmd, Key::Comma, Command::Preferences));
        }
        Self { bindings }
    }
}

impl Keymap {
    pub fn shortcut_for(&self, c: Command) -> Option<KeyboardShortcut> {
        self.bindings.iter().find(|(_, cmd)| *cmd == c).map(|(s, _)| *s)
    }

    /// Consume the first matching shortcut from this frame's input.
    pub fn poll(&self, ctx: &egui::Context) -> Option<Command> {
        // Longer modifier combinations first so Shift+Z doesn't trigger Z.
        let mut sorted: Vec<&(KeyboardShortcut, Command)> = self.bindings.iter().collect();
        sorted.sort_by_key(|(s, _)| std::cmp::Reverse(s.modifiers.ctrl as u8 + s.modifiers.command as u8 + s.modifiers.shift as u8 + s.modifiers.alt as u8));
        ctx.input_mut(|i| sorted.into_iter().find(|(s, _)| i.consume_shortcut(s)).map(|(_, c)| *c))
    }
}
