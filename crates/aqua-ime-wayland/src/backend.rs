use crate::model::{Batch, Link, Outgoing, Refusal, Session};
use aqua_protocol::{ImeBackend, ImeCommand, ImeEvent};
use aqua_shell_wayland::{
    CursorRect, Surrounding, TextInput, TextInputEvent, TextInputState, Window,
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WaylandImeError {
    #[error("text input for window {window:?}: {reason}")]
    Attach {
        window: aqua_protocol::WindowId,
        reason: String,
    },
    #[error("{0} is not supported by text-input-v3 on Wayland")]
    Unsupported(&'static str),
}

struct Wire<'a>(&'a TextInput);

fn state(outgoing: &Outgoing) -> TextInputState {
    TextInputState {
        surrounding: Some(Surrounding {
            text: outgoing.surrounding.text.clone(),
            cursor: outgoing.surrounding.cursor,
            anchor: outgoing.surrounding.anchor,
        }),
        cursor: outgoing.caret.map(|caret| CursorRect {
            x: caret.x,
            y: caret.y,
            width: caret.width,
            height: caret.height,
        }),
        changed_by_input_method: outgoing.changed_by_input_method,
    }
}

impl Link for Wire<'_> {
    fn enable(&mut self, outgoing: &Outgoing) -> Option<u32> {
        self.0.enable(&state(outgoing))
    }

    fn update(&mut self, outgoing: &Outgoing) -> Option<u32> {
        self.0.update(&state(outgoing))
    }

    fn disable(&mut self) -> Option<u32> {
        self.0.disable()
    }
}

pub struct WaylandIme {
    input: TextInput,
    session: Session,
    incoming: Vec<TextInputEvent>,
}

impl WaylandIme {
    pub fn new(window: Window) -> Result<Self, WaylandImeError> {
        let input = TextInput::attach(window).map_err(|error| WaylandImeError::Attach {
            window: window.id(),
            reason: error.to_string(),
        })?;
        let session = Session::new(input.is_focused());
        Ok(Self {
            input,
            session,
            incoming: Vec::new(),
        })
    }

    pub fn is_composing(&self) -> bool {
        self.session.is_composing()
    }

    fn pull(&mut self) {
        self.input.drain(&mut self.incoming);
        let mut wire = Wire(&self.input);
        for event in self.incoming.drain(..) {
            match event {
                TextInputEvent::Entered => self.session.entered(&mut wire),
                TextInputEvent::Left => self.session.left(),
                TextInputEvent::Done(batch) => self.session.done(Batch {
                    preedit: batch.preedit.map(|preedit| (preedit.text, preedit.cursor)),
                    commit: batch.commit,
                    delete: batch.delete,
                    serial: batch.serial,
                }),
            }
        }
    }
}

impl ImeBackend for WaylandIme {
    type Error = WaylandImeError;

    fn command(&mut self, command: ImeCommand) -> Result<(), WaylandImeError> {
        self.pull();
        self.session
            .command(command, &mut Wire(&self.input))
            .map_err(|refusal| match refusal {
                Refusal::Reconversion => WaylandImeError::Unsupported("reconversion"),
            })
    }

    fn drain(&mut self, out: &mut Vec<ImeEvent>) {
        self.pull();
        self.session.drain(out);
    }
}
