use std::cell::Cell;
use std::rc::Rc;

use aqua::{px, Aqua, Collection, Context, LiveList, Program, Style};
use aqua_core::Size;

#[derive(Clone, PartialEq)]
struct Message {
    text: String,
    unread: bool,
    sequence: u32,
}

fn conversation(context: &Context) -> Program {
    context.program(|scope| {
        let messages = Collection::new(
            scope,
            (0..20_000u32).map(|sequence| {
                (
                    sequence,
                    Message {
                        text: format!("Message {sequence}: a retained conversation row."),
                        unread: sequence % 7 == 0,
                        sequence,
                    },
                )
            }),
        )
        .expect("initial message keys are unique");
        let unread = messages.count_where(scope, |message| message.unread);
        let unread_label = scope.derive((unread,), |(count,)| format!("{count} unread messages"));
        let query = messages.query(scope, |_| true, |message| message.sequence);
        let unread_only = Rc::new(Cell::new(false));
        let filter_query = query.clone();
        let filter = Aqua::Button()
            .on_activate(move || {
                let enabled = !unread_only.get();
                unread_only.set(enabled);
                let _ = filter_query.configure(
                    move |message| !enabled || message.unread,
                    |message| message.sequence,
                );
            })
            .child(Aqua::Text("Toggle unread filter").build())
            .build();
        let streaming = messages.clone();
        let append = Aqua::Button()
            .on_activate(move || {
                if let Some(message) = streaming.get(&0) {
                    let mut next = (*message).clone();
                    next.text.push_str(" another token");
                    let _ = streaming.update(&0, next);
                }
            })
            .child(Aqua::Text("Append token to first message").build())
            .build();
        let mark_read = Aqua::Button()
            .on_activate(move || {
                let _ = messages.update_field(
                    &0,
                    false,
                    |message| message.unread,
                    |message, unread| message.unread = unread,
                );
            })
            .child(Aqua::Text("Mark first message read").build())
            .build();
        let list = LiveList::query(query, |owner, _, record| {
            let text = owner.derive((record,), |(message,)| message.text.clone());
            Aqua::Text(text)
                .build()
                .layout_boundary(Size::new(800.0, 32.0))
        })
        .styles([Style::new().width(px(840.0)).height(px(500.0))])
        .build();
        Aqua::Box()
            .layout("column")
            .gap(12)
            .padding(20)
            .child(Aqua::Text(unread_label).build())
            .child(
                Aqua::Box()
                    .gap(16)
                    .children([filter, append, mark_read])
                    .build(),
            )
            .child(list)
            .build()
    })
}

fn main() -> Result<(), aqua::Error> {
    let headless = std::env::args().any(|argument| argument == "--headless");
    Aqua::App()
        .config(
            Aqua::Config().headless(headless).window(
                Aqua::Window()
                    .title("Aqua — Persistent conversation")
                    .size(px(900.0), px(640.0)),
            ),
        )
        .run(conversation)
}
