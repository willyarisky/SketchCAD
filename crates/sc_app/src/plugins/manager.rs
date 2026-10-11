//! Extension files on disk: where they live, installing `.rbz` / `.rb` files, uninstalling and
//! the enabled/disabled settings — SketchUp's Extension Manager, minus the Extension Warehouse.

use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

/// SketchCAD's per-user folders.
#[derive(Clone, Debug)]
pub struct Dirs {
    /// Root of SketchCAD's data (e.g. `~/Library/Application Support/SketchCAD`).
    pub data: PathBuf,
    /// Installed extensions; every top-level `.rb` here loads at start-up, as in SketchUp.
    pub plugins: PathBuf,
    /// SketchCAD's own Ruby API implementation, rewritten at every start.
    pub ruby_api: PathBuf,
    /// Enabled/disabled extensions.
    pub settings: PathBuf,
}

impl Dirs {
    pub fn new(data: PathBuf) -> Self {
        Self { plugins: data.join("Plugins"), ruby_api: data.join("RubyAPI"), settings: data.join("extensions.json"), data }
    }

    /// The platform's per-user application data folder (`$SKETCHCAD_DATA_DIR` overrides it).
    pub fn user() -> Self {
        if let Some(d) = std::env::var_os("SKETCHCAD_DATA_DIR") {
            return Self::new(PathBuf::from(d));
        }
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default();
        let base = if cfg!(target_os = "macos") {
            home.join("Library/Application Support")
        } else if cfg!(windows) {
            std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| home.join("AppData/Roaming"))
        } else {
            std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share"))
        };
        Self::new(base.join("SketchCAD"))
    }

    pub fn ensure(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.plugins)?;
        std::fs::create_dir_all(&self.ruby_api)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Settings {
    /// Names of extensions the user turned off.
    #[serde(default)]
    pub disabled: Vec<String>,
}

impl Settings {
    pub fn load(dirs: &Dirs) -> Self {
        std::fs::read(&dirs.settings).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, dirs: &Dirs) -> std::io::Result<()> {
        std::fs::write(&dirs.settings, serde_json::to_vec_pretty(self)?)
    }
}

/// Install an extension package into `dirs.plugins`.
///
/// `.rbz` (and `.zip`) archives are SketchUp's format: a loader `.rb` at the top level next to the
/// extension's support folder. A bare `.rb` file is copied as is. Returns the top-level `.rb`
/// files to load now.
pub fn install(dirs: &Dirs, package: &Path) -> Result<Vec<PathBuf>, String> {
    dirs.ensure().map_err(|e| e.to_string())?;
    let ext = package.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    match ext.as_str() {
        "rb" => {
            let name = package.file_name().ok_or("not a file")?;
            let dest = dirs.plugins.join(name);
            std::fs::copy(package, &dest).map_err(|e| format!("Could not copy {}: {e}", package.display()))?;
            Ok(vec![dest])
        }
        "rbz" | "zip" => install_archive(dirs, package),
        "rbe" | "rbs" => Err("Encrypted extension files only run inside SketchUp.".into()),
        _ => Err("Choose a SketchUp extension (.rbz) or Ruby script (.rb).".into()),
    }
}

fn install_archive(dirs: &Dirs, package: &Path) -> Result<Vec<PathBuf>, String> {
    let file = std::fs::File::open(package).map_err(|e| format!("Could not open {}: {e}", package.display()))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("Not a valid .rbz archive: {e}"))?;

    // Validate everything before writing anything.
    let mut entries = Vec::new();
    let mut loaders = Vec::new();
    for i in 0..zip.len() {
        let f = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = f.enclosed_name() else { return Err(format!("Unsafe path in archive: {}", f.name())) };
        if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
            return Err(format!("Unsafe path in archive: {}", f.name()));
        }
        if rel.components().count() == 1 && rel.extension().is_some_and(|e| e.eq_ignore_ascii_case("rb")) {
            loaders.push(dirs.plugins.join(&rel));
        }
        entries.push((i, rel, f.is_dir()));
    }
    if loaders.is_empty() {
        return Err("This archive has no loader .rb file at its top level, so it is not a SketchUp extension.".into());
    }

    for (i, rel, is_dir) in entries {
        let dest = dirs.plugins.join(&rel);
        if is_dir {
            std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut f = zip.by_index(i).map_err(|e| e.to_string())?;
        let mut out = std::fs::File::create(&dest).map_err(|e| format!("Could not write {}: {e}", dest.display()))?;
        std::io::copy(&mut f, &mut out).map_err(|e| format!("Could not extract {}: {e}", rel.display()))?;
    }
    loaders.sort();
    Ok(loaders)
}

/// Delete an extension's files: its loader `.rb` and the support folder its `path` points into.
/// Only paths inside the Plugins folder are ever removed.
pub fn uninstall(dirs: &Dirs, loader: Option<&Path>, ext_path: &str) -> Result<Vec<PathBuf>, String> {
    let plugins = dirs.plugins.canonicalize().map_err(|e| e.to_string())?;
    let mut targets = Vec::new();
    if let Some(l) = loader {
        targets.push(l.to_path_buf());
    }
    // The support folder: the first component of the extension path below Plugins.
    let p = Path::new(ext_path);
    let abs = if p.is_absolute() { p.to_path_buf() } else { dirs.plugins.join(p) };
    let abs = abs.parent().and_then(|d| d.canonicalize().ok());
    if let Some(rel) = abs.as_deref().and_then(|a| a.strip_prefix(&plugins).ok())
        && let Some(Component::Normal(first)) = rel.components().next() {
            targets.push(plugins.join(first));
        }

    let mut removed = Vec::new();
    for t in targets {
        let Ok(t) = t.canonicalize() else { continue };
        if !t.starts_with(&plugins) || t == plugins {
            continue;
        }
        let r = if t.is_dir() { std::fs::remove_dir_all(&t) } else { std::fs::remove_file(&t) };
        r.map_err(|e| format!("Could not remove {}: {e}", t.display()))?;
        removed.push(t);
    }
    if removed.is_empty() {
        return Err("Could not find this extension's files in the Plugins folder.".into());
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_dirs(name: &str) -> Dirs {
        let d = std::env::temp_dir().join(format!("skcad-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let dirs = Dirs::new(d);
        dirs.ensure().unwrap();
        dirs
    }

    fn rbz(path: &Path, files: &[(&str, &str)]) {
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, body) in files {
            z.start_file(*name, zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)).unwrap();
            z.write_all(body.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }

    #[test]
    fn install_and_uninstall_rbz() {
        let dirs = temp_dirs("install");
        let pkg = dirs.data.join("demo.rbz");
        rbz(&pkg, &[("demo_ext.rb", "# loader"), ("demo_ext/main.rb", "# main")]);
        let loaders = install(&dirs, &pkg).unwrap();
        assert_eq!(loaders, vec![dirs.plugins.join("demo_ext.rb")]);
        assert!(dirs.plugins.join("demo_ext/main.rb").is_file());

        let removed = uninstall(&dirs, Some(&dirs.plugins.join("demo_ext.rb")), "demo_ext/main").unwrap();
        assert_eq!(removed.len(), 2);
        assert!(!dirs.plugins.join("demo_ext").exists() && !dirs.plugins.join("demo_ext.rb").exists());
        assert!(dirs.plugins.is_dir(), "the Plugins folder itself stays");
        std::fs::remove_dir_all(&dirs.data).ok();
    }

    #[test]
    fn rejects_archives_without_loader_or_with_unsafe_paths() {
        let dirs = temp_dirs("reject");
        let pkg = dirs.data.join("bad.rbz");
        rbz(&pkg, &[("folder/main.rb", "")]);
        assert!(install(&dirs, &pkg).unwrap_err().contains("loader"));
        rbz(&pkg, &[("ok.rb", ""), ("../escape.rb", "")]);
        assert!(install(&dirs, &pkg).unwrap_err().contains("Unsafe"));
        assert!(!dirs.data.join("escape.rb").exists());
        std::fs::remove_dir_all(&dirs.data).ok();
    }
}
