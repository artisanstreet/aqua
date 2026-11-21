#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::process::Command;

use aqua_shaders::{find_spirv, Stage, SPIRV};

const MAGIC: u32 = 0x0723_0203;
const OP_NAME: u32 = 5;
const OP_DECORATE: u32 = 71;
const DECORATION_LOCATION: u32 = 30;
const DECORATION_INDEX: u32 = 32;

#[derive(Default)]
struct Interface {
    names: HashMap<u32, String>,
    locations: HashMap<u32, u32>,
    indices: HashMap<u32, u32>,
}

fn words(bytes: &[u8]) -> Vec<u32> {
    assert_eq!(bytes.len() % 4, 0, "SPIR-V is a whole number of words");
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| u32::from_le_bytes(*chunk))
        .collect()
}

fn literal_string(operands: &[u32]) -> String {
    let bytes: Vec<u8> = operands
        .iter()
        .flat_map(|word| word.to_le_bytes())
        .take_while(|byte| *byte != 0)
        .collect();
    String::from_utf8(bytes).expect("SPIR-V names are UTF-8")
}

fn interface(name: &str) -> Interface {
    let blob = find_spirv(name).unwrap_or_else(|| panic!("{name} has no SPIR-V blob"));
    let words = words(blob.bytes);
    assert_eq!(words[0], MAGIC, "{name} starts with the SPIR-V magic");
    let mut found = Interface::default();
    let mut at = 5;
    while at < words.len() {
        let count = (words[at] >> 16) as usize;
        let opcode = words[at] & 0xffff;
        let operands = &words[at + 1..at + count];
        match (opcode, operands) {
            (OP_NAME, [target, rest @ ..]) => {
                found.names.insert(*target, literal_string(rest));
            }
            (OP_DECORATE, [target, DECORATION_LOCATION, location]) => {
                found.locations.insert(*target, *location);
            }
            (OP_DECORATE, [target, DECORATION_INDEX, index]) => {
                found.indices.insert(*target, *index);
            }
            _ => {}
        }
        at += count.max(1);
    }
    found
}

fn input_locations(name: &str) -> Vec<(String, u32)> {
    let interface = interface(name);
    let mut inputs: Vec<(String, u32)> = interface
        .locations
        .iter()
        .filter_map(|(id, location)| {
            interface
                .names
                .get(id)
                .and_then(|variable| variable.strip_prefix("in.var."))
                .map(|semantic| (semantic.to_string(), *location))
        })
        .collect();
    inputs.sort_by_key(|(_, location)| *location);
    inputs
}

fn spirv_val() -> Option<std::path::PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|directory| directory.join("spirv-val"))
        .find(|candidate| candidate.exists())
}

#[test]
fn every_entry_point_compiles_to_valid_spirv_for_vulkan_1_1() {
    assert_eq!(SPIRV.len(), 21, "every HLSL entry point has a SPIR-V blob");
    let Some(validator) = spirv_val() else {
        eprintln!(
            "SKIP-NO-SPIRV-VAL: spirv-val is not on PATH (nix-shell nix/linux.nix provides it)"
        );
        assert!(
            std::env::var_os("AQUA_REQUIRE_GPU").is_none_or(|value| value != "1"),
            "AQUA_REQUIRE_GPU=1 but spirv-val is missing"
        );
        return;
    };
    let directory = std::env::temp_dir().join(format!("aqua-spirv-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("temporary directory");
    let failures: Vec<String> = SPIRV
        .iter()
        .filter_map(|blob| {
            assert!(!blob.bytes.is_empty(), "{} is empty", blob.name);
            let path = directory.join(format!("{}.spv", blob.name));
            std::fs::write(&path, blob.bytes).expect("blob written");
            let output = Command::new(&validator)
                .args(["--target-env", "vulkan1.1"])
                .arg(&path)
                .output()
                .expect("spirv-val runs");
            (!output.status.success()).then(|| {
                format!(
                    "{}: {}{}",
                    blob.name,
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                )
            })
        })
        .collect();
    let _ = std::fs::remove_dir_all(&directory);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn vertex_inputs_keep_hlsl_declaration_order_as_locations() {
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
    for (shader, semantics) in expected {
        let found = input_locations(shader);
        let wanted: Vec<(String, u32)> = semantics
            .iter()
            .enumerate()
            .map(|(location, semantic)| (semantic.to_string(), location as u32))
            .collect();
        assert_eq!(found, wanted, "{shader} input locations");
    }
    assert!(SPIRV
        .iter()
        .filter(|blob| blob.stage == Stage::Vertex)
        .all(|blob| expected.iter().any(|(name, _)| *name == blob.name)
            || input_locations(blob.name).is_empty()));
}

#[test]
fn image_replace_writes_dual_source_outputs() {
    let interface = interface("ps_image_replace");
    let mut outputs: Vec<(String, u32, u32)> = interface
        .names
        .iter()
        .filter_map(|(id, name)| {
            name.strip_prefix("out.var.SV_Target").map(|_| {
                (
                    name.clone(),
                    interface.locations.get(id).copied().unwrap_or(u32::MAX),
                    interface.indices.get(id).copied().unwrap_or(0),
                )
            })
        })
        .collect();
    outputs.sort();
    assert_eq!(
        outputs,
        vec![
            ("out.var.SV_Target0".to_string(), 0, 0),
            ("out.var.SV_Target1".to_string(), 0, 1),
        ]
    );
}
