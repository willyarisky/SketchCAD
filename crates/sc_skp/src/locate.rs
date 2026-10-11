//! Finding the SketchUp C API library and the matching converter, and running conversions.

use crate::SkpModel;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A SketchUp C API library found on this machine.
#[derive(Clone, Debug)]
pub struct Sdk {
    /// The library file itself (`.../SketchUpAPI.framework/Versions/A/SketchUpAPI`, `SketchUpAPI.dll`).
    pub library: PathBuf,
    /// Folders its own dependencies are found in.
    pub search_dirs: Vec<PathBuf>,
    /// Where it came from, for messages ("SketchUp 2017", "SketchUp SDK").
    pub label: String,
    pub arch: Vec<Arch>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    Arm64,
    Other,
}

impl Arch {
    pub fn host() -> Arch {
        if cfg!(target_arch = "x86_64") {
            Arch::X86_64
        } else if cfg!(target_arch = "aarch64") {
            Arch::Arm64
        } else {
            Arch::Other
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Arm64 => "arm64",
            Arch::Other => "other",
        }
    }

    fn triple(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64-apple-darwin",
            Arch::Arm64 => "aarch64-apple-darwin",
            Arch::Other => "",
        }
    }
}

/// CPU architectures in a Mach-O file (thin or universal). Empty if it isn't one.
pub fn macho_archs(path: &Path) -> Vec<Arch> {
    use std::io::Read;
    let mut buf = vec![0u8; 4096];
    let Ok(n) = std::fs::File::open(path).and_then(|mut f| f.read(&mut buf)) else { return Vec::new() };
    let buf = &buf[..n];
    let be = |o: usize| buf.get(o..o + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    let le = |o: usize| buf.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let cpu = |c: u32| match c {
        0x0100_0007 => Arch::X86_64,
        0x0100_000c => Arch::Arm64,
        _ => Arch::Other,
    };
    match be(0) {
        Some(0xcafe_babe) => {
            let count = be(4).unwrap_or(0).min(16) as usize;
            (0..count).filter_map(|i| be(8 + i * 20).map(cpu)).collect()
        }
        Some(0xcffa_edfe) => le(4).map(cpu).into_iter().collect(),
        _ => Vec::new(),
    }
}

fn sdk_from(path: &Path, label: &str) -> Option<Sdk> {
    // Accept the framework folder, the folder containing it, or the library file itself.
    let candidates = [
        path.join("Versions/A/SketchUpAPI"),
        path.join("SketchUpAPI.framework/Versions/A/SketchUpAPI"),
        path.join("SketchUpAPI.dll"),
        path.to_path_buf(),
    ];
    let library = candidates.into_iter().find(|p| p.is_file())?;
    let lib_dir = library.parent()?.to_path_buf();
    let mut search_dirs = vec![lib_dir.join("Libraries"), lib_dir.clone()];
    // Inside an app bundle the dependencies sit next to the framework (Contents/Frameworks).
    if let Some(fw_parent) = library.ancestors().find(|a| a.extension().is_some_and(|e| e == "framework")).and_then(Path::parent) {
        search_dirs.push(fw_parent.to_path_buf());
    }
    let arch = if cfg!(target_os = "macos") { macho_archs(&library) } else { vec![Arch::host()] };
    Some(Sdk { library, search_dirs, label: label.to_string(), arch })
}

/// Look for the SketchUp C API: `$SKETCHCAD_SKP_SDK`, an unpacked SDK in the usual places, then
/// SketchUp installations (newest first).
pub fn find_sdk() -> Option<Sdk> {
    if let Some(p) = std::env::var_os("SKETCHCAD_SKP_SDK") {
        return sdk_from(Path::new(&p), "SKETCHCAD_SKP_SDK");
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let mut roots: Vec<(PathBuf, String)> = Vec::new();
    for dir in [home.join("SketchUpSDK"), home.join("Developer/SketchUpSDK"), home.join("Downloads")] {
        if let Ok(rd) = std::fs::read_dir(&dir) {
            let mut subs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
            subs.sort();
            subs.reverse();
            roots.extend(subs.into_iter().filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().contains("sdk"))).map(|p| (p, "SketchUp SDK".to_string())));
        }
        roots.push((dir, "SketchUp SDK".into()));
    }
    if cfg!(target_os = "macos")
        && let Ok(rd) = std::fs::read_dir("/Applications") {
            let mut apps: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("SketchUp 20"))).collect();
            apps.sort();
            apps.reverse();
            for a in apps {
                let label = a.file_name().unwrap().to_string_lossy().into_owned();
                roots.push((a.join("SketchUp.app/Contents/Frameworks/SketchUpAPI.framework"), label));
            }
        }
    if cfg!(windows)
        && let Ok(rd) = std::fs::read_dir("C:\\Program Files\\SketchUp") {
            let mut apps: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
            apps.sort();
            apps.reverse();
            roots.extend(apps.into_iter().map(|a| (a.clone(), a.file_name().unwrap().to_string_lossy().into_owned())));
        }
    roots.into_iter().find_map(|(p, label)| sdk_from(&p, &label))
}

/// The converter build that can load `sdk` (same CPU architecture), looked for next to the
/// running executable and in the cargo target folders during development.
pub fn find_converter(sdk: &Sdk) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("no executable folder")?.to_path_buf();
    let name = if cfg!(windows) { "sketchcad-skp.exe" } else { "sketchcad-skp" };
    let host = Arch::host();
    let mut wanted: Vec<Arch> = Vec::new();
    if sdk.arch.contains(&host) {
        wanted.push(host);
    }
    wanted.extend(sdk.arch.iter().copied().filter(|a| *a != host && *a != Arch::Other));
    if wanted.is_empty() {
        return Err(format!("The SketchUp library at {} is not for this computer's processor.", sdk.library.display()));
    }
    // target/<profile>/ (also target/<profile>/deps for tests) → target/<triple>/<profile>/.
    let target_dir = dir.ancestors().find(|a| a.file_name().is_some_and(|n| n == "target")).map(Path::to_path_buf);
    for arch in &wanted {
        let mut c = Vec::new();
        c.push(dir.join(format!("sketchcad-skp-{}", arch.suffix())));
        if *arch == host {
            c.push(dir.join(name));
            c.push(dir.parent().map(|p| p.join(name)).unwrap_or_default());
        }
        if let Some(t) = &target_dir {
            for profile in ["release", "debug"] {
                c.push(t.join(arch.triple()).join(profile).join(name));
                if *arch == host {
                    c.push(t.join(profile).join(name));
                }
            }
        }
        if let Some(p) = c.into_iter().find(|p| p.is_file()) {
            return Ok(p);
        }
    }
    let arch = wanted[0];
    Err(format!(
        "The SketchUp file converter for {} was not found. Build it with:\n  cargo build --release -p sc_skp --target {}",
        arch.suffix(),
        arch.triple()
    ))
}

fn run(sdk: &Sdk, args: &[&std::ffi::OsStr]) -> Result<(), String> {
    // The SketchUp library occasionally fails to start when several copies launch at once
    // (seen under Rosetta); one retry covers that without hiding real errors.
    run_once(sdk, args).or_else(|_| {
        std::thread::sleep(std::time::Duration::from_millis(250));
        run_once(sdk, args)
    })
}

fn run_once(sdk: &Sdk, args: &[&std::ffi::OsStr]) -> Result<(), String> {
    let conv = find_converter(sdk)?;
    let mut cmd = Command::new(&conv);
    cmd.arg("--lib").arg(&sdk.library).args(args);
    if cfg!(target_os = "macos") {
        let dirs = std::env::join_paths(&sdk.search_dirs).map_err(|e| e.to_string())?;
        cmd.env("DYLD_LIBRARY_PATH", &dirs).env("DYLD_FRAMEWORK_PATH", &dirs);
    } else if cfg!(windows) {
        let mut path = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect::<Vec<_>>();
        path.splice(0..0, sdk.search_dirs.iter().cloned());
        cmd.env("PATH", std::env::join_paths(path).map_err(|e| e.to_string())?);
    }
    let out = cmd.output().map_err(|e| format!("Could not run {}: {e}", conv.display()))?;
    if out.status.success() {
        return Ok(());
    }
    let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(if msg.is_empty() { format!("The SketchUp file converter failed ({})", out.status) } else { msg })
}

fn temp_json(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    std::env::temp_dir().join(format!("sketchcad-{tag}-{}-{}.json", std::process::id(), N.fetch_add(1, Ordering::Relaxed)))
}

/// Read a `.skp` / `.skb` file.
pub fn read(sdk: &Sdk, path: &Path) -> Result<SkpModel, String> {
    let json = temp_json("read");
    let r = run(sdk, &["read".as_ref(), path.as_os_str(), json.as_os_str()]).and_then(|_| {
        let bytes = std::fs::read(&json).map_err(|e| e.to_string())?;
        serde_json::from_slice(&bytes).map_err(|e| format!("Bad converter output: {e}"))
    });
    let _ = std::fs::remove_file(&json);
    r
}

/// Write a `.skp` / `.skb` file (in the newest format the library supports).
pub fn write(sdk: &Sdk, model: &SkpModel, path: &Path) -> Result<(), String> {
    let json = temp_json("write");
    std::fs::write(&json, serde_json::to_vec(model).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let r = run(sdk, &["write".as_ref(), json.as_os_str(), path.as_os_str()]);
    let _ = std::fs::remove_file(&json);
    r
}

/// "SketchUp 2017" for the library `sdk` (asks the library; falls back to the label).
pub fn library_version(sdk: &Sdk) -> String {
    let json = temp_json("version");
    let v = run(sdk, &["version".as_ref(), json.as_os_str()]).ok().and_then(|_| std::fs::read_to_string(&json).ok());
    let _ = std::fs::remove_file(&json);
    v.map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).unwrap_or_else(|| sdk.label.clone())
}
