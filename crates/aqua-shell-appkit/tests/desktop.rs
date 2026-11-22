#[cfg(target_os = "macos")]
mod support;

fn main() {
    #[cfg(target_os = "macos")]
    match std::env::var_os(cases::BUNDLED) {
        Some(_) => cases::bundled(),
        None => support::main(&cases::ALL),
    }
    #[cfg(not(target_os = "macos"))]
    println!("desktop: skipped; requires macOS");
}

#[cfg(target_os = "macos")]
mod cases {
    use super::support::{self, case, wait_for, Case};
    use aqua_core::Oklch;
    use aqua_protocol::TaskbarBadge;
    use aqua_shell_appkit::notifications::{
        AppIdentity, Bundle, Notification, NotificationAction, NotificationCenter,
        NotificationError,
    };
    use aqua_shell_appkit::open::{OpenError, OpenTarget, Workspace};
    use aqua_shell_appkit::pressure::{Level, PressureSource};
    use aqua_shell_appkit::testing::desktop::{
        ballast_above, default_application_for, footprint_bytes, running, terminate,
        PRESSURE_NOTE_FLOOR,
    };
    use aqua_shell_appkit::{dock, icon};
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::mpsc::{channel, Receiver};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    pub const BUNDLED: &str = "AQUA_DESKTOP_BUNDLED";
    const BUNDLE_ID: &str = "org.aqua.tests.notifications";
    const ANSWER_LIMIT: Duration = Duration::from_secs(10);
    const CHILD_LIMIT: Duration = Duration::from_secs(30);

    pub const ALL: [Case; 8] = [
        case(
            "urls_files_and_reveals_reach_the_matching_workspace_call",
            workspace_calls,
        ),
        case("a_text_file_opens_in_its_default_application", opens_a_file),
        case(
            "setting_a_count_shows_it_on_the_dock_tile_and_zero_clears_it",
            dock_badge,
        ),
        case(
            "a_file_icon_renders_at_the_requested_edge_with_visible_pixels",
            file_icon,
        ),
        case(
            "an_unbundled_process_reports_notifications_unsupported_without_crashing",
            unbundled,
        ),
        case(
            "a_bundled_app_can_ask_for_authorization_and_learns_the_answer",
            bundled_app,
        ),
        case(
            "a_simulated_warning_reaches_an_app_sized_process_and_normal_follows",
            simulated_pressure,
        ),
        case(
            "an_injected_level_reads_as_elevated_until_it_returns_to_normal",
            injected_pressure,
        ),
    ];

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        Url(String),
        File(PathBuf),
        Reveal(PathBuf),
    }

    #[derive(Default)]
    struct FakeWorkspace {
        calls: RefCell<Vec<Call>>,
    }

    impl Workspace for FakeWorkspace {
        fn open_url(&self, url: &url::Url) -> Result<(), OpenError> {
            self.calls.borrow_mut().push(Call::Url(url.to_string()));
            Ok(())
        }

        fn open_file(&self, path: &Path) -> Result<(), OpenError> {
            self.calls.borrow_mut().push(Call::File(path.to_path_buf()));
            Ok(())
        }

        fn reveal(&self, path: &Path) -> Result<(), OpenError> {
            self.calls
                .borrow_mut()
                .push(Call::Reveal(path.to_path_buf()));
            Ok(())
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("aqua-desktop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("the scratch directory is created");
        let path = dir.join(name);
        std::fs::write(&path, b"aqua\n").expect("the scratch file is written");
        path
    }

    fn workspace_calls() {
        let file = scratch("a b.txt");
        let canonical = file.canonicalize().expect("the scratch file resolves");
        let workspace = FakeWorkspace::default();
        OpenTarget::url("https://example.com/a?b=c")
            .and_then(|target| target.execute_with(&workspace))
            .expect("the URL opens");
        OpenTarget::path(&file)
            .and_then(|target| target.execute_with(&workspace))
            .expect("the file opens");
        OpenTarget::reveal(&file)
            .and_then(|target| target.execute_with(&workspace))
            .expect("the file is revealed");
        assert_eq!(
            workspace.calls.into_inner(),
            vec![
                Call::Url("https://example.com/a?b=c".into()),
                Call::File(canonical.clone()),
                Call::Reveal(canonical),
            ]
        );
        assert!(matches!(
            OpenTarget::url("file:///etc/passwd"),
            Err(OpenError::UnsupportedScheme(_))
        ));
    }

    fn opens_a_file() {
        if !support::appkit() {
            return;
        }
        aqua_shell_appkit::testing::activate();
        let file = scratch("opened.txt");
        let Some(application) = default_application_for(&file) else {
            println!("SKIP-NO-DEFAULT-APP: LaunchServices names no application for .txt");
            return;
        };
        terminate(&application);
        OpenTarget::path(&file)
            .and_then(|target| target.execute())
            .expect("NSWorkspace accepts the file");
        let watched = application.clone();
        let launched = wait_for(Duration::from_secs(20), move || running(&watched));
        terminate(&application);
        let watched = application.clone();
        let gone = wait_for(Duration::from_secs(20), move || !running(&watched));
        assert!(launched, "{application} starts to open the file");
        assert!(gone, "{application} quits before the next case runs");
    }

    fn dock_badge() {
        if !support::appkit() {
            return;
        }
        aqua_shell_appkit::testing::activate();
        let badge = |count| {
            Some(TaskbarBadge {
                count,
                color: Oklch::BLACK,
            })
        };
        dock::set_badge(badge(3)).expect("the dock tile takes a count");
        assert_eq!(dock::badge_label().as_deref(), Some("3"));
        dock::set_badge(badge(42)).expect("the dock tile takes a large count");
        assert_eq!(dock::badge_label().as_deref(), Some("9+"));
        dock::set_badge(badge(0)).expect("a zero count clears the tile");
        assert_eq!(dock::badge_label(), None);
        dock::set_badge(badge(1)).expect("the dock tile takes a count");
        dock::set_badge(None).expect("no badge clears the tile");
        assert_eq!(dock::badge_label(), None);
    }

    fn file_icon() {
        let edge = 48;
        let icon = icon::file_icon(Path::new("/Applications"), edge)
            .expect("the Applications folder has an icon");
        assert_eq!((icon.width, icon.height), (edge, edge));
        assert_eq!(icon.rgba.len(), (edge * edge * 4) as usize);
        let (pixels, _) = icon.rgba.as_chunks::<4>();
        let visible = pixels.iter().filter(|pixel| pixel[3] > 0).count();
        assert!(
            visible > pixels.len() / 4,
            "the folder icon covers a good part of its square: {visible} of {}",
            pixels.len()
        );
        let coloured = pixels
            .iter()
            .filter(|pixel| pixel[3] == 255 && (pixel[0] != pixel[1] || pixel[1] != pixel[2]))
            .count();
        assert!(coloured > 0, "the folder icon is drawn in colour");
        assert_eq!(icon::file_icon(Path::new("/Applications"), 0), None);
        assert_eq!(
            icon::file_icon(Path::new("/nonexistent/aqua/file"), edge),
            None
        );
    }

    fn identity() -> AppIdentity {
        AppIdentity::new(BUNDLE_ID, "Aqua notifications").expect("the test identity is valid")
    }

    fn unbundled() {
        assert_eq!(Bundle::of_this_process(), Bundle::Missing);
        let center = NotificationCenter::new(identity(), Arc::new(|_| {}));
        assert!(matches!(center, Err(NotificationError::Unsupported)));
    }

    fn info_plist(executable: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\"><dict>\
             <key>CFBundleIdentifier</key><string>{BUNDLE_ID}</string>\
             <key>CFBundleExecutable</key><string>{executable}</string>\
             <key>CFBundleName</key><string>Aqua notifications</string>\
             <key>CFBundlePackageType</key><string>APPL</string>\
             <key>CFBundleShortVersionString</key><string>1.0</string>\
             <key>CFBundleVersion</key><string>1</string>\
             <key>LSMinimumSystemVersion</key><string>14.0</string>\
             </dict></plist>\n"
        )
    }

    fn build_bundle() -> PathBuf {
        let root = std::env::temp_dir().join(format!("aqua-bundle-{}", std::process::id()));
        let app = root.join("AquaNotifications.app");
        let macos = app.join("Contents").join("MacOS");
        std::fs::create_dir_all(&macos).expect("the bundle directories are created");
        let executable = "aqua-notifications";
        std::fs::copy(
            std::env::current_exe().expect("the test binary knows its path"),
            macos.join(executable),
        )
        .expect("the test binary is copied into the bundle");
        std::fs::write(
            app.join("Contents").join("Info.plist"),
            info_plist(executable),
        )
        .expect("Info.plist is written");
        let signed = Command::new("codesign")
            .args(["--force", "--sign", "-"])
            .arg(&app)
            .status()
            .expect("codesign runs");
        assert!(signed.success(), "the bundle is signed ad hoc");
        macos.join(executable)
    }

    fn bundled_app() {
        let executable = build_bundle();
        let mut child = Command::new(&executable)
            .env(BUNDLED, "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("the bundled app starts");
        let mut stdout = child.stdout.take().expect("stdout is piped");
        let (sender, finished) = channel();
        std::thread::spawn(move || {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut stdout, &mut text)
                .expect("the bundled app writes UTF-8");
            sender.send(text).expect("the parent waits for the output");
        });
        let output = match finished.recv_timeout(CHILD_LIMIT) {
            Ok(output) => output,
            Err(_) => {
                child.kill().expect("the hung bundled app is killed");
                panic!("the bundled app did not finish within {CHILD_LIMIT:?}");
            }
        };
        let status = child.wait().expect("the bundled app exits");
        println!("{output}");
        assert!(status.success(), "the bundled app exits cleanly: {status}");
        let line = |prefix: &str| {
            output
                .lines()
                .find_map(|line| line.strip_prefix(prefix))
                .map(str::to_owned)
                .unwrap_or_else(|| panic!("the bundled app reports {prefix:?}: {output}"))
        };
        assert_eq!(
            line("bundle "),
            format!("App {{ identifier: {BUNDLE_ID:?} }}")
        );
        assert_eq!(line("center "), "created");
        let authorization = line("authorization ");
        let shown = line("show ");
        assert_eq!(line("withdraw "), "Ok(())");
        assert_eq!(line("invalid "), "Err(InvalidContent)");
        match authorization.as_str() {
            "Pending" => println!(
                "SKIP-NO-AUTHORIZATION-ANSWER: the authorization prompt waited for a person"
            ),
            "Granted" => assert_eq!(shown, "Ok(())"),
            _ => assert!(shown.starts_with("Err(Native("), "{authorization}: {shown}"),
        }
    }

    pub fn bundled() {
        println!("bundle {:?}", Bundle::of_this_process());
        let mut center = NotificationCenter::new(identity(), Arc::new(|_| {}))
            .expect("a bundled app gets a notification center");
        println!("center created");
        println!(
            "authorization {:?}",
            center.wait_for_authorization(ANSWER_LIMIT)
        );
        let notification = Notification {
            tag: "aqua-test".into(),
            title: "Aqua".into(),
            body: "A test notification".into(),
            actions: vec![NotificationAction {
                id: "open".into(),
                label: "Open".into(),
            }],
        };
        println!("show {:?}", center.show(notification));
        println!("withdraw {:?}", center.withdraw("aqua-test"));
        let invalid = Notification {
            tag: String::new(),
            title: "Aqua".into(),
            body: String::new(),
            actions: Vec::new(),
        };
        println!("invalid {:?}", center.show(invalid));
    }

    fn sudo() -> bool {
        Command::new("sudo")
            .args(["-n", "true"])
            .status()
            .is_ok_and(|status| status.success())
    }

    fn simulate(level: &str) {
        let status = Command::new("sudo")
            .args(["-n", "memory_pressure", "-S", "-l", level])
            .status()
            .expect("memory_pressure runs");
        assert!(status.success(), "memory_pressure simulates {level}");
    }

    fn woken() -> (Receiver<()>, aqua_shell_appkit::pressure::Wake) {
        let (sender, receiver) = channel();
        let sender = Mutex::new(sender);
        (
            receiver,
            Arc::new(move || {
                let _delivered = sender
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .send(());
            }),
        )
    }

    fn until(events: &Receiver<()>, mut reached: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + ANSWER_LIMIT;
        while !reached() {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            if events.recv_timeout(left).is_err() {
                return reached();
            }
        }
        true
    }

    fn until_level(source: &PressureSource, events: &Receiver<()>, wanted: Level) -> bool {
        until(events, || source.level() == wanted)
    }

    fn simulated_pressure() {
        if !sudo() {
            println!("SKIP-NO-SUDO: memory_pressure -S needs passwordless sudo");
            return;
        }
        let ballast = ballast_above(PRESSURE_NOTE_FLOOR);
        let footprint = footprint_bytes();
        println!(
            "footprint {} MB with {} MB ballast",
            footprint >> 20,
            ballast.len() >> 20
        );
        let (events, wake) = woken();
        let source = PressureSource::system(Some(wake));
        let simulator = std::thread::spawn(|| simulate("warn"));
        let warned = until(&events, || source.take_elevated());
        simulator
            .join()
            .expect("memory_pressure simulates a warning and resets it");
        assert!(
            warned,
            "a {} MB process hears the simulated warning within {ANSWER_LIMIT:?}",
            footprint >> 20
        );
        assert!(
            until_level(&source, &events, Level::Normal),
            "the dispatch source reports the reset to normal"
        );
        assert!(!source.take_elevated(), "normal memory is not elevated");
    }

    fn injected_pressure() {
        let (events, wake) = woken();
        let (source, injector) = PressureSource::injectable(Some(wake));
        assert!(!source.take_elevated());
        injector.inject(Level::Critical);
        assert!(until_level(&source, &events, Level::Critical));
        injector.inject(Level::Normal);
        assert!(until_level(&source, &events, Level::Normal));
        assert!(
            source.take_elevated(),
            "a warning that ended before the poll still counts once"
        );
        assert!(!source.take_elevated());
    }
}
