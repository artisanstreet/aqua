//! Runnable `ui!` todo-list example against a miniature mock facade.
//!
//! Run with `cargo run -p aqua-macros --example todo` (once the crate joins
//! the workspace; inside this directory `cargo run --example todo` works).

#![allow(dead_code)] // mock builders intentionally cover more than the demo uses
//! It exercises the design-doc authoring shape — `Box` layout, keyed `for`
//! rows, conditionals, and callbacks — and prints the resulting element tree.
//! Against the real `::aqua` facade only the macro preamble changes.

use aqua_macros::ui;

mod mock {
    pub mod ui {
        #[derive(Debug)]
        pub struct Element {
            pub kind: &'static str,
            pub props: Vec<(&'static str, String)>,
            pub children: Vec<Element>,
            pub key: Option<String>,
        }

        impl Element {
            pub fn with_key(mut self, key: impl Into<Key>) -> Self {
                self.key = Some(key.into().0);
                self
            }
            pub fn render(&self, indent: usize) {
                let pad = "  ".repeat(indent);
                let key = self
                    .key
                    .as_deref()
                    .map(|k| format!(" key={k}"))
                    .unwrap_or_default();
                let props = self
                    .props
                    .iter()
                    .map(|(k, v)| format!(" {k}={v}"))
                    .collect::<String>();
                println!("{pad}<{}{key}{props}>", self.kind);
                for c in &self.children {
                    c.render(indent + 1);
                }
                println!("{pad}</{}>", self.kind);
            }
        }

        pub struct Key(pub String);
        impl From<u64> for Key {
            fn from(v: u64) -> Self {
                Key(v.to_string())
            }
        }

        macro_rules! leaf {
            ($name:ident, [ $( $prop:ident ),* ]) => {
                #[derive(Default)]
                pub struct $name {
                    props: Vec<(&'static str, String)>,
                    children: Vec<Element>,
                }
                impl $name {
                    pub fn builder() -> Self {
                        Self::default()
                    }
                    pub fn arg(mut self, v: impl std::fmt::Debug) -> Self {
                        self.props.push(("arg", format!("{v:?}")));
                        self
                    }
                    $(
                        pub fn $prop(mut self, v: impl std::fmt::Debug) -> Self {
                            self.props.push((stringify!($prop), format!("{v:?}")));
                            self
                        }
                    )*
                    pub fn child(mut self, c: impl Into<Element>) -> Self {
                        self.children.push(c.into());
                        self
                    }
                    pub fn build(self) -> Element {
                        Element {
                            kind: stringify!($name),
                            props: self.props,
                            children: self.children,
                            key: None,
                        }
                    }
                }
            };
        }

        leaf!(Box, [layout, gap]);
        leaf!(Text, [style, bind, editable, placeholder]);
        leaf!(Fragment, []);

        impl Fragment {
            pub fn from_nodes<I, E>(iter: I) -> Fragment
            where
                I: IntoIterator<Item = E>,
                E: Into<Element>,
            {
                Fragment {
                    props: vec![],
                    children: iter.into_iter().map(Into::into).collect(),
                }
            }
        }
        impl From<Fragment> for Element {
            fn from(f: Fragment) -> Self {
                f.build()
            }
        }

        #[derive(Default)]
        pub struct Button {
            props: Vec<(&'static str, String)>,
            children: Vec<Element>,
        }
        impl Button {
            pub fn builder() -> Self {
                Self::default()
            }
            pub fn on_activate<F: Fn(()) + 'static>(mut self, _f: F) -> Self {
                self.props.push(("on_activate", "callback".into()));
                self
            }
            pub fn child(mut self, c: impl Into<Element>) -> Self {
                self.children.push(c.into());
                self
            }
            pub fn build(self) -> Element {
                Element {
                    kind: "Button",
                    props: self.props,
                    children: self.children,
                    key: None,
                }
            }
        }
    }
}

struct Todo {
    id: u64,
    label: String,
    done: bool,
}

fn main() {
    let column = "column";
    let row = "row";
    let heading = "heading";
    let todos = [
        Todo {
            id: 1,
            label: "write macro".into(),
            done: true,
        },
        Todo {
            id: 2,
            label: "wire builders".into(),
            done: false,
        },
    ];
    let show_done = true;

    let el: mock::ui::Element = ui! {
        crate = crate::mock;
        Box(layout: column, gap: 16) {
            Text("Todo App", style: heading)
            Box(layout: row, gap: 8) {
                Text(bind: "new todo", editable: true, placeholder: "New todo...")
                Button(on_activate: move |_: ()| {}) {
                    Text("Add")
                }
            }
            for todo in todos.iter() {
                key(todo.id) {
                    if todo.done {
                        Text(todo.label.clone())
                    } else if show_done {
                        Text(todo.label.clone(), style: heading)
                    } else {
                        Text("hidden")
                    }
                }
            }
        }
    };
    el.render(0);
}
