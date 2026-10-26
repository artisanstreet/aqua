//! `ui!` — declarative UI authoring macro for the Aqua GUI framework.
//!
//! See `docs/UI_MACRO.md` for the grammar, expansion contract, and examples.
//!
//! The macro expands to ordinary builder-expression calls against the facade
//! crate path (default `::aqua`, overridable with a leading
//! `crate = path;` preamble). It never evaluates Rust code and it has no
//! runtime dependency of its own.

mod codegen;
mod parser;

use proc_macro::TokenStream;

/// Declare UI structure and embed ordinary Rust computations.
///
/// ```ignore
/// ui! {
///     Box(layout: column, gap: 16) {
///         Text("Todo App", style: theme.heading)
///         for todo in todos.iter() {
///             key(todo.id) { Text(todo.label) }
///         }
///     }
/// }
/// ```
#[proc_macro]
pub fn ui(input: TokenStream) -> TokenStream {
    let input = proc_macro2::TokenStream::from(input);
    match syn::parse2::<parser::UiInput>(input) {
        Ok(parsed) => codegen::expand(&parsed).into(),
        Err(err) => err.to_compile_error().into(),
    }
}
