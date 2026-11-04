use std::cell::RefCell;
use windows::core::{implement, Result};
use windows::Win32::UI::TextServices::{ITfEditSession, ITfEditSession_Impl};

type Body = Box<dyn FnOnce(u32) -> Result<()>>;

#[implement(ITfEditSession)]
pub(crate) struct Session {
    body: RefCell<Option<Body>>,
}

impl Session {
    pub(crate) fn spawn(body: impl FnOnce(u32) -> Result<()> + 'static) -> ITfEditSession {
        Session {
            body: RefCell::new(Some(Box::new(body))),
        }
        .into()
    }
}

impl ITfEditSession_Impl for Session_Impl {
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
        match self.body.borrow_mut().take() {
            Some(body) => body(cookie),
            None => Ok(()),
        }
    }
}
