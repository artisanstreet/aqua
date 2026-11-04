pub const PURE_CRATES: &[&str] = &[
    "aqua-core",
    "aqua-reactive",
    "aqua-style",
    "aqua-ui",
    "aqua-layout",
    "aqua-motion",
    "aqua-text",
    "aqua-media",
    "aqua-scene",
    "aqua-graph",
    "aqua-protocol",
    "aqua-runtime",
    "aqua-kernels",
];

pub const UNSAFE_AUDITED_CRATES: &[&str] = &["aqua-kernels"];

pub const FORBIDDEN_ADAPTER_CRATES: &[&str] = &[
    "aqua",
    "aqua-gpu-d3d12",
    "aqua-gpu-ref",
    "aqua-gpu-wgpu",
    "aqua-shell-win32",
    "aqua-ime-tsf",
    "aqua-a11y-uia",
    "aqua-fonts-system",
    "aqua-shaders",
    "aqua-transport",
    "aqua-oracle-dwrite",
    "aqua-platform",
    "aqua-render",
    "aqua-verify",
];

pub const FORBIDDEN_EXTERNAL_CRATES: &[&str] = &["windows", "ash", "objc2", "winit", "wgpu"];

pub const FORBIDDEN_SOURCE_TOKENS: &[&str] = &[
    "Instant::now",
    "SystemTime::now",
    "std::thread",
    "thread::",
    "std::fs",
    "fs::",
    "println!",
    "eprintln!",
    "print!",
    "eprint!",
    "dbg!",
    "stdout()",
    "stderr()",
];

pub const ALLOWED_ADAPTER_EDGES: &[(&str, &str)] = &[("aqua-text", "aqua-fonts-system")];

pub const KERNEL_CRATE: &str = "aqua-kernels";

pub const KERNEL_TWIN_FN: &str = "pub fn reference";

pub const REFERENCE_ONLY_KERNEL_USERS: &[&str] = &["aqua-gpu-ref"];
