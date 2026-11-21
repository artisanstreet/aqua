fn main() {
    #[cfg(target_os = "linux")]
    linux::main();
    #[cfg(not(target_os = "linux"))]
    eprintln!("atspi: skipped; requires Linux");
}

#[cfg(target_os = "linux")]
mod linux {
    use std::collections::HashMap;
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{channel, Receiver};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use aqua_a11y_atspi::{AtspiAdapter, Connection};
    use aqua_core::{NodeId, Point, RawId, Rect, Role, SemanticNode};
    use aqua_portal::testing::{find_binary, scratch_dir, unavailable, PrivateBus};
    use aqua_protocol::{
        A11yAction, A11yActionData, A11yNode, A11yText, A11yTextRun, AccessTreeUpdate,
    };
    use futures_lite::{future, StreamExt};
    use zbus::message::Message;
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
    use zbus::{MatchRule, MessageStream};

    const CHILD: &str = "AQUA_ATSPI_TEST_CHILD";
    const WAIT: Duration = Duration::from_secs(10);
    const REGISTRY: &str = "org.a11y.atspi.Registry";
    const ROOT: &str = "/org/a11y/atspi/accessible/root";
    const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
    const FRAME: u32 = 23;
    const PUSH_BUTTON: u32 = 43;
    const ENTRY: u32 = 79;
    const LAUNCHER_PATHS: &[&str] = &[
        "/usr/libexec/at-spi-bus-launcher",
        "/usr/lib/at-spi2-core/at-spi-bus-launcher",
    ];

    pub fn main() {
        match std::env::var_os(CHILD) {
            Some(_) => child(),
            None => parent(),
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

    fn launcher_binary() -> Option<std::path::PathBuf> {
        std::env::var_os("AQUA_ATSPI_BUS_LAUNCHER")
            .map(std::path::PathBuf::from)
            .filter(|path| path.is_file())
            .or_else(|| find_binary("at-spi-bus-launcher", LAUNCHER_PATHS))
    }

    fn isolated(command: &mut Command, bus: &PrivateBus, runtime: &std::path::Path) {
        command
            .env("DBUS_SESSION_BUS_ADDRESS", bus.address())
            .env("XDG_RUNTIME_DIR", runtime)
            .env("GSETTINGS_BACKEND", "memory")
            .env_remove("AT_SPI_BUS_ADDRESS")
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY");
    }

    fn parent() {
        if !run_isolated() {
            eprintln!("atspi: FAILED");
            std::process::exit(1);
        }
    }

    fn run_isolated() -> bool {
        let Some(bus) = PrivateBus::start() else {
            return true;
        };
        let Some(binary) = launcher_binary() else {
            let _: Option<()> = unavailable(
                "SKIP-NO-ATSPI: at-spi-bus-launcher was not found (nix-shell nix/linux.nix provides it; AQUA_ATSPI_BUS_LAUNCHER overrides)".into(),
            );
            return true;
        };
        let runtime = scratch_dir("atspi").expect("a private runtime directory");
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
        bus.wait_for_name("org.a11y.Bus", WAIT)
            .expect("at-spi-bus-launcher registers org.a11y.Bus");
        let mut tests = Command::new(std::env::current_exe().expect("the test binary path"));
        tests.args(std::env::args_os().skip(1)).env(CHILD, "1");
        isolated(&mut tests, &bus, &runtime);
        let status = tests.status().expect("the test child runs");
        drop(launcher);
        let _ = std::fs::remove_dir_all(&runtime);
        status.success()
    }

    type Object = (String, OwnedObjectPath);
    type Case = (&'static str, fn(&Client));

    struct Client {
        session: zbus::Connection,
        a11y: zbus::Connection,
    }

    fn bounded<T>(what: &str, work: impl std::future::Future<Output = T>) -> T {
        zbus::block_on(future::or(async { Some(work.await) }, async {
            async_io::Timer::after(WAIT).await;
            None
        }))
        .unwrap_or_else(|| panic!("{what} did not answer within {WAIT:?}"))
    }

    fn object(destination: &str, path: &str) -> Object {
        (
            destination.to_owned(),
            OwnedObjectPath::try_from(path).expect("valid object path"),
        )
    }

    impl Client {
        fn connect() -> Client {
            bounded("connecting to the accessibility bus", async {
                let session = zbus::connection::Builder::session()
                    .expect("a session bus address")
                    .build()
                    .await
                    .expect("the private session bus");
                let reply = session
                    .call_method(
                        Some("org.a11y.Bus"),
                        "/org/a11y/bus",
                        Some("org.a11y.Bus"),
                        "GetAddress",
                        &(),
                    )
                    .await
                    .expect("org.a11y.Bus.GetAddress");
                let address: String = reply.body().deserialize().expect("an address");
                let a11y = zbus::connection::Builder::address(address.as_str())
                    .expect("a valid accessibility bus address")
                    .build()
                    .await
                    .expect("the accessibility bus");
                Client { session, a11y }
            })
        }

        fn call<B, R>(&self, target: &Object, interface: &str, member: &str, body: &B) -> R
        where
            B: serde::Serialize + zbus::zvariant::DynamicType,
            R: for<'d> serde::Deserialize<'d> + zbus::zvariant::Type,
        {
            bounded(
                &format!("{interface}.{member} on {target:?}"),
                self.a11y.call_method(
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

        fn property(&self, target: &Object, interface: &str, name: &str) -> OwnedValue {
            self.call(
                target,
                "org.freedesktop.DBus.Properties",
                "Get",
                &(interface, name),
            )
        }

        fn name(&self, target: &Object) -> String {
            String::try_from(self.property(target, ACCESSIBLE, "Name")).expect("a string name")
        }

        fn role(&self, target: &Object) -> u32 {
            self.call(target, ACCESSIBLE, "GetRole", &())
        }

        fn interfaces(&self, target: &Object) -> Vec<String> {
            self.call(target, ACCESSIBLE, "GetInterfaces", &())
        }

        fn children(&self, target: &Object) -> Vec<Object> {
            self.call(target, ACCESSIBLE, "GetChildren", &())
        }

        fn applications(&self) -> Vec<String> {
            self.children(&object(REGISTRY, ROOT))
                .iter()
                .map(|app| self.name(app))
                .collect()
        }

        fn application(&self) -> Object {
            let name = executable_name();
            self.children(&object(REGISTRY, ROOT))
                .into_iter()
                .find(|app| self.name(app) == name)
                .unwrap_or_else(|| panic!("{name} is registered with the AT-SPI registry"))
        }

        fn window(&self, name: &str) -> Object {
            self.children(&self.application())
                .into_iter()
                .find(|window| self.name(window) == name)
                .unwrap_or_else(|| panic!("the application exposes the window {name:?}"))
        }

        fn descendant(&self, under: &Object, name: &str) -> Object {
            let mut pending = self.children(under);
            while let Some(candidate) = pending.pop() {
                if self.name(&candidate) == name {
                    return candidate;
                }
                pending.extend(self.children(&candidate));
            }
            panic!("no accessible named {name:?} under {under:?}")
        }

        fn enable(&self, enabled: bool) {
            bounded(
                "setting org.a11y.Status.IsEnabled",
                self.session.call_method(
                    Some("org.a11y.Bus"),
                    "/org/a11y/bus",
                    Some("org.freedesktop.DBus.Properties"),
                    "Set",
                    &("org.a11y.Status", "IsEnabled", Value::from(enabled)),
                ),
            )
            .expect("org.a11y.Status.IsEnabled is writable");
        }

        fn listen(&self, interface: &str, member: &str) -> MessageStream {
            let rule = MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .interface(interface.to_owned())
                .and_then(|rule| rule.member(member.to_owned()))
                .expect("a valid match rule")
                .build();
            zbus::block_on(MessageStream::for_match_rule(rule, &self.a11y, None))
                .expect("the accessibility bus accepts the match rule")
        }
    }

    fn executable_name() -> String {
        std::env::current_exe()
            .ok()
            .and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .expect("the test binary has a file name")
    }

    fn wait_for(stream: &mut MessageStream, what: &str, accept: impl Fn(&Message) -> bool) {
        let found = zbus::block_on(future::or(
            async {
                while let Some(Ok(message)) = stream.next().await {
                    if accept(&message) {
                        return true;
                    }
                }
                false
            },
            async {
                async_io::Timer::after(WAIT).await;
                false
            },
        ));
        assert!(found, "a screen reader sees {what} within {WAIT:?}");
    }

    fn signal_kind(message: &Message) -> String {
        message
            .body()
            .deserialize::<(String, i32, i32, OwnedValue, HashMap<String, OwnedValue>)>()
            .map(|(kind, ..)| kind)
            .unwrap_or_default()
    }

    fn signal_path(message: &Message) -> String {
        message
            .header()
            .path()
            .map(|path| path.as_str().to_owned())
            .unwrap_or_default()
    }

    struct Attached {
        adapter: AtspiAdapter,
        wakes: Arc<AtomicUsize>,
        woken: Receiver<()>,
    }

    impl Attached {
        fn new() -> Attached {
            let wakes = Arc::new(AtomicUsize::new(0));
            let (sender, woken) = channel();
            let sender = Mutex::new(sender);
            let counter = wakes.clone();
            let adapter = AtspiAdapter::attach(Arc::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                let _ = sender.lock().unwrap().send(());
            }));
            Attached {
                adapter,
                wakes,
                woken,
            }
        }

        fn woken(&self, what: &str) {
            self.woken
                .recv_timeout(WAIT)
                .unwrap_or_else(|_| panic!("the adapter wakes the loop when {what}"));
        }

        fn wakes(&self) -> usize {
            self.wakes.load(Ordering::SeqCst)
        }

        fn publish(&mut self, client: &Client, update: &AccessTreeUpdate) {
            let mut added = client.listen("org.a11y.atspi.Event.Object", "ChildrenChanged");
            self.adapter.apply(update);
            wait_for(&mut added, "the window being added", |message| {
                signal_path(message) == ROOT && signal_kind(message).starts_with("add")
            });
        }
    }

    fn id(index: u32) -> NodeId {
        NodeId::from_raw(RawId {
            index,
            generation: 1,
        })
    }

    fn bounds(y: f32) -> Rect {
        Rect {
            x: 10.0,
            y,
            w: 160.0,
            h: 24.0,
        }
    }

    fn node(index: u32, role: Role, label: &str, children: Vec<NodeId>) -> A11yNode {
        let mut semantic = SemanticNode::new(id(index), role);
        semantic.label = label.to_owned();
        semantic.children = children.clone();
        semantic.bounds = Some(bounds(index as f32 * 30.0));
        A11yNode {
            text: None,
            semantic,
            bounds: Some(bounds(index as f32 * 30.0)),
            scroll: Point::ZERO,
            children,
        }
    }

    fn window(title: &str, status: &str) -> AccessTreeUpdate {
        AccessTreeUpdate {
            root: id(0),
            focus: id(0),
            nodes: vec![
                node(0, Role::Group, title, vec![id(1), id(2)]),
                node(1, Role::Button, "Save", vec![]),
                node(2, Role::Text, status, vec![]),
            ],
        }
    }

    fn editor(title: &str, text: &str, caret: usize) -> AccessTreeUpdate {
        let mut field = node(1, Role::TextField, "Message", vec![]);
        field.semantic.focusable = true;
        field.text = Some(A11yText {
            runs: vec![A11yTextRun {
                text: text.to_owned(),
                byte_start: 0,
                bounds: bounds(30.0),
                character_lengths: vec![1; text.len()],
                word_starts: vec![0, 6],
                character_positions: (0..text.len()).map(|index| index as f32 * 8.0).collect(),
                character_widths: vec![8.0; text.len()],
                rtl: false,
            }],
            selection: Some((caret, caret)),
            selection_enabled: true,
            links: vec![],
        });
        AccessTreeUpdate {
            root: id(0),
            focus: id(1),
            nodes: vec![node(0, Role::Group, title, vec![id(1)]), field],
        }
    }

    fn the_tree_is_published_only_after_an_assistive_client_connects(client: &Client) {
        let mut attached = Attached::new();
        let update = window("Gate", "Idle");
        attached.adapter.apply(&update);
        assert_eq!(attached.adapter.connection(), Connection::Dormant);
        assert_eq!(attached.adapter.epoch(), 0);
        assert!(!client.applications().contains(&executable_name()));

        client.enable(true);
        attached.woken("an assistive client connects");
        assert_eq!(attached.adapter.connection(), Connection::Connected);
        assert_eq!(attached.adapter.epoch(), 1);
        attached.publish(client, &update);
        assert_eq!(client.role(&client.window("Gate")), FRAME);
        assert_eq!(attached.wakes(), 1);
    }

    fn a_button_is_exposed_with_its_name_and_push_button_role(client: &Client) {
        let mut attached = Attached::new();
        attached.woken("it attaches while a client is connected");
        attached.publish(client, &window("Buttons", "Idle"));
        let button = client.descendant(&client.window("Buttons"), "Save");
        assert_eq!(client.role(&button), PUSH_BUTTON);
        assert!(client
            .interfaces(&button)
            .contains(&"org.a11y.atspi.Action".to_owned()));
        let action: String = client.call(&button, "org.a11y.atspi.Action", "GetName", &(0i32,));
        assert_eq!(action, "click");
        assert_eq!(attached.wakes(), 1);
    }

    fn invoking_the_default_action_presses_the_button(client: &Client) {
        let mut attached = Attached::new();
        attached.woken("it attaches while a client is connected");
        attached.publish(client, &window("Presses", "Idle"));
        let window_object = client.window("Presses");
        let button = client.descendant(&window_object, "Save");
        let invoked: bool = client.call(&button, "org.a11y.atspi.Action", "DoAction", &(0i32,));
        assert!(invoked);
        attached.woken("a screen reader invokes an action");
        let actions = attached.adapter.drain_actions();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].node, id(1));
        assert_eq!(actions[0].action, A11yAction::Press);
        assert_eq!(actions[0].data, A11yActionData::None);

        let mut renamed = client.listen("org.a11y.atspi.Event.Object", "PropertyChange");
        attached.adapter.apply(&window("Presses", "Saved"));
        wait_for(&mut renamed, "the status text change", |message| {
            signal_kind(message) == "accessible-name"
        });
        assert!(client
            .children(&window_object)
            .iter()
            .any(|child| client.name(child) == "Saved"));
        assert_eq!(attached.wakes(), 2);
    }

    fn editable_text_exposes_its_value_and_caret(client: &Client) {
        let mut attached = Attached::new();
        attached.woken("it attaches while a client is connected");
        attached.publish(client, &editor("Compose", "hello world", 5));
        let field = client.descendant(&client.window("Compose"), "Message");
        assert_eq!(client.role(&field), ENTRY);
        let text: String = client.call(&field, "org.a11y.atspi.Text", "GetText", &(0i32, -1i32));
        assert_eq!(text, "hello world");
        let caret =
            i32::try_from(client.property(&field, "org.a11y.atspi.Text", "CaretOffset")).unwrap();
        assert_eq!(caret, 5);
    }

    fn window_focus_changes_raise_focus_events(client: &Client) {
        let mut attached = Attached::new();
        attached.woken("it attaches while a client is connected");
        attached.publish(client, &window("Focus", "Idle"));
        let path = client.window("Focus").1.as_str().to_owned();
        let mut activated = client.listen("org.a11y.atspi.Event.Window", "Activate");
        attached.adapter.focus(true);
        wait_for(&mut activated, "the window activating", |message| {
            signal_path(message) == path
        });
        let mut deactivated = client.listen("org.a11y.atspi.Event.Window", "Deactivate");
        attached.adapter.focus(false);
        wait_for(&mut deactivated, "the window deactivating", |message| {
            signal_path(message) == path
        });
    }

    fn a_returning_screen_reader_gets_the_full_tree_under_a_new_epoch(client: &Client) {
        let mut attached = Attached::new();
        attached.woken("it attaches while a client is connected");
        let update = window("Return", "Idle");
        attached.publish(client, &update);
        let epoch = attached.adapter.epoch();

        client.enable(false);
        attached.woken("the assistive client leaves");
        assert_eq!(attached.adapter.connection(), Connection::Dormant);
        client.enable(true);
        attached.woken("the assistive client returns");
        assert_eq!(attached.adapter.connection(), Connection::Connected);
        assert_eq!(attached.adapter.epoch(), epoch + 1);
        attached.publish(client, &update);
        client.descendant(&client.window("Return"), "Save");
        assert_eq!(attached.wakes(), 3);
    }

    fn child() {
        let client = Client::connect();
        let tests: &[Case] = &[
            (
                "the_tree_is_published_only_after_an_assistive_client_connects",
                the_tree_is_published_only_after_an_assistive_client_connects,
            ),
            (
                "a_button_is_exposed_with_its_name_and_push_button_role",
                a_button_is_exposed_with_its_name_and_push_button_role,
            ),
            (
                "invoking_the_default_action_presses_the_button",
                invoking_the_default_action_presses_the_button,
            ),
            (
                "editable_text_exposes_its_value_and_caret",
                editable_text_exposes_its_value_and_caret,
            ),
            (
                "window_focus_changes_raise_focus_events",
                window_focus_changes_raise_focus_events,
            ),
            (
                "a_returning_screen_reader_gets_the_full_tree_under_a_new_epoch",
                a_returning_screen_reader_gets_the_full_tree_under_a_new_epoch,
            ),
        ];
        for (name, test) in tests {
            test(&client);
            eprintln!("test {name} ... ok");
        }
        eprintln!("atspi: {} passed", tests.len());
    }
}
