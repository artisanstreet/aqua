use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Entry {
    file: &'static str,
    entry: &'static str,
}

const ENTRIES: &[Entry] = &[
    Entry {
        file: "quad.hlsl",
        entry: "vs_quad",
    },
    Entry {
        file: "quad.hlsl",
        entry: "ps_quad",
    },
    Entry {
        file: "layer.hlsl",
        entry: "ps_layer",
    },
    Entry {
        file: "layer.hlsl",
        entry: "vs_fullscreen",
    },
    Entry {
        file: "layer.hlsl",
        entry: "ps_copy",
    },
    Entry {
        file: "shadow.hlsl",
        entry: "vs_shadow",
    },
    Entry {
        file: "shadow.hlsl",
        entry: "ps_shadow",
    },
    Entry {
        file: "blur.hlsl",
        entry: "ps_blur_down",
    },
    Entry {
        file: "blur.hlsl",
        entry: "ps_blur_up",
    },
    Entry {
        file: "glyph.hlsl",
        entry: "vs_glyph",
    },
    Entry {
        file: "glyph.hlsl",
        entry: "ps_glyph",
    },
    Entry {
        file: "image.hlsl",
        entry: "vs_image",
    },
    Entry {
        file: "image.hlsl",
        entry: "ps_image",
    },
];

enum Host {
    Native,
    Wsl,
}

struct Dxc {
    path: PathBuf,
    host: Host,
}

fn windows_kit_dxc(root: &Path) -> Option<PathBuf> {
    let mut versions: Vec<PathBuf> = fs::read_dir(root)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    versions.sort();
    versions
        .into_iter()
        .rev()
        .map(|dir| dir.join("x64").join("dxc.exe"))
        .find(|candidate| candidate.exists())
}

fn on_path(name: &str) -> Option<PathBuf> {
    let paths = env::var_os("PATH")?;
    env::split_paths(&paths)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.exists())
}

fn locate() -> Option<Dxc> {
    let native_windows = cfg!(windows);
    let explicit = env::var_os("AQUA_DXC").map(PathBuf::from);
    let kits = [
        "/mnt/c/Program Files (x86)/Windows Kits/10/bin",
        "C:\\Program Files (x86)\\Windows Kits\\10\\bin",
    ];
    let candidates = explicit
        .into_iter()
        .chain(on_path("dxc.exe"))
        .chain(on_path("dxc"))
        .chain(
            kits.iter()
                .filter_map(|root| windows_kit_dxc(Path::new(root))),
        );
    candidates
        .into_iter()
        .find(|path| path.exists())
        .map(|path| {
            let is_exe = path.extension().is_some_and(|ext| ext == "exe");
            Dxc {
                host: if is_exe && !native_windows {
                    Host::Wsl
                } else {
                    Host::Native
                },
                path,
            }
        })
}

fn windows_path(dxc: &Dxc, path: &Path) -> String {
    match dxc.host {
        Host::Native => path.display().to_string(),
        Host::Wsl => {
            let output = Command::new("wslpath")
                .arg("-w")
                .arg(path)
                .output()
                .expect("wslpath is required to drive dxc.exe from WSL");
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
    }
}

fn profile(entry: &str) -> &'static str {
    match entry.split('_').next() {
        Some("vs") => "vs_6_0",
        Some("ps") => "ps_6_0",
        Some("cs") => "cs_6_0",
        _ => panic!("entry {entry} must start with vs_, ps_ or cs_"),
    }
}

fn stage(entry: &str) -> &'static str {
    match entry.split('_').next() {
        Some("vs") => "Vertex",
        Some("ps") => "Pixel",
        _ => "Compute",
    }
}

fn compile(dxc: &Dxc, source: &Path, include: &Path, entry: &str, output: &Path) {
    let result = Command::new(&dxc.path)
        .args(["-nologo", "-Zpr", "-O3", "-T", profile(entry), "-E", entry])
        .arg("-I")
        .arg(windows_path(dxc, include))
        .arg("-Fo")
        .arg(windows_path(dxc, output))
        .arg(windows_path(dxc, source))
        .output()
        .expect("failed to launch dxc");
    if !result.status.success() || !output.exists() {
        panic!(
            "dxc failed for {} {entry}\n{}{}",
            source.display(),
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let hlsl = manifest.join("hlsl");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    println!("cargo:rerun-if-changed=hlsl");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=AQUA_DXC");
    let dxc = locate().expect("dxc not found: set AQUA_DXC to dxc or dxc.exe from the Windows SDK");
    let mut generated = String::from("pub static BLOBS: &[Blob] = &[\n");
    for entry in ENTRIES {
        let output = out.join(format!("{}.dxil", entry.entry));
        compile(&dxc, &hlsl.join(entry.file), &hlsl, entry.entry, &output);
        writeln!(
            generated,
            "    Blob {{ name: \"{}\", stage: Stage::{}, bytes: include_bytes!(\"{}\") }},",
            entry.entry,
            stage(entry.entry),
            output.display().to_string().replace('\\', "/")
        )
        .unwrap();
    }
    generated.push_str("];\n");
    fs::write(out.join("blobs.rs"), generated).unwrap();
}
