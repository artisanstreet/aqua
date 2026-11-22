use std::time::Duration;

use aqua_core::{ClickSettings, Point};
use aqua_protocol::prefs::{PrefState, PreferenceSource, SystemPrefs, TextScale};
use aqua_protocol::ColorScheme;

pub(crate) const NAMESPACES: &[&str] = &[
    "org.freedesktop.appearance",
    "org.gnome.desktop.interface",
    "org.gnome.desktop.a11y.interface",
    "org.gnome.desktop.peripherals.mouse",
    "org.kde.kdeglobals.KDE",
];

const DEFAULT_DOUBLE_CLICK: Duration = Duration::from_millis(400);
const DOUBLE_CLICK_DISTANCE: f32 = 5.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Key {
    ColorScheme,
    Contrast,
    ReducedMotion,
    EnableAnimations,
    HighContrast,
    TextScaling,
    GnomeDoubleClick,
    KdeDoubleClick,
}

impl Key {
    pub(crate) fn parse(namespace: &str, name: &str) -> Option<Key> {
        match (namespace, name) {
            ("org.freedesktop.appearance", "color-scheme") => Some(Key::ColorScheme),
            ("org.freedesktop.appearance", "contrast") => Some(Key::Contrast),
            ("org.freedesktop.appearance", "reduced-motion") => Some(Key::ReducedMotion),
            ("org.gnome.desktop.interface", "enable-animations") => Some(Key::EnableAnimations),
            ("org.gnome.desktop.interface", "text-scaling-factor") => Some(Key::TextScaling),
            ("org.gnome.desktop.a11y.interface", "high-contrast") => Some(Key::HighContrast),
            ("org.gnome.desktop.peripherals.mouse", "double-click") => Some(Key::GnomeDoubleClick),
            ("org.kde.kdeglobals.KDE", "DoubleClickInterval") => Some(Key::KdeDoubleClick),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Reading {
    Unsigned(u64),
    Signed(i64),
    Float(f64),
    Bool(bool),
    Text(String),
}

impl Reading {
    fn unsigned(&self) -> Option<u64> {
        match self {
            Reading::Unsigned(value) => Some(*value),
            Reading::Signed(value) => u64::try_from(*value).ok(),
            Reading::Text(text) => text.trim().parse().ok(),
            Reading::Float(_) | Reading::Bool(_) => None,
        }
    }

    fn float(&self) -> Option<f64> {
        match self {
            Reading::Float(value) => Some(*value),
            Reading::Text(text) => text.trim().parse().ok(),
            Reading::Unsigned(_) | Reading::Signed(_) | Reading::Bool(_) => None,
        }
    }

    fn flag(&self) -> Option<bool> {
        match self {
            Reading::Bool(value) => Some(*value),
            Reading::Text(text) => match text.trim() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            },
            Reading::Unsigned(_) | Reading::Signed(_) | Reading::Float(_) => None,
        }
    }

    fn preference(&self) -> Option<PrefState> {
        match self.unsigned() {
            Some(0) => Some(PrefState::Off),
            Some(1) => Some(PrefState::On),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Desktop {
    scheme: Option<ColorScheme>,
    contrast: Option<PrefState>,
    reduced_motion: Option<PrefState>,
    animations: Option<bool>,
    high_contrast: Option<bool>,
    text_scale: Option<TextScale>,
    double_click: Option<Duration>,
}

fn either(primary: Option<PrefState>, secondary: Option<bool>) -> PrefState {
    match (primary, secondary) {
        (Some(PrefState::On), _) | (_, Some(true)) => PrefState::On,
        (Some(PrefState::Off), _) | (_, Some(false)) => PrefState::Off,
        _ => PrefState::Unknown,
    }
}

impl Desktop {
    pub(crate) fn set(&mut self, key: Key, reading: &Reading) {
        match key {
            Key::ColorScheme => {
                self.scheme = match reading.unsigned() {
                    Some(1) => Some(ColorScheme::Dark),
                    Some(2) => Some(ColorScheme::Light),
                    Some(_) => Some(ColorScheme::Unknown),
                    None => self.scheme,
                }
            }
            Key::Contrast => self.contrast = reading.preference().or(self.contrast),
            Key::ReducedMotion => {
                self.reduced_motion = reading.preference().or(self.reduced_motion)
            }
            Key::EnableAnimations => self.animations = reading.flag().or(self.animations),
            Key::HighContrast => self.high_contrast = reading.flag().or(self.high_contrast),
            Key::TextScaling => {
                self.text_scale = reading
                    .float()
                    .filter(|factor| factor.is_finite() && *factor > 0.0)
                    .map(|factor| TextScale::from_percent((factor * 100.0).round() as u32))
                    .or(self.text_scale)
            }
            Key::GnomeDoubleClick | Key::KdeDoubleClick => {
                self.double_click = reading
                    .unsigned()
                    .filter(|millis| (1..=5_000).contains(millis))
                    .map(Duration::from_millis)
                    .or(self.double_click)
            }
        }
    }

    pub(crate) fn system_prefs(&self) -> SystemPrefs {
        SystemPrefs {
            reduce_motion: either(self.reduced_motion, self.animations.map(|on| !on)),
            reduce_transparency: PrefState::Unknown,
            high_contrast: either(self.contrast, self.high_contrast),
            text_scale: self.text_scale.unwrap_or(TextScale::NORMAL),
            theme: self.scheme.unwrap_or(ColorScheme::Unknown),
            source: PreferenceSource::Portal,
        }
    }

    pub(crate) fn click_settings(&self) -> ClickSettings {
        click_settings(self.double_click.unwrap_or(DEFAULT_DOUBLE_CLICK))
    }
}

pub(crate) fn default_click_settings() -> ClickSettings {
    click_settings(DEFAULT_DOUBLE_CLICK)
}

fn click_settings(interval: Duration) -> ClickSettings {
    ClickSettings::new(
        interval,
        Point::new(DOUBLE_CLICK_DISTANCE, DOUBLE_CLICK_DISTANCE),
    )
    .unwrap_or_else(|| panic!("aqua: {interval:?} is not a valid double-click interval"))
}

pub(crate) fn with_environment(
    system: SystemPrefs,
    lookup: impl Fn(&str) -> Option<String>,
) -> SystemPrefs {
    let overridden = SystemPrefs::fallback().with_overrides(lookup);
    match overridden.source {
        PreferenceSource::EnvOverride => SystemPrefs {
            theme: system.theme,
            ..overridden
        },
        _ => system,
    }
}
