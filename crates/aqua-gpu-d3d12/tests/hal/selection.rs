use aqua_gpu_d3d12::hal::selection::*;

#[test]
fn preference_parses_names_and_luids() {
    assert_eq!(parse_preference("WARP"), Some(AdapterPreference::Warp));
    assert_eq!(
        parse_preference("integrated"),
        Some(AdapterPreference::MinimumPower)
    );
    assert_eq!(
        parse_preference("luid:0x1f2e"),
        Some(AdapterPreference::Luid(0x1f2e))
    );
    assert_eq!(parse_preference("nonsense"), None);
}
