//! End-to-end tests for `ui!`: the macro expands against a mock facade
//! implementing the runtime contract from `docs/UI_MACRO.md`.
//!
//! Each test uses the `crate = crate::mock;` preamble so the generated code
//! targets this file's `mock` module instead of the real `::aqua` facade
//! (which is owned by the facade agent). Switching a test to the real facade
//! only requires changing the preamble and the expected builder surface.

use aqua_macros::ui;

/// Minimal stand-in for the `::aqua::ui` runtime surface the macro targets.
/// Mirrors the contract in `docs/UI_MACRO.md`; the real `aqua-ui`/`aqua`
/// implementation must provide the same shapes (method names and call order).
pub mod mock {
    pub mod ui {
        #[derive(Debug, Clone)]
        pub struct Element {
            pub kind: String,
            pub props: Vec<(String, String)>,
            pub children: Vec<Element>,
            pub key: Option<String>,
        }

        impl Element {
            pub fn with_key(mut self, key: impl Into<Key>) -> Self {
                self.key = Some(key.into().0);
                self
            }

            /// S-expression rendering for assertions, e.g.
            /// `(Box layout=column gap=16 (Text arg="hi"))`.
            pub fn to_debug(&self) -> String {
                let mut s = String::from("(");
                s.push_str(&self.kind);
                if let Some(k) = &self.key {
                    s.push_str(&format!(" key={k}"));
                }
                for (k, v) in &self.props {
                    s.push_str(&format!(" {k}={v}"));
                }
                for c in &self.children {
                    s.push(' ');
                    s.push_str(&c.to_debug());
                }
                s.push(')');
                s
            }
        }

        #[derive(Debug, Clone)]
        pub struct Key(pub String);
        impl From<&str> for Key {
            fn from(v: &str) -> Self {
                Key(v.to_owned())
            }
        }
        impl From<String> for Key {
            fn from(v: String) -> Self {
                Key(v)
            }
        }
        impl From<u64> for Key {
            fn from(v: u64) -> Self {
                Key(v.to_string())
            }
        }
        impl From<u32> for Key {
            fn from(v: u32) -> Self {
                Key(v.to_string())
            }
        }
        impl From<usize> for Key {
            fn from(v: usize) -> Self {
                Key(v.to_string())
            }
        }
        impl From<i32> for Key {
            fn from(v: i32) -> Self {
                Key(v.to_string())
            }
        }

        fn prop<T: std::fmt::Debug>(v: T) -> String {
            format!("{v:?}")
        }

        // ---- Fragment ----------------------------------------------------
        #[derive(Debug, Default)]
        pub struct Fragment {
            children: Vec<Element>,
        }

        impl Fragment {
            pub fn builder() -> Self {
                Fragment::default()
            }
            pub fn child(mut self, child: impl Into<Element>) -> Self {
                self.children.push(child.into());
                self
            }
            pub fn build(self) -> Element {
                Element {
                    kind: "Fragment".into(),
                    props: vec![],
                    children: self.children,
                    key: None,
                }
            }
            pub fn from_nodes<I, E>(iter: I) -> Fragment
            where
                I: IntoIterator<Item = E>,
                E: Into<Element>,
            {
                Fragment {
                    children: iter.into_iter().map(Into::into).collect(),
                }
            }
        }

        impl From<Fragment> for Element {
            fn from(f: Fragment) -> Self {
                f.build()
            }
        }

        macro_rules! builder {
            ($name:ident, [ $( $prop:ident ),* ]) => {
                #[derive(Debug, Default)]
                pub struct $name {
                    props: Vec<(String, String)>,
                    children: Vec<Element>,
                }
                impl $name {
                    pub fn builder() -> Self {
                        Self::default()
                    }
                    pub fn arg(mut self, v: impl std::fmt::Debug) -> Self {
                        self.props.push(("arg".into(), prop(v)));
                        self
                    }
                    $(
                        pub fn $prop(mut self, v: impl std::fmt::Debug) -> Self {
                            self.props.push((stringify!($prop).into(), prop(v)));
                            self
                        }
                    )*
                    pub fn child(mut self, child: impl Into<Element>) -> Self {
                        self.children.push(child.into());
                        self
                    }
                    pub fn build(self) -> Element {
                        Element {
                            kind: stringify!($name).into(),
                            props: self.props,
                            children: self.children,
                            key: None,
                        }
                    }
                }
            };
        }

        builder!(Box, [layout, gap, style]);
        builder!(
            Text,
            [
                value,
                style,
                bind,
                editable,
                multiline,
                wrap,
                placeholder,
                selectable
            ]
        );
        builder!(Dynamic, [value]);
        builder!(Scroll, [layout]);
        builder!(Media, [source]);
        builder!(Canvas, [layout]);
        builder!(Viewport3D, [scene]);

        /// Button needs a non-`Debug` callback prop, so it is hand-written
        /// rather than generated by `builder!`.
        #[derive(Debug, Default)]
        pub struct Button {
            props: Vec<(String, String)>,
            children: Vec<Element>,
        }
        impl Button {
            pub fn builder() -> Self {
                Self::default()
            }
            pub fn arg(mut self, v: impl std::fmt::Debug) -> Self {
                self.props.push(("arg".into(), prop(v)));
                self
            }
            pub fn layout(mut self, v: impl std::fmt::Debug) -> Self {
                self.props.push(("layout".into(), prop(v)));
                self
            }
            pub fn on_activate<F>(mut self, _f: F) -> Self
            where
                F: Fn(()) + 'static,
            {
                self.props.push(("on_activate".into(), "callback".into()));
                self
            }
            pub fn child(mut self, child: impl Into<Element>) -> Self {
                self.children.push(child.into());
                self
            }
            pub fn build(self) -> Element {
                Element {
                    kind: "Button".into(),
                    props: self.props,
                    children: self.children,
                    key: None,
                }
            }
        }
    }

    /// Custom user component outside `mock::ui`, proving custom paths resolve
    /// to `<Path>::builder()` rather than the facade path.
    #[derive(Debug, Default)]
    pub struct Card {
        props: Vec<(String, String)>,
        children: Vec<super::mock::ui::Element>,
    }

    impl Card {
        pub fn builder() -> Self {
            Self::default()
        }
        pub fn title(mut self, v: impl std::fmt::Debug) -> Self {
            self.props.push(("title".into(), format!("{v:?}")));
            self
        }
        pub fn child(mut self, child: impl Into<super::mock::ui::Element>) -> Self {
            self.children.push(child.into());
            self
        }
        pub fn build(self) -> super::mock::ui::Element {
            super::mock::ui::Element {
                kind: "Card".into(),
                props: self.props,
                children: self.children,
                key: None,
            }
        }
    }
}

use mock::ui::Element;

#[test]
fn todo_app_structure() {
    let theme_heading = "heading-style";
    let column = "column";
    let row = "row";
    let mut new_todo = String::from("buy milk");
    let added: Vec<String> = Vec::new();
    let added_len = added.len();
    let el: Element = ui! {
        crate = crate::mock;
        Box(layout: column, gap: 16) {
            Text("Todo App", style: theme_heading)
            Box(layout: row, gap: 8) {
                Text(bind: new_todo.clone(), editable: true, placeholder: "New todo...")
                Button(on_activate: move |_: ()| { let _ = added_len; }) {
                    Text("Add")
                }
            }
        }
    };
    let _ = &mut new_todo;
    let dbg = el.to_debug();
    assert!(dbg.contains("(Box layout=\"column\" gap=16"), "{dbg}");
    assert!(
        dbg.contains("(Text arg=\"Todo App\" style=\"heading-style\""),
        "{dbg}"
    );
    assert!(dbg.contains("(Button"), "{dbg}");
    assert!(dbg.contains("(Text arg=\"Add\""), "{dbg}");
    assert!(dbg.contains("placeholder=\"New todo...\""), "{dbg}");
}

#[test]
fn for_key_iteration_preserves_identity() {
    struct Todo {
        id: u64,
        label: String,
    }
    let todos = [
        Todo {
            id: 1,
            label: "one".into(),
        },
        Todo {
            id: 2,
            label: "two".into(),
        },
    ];
    let column = "column";
    let el: Element = ui! {
        crate = crate::mock;
        Box(layout: column) {
            for todo in todos.iter() {
                key(todo.id) {
                    Text(todo.label.clone())
                }
            }
        }
    };
    let dbg = el.to_debug();
    assert!(dbg.contains("key=1"), "{dbg}");
    assert!(dbg.contains("key=2"), "{dbg}");
    // Keys appear in iteration order.
    assert!(
        dbg.find("key=1").unwrap() < dbg.find("key=2").unwrap(),
        "{dbg}"
    );
}

#[test]
fn conditionals_branches_and_match() {
    for (flag, want) in [(true, "\"a\""), (false, "\"b\"")] {
        let el: Element = ui! {
            crate = crate::mock;
            Box {
                if flag { Text("a") } else { Text("b") }
            }
        };
        assert!(el.to_debug().contains(want), "{}", el.to_debug());
    }
    let el: Element = ui! {
        crate = crate::mock;
        Box {
            if false { Text("x") }
        }
    };
    // `if` without `else` yields an empty fragment child.
    assert!(el.to_debug().contains("(Fragment)"), "{}", el.to_debug());

    enum State {
        On(String),
        Off,
    }
    let state = State::On("lit".to_owned());
    let el: Element = ui! {
        crate = crate::mock;
        Box {
            match state {
                State::On(label) => { Text(label) },
                State::Off => { Text("off") },
            }
        }
    };
    assert!(el.to_debug().contains("\"lit\""), "{}", el.to_debug());
    let off = State::Off;
    let el: Element = ui! {
        crate = crate::mock;
        Box {
            match off {
                State::On(label) => { Text(label) },
                State::Off => { Text("off") },
            }
        }
    };
    assert!(el.to_debug().contains("\"off\""), "{}", el.to_debug());
}

#[test]
fn lets_blocks_and_plain_exprs() {
    fn format_title(prefix: &str) -> String {
        format!("{prefix}!")
    }
    // Custom render function returning an `Element`: usable from `ui!` via a
    // Rust block escape. (A `{ ... }` group directly after `Comp(...)` binds
    // as that component's children; the `;` here separates the two nodes.)
    fn todo_row(label: &str) -> Element {
        ui! {
            crate = crate::mock;
            Text(label)
        }
    }
    let el: Element = ui! {
        crate = crate::mock;
        Box {
            let title = format_title("todos");
            Text(title.clone());
            { todo_row(&title) }
        }
    };
    let dbg = el.to_debug();
    assert!(dbg.contains("\"todos!\""), "{dbg}");
}

#[test]
fn dynamic_is_explicit_no_hidden_subscription() {
    // `Dynamic(value: ...)` is an ordinary component: the expression is
    // passed through untouched and no reactive read or subscription happens
    // inside the macro. Wiring the value to the reactive graph is the
    // runtime builder's job (aqua-reactive integration, pending).
    let label = String::from("live");
    let el: Element = ui! {
        crate = crate::mock;
        Box {
            Dynamic(value: label.clone())
        }
    };
    let dbg = el.to_debug();
    assert!(dbg.contains("(Dynamic value=\"live\")"), "{dbg}");
}

#[test]
fn custom_component_and_equals_separator() {
    let el: Element = ui! {
        crate = crate::mock;
        crate::mock::Card(title = "hello") {
            Text("inside")
        }
    };
    let dbg = el.to_debug();
    assert!(dbg.contains("(Card title=\"hello\""), "{dbg}");
    assert!(dbg.contains("(Text arg=\"inside\""), "{dbg}");
}
