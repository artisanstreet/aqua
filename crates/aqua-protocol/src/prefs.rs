use crate::shell::ColorScheme;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PrefState {
    On,
    Off,
    #[default]
    Unknown,
}

impl PrefState {
    pub fn is_on(self) -> bool {
        matches!(self, PrefState::On)
    }

    pub fn parse_override(text: &str) -> Option<PrefState> {
        match text.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" => Some(PrefState::On),
            "0" | "false" | "no" => Some(PrefState::Off),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreferenceSource {
    EnvOverride,
    WindowsSystem,
    Fallback,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemPrefs {
    pub reduce_motion: PrefState,
    pub reduce_transparency: PrefState,
    pub high_contrast: PrefState,
    pub theme: ColorScheme,
    pub source: PreferenceSource,
}

impl SystemPrefs {
    pub fn fallback() -> Self {
        Self {
            reduce_motion: PrefState::Unknown,
            reduce_transparency: PrefState::Unknown,
            high_contrast: PrefState::Unknown,
            theme: ColorScheme::Unknown,
            source: PreferenceSource::Fallback,
        }
    }

    pub fn with_overrides(mut self, lookup: impl Fn(&str) -> Option<String>) -> Self {
        let mut overridden = false;
        for (slot, name) in [
            (&mut self.reduce_motion, "AQUA_PREF_REDUCED_MOTION"),
            (
                &mut self.reduce_transparency,
                "AQUA_PREF_REDUCED_TRANSPARENCY",
            ),
            (&mut self.high_contrast, "AQUA_PREF_HIGH_CONTRAST"),
        ] {
            if let Some(state) = lookup(name).and_then(|v| PrefState::parse_override(&v)) {
                *slot = state;
                overridden = true;
            }
        }
        if overridden {
            self.source = PreferenceSource::EnvOverride;
        }
        self
    }
}

pub fn should_reduce_motion(prefs: &SystemPrefs) -> bool {
    prefs.reduce_motion.is_on()
}

pub fn prefers_opaque_frame(prefs: &SystemPrefs) -> bool {
    prefs.high_contrast.is_on() || prefs.reduce_transparency.is_on()
}
