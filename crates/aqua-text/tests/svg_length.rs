use aqua_text::svg::length::{font_size, lengths, parse_length, resolve, Length};

#[test]
fn units_resolve_against_the_font_size() {
    assert_eq!(parse_length("12pt"), Some(Length::Px(16.0)));
    assert_eq!(resolve(parse_length("2em").unwrap(), 10.0), Some(20.0));
    assert_eq!(resolve(parse_length("1ex").unwrap(), 10.0), Some(5.0));
    assert_eq!(resolve(parse_length("1.5e1px").unwrap(), 10.0), Some(15.0));
    assert_eq!(resolve(parse_length("50%").unwrap(), 10.0), None);
    assert_eq!(parse_length("3furlongs"), None);
}

#[test]
fn font_sizes_inherit_em_percent_and_keywords_from_the_parent() {
    assert_eq!(font_size("1.5em", 20.0), Some(30.0));
    assert_eq!(font_size("50%", 20.0), Some(10.0));
    assert_eq!(font_size("larger", 20.0), Some(24.0));
    assert_eq!(font_size("x-large", 20.0), Some(24.0));
    assert_eq!(font_size("0", 20.0), None);
    assert_eq!(font_size("garbage", 20.0), None);
}

#[test]
fn length_lists_keep_unresolvable_entries_in_place() {
    assert_eq!(
        lengths(Some("1em, 10% 3"), 10.0),
        vec![Some(10.0), None, Some(3.0)]
    );
}
