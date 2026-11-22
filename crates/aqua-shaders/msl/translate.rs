use spirv_cross2::compile::msl::{BindTarget, CompilerOptions, MetalPlatform, ResourceBinding};
use spirv_cross2::spirv::ExecutionModel;
use spirv_cross2::targets::Msl;
use spirv_cross2::{Compiler, Module, SpirvCrossError};

use crate::map::{slot, Slot, BINDINGS, LANGUAGE, SET};

fn target(slot: Slot) -> BindTarget {
    let index = match slot {
        Slot::Buffer(index) | Slot::Texture(index) | Slot::Sampler(index) => index,
    };
    BindTarget {
        buffer: index,
        texture: index,
        sampler: index,
        count: None,
    }
}

pub fn translate(words: &[u32]) -> Result<String, SpirvCrossError> {
    let mut compiler = Compiler::<Msl>::new(Module::from_words(words))?;
    let models: Vec<ExecutionModel> = compiler
        .entry_points()?
        .map(|entry| entry.execution_model)
        .collect();
    for model in models {
        for binding in 0..BINDINGS {
            if let Some(slot) = slot(binding) {
                compiler.add_resource_binding(
                    model,
                    ResourceBinding::Qualified { set: SET, binding },
                    &target(slot),
                )?;
            }
        }
    }
    let mut options = CompilerOptions::default();
    options.version = LANGUAGE.into();
    options.platform = MetalPlatform::MacOS;
    Ok(compiler.compile(&options)?.to_string())
}

pub fn words(bytes: &[u8]) -> Vec<u32> {
    assert_eq!(
        bytes.len() % 4,
        0,
        "invariant: SPIR-V is a whole number of 32-bit words"
    );
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| u32::from_le_bytes(*chunk))
        .collect()
}
