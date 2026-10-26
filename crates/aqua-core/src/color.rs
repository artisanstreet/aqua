//! Canonical public color model: OKLCH with alpha.
//!
//! This is the single authoring interchange type. External formats (hex sRGB,
//! display P3-approximated sRGB, HSL) convert here at import boundaries; GPU
//! working values (premultiplied linear light) convert at the render
//! boundary via [`Oklch::to_gpu`]. The conversion math was reviewed against
//! `aqua-render`'s prototype: identical matrices and transfer curve.

// The sRGB/OKLab matrices below are the published constants (IEC 61966-2-1,
// Björn Ottosson's OKLab derivation). Their decimal digits exceed f32
// precision by design; storage rounds to the nearest representable value.
#![allow(clippy::excessive_precision)]

/// Authored UI color: OKLCH + alpha.
///
/// Ranges: `l` 0..=1 (perceptual lightness), `c` >= 0 (chroma),
/// `h` degrees 0..360 (hue angle), `alpha` 0..=1.
///
/// `Eq` is deliberately absent: hue is meaningless at near-zero chroma, so
/// bitwise equality would reject visually identical colors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Oklch {
    /// Perceptual lightness, 0 (black) .. 1 (white).
    pub l: f32,
    /// Chroma, >= 0.
    pub c: f32,
    /// Hue angle in degrees, normalized to 0..360.
    pub h: f32,
    /// Opacity, 0 (transparent) .. 1 (opaque).
    pub alpha: f32,
}

impl Oklch {
    /// Opaque black.
    pub const BLACK: Oklch = Oklch {
        l: 0.0,
        c: 0.0,
        h: 0.0,
        alpha: 1.0,
    };
    /// Opaque white.
    pub const WHITE: Oklch = Oklch {
        l: 1.0,
        c: 0.0,
        h: 0.0,
        alpha: 1.0,
    };
    /// Fully transparent (all channels zero).
    pub const TRANSPARENT: Oklch = Oklch {
        l: 0.0,
        c: 0.0,
        h: 0.0,
        alpha: 0.0,
    };

    /// Construct an opaque color, including in a constant style.
    /// Chroma is nonnegative and hue is normalized to 0..360 degrees.
    #[must_use]
    pub const fn new(l: f32, c: f32, h_deg: f32) -> Self {
        let h = h_deg % 360.0;
        Self {
            l,
            c: if c > 0.0 { c } else { 0.0 },
            h: if h < 0.0 { h + 360.0 } else { h },
            alpha: 1.0,
        }
    }

    /// Set alpha, clamped to 0..=1, including in constant styles.
    #[must_use]
    pub const fn with_alpha(mut self, alpha: f32) -> Self {
        self.alpha = if alpha < 0.0 {
            0.0
        } else if alpha > 1.0 {
            1.0
        } else {
            alpha
        };
        self
    }

    /// Construct directly with alpha. Hue is normalized to 0..360;
    /// alpha is clamped. Retained for compatibility with color imports.
    #[allow(clippy::self_named_constructors)]
    #[must_use]
    pub fn oklch(l: f32, c: f32, h_deg: f32, alpha: f32) -> Self {
        Self {
            l,
            c: c.max(0.0),
            h: h_deg.rem_euclid(360.0),
            alpha: alpha.clamp(0.0, 1.0),
        }
    }

    /// Parse `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa` as sRGB and convert.
    /// Returns `None` on malformed input; never panics (ASCII is validated
    /// before any byte slicing, so non-ASCII input is `None`, not a panic).
    #[must_use]
    pub fn from_hex(hex: &str) -> Option<Self> {
        let h = hex.strip_prefix('#')?;
        if !h.is_ascii() {
            return None;
        }
        let bytes: Vec<u8> = match h.len() {
            3 | 4 => h
                .chars()
                .map(|ch| ch.to_digit(16).map(|d| (d * 17) as u8))
                .collect::<Option<Vec<u8>>>()?,
            6 | 8 => (0..h.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&h[i..i + 2], 16).ok())
                .collect::<Option<Vec<u8>>>()?,
            _ => return None,
        };
        let (r, g, b, a) = match bytes.len() {
            3 => (bytes[0], bytes[1], bytes[2], 255),
            4 => (bytes[0], bytes[1], bytes[2], bytes[3]),
            6 => (bytes[0], bytes[1], bytes[2], 255),
            8 => (bytes[0], bytes[1], bytes[2], bytes[3]),
            _ => return None,
        };
        Some(Self::from_srgb8(r, g, b, f32::from(a) / 255.0))
    }

    /// Convert 8-bit sRGB + float alpha into the canonical model.
    #[must_use]
    pub fn from_srgb8(r: u8, g: u8, b: u8, alpha: f32) -> Self {
        let lin = srgb8_to_linear([r, g, b]);
        let (l, a, b2) = linear_srgb_to_oklab(lin);
        let (c, h) = oklab_to_lch(a, b2);
        Self::oklch(l, c, h, alpha)
    }

    /// Convert HSL to the canonical model. Convention: `h` degrees 0..360,
    /// `s`/`l` 0..=1, CSS Color 4 semantics (gray when `s == 0`).
    #[must_use]
    pub fn from_hsl(h_deg: f32, s: f32, l: f32, alpha: f32) -> Self {
        let h = h_deg.rem_euclid(360.0) / 360.0;
        let s = s.clamp(0.0, 1.0);
        let l = l.clamp(0.0, 1.0);
        let (r, g, b) = if s == 0.0 {
            (l, l, l)
        } else {
            let q = if l < 0.5 {
                l * (1.0 + s)
            } else {
                l + s - l * s
            };
            let p = 2.0 * l - q;
            let channel = |t: f32| {
                let t = t.rem_euclid(1.0);
                if t < 1.0 / 6.0 {
                    p + (q - p) * 6.0 * t
                } else if t < 0.5 {
                    q
                } else if t < 2.0 / 3.0 {
                    p + (q - p) * (2.0 / 3.0 - t) * 6.0
                } else {
                    p
                }
            };
            (channel(h + 1.0 / 3.0), channel(h), channel(h - 1.0 / 3.0))
        };
        let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        Self::from_srgb8(to8(r), to8(g), to8(b), alpha)
    }

    /// Render-boundary value: premultiplied linear-light sRGB + alpha, ready
    /// for shaders and GPU composition.
    #[must_use]
    pub fn to_gpu(self) -> [f32; 4] {
        let (a, b) = lch_to_oklab(self.c, self.h);
        let lin = oklab_to_linear_srgb(self.l, a, b);
        let alpha = self.alpha.clamp(0.0, 1.0);
        [lin[0] * alpha, lin[1] * alpha, lin[2] * alpha, alpha]
    }

    /// Non-premultiplied linear-light sRGB + alpha (readback/comparison).
    #[must_use]
    pub fn to_linear(self) -> [f32; 4] {
        let (a, b) = lch_to_oklab(self.c, self.h);
        let lin = oklab_to_linear_srgb(self.l, a, b);
        [lin[0], lin[1], lin[2], self.alpha.clamp(0.0, 1.0)]
    }

    /// Quantized 8-bit sRGB + float alpha (exports, snapshots, tests).
    /// Repeated round-trips through this function lose precision; preserve
    /// the original [`Oklch`] when exact re-editing is required.
    #[must_use]
    pub fn to_srgb8(self) -> [u8; 4] {
        let (a, b) = lch_to_oklab(self.c, self.h);
        let lin = oklab_to_linear_srgb(self.l, a, b);
        let [r, g, b] = linear_to_srgb8(lin);
        let alpha = (self.alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
        [r, g, b, alpha]
    }

    /// Interpolate toward `other`. Lightness/chroma/alpha lerp linearly; hue
    /// follows the shortest angular path. Neutral endpoints (chroma ~ 0)
    /// keep the non-neutral hue so gradients avoid arbitrary hue jumps.
    #[must_use]
    pub fn lerp(self, other: Oklch, t: f32) -> Oklch {
        let t = t.clamp(0.0, 1.0);
        let h0 = if self.c < 1e-4 { other.h } else { self.h };
        let h1 = if other.c < 1e-4 { self.h } else { other.h };
        let mut dh = (h1 - h0).rem_euclid(360.0);
        if dh > 180.0 {
            dh -= 360.0;
        }
        Oklch::oklch(
            self.l + (other.l - self.l) * t,
            self.c + (other.c - self.c) * t,
            h0 + dh * t,
            self.alpha + (other.alpha - self.alpha) * t,
        )
    }

    /// Serialize as `#rrggbb` (opaque) or `#rrggbbaa` (translucent).
    #[must_use]
    pub fn to_hex(self) -> String {
        let [r, g, b, a] = self.to_srgb8();
        if a == 255 {
            format!("#{r:02x}{g:02x}{b:02x}")
        } else {
            format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
        }
    }
}

/// sRGB bytes to linear light (IEC 61966-2-1 transfer curve).
#[must_use]
pub fn srgb8_to_linear([r, g, b]: [u8; 3]) -> [f32; 3] {
    [r, g, b].map(|v| {
        let s = f32::from(v) / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    })
}

/// Linear light to sRGB bytes.
#[must_use]
pub fn linear_to_srgb8([r, g, b]: [f32; 3]) -> [u8; 3] {
    [r, g, b].map(|v| {
        let v = v.clamp(0.0, 1.0);
        let s = if v <= 0.0031308 {
            v * 12.92
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        };
        (s * 255.0).round().clamp(0.0, 255.0) as u8
    })
}

fn linear_srgb_to_oklab([r, g, b]: [f32; 3]) -> (f32, f32, f32) {
    let l = 0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b;
    let m = 0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b;
    let s = 0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b;
    let (l_, m_, s_) = (l.cbrt(), m.cbrt(), s.cbrt());
    (
        0.2104542553 * l_ + 0.7936177850 * m_ - 0.0040720468 * s_,
        1.9779984951 * l_ - 2.4285922050 * m_ + 0.4505937099 * s_,
        0.0259040371 * l_ + 0.7827717662 * m_ - 0.8086757660 * s_,
    )
}

fn oklab_to_linear_srgb(l: f32, a: f32, b: f32) -> [f32; 3] {
    let (l_, m_, s_) = (
        l + 0.3963377774 * a + 0.2158037573 * b,
        l - 0.1055613458 * a - 0.0638541728 * b,
        l - 0.0894841775 * a - 1.2914855480 * b,
    );
    let (l2, m2, s2) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    [
        4.0767416621 * l2 - 3.3077115913 * m2 + 0.2309699292 * s2,
        -1.2684380046 * l2 + 2.6097574011 * m2 - 0.3413193965 * s2,
        -0.0041960863 * l2 - 0.7034186147 * m2 + 1.7076147010 * s2,
    ]
}

fn oklab_to_lch(a: f32, b: f32) -> (f32, f32) {
    (a.hypot(b), b.atan2(a).to_degrees().rem_euclid(360.0))
}

fn lch_to_oklab(c: f32, h_deg: f32) -> (f32, f32) {
    let h = h_deg.to_radians();
    (c * h.cos(), c * h.sin())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn const_color_construction_normalizes_hue_and_alpha() {
        const COLOR: Oklch = Oklch::new(0.5, 0.12, -30.0).with_alpha(0.5);
        assert_eq!(COLOR, Oklch::oklch(0.5, 0.12, -30.0, 0.5));
        const CLAMPED: Oklch = Oklch::new(0.5, -0.1, 750.0).with_alpha(2.0);
        assert_eq!(CLAMPED, Oklch::oklch(0.5, -0.1, 750.0, 2.0));
        assert_eq!(COLOR.with_alpha(-1.0).alpha, 0.0);
    }

    #[test]
    fn hex_roundtrip_primary_colors() {
        for (hex, expect) in [
            ("#ff0000", [255, 0, 0]),
            ("#00ff00", [0, 255, 0]),
            ("#0000ff", [0, 0, 255]),
            ("#ffffff", [255, 255, 255]),
            ("#000000", [0, 0, 0]),
            ("#f00", [255, 0, 0]),
        ] {
            let c = Oklch::from_hex(hex).expect("valid hex");
            let [r, g, b] = linear_to_srgb8([c.to_linear()[0], c.to_linear()[1], c.to_linear()[2]]);
            assert_eq!([r, g, b], expect, "hex {hex}");
        }
    }

    #[test]
    fn hex_malformed_returns_none_never_panics() {
        for bad in [
            "",
            "red",
            "#gg0000",
            "#12345",
            "#1234567",
            "ff0000",
            "#",
            // Non-ASCII must not panic on byte slicing.
            "#ééé",
            "#üüüüüü",
            "#日本語日本",
            "#ff00\u{00e9}0",
        ] {
            assert!(Oklch::from_hex(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn shorthand_alpha_and_full_alpha() {
        let short = Oklch::from_hex("#f008").unwrap();
        let full = Oklch::from_hex("#ff000088").unwrap();
        assert!((short.alpha - full.alpha).abs() < 0.01);
        assert_eq!(short.to_srgb8()[0], 255);
    }

    #[test]
    fn neutral_has_near_zero_chroma() {
        let gray = Oklch::from_hex("#808080").unwrap();
        assert!(gray.c < 0.005, "chroma = {}", gray.c);
    }

    #[test]
    fn hsl_convention_matches_css_primaries() {
        let red = Oklch::from_hsl(0.0, 1.0, 0.5, 1.0);
        assert_eq!(red.to_srgb8()[0..3], [255, 0, 0]);
        let green = Oklch::from_hsl(120.0, 1.0, 0.5, 1.0);
        assert_eq!(green.to_srgb8()[0..3], [0, 255, 0]);
        let blue = Oklch::from_hsl(240.0, 1.0, 0.5, 1.0);
        assert_eq!(blue.to_srgb8()[0..3], [0, 0, 255]);
        // Hue wraps: 360 == 0, negative hues wrap forward.
        assert_eq!(
            Oklch::from_hsl(360.0, 1.0, 0.5, 1.0).to_srgb8()[0..3],
            [255, 0, 0]
        );
        assert_eq!(
            Oklch::from_hsl(-120.0, 1.0, 0.5, 1.0).to_srgb8()[0..3],
            [0, 0, 255]
        );
        // Lightness extremes are black/white regardless of hue/saturation.
        assert_eq!(
            Oklch::from_hsl(200.0, 0.8, 0.0, 1.0).to_srgb8()[0..3],
            [0, 0, 0]
        );
        assert_eq!(
            Oklch::from_hsl(200.0, 0.8, 1.0, 1.0).to_srgb8()[0..3],
            [255, 255, 255]
        );
        let gray = Oklch::from_hsl(123.0, 0.0, 0.5, 1.0);
        let [r, g, b, _] = gray.to_srgb8();
        assert_eq!([r, g, b], [128, 128, 128]);
        assert!(gray.c < 0.01);
    }

    #[test]
    fn translucent_output_is_premultiplied() {
        let c = Oklch::from_hex("#ff0000").unwrap();
        let half = Oklch { alpha: 0.5, ..c };
        let gpu = half.to_gpu();
        assert!((gpu[3] - 0.5).abs() < 1e-6);
        let full = c.to_gpu();
        assert!((gpu[0] - full[0] * 0.5).abs() < 1e-5);
    }

    #[test]
    fn lerp_takes_short_hue_path() {
        let a = Oklch::oklch(0.6, 0.1, 10.0, 1.0);
        let b = Oklch::oklch(0.6, 0.1, 350.0, 1.0);
        let mid = a.lerp(b, 0.5);
        // Shortest path from 10° to 350° passes through 0°, not 180°.
        assert!(mid.h < 1.0 || mid.h > 359.0, "h = {}", mid.h);
    }

    #[test]
    fn hex_serialization_roundtrip() {
        let c = Oklch::from_hex("#1a2b3c").unwrap();
        assert_eq!(c.to_hex(), "#1a2b3c");
    }
}
