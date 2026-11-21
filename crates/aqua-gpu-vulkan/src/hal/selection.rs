#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum AdapterPreference {
    #[default]
    Any,
    Software,
    Hardware,
    Named(String),
}

pub const ADAPTER_VARIABLE: &str = "AQUA_VULKAN_ADAPTER";

pub fn parse_preference(text: &str) -> AdapterPreference {
    match text.trim().to_ascii_lowercase().as_str() {
        "" | "any" => AdapterPreference::Any,
        "software" | "cpu" => AdapterPreference::Software,
        "hardware" | "gpu" => AdapterPreference::Hardware,
        name => AdapterPreference::Named(name.to_owned()),
    }
}

impl AdapterPreference {
    pub fn from_env() -> Self {
        std::env::var(ADAPTER_VARIABLE)
            .map(|text| parse_preference(&text))
            .unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Adapter {
    Discrete,
    Integrated,
    Virtual,
    Software,
    Other,
}

impl Adapter {
    fn rank(self) -> u8 {
        match self {
            Adapter::Discrete => 0,
            Adapter::Integrated => 1,
            Adapter::Virtual => 2,
            Adapter::Other => 3,
            Adapter::Software => 4,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fitness {
    Usable,
    Lacks(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub name: String,
    pub kind: Adapter,
    pub fitness: Fitness,
}

fn wanted(preference: &AdapterPreference, candidate: &Candidate) -> bool {
    match preference {
        AdapterPreference::Any => true,
        AdapterPreference::Software => candidate.kind == Adapter::Software,
        AdapterPreference::Hardware => candidate.kind != Adapter::Software,
        AdapterPreference::Named(name) => candidate
            .name
            .to_ascii_lowercase()
            .contains(&name.to_ascii_lowercase()),
    }
}

pub fn choose(preference: &AdapterPreference, candidates: &[Candidate]) -> Option<usize> {
    candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| candidate.fitness == Fitness::Usable)
        .filter(|(_, candidate)| wanted(preference, candidate))
        .min_by_key(|(index, candidate)| (candidate.kind.rank(), *index))
        .map(|(index, _)| index)
}
