#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use aqua::config::AquaConfig;
use aqua::{px, Aqua, Style};
use aqua_core::Size;
use aqua_protocol::{AquaWindowId, WindowSpec};
use aqua_reactive::Scope;
use aqua_runtime::{Runtime, RuntimeError};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::Element;

const FIRST_TITLE: &str = "Aqua multi-window first";
const SECOND_TITLE: &str = "Aqua multi-window second";

fn page(label: &'static str, count: u32, press: impl Fn() + 'static) -> Element {
    Aqua::Box()
        .style(Style::new().column().width(px(360.0)).height(px(200.0)))
        .child(Aqua::Text(format!("{label} window")).build())
        .child(Aqua::Text(format!("{label} count {count}")).build())
        .child(
            Aqua::Button()
                .style(Style::new().width(px(180.0)).height(px(40.0)))
                .on_activate(press)
                .child(Aqua::Text(format!("Press {label}")).build())
                .build(),
        )
        .build()
}

fn mount_page(runtime: &mut Runtime, label: &'static str) -> Result<(), RuntimeError> {
    let (count, set_count) = runtime.scope().signal(0u32);
    runtime.mount_reactive((count,), move |(count,)| {
        let set_count = set_count.clone();
        page(label, count, move || set_count.update(|count| *count += 1))
    })
}

fn root(_scope: &Scope, label: &'static str) -> Element {
    page(label, 0, || {})
}

fn main() -> Result<(), aqua::Error> {
    let mut config = AquaConfig::default();
    config.window.title = FIRST_TITLE.into();
    let aqua = Aqua::new(config).map_err(aqua::Error::Init)?;
    let fonts: SharedFonts = aqua.fonts().clone();
    let runs: SharedRuns = aqua.runs().clone();
    let second = WindowSpec {
        title: SECOND_TITLE.into(),
        size: Size { w: 360.0, h: 200.0 },
        ..WindowSpec::default()
    };
    aqua.run_ui_windows_opening(
        [second],
        |scope| root(scope, "first"),
        move |_: AquaWindowId| {
            let mut runtime = Runtime::new(fonts.clone(), runs.clone());
            mount_page(&mut runtime, "second")?;
            Ok(runtime)
        },
    )
    .map_err(aqua::Error::Init)
}
