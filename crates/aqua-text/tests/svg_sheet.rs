use aqua_text::svg::sheet::Sheet;
use roxmltree::Document;

fn declared(sheet: &Sheet, document: &Document<'_>, id: &str) -> Vec<(String, String)> {
    let node = document
        .descendants()
        .find(|node| node.attribute("id") == Some(id))
        .expect("node");
    sheet.declarations(node)
}

fn value(declarations: &[(String, String)], name: &str) -> Option<String> {
    declarations
        .iter()
        .rev()
        .find(|(declared, _)| declared == name)
        .map(|(_, value)| value.clone())
}

#[test]
fn higher_specificity_wins_regardless_of_order_and_later_rules_break_ties() {
    let document = Document::parse(
        r#"<svg><style>#a { fill: red } .k { fill: green } text { fill: blue; stroke: black } .k { stroke: white }</style><text id="a" class="k"/><text id="b" class="k"/></svg>"#,
    )
    .unwrap();
    let sheet = Sheet::parse_document(&document);
    let a = declared(&sheet, &document, "a");
    let b = declared(&sheet, &document, "b");
    assert_eq!(value(&a, "fill").as_deref(), Some("red"));
    assert_eq!(value(&b, "fill").as_deref(), Some("green"));
    assert_eq!(value(&b, "stroke").as_deref(), Some("white"));
}

#[test]
fn descendant_selectors_comma_lists_comments_and_at_rules() {
    let document = Document::parse(
        r#"<svg><g class="label"><text id="in"/></g><text id="out"/><style>/* note */ @media print { text { fill: red } } .label text, #out { font-size: 2em !important } @font-face { font-family: x }</style></svg>"#,
    )
    .unwrap();
    let sheet = Sheet::parse_document(&document);
    assert_eq!(
        value(&declared(&sheet, &document, "in"), "font-size").as_deref(),
        Some("2em")
    );
    assert_eq!(
        value(&declared(&sheet, &document, "out"), "font-size").as_deref(),
        Some("2em")
    );
    assert_eq!(value(&declared(&sheet, &document, "in"), "fill"), None);
}

#[test]
fn documents_without_style_elements_have_empty_sheets() {
    let document = Document::parse("<svg><text id=\"a\"/></svg>").unwrap();
    assert!(Sheet::parse_document(&document).is_empty());
}
