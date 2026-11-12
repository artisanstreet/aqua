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

pub const ADAPTER_CRATES: &[&str] = &[
    "aqua-gpu-d3d11",
    "aqua-gpu-d3d12",
    "aqua-gpu-ref",
    "aqua-shell-win32",
    "aqua-ime-tsf",
    "aqua-a11y-uia",
    "aqua-fonts-system",
    "aqua-shaders",
    "aqua-transport",
    "aqua-oracle-dwrite",
    "aqua-avif",
];

pub const HOST_CRATES: &[&str] = &["aqua"];

pub const VERIFY_CRATES: &[&str] = &["aqua-verify"];

pub const BANNED_EVERYWHERE: &[&str] = &[
    "wgpu",
    "wgpu-core",
    "wgpu-hal",
    "wgpu-types",
    "glyphon",
    "cosmic-text",
    "winit",
    "accesskit_winit",
    "fontdb",
    "aqua-render",
    "aqua-platform",
    "aqua-gpu-wgpu",
];

pub const UNSAFE_AUDITED_CRATES: &[&str] = &["aqua-kernels"];

pub const FORBIDDEN_ADAPTER_CRATES: &[&str] = &[
    "aqua",
    "aqua-gpu-d3d11",
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
    "aqua-avif",
    "aqua-platform",
    "aqua-render",
    "aqua-verify",
];

pub const FORBIDDEN_EXTERNAL_CRATES: &[&str] = &[
    "windows",
    "ash",
    "objc2",
    "winit",
    "wgpu",
    "glyphon",
    "cosmic-text",
    "fontdb",
];

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

pub const ALLOWED_ADAPTER_EDGES: &[(&str, &str)] = &[
    ("aqua-text", "aqua-fonts-system"),
    ("aqua-media", "aqua-avif"),
];

pub const KERNEL_CRATE: &str = "aqua-kernels";

pub const KERNEL_TWIN_FN: &str = "pub fn reference";

pub const REFERENCE_ONLY_KERNEL_USERS: &[&str] = &["aqua-gpu-ref"];

pub const ADAPTER_PORT_IMPLS: &[(&str, &str)] = &[
    ("aqua-gpu-d3d11", "GpuBackend"),
    ("aqua-gpu-d3d12", "GpuBackend"),
    ("aqua-gpu-ref", "GpuBackend"),
    ("aqua-ime-tsf", "ImeBackend"),
    ("aqua-transport", "RenderHost"),
    ("aqua-transport", "Lowering"),
];

pub const ADAPTER_ROOT_ITEMS: &[(&str, &[&str])] = &[
    ("aqua-avif", &["decode", "dimensions", "AvifError", "Rgba"]),
    (
        "aqua-gpu-d3d11",
        &[
            "hal",
            "half",
            "ledger",
            "lifetimes",
            "memo",
            "stages",
            "pack",
            "pool",
            "sprite",
            "window",
            "scene",
        ],
    ),
    (
        "aqua-gpu-d3d12",
        &[
            "hal",
            "half",
            "ledger",
            "pack",
            "placement",
            "policy",
            "sprite",
            "window",
            "scene",
        ],
    ),
    (
        "aqua-gpu-ref",
        &[
            "backend",
            "conformance",
            "erf",
            "filter",
            "frame",
            "kernels",
            "raster",
            "sample",
            "shadow",
            "sprites",
            "RefBackend",
        ],
    ),
    (
        "aqua-shell-win32",
        &[
            "capability",
            "dialog_protocol",
            "hooks",
            "icon",
            "input",
            "notifications",
            "occlusion",
            "open",
            "picker",
            "pointer",
            "pointer_source",
            "regions",
            "services",
            "timing",
            "wake_trace",
            "Chrome",
            "FileDrag",
            "Handler",
            "ShellError",
            "ShellEvent",
            "WindowKind",
            "WindowOptions",
            "set_message_hook",
            "add_message_hook",
            "MessageHook",
            "RawMessage",
            "clipboard",
            "dialogs",
            "local_time",
            "prefs",
            "taskbar",
            "NativeHandle",
            "now",
            "pump_sent_messages",
            "quit",
            "run",
            "schedule",
            "set_waitables",
            "Waker",
            "MaterialOutcome",
            "Monitor",
            "Window",
        ],
    ),
    (
        "aqua-ime-tsf",
        &[
            "double",
            "keystroke",
            "model",
            "Keystrokes",
            "key_phase",
            "KeyPhase",
            "TsfError",
            "TsfIme",
        ],
    ),
    (
        "aqua-a11y-uia",
        &[
            "accesskit",
            "A11yBridge",
            "BridgeActionHandler",
            "BridgeActivationHandler",
            "Connection",
            "WakeFn",
            "inspect",
            "probe",
            "render_json",
            "render_text",
            "InspectorNode",
            "build_tree_update",
            "map_role",
            "pack_node_id",
            "unpack_node_id",
            "A11yTree",
            "AttachError",
            "UiaAdapter",
            "UiaRouter",
        ],
    ),
    (
        "aqua-fonts-system",
        &[
            "ID",
            "Style",
            "Weight",
            "Stretch",
            "Source",
            "FaceInfo",
            "Query",
            "FaceBytes",
            "FaceFingerprint",
            "FontDatabase",
            "read_cache_file",
            "write_cache_file",
            "default_fallback_index_path",
            "system_font_directories",
            "weight_distance_order",
        ],
    ),
    ("aqua-shaders", &["Stage", "Blob", "find", "find_dxbc"]),
    (
        "aqua-oracle-dwrite",
        &[
            "BlendParams",
            "Canvas",
            "ColorCanvas",
            "FontSource",
            "GlyphRunSpec",
            "Oracle",
            "OracleError",
            "RenderMode",
            "ShapedGlyph",
            "StrikeImage",
            "SystemRendering",
        ],
    ),
    (
        "aqua-transport",
        &[
            "Clock",
            "FrameRecord",
            "GeometryLowering",
            "GpuRenderHost",
            "HostConfig",
            "HostCounters",
            "IDLE_TRIM_AFTER",
            "Inspector",
            "InspectorError",
            "InspectorServer",
            "InspectorSnapshot",
            "LowerContext",
            "Lowered",
            "Lowering",
            "MonotonicClock",
            "Notify",
            "Receiver",
            "RecvError",
            "RenderHost",
            "RenderThread",
            "SendError",
            "Sender",
            "SharedClock",
            "SurfaceKind",
            "TrimAction",
            "TrimHandle",
            "TrimInput",
            "TrimPolicy",
            "TrimRequest",
            "channel",
            "channel_with_notify",
            "gpu_host",
            "inspector",
            "lower",
            "lower_items",
            "ring",
            "spawn_render_thread",
            "spawn_render_thread_with",
            "stats_json",
            "thread",
            "tree_json",
            "trim",
        ],
    ),
];

pub fn root_public_items(source: &str) -> Vec<String> {
    let code = crate::lint::blank_comments_and_strings(source);
    let mut items = Vec::new();
    let mut statement = String::new();
    for line in code.lines() {
        match (statement.is_empty(), line.starts_with("pub ")) {
            (true, false) => {}
            (true, true) if line.starts_with("pub use ") && !line.contains(';') => {
                statement.push_str(line);
            }
            (true, true) => items.extend(public_names(line)),
            (false, _) => {
                statement.push(' ');
                statement.push_str(line.trim());
                if line.contains(';') {
                    items.extend(public_names(&statement));
                    statement.clear();
                }
            }
        }
    }
    items
}

fn public_names(statement: &str) -> Vec<String> {
    let body = statement.trim_start_matches("pub ").trim();
    match body.strip_prefix("use ") {
        Some(path) => use_leaves(path.trim_end_matches(';')),
        None => {
            let mut words = body.split_whitespace().skip_while(|word| {
                matches!(*word, "unsafe" | "const" | "async" | "extern") && body.contains("fn ")
            });
            match words.next() {
                Some("mod" | "struct" | "enum" | "fn" | "trait" | "const" | "type" | "static") => {
                    words
                        .next()
                        .map(|name| {
                            name.chars()
                                .take_while(|c| c.is_alphanumeric() || *c == '_')
                                .collect::<String>()
                        })
                        .into_iter()
                        .collect()
                }
                _ => Vec::new(),
            }
        }
    }
}

fn use_leaves(path: &str) -> Vec<String> {
    match path.split_once('{') {
        Some((_, group)) => group
            .trim_end_matches('}')
            .split(',')
            .map(str::trim)
            .filter(|leaf| !leaf.is_empty())
            .map(leaf_name)
            .collect(),
        None => vec![leaf_name(path.trim())],
    }
}

fn leaf_name(leaf: &str) -> String {
    leaf.rsplit(" as ")
        .next()
        .unwrap_or(leaf)
        .rsplit("::")
        .next()
        .unwrap_or(leaf)
        .trim()
        .to_string()
}

pub fn architecture_crate_roles(document: &str) -> Vec<(String, String)> {
    let mut roles = Vec::new();
    let mut role = String::new();
    let mut inside = false;
    for line in document.lines() {
        match (inside, line.trim_end()) {
            (false, "## 4. Crate structure") => inside = true,
            (true, heading) if heading.starts_with("### Enforcement") => break,
            (true, heading) => match heading.split_whitespace().next() {
                Some(word) if !heading.starts_with(' ') && heading != "```" => {
                    role = word.to_string()
                }
                Some(name) if name.starts_with("aqua") => {
                    roles.push((role.clone(), name.to_string()))
                }
                _ => {}
            },
            (false, _) => {}
        }
    }
    roles
}
