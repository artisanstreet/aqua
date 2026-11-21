fn main() {
    #[cfg(target_os = "macos")]
    macos::main();
    #[cfg(not(target_os = "macos"))]
    eprintln!("host_macos: skipped; requires macOS");
}

#[cfg(target_os = "macos")]
mod macos {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpStream;
    use std::num::NonZeroUsize;
    use std::path::PathBuf;
    use std::process::{Child, ChildStdout, Command, ExitStatus, Stdio};
    use std::sync::mpsc::{channel, Receiver};
    use std::time::{Duration, Instant};

    use aqua::mailbox::Mailbox;
    use aqua::runtime::{AppCtx, Application};
    use aqua::{px, Aqua, AquaConfig, Context, IntoView};
    use aqua_core::{DisplayItem, InputEvent, Modifiers, Oklch, Rect};
    use aqua_protocol::{
        AquaWindowId, Instant as AquaInstant, ObservedWindowState, WindowEvent, WindowOptions,
        WindowRequest,
    };
    use aqua_scene::Scene;
    use aqua_shell_appkit::testing::capture::window_number_image;
    use aqua_shell_appkit::testing::cases::{case, Case};
    use aqua_shell_appkit::testing::inject::{Button, Motion};
    use aqua_shell_appkit::testing::post::{self, END, TAB};
    use aqua_shell_appkit::testing::screen::{windows_of, OnScreen};
    use aqua_shell_appkit::testing::{available, Facility};
    use image::RgbaImage;
    use serde_json::Value;

    const APP: &str = "AQUA_HOST_MACOS_APP";
    const WINDOWS: &str = "AQUA_HOST_MACOS_WINDOWS";
    const VETO: &str = "AQUA_HOST_MACOS_VETO";
    const ANIMATE: &str = "AQUA_HOST_MACOS_ANIMATE";
    const LIMIT: Duration = Duration::from_secs(15);
    const TICK: Duration = Duration::from_millis(20);
    const SIZE: (f64, f64) = (480.0, 320.0);
    const RED: [u8; 3] = [220, 56, 56];
    const BLUE: [u8; 3] = [40, 108, 255];
    const KEY_A: u16 = 0;
    const KEY_C: u16 = 8;
    const KEY_V: u16 = 9;
    const KEY_Q: u16 = 12;
    const KEY_M: u16 = 46;

    pub fn main() {
        match std::env::var(APP).as_deref() {
            Ok("probe") => probe(),
            Ok("field") => field(),
            Ok(other) => panic!("no app mode {other}"),
            Err(_) => aqua_shell_appkit::testing::cases::main(&ALL),
        }
    }

    const ALL: [Case; 17] = [
        case(
            "a_real_app_shows_a_window_and_presents_frames",
            shows_and_presents,
        ),
        case("clicking_a_button_runs_its_handler", clicking),
        case(
            "typing_copying_and_pasting_in_a_field_shows_the_text_on_screen",
            typing,
        ),
        case(
            "text_renders_with_fonts_found_through_core_text_alone",
            core_text,
        ),
        case("resizing_the_window_reflows_and_repaints", resizing),
        case("closing_can_be_vetoed", vetoing),
        case("two_windows_render_independently", two_windows),
        case(
            "a_covered_window_stops_rendering_and_resumes_when_shown",
            covering,
        ),
        case("the_smoke_run_exits_with_presented_frames", smoke),
        case(
            "the_showcase_example_renders_takes_input_and_quits_cleanly",
            showcase,
        ),
        case(
            "the_todo_example_renders_takes_input_and_quits_cleanly",
            todo,
        ),
        case(
            "the_editor_shell_example_renders_takes_input_and_quits_cleanly",
            editor_shell,
        ),
        case(
            "the_conversation_example_renders_takes_input_and_quits_cleanly",
            conversation,
        ),
        case(
            "the_multi_window_example_renders_both_windows_takes_input_and_quits_cleanly",
            multi_window,
        ),
        case(
            "the_collections_example_renders_takes_input_and_quits_cleanly",
            collections,
        ),
        case(
            "the_pixel_storm_example_renders_takes_input_and_quits_cleanly",
            pixel_storm,
        ),
        case(
            "the_windows_input_smoke_example_renders_takes_input_and_quits_cleanly",
            windows_input_smoke,
        ),
    ];

    fn report(line: &str) {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    }

    fn fill(color: [u8; 3], shade: u8) -> Oklch {
        Oklch::from_srgb8(color[0], color[1].saturating_sub(shade), color[2], 1.0)
    }

    struct Probe {
        windows: Vec<AquaWindowId>,
        scenes: Vec<Scene>,
        wanted: usize,
        veto: bool,
        vetoed: bool,
        animate: bool,
        frame: u8,
        restore: Mailbox<AquaWindowId>,
    }

    impl Probe {
        fn index(&self, id: AquaWindowId) -> Option<usize> {
            self.windows.iter().position(|window| *window == id)
        }
    }

    impl Application for Probe {
        fn on_start(&mut self, ctx: &mut AppCtx, id: AquaWindowId) {
            self.windows.push(id);
            self.scenes.push(Scene::new());
            report(&format!("started {}", self.windows.len()));
            ctx.bind_mailbox(id, &self.restore);
            if self.windows.len() < self.wanted {
                ctx.request_window(WindowRequest::Create(WindowOptions {
                    title: format!("aqua probe {}", self.windows.len() + 1),
                    inner_size: aqua_protocol::LogicalSize::new(SIZE.0, SIZE.1),
                    ..WindowOptions::default()
                }))
                .expect("the second window is requested");
            }
        }

        fn can_close(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId) -> bool {
            match self.veto && !self.vetoed {
                true => {
                    self.vetoed = true;
                    report("vetoed");
                    false
                }
                false => true,
            }
        }

        fn on_window_event(&mut self, _ctx: &mut AppCtx, id: AquaWindowId, event: &WindowEvent) {
            match event {
                WindowEvent::Resized(_, size) => {
                    report(&format!("resized {}x{}", size.width, size.height))
                }
                WindowEvent::Occluded(_, occluded) => {
                    report(&format!("occluded {occluded}"));
                    if *occluded && self.animate {
                        let sender = self.restore.sender();
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_secs(1));
                            let _ = sender.send(id);
                        });
                    }
                }
                WindowEvent::Destroyed(_) => report("destroyed"),
                _ => {}
            }
        }

        fn on_wake(&mut self, ctx: &mut AppCtx, _id: AquaWindowId) {
            for id in self.restore.drain() {
                report("restoring");
                let _ = ctx.request_window(WindowRequest::SetMinimized(id, false));
            }
        }

        fn on_input(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId, _event: &InputEvent) {}

        fn build_scene(
            &mut self,
            window: &ObservedWindowState,
            now: AquaInstant,
        ) -> Option<AquaInstant> {
            let index = self.index(window.id)?;
            self.frame = self.frame.wrapping_add(1);
            let shade = match self.animate {
                true => self.frame % 2,
                false => 0,
            };
            let color = match index {
                0 => RED,
                _ => BLUE,
            };
            let item = DisplayItem::Rect {
                bounds: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: window.inner_size.width as f32,
                    h: window.inner_size.height as f32,
                },
                radius: 0.0,
                fill: fill(color, shade),
                border_width: 0.0,
                border_color: Oklch::TRANSPARENT,
                shadow: None,
            };
            let scene = &mut self.scenes[index];
            let first = scene.ordered().next().map(|(id, _)| id);
            match first {
                Some(id) => {
                    scene.update(id, item);
                }
                None => {
                    scene.insert(item);
                }
            }
            self.animate.then(|| now + Duration::from_millis(16))
        }

        fn scene_mut(&mut self, id: AquaWindowId) -> Option<&mut Scene> {
            let index = self.index(id)?;
            self.scenes.get_mut(index)
        }
    }

    fn flag(name: &str) -> bool {
        std::env::var_os(name).is_some_and(|value| value == "1")
    }

    fn probe() {
        let mut config = AquaConfig::default();
        config.window.title = "aqua probe 1".into();
        config.window.inner_size = aqua_protocol::LogicalSize::new(SIZE.0, SIZE.1);
        let wanted = std::env::var(WINDOWS)
            .ok()
            .and_then(|count| count.parse().ok())
            .unwrap_or(1);
        let result = Aqua::new(config).and_then(|aqua| {
            aqua.run(Probe {
                windows: Vec::new(),
                scenes: Vec::new(),
                wanted,
                veto: flag(VETO),
                vetoed: false,
                animate: flag(ANIMATE),
                frame: 0,
                restore: Mailbox::new(NonZeroUsize::new(4).expect("four is not zero")),
            })
        });
        if let Err(error) = result {
            eprintln!("host_macos: the probe failed: {error}");
            std::process::exit(1);
        }
        report("exited");
    }

    fn field_view(cx: &Context) -> impl IntoView + use<> {
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

    fn field() {
        let result = Aqua::App()
            .config(
                Aqua::Config().window(
                    Aqua::Window()
                        .title("aqua field")
                        .size(px(SIZE.0 as f32), px(SIZE.1 as f32)),
                ),
            )
            .run(field_view);
        if let Err(error) = result {
            eprintln!("host_macos: the field app failed: {error}");
            std::process::exit(1);
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

        fn expect(&self, what: &str, accept: impl Fn(&str) -> bool) -> String {
            let deadline = Instant::now() + LIMIT;
            let mut seen = Vec::new();
            while let Some(left) = deadline.checked_duration_since(Instant::now()) {
                match self.0.recv_timeout(left) {
                    Ok(line) if accept(&line) => return line,
                    Ok(line) => seen.push(line),
                    Err(_) => break,
                }
            }
            panic!("the app never reported {what}; it reported {seen:?}");
        }
    }

    struct App {
        child: Child,
        label: String,
        lines: Option<Lines>,
        stderr: Option<std::thread::JoinHandle<String>>,
    }

    impl Drop for App {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    impl App {
        fn start(mut command: Command, label: &str) -> App {
            let mut child = command
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap_or_else(|error| panic!("{label} did not start: {error}"));
            let lines = Lines::follow(child.stdout.take().expect("stdout is piped"));
            let mut stderr = child.stderr.take().expect("stderr is piped");
            let stderr = std::thread::spawn(move || {
                let mut text = String::new();
                let _ = stderr.read_to_string(&mut text);
                text
            });
            App {
                child,
                label: label.to_owned(),
                lines: Some(lines),
                stderr: Some(stderr),
            }
        }

        fn lines(&self) -> &Lines {
            self.lines.as_ref().expect("the app's stdout is followed")
        }

        fn pid(&self) -> u32 {
            self.child.id()
        }

        fn running(&mut self) -> bool {
            self.child
                .try_wait()
                .expect("the app process is waitable")
                .is_none()
        }

        fn exit(&mut self) -> (ExitStatus, String) {
            let status = within(&format!("{} to exit", self.label), || {
                self.child.try_wait().expect("the app process is waitable")
            });
            let stderr = self
                .stderr
                .take()
                .map(|reader| reader.join().expect("the stderr reader finishes"))
                .unwrap_or_default();
            (status, stderr)
        }

        fn quits_cleanly(&mut self) {
            post::tap(KEY_Q, Modifiers::LOGO);
            let (status, stderr) = self.exit();
            assert!(
                status.success(),
                "{} quits cleanly on Command-Q: {status}\n{stderr}",
                self.label
            );
        }

        fn windows(&self, count: usize) -> Vec<OnScreen> {
            let pid = self.pid();
            within(&format!("{} to show {count} windows", self.label), || {
                let windows = windows_of(pid);
                (windows.len() == count).then_some(windows)
            })
        }
    }

    fn command(mode: &str, env: &[(&str, &str)]) -> Command {
        let mut command =
            Command::new(std::env::current_exe().expect("the test binary knows its path"));
        command.env(APP, mode);
        env.iter().for_each(|(key, value)| {
            command.env(key, value);
        });
        command
    }

    fn example(name: &str) -> PathBuf {
        let binary = std::env::current_exe().expect("the test binary path");
        binary
            .ancestors()
            .map(|directory| directory.join("examples").join(name))
            .find(|program| program.is_file())
            .unwrap_or_else(|| panic!("the {name} example is built next to {}", binary.display()))
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

        fn frames(&self) -> Option<u64> {
            self.ask("stats")?["frames_presented"].as_u64()
        }

        fn value(&self) -> Option<String> {
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
            find(&reply["tree"], "TextInput")?["value"]
                .as_str()
                .map(str::to_owned)
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

        fn await_frames_above(&self, what: &str, floor: u64) -> u64 {
            within(what, || self.frames().filter(|frames| *frames > floor))
        }
    }

    fn inspected(mode: &str, env: &[(&str, &str)]) -> (App, Inspector) {
        let port = free_port();
        let port_text = port.to_string();
        let mut env = env.to_vec();
        env.push(("AQUA_INSPECTOR", port_text.as_str()));
        (App::start(command(mode, &env), mode), Inspector(port))
    }

    fn content_top(window: &OnScreen, logical_height: f64) -> f64 {
        window.bounds.y + (window.bounds.height - logical_height)
    }

    fn at(window: &OnScreen, logical_height: f64, (x, y): (f64, f64)) -> (f64, f64) {
        (window.bounds.x + x, content_top(window, logical_height) + y)
    }

    fn click(point: (f64, f64)) {
        post::mouse_at(Button::Left, Motion::Moved, point, 0);
        post::mouse_at(Button::Left, Motion::Down, point, 1);
        post::mouse_at(Button::Left, Motion::Up, point, 1);
    }

    fn capturing() -> bool {
        available(Facility::WindowServer) && available(Facility::ScreenCapture)
    }

    fn rgb(image: &RgbaImage, x: u32, y: u32) -> [u8; 3] {
        let [r, g, b, _] = image.get_pixel(x, y).0;
        [r, g, b]
    }

    fn close(left: [u8; 3], right: [u8; 3], tolerance: u8) -> bool {
        left.iter()
            .zip(right)
            .all(|(left, right)| left.abs_diff(right) <= tolerance)
    }

    fn content_pixel(window: &OnScreen, logical_height: f64, (x, y): (f64, f64)) -> [u8; 3] {
        let image = window_number_image(window.number);
        let scale = f64::from(image.width()) / window.bounds.width;
        let top = (window.bounds.height - logical_height) * scale;
        rgb(&image, (x * scale) as u32, (top + y * scale) as u32)
    }

    fn shows(window: &OnScreen, logical_height: f64, point: (f64, f64), color: [u8; 3]) {
        let mut last = [0; 3];
        let shown = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            within("the window to show its colour", || {
                last = content_pixel(window, logical_height, point);
                close(last, color, 4).then_some(())
            })
        }));
        assert!(
            shown.is_ok(),
            "window {} shows {last:?} at {point:?}, not {color:?}",
            window.title
        );
    }

    fn rendered(window: &OnScreen) {
        within(
            &format!("{} to show rendered content", window.title),
            || {
                let image = window_number_image(window.number);
                let first = image.get_pixel(image.width() / 2, image.height() - 2).0;
                let varied = (0..image.height())
                    .step_by(3)
                    .flat_map(|y| (0..image.width()).step_by(3).map(move |x| (x, y)))
                    .filter(|(x, y)| image.get_pixel(*x, *y).0 != first)
                    .count();
                (varied > 50).then_some(())
            },
        );
    }

    fn inked(window: &OnScreen, logical_height: f64, region: Rect) -> usize {
        let image = window_number_image(window.number);
        let scale = f64::from(image.width()) / window.bounds.width;
        let top = (window.bounds.height - logical_height) * scale;
        let pixel = |x: f32, y: f32| {
            rgb(
                &image,
                (f64::from(x) * scale) as u32,
                (top + f64::from(y) * scale) as u32,
            )
        };
        let mut seen = std::collections::HashMap::new();
        let mut y = region.y;
        while y < region.y + region.h {
            let mut x = region.x;
            while x < region.x + region.w {
                *seen.entry(pixel(x, y)).or_insert(0usize) += 1;
                x += 1.0;
            }
            y += 1.0;
        }
        let ground = seen
            .into_iter()
            .max_by_key(|(_, count)| *count)
            .map(|(colour, _)| colour)
            .expect("the region holds pixels");
        let mut count = 0;
        let mut y = region.y;
        while y < region.y + region.h {
            let mut x = region.x;
            while x < region.x + region.w {
                if !close(pixel(x, y), ground, 24) {
                    count += 1;
                }
                x += 1.0;
            }
            y += 1.0;
        }
        count
    }

    fn shows_and_presents() {
        if !capturing() {
            return;
        }
        let (mut app, inspector) = inspected("probe", &[]);
        let window = app.windows(1).remove(0);
        shows(&window, SIZE.1, (240.0, 160.0), RED);
        let frames = inspector.await_frames_above("the first presented frame", 0);
        println!("presented {frames} frames; window {window:?}");
        app.quits_cleanly();
        app.lines()
            .expect("the run to return", |line| line == "exited");
    }

    fn clicking() {
        if !capturing() || !available(Facility::PostEvent) {
            return;
        }
        let (mut app, inspector) = inspected("field", &[]);
        inspector.await_value("the first tree", "draft");
        let window = app.windows(1).remove(0);
        rendered(&window);
        click(at(&window, SIZE.1, (80.0, 20.0)));
        inspector.await_value("the button's handler", "draft!");
        app.quits_cleanly();
    }

    fn typing() {
        if !capturing() || !available(Facility::PostEvent) {
            return;
        }
        let (mut app, inspector) = inspected("field", &[]);
        inspector.await_value("the first tree", "draft");
        let window = app.windows(1).remove(0);
        rendered(&window);
        let line = Rect {
            x: 0.0,
            y: 40.0,
            w: 300.0,
            h: 32.0,
        };
        let before = inked(&window, SIZE.1, line);
        click(at(&window, SIZE.1, (100.0, 56.0)));
        post::tap(END, Modifiers::NONE);
        post::type_text("xyz");
        inspector.await_value("typed text", "draftxyz");
        let after = within("the typed text to reach the screen", || {
            let after = inked(&window, SIZE.1, line);
            (after > before + 20).then_some(after)
        });
        println!("field ink before {before}, after {after}");
        post::tap(KEY_A, Modifiers::LOGO);
        post::tap(KEY_C, Modifiers::LOGO);
        post::tap(END, Modifiers::NONE);
        post::tap(KEY_V, Modifiers::LOGO);
        inspector.await_value(
            "Command-A, Command-C, End, Command-V through the app clipboard",
            "draftxyzdraftxyz",
        );
        app.quits_cleanly();
    }

    fn core_text() {
        if !capturing() {
            return;
        }
        let mut command = command("field", &[]);
        command.env_remove("AQUA_FONT_DIRS");
        let mut app = App::start(command, "field without AQUA_FONT_DIRS");
        let window = app.windows(1).remove(0);
        let button = Rect {
            x: 4.0,
            y: 4.0,
            w: 152.0,
            h: 32.0,
        };
        let ink = within("the Press label to paint a Core Text font", || {
            let ink = inked(&window, SIZE.1, button);
            (ink > 60).then_some(ink)
        });
        assert!(
            ink < (152 * 32) / 2,
            "only the label is inked: {ink} pixels"
        );
        println!("the Press label inks {ink} pixels");
        app.quits_cleanly();
    }

    fn drag(from: (f64, f64), by: (f64, f64)) {
        post::mouse_at(Button::Left, Motion::Moved, from, 0);
        post::mouse_at(Button::Left, Motion::Down, from, 1);
        (1..=12).for_each(|step| {
            let fraction = f64::from(step) / 12.0;
            post::mouse_at(
                Button::Left,
                Motion::Dragged,
                (from.0 + by.0 * fraction, from.1 + by.1 * fraction),
                1,
            );
            std::thread::sleep(TICK);
        });
        let to = (from.0 + by.0, from.1 + by.1);
        post::mouse_at(Button::Left, Motion::Up, to, 1);
    }

    fn resizing() {
        if !capturing() || !available(Facility::PostEvent) {
            return;
        }
        let mut app = App::start(command("probe", &[]), "probe");
        let window = app.windows(1).remove(0);
        shows(&window, SIZE.1, (240.0, 160.0), RED);
        let corner = (
            window.bounds.x + window.bounds.width - 2.0,
            window.bounds.y + window.bounds.height - 2.0,
        );
        drag(corner, (120.0, 80.0));
        let size = |line: &str| {
            line.strip_prefix("resized ")
                .and_then(|size| size.split_once('x'))
                .and_then(|(width, height)| {
                    Some((width.parse::<f64>().ok()?, height.parse::<f64>().ok()?))
                })
        };
        let line = app.lines().expect("a larger size", |line| {
            size(line)
                .is_some_and(|(width, height)| width > SIZE.0 + 40.0 && height > SIZE.1 + 30.0)
        });
        let mut latest = size(&line).expect("the line names a size");
        while let Ok(next) = app.lines().0.recv_timeout(Duration::from_millis(300)) {
            latest = size(&next).unwrap_or(latest);
        }
        let (width, height) = latest;
        println!("settled at {width}x{height}");
        let grown = within("the window to settle at its new size on screen", || {
            windows_of(app.pid())
                .into_iter()
                .find(|grown| (grown.bounds.width - width).abs() < 1.0)
        });
        shows(&grown, height, (width - 8.0, height - 8.0), RED);
        app.quits_cleanly();
    }

    fn vetoing() {
        if !capturing() || !available(Facility::PostEvent) {
            return;
        }
        let mut app = App::start(command("probe", &[(VETO, "1")]), "vetoing probe");
        let window = app.windows(1).remove(0);
        shows(&window, SIZE.1, (240.0, 160.0), RED);
        post::tap(KEY_Q, Modifiers::LOGO);
        app.lines().expect("the veto", |line| line == "vetoed");
        let still = app.windows(1).remove(0);
        shows(&still, SIZE.1, (240.0, 160.0), RED);
        assert!(app.running(), "a vetoed close keeps the app running");
        app.quits_cleanly();
    }

    fn two_windows() {
        if !capturing() || !available(Facility::PostEvent) {
            return;
        }
        let (mut app, inspector) = inspected("probe", &[(WINDOWS, "2")]);
        let windows = app.windows(2);
        let first = windows
            .iter()
            .find(|window| window.title == "aqua probe 1")
            .expect("the first window is on screen")
            .clone();
        let second = windows
            .iter()
            .find(|window| window.title == "aqua probe 2")
            .expect("the second window is on screen")
            .clone();
        shows(&first, SIZE.1, (240.0, 160.0), RED);
        shows(&second, SIZE.1, (240.0, 160.0), BLUE);
        click((second.bounds.x + 14.0, second.bounds.y + 14.0));
        app.lines()
            .expect("the second window to close", |line| line == "destroyed");
        let remaining = app.windows(1).remove(0);
        assert_eq!(remaining.title, "aqua probe 1");
        assert!(app.running(), "closing one window keeps the app running");
        shows(&remaining, SIZE.1, (240.0, 160.0), RED);
        inspector.await_frames_above("frames after the close", 0);
        app.quits_cleanly();
    }

    fn covering() {
        if !capturing() || !available(Facility::PostEvent) {
            return;
        }
        let (mut app, inspector) = inspected("probe", &[(ANIMATE, "1")]);
        let window = app.windows(1).remove(0);
        shows(&window, SIZE.1, (240.0, 160.0), RED);
        let running = inspector.await_frames_above("animated frames", 10);
        post::tap(KEY_M, Modifiers::LOGO);
        app.lines()
            .expect("the minimized window to be occluded", |line| {
                line == "occluded true"
            });
        let hidden = inspector.frames().expect("the inspector answers");
        app.lines()
            .expect("the restore", |line| line == "restoring");
        let before_restore = inspector.frames().expect("the inspector answers");
        assert!(
            before_restore <= hidden + 3,
            "a minimized window renders nothing: {hidden} frames when hidden, {before_restore} a second later"
        );
        app.lines()
            .expect("the restored window to be visible", |line| {
                line == "occluded false"
            });
        let resumed = inspector.await_frames_above("frames after the restore", before_restore + 10);
        println!("frames: running {running}, hidden {hidden}, before restore {before_restore}, resumed {resumed}");
        app.quits_cleanly();
    }

    fn smoke() {
        if !available(Facility::WindowServer) {
            return;
        }
        let mut timed = App::start(command("probe", &[("AQUA_SMOKE_MS", "800")]), "smoke");
        let (status, stderr) = timed.exit();
        assert!(
            status.success(),
            "a time budget ends the app cleanly\n{stderr}"
        );
        let presented = stderr
            .lines()
            .find_map(|line| line.split("presented_frames=").nth(1))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|count| count.parse::<u64>().ok())
            .unwrap_or_else(|| panic!("the smoke report names its presented frames:\n{stderr}"));
        assert!(presented > 0, "the smoke run presented frames\n{stderr}");
        let mut frames = App::start(command("probe", &[("AQUA_SMOKE_FRAMES", "1")]), "frames");
        let (status, stderr) = frames.exit();
        assert!(
            status.success(),
            "a frame budget ends the app cleanly\n{stderr}"
        );
    }

    fn exercise(name: &str, count: usize, args: &[&str]) {
        if !capturing() || !available(Facility::PostEvent) {
            return;
        }
        let mut program = Command::new(example(name));
        program.args(args);
        let mut app = App::start(program, name);
        let windows = app.windows(count);
        windows.iter().for_each(rendered);
        let first = &windows[0];
        let centre = (
            first.bounds.x + first.bounds.width / 2.0,
            first.bounds.y + first.bounds.height / 2.0,
        );
        click(centre);
        post::scroll_at(centre, -3);
        post::tap(TAB, Modifiers::NONE);
        post::type_text("aqua");
        windows.iter().for_each(rendered);
        assert!(app.running(), "{name} survives keyboard and pointer input");
        app.quits_cleanly();
    }

    fn showcase() {
        exercise("showcase", 1, &[]);
    }

    fn todo() {
        exercise("todo", 1, &[]);
    }

    fn editor_shell() {
        exercise("editor_shell", 1, &[]);
    }

    fn conversation() {
        exercise("conversation", 1, &[]);
    }

    fn multi_window() {
        exercise("multi_window", 2, &[]);
    }

    fn collections() {
        exercise("collections", 1, &[]);
    }

    fn pixel_storm() {
        exercise("pixel_storm", 1, &["--size", "160x100"]);
    }

    fn windows_input_smoke() {
        exercise("windows_input_smoke", 1, &[]);
    }
}
