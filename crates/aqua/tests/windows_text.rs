#[cfg(windows)]
fn main() {
    use aqua::{
        ui::{Box as UiBox, Text},
        Aqua, AquaConfig, UiApp,
    };
    use aqua_core::Transform2D;
    use aqua_text::{SharedFonts, SharedRuns};
    use aqua_ui::style::{px, Axis, Style};
    if std::env::args().any(|argument| argument == "--window") {
        let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
        app.mount(|_| {
            UiBox::new()
                .style(Style::new().axis(Axis::Column))
                .children([
                    Text::new("Aé👩‍💻Z")
                        .editable(true)
                        .style(Style::new().width(px(180.0)).height(px(50.0)))
                        .build()
                        .aria_label("Original text"),
                    Text::new("Aé👩‍💻Z")
                        .editable(true)
                        .style(Style::new().width(px(180.0)).height(px(50.0)).transform(
                            Transform2D::scale(2.0, 2.0).then(Transform2D::translate(30.0, 10.0)),
                        ))
                        .build()
                        .aria_label("Transformed text"),
                ])
                .build()
        })
        .unwrap();
        let mut config = AquaConfig::default();
        config.window.title = "Aqua native text geometry".into();
        Aqua::new(config).unwrap().run(app).unwrap();
        return;
    }
    let script = include_str!("../../../scripts/windows-accessibility.ps1");
    let executable = std::env::current_exe()
        .unwrap()
        .to_string_lossy()
        .replace('\'', "''");
    let command =
        format!("& {{ {script} }} -Executable '{executable}' -Arguments '--window' -TextGeometry");
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &command])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("WINDOWS_TEXT_GEOMETRY_OK"), "{stdout}");
    print!("{stdout}");
}

#[cfg(not(windows))]
fn main() {}
