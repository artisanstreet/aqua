//! Code generation for the `ui!` macro: [`UiInput`](crate::parser::UiInput)
//! to builder-expression tokens.
//!
//! The generated code only depends on a small, documented runtime surface
//! (`docs/UI_MACRO.md`, "runtime contract"). The macro crate itself has no
//! runtime dependency, so there is no dependency cycle with `aqua-ui` or the
//! `aqua` facade.

use proc_macro2::TokenStream;
use quote::{quote, ToTokens};

use crate::parser::{
    Arg, ChildItem, ComponentNode, ElseBranch, ForNode, IfNode, KeyNode, MatchNode, Node, UiInput,
};

/// Builtin component names. A single-segment path matching one of these
/// resolves to `#crate_path::ui::Name::builder()`; every other path is a
/// custom component and resolves to `<Path>::builder()`.
const BUILTINS: &[&str] = &[
    "Box",
    "Text",
    "Media",
    "Scroll",
    "Canvas",
    "Viewport3D",
    "Button",
    "Checkbox",
    "Slider",
    "Dropdown",
    "Tabs",
    "Fragment",
    "Dynamic",
];

/// Expand a parsed `ui!` body into an expression evaluating to an `Element`.
pub fn expand(input: &UiInput) -> TokenStream {
    let c = &input.crate_path;
    let mut stmts = TokenStream::new();
    for item in &input.items {
        push_child_item(&mut stmts, c, item);
    }
    quote! {{
        let mut __aqua_b = #c::ui::Fragment::builder();
        #stmts
        __aqua_b.build()
    }}
}

/// Emit builder statements for one children-list entry into `out`.
///
/// `let` statements are spliced verbatim so later siblings (and property
/// expressions) see the bindings with normal Rust lexical scoping. Nodes
/// become `__aqua_b = __aqua_b.child(<element expr>);`.
fn push_child_item(out: &mut TokenStream, c: &syn::Path, item: &ChildItem) {
    match item {
        ChildItem::Let(stmt) => stmt.to_tokens(out),
        ChildItem::Node(node) => {
            let expr = expand_node(c, node);
            out.extend(quote! { __aqua_b = __aqua_b.child(#expr); });
        }
    }
}

fn expand_children_block(c: &syn::Path, items: &[ChildItem]) -> TokenStream {
    let mut stmts = TokenStream::new();
    for item in items {
        push_child_item(&mut stmts, c, item);
    }
    quote! {{
        let mut __aqua_b = #c::ui::Fragment::builder();
        #stmts
        __aqua_b.build()
    }}
}

fn expand_node(c: &syn::Path, node: &Node) -> TokenStream {
    match node {
        Node::Component(comp) => expand_component(c, comp),
        Node::Key(key) => expand_key(c, key),
        Node::For(for_node) => expand_for(c, for_node),
        Node::If(if_node) => expand_if(c, if_node),
        Node::Match(match_node) => expand_match(c, match_node),
        Node::Block(block) => {
            // A block holding exactly one value expression is spliced without
            // its braces: they would only trip `unused_braces` in user code.
            // Anything else keeps its braces (statement scope is load-bearing).
            if block.stmts.len() == 1 {
                if let syn::Stmt::Expr(expr, None) = &block.stmts[0] {
                    return quote! { (#expr) };
                }
            }
            quote! { #block }
        }
        Node::Expr(expr) => quote! { (#expr) },
    }
}

fn expand_component(c: &syn::Path, comp: &ComponentNode) -> TokenStream {
    let init = builder_init(c, &comp.path);
    let mut stmts = TokenStream::new();
    for arg in &comp.args {
        match arg {
            Arg::Positional(expr) => {
                stmts.extend(quote! { __aqua_b = __aqua_b.arg(#expr); });
            }
            Arg::Named { name, value } => {
                stmts.extend(quote! { __aqua_b = __aqua_b.#name(#value); });
            }
        }
    }
    if let Some(children) = &comp.children {
        for item in children {
            push_child_item(&mut stmts, c, item);
        }
    }
    quote! {{
        let mut __aqua_b = #init;
        #stmts
        __aqua_b.build()
    }}
}

fn builder_init(c: &syn::Path, path: &syn::Path) -> TokenStream {
    if is_builtin(path) {
        let name = &path.segments.last().unwrap().ident;
        quote! { #c::ui::#name::builder() }
    } else {
        quote! { #path::builder() }
    }
}

fn is_builtin(path: &syn::Path) -> bool {
    if path.leading_colon.is_some() || path.segments.len() != 1 {
        return false;
    }
    let name = path.segments.first().unwrap().ident.to_string();
    BUILTINS.iter().any(|b| *b == name)
}

fn expand_key(c: &syn::Path, key: &KeyNode) -> TokenStream {
    let key_expr = &key.key_expr;
    // A single-node body keeps the tree shape flat: `node.with_key(k)`.
    if let [ChildItem::Node(node)] = key.body.as_slice() {
        let inner = expand_node(c, node);
        return quote! { (#inner).with_key(#key_expr) };
    }
    let mut stmts = TokenStream::new();
    for item in &key.body {
        push_child_item(&mut stmts, c, item);
    }
    quote! {{
        let mut __aqua_b = #c::ui::Fragment::builder();
        #stmts
        __aqua_b.build().with_key(#key_expr)
    }}
}

fn expand_for(c: &syn::Path, for_node: &ForNode) -> TokenStream {
    let ForNode { pat, iter, body } = for_node;
    let mut stmts = TokenStream::new();
    for item in body {
        push_child_item(&mut stmts, c, item);
    }
    quote! {
        #c::ui::Fragment::from_nodes(
            (#iter).into_iter().map(|#pat| {
                let mut __aqua_b = #c::ui::Fragment::builder();
                #stmts
                __aqua_b.build()
            })
        )
    }
}

fn expand_if(c: &syn::Path, if_node: &IfNode) -> TokenStream {
    let IfNode {
        cond,
        then_items,
        else_branch,
    } = if_node;
    let then_block = expand_children_block(c, then_items);
    // NOTE: `else if` must be emitted bare — `else (if ...)` is not valid
    // Rust (`else` requires a block or a trailing `if` directly).
    let else_tokens = match else_branch {
        None => {
            quote! { else { #c::ui::Fragment::builder().build() } }
        }
        Some(ElseBranch::Else(items)) => {
            let block = expand_children_block(c, items);
            quote! { else #block }
        }
        Some(ElseBranch::ElseIf(nested)) => {
            let inner = expand_if(c, nested);
            quote! { else #inner }
        }
    };
    quote! {
        if #cond #then_block #else_tokens
    }
}

fn expand_match(c: &syn::Path, match_node: &MatchNode) -> TokenStream {
    let MatchNode { scrutinee, arms } = match_node;
    let mut arm_tokens = TokenStream::new();
    for arm in arms {
        let pat = &arm.pat;
        let guard = arm.guard.as_ref().map(|g| quote! { if #g });
        let body = expand_children_block(c, &arm.body);
        arm_tokens.extend(quote! { #pat #guard => #body, });
    }
    quote! {
        match #scrutinee { #arm_tokens }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::UiInput;

    fn expand_src(src: &str) -> String {
        let input: UiInput = syn::parse_str(src)
            .unwrap_or_else(|e| panic!("expected `{src}` to parse, got error: {e}"));
        expand(&input).to_string()
    }

    #[test]
    fn builder_chain_and_crate_path() {
        let out = expand_src(r#"crate = crate::mock; Box(layout: column, gap: 16) { Text("hi") }"#);
        assert!(
            out.contains("crate :: mock :: ui :: Box :: builder ()"),
            "{out}"
        );
        assert!(
            out.contains("__aqua_b = __aqua_b . layout (column)"),
            "{out}"
        );
        assert!(out.contains("__aqua_b = __aqua_b . gap (16)"), "{out}");
        assert!(
            out.contains("crate :: mock :: ui :: Text :: builder ()"),
            "{out}"
        );
        assert!(out.contains("__aqua_b = __aqua_b . arg (\"hi\")"), "{out}");
    }

    #[test]
    fn custom_component_uses_path_builder() {
        let out = expand_src(r#"crate = crate::mock; app::Card(title: "t") { Text("x") }"#);
        assert!(out.contains("app :: Card :: builder ()"), "{out}");
        assert!(out.contains("__aqua_b = __aqua_b . title (\"t\")"), "{out}");
        // Single-segment custom names also resolve to their own builder.
        let out = expand_src(r#"crate = crate::mock; Card(title: "t")"#);
        assert!(
            out.contains("Card :: builder ()") && !out.contains("mock :: ui :: Card"),
            "{out}"
        );
    }

    #[test]
    fn key_for_if_match_shapes() {
        let out = expand_src(
            r#"crate = crate::mock; Box {
                for todo in todos.iter() { key(todo.id) { Text(todo.label) } }
                if flag { Text("a") } else { Text("b") }
                match s { Some(x) => { Text(x) }, None => { Text("n") }, }
            }"#,
        );
        assert!(out.contains("Fragment :: from_nodes"), "{out}");
        assert!(out.contains(". with_key (todo . id)"), "{out}");
        assert!(out.contains("if flag"), "{out}");
        assert!(out.contains("match s"), "{out}");
    }

    #[test]
    fn default_crate_path_is_aqua_facade() {
        let out = expand_src(r#"Text("hi")"#);
        assert!(out.contains(":: aqua :: ui :: Text :: builder ()"), "{out}");
    }
}
