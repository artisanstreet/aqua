fn main() {
    #[cfg(target_os = "linux")]
    wayland::main();
    #[cfg(not(target_os = "linux"))]
    eprintln!("host_wayland: skipped; requires Linux");
}

#[cfg(target_os = "linux")]
#[path = "../../aqua-portal/tests/support/mod.rs"]
mod portal;

#[cfg(target_os = "linux")]
mod wayland {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpStream;
    use std::os::unix::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::{Child, ChildStdout, Command, ExitStatus, Stdio};
    use std::sync::mpsc::{channel, Receiver};
    use std::time::{Duration, Instant};

    use aqua::config::{AquaConfig, Titlebar};
    use aqua::runtime::{AppCtx, Application};
    use aqua::{px, AppId, Aqua, Context, IntoView};
    use aqua_core::InputEvent;
    use aqua_fonts_system::testing::{FixtureFont, FontconfigFixture};
    use aqua_fonts_system::Generic;
    use aqua_portal::testing::{find_binary, scratch_dir, unavailable, PrivateBus};
    use aqua_protocol::{AquaWindowId, Instant as AquaInstant, ObservedWindowState, WindowEvent};
    use aqua_shell_wayland::testing::{
        capture, Compositor, DataControl, Edit, InputMethod, Rect, Selection, VirtualKeyboard,
        VirtualPointer,
    };
    use futures_lite::future;

    use super::portal;
    use serde_json::Value;
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value as Variant};

    const CHILD: &str = "AQUA_HOST_WAYLAND_APP";
    const APP_ID: &str = "org.aqua.HostWayland";
    const RECORDER_ID: &str = "org.aqua.HostWayland.Recorder";
    const APPEARANCE: &str = "org.freedesktop.appearance";
    const TITLE: &str = "aqua host wayland";
    const LIMIT: Duration = Duration::from_secs(20);
    const TICK: Duration = Duration::from_millis(20);
    const TEXT: &str = "text/plain;charset=utf-8";
    const KEY_X: u32 = 45;
    const KEY_V: u32 = 47;
    const KEY_END: u32 = 107;
    const KEY_TAB: u32 = 15;
    const REGISTRY: &str = "org.a11y.atspi.Registry";
    const ROOT: &str = "/org/a11y/atspi/accessible/root";
    const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
    const PUSH_BUTTON: u32 = 43;
    const LAUNCHER_PATHS: &[&str] = &[
        "/usr/libexec/at-spi-bus-launcher",
        "/usr/lib/at-spi2-core/at-spi-bus-launcher",
    ];
    const EXAMPLES: &[(&str, usize)] = &[
        ("showcase", 1),
        ("todo", 1),
        ("editor_shell", 1),
        ("conversation", 1),
        ("multi_window", 2),
    ];

    pub fn main() {
        match std::env::var(CHILD).as_deref() {
            Ok("recorder") => recorder(),
            Ok(_) => child(),
            Err(_) => parent(),
        }
    }

    fn app(cx: &Context) -> impl IntoView + use<> {
        let (read, write) = cx.scope().signal(String::from("draft"));
        let bump = write.clone();
        Aqua::Box()
            .styles([Aqua::Style().column().width(px(400.0)).height(px(200.0))])
            .child(
                Aqua::Button()
                    .styles([Aqua::Style().width(px(160.0)).height(px(40.0))])
                    .on_activate(move || bump.update(|text| text.push('!')))
                    .child(Aqua::Text("Press").build())
                    .build(),
            )
            .child(
                Aqua::Text("")
                    .bind((read, write))
                    .editable(true)
                    .styles([Aqua::Style().width(px(300.0)).height(px(32.0))])
                    .build(),
            )
            .build()
    }

    fn child() {
        let app_id = AppId::new(APP_ID).expect("the test app id is valid");
        let result = Aqua::App()
            .config(
                Aqua::Config().app_id(app_id).window(
                    Aqua::Window()
                        .title(TITLE)
                        .size(px(480.0), px(320.0))
                        .titlebar(Titlebar::Hidden),
                ),
            )
            .run(app);
        if let Err(error) = result {
            eprintln!("host_wayland: the app failed: {error}");
            std::process::exit(1);
        }
    }

    struct Recorder;

    impl Application for Recorder {
        fn on_window_event(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId, event: &WindowEvent) {
            let line = match event {
                WindowEvent::ThemeChanged(_, scheme) => format!("theme {scheme:?}"),
                WindowEvent::Resized(_, size) => format!("resized {}x{}", size.width, size.height),
                WindowEvent::ScaleChanged(_, scale) => format!("scale {scale}"),
                _ => return,
            };
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{line}");
            let _ = out.flush();
        }

        fn on_input(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId, _event: &InputEvent) {}

        fn build_scene(
            &mut self,
            _window: &ObservedWindowState,
            _now: AquaInstant,
        ) -> Option<AquaInstant> {
            None
        }
    }

    fn recorder() {
        let mut config = AquaConfig {
            app_id: Some(AppId::new(RECORDER_ID).expect("the recorder app id is valid")),
            ..AquaConfig::default()
        };
        config.window.title = "aqua host wayland recorder".into();
        let result = Aqua::new(config).and_then(|aqua| aqua.run(Recorder));
        if let Err(error) = result {
            eprintln!("host_wayland: the recorder failed: {error}");
            std::process::exit(1);
        }
    }

    struct Lines(Receiver<String>);

    impl Lines {
        fn follow(stdout: ChildStdout) -> Lines {
            let (sender, lines) = channel();
            std::thread::spawn(move || {
                BufReader::new(stdout)
                    .lines()
                    .map_while(Result::ok)
                    .try_for_each(|line| sender.send(line))
            });
            Lines(lines)
        }

        fn expect(&self, what: &str, accept: impl Fn(&str) -> bool) {
            let deadline = Instant::now() + LIMIT;
            let mut seen = Vec::new();
            while let Some(left) = deadline.checked_duration_since(Instant::now()) {
                match self.0.recv_timeout(left) {
                    Ok(line) if accept(&line) => return,
                    Ok(line) => seen.push(line),
                    Err(_) => break,
                }
            }
            panic!("the app never reported {what}; it reported {seen:?}");
        }
    }

    struct Desktop {
        compositor: Compositor,
        bus: PrivateBus,
        _launcher: Launcher,
        runtime: PathBuf,
    }

    impl Drop for Desktop {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.runtime);
        }
    }

    struct Launcher(Child);

    impl Drop for Launcher {
        fn drop(&mut self) {
            if let Some(group) = rustix::process::Pid::from_raw(self.0.id() as i32) {
                let _ = rustix::process::kill_process_group(group, rustix::process::Signal::TERM);
            }
            let _ = self.0.wait();
        }
    }

    struct App {
        child: Child,
        label: String,
    }

    impl Drop for App {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    impl App {
        fn exit(&mut self) -> ExitStatus {
            within(&format!("{} to exit", self.label), || {
                self.child.try_wait().expect("the app process is waitable")
            })
        }

        fn running(&mut self) -> bool {
            self.child
                .try_wait()
                .expect("the app process is waitable")
                .is_none()
        }
    }

    fn within<T>(what: &str, mut probe: impl FnMut() -> Option<T>) -> T {
        let deadline = Instant::now() + LIMIT;
        loop {
            if let Some(value) = probe() {
                return value;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(TICK);
        }
    }

    fn isolated(command: &mut Command, bus: &PrivateBus, runtime: &Path) {
        command
            .env("DBUS_SESSION_BUS_ADDRESS", bus.address())
            .env("XDG_RUNTIME_DIR", runtime)
            .env("GSETTINGS_BACKEND", "memory")
            .env_remove("AT_SPI_BUS_ADDRESS")
            .env_remove("AQUA_FONT_DIRS")
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("SWAYSOCK");
    }

    impl Desktop {
        fn start() -> Option<Desktop> {
            let bus = PrivateBus::start()?;
            let binary = std::env::var_os("AQUA_ATSPI_BUS_LAUNCHER")
                .map(PathBuf::from)
                .filter(|path| path.is_file())
                .or_else(|| find_binary("at-spi-bus-launcher", LAUNCHER_PATHS));
            let Some(binary) = binary else {
                return unavailable(
                    "SKIP-NO-ATSPI: at-spi-bus-launcher was not found (nix-shell nix/linux.nix provides it; AQUA_ATSPI_BUS_LAUNCHER overrides)".into(),
                );
            };
            let compositor = Compositor::start()?;
            let runtime = scratch_dir("host-wayland").expect("a private runtime directory");
            let mut command = Command::new(&binary);
            command
                .arg("--launch-immediately")
                .env_remove("LD_LIBRARY_PATH")
                .stdin(Stdio::null())
                .process_group(0);
            isolated(&mut command, &bus, &runtime);
            let launcher = Launcher(
                command
                    .spawn()
                    .unwrap_or_else(|error| panic!("{} did not start: {error}", binary.display())),
            );
            bus.wait_for_name("org.a11y.Bus", LIMIT)
                .expect("at-spi-bus-launcher registers org.a11y.Bus");
            Some(Desktop {
                compositor,
                bus,
                _launcher: launcher,
                runtime,
            })
        }

        fn command(&self, program: &Path, env: &[(&str, &str)]) -> Command {
            let mut command = Command::new(program);
            isolated(&mut command, &self.bus, self.compositor.runtime_dir());
            command
                .env("WAYLAND_DISPLAY", self.compositor.display())
                .envs(env.iter().copied())
                .stdin(Stdio::null());
            command
        }

        fn spawn(&self, program: &Path, label: &str, env: &[(&str, &str)]) -> App {
            start(self.command(program, env), label)
        }

        fn spawn_test_app(&self, env: &[(&str, &str)]) -> App {
            let program = std::env::current_exe().expect("the test binary path");
            let env: Vec<(&str, &str)> = [(CHILD, "field")]
                .into_iter()
                .chain(env.iter().copied())
                .collect();
            self.spawn(&program, "the test app", &env)
        }

        fn spawn_recorder(&self) -> (App, Lines) {
            let program = std::env::current_exe().expect("the test binary path");
            let mut command = self.command(&program, &[(CHILD, "recorder")]);
            command.stdout(Stdio::piped());
            let mut app = start(command, "the recorder");
            let stdout = app
                .child
                .stdout
                .take()
                .expect("the recorder's stdout is piped");
            (app, Lines::follow(stdout))
        }

        fn views(&self, app_id: &str) -> Vec<(i64, Rect)> {
            let mut found = Vec::new();
            let mut pending = vec![self.compositor.tree().expect("sway answers get_tree")];
            while let Some(node) = pending.pop() {
                if node["app_id"] == app_id
                    && let Some(rect) = rect_of(&node["rect"])
                    && rect.width > 0
                    && rect.height > 0
                {
                    found.push((node["id"].as_i64().unwrap_or_default(), rect));
                }
                for key in ["nodes", "floating_nodes"] {
                    pending.extend(node[key].as_array().into_iter().flatten().cloned());
                }
            }
            found.sort_by_key(|(id, _)| *id);
            found
        }

        fn mapped(&self, app_id: &str, count: usize) -> Vec<(i64, Rect)> {
            within(&format!("{count} mapped windows of {app_id}"), || {
                let views = self.views(app_id);
                (views.len() == count).then_some(views)
            })
        }

        fn rendered(&self, rect: Rect) {
            within("the window to show rendered content", || {
                let shot = capture(&self.compositor, false).expect("sway captures the output");
                let pixel = |x: i32, y: i32| shot.bgra_at(x as u32, y as u32);
                let first = pixel(rect.x, rect.y);
                let rows = (rect.y..rect.y + rect.height).step_by(2);
                rows.flat_map(|y| {
                    (rect.x..rect.x + rect.width)
                        .step_by(2)
                        .map(move |x| (x, y))
                })
                .any(|(x, y)| pixel(x, y) != first)
                .then_some(())
            });
        }

        fn kill(&self, criteria: &str) {
            self.compositor
                .command(&format!("{criteria} kill"))
                .expect("sway closes the window");
        }
    }

    fn start(mut command: Command, label: &str) -> App {
        App {
            child: command
                .spawn()
                .unwrap_or_else(|error| panic!("{label} did not start: {error}")),
            label: label.to_owned(),
        }
    }

    fn rect_of(value: &Value) -> Option<Rect> {
        Some(Rect {
            x: value["x"].as_i64()? as i32,
            y: value["y"].as_i64()? as i32,
            width: value["width"].as_i64()? as i32,
            height: value["height"].as_i64()? as i32,
        })
    }

    fn free_port() -> u16 {
        std::net::TcpListener::bind(("127.0.0.1", 0))
            .expect("a loopback port")
            .local_addr()
            .expect("the bound address")
            .port()
    }

    struct Inspector(u16);

    impl Inspector {
        fn ask(&self, command: &str) -> Option<Value> {
            let mut stream = TcpStream::connect(("127.0.0.1", self.0)).ok()?;
            stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
            writeln!(stream, "{command}").ok()?;
            let mut line = String::new();
            BufReader::new(stream).read_line(&mut line).ok()?;
            serde_json::from_str(&line).ok()
        }

        fn field(&self, role: &str, field: &str) -> Option<Value> {
            fn find<'a>(node: &'a Value, role: &str) -> Option<&'a Value> {
                if node["role"] == role {
                    return Some(node);
                }
                node["children"]
                    .as_array()?
                    .iter()
                    .find_map(|child| find(child, role))
            }
            let reply = self.ask("tree")?;
            find(&reply["tree"], role).map(|node| node[field].clone())
        }

        fn value(&self) -> Option<String> {
            self.field("TextInput", "value")
                .and_then(|value| value.as_str().map(str::to_owned))
        }

        fn await_value(&self, what: &str, expected: &str) {
            let mut last = None;
            let found = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                within(what, || {
                    last = self.value();
                    (last.as_deref() == Some(expected)).then_some(())
                })
            }));
            assert!(
                found.is_ok(),
                "{what}: the field never read {expected:?}; last value {last:?}"
            );
        }
    }

    type Object = (String, OwnedObjectPath);

    struct Accessibility {
        session: zbus::Connection,
        a11y: Option<zbus::Connection>,
    }

    fn bounded<T>(what: &str, work: impl std::future::Future<Output = T>) -> T {
        zbus::block_on(future::or(async { Some(work.await) }, async {
            async_io::Timer::after(LIMIT).await;
            None
        }))
        .unwrap_or_else(|| panic!("{what} did not answer within {LIMIT:?}"))
    }

    fn object(destination: &str, path: &str) -> Object {
        (
            destination.to_owned(),
            OwnedObjectPath::try_from(path).expect("a valid object path"),
        )
    }

    impl Accessibility {
        fn connect(bus: &PrivateBus) -> Accessibility {
            let session = bounded("connecting to the private session bus", async {
                zbus::connection::Builder::address(bus.address())
                    .expect("a valid bus address")
                    .build()
                    .await
                    .expect("the private session bus")
            });
            bounded(
                "enabling accessibility",
                session.call_method(
                    Some("org.a11y.Bus"),
                    "/org/a11y/bus",
                    Some("org.freedesktop.DBus.Properties"),
                    "Set",
                    &("org.a11y.Status", "IsEnabled", Variant::from(true)),
                ),
            )
            .expect("org.a11y.Status.IsEnabled is writable");
            Accessibility {
                session,
                a11y: None,
            }
        }

        fn bus(&mut self) -> &zbus::Connection {
            if self.a11y.is_none() {
                let reply = bounded(
                    "org.a11y.Bus.GetAddress",
                    self.session.call_method(
                        Some("org.a11y.Bus"),
                        "/org/a11y/bus",
                        Some("org.a11y.Bus"),
                        "GetAddress",
                        &(),
                    ),
                )
                .expect("org.a11y.Bus answers GetAddress");
                let address: String = reply.body().deserialize().expect("an address");
                self.a11y = Some(bounded("connecting to the accessibility bus", async {
                    zbus::connection::Builder::address(address.as_str())
                        .expect("a valid accessibility bus address")
                        .build()
                        .await
                        .expect("the accessibility bus")
                }));
            }
            self.a11y
                .as_ref()
                .expect("the accessibility bus is connected")
        }

        fn call<B, R>(&mut self, target: &Object, interface: &str, member: &str, body: &B) -> R
        where
            B: serde::Serialize + zbus::zvariant::DynamicType,
            R: for<'d> serde::Deserialize<'d> + zbus::zvariant::Type,
        {
            let bus = self.bus().clone();
            bounded(
                &format!("{interface}.{member} on {target:?}"),
                bus.call_method(
                    Some(target.0.as_str()),
                    target.1.as_str(),
                    Some(interface),
                    member,
                    body,
                ),
            )
            .unwrap_or_else(|error| panic!("{interface}.{member} on {target:?}: {error}"))
            .body()
            .deserialize()
            .unwrap_or_else(|error| panic!("{interface}.{member} reply: {error}"))
        }

        fn name(&mut self, target: &Object) -> String {
            let value: OwnedValue = self.call(
                target,
                "org.freedesktop.DBus.Properties",
                "Get",
                &(ACCESSIBLE, "Name"),
            );
            String::try_from(value).expect("a string name")
        }

        fn children(&mut self, target: &Object) -> Vec<Object> {
            self.call(target, ACCESSIBLE, "GetChildren", &())
        }

        fn window(&mut self, title: &str) -> Option<Object> {
            let applications = self.children(&object(REGISTRY, ROOT));
            let windows: Vec<Object> = applications
                .iter()
                .flat_map(|application| self.children(application))
                .collect();
            windows
                .into_iter()
                .find(|window| self.name(window) == title)
        }

        fn descendant(&mut self, under: &Object, name: &str) -> Option<Object> {
            let mut pending = self.children(under);
            while let Some(candidate) = pending.pop() {
                if self.name(&candidate) == name {
                    return Some(candidate);
                }
                pending.extend(self.children(&candidate));
            }
            None
        }
    }

    struct Clipboard(DataControl);

    impl Clipboard {
        fn text(&self, which: Selection) -> Option<String> {
            let offered = self.0.offered(which)?;
            if !offered.iter().any(|mime| mime == TEXT) {
                return None;
            }
            let bytes = self
                .0
                .read(which, TEXT)
                .recv_timeout(LIMIT)
                .expect("the data control client answers")
                .expect("the selection is readable")?;
            String::from_utf8(bytes).ok()
        }
    }

    fn chord(keyboard: &mut VirtualKeyboard, key: u32) {
        keyboard
            .press(VirtualKeyboard::KEY_LEFTCTRL)
            .expect("ctrl goes down");
        keyboard.tap(key).expect("the key is typed");
        keyboard
            .release(VirtualKeyboard::KEY_LEFTCTRL)
            .expect("ctrl comes up");
    }

    fn a_real_app_types_composes_copies_pastes_and_is_read_by_a_screen_reader(desktop: &Desktop) {
        let mut accessibility = Accessibility::connect(&desktop.bus);
        let mut method =
            InputMethod::new(&desktop.compositor).expect("the fake input method binds");
        let mut keyboard = VirtualKeyboard::with_layout(&desktop.compositor, "us", "")
            .expect("the virtual keyboard binds");
        let pointer = VirtualPointer::new(&desktop.compositor).expect("the virtual pointer binds");
        let clipboard =
            Clipboard(DataControl::connect(&desktop.compositor).expect("data control binds"));
        let port = free_port().to_string();
        let mut app = desktop.spawn_test_app(&[("AQUA_INSPECTOR", port.as_str())]);
        let inspector = Inspector(port.parse().expect("a port number"));

        let (_, rect) = desktop.mapped(APP_ID, 1)[0];
        inspector.await_value("the first tree", "draft");
        desktop.rendered(rect);

        let window = within("the window on the accessibility bus", || {
            accessibility.window(TITLE)
        });
        let button = within("the Press button on the accessibility bus", || {
            accessibility.descendant(&window, "Press")
        });
        let role: u32 = accessibility.call(&button, ACCESSIBLE, "GetRole", &());
        assert_eq!(role, PUSH_BUTTON, "Press is exposed as a push button");
        let pressed: bool = accessibility.call(&button, "org.a11y.atspi.Action", "DoAction", &0i32);
        assert!(pressed, "the screen reader's default action is accepted");
        inspector.await_value("the screen reader's press", "draft!");

        pointer
            .move_to((rect.x + 100) as u32, (rect.y + 56) as u32)
            .expect("the pointer moves over the field");
        pointer
            .press(VirtualPointer::LEFT)
            .and_then(|()| pointer.release(VirtualPointer::LEFT))
            .expect("the field is clicked");
        within("the field to take focus", || {
            inspector
                .field("TextInput", "focused")
                .filter(|focused| focused == true)
        });
        keyboard.tap(KEY_END).expect("End moves the caret");
        keyboard.tap(KEY_X).expect("x is typed");
        inspector.await_value("typed text", "draft!x");

        within("the input method to activate", || {
            method
                .poll()
                .expect("the input method polls")
                .active
                .then_some(())
        });
        method
            .send(Edit {
                preedit: Some(("にほん", 0, "にほん".len() as i32)),
                ..Edit::default()
            })
            .expect("the preedit is sent");
        method
            .send(Edit {
                commit: Some("日本"),
                ..Edit::default()
            })
            .expect("the commit is sent");
        inspector.await_value("the committed composition", "draft!x日本");

        chord(&mut keyboard, VirtualKeyboard::KEY_A);
        chord(&mut keyboard, VirtualKeyboard::KEY_C);
        within("the copy to reach the clipboard", || {
            clipboard
                .text(Selection::Clipboard)
                .filter(|text| text == "draft!x日本")
        });

        clipboard.0.offer(
            Selection::Clipboard,
            vec![(TEXT.to_owned(), " pasted".as_bytes().to_vec())],
        );
        within("the other client's selection to be offered", || {
            clipboard
                .text(Selection::Clipboard)
                .filter(|text| text == " pasted")
        });
        keyboard.tap(KEY_END).expect("End collapses the selection");
        chord(&mut keyboard, KEY_V);
        inspector.await_value("the paste", "draft!x日本 pasted");

        desktop.kill(&format!("[app_id=\"^{APP_ID}$\"]"));
        assert!(
            app.exit().success(),
            "the app quits cleanly when its window closes"
        );
    }

    fn system_theme_resize_and_scale_changes_reach_a_running_app(desktop: &Desktop) {
        let (settings, _) = portal::desktop(
            &desktop.bus,
            portal::Desktop {
                settings: Some(vec![(
                    APPEARANCE,
                    "color-scheme",
                    portal::Setting::Unsigned(2),
                )]),
                ..portal::Desktop::default()
            },
        );
        let recorder = format!("[app_id=\"^{RECORDER_ID}$\"]");
        desktop
            .compositor
            .command(&format!("for_window {recorder} floating enable"))
            .expect("sway accepts the floating rule");
        let (mut app, lines) = desktop.spawn_recorder();
        desktop.mapped(RECORDER_ID, 1);
        settings.emit(
            portal::PORTAL_PATH,
            "org.freedesktop.portal.Settings",
            "SettingChanged",
            &(
                APPEARANCE,
                "color-scheme",
                portal::Setting::Unsigned(1).value(),
            ),
        );
        lines.expect("the dark system theme", |line| line == "theme Dark");
        desktop
            .compositor
            .command(&format!("{recorder} resize set 500 300"))
            .expect("sway resizes the floating window");
        lines.expect("the logical resize", |line| line == "resized 500x300");
        desktop
            .compositor
            .command("output HEADLESS-1 scale 2")
            .expect("sway rescales the output");
        lines.expect("the new scale", |line| line == "scale 2");
        lines.expect("the unchanged logical size", |line| {
            line == "resized 500x300"
        });
        desktop
            .compositor
            .command("output HEADLESS-1 scale 1")
            .expect("sway restores the output scale");
        desktop.kill(&recorder);
        assert!(
            app.exit().success(),
            "the recorder quits cleanly when its window closes"
        );
    }

    fn smoke_mode_exits_after_the_requested_presented_frames(desktop: &Desktop) {
        let mut frames = desktop.spawn_test_app(&[("AQUA_SMOKE_FRAMES", "1")]);
        assert!(
            frames.exit().success(),
            "a frame budget ends the app cleanly"
        );
        let mut timed = desktop.spawn_test_app(&[("AQUA_SMOKE_MS", "400")]);
        assert!(timed.exit().success(), "a time budget ends the app cleanly");
        within("the smoke windows to unmap", || {
            desktop.views(APP_ID).is_empty().then_some(())
        });
    }

    fn example(name: &str) -> PathBuf {
        let binary = std::env::current_exe().expect("the test binary path");
        binary
            .ancestors()
            .map(|directory| directory.join("examples").join(name))
            .find(|program| program.is_file())
            .unwrap_or_else(|| panic!("the {name} example is built next to {}", binary.display()))
    }

    fn button_pixels(desktop: &Desktop, window: Rect) -> Vec<[u8; 4]> {
        let shot = capture(&desktop.compositor, false).expect("sway captures the output");
        (window.y..window.y + 40)
            .flat_map(|y| (window.x..window.x + 160).map(move |x| (x, y)))
            .map(|(x, y)| {
                shot.bgra_at(x as u32, y as u32)
                    .expect("the button lies inside the output")
            })
            .collect()
    }

    fn text_renders_with_fonts_found_through_fontconfig_alone(desktop: &Desktop) {
        let fixture = FontconfigFixture::new("host-fonts")
            .expect("a scratch directory")
            .prefer(Generic::SansSerif, &["Fixture Sans"]);
        fixture
            .install("nested/sans.ttf", &FixtureFont::new("Fixture Sans").ascii())
            .expect("the fixture font is written");
        let config = fixture.write_config().expect("the fontconfig file");
        let config = config.to_string_lossy();
        let elsewhere = fixture.root().join("nothing-here");
        std::fs::create_dir_all(&elsewhere).expect("an empty data directory");
        let elsewhere = elsewhere.to_string_lossy();
        let mut app = desktop.spawn_test_app(&[
            ("FONTCONFIG_FILE", config.as_ref()),
            ("HOME", elsewhere.as_ref()),
            ("XDG_DATA_HOME", elsewhere.as_ref()),
            ("XDG_DATA_DIRS", elsewhere.as_ref()),
        ]);
        let (_, rect) = desktop.mapped(APP_ID, 1)[0];
        let inked = within("the Press label to paint the fixture font", || {
            let pixels = button_pixels(desktop, rect);
            let ground = pixels[0];
            let inked = pixels.iter().filter(|pixel| **pixel != ground).count();
            (inked > 150).then_some(inked)
        });
        assert!(
            inked < 160 * 40 / 2,
            "only the label is inked: {inked} pixels"
        );
        desktop.kill(&format!("[app_id=\"^{APP_ID}$\"]"));
        assert!(
            app.exit().success(),
            "the test app quits when its window closes"
        );
    }

    fn every_example_renders_takes_input_and_quits_cleanly(desktop: &Desktop) {
        let pointer = VirtualPointer::new(&desktop.compositor).expect("the virtual pointer binds");
        let mut keyboard = VirtualKeyboard::with_layout(&desktop.compositor, "us", "")
            .expect("the virtual keyboard binds");
        for (name, windows) in EXAMPLES {
            let program = example(name);
            let mut app = desktop.spawn(&program, name, &[]);
            let views = desktop.mapped(name, *windows);
            views.iter().for_each(|(_, rect)| desktop.rendered(*rect));
            let (_, first) = views[0];
            pointer
                .move_to(
                    (first.x + first.width / 2) as u32,
                    (first.y + first.height / 2) as u32,
                )
                .expect("the pointer moves over the window");
            pointer.wheel(true, 1).expect("the wheel scrolls");
            keyboard.tap(KEY_TAB).expect("Tab is typed");
            desktop.rendered(first);
            assert!(app.running(), "{name} survives keyboard and pointer input");
            if let [(_, kept), .., (closed, _)] = views.as_slice() {
                desktop.kill(&format!("[con_id={closed}]"));
                desktop.mapped(name, windows - 1);
                assert!(
                    app.running(),
                    "{name} keeps running after one of its windows closes"
                );
                desktop.rendered(*kept);
            }
            desktop.kill(&format!("[app_id=\"^{name}$\"]"));
            assert!(
                app.exit().success(),
                "{name} quits cleanly when its windows close"
            );
        }
    }

    type Case = (&'static str, fn(&Desktop));

    pub fn parent() {
        let Some(desktop) = Desktop::start() else {
            return;
        };
        let cases: &[Case] = &[
            (
                "a_real_app_types_composes_copies_pastes_and_is_read_by_a_screen_reader",
                a_real_app_types_composes_copies_pastes_and_is_read_by_a_screen_reader,
            ),
            (
                "system_theme_resize_and_scale_changes_reach_a_running_app",
                system_theme_resize_and_scale_changes_reach_a_running_app,
            ),
            (
                "smoke_mode_exits_after_the_requested_presented_frames",
                smoke_mode_exits_after_the_requested_presented_frames,
            ),
            (
                "text_renders_with_fonts_found_through_fontconfig_alone",
                text_renders_with_fonts_found_through_fontconfig_alone,
            ),
            (
                "every_example_renders_takes_input_and_quits_cleanly",
                every_example_renders_takes_input_and_quits_cleanly,
            ),
        ];
        let filter = std::env::args().skip(1).find(|arg| !arg.starts_with('-'));
        let mut failed = Vec::new();
        for (name, case) in cases {
            if filter
                .as_deref()
                .is_some_and(|filter| !name.contains(filter))
            {
                continue;
            }
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| case(&desktop)));
            match outcome {
                Ok(()) => eprintln!("test {name} ... ok"),
                Err(_) => {
                    eprintln!("test {name} ... FAILED");
                    failed.push(*name);
                }
            }
        }
        drop(desktop);
        if !failed.is_empty() {
            eprintln!("host_wayland: FAILED: {failed:?}");
            std::process::exit(1);
        }
    }
}
