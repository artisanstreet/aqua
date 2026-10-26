//! Parser for the `ui!` macro input.
//!
//! The grammar accepted here is documented in `docs/UI_MACRO.md`. This module
//! only parses; token generation lives in [`crate::codegen`].
//!
//! Design notes:
//! - Property values, `for` iterators, `if`/`match` scrutinees and key
//!   expressions are ordinary Rust expressions parsed with `syn`. They are
//!   passed through to the expansion unchanged, so normal lexical scoping
//!   applies and the macro never evaluates Rust code.
//! - `for` / `if` / `match` bodies contain DSL nodes (plus `let` statements),
//!   while a bare `{ ... }` in child position is a Rust block escape.

use syn::parse::{Parse, ParseStream};
use syn::{braced, parenthesized, Token};

syn::custom_keyword!(key);

/// Whole `ui! { ... }` body.
#[derive(Debug)]
pub struct UiInput {
    pub crate_path: syn::Path,
    pub items: Vec<ChildItem>,
}

/// One entry in a children list (or at the macro root): a `let` statement
/// spliced verbatim, or a UI node.
#[derive(Debug)]
pub enum ChildItem {
    Let(syn::Stmt),
    Node(Node),
}

/// A single UI node.
#[derive(Debug)]
pub enum Node {
    Component(ComponentNode),
    Key(KeyNode),
    For(ForNode),
    If(IfNode),
    Match(MatchNode),
    /// `{ ... }` Rust block escape; its value becomes a child.
    Block(syn::Block),
    /// Bare Rust expression; its value becomes a child.
    Expr(syn::Expr),
}

/// `Path(prop: value, ...) { children }` or `Path(...)`, etc.
#[derive(Debug)]
pub struct ComponentNode {
    pub path: syn::Path,
    pub args: Vec<Arg>,
    pub children: Option<Vec<ChildItem>>,
}

/// One property: positional (`Text("hi")`) or named (`gap: 16`).
#[derive(Debug)]
pub enum Arg {
    Positional(syn::Expr),
    Named { name: syn::Ident, value: syn::Expr },
}

/// `key(expr) { nodes }` — keyed region, usually inside a `for` body.
#[derive(Debug)]
pub struct KeyNode {
    pub key_expr: syn::Expr,
    pub body: Vec<ChildItem>,
}

/// `for pat in iter { nodes }` — setup-time iteration producing children.
#[derive(Debug)]
pub struct ForNode {
    pub pat: syn::Pat,
    pub iter: syn::Expr,
    pub body: Vec<ChildItem>,
}

/// `if cond { nodes } else if ... else { nodes }`.
#[derive(Debug)]
pub struct IfNode {
    pub cond: syn::Expr,
    pub then_items: Vec<ChildItem>,
    pub else_branch: Option<ElseBranch>,
}

#[derive(Debug)]
pub enum ElseBranch {
    Else(Vec<ChildItem>),
    ElseIf(Box<IfNode>),
}

/// `match scrut { pat => { nodes }, ... }`.
#[derive(Debug)]
pub struct MatchNode {
    pub scrutinee: syn::Expr,
    pub arms: Vec<MatchArm>,
}

#[derive(Debug)]
pub struct MatchArm {
    pub pat: syn::Pat,
    pub guard: Option<syn::Expr>,
    pub body: Vec<ChildItem>,
}

impl Parse for UiInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let crate_path = parse_crate_preamble(input)?.unwrap_or_else(|| syn::parse_quote!(::aqua));
        let mut items = Vec::new();
        while !input.is_empty() {
            if input.peek(Token![;]) {
                input.parse::<Token![;]>()?;
                continue;
            }
            items.push(input.parse::<ChildItem>()?);
        }
        if items.is_empty() {
            return Err(syn::Error::new(
                input.span(),
                "ui!: expected at least one node; the macro body must contain UI nodes, \
                 `let` statements, or control flow (`for` / `if` / `match`)",
            ));
        }
        Ok(UiInput { crate_path, items })
    }
}

/// Optional leading `crate = path;` (or `crate: path;`) preamble that
/// overrides the facade crate path used in generated code.
fn parse_crate_preamble(input: ParseStream) -> syn::Result<Option<syn::Path>> {
    if input.peek(Token![crate]) {
        let fork = input.fork();
        fork.parse::<Token![crate]>()?;
        // `crate::Foo` (joint `::`) is a component path, not a preamble.
        // Only a lone `=` or lone `:` (Spacing::Alone) introduces the
        // `crate = path;` / `crate: path;` preamble.
        enum Sep {
            Eq,
            Colon,
        }
        let sep = match fork.cursor().punct() {
            Some((op, _)) if op.as_char() == '=' => Some(Sep::Eq),
            Some((op, _)) if op.as_char() == ':' && op.spacing() == proc_macro2::Spacing::Alone => {
                Some(Sep::Colon)
            }
            _ => None,
        };
        if let Some(sep) = sep {
            input.parse::<Token![crate]>()?;
            if matches!(sep, Sep::Eq) {
                input.parse::<Token![=]>()?;
            } else {
                input.parse::<Token![:]>()?;
            }
            let path: syn::Path = input.parse()?;
            input.parse::<Token![;]>().map_err(|e| {
                syn::Error::new(
                    e.span(),
                    "ui!: expected `;` after the `crate = path` preamble",
                )
            })?;
            return Ok(Some(path));
        }
    }
    Ok(None)
}

impl Parse for ChildItem {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        if input.peek(Token![let]) {
            return Ok(ChildItem::Let(input.parse()?));
        }
        Ok(ChildItem::Node(input.parse()?))
    }
}

impl Parse for Node {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        if input.peek(Token![for]) {
            return Ok(Node::For(input.parse()?));
        }
        if input.peek(Token![if]) {
            return Ok(Node::If(input.parse()?));
        }
        if input.peek(Token![match]) {
            return Ok(Node::Match(input.parse()?));
        }
        if input.peek(syn::token::Brace) {
            return Ok(Node::Block(input.parse()?));
        }
        // `key(expr) { ... }` keyed region. A bare `key(...)` without a
        // trailing brace block falls through to component parsing.
        if input.peek(key) {
            let fork = input.fork();
            fork.parse::<key>()?;
            let inner;
            if fork.peek(syn::token::Paren) {
                parenthesized!(inner in fork);
                let _: syn::Expr = inner.parse()?;
                if inner.is_empty() && fork.peek(syn::token::Brace) {
                    return Ok(Node::Key(input.parse()?));
                }
            }
        }
        // Component: a path directly followed by `(...)` and/or `{...}`,
        // whose final segment starts with an uppercase letter (components
        // are type builders, so `Box`, `Text`, `app::Card`). A lowercase
        // call such as `make_child(1)` is an ordinary Rust expression;
        // anything else is a plain Rust expression child.
        let fork = input.fork();
        if let Ok(_path) = fork.parse::<syn::Path>() {
            if (fork.peek(syn::token::Paren) || fork.peek(syn::token::Brace))
                && last_segment_is_uppercase(&_path)
            {
                return Ok(Node::Component(input.parse()?));
            }
        }
        let expr: syn::Expr = input.parse().map_err(|e| {
            syn::Error::new(
                e.span(),
                format!(
                    "ui!: expected a component (`Box(...) {{ ... }}`), `key`, `for`, \
                     `if`, `match`, a `{{ ... }}` Rust block, or a Rust expression; {e}"
                ),
            )
        })?;
        if input.peek(Token![;]) {
            input.parse::<Token![;]>()?;
        }
        Ok(Node::Expr(expr))
    }
}

impl Parse for ComponentNode {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let path: syn::Path = input.parse()?;
        let mut args = Vec::new();
        if input.peek(syn::token::Paren) {
            let content;
            parenthesized!(content in input);
            while !content.is_empty() {
                args.push(content.parse::<Arg>()?);
                if content.peek(Token![,]) {
                    content.parse::<Token![,]>()?;
                } else if !content.is_empty() {
                    return Err(
                        content.error("ui!: expected `,` between properties in the argument list")
                    );
                }
            }
        }
        let mut children = None;
        if input.peek(syn::token::Brace) {
            let content;
            braced!(content in input);
            children = Some(parse_child_items(&content)?);
        } else if input.peek(Token![;]) {
            input.parse::<Token![;]>()?;
        }
        Ok(ComponentNode {
            path,
            args,
            children,
        })
    }
}

impl Parse for Arg {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        if input.peek(syn::Ident) {
            let fork = input.fork();
            let name: syn::Ident = fork.parse()?;
            // A lone `=` or lone `:` (Spacing::Alone) introduces a named
            // property. A joint `::` (as in `layout::assoc_fn()`) means the
            // argument is a positional Rust expression.
            enum Sep {
                Eq,
                Colon,
            }
            let sep = match fork.cursor().punct() {
                Some((op, _)) if op.as_char() == '=' => Some(Sep::Eq),
                Some((op, _))
                    if op.as_char() == ':' && op.spacing() == proc_macro2::Spacing::Alone =>
                {
                    Some(Sep::Colon)
                }
                _ => None,
            };
            if let Some(sep) = sep {
                input.parse::<syn::Ident>()?;
                if matches!(sep, Sep::Eq) {
                    input.parse::<Token![=]>()?;
                } else {
                    input.parse::<Token![:]>()?;
                }
                let value: syn::Expr = input.parse().map_err(|e| {
                    syn::Error::new(
                        e.span(),
                        format!("ui!: expected a Rust expression for property `{name}`; {e}"),
                    )
                })?;
                return Ok(Arg::Named { name, value });
            }
        }
        Ok(Arg::Positional(input.parse()?))
    }
}

impl Parse for KeyNode {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        input.parse::<key>()?;
        let inner;
        parenthesized!(inner in input);
        let key_expr: syn::Expr = inner.parse().map_err(|e| {
            syn::Error::new(
                e.span(),
                format!("ui!: expected a Rust key expression; {e}"),
            )
        })?;
        if !inner.is_empty() {
            return Err(inner.error("ui!: expected exactly one key expression"));
        }
        let content;
        braced!(content in input);
        let body = parse_child_items(&content)?;
        if input.peek(Token![;]) {
            input.parse::<Token![;]>()?;
        }
        Ok(KeyNode { key_expr, body })
    }
}

impl Parse for ForNode {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        input.parse::<Token![for]>()?;
        let pat = syn::Pat::parse_single(input).map_err(|e| {
            syn::Error::new(e.span(), format!("ui!: expected a `for` pattern; {e}"))
        })?;
        input.parse::<Token![in]>().map_err(|e| {
            syn::Error::new(
                e.span(),
                "ui!: expected `in` after the `for` pattern, e.g. `for todo in todos.iter()`",
            )
        })?;
        let iter = syn::Expr::parse_without_eager_brace(input).map_err(|e| {
            syn::Error::new(
                e.span(),
                format!("ui!: expected the `for` iterator expression; {e}"),
            )
        })?;
        let content;
        braced!(content in input);
        let body = parse_child_items(&content)?;
        if input.peek(Token![;]) {
            input.parse::<Token![;]>()?;
        }
        Ok(ForNode { pat, iter, body })
    }
}

impl Parse for IfNode {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        input.parse::<Token![if]>()?;
        let cond = syn::Expr::parse_without_eager_brace(input).map_err(|e| {
            syn::Error::new(
                e.span(),
                format!("ui!: expected the `if` condition expression; {e}"),
            )
        })?;
        let then_content;
        braced!(then_content in input);
        let then_items = parse_child_items(&then_content)?;
        let mut else_branch = None;
        if input.peek(Token![else]) {
            input.parse::<Token![else]>()?;
            if input.peek(Token![if]) {
                let nested: IfNode = input.parse()?;
                else_branch = Some(ElseBranch::ElseIf(Box::new(nested)));
            } else {
                let else_content;
                braced!(else_content in input);
                else_branch = Some(ElseBranch::Else(parse_child_items(&else_content)?));
            }
        }
        if input.peek(Token![;]) {
            input.parse::<Token![;]>()?;
        }
        Ok(IfNode {
            cond,
            then_items,
            else_branch,
        })
    }
}

impl Parse for MatchNode {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        input.parse::<Token![match]>()?;
        let scrutinee = syn::Expr::parse_without_eager_brace(input).map_err(|e| {
            syn::Error::new(
                e.span(),
                format!("ui!: expected the `match` scrutinee expression; {e}"),
            )
        })?;
        let content;
        braced!(content in input);
        let mut arms = Vec::new();
        while !content.is_empty() {
            if content.peek(Token![;]) {
                content.parse::<Token![;]>()?;
                continue;
            }
            arms.push(content.parse::<MatchArm>()?);
            if content.peek(Token![,]) {
                content.parse::<Token![,]>()?;
            }
        }
        if arms.is_empty() {
            return Err(syn::Error::new(
                content.span(),
                "ui!: `match` needs at least one arm, e.g. `Some(x) => { Text(x) }`",
            ));
        }
        if input.peek(Token![;]) {
            input.parse::<Token![;]>()?;
        }
        Ok(MatchNode { scrutinee, arms })
    }
}

impl Parse for MatchArm {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let pat = syn::Pat::parse_multi_with_leading_vert(input).map_err(|e| {
            syn::Error::new(
                e.span(),
                format!("ui!: expected a `match` arm pattern; {e}"),
            )
        })?;
        let mut guard = None;
        if input.peek(Token![if]) {
            input.parse::<Token![if]>()?;
            guard = Some(input.parse::<syn::Expr>().map_err(|e| {
                syn::Error::new(
                    e.span(),
                    format!("ui!: expected the match-guard expression; {e}"),
                )
            })?);
        }
        input.parse::<Token![=>]>().map_err(|e| {
            syn::Error::new(
                e.span(),
                "ui!: expected `=>` after the match arm pattern, e.g. `Some(x) => { Text(x) }`",
            )
        })?;
        let content;
        braced!(content in input);
        let body = parse_child_items(&content)?;
        Ok(MatchArm { pat, guard, body })
    }
}

/// Components are type builders, so by Rust convention their final path
/// segment starts with an uppercase letter (`Text`, `app::Card`).
fn last_segment_is_uppercase(path: &syn::Path) -> bool {
    path.segments
        .last()
        .map(|seg| {
            seg.ident
                .to_string()
                .chars()
                .next()
                .map(|c| c.is_ascii_uppercase())
                .unwrap_or(false)
        })
        .unwrap_or(false)
}

fn parse_child_items(input: ParseStream) -> syn::Result<Vec<ChildItem>> {
    let mut items = Vec::new();
    while !input.is_empty() {
        if input.peek(Token![;]) {
            input.parse::<Token![;]>()?;
            continue;
        }
        items.push(input.parse::<ChildItem>()?);
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::ToTokens;

    fn parse_ok(src: &str) -> UiInput {
        syn::parse_str::<UiInput>(src)
            .unwrap_or_else(|e| panic!("expected `{src}` to parse, got error: {e}"))
    }

    #[test]
    fn parses_todo_example_shape() {
        let ui = parse_ok(
            r#"Box(layout: column, gap: 16) {
                Text("Todo App", style: theme.heading)
                Box(layout: row, gap: 8) {
                    Text(bind: new_todo, editable: true, placeholder: "New todo...")
                    Button(on_activate: add_todo) { Text("Add") }
                }
            }"#,
        );
        assert_eq!(ui.items.len(), 1);
        match &ui.items[0] {
            ChildItem::Node(Node::Component(c)) => {
                assert_eq!(c.path.segments.last().unwrap().ident.to_string(), "Box");
                assert_eq!(c.args.len(), 2);
                assert!(c.children.as_ref().unwrap().len() == 2);
            }
            other => panic!("unexpected node: {other:?}"),
        }
    }

    #[test]
    fn parses_for_key_if_match_let_block_expr() {
        let ui = parse_ok(
            r#"Box {
                let title = format_title(config);
                for todo in todos.iter() { key(todo.id) { Text(todo.label) } }
                if show_footer { Text("footer") } else { Text("empty") }
                match state { Some(x) if ready => { Text(x) }, None => { Text("none") }, }
                { compute_child() }
                some_helper(1, 2);
            }"#,
        );
        let children = match &ui.items[0] {
            ChildItem::Node(Node::Component(c)) => c.children.as_ref().unwrap(),
            other => panic!("unexpected root: {other:?}"),
        };
        assert_eq!(children.len(), 6);
        assert!(matches!(children[0], ChildItem::Let(_)));
        assert!(matches!(children[1], ChildItem::Node(Node::For(_))));
        assert!(matches!(children[2], ChildItem::Node(Node::If(_))));
        assert!(matches!(children[3], ChildItem::Node(Node::Match(_))));
        assert!(matches!(children[4], ChildItem::Node(Node::Block(_))));
        assert!(matches!(children[5], ChildItem::Node(Node::Expr(_))));
    }

    #[test]
    fn parses_crate_preamble_forms() {
        let ui = parse_ok(r#"crate = crate::mock; Text("hi")"#);
        assert_eq!(ui.crate_path.to_token_stream().to_string(), "crate :: mock");
        let ui = parse_ok(r#"crate: ::my_aqua; Text("hi")"#);
        assert_eq!(ui.crate_path.to_token_stream().to_string(), ":: my_aqua");
        // A component path rooted at `crate::` must not be mistaken for a preamble.
        let ui = parse_ok(r#"crate::Card(title: "t") { Text("x") }"#);
        assert_eq!(ui.crate_path.to_token_stream().to_string(), ":: aqua");
        match &ui.items[0] {
            ChildItem::Node(Node::Component(c)) => {
                assert_eq!(c.path.to_token_stream().to_string(), "crate :: Card");
            }
            other => panic!("unexpected node: {other:?}"),
        }
    }

    #[test]
    fn rejects_empty_and_bad_for() {
        let err = syn::parse_str::<UiInput>("").unwrap_err();
        assert!(err.to_string().contains("at least one node"), "{err}");
        let err = syn::parse_str::<UiInput>("Box { for x xs { Text(x) } }").unwrap_err();
        assert!(err.to_string().contains("expected `in`"), "{err}");
    }
}
