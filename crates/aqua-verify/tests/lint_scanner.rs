use std::path::Path;

use aqua_verify::lint::scan_source;
use aqua_verify::metadata::{breaches, parse};

fn tokens(source: &str) -> Vec<(&'static str, usize)> {
    scan_source(Path::new("x.rs"), source)
        .into_iter()
        .map(|hit| (hit.token, hit.line))
        .collect()
}

#[test]
fn reports_calls_with_their_line() {
    let hits =
        tokens("fn a() {\n    let t = std::time::Instant::now();\n    eprintln!(\"x\");\n}\n");
    assert!(hits.contains(&("Instant::now", 2)));
    assert!(hits.contains(&("eprintln!", 3)));
    assert!(
        !hits
            .iter()
            .any(|(token, _)| *token == "print!" || *token == "eprint!")
    );
}

#[test]
fn ignores_comments_strings_and_raw_strings() {
    let source = r##"
// Instant::now()
/* nested /* std::thread::spawn */ still comment println!() */
const A: &str = "Instant::now() and \" dbg!(x)";
const B: &str = r#"std::fs::read("x") " eprintln!"#;
const C: char = '"';
fn lifetime<'a>(x: &'a str) -> &'a str { x }
"##;
    assert_eq!(tokens(source), Vec::new());
}

#[test]
fn skips_cfg_test_modules_and_functions_but_not_following_code() {
    let source = "\
#[cfg(test)]
mod tests {
    fn helper() { std::fs::read(\"a\"); if true { println!(); } }
}
fn real() { std::thread::spawn(|| {}); }
#[cfg(test)]
fn single() { dbg!(1); }
fn after() { eprint!(); }
";
    let hits = tokens(source);
    assert!(hits.contains(&("std::thread", 5)));
    assert!(hits.contains(&("eprint!", 8)));
    assert!(
        !hits
            .iter()
            .any(|(token, _)| *token == "std::fs" || *token == "dbg!" || *token == "println!")
    );
}

#[test]
fn requires_an_identifier_boundary_before_the_token() {
    let hits = tokens("fn f() { my_thread::run(); notfs::open(); eprint!(); sprint!(); }");
    assert_eq!(hits, vec![("eprint!", 1)]);
}

#[test]
fn metadata_breach_reports_the_shortest_path_and_ignores_dev_dependencies() {
    let json = serde_json::json!({
        "packages": [
            {"id": "core 1", "name": "core", "manifest_path": "/w/core/Cargo.toml"},
            {"id": "mid 1", "name": "mid", "manifest_path": "/w/mid/Cargo.toml"},
            {"id": "wgpu 1", "name": "wgpu", "manifest_path": "/r/wgpu/Cargo.toml"},
            {"id": "winit 1", "name": "winit", "manifest_path": "/r/winit/Cargo.toml"}
        ],
        "workspace_members": ["core 1", "mid 1"],
        "resolve": {"nodes": [
            {"id": "core 1", "deps": [
                {"pkg": "mid 1", "dep_kinds": [{"kind": null}]},
                {"pkg": "winit 1", "dep_kinds": [{"kind": "dev"}]}
            ]},
            {"id": "mid 1", "deps": [{"pkg": "wgpu 1", "dep_kinds": [{"kind": null}]}]},
            {"id": "wgpu 1", "deps": []},
            {"id": "winit 1", "deps": []}
        ]}
    });
    let workspace = parse(json.to_string().as_bytes()).unwrap();
    let banned = |name: &str| name == "wgpu" || name == "winit";
    let found = breaches(&workspace, "core", &banned, &|_, _| false);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].path, vec!["core", "mid", "wgpu"]);
}

#[test]
fn an_allowed_edge_is_not_followed() {
    let json = serde_json::json!({
        "packages": [
            {"id": "text 1", "name": "text", "manifest_path": "/w/text/Cargo.toml"},
            {"id": "fonts 1", "name": "fonts", "manifest_path": "/w/fonts/Cargo.toml"},
            {"id": "winit 1", "name": "winit", "manifest_path": "/r/winit/Cargo.toml"}
        ],
        "workspace_members": ["text 1", "fonts 1"],
        "resolve": {"nodes": [
            {"id": "text 1", "deps": [{"pkg": "fonts 1", "dep_kinds": [{"kind": null}]}]},
            {"id": "fonts 1", "deps": [{"pkg": "winit 1", "dep_kinds": [{"kind": null}]}]},
            {"id": "winit 1", "deps": []}
        ]}
    });
    let workspace = parse(json.to_string().as_bytes()).unwrap();
    let banned = |name: &str| name == "fonts" || name == "winit";
    assert_eq!(
        breaches(&workspace, "text", &banned, &|_, _| false).len(),
        1
    );
    assert!(
        breaches(&workspace, "text", &banned, &|from, to| (from, to)
            == ("text", "fonts"))
        .is_empty()
    );
}

#[test]
fn root_items_follow_multiline_use_groups_and_skip_nested_items() {
    use aqua_verify::policy::root_public_items;
    let source = "pub mod a;\n// pub mod hidden;\npub use x::{\n    B,\n    c as D,\n};\n#[cfg(windows)]\npub struct S;\nimpl S {\n    pub fn nested() {}\n}\npub use y;\n";
    assert_eq!(root_public_items(source), vec!["a", "B", "D", "S", "y"]);
}

#[test]
fn architecture_roles_follow_the_section_four_block() {
    use aqua_verify::policy::architecture_crate_roles;
    let document = "## 4. Crate structure\n\n```\nPure (x)\n  aqua-core   ids\nAdapters\n  aqua-gpu-ref  oracle\nHost\n  aqua   api\n```\n\n### Enforcement\n  aqua-late\n";
    assert_eq!(
        architecture_crate_roles(document),
        vec![
            ("Pure".to_string(), "aqua-core".to_string()),
            ("Adapters".to_string(), "aqua-gpu-ref".to_string()),
            ("Host".to_string(), "aqua".to_string()),
        ]
    );
}
