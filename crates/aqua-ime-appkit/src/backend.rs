use crate::model::{Effect, Inserted, Refusal, ScreenMap, Session};
use aqua_protocol::{ImeBackend, ImeCommand, ImeEvent};
use aqua_shell_appkit::{Insertion, ScreenRect, TextInputClient, Utf16Range, Window};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AppKitImeError {
    #[error("{0}")]
    Unsupported(&'static str),
}

fn units(value: usize) -> u32 {
    u32::try_from(value).expect("invariant: NSRanges from AppKit fit the UTF-16 document")
}

fn protocol(range: Utf16Range) -> aqua_protocol::Utf16Range {
    aqua_protocol::Utf16Range::new(units(range.start), units(range.end()))
}

fn shell(range: aqua_protocol::Utf16Range) -> Utf16Range {
    Utf16Range {
        start: range.start as usize,
        len: range.len() as usize,
    }
}

fn screen_map(window: Window) -> Option<ScreenMap> {
    window.view_on_screen().map(|view| ScreenMap {
        left: view.x,
        top: view.y + view.height,
    })
}

struct Client {
    window: Window,
    session: Rc<RefCell<Session>>,
}

impl Client {
    fn push<T>(&self, change: impl FnOnce(&mut Session) -> T) -> T {
        let (result, pending) = {
            let mut session = self.session.borrow_mut();
            let result = change(&mut session);
            (result, session.has_events())
        };
        if pending {
            self.window.waker().wake();
        }
        result
    }
}

impl TextInputClient for Client {
    fn key_pressed(&mut self) {
        self.push(Session::key_pressed);
    }

    fn deactivated(&mut self) {
        self.push(Session::deactivated);
    }

    fn marked_range(&self) -> Option<Utf16Range> {
        self.session.borrow().marked_range().map(shell)
    }

    fn selected_range(&self) -> Option<Utf16Range> {
        self.session.borrow().selected_range().map(shell)
    }

    fn set_marked_text(
        &mut self,
        text: &str,
        selected: Utf16Range,
        replacement: Option<Utf16Range>,
    ) {
        self.push(|session| {
            session.set_marked_text(text, protocol(selected), replacement.map(protocol))
        });
    }

    fn unmark_text(&mut self) {
        self.push(Session::unmark_text);
    }

    fn insert_text(&mut self, text: &str, replacement: Option<Utf16Range>) -> Insertion {
        match self.push(|session| session.insert_text(text, replacement.map(protocol))) {
            Inserted::Typed => Insertion::Typed,
            Inserted::Taken => Insertion::Taken,
        }
    }

    fn attributed_substring(&self, range: Utf16Range) -> Option<(String, Utf16Range)> {
        self.session
            .borrow()
            .attributed_substring(protocol(range))
            .map(|(text, covered)| (text, shell(covered)))
    }

    fn first_rect(&self, range: Utf16Range) -> Option<(ScreenRect, Utf16Range)> {
        let map = screen_map(self.window)?;
        self.session.borrow().first_rect(protocol(range), map).map(
            |((x, y, width, height), covered)| {
                (
                    ScreenRect {
                        x,
                        y,
                        width,
                        height,
                    },
                    shell(covered),
                )
            },
        )
    }

    fn character_index(&self, screen_point: (f64, f64)) -> Option<usize> {
        let map = screen_map(self.window)?;
        self.session
            .borrow()
            .character_index(screen_point, map)
            .map(|offset| offset as usize)
    }
}

pub struct AppKitIme {
    window: Window,
    session: Rc<RefCell<Session>>,
}

impl AppKitIme {
    pub fn new(window: Window) -> AppKitIme {
        AppKitIme {
            window,
            session: Rc::new(RefCell::new(Session::new())),
        }
    }

    pub fn is_composing(&self) -> bool {
        self.session.borrow().is_composing()
    }

    fn apply(&self, effect: Effect) {
        match effect {
            Effect::Install => self.window.set_text_input_client(Some(Box::new(Client {
                window: self.window,
                session: self.session.clone(),
            }))),
            Effect::Remove => self.window.set_text_input_client(None),
            Effect::Discard => self.window.discard_marked_text(),
            Effect::Invalidate => self.window.invalidate_character_coordinates(),
        }
    }
}

impl ImeBackend for AppKitIme {
    type Error = AppKitImeError;

    fn command(&mut self, command: ImeCommand) -> Result<(), AppKitImeError> {
        let effects =
            self.session
                .borrow_mut()
                .command(command)
                .map_err(|refusal| match refusal {
                    Refusal::Reconversion => {
                        AppKitImeError::Unsupported("reconversion is started by the input method")
                    }
                })?;
        effects.into_iter().for_each(|effect| self.apply(effect));
        if self.session.borrow().has_events() {
            self.window.waker().wake();
        }
        Ok(())
    }

    fn drain(&mut self, out: &mut Vec<ImeEvent>) {
        self.session.borrow_mut().drain(out);
    }
}
