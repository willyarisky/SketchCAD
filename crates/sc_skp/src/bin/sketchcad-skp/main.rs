//! `sketchcad-skp`: converts between SketchUp `.skp`/`.skb` files and SketchCAD's JSON description
//! ([`sc_skp::SkpModel`]) using Trimble's SketchUp C API library, loaded at run time.
//!
//! ```text
//! sketchcad-skp --lib <SketchUpAPI library> read <in.skp> <out.json>
//! sketchcad-skp --lib <SketchUpAPI library> write <in.json> <out.skp>
//! sketchcad-skp --lib <SketchUpAPI library> version <out.txt>
//! ```
//!
//! SketchCAD runs this; it sets the library search path so the SketchUp library finds its own
//! dependencies. Errors go to stderr with a non-zero exit code.

mod read;
mod sdk;
mod write;

use sdk::{Api, Ref, SU_ERROR_NONE};
use std::path::{Path, PathBuf};

fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let lib = match args.iter().position(|a| a == "--lib") {
        Some(i) if i + 1 < args.len() => {
            let l = PathBuf::from(args.remove(i + 1));
            args.remove(i);
            l
        }
        _ => return Err("usage: sketchcad-skp --lib <SketchUpAPI> (read <in.skp> <out.json> | write <in.json> <out.skp> | version <out.txt>)".into()),
    };
    // SAFETY: the caller points us at a SketchUp C API library.
    let api = unsafe { Api::load(&lib)? };
    unsafe { (api.SUInitialize)() };
    let result = command(&api, &args);
    unsafe { (api.SUTerminate)() };
    result
}

fn library_version(api: &Api) -> String {
    unsafe {
        let mut m = Ref::NULL;
        if (api.SUModelCreate)(&mut m) != SU_ERROR_NONE {
            return String::new();
        }
        let (mut a, mut b, mut c) = (0, 0, 0);
        let v = if (api.SUModelGetVersion)(m, &mut a, &mut b, &mut c) == SU_ERROR_NONE { format!("{a}.{b}.{c}") } else { String::new() };
        (api.SUModelRelease)(&mut m);
        v
    }
}

fn command(api: &Api, args: &[String]) -> Result<(), String> {
    match args {
        [cmd, input, output] if cmd == "read" => {
            let model = read::read(api, input.as_ref(), output.as_ref(), &library_version(api))?;
            let json = serde_json::to_vec(&model).map_err(|e| e.to_string())?;
            std::fs::write(output, json).map_err(|e| format!("Could not write {output}: {e}"))
        }
        [cmd, input, output] if cmd == "write" => {
            let bytes = std::fs::read(input).map_err(|e| format!("Could not read {input}: {e}"))?;
            let model: sc_skp::SkpModel = serde_json::from_slice(&bytes).map_err(|e| format!("Bad model description: {e}"))?;
            // Texture images sit beside the JSON we were handed.
            let dir = Path::new(input).parent().unwrap_or(Path::new("."));
            write::write(api, &model, dir, output.as_ref())
        }
        [cmd, output] if cmd == "version" => {
            let v = library_version(api);
            std::fs::write(output, sc_skp::version_name(&v)).map_err(|e| e.to_string())
        }
        _ => Err(format!("unknown command: {}", args.join(" "))),
    }
}
