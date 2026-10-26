//! Native keyboard regression child, driven by scripts/windows-input-smoke.ps1.
//! The script posts messages only to this child's window; it never sends global input.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
use aqua::prelude::*;
use aqua::Application;
use aqua::{ui, UiApp};
use aqua_reactive::ReadSignal;

struct KeyboardProbe {
    ui: UiApp,
    text: ReadSignal<String>,
    checked_first: bool,
}

impl Application for KeyboardProbe {
    fn on_window_event(&mut self, ctx: &mut AppCtx, id: AquaWindowId, event: &WindowEvent) {
        self.ui.on_window_event(ctx, id, event);
    }
    fn on_input(&mut self, ctx: &mut AppCtx, id: AquaWindowId, event: &InputEvent) {
        self.ui.on_input(ctx, id, event);
        if matches!(
            event,
            InputEvent::KeyDown(KeyEvent {
                key: Key::Named(NamedKey::Function(11)),
                ..
            })
        ) {
            self.ui.sync().unwrap();
            assert_eq!(
                self.text.get(),
                "AB",
                "one native Backspace deletes c exactly once and inserts no control text"
            );
            self.checked_first = true;
        }
        if matches!(
            event,
            InputEvent::KeyDown(KeyEvent {
                key: Key::Named(NamedKey::Function(12)),
                ..
            })
        ) {
            self.ui.sync().unwrap();
            assert!(
                self.checked_first,
                "intermediate native Backspace assertion must run"
            );
            assert_eq!(
                self.text.get(),
                "",
                "native Backspace must remove characters without reinserting controls"
            );
            println!("WINDOWS_INPUT_OK: native Backspace, Delete, Tab and Enter leave no control characters");
            ctx.quit();
        }
    }
    fn build_scene(
        &mut self,
        scene: &mut Scene,
        window: &ObservedWindowState,
    ) -> Option<std::time::Instant> {
        self.ui.build_scene(scene, window)
    }
    fn ime_cursor_area(&self) -> Option<Rect> {
        self.ui.ime_anchor()
    }
}

fn main() -> Result<(), String> {
    let mut config = AquaConfig::default();
    config.window.title = "Aqua native keyboard regression".into();
    config.window.inner_size = aqua_platform::geometry::LogicalSize::new(400.0, 160.0);
    let aqua = Aqua::new(config)?;
    let mut app = UiApp::new(aqua.fonts().clone(), aqua.runs().clone());
    let (read, write) = app.scope().signal("AB".to_owned());
    app.mount(|_| {
        ui! {
            Box(padding: 16, width: 360) {
                Text(bind: (read.clone(), write), width: 320, height: 48)
            }
        }
    })
    .map_err(|e| e.to_string())?;
    let field = app.tree().traverse().into_iter().find(|id| matches!(&app.tree().get(*id).unwrap().kind, aqua_ui::NodeKind::Text(t) if t.is_editable())).unwrap();
    app.tree_mut().set_focused(Some(field));
    aqua.run(KeyboardProbe {
        ui: app,
        text: read,
        checked_first: false,
    })
}
