fn main() {
    #[cfg(target_os = "macos")]
    bundle::main();
    #[cfg(not(target_os = "macos"))]
    eprintln!("macos_bundle: requires macOS; package it with scripts/macos-bundle.sh");
}

#[cfg(target_os = "macos")]
mod bundle {
    use std::sync::Arc;
    use std::time::Duration;

    use aqua::prelude::{AppIdentity, NotificationCenter};
    use aqua::runtime::{AppCtx, Application};
    use aqua::{AppId, Aqua, AquaConfig};
    use aqua_core::shader::FragmentShader;
    use aqua_core::{DisplayItem, InputEvent, Rect};
    use aqua_protocol::{AquaWindowId, Instant, LogicalSize, ObservedWindowState, WindowEvent};
    use aqua_scene::Scene;

    const APP_ID: &str = "org.aqua.examples.bundle";
    const NAME: &str = "Aqua Bundle";
    const SHADE: &str = "struct Input { float4 position : SV_Position; float2 uv : TEXCOORD0; };\n\
        float4 shade(Input input) : SV_Target { return float4(1.0, 0.0, 1.0, 1.0); }\n";

    struct Backdrop {
        scene: Scene,
        program: Arc<FragmentShader>,
    }

    impl Application for Backdrop {
        fn on_window_event(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId, event: &WindowEvent) {
            if let WindowEvent::Resized(_, size) = event {
                println!("resized {}x{}", size.width, size.height);
            }
        }

        fn on_input(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId, _event: &InputEvent) {}

        fn build_scene(&mut self, window: &ObservedWindowState, _now: Instant) -> Option<Instant> {
            let item = DisplayItem::Shader {
                bounds: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: window.inner_size.width as f32,
                    h: window.inner_size.height as f32,
                },
                program: self.program.clone(),
                parameters: Arc::from([0.0f32; 4]),
                animated: false,
            };
            let first = self.scene.ordered().next().map(|(id, _)| id);
            match first {
                Some(id) => {
                    self.scene.update(id, item);
                }
                None => {
                    self.scene.insert(item);
                }
            }
            None
        }

        fn scene_mut(&mut self, _id: AquaWindowId) -> Option<&mut Scene> {
            Some(&mut self.scene)
        }
    }

    fn report_notifications() {
        let identity = AppIdentity::new(APP_ID, NAME).expect("the example's identity is valid");
        match NotificationCenter::new(identity, Arc::new(|_| {})) {
            Ok(center) => println!(
                "notifications reached the user notification center: {:?}",
                center.wait_for_authorization(Duration::from_secs(10))
            ),
            Err(error) => println!("notifications unavailable: {error:?}"),
        }
    }

    pub fn main() {
        report_notifications();
        let mut config = AquaConfig {
            app_id: Some(AppId::new(APP_ID).expect("the example's app id is valid")),
            ..AquaConfig::default()
        };
        config.window.title = NAME.into();
        config.window.inner_size = LogicalSize::new(320.0, 200.0);
        let result = Aqua::new(config).and_then(|aqua| {
            aqua.run(Backdrop {
                scene: Scene::new(),
                program: Arc::new(FragmentShader {
                    name: "macos_bundle.backdrop".into(),
                    hlsl: SHADE.into(),
                    entry: "shade".into(),
                    reference: None,
                }),
            })
        });
        if let Err(error) = result {
            eprintln!("macos_bundle: {error}");
            std::process::exit(1);
        }
    }
}
