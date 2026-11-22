use aqua_portal::testing::PrivateBus;
use aqua_shell_wayland::testing::Compositor;

pub struct Session {
    _compositor: Compositor,
    _bus: PrivateBus,
}

pub fn session(floating_titles: &str) -> Option<Session> {
    let bus = PrivateBus::start()?;
    let compositor = Compositor::start()?;
    compositor
        .command(&format!(
            "for_window [title=\"{floating_titles}\"] floating enable"
        ))
        .expect("sway accepts the floating rule");
    compositor
        .attach()
        .expect("the shell connects to the headless compositor");
    unsafe {
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", bus.address());
        std::env::set_var("XDG_RUNTIME_DIR", compositor.runtime_dir());
        std::env::set_var("WAYLAND_DISPLAY", compositor.display());
        std::env::remove_var("WAYLAND_SOCKET");
    }
    Some(Session {
        _compositor: compositor,
        _bus: bus,
    })
}
