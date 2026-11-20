use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Entry {
    file: &'static str,
    entry: &'static str,
}

const ENTRIES: &[Entry] = &[
    Entry {
        file: "quad.hlsl",
        entry: "vs_srgba",
    },
    Entry {
        file: "quad.hlsl",
        entry: "vs_solid",
    },
    Entry {
        file: "quad.hlsl",
        entry: "ps_solid",
    },
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
        file: "slug.hlsl",
        entry: "vs_slug",
    },
    Entry {
        file: "slug.hlsl",
        entry: "ps_slug",
    },
    Entry {
        file: "image.hlsl",
        entry: "vs_image",
    },
    Entry {
        file: "image.hlsl",
        entry: "ps_image",
    },
    Entry {
        file: "image.hlsl",
        entry: "ps_image_replace",
    },
    Entry {
        file: "color_matrix.hlsl",
        entry: "ps_color_matrix",
    },
    Entry {
        file: "compute.hlsl",
        entry: "cs_row_box_blur",
    },
];

const LANES: usize = 6;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Bytecode {
    Dxil { bindless: bool },
    Dxbc,
    Spirv,
}

enum Host {
    Native,
    Wsl,
}

struct Tool {
    path: PathBuf,
    host: Host,
}

struct Job<'a> {
    entry: &'a Entry,
    bytecode: Bytecode,
    name: String,
    output: PathBuf,
}

fn windows_kit_tool(root: &Path, name: &str) -> Option<PathBuf> {
    let mut versions: Vec<PathBuf> = fs::read_dir(root)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    versions.sort();
    versions
        .into_iter()
        .rev()
        .map(|dir| dir.join("x64").join(name))
        .find(|candidate| candidate.exists())
}

fn on_path(name: &str) -> Option<PathBuf> {
    let paths = env::var_os("PATH")?;
    env::split_paths(&paths)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.exists())
}

fn locate(variable: &str, stem: &str) -> Option<Tool> {
    let native_windows = cfg!(windows);
    let explicit = env::var_os(variable).map(PathBuf::from);
    let exe = format!("{stem}.exe");
    let kits = [
        "/mnt/c/Program Files (x86)/Windows Kits/10/bin",
        "C:\\Program Files (x86)\\Windows Kits\\10\\bin",
    ];
    let candidates = explicit
        .into_iter()
        .chain(on_path(&exe))
        .chain(on_path(stem))
        .chain(
            kits.iter()
                .filter_map(|root| windows_kit_tool(Path::new(root), &exe)),
        );
    candidates
        .into_iter()
        .find(|path| path.exists())
        .map(|path| {
            let is_exe = path.extension().is_some_and(|ext| ext == "exe");
            Tool {
                host: if is_exe && !native_windows {
                    Host::Wsl
                } else {
                    Host::Native
                },
                path,
            }
        })
}

fn locate_spirv() -> Option<Tool> {
    env::var_os("AQUA_DXC_SPIRV")
        .map(PathBuf::from)
        .into_iter()
        .chain(on_path("dxc"))
        .find(|path| path.exists())
        .map(|path| Tool {
            path,
            host: Host::Native,
        })
}

enum Toolchain {
    Windows { dxc: Tool, fxc: Tool },
    Spirv { dxc: Tool },
}

impl Toolchain {
    fn tool(&self, bytecode: Bytecode) -> &Tool {
        match (self, bytecode) {
            (Toolchain::Windows { dxc, .. }, Bytecode::Dxil { .. }) => dxc,
            (Toolchain::Windows { fxc, .. }, Bytecode::Dxbc) => fxc,
            (Toolchain::Spirv { dxc }, Bytecode::Spirv) => dxc,
            _ => panic!("a shader job was planned for a toolchain that cannot compile it"),
        }
    }
}

const SPIRV_ARGUMENTS: &[&str] = &[
    "-spirv",
    "-fspv-target-env=vulkan1.1",
    "-fvk-use-dx-layout",
    "-fvk-b-shift",
    "0",
    "0",
    "-fvk-t-shift",
    "16",
    "0",
    "-fvk-s-shift",
    "32",
    "0",
    "-fvk-u-shift",
    "48",
    "0",
];

fn windows_path(tool: &Tool, path: &Path) -> String {
    match tool.host {
        Host::Native => path.display().to_string(),
        Host::Wsl => {
            let output =
                Command::new("wslpath").arg("-w").arg(path).output().expect(
                    "wslpath is required to drive the Windows SDK shader compilers from WSL",
                );
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
    }
}

fn profile(entry: &str, bytecode: Bytecode) -> &'static str {
    match (entry.split('_').next(), bytecode) {
        (Some("ps"), Bytecode::Dxil { bindless: true }) => "ps_6_6",
        (Some("cs"), Bytecode::Dxil { bindless: true }) => "cs_6_6",
        (Some("vs"), Bytecode::Dxil { .. }) => "vs_6_0",
        (Some("ps"), Bytecode::Dxil { bindless: false }) => "ps_6_0",
        (Some("cs"), Bytecode::Dxil { bindless: false }) => "cs_6_0",
        (Some("vs"), Bytecode::Dxbc) => "vs_5_0",
        (Some("ps"), Bytecode::Dxbc) => "ps_5_0",
        (Some("cs"), Bytecode::Dxbc) => "cs_5_0",
        (Some("vs"), Bytecode::Spirv) => "vs_6_0",
        (Some("ps"), Bytecode::Spirv) => "ps_6_0",
        (Some("cs"), Bytecode::Spirv) => "cs_6_0",
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

fn compile(tool: &Tool, hlsl: &Path, job: &Job) -> Result<(), String> {
    let source = hlsl.join(job.entry.file);
    let mut command = Command::new(&tool.path);
    command.args([
        "-nologo",
        "-Zpr",
        "-O3",
        "-T",
        profile(job.entry.entry, job.bytecode),
        "-E",
        job.entry.entry,
    ]);
    match job.bytecode {
        Bytecode::Dxil { bindless: true } => {
            command.args(["-D", "BINDLESS=1"]);
        }
        Bytecode::Spirv => {
            command.args(SPIRV_ARGUMENTS);
        }
        Bytecode::Dxil { bindless: false } | Bytecode::Dxbc => {}
    }
    let result = command
        .arg("-I")
        .arg(windows_path(tool, hlsl))
        .arg("-Fo")
        .arg(windows_path(tool, &job.output))
        .arg(windows_path(tool, &source))
        .output()
        .map_err(|error| format!("failed to launch {}: {error}", tool.path.display()))?;
    match result.status.success() && job.output.exists() {
        true => Ok(()),
        false => Err(format!(
            "{} failed for {} {}\n{}{}",
            tool.path.display(),
            source.display(),
            job.entry.entry,
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        )),
    }
}

fn run_all(jobs: &[Job], hlsl: &Path, toolchain: &Toolchain) {
    let next = AtomicUsize::new(0);
    let failures: Vec<String> = std::thread::scope(|scope| {
        let lanes: Vec<_> = (0..LANES.min(jobs.len()))
            .map(|_| {
                scope.spawn(|| {
                    let mut failed = Vec::new();
                    while let Some(job) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) {
                        if let Err(message) = compile(toolchain.tool(job.bytecode), hlsl, job) {
                            failed.push(message);
                        }
                    }
                    failed
                })
            })
            .collect();
        lanes
            .into_iter()
            .flat_map(|lane| {
                lane.join()
                    .unwrap_or_else(|_| vec!["compile lane panicked".into()])
            })
            .collect()
    });
    if !failures.is_empty() {
        panic!("{}", failures.join("\n"));
    }
}

fn table(generated: &mut String, name: &str, jobs: &[Job], wanted: impl Fn(Bytecode) -> bool) {
    writeln!(generated, "pub static {name}: &[Blob] = &[").unwrap();
    for job in jobs.iter().filter(|job| wanted(job.bytecode)) {
        writeln!(
            generated,
            "    Blob {{ name: \"{}\", stage: Stage::{}, bytes: include_bytes!(\"{}\") }},",
            job.name,
            stage(job.entry.entry),
            job.output.display().to_string().replace('\\', "/")
        )
        .unwrap();
    }
    generated.push_str("];\n");
}

enum Mode {
    Windows,
    Spirv,
    Skip,
}

fn mode() -> Mode {
    let target = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let skip_requested = env::var_os("AQUA_SHADERS_SKIP").is_some_and(|value| value == "1");
    match (target.as_str(), skip_requested) {
        ("windows" | "linux", true) => {
            println!(
                "cargo:warning=AQUA_SHADERS_SKIP=1: shader tables are empty, the GPU backends will fail at runtime"
            );
            Mode::Skip
        }
        ("windows", false) => Mode::Windows,
        ("linux", false) => Mode::Spirv,
        _ => Mode::Skip,
    }
}

fn write_tables(out: &Path, jobs: &[Job]) {
    let mut generated = String::new();
    table(&mut generated, "BLOBS", jobs, |bytecode| {
        matches!(bytecode, Bytecode::Dxil { .. })
    });
    table(&mut generated, "DXBC", jobs, |bytecode| {
        bytecode == Bytecode::Dxbc
    });
    table(&mut generated, "SPIRV", jobs, |bytecode| {
        bytecode == Bytecode::Spirv
    });
    fs::write(out.join("blobs.rs"), generated).unwrap();
}

fn job(entry: &'static Entry, bytecode: Bytecode, out: &Path) -> Job<'static> {
    let (name, file) = match bytecode {
        Bytecode::Dxil { bindless: true } => {
            let name = format!("{}_bl", entry.entry);
            let file = format!("{name}.dxil");
            (name, file)
        }
        Bytecode::Dxil { bindless: false } => {
            (entry.entry.to_string(), format!("{}.dxil", entry.entry))
        }
        Bytecode::Dxbc => (entry.entry.to_string(), format!("{}.dxbc", entry.entry)),
        Bytecode::Spirv => (entry.entry.to_string(), format!("{}.spv", entry.entry)),
    };
    Job {
        entry,
        bytecode,
        name,
        output: out.join(file),
    }
}

fn windows_jobs(out: &Path) -> Vec<Job<'static>> {
    let dxil = ENTRIES
        .iter()
        .map(|entry| (entry, Bytecode::Dxil { bindless: false }))
        .chain(
            ENTRIES
                .iter()
                .filter(|entry| entry.entry.starts_with("ps_"))
                .map(|entry| (entry, Bytecode::Dxil { bindless: true })),
        );
    let dxbc = ENTRIES.iter().map(|entry| (entry, Bytecode::Dxbc));
    dxil.chain(dxbc)
        .map(|(entry, bytecode)| job(entry, bytecode, out))
        .collect()
}

fn spirv_jobs(out: &Path) -> Vec<Job<'static>> {
    ENTRIES
        .iter()
        .map(|entry| job(entry, Bytecode::Spirv, out))
        .collect()
}

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let hlsl = manifest.join("hlsl");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    println!("cargo:rerun-if-changed=hlsl");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=AQUA_DXC");
    println!("cargo:rerun-if-env-changed=AQUA_FXC");
    println!("cargo:rerun-if-env-changed=AQUA_DXC_SPIRV");
    println!("cargo:rerun-if-env-changed=AQUA_SHADERS_SKIP");
    let (toolchain, jobs) = match mode() {
        Mode::Skip => {
            write_tables(&out, &[]);
            return;
        }
        Mode::Windows => {
            let dxc = locate("AQUA_DXC", "dxc")
                .expect("dxc not found: set AQUA_DXC to dxc or dxc.exe from the Windows SDK");
            let fxc = locate("AQUA_FXC", "fxc").expect(
                "fxc not found: set AQUA_FXC to fxc.exe from the Windows SDK (same bin folder as dxc.exe)",
            );
            (Toolchain::Windows { dxc, fxc }, windows_jobs(&out))
        }
        Mode::Spirv => {
            let dxc = locate_spirv().expect(
                "dxc with SPIR-V codegen not found: set AQUA_DXC_SPIRV (nix-shell nix/linux.nix exports it) or AQUA_SHADERS_SKIP=1",
            );
            (Toolchain::Spirv { dxc }, spirv_jobs(&out))
        }
    };
    jobs.iter().for_each(|job| {
        let _ = fs::remove_file(&job.output);
    });
    run_all(&jobs, &hlsl, &toolchain);
    write_tables(&out, &jobs);
}
