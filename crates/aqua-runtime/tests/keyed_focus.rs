use aqua_runtime::Runtime;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::ui::Box;

fn control(key: &str, selector: &str) -> aqua_ui::element::Element {
    Box::new()
        .build()
        .with_key(key)
        .focusable(true)
        .debug_selector(selector)
        .aria_description("Search available extensions")
}

#[test]
fn focus_and_selector_follow_live_keyed_nodes_through_refresh_and_removal() {
    let mut app = Runtime::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| control("search", "market-search")).unwrap();
    let node = app.find_debug_selector("market-search").unwrap();
    assert!(app.focus_key("search"));
    assert_eq!(app.tree().focused(), Some(node));
    assert_eq!(
        app.tree()
            .emit_semantics()
            .iter()
            .find(|item| item.node == node)
            .unwrap()
            .description
            .as_deref(),
        Some("Search available extensions")
    );
    app.refresh(vec![control("search", "search-revised")])
        .unwrap();
    assert_eq!(app.find_debug_selector("market-search"), None);
    assert_eq!(app.find_debug_selector("search-revised"), Some(node));
    assert_eq!(app.tree().focused(), Some(node));
    app.refresh(vec![control("other", "search-revised")])
        .unwrap();
    assert!(!app.focus_key("search"));
    assert_ne!(app.find_debug_selector("search-revised"), Some(node));
}

#[test]
fn duplicate_live_keys_or_selectors_do_not_choose_an_arbitrary_target() {
    let mut app = Runtime::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| {
        Box::new()
            .child(control("same", "same"))
            .child(control("same", "same"))
            .build()
    })
    .unwrap();
    assert!(!app.focus_key("same"));
    assert_eq!(app.find_debug_selector("same"), None);
    assert_eq!(app.tree().focused(), None);
}
