//! Looks inside a `.skp` / `.skb` without Trimble's library.
//!
//! This is the bench for decoding `model.dat`. The framing is known (see [`sc_skp::native::tlv`])
//! but the keys are bare numbers, so the way forward is comparison: save two models that differ
//! by one known thing, and see which keys move.
//!
//! ```text
//! skp-inspect summary <file>            what the container holds
//! skp-inspect tree    <file> [depth]    the record tree
//! skp-inspect keys    <file>            every key and how often it occurs
//! skp-inspect values  <file> <key>      what records with that key actually hold
//! skp-inspect diff    <a> <b>           which keys differ, and by how many
//! ```

use sc_skp::native::{self, tlv};
use std::collections::BTreeMap;

fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn load(path: &str) -> Result<native::NativeSkp, String> {
    native::read(std::path::Path::new(path))
}

/// The geometry stream, or an explanation of why there isn't one.
fn model_dat(f: &native::NativeSkp) -> Result<&[u8], String> {
    f.model_dat.as_deref().ok_or_else(|| format!("{} keeps its geometry in the older MFC container, which has no record tree to show", f.version_name()))
}

fn census(bytes: &[u8]) -> Result<BTreeMap<u16, usize>, String> {
    let root = tlv::root(bytes).ok_or("model.dat is not a single well-formed record")?;
    Ok(tlv::key_census(&root))
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["summary", path] => {
            let f = load(path)?;
            println!("version    {} ({})", f.version_name(), f.version);
            println!("container  {:?}", native::format(&std::fs::read(path).map_err(|e| e.to_string())?));
            println!("materials  {}", f.materials.len());
            for m in f.materials.iter().take(40) {
                let tex = m.texture_file.as_deref().map(|t| format!("  texture {t} @ {:?}in", m.texture_scale)).unwrap_or_default();
                println!("    {:<44} rgba {:?}{tex}", m.name, m.color);
            }
            if f.materials.len() > 40 {
                println!("    ... and {} more", f.materials.len() - 40);
            }
            println!("textures   {} ({} bytes of image)", f.textures.len(), f.textures.iter().map(|t| t.data.len()).sum::<usize>());
            println!("thumbnail  {}", f.thumbnail.as_ref().map_or("none".into(), |t| format!("{} bytes", t.len())));
            match &f.model_dat {
                Some(d) => {
                    let keys = census(d)?;
                    println!("model.dat  {} bytes, {} records, {} distinct keys", d.len(), keys.values().sum::<usize>(), keys.len());
                }
                None => println!("model.dat  none (legacy MFC container)"),
            }
            Ok(())
        }
        ["tree", path] | ["tree", path, _] => {
            let depth = args.get(2).and_then(|d| d.parse().ok()).unwrap_or(6);
            let f = load(path)?;
            let bytes = model_dat(&f)?;
            let root = tlv::root(bytes).ok_or("model.dat is not a single well-formed record")?;
            let mut out = String::new();
            tlv::dump(&root, depth, 32, &mut out);
            print!("{out}");
            Ok(())
        }
        ["keys", path] => {
            let f = load(path)?;
            let keys = census(model_dat(&f)?)?;
            let mut rows: Vec<_> = keys.into_iter().collect();
            rows.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
            for (k, n) in rows {
                println!("0x{k:04x}  {n:>8}");
            }
            Ok(())
        }
        ["values", path, key] => {
            let want = u16::from_str_radix(key.trim_start_matches("0x"), 16).map_err(|_| format!("{key} is not a hex key like 0x05dc"))?;
            let f = load(path)?;
            let root = tlv::root(model_dat(&f)?).ok_or("model.dat is not a single well-formed record")?;
            let mut seen = 0usize;
            // A census says how often a key occurs; this says what it actually holds, which is
            // what tells a container apart from a number.
            fn go(r: &tlv::Record, want: u16, seen: &mut usize, depth: usize) {
                if r.key == want && *seen < 20 {
                    *seen += 1;
                    let hex: String = r.data.iter().take(24).map(|b| format!("{b:02x}")).collect();
                    // Nesting is the strongest signal there is: once a payload parses as records,
                    // reading the same bytes as a number is just noise.
                    let note = if let Some(kids) = r.children() {
                        format!("  {} children: {}", kids.len(), kids.iter().map(|k| format!("0x{:04x}", k.key)).collect::<Vec<_>>().join(" "))
                    } else if let Some(t) = r.text().filter(|t| !t.is_empty()) {
                        format!("  text {t:?}")
                    } else {
                        match r.data.len() {
                            2 => format!("  u16 {}", r.u16().unwrap_or(0)),
                            4 => format!("  u32 {}  f32 {:.6}", r.u32().unwrap_or(0), f32::from_bits(r.u32().unwrap_or(0))),
                            8 => format!("  u64 {}  f64 {:.6}", u64::from_le_bytes(r.data.try_into().unwrap()), f64::from_le_bytes(r.data.try_into().unwrap())),
                            _ => String::new(),
                        }
                    };
                    println!("len {:<5} {hex}{note}", r.data.len());
                }
                if depth > 64 {
                    return;
                }
                if let Some(kids) = r.children() {
                    for k in kids {
                        go(&k, want, seen, depth + 1);
                    }
                }
            }
            go(&root, want, &mut seen, 0);
            if seen == 0 {
                println!("(key 0x{want:04x} does not occur)");
            }
            Ok(())
        }
        ["diff", a, b] => {
            let (fa, fb) = (load(a)?, load(b)?);
            let (ka, kb) = (census(model_dat(&fa)?)?, census(model_dat(&fb)?)?);
            println!("{:<8} {:>10} {:>10} {:>10}", "key", "a", "b", "b - a");
            let mut any = false;
            for k in ka.keys().chain(kb.keys()).copied().collect::<std::collections::BTreeSet<_>>() {
                let (x, y) = (ka.get(&k).copied().unwrap_or(0), kb.get(&k).copied().unwrap_or(0));
                if x != y {
                    any = true;
                    println!("0x{k:04x}   {x:>10} {y:>10} {:>+10}", y as i64 - x as i64);
                }
            }
            if !any {
                println!("(the two files use the same keys the same number of times)");
            }
            Ok(())
        }
        _ => Err(concat!(
            "usage:\n",
            "  skp-inspect summary <file>          what the container holds\n",
            "  skp-inspect tree    <file> [depth]  the record tree (default depth 6)\n",
            "  skp-inspect keys    <file>          every key and how often it occurs\n",
            "  skp-inspect values  <file> <key>    what records with that key hold, e.g. 0x05dc\n",
            "  skp-inspect diff    <a> <b>         which keys differ, and by how many"
        )
        .to_string()),
    }
}
