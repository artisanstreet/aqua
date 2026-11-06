use aqua_core::geometry::Rect;
use aqua_graph::{Extent, PaintCommand, Shadow, ShadowCast};
use aqua_protocol::SurfacePixels;

use super::cases::{backdrop_blur, build, hex, quad, stripes, Case};

pub const GLASS_EXTENT: Extent = Extent {
    width: 320,
    height: 256,
};

const BLUR_RADIUS: f32 = 24.0;
const CORNER_RADIUS: f32 = 22.0;
const SMALL: Rect = Rect {
    x: 40.0,
    y: 40.0,
    w: 200.0,
    h: 64.0,
};

fn large(extent: Extent) -> Rect {
    Rect {
        x: 12.0,
        y: 12.0,
        w: extent.width as f32 - 24.0,
        h: extent.height as f32 - 24.0,
    }
}

#[derive(Clone, Copy)]
enum Wrap {
    Bare,
    Rounded,
    Group,
    RoundedInGroup,
}

pub struct GlassCase {
    pub case: Case,
    pub glass: Rect,
}

fn glass_commands(extent: Extent, glass: Rect, wrap: Wrap) -> Vec<PaintCommand> {
    let pane = [
        backdrop_blur(glass, BLUR_RADIUS, CORNER_RADIUS),
        quad(
            glass.x,
            glass.y,
            glass.w,
            glass.h,
            "#202020",
            0.3,
            CORNER_RADIUS,
            0.0,
        ),
        quad(
            glass.x + 8.0,
            glass.y + 8.0,
            glass.w - 16.0,
            glass.h - 16.0,
            "#ffffff",
            0.04,
            CORNER_RADIUS - 8.0,
            0.0,
        ),
    ];
    let rounded = PaintCommand::PushRoundedClip {
        rect: glass,
        radius: CORNER_RADIUS,
    };
    let mut commands = stripes(extent);
    match wrap {
        Wrap::Bare => commands.extend(pane),
        Wrap::Rounded => {
            commands.push(rounded);
            commands.extend(pane);
            commands.push(PaintCommand::PopClip);
        }
        Wrap::Group => {
            commands.push(PaintCommand::PushOpacity(0.97));
            commands.extend(pane);
            commands.push(PaintCommand::PopOpacity);
        }
        Wrap::RoundedInGroup => {
            commands.push(PaintCommand::PushOpacity(0.97));
            commands.push(rounded);
            commands.extend(pane);
            commands.push(PaintCommand::PopClip);
            commands.push(PaintCommand::PopOpacity);
        }
    }
    commands
}

pub fn glass_cases() -> Vec<GlassCase> {
    glass_cases_in(GLASS_EXTENT)
}

pub fn glass_cases_in(extent: Extent) -> Vec<GlassCase> {
    [
        ("glass_bare_small", SMALL, Wrap::Bare),
        ("glass_rounded_small", SMALL, Wrap::Rounded),
        ("glass_rounded_large", large(extent), Wrap::Rounded),
        ("glass_group_small", SMALL, Wrap::Group),
        ("glass_rounded_in_group_small", SMALL, Wrap::RoundedInGroup),
        (
            "glass_rounded_in_group_large",
            large(extent),
            Wrap::RoundedInGroup,
        ),
    ]
    .into_iter()
    .map(|(name, glass, wrap)| GlassCase {
        case: build(
            name,
            extent,
            hex("#000000", 1.0),
            &glass_commands(extent, glass, wrap),
        ),
        glass,
    })
    .collect()
}

pub struct RimPair {
    pub bare: Case,
    pub rimmed: Case,
    pub glass: Rect,
}

fn rim(glass: Rect, offset_y: f32, color: &str, alpha: f32, sigma: f32) -> PaintCommand {
    PaintCommand::Shadow(Shadow {
        bounds: glass,
        radius: CORNER_RADIUS,
        offset: [0.0, offset_y],
        sigma,
        spread: 0.0,
        color: hex(color, alpha),
        cast: ShadowCast::Outside,
    })
}

pub fn rim_pair() -> RimPair {
    let glass = SMALL;
    let mut rimmed = stripes(GLASS_EXTENT);
    rimmed.extend([
        rim(glass, -1.0, "#ffffff", 0.14, 0.0),
        rim(glass, 8.0, "#000000", 0.5, 12.0),
    ]);
    rimmed.extend(
        glass_commands(GLASS_EXTENT, glass, Wrap::Bare).split_off(stripes(GLASS_EXTENT).len()),
    );
    RimPair {
        bare: build(
            "glass_without_drop_shadows",
            GLASS_EXTENT,
            hex("#000000", 1.0),
            &glass_commands(GLASS_EXTENT, glass, Wrap::Bare),
        ),
        rimmed: build(
            "glass_over_its_own_drop_shadows",
            GLASS_EXTENT,
            hex("#000000", 1.0),
            &rimmed,
        ),
        glass,
    }
}

pub fn interior_difference(bare: &SurfacePixels, rimmed: &SurfacePixels, glass: Rect) -> u8 {
    let width = bare.extent.width as usize;
    let inset = CORNER_RADIUS + BLUR_RADIUS;
    let rows = (glass.y + 2.0) as usize..(glass.bottom() - 2.0) as usize;
    let columns = (glass.x + inset) as usize..(glass.right() - inset) as usize;
    rows.flat_map(|y| columns.clone().map(move |x| (y * width + x) * 4))
        .flat_map(|index| (0..3).map(move |channel| index + channel))
        .map(|index| bare.rgba8[index].abs_diff(rimmed.rgba8[index]))
        .max()
        .unwrap_or(0)
}

pub fn cases() -> Vec<Case> {
    let pair = rim_pair();
    glass_cases()
        .into_iter()
        .map(|glass| glass.case)
        .chain([pair.bare, pair.rimmed])
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassProfile {
    pub spread: u8,
    pub steepest: u8,
}

pub const MIN_SPREAD: u8 = 12;
pub const MAX_STEP: u8 = 16;

pub fn profile(pixels: &SurfacePixels, glass: Rect, row: u32) -> GlassProfile {
    let width = pixels.extent.width as usize;
    let from = (glass.x + CORNER_RADIUS) as usize;
    let to = (glass.right() - CORNER_RADIUS) as usize;
    let values: Vec<u8> = (from..to)
        .map(|x| pixels.rgba8[(row as usize * width + x) * 4])
        .collect();
    let spread =
        values.iter().max().copied().unwrap_or(0) - values.iter().min().copied().unwrap_or(0);
    let steepest = values
        .windows(2)
        .map(|pair| pair[0].abs_diff(pair[1]))
        .max()
        .unwrap_or(0);
    GlassProfile { spread, steepest }
}

pub fn column_steepest(pixels: &SurfacePixels, glass: Rect) -> u8 {
    let width = pixels.extent.width as usize;
    let x = (glass.x + glass.w / 2.0) as usize;
    let values: Vec<u8> = ((glass.y + 3.0) as usize..(glass.bottom() - 3.0) as usize)
        .map(|y| pixels.rgba8[(y * width + x) * 4])
        .collect();
    values
        .windows(2)
        .map(|pair| pair[0].abs_diff(pair[1]))
        .max()
        .unwrap_or(0)
}

pub fn rows(glass: Rect) -> [u32; 3] {
    let middle = glass.y + glass.h / 2.0;
    [
        (glass.y + CORNER_RADIUS / 2.0) as u32,
        middle as u32,
        (glass.bottom() - CORNER_RADIUS / 2.0) as u32,
    ]
}

pub fn blurred(profile: GlassProfile) -> bool {
    profile.spread >= MIN_SPREAD && profile.steepest <= MAX_STEP
}
