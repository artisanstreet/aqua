#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum AdapterPreference {
    #[default]
    SystemDefault,
    Named(String),
}

pub const ADAPTER_VARIABLE: &str = "AQUA_METAL_ADAPTER";

pub fn parse_preference(text: &str) -> AdapterPreference {
    match text.trim() {
        "" => AdapterPreference::SystemDefault,
        name if name.eq_ignore_ascii_case("default") => AdapterPreference::SystemDefault,
        name => AdapterPreference::Named(name.to_ascii_lowercase()),
    }
}

impl AdapterPreference {
    pub fn from_env() -> Self {
        std::env::var(ADAPTER_VARIABLE)
            .map(|text| parse_preference(&text))
            .unwrap_or_default()
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
    pub system_default: bool,
    pub fitness: Fitness,
}

fn wanted(preference: &AdapterPreference, candidate: &Candidate) -> bool {
    match preference {
        AdapterPreference::SystemDefault => true,
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
        .min_by_key(|(index, candidate)| (!candidate.system_default, *index))
        .map(|(index, _)| index)
}
