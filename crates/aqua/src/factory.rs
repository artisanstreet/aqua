//! Canonical Aqua authoring factories.
//!
//! `Aqua` remains the existing concrete runtime struct defined in
//! [`crate::runtime`]. The associated functions here are the canonical
//! authoring entry point: `Aqua::Box()`, `Aqua::Text(..)`, `Aqua::Button()`,
//! and so on. No second `Aqua` type or namespace alias is introduced.
//!
//! Collection builders (`List`/`Table`/`Column`) and the `fr` track helper
//! live in `aqua-ui/src/collection.rs` and are re-exported here. This module adds no collection
//! builder/type/sizing definitions of its own — just the `Aqua::`
//! forwarding functions.

use aqua_core::ImageId;
use aqua_reactive::ReadSignal;

use aqua_ui::element::{
    ui as builders, Button, Canvas, Checkbox, Dropdown, Dynamic, Fragment, Media, NumberArg,
    Scroll, Slider, Tabs, Text, TextSource, Viewport3D,
};

// Collection types are re-exported here so the
// canonical `Aqua::List()` / `aqua::fr` paths resolve to the real types.
pub use aqua_ui::collection::{fr, Column, ColumnBuilder, ColumnWidth, List, Table};

use crate::config::{Config, Window};
use crate::runtime::Aqua;
use aqua_ui::style::Style;

// ---------------------------------------------------------------------------
// Canonical factories on the existing concrete runtime struct.
// Capitalized names are intentional (`#[allow(non_snake_case)]`).
// ---------------------------------------------------------------------------

#[allow(non_snake_case)]
impl Aqua {
    /// Container builder (`Box::new()`).
    pub fn Box() -> builders::Box {
        builders::Box::new()
    }

    /// Text builder with content (literal or `ReadSignal`; see [`TextSource`]).
    pub fn Text(content: impl TextSource) -> Text {
        Text::new(content)
    }

    /// Button builder.
    pub fn Button() -> Button {
        Button::new()
    }

    /// Scroll builder.
    pub fn Scroll() -> Scroll {
        Scroll::new()
    }

    /// Canvas builder.
    pub fn Canvas() -> Canvas {
        Canvas::new()
    }

    /// 3D viewport builder.
    pub fn Viewport3D() -> Viewport3D {
        Viewport3D::new()
    }

    /// Checkbox builder with its label.
    pub fn Checkbox(label: impl Into<String>) -> Checkbox {
        Checkbox::new(label)
    }

    /// Slider builder with its 0..=1 value.
    pub fn Slider(value: impl NumberArg) -> Slider {
        Slider::new(value)
    }

    /// Dropdown builder with its activator label.
    pub fn Dropdown(label: impl Into<String>) -> Dropdown {
        Dropdown::new(label)
    }

    /// Tabs builder.
    pub fn Tabs() -> Tabs {
        Tabs::new()
    }

    /// Structural fragment builder (no visual node).
    pub fn Fragment() -> Fragment {
        Fragment::new()
    }

    /// Explicit dynamic-text builder from a `ReadSignal`.
    pub fn Dynamic(source: ReadSignal<String>) -> Dynamic {
        Dynamic::new(source)
    }

    /// Media builder with its asset.
    pub fn Media(image: ImageId) -> Media {
        Media::new(image)
    }

    /// Collection list builder (single source: `aqua_ui::collection`).
    pub fn List<T>() -> List<T> {
        List::new()
    }

    /// Collection table builder (single source: `aqua_ui::collection`).
    pub fn Table<T>() -> Table<T> {
        Table::new()
    }

    /// Column builder with its id (completes to a type-erased `Column<T>`).
    pub fn Column<T>(id: impl Into<String>) -> ColumnBuilder<T> {
        ColumnBuilder::new(id)
    }

    /// Canonical root application builder.
    pub fn App() -> crate::app::App {
        crate::app::App::new()
    }

    /// Canonical authoring config builder.
    pub fn Config() -> Config {
        Config::new()
    }

    /// Canonical authoring window builder.
    pub fn Window() -> Window {
        Window::new()
    }

    /// Canonical partial style (all fields unset).
    pub const fn Style() -> Style {
        Style::new()
    }
}
