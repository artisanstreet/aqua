# Authoring UI with `Aqua::*()` factories

Canonical entry point: associated factories on the existing concrete
runtime struct [`Aqua`](crates/aqua/src/runtime.rs). There is no second
`Aqua` type and no namespace alias.

```rust
use aqua::{Aqua, px};

let ui = Aqua::Box()
    .styles([Aqua::Style().width(px(200.0))])
    .child(Aqua::Text("Tasks").build())
    .child(
        Aqua::Button()
            .on_activate(|| {})
            .child(Aqua::Text("Add").build())
            .build(),
    )
    .build();
```

## Factory inventory

| Factory | Builder input |
|---|---|
| `Aqua::Box()` | container |
| `Aqua::Text(content)` | literal or `ReadSignal` (`content: impl TextSource`) |
| `Aqua::Button()` | activation |
| `Aqua::Scroll()` | scrollable viewport |
| `Aqua::Canvas()` | custom painter |
| `Aqua::Viewport3D()` | programmable GPU viewport |
| `Aqua::Checkbox(label)` | stateless toggle, app-owned `checked` |
| `Aqua::Slider(value)` | 0..=1 visual value |
| `Aqua::Dropdown(label)` | activator + open notification |
| `Aqua::Tabs()` | tab row, app-owned `selected` |
| `Aqua::Fragment()` | structural group, no node |
| `Aqua::Dynamic(source)` | explicit `ReadSignal` text |
| `Aqua::Media(image)` | visual asset layer |
| `Aqua::List::<T>()` | virtualized-capable row list |
| `Aqua::Table::<T>()` | virtualized-capable table |
| `Aqua::Column::<T>(id)` | column builder → `Column<T>` |
| `Aqua::App()` | root application builder |
| `Aqua::Config()` | authoring config builder |
| `Aqua::Window()` | authoring window builder |
| `Aqua::Style()` | empty partial style |

Capitalized names are intentional (`#[allow(non_snake_case)]` on the
impl).

## Children: arrays, `Vec`s, lazy iterators

`.children()` accepts arrays, vectors, or iterators and appends in call
order. It can be mixed with `.child()` without changing sibling order:

```rust
use aqua::Aqua;

let ui = Aqua::Box()
    .children([header(), composer()])
    .children(extra_components) // Vec<Element>
    .children(todos.iter().map(todo_row))
    .build();
```

These expressions run when the builder runs. To update mounted structure,
return the builder from `Context::view` with explicit signal dependencies;
use stable row keys to preserve identity across updates. Passing an
iterator alone does not create a reactive subscription.

## Collections: `List`, `Table`, `Column`, `fr`

```rust
use aqua::{Aqua, fr, px};

// 100k rows, opt-in virtualization (OFF by default), real styles.
let list = Aqua::List()
    .items(rows.iter().cloned())       // arrays, Vecs, lazy iterators
    .key(|r: &Row| r.id)              // K: Into<Key>
    .row(|r: &Row| Aqua::Text(r.name.clone()).build())
    .styles([LIST_STYLE])
    .virtualize()                     // opt in; default mounts every row
    .build();

// Table with a shared width plan and real header elements.
let table = Aqua::Table()
    .items(rows.iter().cloned())
    .key(|r: &Row| r.id)
    .columns([
        Aqua::Column("name")
            .header(|| Aqua::Text("Name").build())
            .cell(|r: &Row| Aqua::Text(r.name.clone()).build())
            .width(fr(2.0))
            .build(),
        Aqua::Column("value")
            .header(|| Aqua::Text("Value").build())
            .cell(|r: &Row| Aqua::Text(format!("{}", r.value)).build())
            .width(px(90.0))
            .min_width(60)
            .build(),
    ])
    .styles([TABLE_STYLE]) // bound the virtualized viewport here
    .header_styles([HEADER_STYLE])
    .row_styles([ROW_STYLE])
    .cell_styles([CELL_STYLE])
    .virtualize()
    .build();
```

Rules (frozen contract [`COLLECTION_IMPLEMENTATION_CONTRACT.md`](COLLECTION_IMPLEMENTATION_CONTRACT.md)):

- `fr` is a table-track helper returning `ColumnWidth`. It does not extend
  `Length`: padding, margins, radii, and window sizes stay px-only.
- Table headers are arbitrary real elements. Sorting/ordering is
  application-owned: put a real `Button` in the header that re-sorts your
  signal — never a fake control.
- Column widths use explicit `px`, `pct`, or proportional `fr` tracks.
  There is no implicit scan of virtual rows to fit content.
- Runtime construction implies no reactive subscription. Structural updates
  go through explicit `Context::view` dependencies; targeted prop updates
  flow through bindings.

## Explicit reactivity, no hidden subscriptions

```rust
fn app(cx: &aqua::Context) -> impl aqua::IntoView {
    let (rows_r, rows_w) = cx.scope().signal(Vec::<Row>::new());
    cx.view((rows_r,), move |(rows,)| render(&rows))
}
```

The dependency tuple is the whole subscription set. See
[`APPLICATION.md`](APPLICATION.md) and [`REACTIVITY.md`](REACTIVITY.md).

## Layout delegation

`UiApp::layout` delegates to `View::compute_layout(&mut UiTree, Size,
&dyn TextMeasurer)` when a view is mounted and falls back to direct tree
layout otherwise. App code never calls either directly: drive frames through
`sync()` + `build_scene_into(..)`.
