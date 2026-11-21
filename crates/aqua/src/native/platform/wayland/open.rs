use std::path::Path;

pub use aqua_portal::open::{OpenError, Origin};
use aqua_shell_wayland::Window;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenTarget(aqua_portal::open::OpenTarget);

impl OpenTarget {
    pub fn url(input: &str) -> Result<Self, OpenError> {
        aqua_portal::open::OpenTarget::url(input).map(Self)
    }

    pub fn path(path: &Path) -> Result<Self, OpenError> {
        aqua_portal::open::OpenTarget::path(path).map(Self)
    }

    pub fn reveal(path: &Path) -> Result<Self, OpenError> {
        aqua_portal::open::OpenTarget::reveal(path).map(Self)
    }

    pub fn execute(&self) -> Result<(), OpenError> {
        let bus =
            aqua_portal::Bus::session().map_err(|error| OpenError::Desktop(error.to_string()))?;
        self.0.execute_on(&bus, &origin())
    }
}

pub fn origin() -> Origin {
    let Some(window) = Window::engaged() else {
        return Origin::default();
    };
    let parent = window
        .exported_handle()
        .map(|handle| Origin::wayland(&handle))
        .unwrap_or_default();
    match window.activation_token() {
        Ok(token) => parent.with_activation_token(&token),
        Err(_) => parent,
    }
}
