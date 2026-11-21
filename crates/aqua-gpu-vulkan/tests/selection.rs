use aqua_gpu_vulkan::hal::selection::{
    choose, parse_preference, Adapter, AdapterPreference, Candidate, Fitness,
};

fn candidate(name: &str, kind: Adapter, fitness: Fitness) -> Candidate {
    Candidate {
        name: name.into(),
        kind,
        fitness,
    }
}

fn machine() -> Vec<Candidate> {
    vec![
        candidate("llvmpipe (LLVM 21)", Adapter::Software, Fitness::Usable),
        candidate(
            "Microsoft Direct3D12 (dzn)",
            Adapter::Discrete,
            Fitness::Lacks("Vulkan 1.3"),
        ),
        candidate("Intel Arc A380", Adapter::Integrated, Fitness::Usable),
        candidate("AMD Radeon RX 9070 XT", Adapter::Discrete, Fitness::Usable),
    ]
}

#[test]
fn any_prefers_a_discrete_gpu_and_never_picks_an_unusable_device() {
    assert_eq!(choose(&AdapterPreference::Any, &machine()), Some(3));
    assert_eq!(
        choose(&AdapterPreference::Hardware, &machine()[..3]),
        Some(2)
    );
    assert_eq!(choose(&parse_preference("dzn"), &machine()), None);
}

#[test]
fn software_and_named_preferences_select_their_device_case_insensitively() {
    assert_eq!(choose(&parse_preference("software"), &machine()), Some(0));
    assert_eq!(choose(&parse_preference(" Radeon "), &machine()), Some(3));
    assert_eq!(
        choose(&AdapterPreference::Named("LLVMPIPE".into()), &machine()),
        Some(0)
    );
    assert_eq!(choose(&AdapterPreference::Hardware, &machine()[..2]), None);
}
