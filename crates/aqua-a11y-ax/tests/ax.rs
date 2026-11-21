fn main() {
    #[cfg(target_os = "macos")]
    macos::main();
    #[cfg(not(target_os = "macos"))]
    println!("ax: skipped; requires macOS");
}

#[cfg(target_os = "macos")]
mod macos {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Child, ChildStdin, Command, Stdio};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{channel, Receiver};
    use std::sync::Arc;
    use std::time::Duration;

    use aqua_a11y_ax::testing::{self as ax, Element, Frame, Observer};
    use aqua_a11y_ax::{AxAdapter, Connection};
    use aqua_core::{NodeId, Point, RawId, Rect, Role, SemanticNode};
    use aqua_protocol::{
        A11yAction, A11yActionData, A11yActionRequest, A11yNode, A11yText, A11yTextRun,
        AccessTreeUpdate, Instant,
    };
    use aqua_shell_appkit::testing::cases::{case, Case};
    use aqua_shell_appkit::testing::{self as shell, Facility};
    use aqua_shell_appkit::{now, quit, run, schedule, Handler, ShellEvent, Window, WindowOptions};

    const APP: &str = "AQUA_AX_APP";
    const WAIT: Duration = Duration::from_secs(10);
    const TITLE: &str = "aqua ax";
    const SIZE: (f64, f64) = (360.0, 240.0);
    const BUTTON: (f64, f64, f64, f64) = (20.0, 30.0, 120.0, 24.0);
    const STATUS: (f64, f64, f64, f64) = (20.0, 70.0, 200.0, 20.0);
    const FIELD: (f64, f64, f64, f64) = (20.0, 110.0, 240.0, 28.0);
    const ROOT: u32 = 0;
    const SAVE: u32 = 1;
    const MESSAGE: u32 = 3;

    pub fn main() {
        match std::env::var_os(APP) {
            Some(_) => app(),
            None => aqua_shell_appkit::testing::cases::main(&ALL),
        }
    }

    const ALL: [Case; 5] = [
        case(
            "the_tree_appears_only_after_an_assistive_query",
            appears_on_query,
        ),
        case(
            "a_button_is_an_ax_button_whose_press_arrives_as_press",
            pressing,
        ),
        case(
            "a_text_field_exposes_its_value_and_selected_range_and_takes_a_new_value",
            editing,
        ),
        case("focus_moves_the_focused_ui_element", focusing),
        case(
            "element_frames_lie_inside_the_window_on_screen_and_hit_test_back",
            framing,
        ),
    ];

    fn report(line: &str) {
        let mut out = std::io::stdout().lock();
        writeln!(out, "{line}").expect("the app reports to its client");
        out.flush().expect("the app reports to its client");
    }

    fn id(index: u32) -> NodeId {
        NodeId::from_raw(RawId {
            index,
            generation: 1,
        })
    }

    fn physical(scale: f64, (x, y, w, h): (f64, f64, f64, f64)) -> Rect {
        Rect {
            x: (x * scale) as f32,
            y: (y * scale) as f32,
            w: (w * scale) as f32,
            h: (h * scale) as f32,
        }
    }

    fn node(index: u32, role: Role, label: &str, bounds: Rect, children: Vec<NodeId>) -> A11yNode {
        let mut semantic = SemanticNode::new(id(index), role);
        semantic.label = label.to_owned();
        semantic.children = children.clone();
        semantic.bounds = Some(bounds);
        A11yNode {
            text: None,
            semantic,
            bounds: Some(bounds),
            scroll: Point::ZERO,
            children,
        }
    }

    #[derive(Clone, PartialEq)]
    struct Model {
        status: String,
        text: String,
        caret: usize,
        focus: u32,
    }

    impl Model {
        fn new() -> Model {
            Model {
                status: "Idle".into(),
                text: "hello world".into(),
                caret: 5,
                focus: ROOT,
            }
        }

        fn update(&self, scale: f64) -> AccessTreeUpdate {
            let field_bounds = physical(scale, FIELD);
            let mut field = node(MESSAGE, Role::TextField, "Message", field_bounds, vec![]);
            field.semantic.focusable = true;
            let advance = (8.0 * scale) as f32;
            field.text = Some(A11yText {
                runs: vec![A11yTextRun {
                    text: self.text.clone(),
                    byte_start: 0,
                    bounds: field_bounds,
                    character_lengths: vec![1; self.text.len()],
                    word_starts: vec![0],
                    character_positions: (0..self.text.len())
                        .map(|index| index as f32 * advance)
                        .collect(),
                    character_widths: vec![advance; self.text.len()],
                    rtl: false,
                }],
                selection: Some((self.caret, self.caret)),
                selection_enabled: true,
                links: vec![],
            });
            let mut save = node(SAVE, Role::Button, "Save", physical(scale, BUTTON), vec![]);
            save.semantic.focusable = true;
            AccessTreeUpdate {
                root: id(ROOT),
                focus: id(self.focus),
                nodes: vec![
                    node(
                        ROOT,
                        Role::Group,
                        "",
                        physical(scale, (0.0, 0.0, SIZE.0, SIZE.1)),
                        vec![id(SAVE), id(2), id(MESSAGE)],
                    ),
                    save,
                    node(2, Role::Text, &self.status, physical(scale, STATUS), vec![]),
                    field,
                ],
            }
        }

        fn act(&mut self, request: &A11yActionRequest) {
            match (&request.action, &request.data) {
                (A11yAction::Press, _) if request.node == id(SAVE) => self.status = "Saved".into(),
                (A11yAction::SetValue, A11yActionData::Value(text))
                    if request.node == id(MESSAGE) =>
                {
                    self.text = text.clone();
                    self.caret = text.len();
                }
                (A11yAction::Focus, _) => {
                    self.focus = request.node.raw().index;
                }
                other => panic!("the probe app got an action it does not expect: {other:?}"),
            }
        }
    }

    struct App {
        window: Window,
        adapter: AxAdapter,
        model: Model,
        applied: Option<Model>,
        commands: Receiver<String>,
        wakes: Arc<AtomicUsize>,
        connected: bool,
    }

    impl App {
        fn wakes(&self) -> usize {
            self.wakes.load(Ordering::SeqCst)
        }

        fn command(&mut self, line: &str) {
            let words: Vec<&str> = line.split_whitespace().collect();
            match words.as_slice() {
                ["focus", node] => {
                    self.model.focus = node.parse().expect("focus takes a node index");
                }
                ["probe"] => report(&format!(
                    "probe epoch={} wakes={}",
                    self.adapter.epoch(),
                    self.wakes()
                )),
                ["quit"] => quit(),
                other => panic!("the probe app got an unknown command {other:?}"),
            }
        }

        fn turn(&mut self) {
            while let Ok(line) = self.commands.try_recv() {
                self.command(&line);
            }
            for request in self.adapter.drain_actions() {
                report(&format!(
                    "action {:?} node={} data={:?} wakes={}",
                    request.action,
                    request.node.raw().index,
                    request.data,
                    self.wakes()
                ));
                self.model.act(&request);
            }
            if !self.connected && self.adapter.connection() == Connection::Connected {
                self.connected = true;
                report(&format!(
                    "connected epoch={} wakes={}",
                    self.adapter.epoch(),
                    self.wakes()
                ));
            }
            if self.applied.as_ref() == Some(&self.model) {
                return;
            }
            self.adapter.set_window_title(&self.window.title());
            self.adapter.apply(&self.model.update(self.window.scale()));
            match self.connected {
                true => {
                    self.applied = Some(self.model.clone());
                    report(&format!(
                        "applied status={} text={} focus={}",
                        self.model.status, self.model.text, self.model.focus
                    ));
                }
                false => report(&format!(
                    "dormant epoch={} wakes={}",
                    self.adapter.epoch(),
                    self.wakes()
                )),
            }
        }
    }

    impl Handler for App {
        fn window_event(&mut self, id: aqua_protocol::WindowId, event: ShellEvent) {
            if id != self.window.id() {
                return;
            }
            if let ShellEvent::Focused(focused) = event {
                self.adapter.focus(focused);
                report(&format!("focused {focused}"));
            }
        }

        fn woken(&mut self) {
            self.turn();
        }

        fn tick(&mut self, _now: Instant) -> Option<Instant> {
            self.turn();
            None
        }
    }

    fn app() {
        let window = Window::create(&WindowOptions {
            title: TITLE.into(),
            logical_size: SIZE,
            ..WindowOptions::default()
        })
        .expect("the probe window opens");
        shell::activate();
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = wakes.clone();
        let waker = window.waker();
        let adapter = AxAdapter::attach(
            window,
            Arc::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                waker.wake();
            }),
        );
        let (sender, commands) = channel();
        let waker = window.waker();
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let line = line.expect("the client writes UTF-8 commands");
                if sender.send(line).is_err() {
                    return;
                }
                waker.wake();
            }
            if sender.send("quit".into()).is_ok() {
                waker.wake();
            }
        });
        schedule(now());
        run(App {
            window,
            adapter,
            model: Model::new(),
            applied: None,
            commands,
            wakes,
            connected: false,
        })
        .expect("the probe app runs until its client quits it");
    }

    struct Session {
        child: Child,
        stdin: ChildStdin,
        reports: Receiver<String>,
        seen: Vec<String>,
        pending: Vec<String>,
        app: Element,
    }

    impl Session {
        fn start() -> Option<Session> {
            if !shell::available(Facility::WindowServer) || !ax::available() {
                return None;
            }
            let exe = std::env::current_exe().expect("the test binary knows its path");
            let mut child = Command::new(exe)
                .env(APP, "1")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .expect("the probe app starts");
            let stdin = child.stdin.take().expect("stdin is piped");
            let stdout = child.stdout.take().expect("stdout is piped");
            let (sender, reports) = channel();
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { return };
                    if sender.send(line).is_err() {
                        return;
                    }
                }
            });
            let app = Element::application(child.id());
            let mut session = Session {
                child,
                stdin,
                reports,
                seen: Vec::new(),
                pending: Vec::new(),
                app,
            };
            let ready = session.expect("dormant");
            assert_eq!(ready, "dormant epoch=0 wakes=0", "the app attaches dormant");
            Some(session)
        }

        fn expect(&mut self, prefix: &str) -> String {
            if let Some(index) = self
                .pending
                .iter()
                .position(|line| line.starts_with(prefix))
            {
                return self.pending.remove(index);
            }
            loop {
                match self.reports.recv_timeout(WAIT) {
                    Ok(line) => {
                        self.seen.push(line.clone());
                        if line.starts_with(prefix) {
                            return line;
                        }
                        self.pending.push(line);
                    }
                    Err(_) => panic!(
                        "the app did not report {prefix:?} within {WAIT:?}; it reported {:#?}",
                        self.seen
                    ),
                }
            }
        }

        fn send(&mut self, command: &str) {
            writeln!(self.stdin, "{command}").expect("the app reads commands");
            self.stdin.flush().expect("the app reads commands");
        }

        fn window(&self) -> Element {
            self.app
                .windows()
                .into_iter()
                .find(|window| window.title().as_deref() == Some(TITLE))
                .unwrap_or_else(|| panic!("the app has a window titled {TITLE:?}"))
        }

        fn connect(&mut self) -> Element {
            let window = self.window();
            window.children();
            let connected = self.expect("connected");
            assert_eq!(
                connected, "connected epoch=1 wakes=1",
                "the first assistive query connects once and wakes the loop once"
            );
            self.expect("applied");
            self.root(&window)
        }

        fn root(&self, window: &Element) -> Element {
            window
                .children()
                .into_iter()
                .find(|child| child.role().as_deref() == Some("AXGroup"))
                .unwrap_or_else(|| {
                    panic!(
                        "the window exposes the app's root group: {:?}",
                        window.children()
                    )
                })
        }

        fn wakes(&mut self) -> String {
            self.send("probe");
            self.expect("probe")
        }
    }

    impl Drop for Session {
        fn drop(&mut self) {
            match std::thread::panicking() {
                true => {
                    if let Err(error) = self.child.kill() {
                        eprintln!("the probe app could not be killed: {error}");
                    }
                }
                false => self.send("quit"),
            }
            let status = self.child.wait().expect("the probe app exits");
            if !std::thread::panicking() {
                assert!(status.success(), "the probe app exits cleanly: {status}");
            }
        }
    }

    fn child(root: &Element, role: &str, name: &str) -> Element {
        root.find(role, name).unwrap_or_else(|| {
            panic!(
                "the tree has a {role} named {name:?}: {:?}",
                root.descendants()
            )
        })
    }

    fn appears_on_query() {
        let Some(mut session) = Session::start() else {
            return;
        };
        assert_eq!(session.wakes(), "probe epoch=0 wakes=0");
        let root = session.connect();
        assert_eq!(
            root.title().as_deref(),
            Some(TITLE),
            "the root is named after the window"
        );
        let roles: Vec<String> = root
            .children()
            .iter()
            .map(|child| child.role().unwrap_or_default())
            .collect();
        assert_eq!(roles, ["AXButton", "AXStaticText", "AXTextField"]);
        assert_eq!(
            root.parent().and_then(|parent| parent.title()).as_deref(),
            Some(TITLE),
            "the root group's parent is the window"
        );
        assert_eq!(session.wakes(), "probe epoch=1 wakes=1");
    }

    fn pressing() {
        let Some(mut session) = Session::start() else {
            return;
        };
        let root = session.connect();
        let save = child(&root, "AXButton", "Save");
        assert!(
            save.actions().contains(&"AXPress".to_owned()),
            "{:?}",
            save.actions()
        );
        let status = child(&root, "AXStaticText", "Idle");
        save.press().expect("AXPress on the button succeeds");
        assert_eq!(
            session.expect("action"),
            "action Press node=1 data=None wakes=2",
            "the press arrives as one Press and wakes the loop once"
        );
        session.expect("applied status=Saved");
        assert_eq!(status.value().as_deref(), Some("Saved"));
        assert_eq!(session.wakes(), "probe epoch=1 wakes=2");
    }

    fn editing() {
        let Some(mut session) = Session::start() else {
            return;
        };
        let root = session.connect();
        let field = child(&root, "AXTextField", "Message");
        assert_eq!(field.value().as_deref(), Some("hello world"));
        assert_eq!(field.selected_range(), Some((5, 5)));
        let observer = Observer::new(session.child.id()).expect("an AX observer is created");
        observer
            .watch(&session.app, "AXValueChanged")
            .expect("the app's value changes can be observed");
        field
            .set_value("hello there")
            .expect("setting AXValue succeeds");
        assert_eq!(
            session.expect("action"),
            "action SetValue node=3 data=Value(\"hello there\") wakes=2"
        );
        session.expect("applied status=Idle text=hello there");
        let changed = observer
            .wait("AXValueChanged", WAIT)
            .expect("the field posts AXValueChanged");
        assert_eq!(changed, field);
        assert_eq!(field.value().as_deref(), Some("hello there"));
        assert_eq!(field.selected_range(), Some((11, 11)));
    }

    fn focusing() {
        let Some(mut session) = Session::start() else {
            return;
        };
        let root = session.connect();
        let field = child(&root, "AXTextField", "Message");
        let save = child(&root, "AXButton", "Save");
        let observer = Observer::new(session.child.id()).expect("an AX observer is created");
        observer
            .watch(&session.app, "AXFocusedUIElementChanged")
            .expect("the app's focus changes can be observed");
        session.send("focus 3");
        session.expect("applied status=Idle text=hello world focus=3");
        let focused = observer
            .wait("AXFocusedUIElementChanged", WAIT)
            .expect("moving focus posts AXFocusedUIElementChanged");
        assert_eq!(focused, field);
        assert_eq!(session.app.focused(), Some(field.clone()));
        assert_eq!(field.flag("AXFocused"), Some(true));
        save.set_focused(true).expect("setting AXFocused succeeds");
        assert_eq!(
            session.expect("action"),
            "action Focus node=1 data=None wakes=2"
        );
        session.expect("applied status=Idle text=hello world focus=1");
        let focused = observer
            .wait("AXFocusedUIElementChanged", WAIT)
            .expect("an assistive focus request posts AXFocusedUIElementChanged");
        assert_eq!(focused, save);
        assert_eq!(session.app.focused(), Some(save));
    }

    fn near(actual: Frame, expected: Frame) -> bool {
        (actual.x - expected.x).abs() < 0.5
            && (actual.y - expected.y).abs() < 0.5
            && (actual.width - expected.width).abs() < 0.5
            && (actual.height - expected.height).abs() < 0.5
    }

    fn framing() {
        let Some(mut session) = Session::start() else {
            return;
        };
        let root = session.connect();
        let window = session.window();
        let outer = window.frame().expect("the window has a frame");
        let content = Frame {
            x: outer.x,
            y: outer.y + outer.height - SIZE.1,
            width: SIZE.0,
            height: SIZE.1,
        };
        let frame = root.frame().expect("the root has a frame");
        assert!(
            near(frame, content),
            "the root covers the content area: {frame:?} vs {content:?}"
        );
        let save = child(&root, "AXButton", "Save");
        let button = save.frame().expect("the button has a frame");
        let expected = Frame {
            x: content.x + BUTTON.0,
            y: content.y + BUTTON.1,
            width: BUTTON.2,
            height: BUTTON.3,
        };
        assert!(
            near(button, expected),
            "the button sits at its logical offset from the content's top left in screen points: {button:?} vs {expected:?}"
        );
        assert!(outer.contains(button));
        let hit = session
            .app
            .element_at(button.center())
            .expect("hit testing the button's centre finds an element");
        assert_eq!(hit, save, "hit testing returns the button");
    }
}
