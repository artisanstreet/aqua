use aqua_gpu_metal::hal::selection::{
    choose, parse_preference, AdapterPreference, Candidate, Fitness,
};

fn candidate(name: &str, system_default: bool, fitness: Fitness) -> Candidate {
    Candidate {
        name: name.into(),
        system_default,
        fitness,
    }
}

fn machine() -> Vec<Candidate> {
    vec![
        candidate("AMD Radeon Pro 5500M", false, Fitness::Usable),
        candidate("Apple Paravirtual device", true, Fitness::Usable),
        candidate("Apple M1", false, Fitness::Lacks("the Common3 GPU family")),
    ]
}

#[test]
fn without_a_preference_the_system_default_device_wins_unless_it_is_unusable() {
    assert_eq!(choose(&parse_preference(""), &machine()), Some(1));
    assert_eq!(choose(&parse_preference(" default "), &machine()), Some(1));
    let mut lacking = machine();
    lacking[1].fitness = Fitness::Lacks("the Common3 GPU family");
    assert_eq!(choose(&AdapterPreference::SystemDefault, &lacking), Some(0));
}

#[test]
fn a_named_preference_matches_a_usable_device_by_case_insensitive_substring() {
    assert_eq!(choose(&parse_preference("radeon"), &machine()), Some(0));
    assert_eq!(
        choose(&parse_preference("PARAVIRTUAL"), &machine()),
        Some(1)
    );
    assert_eq!(choose(&parse_preference("M1"), &machine()), None);
    assert_eq!(choose(&parse_preference("Intel"), &machine()), None);
}
