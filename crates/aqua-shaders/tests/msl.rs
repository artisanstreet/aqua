#![cfg(any(target_os = "linux", target_os = "macos"))]

#[cfg(target_os = "linux")]
#[path = "../msl/map.rs"]
#[allow(dead_code)]
mod map;
#[cfg(target_os = "linux")]
#[path = "../msl/translate.rs"]
mod translate;

use aqua_shaders::Stage;

struct Source {
    name: &'static str,
    stage: Stage,
    text: String,
}

#[cfg(target_os = "linux")]
fn sources() -> Vec<Source> {
    aqua_shaders::SPIRV
        .iter()
        .map(|blob| Source {
            name: blob.name,
            stage: blob.stage,
            text: translate::translate(&translate::words(blob.bytes))
                .unwrap_or_else(|error| panic!("SPIRV-Cross translates {}: {error}", blob.name)),
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn sources() -> Vec<Source> {
    aqua_shaders::MSL
        .iter()
        .map(|blob| Source {
            name: blob.name,
            stage: blob.stage,
            text: String::from_utf8(blob.bytes.to_vec())
                .unwrap_or_else(|_| panic!("{} is UTF-8 MSL", blob.name)),
        })
        .collect()
}

fn source(name: &str) -> Source {
    sources()
        .into_iter()
        .find(|source| source.name == name)
        .unwrap_or_else(|| panic!("{name} has Metal source"))
}

fn entry_line(source: &Source) -> String {
    let qualifier = match source.stage {
        Stage::Vertex => "vertex ",
        Stage::Pixel => "fragment ",
        Stage::Compute => "kernel void ",
    };
    source
        .text
        .lines()
        .find(|line| line.starts_with(qualifier) && line.contains(&format!(" {}(", source.name)))
        .unwrap_or_else(|| {
            panic!(
                "{} declares its entry point under its HLSL name",
                source.name
            )
        })
        .to_owned()
}

fn attributes(source: &Source) -> Vec<(String, u32)> {
    source
        .text
        .lines()
        .filter_map(|line| {
            let (declaration, rest) = line.split_once(" [[attribute(")?;
            let location = rest.split_once(")]]")?.0.parse().ok()?;
            let name = declaration.rsplit(' ').next()?.strip_prefix("in_var_")?;
            Some((name.to_owned(), location))
        })
        .collect()
}

#[test]
fn every_entry_point_has_metal_source_under_its_hlsl_name() {
    let sources = sources();
    assert_eq!(sources.len(), 21, "every HLSL entry point has Metal source");
    for source in &sources {
        assert!(
            source.text.starts_with("#include <metal_stdlib>"),
            "{} is MSL",
            source.name
        );
        entry_line(source);
    }
}

#[test]
fn the_replace_pass_writes_its_weight_as_the_second_dual_source_output() {
    let source = source("ps_image_replace");
    let outputs: Vec<&str> = source
        .text
        .lines()
        .filter(|line| line.contains("[[color("))
        .map(str::trim)
        .collect();
    assert_eq!(
        outputs,
        [
            "float4 out_var_SV_Target0 [[color(0), index(0)]];",
            "float4 out_var_SV_Target1 [[color(0), index(1)]];",
        ]
    );
}

#[test]
fn the_row_box_blur_writes_its_output_at_texture_slot_eight() {
    let entry = entry_line(&source("cs_row_box_blur"));
    assert!(
        entry.contains("texture2d<float> input0 [[texture(0)]]"),
        "{entry}"
    );
    assert!(
        entry.contains("texture2d<float, access::write> _output [[texture(8)]]"),
        "{entry}"
    );
    assert!(
        entry.contains("constant type_Parameters& Parameters [[buffer(0)]]"),
        "{entry}"
    );
}

#[test]
fn frame_parameters_live_at_buffer_zero_and_the_sampler_at_slot_zero() {
    for name in ["vs_quad", "vs_image", "ps_blur_down", "ps_slug", "ps_glyph"] {
        let entry = entry_line(&source(name));
        assert!(
            entry.contains("constant type_Frame& Frame [[buffer(0)]]"),
            "{name} reads Frame at buffer 0: {entry}"
        );
    }
    let blur = entry_line(&source("ps_blur_down"));
    assert!(
        blur.contains(
            "texture2d<float> source [[texture(0)]], sampler linear_clamp [[sampler(0)]]"
        ),
        "{blur}"
    );
    let image = entry_line(&source("ps_image"));
    assert!(
        image.contains("page [[texture(0)]]") && image.contains("mask_page [[texture(1)]]"),
        "{image}"
    );
}

#[test]
fn no_resource_uses_the_instance_stream_slots() {
    for source in sources() {
        let entry = entry_line(&source);
        assert!(
            !entry.contains("[[buffer(30)]]") && !entry.contains("[[buffer(29)]]"),
            "{} binds a resource where the instance streams go: {entry}",
            source.name
        );
    }
}

#[test]
fn vertex_attributes_keep_hlsl_declaration_order() {
    let expected: &[(&str, &[&str])] = &[
        (
            "vs_quad",
            &[
                "BOUNDS",
                "CLIP",
                "SHAPE",
                "FILL",
                "BORDER",
                "GRADIENT",
                "STOPA",
                "STOPB",
                "STOPOFFSETS",
                "XFORM",
            ],
        ),
        ("vs_solid", &["BOUNDS", "FILL", "COLOR_SPACE"]),
        ("vs_srgba", &["BOUNDS", "FILL"]),
        ("vs_shadow", &["BOUNDS", "CLIP", "SHAPE", "OFFSET", "COLOR"]),
        ("vs_glyph", &["BOUNDS", "TEXELS", "CLIP", "COLOR"]),
        ("vs_image", &["BOUNDS", "TEXELS", "CLIP", "PARAMS", "XFORM"]),
        (
            "vs_slug",
            &["PLACEMENT", "GLYPH", "EMBOUNDS", "CLIP", "COLOR"],
        ),
    ];
    for source in sources()
        .iter()
        .filter(|source| source.stage == Stage::Vertex)
    {
        let wanted: Vec<(String, u32)> = expected
            .iter()
            .find(|(name, _)| *name == source.name)
            .map_or(&[][..], |(_, semantics)| *semantics)
            .iter()
            .enumerate()
            .map(|(location, semantic)| (semantic.to_string(), location as u32))
            .collect();
        assert_eq!(attributes(source), wanted, "{} attributes", source.name);
    }
}

#[cfg(target_os = "macos")]
#[test]
fn every_metal_source_compiles_with_the_xcode_metal_compiler() {
    use std::process::Command;
    let probe = Command::new("xcrun")
        .args(["-sdk", "macosx", "--find", "metal"])
        .output();
    if !probe.as_ref().is_ok_and(|output| output.status.success()) {
        eprintln!("SKIP-NO-XCODE: xcrun cannot find the metal compiler");
        assert!(
            std::env::var_os("AQUA_REQUIRE_GPU").is_none_or(|value| value != "1"),
            "AQUA_REQUIRE_GPU=1 but the Xcode metal compiler is missing"
        );
        return;
    }
    let directory = std::env::temp_dir().join(format!("aqua-msl-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("temporary directory");
    let failures: Vec<String> = sources()
        .iter()
        .filter_map(|source| {
            let path = directory.join(format!("{}.metal", source.name));
            std::fs::write(&path, &source.text).expect("MSL written");
            let output = Command::new("xcrun")
                .args(["-sdk", "macosx", "metal", "-std=macos-metal2.4", "-c"])
                .arg(&path)
                .arg("-o")
                .arg(path.with_extension("air"))
                .output()
                .expect("xcrun runs");
            (!output.status.success()).then(|| {
                format!(
                    "{}: {}",
                    source.name,
                    String::from_utf8_lossy(&output.stderr)
                )
            })
        })
        .collect();
    std::fs::remove_dir_all(&directory).expect("temporary directory removed");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
