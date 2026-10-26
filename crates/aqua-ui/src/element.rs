//! Element descriptors and component builders (macro-agent owned).
//!
//! `Element` is the macro-friendly authoring type: `ui!` expands to builder
//! calls against [`ui`] (re-exported so the `aqua` facade can alias it as
//! `aqua::ui`). Conversion into core-owned [`NodeSpec`] plus binding
//! registration happens here; the tree never imports this module.
//!
//! Reactivity stance: builders take plain values and explicit
//! [`aqua_reactive`] handles. Passing a [`ReadSignal`] declares the input;
//! the corresponding effect is created at mount ([`binding::View`]) and
//! pushes [`UiCommand::SetText`] through the shared [`CommandSink`]. The
//! macro performs zero reads and creates zero subscriptions.

use std::hash::Hash;

use aqua_core::{
    CollectionSemantics, DisplayItem, ImageId, NodeId, Oklch, Point, Rect, Role, ViewportId,
};
use aqua_reactive::{ReadSignal, WriteSignal};

use crate::collection_runtime::{
    validate_descriptor, validate_snapshot, CollectionDescriptor, CollectionError, CollectionRole,
};
use crate::style::{Axis, LayoutStyle, Length, Paint, Style, Typography};
use crate::tree::{
    hash_key, ActionCallback, ActionEvent, ActionKind, BoxContent, CanvasContent,
    CanvasPaintCallback, MediaContent, NodeKind, NodeSpec, ScrollContent, TextBacking, TextContent,
    UiTree, ViewportContent,
};
use aqua_text::{LineMode, Selection};

/// User key: hashed with a fixed seed at mount for keyed reconciliation.
/// Anything the macro's `.with_key(expr)` accepts must convert into this.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    /// String key.
    Str(String),
    /// Integer keys (stable `Display` mapping is NOT used; the value hashes).
    U64(u64),
    /// Integer keys.
    U32(u32),
    /// Integer keys.
    Usize(usize),
    /// Integer keys.
    I32(i32),
}

impl From<&str> for Key {
    fn from(v: &str) -> Self {
        Key::Str(v.to_owned())
    }
}

impl From<String> for Key {
    fn from(v: String) -> Self {
        Key::Str(v)
    }
}

impl From<u64> for Key {
    fn from(v: u64) -> Self {
        Key::U64(v)
    }
}

impl From<u32> for Key {
    fn from(v: u32) -> Self {
        Key::U32(v)
    }
}

impl From<usize> for Key {
    fn from(v: usize) -> Self {
        Key::Usize(v)
    }
}

impl From<i32> for Key {
    fn from(v: i32) -> Self {
        Key::I32(v)
    }
}

/// Numeric property values: integer literals (`gap: 16`) and floats both work.
pub trait NumberArg {
    /// Convert to logical px / unitless float.
    fn into_f32(self) -> f32;
}

impl NumberArg for i32 {
    fn into_f32(self) -> f32 {
        self as f32
    }
}

impl NumberArg for u32 {
    fn into_f32(self) -> f32 {
        self as f32
    }
}

impl NumberArg for usize {
    fn into_f32(self) -> f32 {
        self as f32
    }
}

impl NumberArg for f32 {
    fn into_f32(self) -> f32 {
        self
    }
}

impl NumberArg for f64 {
    fn into_f32(self) -> f32 {
        self as f32
    }
}

impl From<i32> for Length {
    fn from(v: i32) -> Self {
        Length::Px(v as f32)
    }
}

impl From<u32> for Length {
    fn from(v: u32) -> Self {
        Length::Px(v as f32)
    }
}

impl From<usize> for Length {
    fn from(v: usize) -> Self {
        Length::Px(v as f32)
    }
}

impl From<f32> for Length {
    fn from(v: f32) -> Self {
        Length::Px(v)
    }
}

impl From<f64> for Length {
    fn from(v: f64) -> Self {
        Length::Px(v as f32)
    }
}

impl From<&str> for Axis {
    /// `"row"` → [`Axis::Row`], `"column"`/`"col"` → [`Axis::Column`].
    /// Anything else panics with the valid set (fail fast at setup, never a
    /// silent fallback axis).
    fn from(v: &str) -> Self {
        match v {
            "row" => Axis::Row,
            "column" | "col" => Axis::Column,
            other => {
                panic!("aqua-ui: unknown layout axis `{other}` (expected \"row\" or \"column\")")
            }
        }
    }
}

impl From<String> for Axis {
    fn from(v: String) -> Self {
        Axis::from(v.as_str())
    }
}

/// Explicit typography overrides carried by every builder.
///
/// Only typography (font size, text color, static wrap) inherits down the
/// element tree — never layout or paint. Each builder records exactly the
/// fields the author set explicitly (via [`Style`] or the direct text
/// setters); at lowering, a child's effective typography is its own
/// overrides over its parent's, resolved against [`Typography::default()`].
/// A builder that sets nothing inherits everything.
///
/// Sealed implementation detail (only constructed inside this module).
#[doc(hidden)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TypoChain {
    font_size: Option<f32>,
    color: Option<Oklch>,
    wrap: Option<bool>,
}

impl TypoChain {
    /// Child chain: own explicit fields win, otherwise the parent's.
    pub(crate) fn child(&self, own: &TypoChain) -> TypoChain {
        TypoChain {
            font_size: own.font_size.or(self.font_size),
            color: own.color.or(self.color),
            wrap: own.wrap.or(self.wrap),
        }
    }

    /// Resolve against the tree defaults (14px / WHITE / no wrap).
    fn resolve(&self) -> Typography {
        let base = Typography::default();
        Typography {
            font_size: self.font_size.unwrap_or(base.font_size),
            color: self.color.unwrap_or(base.color),
            wrap: self.wrap.unwrap_or(base.wrap),
        }
    }
}

/// Mutable resolution targets for one builder: its resolved layout, its
/// resolved paint (rendered for every node kind — no wrapper nodes, no
/// dropped fields), and its explicit typography overrides (always present —
/// every builder propagates inheritance).
///
/// Sealed implementation detail (all implementors live in this module).
#[doc(hidden)]
pub trait Stylable {
    /// Borrow the resolved parts. Implemented by destructuring the
    /// builder's fields so the three borrows are disjoint.
    fn style_parts(&mut self) -> (&mut LayoutStyle, &mut Paint, &mut TypoChain);
    /// Post-apply hook for builders whose rendered props live outside
    /// `LayoutStyle`/`Paint` (text's own fields mirror its explicit type).
    /// Default: none.
    fn post_style(&mut self, _style: &Style) {}
}

/// Seal for [`StyleArg`]: only [`Style`] and [`LayoutStyle`] (by value or
/// reference) are valid `.style(...)` arguments. External implementations
/// are impossible by construction, so the public trait stays closed while
/// [`Stylable`] remains a hidden implementation detail.
mod private {
    use crate::style::{LayoutStyle, Style};

    pub trait Sealed {}
    impl Sealed for Style {}
    impl Sealed for &Style {}
    impl Sealed for LayoutStyle {}
    impl Sealed for &LayoutStyle {}
}

/// Argument accepted by every builder's `.style(...)`: a partial [`Style`]
/// (explicitly-set fields merge in call order; unset fields never clobber)
/// or a legacy total [`LayoutStyle`] (replaces the resolved layout, the
/// historical theme-adapter behavior). Sealed: implementable only for the
/// two style types above.
pub trait StyleArg: private::Sealed {
    /// Apply onto a builder's resolved parts.
    fn apply(&self, target: &mut dyn Stylable);
    /// Record onto a collection descriptor's ordered style list (same
    /// merge semantics: partial styles merge, legacy layouts replace).
    fn push_onto(&self, out: &mut Vec<Style>);
}

impl StyleArg for Style {
    fn apply(&self, target: &mut dyn Stylable) {
        {
            let (layout, paint, typo) = target.style_parts();
            // Every builder carries a real `Paint`; the tree renders it for
            // every node kind (whole-node opacity, border-only, clip) with
            // no wrapper nodes. Nothing is dropped here: per-corner radii
            // are rejected loudly at lowering (see `LowerError`).
            self.apply_to(layout, paint);
            if let Some(v) = self.font_size_opt() {
                typo.font_size = Some(v);
            }
            if let Some(v) = self.text_color_opt() {
                typo.color = Some(v);
            }
            if let Some(v) = self.text_wrap_opt() {
                typo.wrap = Some(v);
            }
        }
        target.post_style(self);
    }

    fn push_onto(&self, out: &mut Vec<Style>) {
        out.push(self.clone());
    }
}

impl StyleArg for &Style {
    fn apply(&self, target: &mut dyn Stylable) {
        (*self).apply(target);
    }

    fn push_onto(&self, out: &mut Vec<Style>) {
        out.push((*self).clone());
    }
}

impl StyleArg for LayoutStyle {
    fn apply(&self, target: &mut dyn Stylable) {
        let (layout, _, _) = target.style_parts();
        *layout = self.clone();
    }

    fn push_onto(&self, out: &mut Vec<Style>) {
        out.push(resolved_layout_style(self));
    }
}

impl StyleArg for &LayoutStyle {
    fn apply(&self, target: &mut dyn Stylable) {
        (*self).apply(target);
    }

    fn push_onto(&self, out: &mut Vec<Style>) {
        out.push(resolved_layout_style(self));
    }
}

/// Convert a resolved [`LayoutStyle`] into an equivalent total [`Style`]:
/// every layout field is set explicitly (later partial styles still merge
/// over per field), paint fields stay unset (a legacy layout replaces the
/// layout and leaves paint alone). Uses only public setters — no access to
/// `Style` internals is required.
fn resolved_layout_style(layout: &LayoutStyle) -> Style {
    use taffy::style::{AlignItems, JustifyContent};
    let mut s = Style::new()
        .width(layout.width)
        .height(layout.height)
        .min_width(layout.min_width)
        .min_height(layout.min_height)
        .max_width(layout.max_width)
        .max_height(layout.max_height)
        .flex_grow(layout.flex_grow)
        .flex_shrink(layout.flex_shrink)
        .axis(layout.axis)
        .wrap(layout.wrap)
        .align(layout.align)
        .justify(layout.justify)
        .gap(layout.gap)
        .padding_insets(layout.padding)
        .margin_insets(layout.margin);
    s = match layout.align_self {
        Some(a) => s.align_self(a),
        None => s.clear_align_self(),
    };
    s = match layout.column_gap {
        Some(g) => s.column_gap(g),
        None => s.clear_column_gap(),
    };
    s = match layout.row_gap {
        Some(g) => s.row_gap(g),
        None => s.clear_row_gap(),
    };
    s = match layout.grid_columns.clone() {
        Some(t) => s.grid_columns(t),
        None => s.clear_grid_columns(),
    };
    s = match layout.grid_rows.clone() {
        Some(t) => s.grid_rows(t),
        None => s.clear_grid_rows(),
    };
    s = match layout.absolute {
        Some(a) => s.absolute(a),
        None => s.clear_absolute(),
    };
    // Touch the imported names so refactors that drop a field mapping fail
    // loudly at build time instead of silently changing semantics.
    let _: AlignItems = layout.align;
    let _: JustifyContent = layout.justify;
    s
}

/// A value a text node can display: a literal (plain data) or an explicit
/// [`ReadSignal`] (declares the dynamic input; the mount step snapshots the
/// initial text and subscribes the update effect — no hidden tracking).
pub trait TextSource {
    /// Resolve into initial text, pushing a subscription binding when dynamic.
    fn apply_to(self, initial: &mut String, bindings: &mut Vec<ElementBinding>);
}

impl TextSource for String {
    fn apply_to(self, initial: &mut String, _bindings: &mut Vec<ElementBinding>) {
        *initial = self;
    }
}

impl TextSource for &str {
    fn apply_to(self, initial: &mut String, _bindings: &mut Vec<ElementBinding>) {
        *initial = self.to_owned();
    }
}

impl TextSource for ReadSignal<String> {
    fn apply_to(self, initial: &mut String, bindings: &mut Vec<ElementBinding>) {
        // Explicit mount-time snapshot (documented, not a subscription).
        *initial = self.try_get().unwrap_or_default();
        bindings.push(ElementBinding::SubscribeText(self));
    }
}

/// Mount-time binding attached to a mounted node. Applied once per node id by
/// [`crate::binding::View`]; effects reference the [`NodeId`], never rebuilt
/// subtrees.
pub enum ElementBinding {
    /// Subscribe a text source: effect pushes `UiCommand::SetText`.
    SubscribeText(ReadSignal<String>),
    /// Two-way editable binding: effect pushes `SetText`; `TextChanged`
    /// actions write back through the write handle (`try_set`, never panics).
    SubscribeEdit {
        /// Read side (effect input).
        read: ReadSignal<String>,
        /// Write side (event handler target).
        write: WriteSignal<String>,
    },
    /// Register an action callback (activation, custom events).
    OnAction {
        /// Which action to listen for.
        kind: ActionKind,
        /// Data-only callback (cannot reenter the tree by signature).
        callback: ActionCallback,
    },
    /// Make the node focusable (keyboard activation path).
    Focusable,
    /// Register a canvas painter callback.
    Painter(CanvasPaintCallback),
}

/// Shared layout sugar implemented once per builder (each builder holds its
/// own [`LayoutStyle`]).
macro_rules! layout_sugar {
    () => {
        /// Main axis (`"row"` / `"column"`, or an [`Axis`]).
        pub fn layout(mut self, axis: impl Into<Axis>) -> Self {
            self.style.axis = axis.into();
            self
        }

        /// Gap between children, both axes.
        pub fn gap(mut self, gap: impl NumberArg) -> Self {
            self.style.gap = gap.into_f32();
            self
        }

        /// Fixed width (`200` → `Px(200)`).
        pub fn width(mut self, width: impl Into<Length>) -> Self {
            self.style.width = width.into();
            self
        }

        /// Fixed height.
        pub fn height(mut self, height: impl Into<Length>) -> Self {
            self.style.height = height.into();
            self
        }

        /// Uniform inner spacing in logical px.
        pub fn padding(mut self, padding: impl NumberArg) -> Self {
            let p = padding.into_f32();
            self.style.padding = aqua_core::Insets {
                left: p,
                right: p,
                top: p,
                bottom: p,
            };
            self
        }

        /// Apply a single style, in call order with every other setter.
        ///
        /// A [`Style`](crate::style::Style) merges only its explicitly-set
        /// fields (later calls win per field; unset fields never clobber,
        /// `clear_*` actively clears). A [`LayoutStyle`] replaces the whole
        /// resolved layout (legacy theme-adapter behavior): set granular
        /// props after this call. `ui!` `style:` / `styles:` map here, so
        /// `Box(styles: [theme, override_])` naturally works.
        pub fn style(mut self, style: impl StyleArg) -> Self {
            style.apply(&mut self);
            self
        }

        /// Apply several styles in order: equivalent to calling
        /// [`.style`](Self::style) once per entry. Fixed arrays, vecs, and
        /// slices all work (`impl AsRef<[Style]>`).
        pub fn styles(mut self, styles: impl AsRef<[Style]>) -> Self {
            for s in styles.as_ref() {
                s.apply(&mut self);
            }
            self
        }

        /// Background fill. Rendered for every node kind (no wrapper).
        pub fn fill(mut self, color: Oklch) -> Self {
            self.paint.fill = Some(color);
            self
        }

        /// Uniform corner radius. Per-corner radii are not renderable and
        /// are rejected loudly at lowering — use this, not `corners`.
        pub fn radius(mut self, radius: impl NumberArg) -> Self {
            self.paint.radius = radius.into_f32();
            self
        }

        /// Uniform border: `(width, color)` tuple (single-expression friendly).
        /// Border-only (no fill) still renders.
        pub fn border(mut self, border: (impl NumberArg, Oklch)) -> Self {
            self.paint.border = Some(crate::style::Border {
                width: border.0.into_f32(),
                color: border.1,
            });
            self
        }

        /// Clip children to this node's bounds.
        pub fn clip(mut self, clip: bool) -> Self {
            self.paint.clip = clip;
            self
        }

        /// Group opacity multiplier 0..=1. Applies to the whole styled
        /// node: background, content, and children.
        pub fn opacity(mut self, opacity: impl NumberArg) -> Self {
            self.paint.opacity = Some(opacity.into_f32());
            self
        }

        /// Replace the whole paint block.
        pub fn paint(mut self, paint: Paint) -> Self {
            self.paint = paint;
            self
        }
    };
}

/// Box (container) builder. `Box::builder()` starts it; `.build()` finishes.
/// `Box::new()` is the same starting point (canonical builders stay).
pub struct Box {
    style: LayoutStyle,
    paint: Paint,
    typo: TypoChain,
    children: Vec<Element>,
}

impl Box {
    /// Start a box builder.
    pub fn builder() -> Self {
        Self {
            style: LayoutStyle::default(),
            paint: Paint::default(),
            typo: TypoChain::default(),
            children: Vec::new(),
        }
    }

    /// Alias for [`Box::builder`].
    pub fn new() -> Self {
        Self::builder()
    }

    layout_sugar!();

    /// Append a child element.
    pub fn child(mut self, child: impl Into<Element>) -> Self {
        self.children.push(child.into());
        self
    }

    /// Append several children in call order (arrays, `Vec`s, and lazy
    /// iterators all work). Repeated calls append; existing single-child
    /// calls remain valid.
    pub fn children<I, E>(mut self, children: I) -> Self
    where
        I: IntoIterator<Item = E>,
        E: Into<Element>,
    {
        for c in children {
            self.children.push(c.into());
        }
        self
    }

    /// Finish as an [`Element`].
    pub fn build(mut self) -> Element {
        let children = std::mem::take(&mut self.children);
        Element {
            kind: ElementKind::Box(self),
            key: None,
            role: None,
            collection_semantics: None,
            children,
            synth: Vec::new(),
            bindings: Vec::new(),
        }
    }
}

/// Text builder: labels, read-only bound text, and editable fields.
pub struct Text {
    style: LayoutStyle,
    paint: Paint,
    typo: TypoChain,
    initial: String,
    font_size: f32,
    color: Oklch,
    editable: bool,
    multiline: bool,
    wrap: bool,
    selectable: bool,
    password: bool,
    placeholder: String,
    pending: Vec<ElementBinding>,
    children: Vec<Element>,
}

impl Default for Text {
    fn default() -> Self {
        Self {
            style: LayoutStyle::default(),
            paint: Paint::default(),
            typo: TypoChain::default(),
            initial: String::new(),
            font_size: 14.0,
            color: Oklch::WHITE,
            editable: false,
            multiline: false,
            wrap: false,
            selectable: false,
            password: false,
            placeholder: String::new(),
            pending: Vec::new(),
            children: Vec::new(),
        }
    }
}

impl Text {
    /// Start a text builder.
    pub fn builder() -> Self {
        Self::default()
    }

    /// Start a text builder with content: `Text::new("hi")`,
    /// `Text::new(title)`, `Text::new(label_signal)`. Same rules as
    /// [`.arg`](Self::arg): literals are static, signals subscribe.
    pub fn new(content: impl TextSource) -> Self {
        Self::builder().arg(content)
    }

    layout_sugar!();

    /// Positional content: `Text("hi")`, `Text(title)`, `Text(label_signal)`.
    /// Literals set static text; a [`ReadSignal`] snapshots the initial text
    /// and subscribes `SetText` updates at mount.
    pub fn arg(mut self, value: impl TextSource) -> Self {
        value.apply_to(&mut self.initial, &mut self.pending);
        self
    }

    /// Named content: `Text(value: label)`. Same rules as [`Text::arg`]:
    /// literals are static, signals are dynamic (snapshot + subscribe).
    pub fn value(mut self, value: impl TextSource) -> Self {
        value.apply_to(&mut self.initial, &mut self.pending);
        self
    }

    /// Read-only dynamic text: snapshots now, subscribes `SetText` updates.
    /// Equivalent to passing the signal to [`Text::value`]; kept as the
    /// explicit spelled-out form.
    pub fn value_signal(mut self, source: ReadSignal<String>) -> Self {
        source.apply_to(&mut self.initial, &mut self.pending);
        self
    }

    /// Canonical two-way editable binding: `(read, write)` tuple.
    /// Mount snapshots `read`, subscribes `SetText` updates, enables editing,
    /// and routes `TextChanged` back through `write` (`try_set`, infallible).
    pub fn bind(mut self, editable: (ReadSignal<String>, WriteSignal<String>)) -> Self {
        let (read, write) = editable;
        self.initial = read.try_get().unwrap_or_default();
        self.editable = true;
        self.pending
            .push(ElementBinding::SubscribeEdit { read, write });
        self
    }

    /// Font size in logical px. Explicit: wins over inherited styles,
    /// and is itself inherited by descendant text.
    pub fn font_size(mut self, size: impl NumberArg) -> Self {
        let size = size.into_f32();
        self.font_size = size;
        self.typo.font_size = Some(size);
        self
    }

    /// Text color. Explicit: wins over inherited styles, and is itself
    /// inherited by descendant text.
    pub fn color(mut self, color: Oklch) -> Self {
        self.color = color;
        self.typo.color = Some(color);
        self
    }

    /// Editable field (editor-backed, grapheme-safe).
    pub fn editable(mut self, editable: bool) -> Self {
        self.editable = editable;
        self
    }

    /// Multiline editing/wrapping.
    pub fn multiline(mut self, multiline: bool) -> Self {
        self.multiline = multiline;
        self
    }

    /// Wrap static labels at the available width. Explicit: wins over
    /// inherited styles, and is itself inherited by descendant text.
    pub fn wrap(mut self, wrap: bool) -> Self {
        self.wrap = wrap;
        self.typo.wrap = Some(wrap);
        self
    }

    /// Whether a static label can be selected.
    pub fn selectable(mut self, selectable: bool) -> Self {
        self.selectable = selectable;
        self
    }

    /// Password masking (bullets for measure/display).
    pub fn password(mut self, password: bool) -> Self {
        self.password = password;
        self
    }

    /// Placeholder shown while an editable node is empty (display only —
    /// committed text is unaffected). Ignored for static labels, matching
    /// [`TextContent`](crate::tree::TextContent) semantics.
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Notify when this text field gains or loses keyboard focus.
    pub fn on_focus(mut self, handler: impl Fn(bool) + 'static) -> Self {
        self.pending.push(ElementBinding::OnAction {
            kind: ActionKind::FocusChanged,
            callback: std::boxed::Box::new(move |event| handler(event.focused.unwrap_or(false))),
        });
        self
    }

    /// Submit a single-line field with Enter. Pointer presses do not submit.
    pub fn on_submit(mut self, handler: impl Fn() + 'static) -> Self {
        self.pending.push(ElementBinding::OnAction {
            kind: ActionKind::Submit,
            callback: std::boxed::Box::new(move |_| handler()),
        });
        self
    }

    /// Append a child element (rare for text; supported for uniformity).
    pub fn child(mut self, child: impl Into<Element>) -> Self {
        self.children.push(child.into());
        self
    }

    /// Append several children in call order (arrays, `Vec`s, and lazy
    /// iterators all work). Repeated calls append; existing single-child
    /// calls remain valid.
    pub fn children<I, E>(mut self, children: I) -> Self
    where
        I: IntoIterator<Item = E>,
        E: Into<Element>,
    {
        for c in children {
            self.children.push(c.into());
        }
        self
    }

    /// Finish as an [`Element`].
    pub fn build(mut self) -> Element {
        let children = std::mem::take(&mut self.children);
        let bindings = std::mem::take(&mut self.pending);
        Element {
            kind: ElementKind::Text(self),
            key: None,
            role: None,
            collection_semantics: None,
            children,
            synth: Vec::new(),
            bindings,
        }
    }
}

/// Media builder: visual asset presentation (raster/vector/video via image
/// layers; audio is out of scope for this visual primitive).
pub struct Media {
    style: LayoutStyle,
    paint: Paint,
    typo: TypoChain,
    image: Option<ImageId>,
    opacity: f32,
    children: Vec<Element>,
}

impl Media {
    /// Start a media builder.
    pub fn builder() -> Self {
        Self {
            style: LayoutStyle::default(),
            paint: Paint::default(),
            typo: TypoChain::default(),
            image: None,
            opacity: 1.0,
            children: Vec::new(),
        }
    }

    /// Start a media builder with its asset: `Media::new(image_id)`.
    pub fn new(image: ImageId) -> Self {
        Self::builder().arg(image)
    }

    layout_sugar!();

    /// Positional asset: `Media(image_id)`.
    pub fn arg(mut self, image: ImageId) -> Self {
        self.image = Some(image);
        self
    }

    /// Asset layer to present (`None` = reserve layout, paint nothing).
    pub fn image(mut self, image: ImageId) -> Self {
        self.image = Some(image);
        self
    }

    /// Append a child element.
    pub fn child(mut self, child: impl Into<Element>) -> Self {
        self.children.push(child.into());
        self
    }

    /// Append several children in call order (arrays, `Vec`s, and lazy
    /// iterators all work). Repeated calls append; existing single-child
    /// calls remain valid.
    pub fn children<I, E>(mut self, children: I) -> Self
    where
        I: IntoIterator<Item = E>,
        E: Into<Element>,
    {
        for c in children {
            self.children.push(c.into());
        }
        self
    }

    /// Finish as an [`Element`].
    pub fn build(mut self) -> Element {
        let children = std::mem::take(&mut self.children);
        Element {
            kind: ElementKind::Media(self),
            key: None,
            role: None,
            collection_semantics: None,
            children,
            synth: Vec::new(),
            bindings: Vec::new(),
        }
    }
}

/// Scroll builder: scrollable viewport with offset, clipping, interaction.
pub struct Scroll {
    style: LayoutStyle,
    paint: Paint,
    typo: TypoChain,
    offset: Point,
    children: Vec<Element>,
}

impl Scroll {
    /// Start a scroll builder.
    pub fn builder() -> Self {
        Self {
            style: LayoutStyle::default(),
            paint: Paint::default(),
            typo: TypoChain::default(),
            offset: Point::ZERO,
            children: Vec::new(),
        }
    }

    /// Alias for [`Scroll::builder`].
    pub fn new() -> Self {
        Self::builder()
    }

    layout_sugar!();

    /// Scroll offset as a [`Point`].
    pub fn offset(mut self, offset: Point) -> Self {
        self.offset = offset;
        self
    }

    /// Scroll offset as an `(x, y)` tuple.
    pub fn offset_xy(mut self, offset: (impl NumberArg, impl NumberArg)) -> Self {
        self.offset = Point::new(offset.0.into_f32(), offset.1.into_f32());
        self
    }

    /// Append a child element.
    pub fn child(mut self, child: impl Into<Element>) -> Self {
        self.children.push(child.into());
        self
    }

    /// Append several children in call order (arrays, `Vec`s, and lazy
    /// iterators all work). Repeated calls append; existing single-child
    /// calls remain valid.
    pub fn children<I, E>(mut self, children: I) -> Self
    where
        I: IntoIterator<Item = E>,
        E: Into<Element>,
    {
        for c in children {
            self.children.push(c.into());
        }
        self
    }

    /// Finish as an [`Element`].
    pub fn build(mut self) -> Element {
        let children = std::mem::take(&mut self.children);
        Element {
            kind: ElementKind::Scroll(self),
            key: None,
            role: None,
            collection_semantics: None,
            children,
            synth: Vec::new(),
            bindings: Vec::new(),
        }
    }
}

/// Canvas builder: application custom drawing integrated with layout.
/// The painter receives committed bounds and returns scene ops; it cannot
/// reach back into the tree (no reentrancy by signature).
pub struct Canvas {
    style: LayoutStyle,
    paint: Paint,
    typo: TypoChain,
    painter: Option<CanvasPaintCallback>,
    children: Vec<Element>,
}

impl Canvas {
    /// Start a canvas builder.
    pub fn builder() -> Self {
        Self {
            style: LayoutStyle::default(),
            paint: Paint::default(),
            typo: TypoChain::default(),
            painter: None,
            children: Vec::new(),
        }
    }

    /// Alias for [`Canvas::builder`].
    pub fn new() -> Self {
        Self::builder()
    }

    layout_sugar!();

    /// Positional painter: `Canvas(|bounds| vec![...])`.
    pub fn arg(mut self, painter: impl Fn(Rect) -> Vec<DisplayItem> + 'static) -> Self {
        self.painter = Some(std::boxed::Box::new(painter));
        self
    }

    /// Register the painter (`None` at the tree = reserve layout, emit nothing).
    pub fn on_paint(mut self, painter: impl Fn(Rect) -> Vec<DisplayItem> + 'static) -> Self {
        self.painter = Some(std::boxed::Box::new(painter));
        self
    }

    /// Append a child element.
    pub fn child(mut self, child: impl Into<Element>) -> Self {
        self.children.push(child.into());
        self
    }

    /// Append several children in call order (arrays, `Vec`s, and lazy
    /// iterators all work). Repeated calls append; existing single-child
    /// calls remain valid.
    pub fn children<I, E>(mut self, children: I) -> Self
    where
        I: IntoIterator<Item = E>,
        E: Into<Element>,
    {
        for c in children {
            self.children.push(c.into());
        }
        self
    }

    /// Finish as an [`Element`].
    pub fn build(mut self) -> Element {
        let children = std::mem::take(&mut self.children);
        let mut bindings = Vec::new();
        if let Some(painter) = self.painter.take() {
            bindings.push(ElementBinding::Painter(painter));
        }
        Element {
            kind: ElementKind::Canvas(self),
            key: None,
            role: None,
            collection_semantics: None,
            children,
            synth: Vec::new(),
            bindings,
        }
    }
}

/// Viewport3D builder: programmable GPU content under ordinary UI layout.
/// Custom renderers plug in through the render extension contract (media3d
/// owner); this builder only names the viewport and external layer.
pub struct Viewport3D {
    style: LayoutStyle,
    paint: Paint,
    typo: TypoChain,
    viewport: Option<ViewportId>,
    layer: Option<u64>,
    children: Vec<Element>,
}

impl Viewport3D {
    /// Start a viewport builder.
    pub fn builder() -> Self {
        Self {
            style: LayoutStyle::default(),
            paint: Paint::default(),
            typo: TypoChain::default(),
            viewport: None,
            layer: None,
            children: Vec::new(),
        }
    }

    /// Alias for [`Viewport3D::builder`].
    pub fn new() -> Self {
        Self::builder()
    }

    layout_sugar!();

    /// Viewport identity.
    pub fn viewport(mut self, viewport: ViewportId) -> Self {
        self.viewport = Some(viewport);
        self
    }

    /// External layer key (`None` = nothing to composite yet).
    pub fn layer(mut self, layer: u64) -> Self {
        self.layer = Some(layer);
        self
    }

    /// Append a child element (UI overlays above the viewport).
    pub fn child(mut self, child: impl Into<Element>) -> Self {
        self.children.push(child.into());
        self
    }

    /// Append several children in call order (arrays, `Vec`s, and lazy
    /// iterators all work). Repeated calls append; existing single-child
    /// calls remain valid.
    pub fn children<I, E>(mut self, children: I) -> Self
    where
        I: IntoIterator<Item = E>,
        E: Into<Element>,
    {
        for c in children {
            self.children.push(c.into());
        }
        self
    }

    /// Finish as an [`Element`].
    pub fn build(mut self) -> Element {
        let children = std::mem::take(&mut self.children);
        Element {
            kind: ElementKind::Viewport3D(self),
            key: None,
            role: None,
            collection_semantics: None,
            children,
            synth: Vec::new(),
            bindings: Vec::new(),
        }
    }
}

/// Button builder: a [`Box`] plus activation behavior. There is no separate
/// retained button primitive — changing visuals never drops activation,
/// focus, or accessibility because those live in bindings, not in the kind.
pub struct Button {
    style: LayoutStyle,
    paint: Paint,
    typo: TypoChain,
    on_activate: Option<ActionCallback>,
    children: Vec<Element>,
}

impl Button {
    /// Start a button builder.
    pub fn builder() -> Self {
        Self {
            style: LayoutStyle::default(),
            paint: Paint::default(),
            typo: TypoChain::default(),
            on_activate: None,
            children: Vec::new(),
        }
    }

    /// Alias for [`Button::builder`].
    pub fn new() -> Self {
        Self::builder()
    }

    layout_sugar!();

    /// Activation handler. Runs on pointer activation and on keyboard
    /// Enter/Space while focused. Must be fast and non-blocking; route tree
    /// mutations back through the [`CommandSink`](crate::tree::CommandSink).
    pub fn on_activate(mut self, handler: impl Fn() + 'static) -> Self {
        self.on_activate = Some(std::boxed::Box::new(move |_| handler()));
        self
    }

    /// Activation handler with event data (source node, kind).
    pub fn on_activate_event(mut self, handler: impl Fn(ActionEvent) + 'static) -> Self {
        self.on_activate = Some(std::boxed::Box::new(handler));
        self
    }

    /// Append a child element (usually a label [`Text`]).
    pub fn child(mut self, child: impl Into<Element>) -> Self {
        self.children.push(child.into());
        self
    }

    /// Append several children in call order (arrays, `Vec`s, and lazy
    /// iterators all work). Repeated calls append; existing single-child
    /// calls remain valid.
    pub fn children<I, E>(mut self, children: I) -> Self
    where
        I: IntoIterator<Item = E>,
        E: Into<Element>,
    {
        for c in children {
            self.children.push(c.into());
        }
        self
    }

    /// Finish as an [`Element`].
    pub fn build(mut self) -> Element {
        let children = std::mem::take(&mut self.children);
        let mut bindings = vec![ElementBinding::Focusable];
        if let Some(on_activate) = self.on_activate.take() {
            bindings.push(ElementBinding::OnAction {
                kind: ActionKind::Activate,
                callback: on_activate,
            });
        }
        Element {
            kind: ElementKind::Button(self),
            key: None,
            role: None,
            collection_semantics: None,
            children,
            synth: Vec::new(),
            bindings,
        }
    }
}

/// Checkbox builder: stateless control. `checked` is a *prop* reflecting app
/// state; `on_toggle` notifies the host, which owns the state and re-syncs.
/// No hidden retained value lives in the tree.
pub struct Checkbox {
    style: LayoutStyle,
    paint: Paint,
    typo: TypoChain,
    label: String,
    checked: bool,
    on_toggle: Option<ActionCallback>,
    children: Vec<Element>,
}

impl Checkbox {
    /// Start a checkbox builder. The row gap defaults to 8px (set once
    /// here, so an explicit `gap(0)` or style gap stays exactly as set).
    pub fn builder() -> Self {
        Self {
            style: LayoutStyle {
                gap: 8.0,
                ..LayoutStyle::default()
            },
            paint: Paint::default(),
            typo: TypoChain::default(),
            label: String::new(),
            checked: false,
            on_toggle: None,
            children: Vec::new(),
        }
    }

    /// Start a checkbox builder with its label: `Checkbox::new("done")`.
    pub fn new(label: impl Into<String>) -> Self {
        Self::builder().arg(label)
    }

    layout_sugar!();

    /// Positional label.
    pub fn arg(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// Label text.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// Checked state (app-owned; re-sync to update).
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    /// Toggle notification (host flips its state and re-syncs).
    pub fn on_toggle(mut self, handler: impl Fn() + 'static) -> Self {
        self.on_toggle = Some(std::boxed::Box::new(move |_| handler()));
        self
    }

    /// Append a child element (after the indicator + label).
    pub fn child(mut self, child: impl Into<Element>) -> Self {
        self.children.push(child.into());
        self
    }

    /// Append several children in call order (arrays, `Vec`s, and lazy
    /// iterators all work). Repeated calls append; existing single-child
    /// calls remain valid.
    pub fn children<I, E>(mut self, children: I) -> Self
    where
        I: IntoIterator<Item = E>,
        E: Into<Element>,
    {
        for c in children {
            self.children.push(c.into());
        }
        self
    }

    /// Finish as an [`Element`]. The indicator + label become synthetic
    /// children (real nodes, no bindings needed); `on_toggle` binds to the
    /// row's `Activate` action.
    pub fn build(mut self) -> Element {
        let children = std::mem::take(&mut self.children);
        let mut bindings = vec![ElementBinding::Focusable];
        if let Some(on_toggle) = self.on_toggle.take() {
            bindings.push(ElementBinding::OnAction {
                kind: ActionKind::Activate,
                callback: on_toggle,
            });
        }
        let mut indicator = Box::builder()
            .width(18)
            .height(18)
            .radius(5)
            .fill(Oklch::TRANSPARENT)
            .border((1.5, Oklch::oklch(0.58, 0.025, 255.0, 1.0)));
        if self.checked {
            indicator = indicator.fill(Oklch::WHITE);
        }
        let synth = vec![
            indicator.build(),
            Text::builder().arg(self.label.clone()).build(),
        ];
        Element {
            kind: ElementKind::Checkbox(self),
            key: None,
            role: None,
            collection_semantics: None,
            children,
            synth,
            bindings,
        }
    }
}

/// Slider builder: visual track + fill from a 0..=1 `value` prop.
///
/// Honest limit: drag/click-to-value interaction is `[UNIMPLEMENTED]` —
/// [`ActionEvent`] carries no pointer position, so no callback can compute a
/// value from a gesture yet. This builder renders real, mounted geometry;
/// value changes flow host → prop → sync. No fake `on_change` is offered.
pub struct Slider {
    style: LayoutStyle,
    paint: Paint,
    typo: TypoChain,
    value: f32,
    children: Vec<Element>,
}

impl Slider {
    /// Start a slider builder. Track size defaults to 160x24px (set once
    /// here, so an explicit `width(auto())` or style width stays as set).
    pub fn builder() -> Self {
        Self {
            style: LayoutStyle {
                width: Length::Px(160.0),
                height: Length::Px(24.0),
                ..LayoutStyle::default()
            },
            paint: Paint::default(),
            typo: TypoChain::default(),
            value: 0.0,
            children: Vec::new(),
        }
    }

    /// Start a slider builder with its value: `Slider::new(0.5)`.
    pub fn new(value: impl NumberArg) -> Self {
        Self::builder().arg(value)
    }

    layout_sugar!();

    /// Positional value 0..=1 (clamped at conversion).
    pub fn arg(mut self, value: impl NumberArg) -> Self {
        self.value = value.into_f32();
        self
    }

    /// Value 0..=1 (clamped at conversion). App-owned; re-sync to update.
    pub fn value(mut self, value: impl NumberArg) -> Self {
        self.value = value.into_f32();
        self
    }

    /// Append a child element.
    pub fn child(mut self, child: impl Into<Element>) -> Self {
        self.children.push(child.into());
        self
    }

    /// Append several children in call order (arrays, `Vec`s, and lazy
    /// iterators all work). Repeated calls append; existing single-child
    /// calls remain valid.
    pub fn children<I, E>(mut self, children: I) -> Self
    where
        I: IntoIterator<Item = E>,
        E: Into<Element>,
    {
        for c in children {
            self.children.push(c.into());
        }
        self
    }

    /// Finish as an [`Element`]. The fill bar is a synthetic child sized
    /// from the clamped value; re-sync with a new value rebuilds it. Track
    /// size defaults (160x24) live in the builder initializer, so an
    /// explicit `auto()` is never coerced back here.
    pub fn build(mut self) -> Element {
        let children = std::mem::take(&mut self.children);
        let value = self.value.clamp(0.0, 1.0);
        let fill = Box::builder()
            .width(Length::Pct(value * 100.0))
            .height(Length::Pct(100.0))
            .radius(4)
            .fill(Oklch::WHITE)
            .build();
        Element {
            kind: ElementKind::Slider(self),
            key: None,
            role: None,
            collection_semantics: None,
            children,
            synth: vec![fill],
            // Deliberately no bindings: without pointer position in
            // ActionEvent there is no honest gesture callback to offer.
            bindings: Vec::new(),
        }
    }
}

/// Dropdown builder: activator rendering + open notification. The popup list
/// itself integrates with the overlays agent's overlay service (pending);
/// this builder mounts the activator only, honestly labeled.
pub struct Dropdown {
    style: LayoutStyle,
    paint: Paint,
    typo: TypoChain,
    label: String,
    on_open: Option<ActionCallback>,
    children: Vec<Element>,
}

impl Dropdown {
    /// Start a dropdown builder.
    pub fn builder() -> Self {
        Self {
            style: LayoutStyle::default(),
            paint: Paint {
                fill: Some(Oklch::WHITE),
                radius: 4.0,
                ..Default::default()
            },
            typo: TypoChain::default(),
            label: String::new(),
            on_open: None,
            children: Vec::new(),
        }
    }

    /// Start a dropdown builder with its activator label.
    pub fn new(label: impl Into<String>) -> Self {
        Self::builder().arg(label)
    }

    layout_sugar!();

    /// Positional activator label.
    pub fn arg(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// Activator label.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// Fires on activation (host opens the popup through the overlay service).
    pub fn on_open(mut self, handler: impl Fn() + 'static) -> Self {
        self.on_open = Some(std::boxed::Box::new(move |_| handler()));
        self
    }

    /// Append a child element.
    pub fn child(mut self, child: impl Into<Element>) -> Self {
        self.children.push(child.into());
        self
    }

    /// Append several children in call order (arrays, `Vec`s, and lazy
    /// iterators all work). Repeated calls append; existing single-child
    /// calls remain valid.
    pub fn children<I, E>(mut self, children: I) -> Self
    where
        I: IntoIterator<Item = E>,
        E: Into<Element>,
    {
        for c in children {
            self.children.push(c.into());
        }
        self
    }

    /// Finish as an [`Element`]. The activator label (when non-empty) is a
    /// synthetic child; `on_open` binds to the row's `Activate` action.
    pub fn build(mut self) -> Element {
        let children = std::mem::take(&mut self.children);
        let mut bindings = vec![ElementBinding::Focusable];
        if let Some(on_open) = self.on_open.take() {
            bindings.push(ElementBinding::OnAction {
                kind: ActionKind::Activate,
                callback: on_open,
            });
        }
        let mut synth = Vec::new();
        if !self.label.is_empty() {
            synth.push(Text::builder().arg(self.label.clone()).build());
        }
        Element {
            kind: ElementKind::Dropdown(self),
            key: None,
            role: None,
            collection_semantics: None,
            children,
            synth,
            bindings,
        }
    }
}

/// Tabs builder: a row of tab buttons. Selection is an app-owned prop;
/// `on_select(index)` fires real activation per tab (index known statically).
/// Panel switching is the host's sync (show the selected panel's subtree).
pub struct Tabs {
    style: LayoutStyle,
    paint: Paint,
    typo: TypoChain,
    tabs: Vec<String>,
    selected: usize,
    on_select: Option<std::rc::Rc<dyn Fn(usize)>>,
    children: Vec<Element>,
}

impl Tabs {
    /// Start a tabs builder.
    pub fn builder() -> Self {
        Self {
            style: LayoutStyle::default(),
            paint: Paint::default(),
            typo: TypoChain::default(),
            tabs: Vec::new(),
            selected: 0,
            on_select: None,
            children: Vec::new(),
        }
    }

    /// Alias for [`Tabs::builder`].
    pub fn new() -> Self {
        Self::builder()
    }

    layout_sugar!();

    /// Tab labels.
    pub fn tabs(mut self, tabs: Vec<String>) -> Self {
        self.tabs = tabs;
        self
    }

    /// Selected tab index (app-owned; re-sync to update).
    pub fn selected(mut self, selected: usize) -> Self {
        self.selected = selected;
        self
    }

    /// Fires with the tab index on activation.
    pub fn on_select(mut self, handler: impl Fn(usize) + 'static) -> Self {
        self.on_select = Some(std::rc::Rc::new(handler));
        self
    }

    /// Append a child element (after the tab row).
    pub fn child(mut self, child: impl Into<Element>) -> Self {
        self.children.push(child.into());
        self
    }

    /// Append several children in call order (arrays, `Vec`s, and lazy
    /// iterators all work). Repeated calls append; existing single-child
    /// calls remain valid.
    pub fn children<I, E>(mut self, children: I) -> Self
    where
        I: IntoIterator<Item = E>,
        E: Into<Element>,
    {
        for c in children {
            self.children.push(c.into());
        }
        self
    }

    /// Finish as an [`Element`]. Tab buttons become synthetic children —
    /// real button elements with per-tab `Activate` bindings carrying the
    /// static tab index — followed by any explicit children.
    pub fn build(mut self) -> Element {
        let children = std::mem::take(&mut self.children);
        let mut synth = Vec::new();
        for (i, label) in self.tabs.iter().enumerate() {
            let mut tab = Button::builder();
            if i == self.selected {
                tab = tab.fill(Oklch::WHITE);
            }
            if let Some(on_select) = &self.on_select {
                let notify = std::rc::Rc::clone(on_select);
                tab = tab.on_activate(move || notify(i));
            }
            synth.push(
                tab.child(Text::builder().arg(label.clone()).build())
                    .build(),
            );
        }
        Element {
            kind: ElementKind::Tabs(self),
            key: None,
            role: None,
            collection_semantics: None,
            children,
            synth,
            bindings: Vec::new(),
        }
    }
}

/// Dynamic builder: explicit dynamic text. `Dynamic(value: source)` snapshots
/// the source at mount and subscribes `SetText` updates — the spelled-out
/// equivalent of passing a signal to [`Text::value`]. No implicit tracking:
/// the input is declared by passing the handle.
pub struct Dynamic {
    style: LayoutStyle,
    paint: Paint,
    typo: TypoChain,
    source: Option<ReadSignal<String>>,
    children: Vec<Element>,
}

impl Dynamic {
    /// Start a dynamic builder.
    pub fn builder() -> Self {
        Self {
            style: LayoutStyle::default(),
            paint: Paint::default(),
            typo: TypoChain::default(),
            source: None,
            children: Vec::new(),
        }
    }

    /// Start a dynamic builder with its source: `Dynamic::new(label_signal)`.
    pub fn new(source: ReadSignal<String>) -> Self {
        Self::builder().value(source)
    }

    layout_sugar!();

    /// The dynamic source (required; snapshots at mount, subscribes after).
    pub fn value(mut self, source: ReadSignal<String>) -> Self {
        self.source = Some(source);
        self
    }

    /// Font size in logical px. Explicit: wins over inherited styles,
    /// and is itself inherited by descendant text.
    pub fn font_size(mut self, size: impl NumberArg) -> Self {
        self.typo.font_size = Some(size.into_f32());
        self
    }

    /// Text color. Explicit: wins over inherited styles, and is itself
    /// inherited by descendant text.
    pub fn text_color(mut self, color: Oklch) -> Self {
        self.typo.color = Some(color);
        self
    }

    /// Static-label wrapping. Explicit: wins over inherited styles.
    pub fn text_wrap(mut self, wrap: bool) -> Self {
        self.typo.wrap = Some(wrap);
        self
    }

    /// Append a child element.
    pub fn child(mut self, child: impl Into<Element>) -> Self {
        self.children.push(child.into());
        self
    }

    /// Append several children in call order (arrays, `Vec`s, and lazy
    /// iterators all work). Repeated calls append; existing single-child
    /// calls remain valid.
    pub fn children<I, E>(mut self, children: I) -> Self
    where
        I: IntoIterator<Item = E>,
        E: Into<Element>,
    {
        for c in children {
            self.children.push(c.into());
        }
        self
    }

    /// Finish as an [`Element`]. The source subscribes `SetText` updates at
    /// mount; lowering without `.value(source)` fails loudly instead.
    pub fn build(mut self) -> Element {
        let children = std::mem::take(&mut self.children);
        let mut bindings = Vec::new();
        if let Some(source) = &self.source {
            bindings.push(ElementBinding::SubscribeText(source.clone()));
        }
        Element {
            kind: ElementKind::Dynamic(self),
            key: None,
            role: None,
            collection_semantics: None,
            children,
            synth: Vec::new(),
            bindings,
        }
    }
}

/// Fragment builder: structural grouping with NO visual node. Fragments
/// flatten into the parent's child list at conversion — never an artificial
/// layout box. `ui!` roots and `for` bodies build fragments.
pub struct Fragment {
    children: Vec<Element>,
}

impl Fragment {
    /// Start a fragment builder.
    pub fn builder() -> Self {
        Self {
            children: Vec::new(),
        }
    }

    /// Alias for [`Fragment::builder`].
    pub fn new() -> Self {
        Self::builder()
    }

    /// Append a child element.
    pub fn child(mut self, child: impl Into<Element>) -> Self {
        self.children.push(child.into());
        self
    }

    /// Append several children in call order (arrays, `Vec`s, and lazy
    /// iterators all work). Repeated calls append; existing single-child
    /// calls remain valid.
    pub fn children<I, E>(mut self, children: I) -> Self
    where
        I: IntoIterator<Item = E>,
        E: Into<Element>,
    {
        for c in children {
            self.children.push(c.into());
        }
        self
    }

    /// Collect an iterator of elements (macro `for` loops target this).
    pub fn from_nodes<I, E>(iter: I) -> Fragment
    where
        I: IntoIterator<Item = E>,
        E: Into<Element>,
    {
        Fragment {
            children: iter.into_iter().map(Into::into).collect(),
        }
    }

    /// Finish as an [`Element`].
    pub fn build(self) -> Element {
        Element {
            kind: ElementKind::Fragment,
            key: None,
            role: None,
            collection_semantics: None,
            children: self.children,
            synth: Vec::new(),
            bindings: Vec::new(),
        }
    }
}

impl From<Fragment> for Element {
    fn from(f: Fragment) -> Self {
        f.build()
    }
}

/// `Stylable` plumbing: every builder resolves to its own stored parts, so
/// `.style(...)`/`.styles([...])` mutate in call order with zero allocation
/// for scalar styles.
macro_rules! stylable {
    ($t:ty) => {
        impl Stylable for $t {
            fn style_parts(&mut self) -> (&mut LayoutStyle, &mut Paint, &mut TypoChain) {
                (&mut self.style, &mut self.paint, &mut self.typo)
            }
        }
    };
}

stylable!(Box);
stylable!(Media);
stylable!(Scroll);
stylable!(Canvas);
stylable!(Viewport3D);
stylable!(Button);
stylable!(Checkbox);
stylable!(Slider);
stylable!(Dropdown);
stylable!(Tabs);
stylable!(Dynamic);

impl Stylable for Text {
    fn style_parts(&mut self) -> (&mut LayoutStyle, &mut Paint, &mut TypoChain) {
        (&mut self.style, &mut self.paint, &mut self.typo)
    }

    fn post_style(&mut self, style: &Style) {
        // The builder's own text fields mirror its explicit overrides, so
        // direct setters and styles compose in call order.
        if let Some(v) = style.font_size_opt() {
            self.font_size = v;
        }
        if let Some(v) = style.text_color_opt() {
            self.color = v;
        }
        if let Some(v) = style.text_wrap_opt() {
            self.wrap = v;
        }
    }
}

/// `Default` for no-arg builders delegates to `builder()` (clippy
/// `new_without_default`): `Box::default()` == `Box::builder()`.
macro_rules! default_via_builder {
    ($t:ty) => {
        impl Default for $t {
            fn default() -> Self {
                Self::builder()
            }
        }
    };
}

default_via_builder!(Box);
default_via_builder!(Scroll);
default_via_builder!(Canvas);
default_via_builder!(Viewport3D);
default_via_builder!(Button);
default_via_builder!(Tabs);
default_via_builder!(Fragment);

/// The authoring element: `ui!` output, converted to [`NodeSpec`] plus
/// mount-time bindings by [`crate::binding::View`].
pub struct Element {
    kind: ElementKind,
    key: Option<Key>,
    /// Explicit semantics role (applied to the mounted node; fragments
    /// propagate to children that lack one). `None` = tree default.
    role: Option<Role>,
    /// Collection coordinates override for this node (lists/tables report
    /// totals and logical indices through core metadata; `None` = unset).
    collection_semantics: Option<CollectionSemantics>,
    /// Explicit children (paint order).
    children: Vec<Element>,
    /// Synthetic children generated by composite builders (checkbox
    /// indicator/label, tab buttons, slider fill). Treated exactly like
    /// explicit children by lowering and binding walks.
    synth: Vec<Element>,
    /// Mount-time bindings (subscriptions, actions, painters, focus).
    bindings: Vec<ElementBinding>,
}

impl Element {
    /// Attach a stable reconciliation key (macro `key(expr)` generates this).
    pub fn with_key(mut self, key: impl Into<Key>) -> Self {
        self.key = Some(key.into());
        self
    }

    /// Convenience: a fragment from parts (hosts driving `sync_children`).
    pub fn fragment(children: Vec<Element>) -> Self {
        Fragment { children }.build()
    }

    /// Wrap a collection descriptor as an element. Lowers into ordinary
    /// retained nodes (a `Scroll` viewport when virtualized, a `Box` column
    /// otherwise); the [`View`](crate::binding::View) keeps the mounted
    /// window in sync on every `compute_layout`.
    pub fn collection(desc: CollectionDescriptor) -> Self {
        Element {
            kind: ElementKind::Collection(desc),
            key: None,
            role: None,
            collection_semantics: None,
            children: Vec::new(),
            synth: Vec::new(),
            bindings: Vec::new(),
        }
    }

    /// Set explicit accessibility semantics for the mounted node (applied at
    /// bind time; fragments propagate to children that lack one). Setting a
    /// role alone never implements behavior — but a wrong role misinforms
    /// assistive tech, so interactive nodes MUST carry their real role.
    pub fn role(mut self, role: Role) -> Self {
        if self.is_fragment() {
            for c in self.synth.iter_mut().chain(self.children.iter_mut()) {
                if c.role.is_none() {
                    c.role = Some(role);
                }
            }
        } else {
            self.role = Some(role);
        }
        self
    }

    /// Override the collection coordinates reported for the mounted node
    /// (totals and logical indices; usually set by the collection runtime —
    /// sources that build custom cells may set positions explicitly).
    /// Fragments propagate to children that lack one, like [`Element::role`].
    pub fn collection_semantics(mut self, cs: CollectionSemantics) -> Self {
        if self.is_fragment() {
            for c in self.synth.iter_mut().chain(self.children.iter_mut()) {
                if c.collection_semantics.is_none() {
                    c.collection_semantics = Some(cs);
                }
            }
        } else {
            self.collection_semantics = Some(cs);
        }
        self
    }

    /// This element's explicit role, if any.
    pub(crate) fn role_opt(&self) -> Option<Role> {
        self.role
    }

    /// This element's collection coordinates override, if any.
    pub(crate) fn collection_semantics_opt(&self) -> Option<CollectionSemantics> {
        self.collection_semantics
    }

    /// Whether this element is a collection container (window-managed by the
    /// runtime; its children are realized, not lowered from stored parts).
    pub(crate) fn is_collection(&self) -> bool {
        matches!(self.kind, ElementKind::Collection(_))
    }

    /// Borrow the collection descriptor, if this is a collection element.
    pub(crate) fn collection_descriptor(&self) -> Option<&CollectionDescriptor> {
        match &self.kind {
            ElementKind::Collection(d) => Some(d),
            _ => None,
        }
    }

    /// Append a child to a built element. Collection containers are
    /// window-managed: attaching children to one is a lowering error, never
    /// a silent drop (see [`LowerError::CollectionChildren`]).
    pub fn child(mut self, child: impl Into<Element>) -> Self {
        self.children.push(child.into());
        self
    }

    /// Append several children to a built element, in call order (arrays,
    /// `Vec`s, and lazy iterators all work). Repeated calls append.
    pub fn children<I, E>(mut self, children: I) -> Self
    where
        I: IntoIterator<Item = E>,
        E: Into<Element>,
    {
        for c in children {
            self.children.push(c.into());
        }
        self
    }

    /// Apply a single style to a built element: same ordered-merge semantics
    /// as the builders. Fragments own no node, so the style applies to every
    /// descendant instead (never silently stored on the fragment itself).
    pub fn style(mut self, style: impl StyleArg) -> Self {
        self.apply_arg(&style);
        self
    }

    /// Apply several styles in order to a built element (see [`Element::style`]).
    pub fn styles(mut self, styles: impl AsRef<[Style]>) -> Self {
        for s in styles.as_ref() {
            self.apply_arg(s);
        }
        self
    }

    fn apply_arg(&mut self, style: &dyn StyleArg) {
        match &mut self.kind {
            ElementKind::Box(b) => style.apply(b),
            ElementKind::Text(t) => style.apply(t),
            ElementKind::Media(m) => style.apply(m),
            ElementKind::Scroll(s) => style.apply(s),
            ElementKind::Canvas(c) => style.apply(c),
            ElementKind::Viewport3D(v) => style.apply(v),
            ElementKind::Button(b) => style.apply(b),
            ElementKind::Checkbox(c) => style.apply(c),
            ElementKind::Slider(s) => style.apply(s),
            ElementKind::Dropdown(d) => style.apply(d),
            ElementKind::Tabs(t) => style.apply(t),
            ElementKind::Dynamic(d) => style.apply(d),
            ElementKind::Collection(d) => {
                // Descriptor styles are normal Style merges in call order:
                // record the new style without disturbing earlier fields.
                style.push_onto(&mut d.styles);
            }
            ElementKind::Fragment => {
                for c in self.synth.iter_mut().chain(self.children.iter_mut()) {
                    c.apply_arg(style);
                }
            }
        }
    }

    /// This element's own explicit typography overrides (`None`-equivalent
    /// empty for fragments, which carry no props of their own).
    pub(crate) fn typo_chain(&self) -> TypoChain {
        match &self.kind {
            ElementKind::Box(b) => b.typo,
            ElementKind::Text(t) => t.typo,
            ElementKind::Media(m) => m.typo,
            ElementKind::Scroll(s) => s.typo,
            ElementKind::Canvas(c) => c.typo,
            ElementKind::Viewport3D(v) => v.typo,
            ElementKind::Button(b) => b.typo,
            ElementKind::Checkbox(c) => c.typo,
            ElementKind::Slider(s) => s.typo,
            ElementKind::Dropdown(d) => d.typo,
            ElementKind::Tabs(t) => t.typo,
            ElementKind::Dynamic(d) => d.typo,
            ElementKind::Collection(d) => {
                let mut typo = TypoChain::default();
                for style in &d.styles {
                    if let Some(v) = style.font_size_opt() {
                        typo.font_size = Some(v);
                    }
                    if let Some(v) = style.text_color_opt() {
                        typo.color = Some(v);
                    }
                    if let Some(v) = style.text_wrap_opt() {
                        typo.wrap = Some(v);
                    }
                }
                typo
            }
            ElementKind::Fragment => TypoChain::default(),
        }
    }

    /// This element's resolved paint (every builder carries a real one).
    pub(crate) fn resolved_paint(&self) -> Paint {
        match &self.kind {
            ElementKind::Box(b) => b.paint,
            ElementKind::Text(t) => t.paint,
            ElementKind::Media(m) => m.paint,
            ElementKind::Scroll(s) => s.paint,
            ElementKind::Canvas(c) => c.paint,
            ElementKind::Viewport3D(v) => v.paint,
            ElementKind::Button(b) => b.paint,
            ElementKind::Checkbox(c) => c.paint,
            ElementKind::Slider(s) => s.paint,
            ElementKind::Dropdown(d) => d.paint,
            ElementKind::Tabs(t) => t.paint,
            ElementKind::Dynamic(d) => d.paint,
            // Fragments vanish at lowering and carry no paint; collections
            // resolve paint from descriptor styles in `lower_one`.
            ElementKind::Collection(_) | ElementKind::Fragment => Paint::default(),
        }
    }

    /// Whether this element is a structural fragment (no node of its own).
    pub(crate) fn is_fragment(&self) -> bool {
        matches!(self.kind, ElementKind::Fragment)
    }

    /// Splice this element into a flat list, expanding fragments
    /// structurally (synthetic children first, matching [`Element`] order).
    /// A keyed fragment collapses onto its single child; any other keyed
    /// fragment is an error (the key would silently vanish). Bindings on a
    /// bare fragment have no node to attach to and are a bug — debug-asserted.
    pub(crate) fn splice_into(self, out: &mut Vec<Element>) -> Result<(), LowerError> {
        if !self.is_fragment() {
            out.push(self);
            return Ok(());
        }
        let Element {
            key,
            role,
            collection_semantics,
            mut synth,
            mut children,
            bindings,
            ..
        } = self;
        debug_assert!(
            bindings.is_empty(),
            "fragment carrying bindings has no node to attach them to"
        );
        let mut flat = Vec::new();
        flat.append(&mut synth);
        flat.append(&mut children);
        // Roles/coordinates set on a fragment propagate to children that
        // lack one (covers children appended after the `.role()` call).
        // Eager propagation in `Element::role` already handled earlier ones.
        for c in flat.iter_mut() {
            if c.role.is_none() {
                c.role = role;
            }
            if c.collection_semantics.is_none() {
                c.collection_semantics = collection_semantics;
            }
        }
        if key.is_some() {
            if flat.len() != 1 {
                return Err(LowerError::KeyedFragment {
                    children: flat.len(),
                });
            }
            let mut only = flat.pop().unwrap();
            if only.key.is_none() {
                only.key = key;
            }
            return only.splice_into(out);
        }
        for c in flat {
            c.splice_into(out)?;
        }
        Ok(())
    }

    /// Lowered-kind discriminant: composite builders map to their retained
    /// kind (buttons/checkbox/slider/dropdown/tabs → `Box`, dynamic → `Text`).
    /// A mounted node whose discriminant differs is replaced, never morphed.
    pub(crate) fn lowered_discriminant(&self) -> u8 {
        match self.kind {
            ElementKind::Box(_)
            | ElementKind::Button(_)
            | ElementKind::Checkbox(_)
            | ElementKind::Slider(_)
            | ElementKind::Dropdown(_)
            | ElementKind::Tabs(_) => 0,
            ElementKind::Text(_) | ElementKind::Dynamic(_) => 1,
            ElementKind::Media(_) => 2,
            ElementKind::Scroll(_) => 3,
            ElementKind::Canvas(_) => 4,
            ElementKind::Viewport3D(_) => 5,
            // Collections lower to a Scroll viewport for virtualized lists,
            // a Box column otherwise (table roots are Box in both modes —
            // header clips wrong under Scroll). Flipping list virtualization
            // remounts, never morphs.
            ElementKind::Collection(ref d) => {
                if d.virtualized && d.source.role() == CollectionRole::List {
                    3
                } else {
                    0
                }
            }
            ElementKind::Fragment => 12,
        }
    }

    /// This element's layout style, if it has retained props (`None` for
    /// fragments, which vanish at lowering).
    pub(crate) fn style_opt(&self) -> Option<&LayoutStyle> {
        match &self.kind {
            ElementKind::Box(b) => Some(&b.style),
            ElementKind::Text(t) => Some(&t.style),
            ElementKind::Media(m) => Some(&m.style),
            ElementKind::Scroll(s) => Some(&s.style),
            ElementKind::Canvas(c) => Some(&c.style),
            ElementKind::Viewport3D(v) => Some(&v.style),
            ElementKind::Button(b) => Some(&b.style),
            ElementKind::Checkbox(c) => Some(&c.style),
            ElementKind::Slider(s) => Some(&s.style),
            ElementKind::Dropdown(d) => Some(&d.style),
            ElementKind::Tabs(t) => Some(&t.style),
            ElementKind::Dynamic(d) => Some(&d.style),
            // Fragments vanish; collections resolve styles from the
            // descriptor in `lower_one`/`refresh_mounted`.
            ElementKind::Collection(_) | ElementKind::Fragment => None,
        }
    }

    /// Accessor for the binding walk (bindings are taken exactly once).
    pub(crate) fn bindings_mut(&mut self) -> &mut Vec<ElementBinding> {
        &mut self.bindings
    }

    /// Mutable flattened children (synthetic first, then explicit, splicing
    /// nested fragments). Mirrors [`Element::flat_children`] exactly —
    /// lowering and binding walks must agree.
    pub(crate) fn flat_children_mut(&mut self) -> Vec<&mut Element> {
        let mut out = Vec::new();
        Self::push_flat_mut(&mut self.synth, &mut out);
        Self::push_flat_mut(&mut self.children, &mut out);
        out
    }

    fn push_flat_mut<'a>(list: &'a mut [Element], out: &mut Vec<&'a mut Element>) {
        for el in list.iter_mut() {
            if el.is_fragment() {
                Self::push_flat_mut(&mut el.synth, out);
                Self::push_flat_mut(&mut el.children, out);
            } else {
                out.push(el);
            }
        }
    }

    /// Refresh a live mounted node in place from this element's props and
    /// re-sync its children. `inherited` is the typography chain from the
    /// element's ancestors. Returns `Ok(false)` when the retained kind
    /// differs (caller remounts) or the node is gone; `Err` on lowering
    /// failures (keyed fragment, sourceless dynamic, non-uniform corners).
    pub(crate) fn refresh_mounted(
        &self,
        tree: &mut UiTree,
        id: NodeId,
        inherited: &TypoChain,
    ) -> Result<bool, LowerError> {
        let Some(rec) = tree.get(id) else {
            return Ok(false);
        };
        if self.lowered_discriminant() != Self::tree_discriminant(&rec.kind) {
            return Ok(false);
        }
        // Clone the committed style up front: later arms refresh through
        // `&mut tree`, so the shared `rec` borrow must end here (NLL).
        let rec_style = rec.style.clone();
        if let Some(style) = self.style_opt() {
            if rec.style != *style {
                tree.set_style(id, style.clone());
            }
        }
        // Paint refreshes through the tree seam for every kind (paint-only,
        // no layout invalidation, no identity change). `set_paint` no-ops on
        // equality, so unchanged styles cost nothing here. Non-uniform
        // corners are rejected here too, so refresh fails as loudly as mount.
        let paint = self.resolved_paint();
        check_paint(&paint)?;
        tree.set_paint(id, paint);
        match &self.kind {
            ElementKind::Box(_)
            | ElementKind::Button(_)
            | ElementKind::Checkbox(_)
            | ElementKind::Slider(_)
            | ElementKind::Dropdown(_)
            | ElementKind::Tabs(_) => {
                // Box-lowered kinds carry paint only (structure lives in
                // synthetic children, synced below).
            }
            ElementKind::Text(t) => {
                let (live_editable, live_multiline) = match tree.get(id) {
                    Some(rec) => match &rec.kind {
                        NodeKind::Text(cur) => (cur.is_editable(), cur.multiline()),
                        _ => return Ok(false),
                    },
                    None => return Ok(false),
                };
                if live_editable != t.editable {
                    // Genuine mode change: rebuild the backing store in
                    // place (node id preserved, text + caret carried over,
                    // undo history resets with the new backing).
                    if !Self::switch_text_backing(tree, id, t) {
                        return Ok(false);
                    }
                } else if live_editable && live_multiline != t.multiline {
                    // In-place line-mode switch (the editor preserves text
                    // and caret; folding newlines going single is editor
                    // policy, not UI policy).
                    if let Some(rec) = tree.get_mut(id) {
                        if let NodeKind::Text(ref mut tc) = rec.kind {
                            if let TextBacking::Editable(ref mut ed) = tc.backing {
                                ed.set_line_mode(if t.multiline {
                                    LineMode::Multi
                                } else {
                                    LineMode::Single
                                });
                            }
                        }
                    }
                }
                // Uncontrolled editable: the live editor owns its content;
                // the descriptor's `initial` is the mount-time default, not a
                // controlled value. Preserve backing/caret/selection and
                // apply cosmetic props only. Controlled text (effect
                // bindings present) and static labels go through set_text.
                let uncontrolled = t.editable
                    && !self.bindings.iter().any(|b| {
                        matches!(
                            b,
                            ElementBinding::SubscribeText(_) | ElementBinding::SubscribeEdit { .. }
                        )
                    });
                let eff = inherited.child(&t.typo).resolve();
                Self::refresh_text(tree, id, &t.initial, t, uncontrolled, &eff);
            }
            ElementKind::Dynamic(d) => {
                let eff = inherited.child(&d.typo).resolve();
                if let Some(source) = &d.source {
                    let text = source.try_get().unwrap_or_default();
                    tree.set_text(id, text);
                }
                if let Some(rec) = tree.get_mut(id) {
                    if let NodeKind::Text(ref mut tc) = rec.kind {
                        tc.font_size = eff.font_size;
                        tc.color = eff.color;
                        tc.wrap = eff.wrap;
                    }
                }
            }
            ElementKind::Media(m) => {
                if let Some(rec) = tree.get_mut(id) {
                    if let NodeKind::Media(ref mut cur) = rec.kind {
                        cur.image = m.image;
                        cur.opacity = m.opacity;
                    }
                }
            }
            ElementKind::Scroll(s) => {
                tree.scroll_to(id, s.offset);
            }
            ElementKind::Canvas(_) => {
                // Painters persist in the registry; nothing to refresh.
            }
            ElementKind::Viewport3D(v) => {
                if let Some(rec) = tree.get_mut(id) {
                    if let NodeKind::Viewport3D(ref mut cur) = rec.kind {
                        cur.viewport = v.viewport;
                        cur.layer = v.layer;
                    }
                }
            }
            ElementKind::Collection(d) => {
                // Container style/paint refresh only, in BOTH modes. The
                // driver owns realization end to end (rows/header/spacers,
                // window, scroll, bindings) and reconciles the snapshot, so
                // refresh never touches children: shells are empty and
                // managed, and re-lowering here would double-run factories.
                if !self.children.is_empty() || !self.synth.is_empty() {
                    return Err(LowerError::CollectionChildren);
                }
                let (layout, paint) = crate::collection_runtime::container_style(d);
                check_paint(&paint)?;
                if rec_style != layout {
                    tree.set_style(id, layout);
                }
                tree.set_paint(id, paint);
                return Ok(true);
            }
            ElementKind::Fragment => {
                // Caller routes fragments to children sync, never here.
            }
        }
        let own = inherited.child(&self.typo_chain());
        let mut child_specs = Vec::new();
        for c in self.flat_children() {
            child_specs.extend(lower_specs_in(c, &own)?);
        }
        tree.sync_children(id, child_specs);
        Ok(true)
    }

    /// Rebuild a text node's backing store in place for a static<->editable
    /// mode change. The node id survives; committed text and caret carry over
    /// (collapsed to the caret for labels); undo history resets with the new
    /// backing — a genuine mode change, never a silent morph. Cosmetic props
    /// are applied by the caller afterwards; the measure leaf re-syncs here
    /// because display metrics may change with the backing.
    fn switch_text_backing(tree: &mut UiTree, id: NodeId, t: &Text) -> bool {
        let (text, caret) = match tree.get(id) {
            Some(rec) => match &rec.kind {
                NodeKind::Text(tc) => (tc.text().to_string(), tc.caret()),
                _ => return false,
            },
            None => return false,
        };
        let mut fresh = if t.editable {
            TextContent::editable(text, t.multiline)
        } else {
            TextContent::label(text)
        };
        // Carry the caret across the backing switch (clamped to grapheme
        // boundaries by the editor/selection constructors).
        let end = fresh.text().len();
        match &mut fresh.backing {
            TextBacking::Editable(ed) => {
                ed.set_caret(caret.unwrap_or(end));
            }
            TextBacking::Static(_) => {
                fresh.selection = Some(Selection::caret_at(caret.unwrap_or(0)));
            }
        }
        match tree.get_mut(id) {
            Some(rec) => match &mut rec.kind {
                NodeKind::Text(cur) => *cur = fresh,
                _ => return false,
            },
            None => return false,
        }
        tree.sync_text_leaf(id);
        true
    }

    fn refresh_text(
        tree: &mut UiTree,
        id: NodeId,
        initial: &str,
        t: &Text,
        preserve_editing: bool,
        eff: &Typography,
    ) {
        if let Some(rec) = tree.get_mut(id) {
            if let NodeKind::Text(ref mut tc) = rec.kind {
                tc.font_size = eff.font_size;
                tc.color = eff.color;
                tc.selectable = t.selectable;
                tc.wrap = eff.wrap;
                tc.password = t.password;
                tc.placeholder = t.placeholder.clone();
            }
        }
        // `set_text` no-ops on equality and preserves editor state; in-flight
        // edits already committed through `TextChanged` before any re-sync.
        // Skipped for uncontrolled editors: their content is local state.
        if !preserve_editing {
            tree.set_text(id, initial);
        }
    }

    fn tree_discriminant(kind: &NodeKind) -> u8 {
        match kind {
            NodeKind::Box(_) => 0,
            NodeKind::Text(_) => 1,
            NodeKind::Media(_) => 2,
            NodeKind::Scroll(_) => 3,
            NodeKind::Canvas(_) => 4,
            NodeKind::Viewport3D(_) => 5,
        }
    }

    /// Flattened children in paint order: synthetic children first, then
    /// explicit ones, splicing nested fragments. Lowering and the binding
    /// walk share this so node/element correspondence always agrees.
    fn flat_children(&self) -> Vec<&Element> {
        let mut out = Vec::new();
        Self::push_flat(&self.synth, &mut out);
        Self::push_flat(&self.children, &mut out);
        out
    }

    fn push_flat<'a>(list: &'a [Element], out: &mut Vec<&'a Element>) {
        for el in list {
            if matches!(el.kind, ElementKind::Fragment) {
                Self::push_flat(&el.synth, out);
                Self::push_flat(&el.children, out);
            } else {
                out.push(el);
            }
        }
    }
}

impl std::fmt::Debug for Element {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Element")
            .field("kind", &self.kind.discriminant_name())
            .field("key", &self.key)
            .field("children", &self.children.len())
            .field("synth", &self.synth.len())
            .field("bindings", &self.bindings.len())
            .finish()
    }
}

/// Retained content of an [`Element`] (the builder structs double as props).
enum ElementKind {
    Box(Box),
    Text(Text),
    Media(Media),
    Scroll(Scroll),
    Canvas(Canvas),
    Viewport3D(Viewport3D),
    Button(Button),
    Checkbox(Checkbox),
    Slider(Slider),
    Dropdown(Dropdown),
    Tabs(Tabs),
    Dynamic(Dynamic),
    /// Collection container: lowers to ordinary nodes (Scroll/Box); the
    /// runtime driver owns the children window. Carries no stored Element
    /// children — rows are generated from the source snapshot.
    Collection(CollectionDescriptor),
    Fragment,
}

impl ElementKind {
    fn discriminant_name(&self) -> &'static str {
        match self {
            ElementKind::Box(_) => "Box",
            ElementKind::Text(_) => "Text",
            ElementKind::Media(_) => "Media",
            ElementKind::Scroll(_) => "Scroll",
            ElementKind::Canvas(_) => "Canvas",
            ElementKind::Viewport3D(_) => "Viewport3D",
            ElementKind::Button(_) => "Button",
            ElementKind::Checkbox(_) => "Checkbox",
            ElementKind::Slider(_) => "Slider",
            ElementKind::Dropdown(_) => "Dropdown",
            ElementKind::Tabs(_) => "Tabs",
            ElementKind::Dynamic(_) => "Dynamic",
            ElementKind::Collection(_) => "Collection",
            ElementKind::Fragment => "Fragment",
        }
    }
}

/// Splice top-level fragments into a flat element list (structural
/// flattening — never an artificial node). A keyed fragment collapses onto
/// its single child; any other keyed fragment is an error (the key would
/// silently vanish). After this, node↔element pairing is always positional.
pub(crate) fn splice_top_level(elements: Vec<Element>) -> Result<Vec<Element>, LowerError> {
    let mut out = Vec::new();
    for el in elements {
        el.splice_into(&mut out)?;
    }
    Ok(out)
}
/// missing dynamic source cannot be represented — all fail here, loudly.
/// Per-corner radii have a scalar-only scene seam (the emitter renders
/// `corners.top_left`): uniform values round-trip exactly, but truly
/// non-uniform values cannot be represented — so a `corners(...)` style with
/// unequal radii reaching lowering is an explicit error, never a silent
/// fallback to the uniform radius.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LowerError {
    /// A keyed fragment flattened to other than exactly one child.
    KeyedFragment {
        /// Flattened child count.
        children: usize,
    },
    /// `Dynamic` built without `.value(source)`.
    DynamicWithoutSource,
    /// Per-corner radii with unequal values cannot be rendered; use uniform
    /// `radius` (or an all-equal `corners` value).
    NonUniformCorners,
    /// A collection element carrying explicit Element children. Collection
    /// containers are window-managed (rows come from the source snapshot);
    /// attaching children would silently drop them, so it fails loudly.
    CollectionChildren,
    /// Collection descriptor or snapshot validation failed.
    Collection(CollectionError),
}

impl std::fmt::Display for LowerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LowerError::KeyedFragment { children } => write!(
                f,
                "keyed fragment flattened to {children} children (expected exactly 1); \
                 wrap the content in a Box or move key() around a single node"
            ),
            LowerError::DynamicWithoutSource => {
                write!(f, "Dynamic built without .value(source)")
            }
            LowerError::NonUniformCorners => {
                write!(
                    f,
                    "per-corner radii (corners) with unequal values have no scene seam; \
                     use uniform radius instead"
                )
            }
            LowerError::CollectionChildren => {
                write!(
                    f,
                    "collection containers are window-managed: rows come from \
                     the source snapshot, explicit children would be dropped"
                )
            }
            LowerError::Collection(e) => write!(f, "collection invalid: {e}"),
        }
    }
}

impl std::error::Error for LowerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LowerError::Collection(e) => Some(e),
            _ => None,
        }
    }
}

/// Reject truly non-uniform per-corner radii: the scene seam renders only
/// `corners.top_left`, so an all-equal value round-trips exactly while
/// unequal values would silently misrender. Uniform values pass through for
/// the emitter; direct [`UiTree::set_paint`] applies without validation
/// (authored values are validated here, at lowering/refresh).
fn check_paint(paint: &Paint) -> Result<(), LowerError> {
    if let Some(c) = paint.corners {
        if !(c.top_left == c.top_right
            && c.top_left == c.bottom_right
            && c.top_left == c.bottom_left)
        {
            return Err(LowerError::NonUniformCorners);
        }
    }
    Ok(())
}

/// Lower a collection element to its container SHELL (agreed shell contract
/// with the View driver owner, via parent):
///
/// - Virtual list root: `Scroll` viewport. Normal list root: `Box` column.
/// - Table root: `Box` column in BOTH modes (discriminant 0, styled column,
///   `Role::Table` applied by the driver) — never `Scroll`, otherwise the
///   header would clip wrong. The driver creates the header clip `Box` and
///   the body (`Scroll` virtual / `Box` normal) as children: the header
///   sits outside the vertical body viewport (clipping guaranteed) and
///   mirrors the body's horizontal scroll offset.
/// - Shell-only: NO `row()`/`header()`/`layout()` factory calls and NO
///   children. Factories run exactly once per mounted row inside the driver
///   (which also binds); lowering must not spend any (consumer test: 32
///   factories for 32 rows, one header call).
/// - Virtualized containers whose authored height is unbounded (`Auto`)
///   default to viewport fill (`Pct(100)`): an Auto virtual root would
///   otherwise expand to the total spacer height and defeat windowing. When
///   the parent is itself unbounded, the driver clamps the windowing
///   viewport to the UiApp viewport (root virtual list, no explicit height,
///   at 600x400 mounts ~30 rows, never 100k).
/// - Every shell node is `collection_managed` (both modes): refresh keeps
///   container style/paint and never touches children or scroll offsets —
///   the driver reconciles the snapshot and owns realization end to end.
fn lower_collection(
    el: &Element,
    desc: &CollectionDescriptor,
    _inherited: &TypoChain,
) -> Result<Vec<NodeSpec>, LowerError> {
    use crate::collection_runtime::container_style;
    if !el.children.is_empty() || !el.synth.is_empty() {
        return Err(LowerError::CollectionChildren);
    }
    validate_descriptor(desc).map_err(LowerError::Collection)?;
    validate_snapshot(desc.source.as_ref()).map_err(LowerError::Collection)?;
    let (layout, paint) = container_style(desc);
    check_paint(&paint)?;
    let key = el.key.clone().map(hash_key);
    let is_table = desc.source.role() == CollectionRole::Table;
    let mut root = if is_table || !desc.virtualized {
        // Table root is Box in both modes (header clips wrong under Scroll);
        // a normal list is a plain column. Empty until the driver realizes.
        NodeSpec::new(NodeKind::Box(BoxContent { paint }), layout)
    } else {
        NodeSpec::new(
            NodeKind::Scroll(ScrollContent {
                offset: Point::ZERO,
            }),
            layout,
        )
    };
    // Same paint convention as `lower_one`: boxes through the legacy slot,
    // everything else through the generic seam.
    root.paint = paint;
    root.key = key;
    root.collection_managed = true;
    Ok(vec![root])
}

/// Convert one non-fragment element to its [`NodeSpec`]. Composite builders
/// lower to honest retained kinds (checkbox/tab/slider structure as real
/// boxes + texts; interaction that the tree cannot represent is a binding
/// or a documented gap, never a fake field).
///
/// `inherited` is the typography chain from the element's ancestors: text
/// nodes resolve their own explicit overrides over it. Box-lowered kinds
/// carry their builder paint in the legacy [`BoxContent`] slot; every other
/// kind carries it in [`NodeSpec::paint`] — no wrapper nodes either way.
fn lower_one(el: &Element, inherited: &TypoChain) -> Result<NodeSpec, LowerError> {
    let paint = el.resolved_paint();
    check_paint(&paint)?;
    let mut spec = match &el.kind {
        ElementKind::Box(b) => NodeSpec::new(
            NodeKind::Box(BoxContent { paint: b.paint }),
            b.style.clone(),
        ),
        ElementKind::Text(t) => {
            let eff = inherited.child(&t.typo).resolve();
            NodeSpec::new(NodeKind::Text(text_content(t, &eff)), t.style.clone())
        }
        ElementKind::Media(m) => NodeSpec::new(
            NodeKind::Media(MediaContent {
                image: m.image,
                opacity: m.opacity,
            }),
            m.style.clone(),
        ),
        ElementKind::Scroll(s) => NodeSpec::new(
            NodeKind::Scroll(ScrollContent { offset: s.offset }),
            s.style.clone(),
        ),
        ElementKind::Canvas(c) => {
            let style = c.style.clone();
            NodeSpec::new(NodeKind::Canvas(CanvasContent { painter: None }), style)
        }
        ElementKind::Viewport3D(v) => NodeSpec::new(
            NodeKind::Viewport3D(ViewportContent {
                viewport: v.viewport,
                layer: v.layer,
            }),
            v.style.clone(),
        ),
        ElementKind::Button(b) => NodeSpec::new(
            NodeKind::Box(BoxContent { paint: b.paint }),
            b.style.clone(),
        ),
        ElementKind::Checkbox(c) => NodeSpec::new(
            NodeKind::Box(BoxContent { paint: c.paint }),
            c.style.clone(),
        ),
        ElementKind::Slider(s) => NodeSpec::new(
            NodeKind::Box(BoxContent { paint: s.paint }),
            s.style.clone(),
        ),
        ElementKind::Dropdown(d) => NodeSpec::new(
            NodeKind::Box(BoxContent { paint: d.paint }),
            d.style.clone(),
        ),
        ElementKind::Tabs(t) => NodeSpec::new(
            NodeKind::Box(BoxContent { paint: t.paint }),
            t.style.clone(),
        ),
        ElementKind::Dynamic(d) => {
            let Some(source) = &d.source else {
                return Err(LowerError::DynamicWithoutSource);
            };
            let eff = inherited.child(&d.typo).resolve();
            let mut tc = TextContent::label(source.try_get().unwrap_or_default());
            tc.font_size = eff.font_size;
            tc.color = eff.color;
            tc.wrap = eff.wrap;
            NodeSpec::new(NodeKind::Text(tc), d.style.clone())
        }
        ElementKind::Collection(_) => {
            unreachable!("collections route through lower_specs_in, never lower_one")
        }
        ElementKind::Fragment => {
            unreachable!("fragments are spliced by lower_specs, never lowered directly")
        }
    };
    // Every node carries its resolved paint: boxes through the legacy
    // `BoxContent` slot (kept identical here), every other kind through
    // `NodeSpec::paint` for the tree's generic emission.
    spec.paint = paint;
    spec.key = el.key.clone().map(hash_key);
    Ok(spec)
}

/// Lower an element to specs, splicing fragments structurally (no artificial
/// nodes). A keyed fragment collapses onto its single child; any other keyed
/// fragment is an error (the key would silently vanish).
///
/// Typography inheritance threads through the element ancestry: each level
/// merges its own explicit overrides over its parent's chain. Layout and
/// paint never inherit — only explicitly set fields travel.
pub fn lower_specs(el: &Element) -> Result<Vec<NodeSpec>, LowerError> {
    lower_specs_in(el, &TypoChain::default())
}

pub(crate) fn lower_specs_in(
    el: &Element,
    inherited: &TypoChain,
) -> Result<Vec<NodeSpec>, LowerError> {
    if let ElementKind::Collection(desc) = &el.kind {
        // Window-managed containers lower through the collection path
        // (initial window + spacers + header); the driver owns updates.
        return lower_collection(el, desc, inherited);
    }
    if matches!(el.kind, ElementKind::Fragment) {
        let flat = el.flat_children();
        if el.key.is_some() {
            if flat.len() != 1 {
                return Err(LowerError::KeyedFragment {
                    children: flat.len(),
                });
            }
            let mut spec = lower_specs_in(flat[0], inherited)?;
            let key = el.key.clone().map(hash_key);
            for s in &mut spec {
                if s.key.is_none() {
                    s.key = key;
                }
            }
            return Ok(spec);
        }
        let mut out = Vec::new();
        for child in flat {
            out.extend(lower_specs_in(child, inherited)?);
        }
        return Ok(out);
    }
    let mut spec = lower_one(el, inherited)?;
    let own = inherited.child(&el.typo_chain());
    for child in el.flat_children() {
        spec.children.extend(lower_specs_in(child, &own)?);
    }
    Ok(vec![spec])
}

fn text_content(t: &Text, eff: &Typography) -> TextContent {
    let mut tc = if t.editable {
        TextContent::editable(t.initial.clone(), t.multiline)
    } else {
        TextContent::label(t.initial.clone())
    };
    tc.font_size = eff.font_size;
    tc.color = eff.color;
    tc.selectable = t.selectable;
    tc.wrap = eff.wrap;
    tc.password = t.password;
    tc.placeholder = t.placeholder.clone();
    tc
}

/// The macro-facing module: `ui!` generates `C::ui::N::builder()` paths.
/// The `aqua` facade aliases this module as `aqua::ui`
/// (`pub use aqua_ui::element::ui;`), so macro output and direct use agree.
pub mod ui {
    pub use super::Box;
    pub use super::Button;
    pub use super::Canvas;
    pub use super::Checkbox;
    pub use super::Dropdown;
    pub use super::Dynamic;
    pub use super::Element;
    pub use super::Fragment;
    pub use super::Key;
    pub use super::Media;
    pub use super::Scroll;
    pub use super::Slider;
    pub use super::Tabs;
    pub use super::Text;
    pub use super::Viewport3D;
    pub use crate::style::Axis;
    /// Canonical styling API, usable from `ui!` property expressions and
    /// direct builder code alike (`ui::Style`, `ui::px`, ...).
    pub use crate::style::{auto, pct, px, Length, Style, Typography};

    /// Canonical DSL axis: `Box(layout: column)`. Lowercase on purpose —
    /// this is authoring vocabulary, not a type or constant convention.
    #[allow(non_upper_case_globals)]
    pub const column: Axis = Axis::Column;

    /// Canonical DSL axis: `Box(layout: row)`.
    #[allow(non_upper_case_globals)]
    pub const row: Axis = Axis::Row;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::{auto, px, Style};

    fn label_spec(text: &str) -> NodeSpec {
        NodeSpec::new(
            NodeKind::Text(TextContent::label(text)),
            LayoutStyle::default(),
        )
    }

    fn text_color_of(spec: &NodeSpec) -> Oklch {
        match &spec.kind {
            NodeKind::Text(t) => t.color,
            other => panic!("expected text, got {other:?}"),
        }
    }

    fn box_paint_of(spec: &NodeSpec) -> Paint {
        match &spec.kind {
            NodeKind::Box(b) => b.paint,
            other => panic!("expected box, got {other:?}"),
        }
    }

    #[test]
    fn placeholder_carries_into_text_content() {
        let el = Text::builder()
            .arg("")
            .editable(true)
            .placeholder("Search...")
            .build();
        let specs = lower_specs(&el).unwrap();
        match &specs[0].kind {
            NodeKind::Text(t) => {
                // Committed text stays empty; the placeholder is display-only.
                assert_eq!(t.text(), "");
                assert_eq!(t.placeholder, "Search...");
            }
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn box_builder_maps_design_props() {
        let el = Box::builder()
            .layout("column")
            .gap(16)
            .width(200)
            .child(Text::builder().arg("hi").build())
            .build();
        let specs = lower_specs(&el).unwrap();
        assert_eq!(specs.len(), 1);
        match &specs[0].kind {
            NodeKind::Box(_) => {}
            other => panic!("expected Box, got {other:?}"),
        }
        assert_eq!(specs[0].style.gap, 16.0);
        assert_eq!(specs[0].style.width, Length::Px(200.0));
        assert!(matches!(specs[0].style.axis, Axis::Column));
        assert_eq!(specs[0].children.len(), 1);
    }

    #[test]
    fn fragment_flattens_without_layout_box() {
        let el = Fragment::builder()
            .child(Text::builder().arg("a").build())
            .child(
                Fragment::builder()
                    .child(Text::builder().arg("b").build())
                    .build(),
            )
            .build();
        let specs = lower_specs(&el).unwrap();
        // Two text specs, zero boxes: fragments vanish structurally.
        assert_eq!(specs.len(), 2);
        for s in &specs {
            assert!(matches!(s.kind, NodeKind::Text(_)), "{:?}", s.kind);
        }
    }

    #[test]
    fn keyed_single_child_collapses_key_onto_child() {
        let el = Fragment::builder()
            .child(Text::builder().arg("x").build())
            .build()
            .with_key("row-1");
        let specs = lower_specs(&el).unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].key, Some(hash_key(Key::Str("row-1".into()))));
    }

    #[test]
    fn keyed_multi_child_fragment_is_an_error() {
        let el = Fragment::builder()
            .child(Text::builder().arg("a").build())
            .child(Text::builder().arg("b").build())
            .build()
            .with_key("k");
        assert!(matches!(
            lower_specs(&el),
            Err(LowerError::KeyedFragment { children: 2 })
        ));
    }

    #[test]
    fn dynamic_without_source_is_an_error() {
        let el = Dynamic::builder().build();
        assert!(matches!(
            lower_specs(&el),
            Err(LowerError::DynamicWithoutSource)
        ));
    }

    #[test]
    fn button_lowers_to_box_and_binds_activate() {
        let el = Button::builder()
            .on_activate(|| {})
            .child(Text::builder().arg("Go").build())
            .build();
        assert!(matches!(el.kind, ElementKind::Button(_)));
        let specs = lower_specs(&el).unwrap();
        assert!(matches!(specs[0].kind, NodeKind::Box(_)));
        // Activate + Focusable bindings travel with the element.
        assert_eq!(el.bindings.len(), 2);
        // Children survive lowering (label text present).
        assert_eq!(specs[0].children.len(), 1);
    }

    #[test]
    fn checkbox_lowers_indicator_and_label() {
        let el = Checkbox::builder().label("done").checked(true).build();
        let specs = lower_specs(&el).unwrap();
        // Indicator + label synthetic children, no user children.
        assert_eq!(specs[0].children.len(), 2);
        assert!(matches!(specs[0].children[0].kind, NodeKind::Box(_)));
        assert!(matches!(specs[0].children[1].kind, NodeKind::Text(_)));
    }

    #[test]
    fn tabs_build_one_button_per_tab() {
        let el = Tabs::builder()
            .tabs(vec!["a".into(), "b".into(), "c".into()])
            .selected(1)
            .on_select(|_| {})
            .build();
        // Three synthetic tab buttons, each carrying Activate + Focusable.
        assert_eq!(el.synth.len(), 3);
        for b in &el.synth {
            assert!(matches!(b.kind, ElementKind::Button(_)));
            assert_eq!(b.bindings.len(), 2);
        }
        let specs = lower_specs(&el).unwrap();
        assert_eq!(specs[0].children.len(), 3);
    }

    #[test]
    fn key_conversions_cover_macro_with_key() {
        let _ = Element::fragment(vec![]).with_key("s");
        let _ = Element::fragment(vec![]).with_key(String::from("s"));
        let _ = Element::fragment(vec![]).with_key(1u64);
        let _ = Element::fragment(vec![]).with_key(2u32);
        let _ = Element::fragment(vec![]).with_key(3usize);
        let _ = Element::fragment(vec![]).with_key(-4i32);
        // Labels used through the builders in real code paths.
        let _ = label_spec("unused");
    }

    // -- Style integration: ordered merge, precedence, inheritance --

    #[test]
    fn styles_merge_ordered_without_defaults_clobbering() {
        let el = Box::builder()
            .styles([
                Style::new().width(px(100.0)).gap(4.0),
                Style::new().gap(16.0),
            ])
            .build();
        let specs = lower_specs(&el).unwrap();
        // Untouched by the second style: still the first style's value.
        assert_eq!(specs[0].style.width, Length::Px(100.0));
        // Set by the second style: wins.
        assert_eq!(specs[0].style.gap, 16.0);
    }

    #[test]
    fn direct_setters_interleave_with_styles_in_call_order() {
        // Direct setter first, style second: style wins per field.
        let el = Box::builder()
            .width(10)
            .fill(Oklch::BLACK)
            .styles([Style::new().width(px(20.0))])
            .build();
        let specs = lower_specs(&el).unwrap();
        assert_eq!(specs[0].style.width, Length::Px(20.0));
        // Paint untouched by the style: the direct setter survives.
        assert_eq!(box_paint_of(&specs[0]).fill, Some(Oklch::BLACK));

        // Style first, direct setter second: direct wins.
        let el = Box::builder()
            .styles([Style::new().width(px(20.0)).gap(4.0)])
            .width(30)
            .build();
        let specs = lower_specs(&el).unwrap();
        assert_eq!(specs[0].style.width, Length::Px(30.0));
        assert_eq!(specs[0].style.gap, 4.0);
    }

    #[test]
    fn legacy_layout_style_replaces() {
        // A total LayoutStyle replaces the resolved layout (historical
        // theme-adapter behavior); granular setters apply after it.
        let el = Box::builder()
            .width(10)
            .style(LayoutStyle {
                gap: 9.0,
                ..LayoutStyle::default()
            })
            .build();
        let specs = lower_specs(&el).unwrap();
        assert_eq!(specs[0].style.width, Length::Auto);
        assert_eq!(specs[0].style.gap, 9.0);
    }

    #[test]
    fn shared_paint_applies_to_button_no_wrapper() {
        // Shared theme paint lands on the Button's own box — no decorative
        // wrapper node is introduced.
        let theme = Style::new().background(Oklch::WHITE).radius(px(6.0));
        let el = Button::builder()
            .styles([theme])
            .child(Text::builder().arg("Go").build())
            .build();
        let specs = lower_specs(&el).unwrap();
        assert_eq!(specs.len(), 1);
        let paint = box_paint_of(&specs[0]);
        assert_eq!(paint.fill, Some(Oklch::WHITE));
        assert_eq!(paint.radius, 6.0);
        // Exactly the explicit child: structure unchanged by styling.
        assert_eq!(specs[0].children.len(), 1);
        // The generic seam carries the same paint for non-box readers.
        assert_eq!(specs[0].paint.fill, Some(Oklch::WHITE));
    }

    #[test]
    fn typography_inherits_only_typography() {
        let el = Box::builder()
            .styles([Style::new()
                .text_color(Oklch::BLACK)
                .font_size(20.0)
                .width(px(200.0))
                .background(Oklch::WHITE)])
            .child(Text::builder().arg("inherits").build())
            .child(Text::builder().arg("override").color(Oklch::WHITE).build())
            .child(Box::builder().build())
            .build();
        let specs = lower_specs(&el).unwrap();
        // First label inherits color + size from the parent style.
        assert_eq!(text_color_of(&specs[0].children[0]), Oklch::BLACK);
        match &specs[0].children[0].kind {
            NodeKind::Text(t) => assert_eq!(t.font_size, 20.0),
            other => panic!("expected text, got {other:?}"),
        }
        // Explicit child override wins over inheritance.
        assert_eq!(text_color_of(&specs[0].children[1]), Oklch::WHITE);
        // Layout never inherits: the child keeps content sizing.
        assert_eq!(specs[0].children[0].style.width, Length::Auto);
        // Paint never inherits: the child box stays transparent.
        assert_eq!(box_paint_of(&specs[0].children[2]).fill, None);
    }

    #[test]
    fn explicit_none_clears() {
        let el = Box::builder()
            .styles([Style::new()
                .background(Oklch::WHITE)
                .clear_background()
                .opacity(0.5)
                .clear_opacity()])
            .build();
        let specs = lower_specs(&el).unwrap();
        let paint = box_paint_of(&specs[0]);
        assert_eq!(paint.fill, None);
        assert_eq!(paint.opacity, None);
    }

    #[test]
    fn uniform_corners_accepted_nonuniform_rejected_at_mount() {
        use aqua_core::CornerRadii;
        // All-equal corners are representable (emitter uses top_left).
        let el = Box::builder()
            .styles([Style::new().corners(CornerRadii::uniform(4.0))])
            .build();
        assert!(lower_specs(&el).is_ok());
        // Unequal radii have no scene seam: loud error, never a silent
        // fallback — for boxes and text alike.
        let bad = Style::new().corners(CornerRadii {
            top_left: 1.0,
            top_right: 2.0,
            bottom_right: 3.0,
            bottom_left: 4.0,
        });
        let el = Box::builder().styles([bad.clone()]).build();
        assert!(matches!(
            lower_specs(&el),
            Err(LowerError::NonUniformCorners)
        ));
        let el = Text::builder().arg("x").styles([bad]).build();
        assert!(matches!(
            lower_specs(&el),
            Err(LowerError::NonUniformCorners)
        ));
    }

    #[test]
    fn refresh_rejects_nonuniform_corners() {
        use crate::tree::UiTree;
        use aqua_core::CornerRadii;
        let mut tree = UiTree::new();
        let id = tree.mount_spec(lower_specs(&Box::builder().build()).unwrap().pop().unwrap());
        // Uniform corners refresh fine.
        let ok = Box::builder()
            .styles([Style::new().corners(CornerRadii::uniform(2.0))])
            .build();
        assert!(ok
            .refresh_mounted(&mut tree, id, &TypoChain::default())
            .unwrap());
        // Unequal radii fail loudly on refresh too (set_paint applies
        // directly, so lowering-time validation is repeated here).
        let bad = Box::builder()
            .styles([Style::new().corners(CornerRadii {
                top_left: 1.0,
                top_right: 2.0,
                bottom_right: 1.0,
                bottom_left: 1.0,
            })])
            .build();
        assert!(matches!(
            bad.refresh_mounted(&mut tree, id, &TypoChain::default()),
            Err(LowerError::NonUniformCorners)
        ));
    }

    #[test]
    fn new_aliases_and_ui_reexports_cover_all_builders() {
        // `new()` aliases: content-bearing builders take their content.
        let _ = Box::new();
        let _ = Button::new();
        let _ = Scroll::new();
        let _ = Canvas::new();
        let _ = Viewport3D::new();
        let _ = Fragment::new();
        let _ = Tabs::new();
        let _ = Text::new("hi");
        let _ = Checkbox::new("done");
        let _ = Slider::new(0.5);
        let _ = Dropdown::new("open");
        let _ = Media::new(ImageId::from_raw(aqua_core::RawId {
            index: 1,
            generation: 0,
        }));
        // `ui::` re-exports resolve the canonical styling API.
        let s = ui::Style::new()
            .width(ui::px(10.0))
            .height(ui::pct(50.0))
            .font_size(12.0);
        let _: ui::Typography = s.resolve_typography();
        let _: ui::Length = ui::auto();
        let el = ui::Box::new().styles([s]).build();
        assert_eq!(lower_specs(&el).unwrap()[0].style.width, Length::Px(10.0));
    }

    #[test]
    fn spec_paint_set_for_nonbox_kinds() {
        // Non-box nodes carry resolved paint in `NodeSpec::paint` for the
        // tree's generic emission (no wrapper nodes).
        let el = Text::builder()
            .arg("x")
            .styles([Style::new().background(Oklch::WHITE)])
            .build();
        let specs = lower_specs(&el).unwrap();
        assert!(matches!(specs[0].kind, NodeKind::Text(_)));
        assert_eq!(specs[0].paint.fill, Some(Oklch::WHITE));
    }

    #[test]
    fn composite_paint_comes_from_builder_not_hardcoded() {
        // Checkbox / Slider / Dropdown paints are builder state (stylable),
        // not hardcoded lowering defaults.
        let el = Checkbox::builder()
            .label("done")
            .styles([Style::new().background(Oklch::BLACK)])
            .build();
        let specs = lower_specs(&el).unwrap();
        assert_eq!(box_paint_of(&specs[0]).fill, Some(Oklch::BLACK));

        let el = Slider::builder()
            .styles([Style::new().background(Oklch::BLACK)])
            .build();
        let specs = lower_specs(&el).unwrap();
        assert_eq!(box_paint_of(&specs[0]).fill, Some(Oklch::BLACK));

        // Dropdown keeps its historical WHITE activator until styled.
        let el = Dropdown::builder().label("open").build();
        let specs = lower_specs(&el).unwrap();
        assert_eq!(box_paint_of(&specs[0]).fill, Some(Oklch::WHITE));
        let el = Dropdown::builder()
            .label("open")
            .styles([Style::new().background(Oklch::BLACK)])
            .build();
        let specs = lower_specs(&el).unwrap();
        assert_eq!(box_paint_of(&specs[0]).fill, Some(Oklch::BLACK));
    }

    #[test]
    fn slider_size_default_is_initial_not_coercion() {
        // Default 160x24 track is set once in the builder; an explicit
        // `auto()` stays content-sized (no lower coercion snaps it back).
        let el = Slider::builder().build();
        let specs = lower_specs(&el).unwrap();
        assert_eq!(specs[0].style.width, Length::Px(160.0));
        assert_eq!(specs[0].style.height, Length::Px(24.0));
        let el = Slider::builder().width(auto()).height(auto()).build();
        let specs = lower_specs(&el).unwrap();
        assert_eq!(specs[0].style.width, Length::Auto);
        assert_eq!(specs[0].style.height, Length::Auto);
        let el = Slider::builder()
            .styles([Style::new().width(px(200.0))])
            .build();
        let specs = lower_specs(&el).unwrap();
        assert_eq!(specs[0].style.width, Length::Px(200.0));
    }

    #[test]
    fn checkbox_gap_default_is_initial_not_coercion() {
        // Default 8px row gap is set once in the builder; an explicit zero
        // stays zero (no build/lower coercion can clobber it).
        let el = Checkbox::builder().label("x").build();
        let specs = lower_specs(&el).unwrap();
        assert_eq!(specs[0].style.gap, 8.0);
        let el = Checkbox::builder().label("x").gap(0).build();
        let specs = lower_specs(&el).unwrap();
        assert_eq!(specs[0].style.gap, 0.0);
        let el = Checkbox::builder()
            .label("x")
            .styles([Style::new().gap(12.0)])
            .build();
        let specs = lower_specs(&el).unwrap();
        assert_eq!(specs[0].style.gap, 12.0);
    }

    #[test]
    fn element_level_styles_apply_and_fragments_recurse() {
        // `.styles` on a built element merges into its own resolved parts.
        let el = Text::builder()
            .arg("x")
            .build()
            .styles([Style::new().font_size(22.0).text_color(Oklch::BLACK)]);
        let specs = lower_specs(&el).unwrap();
        match &specs[0].kind {
            NodeKind::Text(t) => {
                assert_eq!(t.font_size, 22.0);
                assert_eq!(t.color, Oklch::BLACK);
            }
            other => panic!("expected text, got {other:?}"),
        }
        // On a fragment, styles apply to every descendant (fragments own
        // no node, so there is nothing else they could mean).
        let el = Fragment::builder()
            .child(Text::builder().arg("a").build())
            .child(Text::builder().arg("b").build())
            .build()
            .styles([Style::new().text_color(Oklch::BLACK)]);
        let specs = lower_specs(&el).unwrap();
        assert_eq!(specs.len(), 2);
        assert_eq!(text_color_of(&specs[0]), Oklch::BLACK);
        assert_eq!(text_color_of(&specs[1]), Oklch::BLACK);
    }

    #[test]
    fn dynamic_inherits_typography_chain() {
        let rt = aqua_reactive::Runtime::new();
        let scope = rt.create_scope();
        let (v, _set) = scope.signal(String::from("d"));
        // Own explicit wins; otherwise the parent chain applies.
        let el = Box::builder()
            .styles([Style::new().text_color(Oklch::BLACK).font_size(18.0)])
            .child(Dynamic::builder().value(v.clone()).build())
            .child(
                Dynamic::builder()
                    .value(v.clone())
                    .text_color(Oklch::WHITE)
                    .build(),
            )
            .build();
        let specs = lower_specs(&el).unwrap();
        match &specs[0].children[0].kind {
            NodeKind::Text(t) => {
                assert_eq!(t.color, Oklch::BLACK);
                assert_eq!(t.font_size, 18.0);
            }
            other => panic!("expected text, got {other:?}"),
        }
        match &specs[0].children[1].kind {
            NodeKind::Text(t) => {
                assert_eq!(t.color, Oklch::WHITE);
                assert_eq!(t.font_size, 18.0);
            }
            other => panic!("expected text, got {other:?}"),
        }
    }

    // -- Collection elements + children() sugar (runtime owner) --

    use crate::collection_runtime::{
        CollectionDescriptor, CollectionError, CollectionLayout, CollectionRole, CollectionSource,
    };
    use std::rc::Rc;

    struct FakeList {
        n: usize,
    }

    impl CollectionSource for FakeList {
        fn len(&self) -> usize {
            self.n
        }
        fn key(&self, index: usize) -> Key {
            Key::Usize(index)
        }
        fn layout(&self, available_width: f32) -> CollectionLayout {
            CollectionLayout {
                content_width: available_width,
                column_widths: Vec::new(),
            }
        }
        fn row(&self, index: usize, _layout: &CollectionLayout) -> Element {
            Text::builder().arg(format!("row-{index}")).build()
        }
        fn header(&self, _layout: &CollectionLayout) -> Option<Element> {
            None
        }
        fn role(&self) -> CollectionRole {
            CollectionRole::List
        }
        fn validate(&self) -> Result<(), CollectionError> {
            Ok(())
        }
    }

    fn list_desc(n: usize, virtualized: bool) -> CollectionDescriptor {
        CollectionDescriptor::new(Rc::new(FakeList { n }), Vec::new(), virtualized, 32.0, 4)
    }

    #[test]
    fn children_sugar_appends_all_builders_in_order() {
        // Arrays, Vecs, and lazy iterators all work; repeated calls append.
        let el = Box::builder()
            .children([Text::builder().arg("a").build()])
            .children((0..2).map(|i| Text::builder().arg(format!("{i}")).build()))
            .child(Text::builder().arg("z").build())
            .build();
        let specs = lower_specs(&el).unwrap();
        assert_eq!(specs[0].children.len(), 4);
        // Fragment + Element level too.
        let el = Fragment::builder()
            .children([Text::builder().arg("a").build()])
            .build()
            .children([Text::builder().arg("b").build()]);
        assert_eq!(lower_specs(&el).unwrap().len(), 2);
    }

    #[test]
    fn collection_virtualized_lowers_empty_managed_shell() {
        let el = Element::collection(list_desc(100, true));
        assert!(el.is_collection());
        assert!(el.collection_descriptor().is_some());
        assert!(el.role_opt().is_none());
        let specs = lower_specs(&el).unwrap();
        assert_eq!(specs.len(), 1);
        // Shell-only: Scroll body, managed, NO row children and NO factory
        // output — the driver realizes the window and binds exactly once.
        assert!(matches!(specs[0].kind, NodeKind::Scroll(_)));
        assert!(specs[0].collection_managed);
        assert!(specs[0].children.is_empty());
        // Unbounded virtual height defaults to viewport fill so the root
        // cannot expand to the spacer total and defeat windowing.
        assert_eq!(specs[0].style.height, Length::Pct(100.0));
        assert_eq!(specs[0].style.width, Length::Pct(100.0));
    }

    #[test]
    fn collection_nonvirtualized_lowers_empty_managed_shell() {
        // The driver owns ALL realization in both modes (mounts every row
        // and binds once); lowering stays an empty managed shell.
        let specs = lower_specs(&Element::collection(list_desc(5, false))).unwrap();
        assert!(matches!(specs[0].kind, NodeKind::Box(_)));
        assert!(specs[0].collection_managed);
        assert!(specs[0].children.is_empty());
        // Non-virtualized keeps content sizing (mount-all needs no bound).
        assert_eq!(specs[0].style.height, Length::Auto);
    }

    #[test]
    fn collection_table_shell_is_box_both_modes() {
        use std::cell::Cell;
        struct FakeTable {
            n: usize,
            rows_built: Rc<Cell<usize>>,
            heads_built: Rc<Cell<usize>>,
        }
        impl CollectionSource for FakeTable {
            fn len(&self) -> usize {
                self.n
            }
            fn key(&self, index: usize) -> Key {
                Key::Usize(index)
            }
            fn layout(&self, available_width: f32) -> CollectionLayout {
                CollectionLayout {
                    content_width: available_width,
                    column_widths: vec![available_width / 2.0; 2],
                }
            }
            fn row(&self, index: usize, _layout: &CollectionLayout) -> Element {
                self.rows_built.set(self.rows_built.get() + 1);
                Text::builder().arg(format!("row-{index}")).build()
            }
            fn header(&self, _layout: &CollectionLayout) -> Option<Element> {
                self.heads_built.set(self.heads_built.get() + 1);
                Some(Text::builder().arg("head").build())
            }
            fn role(&self) -> CollectionRole {
                CollectionRole::Table
            }
            fn validate(&self) -> Result<(), CollectionError> {
                Ok(())
            }
        }
        for virtualized in [true, false] {
            let rows_built = Rc::new(Cell::new(0usize));
            let heads_built = Rc::new(Cell::new(0usize));
            let desc = CollectionDescriptor::new(
                Rc::new(FakeTable {
                    n: 8,
                    rows_built: Rc::clone(&rows_built),
                    heads_built: Rc::clone(&heads_built),
                }),
                Vec::new(),
                virtualized,
                32.0,
                4,
            );
            let specs = lower_specs(&Element::collection(desc)).unwrap();
            // Table root is Box in both modes (header clips wrong under a
            // Scroll root); empty until the driver realizes.
            assert!(matches!(specs[0].kind, NodeKind::Box(_)));
            assert!(specs[0].collection_managed);
            assert!(specs[0].children.is_empty());
            // Shell-only: zero row/header factory calls (the driver runs
            // each exactly once when it realizes and binds).
            assert_eq!(rows_built.get(), 0);
            assert_eq!(heads_built.get(), 0);
        }
    }

    #[test]
    fn collection_rejects_explicit_children_and_bad_policy() {
        let el = Element::collection(list_desc(10, true)).child(Text::builder().arg("x").build());
        assert!(matches!(
            lower_specs(&el),
            Err(LowerError::CollectionChildren)
        ));
        let bad =
            CollectionDescriptor::new(Rc::new(FakeList { n: 10 }), Vec::new(), true, f32::NAN, 4);
        assert!(matches!(
            lower_specs(&Element::collection(bad)),
            Err(LowerError::Collection(CollectionError::InvalidSize(_)))
        ));
    }

    #[test]
    fn collection_duplicate_keys_fail_loudly() {
        struct Dup;
        impl CollectionSource for Dup {
            fn len(&self) -> usize {
                3
            }
            fn key(&self, _: usize) -> Key {
                Key::Usize(7)
            }
            fn layout(&self, w: f32) -> CollectionLayout {
                CollectionLayout {
                    content_width: w,
                    column_widths: Vec::new(),
                }
            }
            fn row(&self, i: usize, _: &CollectionLayout) -> Element {
                Text::builder().arg(format!("{i}")).build()
            }
            fn header(&self, _: &CollectionLayout) -> Option<Element> {
                None
            }
            fn role(&self) -> CollectionRole {
                CollectionRole::List
            }
            fn validate(&self) -> Result<(), CollectionError> {
                Ok(())
            }
        }
        let desc = CollectionDescriptor::new(Rc::new(Dup), Vec::new(), true, 32.0, 4);
        assert!(matches!(
            lower_specs(&Element::collection(desc)),
            Err(LowerError::Collection(CollectionError::DuplicateKey))
        ));
    }

    #[test]
    fn element_role_sets_and_fragment_propagates() {
        use aqua_core::Role;
        let el = Text::builder().arg("x").build().role(Role::Button);
        assert_eq!(el.role_opt(), Some(Role::Button));
        // Fragment: role lands on children lacking one.
        let el = Fragment::builder()
            .child(Text::builder().arg("a").build())
            .child(Text::builder().arg("b").build().role(Role::Cell))
            .build()
            .role(Role::Row);
        let specs = lower_specs(&el).unwrap();
        assert_eq!(specs.len(), 2);
        // Propagation is observable at bind time through role_opt.
        let el2 = Fragment::builder()
            .child(Text::builder().arg("a").build())
            .build()
            .role(Role::Row);
        let flat = el2.flat_children();
        assert_eq!(flat.len(), 1);
        assert_eq!(flat[0].role_opt(), Some(Role::Row));
    }

    #[test]
    fn collection_element_style_merges_in_order() {
        let desc = CollectionDescriptor::new(
            Rc::new(FakeList { n: 3 }),
            vec![Style::new().width(px(100.0))],
            false,
            32.0,
            4,
        );
        let el = Element::collection(desc)
            .style(Style::new().width(px(200.0)))
            .styles([Style::new().gap(4.0)]);
        let specs = lower_specs(&el).unwrap();
        assert_eq!(specs[0].style.width, Length::Px(200.0));
        assert_eq!(specs[0].style.gap, 4.0);
    }
}
