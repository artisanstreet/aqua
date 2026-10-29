# Aqua builders and collections implementation contract

## Canonical authoring

`Aqua` remains the existing concrete runtime struct. Its capitalized
associated factories are the canonical entry point: `Aqua::Box()`,
`Text(content)`, `Button()`, `Scroll()`, `Media(id)`, `Canvas()`,
`Viewport3D()`, `Checkbox(label)`, `Slider(value)`, `Dropdown(label)`,
`Tabs()`, `Fragment()`, `Dynamic(source)`, `List<T>()`, `Table<T>()`,
`Column<T>(id)`, `App()`, `Config()`, `Window()`, and `Style()`.
No second Aqua struct, namespace alias, or reactive runtime is introduced.
Container `.children(impl IntoIterator<Item = impl Into<Element>>)` appends
in call order and works with arrays, Vecs, and lazy iterators. Existing
single-child calls remain valid. Runtime construction does not imply an
implicit reactive subscription; explicit Context.view dependencies remain.

## Public collections

List and Table builders live in `aqua-ui/src/collection.rs`. Both support
`items(IntoIterator<Item=T>)`, `key(Fn(&T)->K where K:Into<Key>)`, styles,
and `.virtualize()` (OFF by default). List has `.row(Fn(&T)->Element)`.
Table has `.columns(IntoIterator<Item=Column<T>>)`; completed column
builders erase closure types, permitting heterogeneous cells in arrays.
Column builders provide id, arbitrary Element header, cell factory,
width(px(...) or fr(...)), and min/max widths. `fr` is a table track
helper, not an extension of Length's meaning for padding etc.
Table exposes header_styles/row_styles/cell_styles, all normal Style
merges. Each column's outer cell box enforces the shared width; arbitrary
inner elements remain fully styleable. Header/body use one width plan and
horizontal scroll offset. Fit-to-content is explicit, never an implicit
scan of all virtual rows. Ordering/sorting remains application-owned.

## Frozen internal seam (builder owner implements)

`collection.rs` exports:

```rust,ignore
pub struct CollectionLayout {
    pub content_width: f32,
    pub column_widths: Vec<f32>,
}
pub enum CollectionRole { List, Table }
pub trait CollectionSource {
    fn len(&self) -> usize;
    fn key(&self, index: usize) -> Key;
    fn layout(&self, available_width: f32) -> CollectionLayout;
    fn row(&self, index: usize, layout: &CollectionLayout) -> Element;
    fn header(&self, layout: &CollectionLayout) -> Option<Element>;
    fn role(&self) -> CollectionRole;
    fn validate(&self) -> Result<(), CollectionError>;
}
pub struct CollectionDescriptor {
    pub source: std::rc::Rc<dyn CollectionSource>,
    pub styles: Vec<Style>,
    pub virtualized: bool,
    pub estimated_row_height: f32, // default 32 logical px
    pub overscan: usize,           // default 4 rows per side
}
```

Key/Element come from element.rs. CollectionSource is immutable snapshot
data; descriptors refresh through the existing View reconciliation.
CollectionError implements Debug/Clone/PartialEq/Display/Error. Missing
factories/keys, duplicate keys/column ids, invalid sizes/tracks fail at
mount/layout through existing errors, never silently omit rows or panic.
The source may cache a column plan by width, but must not visit all cells
on every scroll. Add reasonable optional measurement/controller seams
only if required; publish exact additions before peers depend on them.

## Runtime seam (runtime owner implements)

`Element::collection(CollectionDescriptor) -> Element` lowers into normal
UI nodes; no renderer primitive, second graph, or platform coupling.
`Element::role(aqua_core::Role) -> Element` sets explicit semantics.
`View::compute_layout(&mut self, tree:&mut UiTree, viewport:Size,
measurer:&dyn TextMeasurer) -> Result<(),String>` integrates collection
realization, bindings, layout, and measured heights. UiApp.layout delegates
to it when a View exists; ordinary trees still use UiTree.compute_layout.

Virtualization uses actual scroll offsets and committed viewport bounds,
bounded overscan, measured variable heights and indexed prefix sums.
Layout settlement is bounded; unknown/zero viewport never means mount all
rows. Preserve a stable key + within-row reading anchor on prepends,
reorders, and height changes. Reuse keyed mounted rows, pin a focused/editing
row without mounting the entire gap, and dispose scopes of unmounted rows.
Unvirtualized mode mounts every row through the same row factories.
Table bodies remain scrollable in both modes so wide columns are reachable;
headers stay outside vertical scrolling and share the horizontal offset.
Mounted collection/row/cell semantics must be correct; total counts and
logical indices should be reported through core metadata where available.
No native screen-reader verification is claimed from headless tests.

## Ownership

- Runtime worker: virtualization.rs, collection_runtime.rs, element.rs,
  binding.rs, tree.rs, layout.rs/events.rs only as necessary. Owns runtime
  tests in aqua-ui/tests/collection_runtime.rs. Also adds children methods.
- Collection API worker: collection.rs, aqua-ui/tests/collection_builders.rs,
  docs/COLLECTIONS.md. Owns List/Table/Column sources and sizing math.
- Facade worker: aqua/src/factory.rs/lib.rs/ui_app.rs; examples/todo.rs;
  aqua/tests/factories.rs; README and authoring docs. Does not edit runtime
  Aqua implementation internals or Windows adapter. Also adds a substantial
  collection demo when source contracts land.
- Parent: module wiring, core/platform semantics, independent review,
  final integration, validation, Windows build/native runs.

All workers use the shared absolute workspace and never reset/stage/commit.
Do not repeatedly poll cargo while another owner is mid-edit. Publish
interfaces early, finish owned work, then freeze for parent gates.
