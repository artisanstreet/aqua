# `ui!` macro — grammar, expansion contract, and status

Owner: macros agent. Crate: `crates/aqua-macros` (`aqua-macros`,
proc-macro, deps only `syn 2` / `quote 1` / `proc-macro2 1` per contract §0 —
verified latest-compatible via `cargo info` on 2026-10-25: syn 2.0.119 in
use). The macro crate has **no runtime dependency**, so there is no
dependency cycle with `aqua-ui`, `aqua-reactive`, or the `aqua` facade.

Design source: `GUI_FRAMEWORK_DESIGN.md` §§ "Authoring model: Rust with an
optional `ui!` DSL", "What `ui!` generates", "Application and component
lifecycle". Nothing here claims an unimplemented feature works; §8 tracks
status honestly.

## 1. What the macro is and is not

- `ui! { ... }` expands to **ordinary builder-expression calls** returning an
  `Element`. It never evaluates Rust code, never reads reactive state, never
  creates subscriptions, and never computes layout/geometry.
- All Rust expressions (property values, `for` iterators, `if`/`match`
  scrutinees, key expressions, `let` initializers, block escapes) are passed
  through verbatim with their original spans. Normal lexical scoping applies;
  ownership/borrow errors point at user code (proven by tests that had to
  satisfy the borrow checker, e.g. `move` closures for callbacks).
- `for` / `if` / `match` inside `ui!` are **setup-time control flow**: they
  run once when the surrounding Rust code runs and produce a fixed child
  list. Changing collections/conditions re-run only if the enclosing Rust
  function re-runs. Reactive lists and conditions are explicit components
  (`Dynamic`, future `List`/`bind_each` from the UI owner) — never macro
  magic. There are no hidden auto-subscriptions anywhere in the expansion.

## 2. Grammar

```text
ui        := [ "crate" ("=" | ":") Path ";" ] items
items     := ( ";" | let-stmt | node )*
node      := component | keyed | for-loop | if-chain | match-expr | block | expr
component := Path "(" [ arg ("," arg)* [","] ] ")" [ "{" items "}" ] [";"]
           | Path "{" items "}"                                        // no-arg + children
arg       := Ident (":" | "=") Expr        // named property
           | Expr                          // positional
keyed     := "key" "(" Expr ")" "{" items "}" [";"]
for-loop  := "for" Pat "in" Expr "{" items "}" [";"]
if-chain  := "if" Expr "{" items "}" ( "else" (if-chain | "{" items "}") )? [";"]
match-expr:= "match" Expr "{" (Pat ["if" Expr] "=>" "{" items "}" ","?)* "}" [";"]
block     := "{" RustBlockContent "}"       // Rust escape, value becomes a child
expr      := RustExpr [";"]                 // Rust escape, value becomes a child
let-stmt  := "let" ... ";"                  // spliced verbatim (normal `let`)
```

Rules that resolve ambiguity (each covered by a test):

1. **Uppercase rule.** A `Path` followed by `(...)`/`{...}` is a component
   only if its final segment starts with an ASCII uppercase letter
   (`Text`, `app::Card`, `crate::mock::Card`). Otherwise it is a Rust
   expression (`make_row(1)`, `todos.len()`). Components are type builders,
   so this matches Rust convention; lowercase component names must use the
   block escape (`{ my_comp(x) }`).
2. **Brace binding.** `{...}` directly after `Comp(...)` is always that
   component's children. A Rust escape after a component needs a separator:
   `Text(t); { helper(&t) }`.
3. **Single-expression blocks are spliced bare.** `{ f(x) }` as a child
   expands to `(f(x))`, avoiding `unused_braces` noise. Multi-statement
   blocks keep their braces (scope is load-bearing).
4. **`key` reservation.** `key(expr) {...}` is a keyed region. A component
   literally named `key` without a trailing brace block still parses as a
   component.
5. **Separators.** Named properties accept `:` or `=`. Nodes and `let`
   statements accept optional trailing `;` (separators, never value-dropping:
   expression children keep their value).
6. **`crate` preamble.** `crate = path;` / `crate: path;` overrides the
   facade path. A lone `:` vs joint `::` is distinguished by punct spacing,
   so `crate::Card(...)` (component) and `crate: ::aqua;` (preamble) both
   work. A path starting with `::` is accepted (`crate: ::aqua;`).

Custom render functions work today with zero DSL support: define
`fn todo_row(t: &Todo) -> Element { ui! { ... } }` and splice
`{ todo_row(&t) }` (tested). Custom *components* `P(...) {...}` expand to
`<P>::builder()` (tested with `mock::Card` / `crate::mock::Card`).

## 3. Expansion model (normative for codegen v0.1)

`ui!` with items `i₁…iₙ` expands to:

```rust
{
    let mut __aqua_b = C::ui::Fragment::builder();
    // per item: `let` spliced verbatim, or
    // `__aqua_b = __aqua_b.child(<element expr>);`
    __aqua_b.build()
}
```

- `C` is the facade path (`::aqua` default).
- `Comp(p, k: v, ...) { kids }` →
  `{ let mut __aqua_b = INIT; __aqua_b = __aqua_b.arg(p); …
     __aqua_b = __aqua_b.k(v); …kids…; __aqua_b.build() }`.
- `key(k) { single node n }` → `(<n>).with_key(k)`; multi-item body →
  fragment built from the body, then `.with_key(k)`.
- `for pat in iter { body }` →
  `C::ui::Fragment::from_nodes((iter).into_iter().map(|pat| { …body… }))`.
- `if c { a } else { b }` → `if c { A } else B` (branch blocks build
  `Element`s; missing `else` builds an empty fragment; `else if` chains nest
  without parentheses — `else (if …)` is invalid Rust).
- `match s { p [if g] => { body } }` → ordinary `match` with fragment bodies.
- `{ … }` / bare expr → child expression as §2 rules 2–3.

## 4. Runtime contract (IMPLEMENTED in `aqua-ui::element` + `binding`)

Generated code uses **only** the surface below. Method shapes are frozen;
value types are documented per builder.

```rust
// Facade requirement (facade owner): `aqua::ui` MUST alias the macro-facing
// module: `pub use aqua_ui::element::ui;`
// Direct path (macro tests, hosts): `aqua_ui::element::ui::N`
// Macro default path: `::aqua::ui::N` (i.e. `crate = ::aqua`, overridable
// per-invocation with `crate = path;`).

pub struct Element;   // aqua_ui::element::Element
pub struct Fragment;  // itself the builder: builder()/child()/build()
pub enum Key;         // From<&str/String/u64/u32/usize/i32>; hashed at mount

impl Fragment {
    pub fn builder() -> Fragment;
    pub fn child(mut self, child: impl Into<Element>) -> Self;
    pub fn from_nodes<I, E>(iter: I) -> Fragment
    where I: IntoIterator<Item = E>, E: Into<Element>;
    pub fn build(self) -> Element;
}
impl From<Fragment> for Element { … }
impl Element {
    pub fn with_key(mut self, key: impl Into<Key>) -> Self;
    pub fn fragment(children: Vec<Element>) -> Self; // hosts driving sync
}

// Per builder N in ui::{Box, Text, Media, Scroll, Canvas, Viewport3D,
// Button, Checkbox, Slider, Dropdown, Tabs, Dynamic}:
impl N {
    pub fn builder() -> N;
    pub fn arg(mut self, …) -> Self;        // positional (component-specific)
    pub fn child(mut self, child: impl Into<Element>) -> Self;
    pub fn build(self) -> Element;
    // …plus named-property methods below; each `name: v` → `.name(v)`.
}
```

Named properties implemented (anything else is a compile error at the
user call site — never a silent no-op):

- All builders: `.layout("row"/"column" | Axis)` — plus canonical lowercase
  constants `ui::column: Axis = Axis::Column` and `ui::row: Axis = Axis::Row`
  (`#[allow(non_upper_case_globals)]`), so `Box(layout: column)` works bare
  with `use aqua_ui::element::ui::{column, row};` (facade prelude re-export
  pending with facade owner). `.gap(i32|f32|…)`
  (`NumberArg`), `.width/.height(impl Into<Length>)` (`200` → `Px(200)`),
  `.padding(n)`, `.style(LayoutStyle)` (replaces; set granular props after).
- `Text`: `.value/.arg(impl TextSource)` — `String`/`&str` literals are
  static; `ReadSignal<String>` snapshots at mount AND subscribes `SetText`
  updates (explicit dynamic, no hidden tracking). `.value_signal(read)` is
  the spelled-out form. `.bind((read, write))` is the canonical two-way
  editable binding (enables editing, routes `TextChanged` back via
  `try_set`). `.editable/.multiline/.wrap/.selectable/.password(bool)`,
  `.placeholder(s)` (display-only; committed text unaffected),
  `.font_size(n)`, `.color(Oklch)`.
  Uncontrolled vs controlled: an editable `Text` with a literal value and
  no bindings is *uncontrolled* — keyed reuse preserves the live editor
  (backing, caret, selection); the descriptor's initial is the mount-time
  default, and re-syncs never clobber typing (mirrors core `update_spec`).
  `value(signal)` / `bind((read, write))` is *controlled*: the reactive
  source dictates content via `SetText` effects. To force content into an
  uncontrolled field, switch it to a controlled binding.
  Mode changes are genuine, never inert: static↔editable flips rebuild the
  backing store in place (node id preserved, text + caret carried over,
  undo history resets); editable single↔multiline flips call the editor's
  `set_line_mode` in place; static wrap changes are cosmetic.
- `Button`: `.on_activate(|| …)` (`Fn()`, no args) and
  `.on_activate_event(|e: ActionEvent| …)`. No retained button primitive:
  lowers to `Box` + `Activate` binding + focusable.
- `Media`: `.arg/.image(ImageId)`, `.opacity(n)`.
- `Scroll`: `.offset(Point)`, `.offset_xy((x, y))`.
- `Canvas`: `.arg/.on_paint(|bounds: Rect| -> Vec<DisplayItem>)`.
- `Viewport3D`: `.viewport(ViewportId)`, `.layer(u64)`.
- `Checkbox`: `.arg/.label(s)`, `.checked(bool)` (app-owned prop),
  `.on_toggle(|| …)`.
- `Slider`: `.arg/.value(n)` (0..=1, clamped). Visual-only: no gesture
  callback exists because `ActionEvent` carries no position (documented gap).
- `Dropdown`: `.arg/.label(s)`, `.on_open(|| …)` (activator only; popup via
  overlay service, pending).
- `Tabs`: `.tabs(Vec<String>)`, `.selected(usize)`, `.on_select(|i| …)`.
- `Dynamic`: `.value(ReadSignal<String>)` (required; missing → loud
  `LowerError::DynamicWithoutSource` at mount).
- Custom component `P(...)`: `<P>::builder()` with the same shape
  (`.arg` / named methods / `.child` / `.build() -> Element`).

Mount/binding (`aqua_ui::binding::View`, scope-owned):

- `View::new(scope, tree.command_sink())`, `set_content(&mut tree, Vec<Element>)`.
  First call mounts (single root required); later calls refresh the root in
  place or keyed-sync children. Returns paired node ids.
- Each bound node gets a **child scope** owning exactly its effects.
  Removed subtrees dispose their scopes (no leaked subscriptions); reused
  ids with changed sources dispose + re-attach without losing the node,
  focus, or caret; unchanged sources keep their effects (fingerprint on
  reactive `NodeId`s — no duplicate subscriptions across syncs).
- Updates flow effects → `CommandSink` → drained at layout boundary. Effects
  never touch the tree (no reentrancy, no `RefCell` around tree state).

## 5. Facade / UI / reactive coordination status (2026-10-25)

- `aqua-ui` has `lib.rs`/`overlay.rs`/`style.rs` but **no `element.rs` yet**,
  so the contract above is a proposal, not a frozen API. Core freezes
  builder names when `element.rs` lands (contract §5 item 4); the macro
  tracks renames same-day (codegen is ~120 lines, tests pin every shape).
- `aqua::ui` alias → `aqua_ui::element`: pending with facade owner.
## 5. Facade / UI / reactive coordination status (2026-10-25, evening)

- `aqua-ui::element` + `aqua-ui::binding` LANDED (macro-agent owned):
  `Element` builders for all builtins, `NodeSpec` lowering with structural
  fragments, `View` mount/sync/binding over `CommandSink`. Wired into the
  crate via the granted additive lines (`pub mod element/binding` in
  `lib.rs`, `aqua-reactive` dep in `aqua-ui/Cargo.toml`).
- `aqua::ui` facade alias: LANDED (`pub use aqua_ui::element::ui;` in
  `aqua/src/lib.rs`). Macro default path `::aqua` works; direct path
  `aqua_ui::element::ui::N` also works.
- `aqua-reactive` graph: LANDED. Bindings subscribe through it; no
  duplicate graph anywhere.
- Workspace: macro crate is a member (`members = ["crates/*"]`, no
  exclusion). `cargo test -p aqua-macros` and `--features aqua-integration`
  both green on the shared workspace. Uses agreed `syn 2 / quote 1 /
  proc-macro2 1`.
- Macro dev-deps (own `Cargo.toml`): `aqua`, `aqua-ui`, `aqua-reactive`,
  `aqua-core` — all resolve and build.
- `tests/real_mount.rs` (always active, 3 tests, no facade needed):
  `crate = crate::real;` override re-exporting the real builders; covers
  mount → signal update → layout, keyed reorder with identity, and explicit
  mount errors.
- `tests/ui_facade.rs` (gated on `aqua-integration`, 3 active): todo smoke
  + keyed list against `::aqua::ui`, plus end-to-end `real_mount_counter`.
- Siblings are unreachable via agent messaging from contributor threads
  (confirmed by parent); interface requests go in this doc + final report
  for parent relay.

## 6. Diagnostics

All parse failures are `syn::Error`s with user-code spans and a `ui!:`-prefixed
message, e.g. `expected 'in' after the 'for' pattern`, `expected '=>' after
the match arm pattern`, `expected at least one node`. Type errors in property
values/callbacks come from the Rust compiler against the builder signatures.

## 7. Tests run (`cargo test -p aqua-macros [--features aqua-integration])

- Unit (8): parser shapes (todo example, for/key/if/match/let/block/expr,
  preamble forms incl. `crate::Card` vs preamble disambiguation, empty/bad-for
  errors); codegen shapes (builder chains, custom-path builders, key/for/if/
  match, default `::aqua` path).
- Integration vs mock facade (6, `tests/ui_macro.rs`): todo-app structure,
  keyed `for` order/identity, if/else + if-less + match (both arms),
  `let` + nested `ui!` + custom function `todo_row`, `=` separator + custom
  component, `Dynamic` explicit pass-through.
- Example (`examples/todo.rs`): runnable, prints the element tree
  (`cargo run --example todo`).
- Dormant real-facade tests (`tests/ui_facade.rs`, 0 active): gated on
  `--features aqua-integration` + the four dev-deps listed in
  `crates/aqua-macros/Cargo.toml`. Includes the end-to-end
  `real_mount_counter`: `ui!` → real `aqua_ui::element::ui` builders →
  `View` mount → signal write → layout → updated text. **Enable once the
  tree (incl. aqua-text) is green** — mock-only coverage would hide API
  drift.
- `cargo fmt --check` clean; `cargo clippy --all-targets` clean (0 warnings)
  for the macro crate. `aqua-ui` modules are `rustfmt`-clean (parse-verified);
  full `cargo check/test/clippy -p aqua-ui` pending the aqua-text fix.

## 8. Gaps / non-goals for this cut

- `Text` placeholder: LANDED (core added `TextContent.placeholder`;
  builder `.placeholder(s)`, lowering, and refresh all carry it;
  display-only, committed text unaffected). Verified by
  `placeholder_carries_into_text_content` and
  `placeholder_shows_when_empty_only`.
- Slider is visual-only (no gesture callback: `ActionEvent` carries no
  position); Dropdown mounts the activator only (popup needs the overlay
  service, overlays owner). Both are labeled `[UNIMPLEMENTED]`-style in
  rustdoc, not faked.
- Reactive collections (`bind_each`/`List`), async resources, and motion
  properties have no macro syntax beyond explicit components; deliberately
  deferred rather than invented here.
- Outer attributes (`#[cfg]`, doc comments) on nodes are rejected; use Rust
  `#[cfg]` around `ui!` or block escapes.
- No hot-reload parser extraction (single crate until tooling needs it).
- Error tests are string-based (`parse_str` error text), not trybuild UI
  tests — trybuild is overkill while the grammar is still settling.
