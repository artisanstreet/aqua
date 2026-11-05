#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdapterPreference {
    HighPerformance,
    MinimumPower,
    Warp,
    Luid(i64),
}

pub fn parse_preference(text: &str) -> Option<AdapterPreference> {
    match text.trim().to_ascii_lowercase().as_str() {
        "warp" | "software" => Some(AdapterPreference::Warp),
        "high" | "high-performance" | "discrete" => Some(AdapterPreference::HighPerformance),
        "low" | "min-power" | "minimum-power" | "integrated" => {
            Some(AdapterPreference::MinimumPower)
        }
        other => other
            .strip_prefix("luid:")
            .and_then(|digits| i64::from_str_radix(digits.trim_start_matches("0x"), 16).ok())
            .map(AdapterPreference::Luid),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Adapter {
    Hardware,
    Software,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterInfo {
    pub name: String,
    pub luid: i64,
    pub vendor_id: u32,
    pub kind: Adapter,
    pub dedicated_video_bytes: u64,
    pub shared_system_bytes: u64,
}
