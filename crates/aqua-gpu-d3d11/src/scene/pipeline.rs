use std::collections::{BTreeMap, BTreeSet, HashMap};

use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::{PCSTR, s};

use super::Error;
use crate::ledger::{Kind as LedgerKind, Token, record};

pub const CONSTANT_FLOATS: usize = 36;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Quad,
    Shadow,
    Glyph,
    Slug,
    Image,
    ImageReplace,
    Layer,
    Copy,
    BlurDown,
    BlurUp,
    ColorMatrix,
}

impl Kind {
    pub const ALL: [Kind; 11] = [
        Kind::Quad,
        Kind::Shadow,
        Kind::Glyph,
        Kind::Slug,
        Kind::Image,
        Kind::ImageReplace,
        Kind::Layer,
        Kind::Copy,
        Kind::BlurDown,
        Kind::BlurUp,
        Kind::ColorMatrix,
    ];
}

const FIRST_FRAME: [Kind; 5] = [
    Kind::Quad,
    Kind::Shadow,
    Kind::Glyph,
    Kind::Slug,
    Kind::Image,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Layout {
    None,
    Quad,
    Shadow,
    Glyph,
    Image,
    Slug,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blending {
    Opaque,
    Over,
    Dual,
}

struct Shaders {
    vertex: &'static str,
    pixel: &'static str,
    layout: Layout,
    blend: Blending,
}

fn shaders(kind: Kind) -> Shaders {
    let instanced = |vertex, pixel, layout| Shaders {
        vertex,
        pixel,
        layout,
        blend: Blending::Over,
    };
    let fullscreen = |pixel, blend| Shaders {
        vertex: "vs_fullscreen",
        pixel,
        layout: Layout::None,
        blend,
    };
    match kind {
        Kind::Quad => instanced("vs_quad", "ps_quad", Layout::Quad),
        Kind::Shadow => instanced("vs_shadow", "ps_shadow", Layout::Shadow),
        Kind::Glyph => instanced("vs_glyph", "ps_glyph", Layout::Glyph),
        Kind::Slug => instanced("vs_slug", "ps_slug", Layout::Slug),
        Kind::Image => instanced("vs_image", "ps_image", Layout::Image),
        Kind::ImageReplace => Shaders {
            blend: Blending::Dual,
            ..instanced("vs_image", "ps_image_replace", Layout::Image)
        },
        Kind::Layer => fullscreen("ps_layer", Blending::Over),
        Kind::Copy => fullscreen("ps_copy", Blending::Opaque),
        Kind::BlurDown => fullscreen("ps_blur_down", Blending::Opaque),
        Kind::BlurUp => fullscreen("ps_blur_up", Blending::Opaque),
        Kind::ColorMatrix => fullscreen("ps_color_matrix", Blending::Opaque),
    }
}

fn element(name: PCSTR, index: u32) -> D3D11_INPUT_ELEMENT_DESC {
    D3D11_INPUT_ELEMENT_DESC {
        SemanticName: name,
        SemanticIndex: 0,
        Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
        InputSlot: 0,
        AlignedByteOffset: index * 16,
        InputSlotClass: D3D11_INPUT_PER_INSTANCE_DATA,
        InstanceDataStepRate: 1,
    }
}

fn elements(layout: Layout) -> Vec<D3D11_INPUT_ELEMENT_DESC> {
    let names: &[PCSTR] = match layout {
        Layout::None => &[],
        Layout::Quad => &[
            s!("BOUNDS"),
            s!("CLIP"),
            s!("SHAPE"),
            s!("FILL"),
            s!("BORDER"),
            s!("GRADIENT"),
            s!("STOPA"),
            s!("STOPB"),
            s!("STOPOFFSETS"),
            s!("XFORM"),
        ],
        Layout::Shadow => &[
            s!("BOUNDS"),
            s!("CLIP"),
            s!("SHAPE"),
            s!("OFFSET"),
            s!("COLOR"),
        ],
        Layout::Glyph => &[s!("BOUNDS"), s!("TEXELS"), s!("CLIP"), s!("COLOR")],
        Layout::Image => &[
            s!("BOUNDS"),
            s!("TEXELS"),
            s!("CLIP"),
            s!("PARAMS"),
            s!("XFORM"),
        ],
        Layout::Slug => &[
            s!("PLACEMENT"),
            s!("GLYPH"),
            s!("EMBOUNDS"),
            s!("CLIP"),
            s!("COLOR"),
        ],
    };
    names
        .iter()
        .enumerate()
        .map(|(index, name)| element(*name, index as u32))
        .collect()
}

fn blend_desc(blending: Blending) -> D3D11_BLEND_DESC {
    let (destination, destination_alpha) = match blending {
        Blending::Dual => (D3D11_BLEND_INV_SRC1_COLOR, D3D11_BLEND_INV_SRC1_ALPHA),
        _ => (D3D11_BLEND_INV_SRC_ALPHA, D3D11_BLEND_INV_SRC_ALPHA),
    };
    let mut desc = D3D11_BLEND_DESC::default();
    desc.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
        BlendEnable: (blending != Blending::Opaque).into(),
        SrcBlend: D3D11_BLEND_ONE,
        DestBlend: destination,
        BlendOp: D3D11_BLEND_OP_ADD,
        SrcBlendAlpha: D3D11_BLEND_ONE,
        DestBlendAlpha: destination_alpha,
        BlendOpAlpha: D3D11_BLEND_OP_ADD,
        RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
    };
    desc
}

fn bytecode(name: &'static str) -> Result<&'static [u8], Error> {
    aqua_shaders::find_dxbc(name)
        .map(|blob| blob.bytes)
        .ok_or(Error::MissingShader(name))
}

#[derive(Clone)]
pub struct Program {
    pub vertex: ID3D11VertexShader,
    pub pixel: ID3D11PixelShader,
    pub layout: Option<ID3D11InputLayout>,
    pub blend: Blending,
}

struct Built {
    program: Program,
    used: bool,
    first_frame: bool,
    _token: Token,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PipelineStats {
    pub compiled: u32,
    pub loaded: u32,
    pub late: u32,
}

pub struct States {
    pub opaque: ID3D11BlendState,
    pub over: ID3D11BlendState,
    pub dual: ID3D11BlendState,
    pub rasterizer: ID3D11RasterizerState,
    pub sampler: ID3D11SamplerState,
}

impl States {
    pub fn blend(&self, blending: Blending) -> &ID3D11BlendState {
        match blending {
            Blending::Opaque => &self.opaque,
            Blending::Over => &self.over,
            Blending::Dual => &self.dual,
        }
    }
}

fn blend_state(device: &ID3D11Device, blending: Blending) -> Result<ID3D11BlendState, Error> {
    let mut state = None;
    unsafe { device.CreateBlendState(&blend_desc(blending), Some(&mut state))? };
    state.ok_or(Error::NoAdapter)
}

fn create_states(device: &ID3D11Device) -> Result<States, Error> {
    let mut rasterizer = None;
    unsafe {
        device.CreateRasterizerState(
            &D3D11_RASTERIZER_DESC {
                FillMode: D3D11_FILL_SOLID,
                CullMode: D3D11_CULL_NONE,
                FrontCounterClockwise: false.into(),
                DepthBias: 0,
                DepthBiasClamp: 0.0,
                SlopeScaledDepthBias: 0.0,
                DepthClipEnable: true.into(),
                ScissorEnable: true.into(),
                MultisampleEnable: false.into(),
                AntialiasedLineEnable: false.into(),
            },
            Some(&mut rasterizer),
        )?
    };
    let mut sampler = None;
    unsafe {
        device.CreateSamplerState(
            &D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                MipLODBias: 0.0,
                MaxAnisotropy: 1,
                ComparisonFunc: D3D11_COMPARISON_NEVER,
                BorderColor: [0.0; 4],
                MinLOD: 0.0,
                MaxLOD: D3D11_FLOAT32_MAX,
            },
            Some(&mut sampler),
        )?
    };
    Ok(States {
        opaque: blend_state(device, Blending::Opaque)?,
        over: blend_state(device, Blending::Over)?,
        dual: blend_state(device, Blending::Dual)?,
        rasterizer: rasterizer.ok_or(Error::NoAdapter)?,
        sampler: sampler.ok_or(Error::NoAdapter)?,
    })
}

fn label(kind: Kind) -> String {
    format!("{kind:?}")
}

fn first_frame_label(kind: Kind) -> String {
    format!("+{kind:?}")
}

pub struct Pipelines {
    device: ID3D11Device,
    pub states: States,
    vertex: HashMap<&'static str, (ID3D11VertexShader, Option<ID3D11InputLayout>)>,
    pixel: HashMap<&'static str, ID3D11PixelShader>,
    built: BTreeMap<Kind, Built>,
    stats: PipelineStats,
    warmed: bool,
    startup_open: bool,
    late: Vec<Kind>,
    hint: Option<BTreeSet<String>>,
}

impl Pipelines {
    pub fn create(device: &ID3D11Device, cache: Option<&[u8]>) -> Result<Self, Error> {
        let (hint, _) = super::hint::decode(cache);
        Ok(Self {
            device: device.clone(),
            states: create_states(device)?,
            vertex: HashMap::new(),
            pixel: HashMap::new(),
            built: BTreeMap::new(),
            stats: PipelineStats::default(),
            warmed: false,
            startup_open: true,
            late: Vec::new(),
            hint: hint.filter(|names| names.iter().all(|name| is_own_label(name))),
        })
    }

    fn vertex_shader(
        &mut self,
        name: &'static str,
        layout: Layout,
    ) -> Result<(ID3D11VertexShader, Option<ID3D11InputLayout>), Error> {
        if let Some(found) = self.vertex.get(name) {
            return Ok(found.clone());
        }
        let bytes = bytecode(name)?;
        let mut shader = None;
        unsafe {
            self.device
                .CreateVertexShader(bytes, None, Some(&mut shader))?
        };
        let shader = shader.ok_or(Error::NoAdapter)?;
        let layout = match layout {
            Layout::None => None,
            layout => {
                let mut created = None;
                unsafe {
                    self.device
                        .CreateInputLayout(&elements(layout), bytes, Some(&mut created))?
                };
                Some(created.ok_or(Error::NoAdapter)?)
            }
        };
        self.vertex.insert(name, (shader.clone(), layout.clone()));
        Ok((shader, layout))
    }

    fn pixel_shader(&mut self, name: &'static str) -> Result<ID3D11PixelShader, Error> {
        if let Some(found) = self.pixel.get(name) {
            return Ok(found.clone());
        }
        let mut shader = None;
        unsafe {
            self.device
                .CreatePixelShader(bytecode(name)?, None, Some(&mut shader))?
        };
        let shader = shader.ok_or(Error::NoAdapter)?;
        self.pixel.insert(name, shader.clone());
        Ok(shader)
    }

    fn build(&mut self, kind: Kind) -> Result<Program, Error> {
        let shaders = shaders(kind);
        let (vertex, layout) = self.vertex_shader(shaders.vertex, shaders.layout)?;
        let pixel = self.pixel_shader(shaders.pixel)?;
        self.stats.compiled += 1;
        Ok(Program {
            vertex,
            pixel,
            layout,
            blend: shaders.blend,
        })
    }

    fn insert(&mut self, kind: Kind, program: Program, used: bool) {
        self.built.insert(
            kind,
            Built {
                program,
                used,
                first_frame: used && self.startup_open,
                _token: record("shader_program", LedgerKind::Shader, 0),
            },
        );
    }

    pub fn get(&mut self, kind: Kind) -> Result<Program, Error> {
        let startup_open = self.startup_open;
        if !self.built.contains_key(&kind) {
            let program = self.build(kind)?;
            if self.warmed {
                self.stats.late += 1;
                self.late.push(kind);
            }
            self.insert(kind, program, true);
        }
        let found = self.built.get_mut(&kind).ok_or(Error::NoAdapter)?;
        found.used = true;
        found.first_frame |= startup_open;
        Ok(found.program.clone())
    }

    pub fn warm(&mut self) -> Result<(), Error> {
        let wanted: Vec<Kind> = Kind::ALL
            .into_iter()
            .filter(|kind| !self.built.contains_key(kind) && self.hinted(*kind))
            .collect();
        self.warm_kinds(&wanted)
    }

    pub fn warm_first_frame(&mut self) -> Result<(), Error> {
        let wanted: Vec<Kind> = Kind::ALL
            .into_iter()
            .filter(|kind| !self.built.contains_key(kind) && self.wanted_at_startup(*kind))
            .collect();
        self.warm_kinds(&wanted)
    }

    fn warm_kinds(&mut self, kinds: &[Kind]) -> Result<(), Error> {
        for kind in kinds {
            let stages = crate::stages::StageClock::from_env();
            let program = self.build(*kind)?;
            self.insert(*kind, program, false);
            stages.mark(&format!("      shader {kind:?}"));
        }
        self.warmed = true;
        Ok(())
    }

    fn wanted_at_startup(&self, kind: Kind) -> bool {
        match &self.hint {
            None => FIRST_FRAME.contains(&kind),
            Some(hint) => kind == Kind::Quad || hint.contains(&first_frame_label(kind)),
        }
    }

    fn hinted(&self, kind: Kind) -> bool {
        self.hint.as_ref().is_none_or(|hint| {
            hint.contains(&label(kind)) || hint.contains(&first_frame_label(kind))
        })
    }

    pub fn close_startup(&mut self) {
        self.startup_open = false;
    }

    pub fn deferred(&self) -> Vec<Kind> {
        Kind::ALL
            .into_iter()
            .filter(|kind| !self.built.contains_key(kind) && self.hinted(*kind))
            .collect()
    }

    pub fn warm_one(&mut self, kind: Kind) -> Result<(), Error> {
        if self.built.contains_key(&kind) {
            return Ok(());
        }
        let program = self.build(kind)?;
        self.insert(kind, program, false);
        Ok(())
    }

    pub fn late(&self) -> &[Kind] {
        &self.late
    }

    pub fn stats(&self) -> PipelineStats {
        self.stats
    }

    fn used_labels(&self) -> BTreeSet<String> {
        let mut merged: BTreeMap<String, bool> = BTreeMap::new();
        let mut note = |name: String, first: bool| {
            *merged.entry(name).or_insert(false) |= first;
        };
        self.hint
            .iter()
            .flatten()
            .for_each(|name| match name.strip_prefix('+') {
                Some(bare) => note(bare.to_owned(), true),
                None => note(name.clone(), false),
            });
        self.built
            .iter()
            .filter(|(_, built)| built.used)
            .for_each(|(kind, built)| note(label(*kind), built.first_frame));
        merged
            .into_iter()
            .map(|(name, first)| match first {
                true => format!("+{name}"),
                false => name,
            })
            .collect()
    }

    pub fn serialize(&self) -> Option<Vec<u8>> {
        Some(super::hint::encode(&self.used_labels(), &[]))
    }
}

fn is_own_label(name: &str) -> bool {
    let bare = name.strip_prefix('+').unwrap_or(name);
    Kind::ALL.iter().any(|kind| label(*kind) == bare)
}
