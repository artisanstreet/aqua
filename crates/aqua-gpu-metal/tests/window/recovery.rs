use std::time::Duration;

use aqua_gpu_metal::window::Alpha;
use aqua_protocol::{FrameInputs, GpuBackend, GpuReadback, PresentHint};
use aqua_shell_appkit::testing::capture;

use super::support::{
    self, close, content, drive, extent_of, pixel, shown, solid, srgb, Flow, RenderThread, LIMIT,
};

pub fn device_loss_recovers_both() {
    if !support::appkit() {
        return;
    }
    let Some(gpu) = RenderThread::start() else {
        return;
    };
    let left = support::window("metal left", (200.0, 120.0), (20, 80));
    let right = support::window("metal right", (200.0, 120.0), (400, 80));
    let surfaces = [
        gpu.attach(left, Alpha::Opaque),
        gpu.attach(right, Alpha::Opaque),
    ];
    let extents = [extent_of(left), extent_of(right)];
    let before = [srgb(200, 60, 60), srgb(60, 200, 60)];
    let after = [[40u8, 90, 200], [230, 200, 40]];
    let mut recovered = false;
    drive(LIMIT, move |_, _| {
        if !recovered {
            surfaces
                .iter()
                .zip(extents)
                .zip(before)
                .for_each(|((surface, extent), color)| {
                    gpu.frame(*surface, solid(extent, color));
                });
            if surfaces.iter().any(|surface| shown(&gpu, *surface) < 2) {
                return Flow::Wait(Duration::from_millis(10));
            }
            let (failed, report, generation) = gpu.run(move |backend| {
                backend.inject_device_removal();
                let failed = backend
                    .execute(
                        &solid(extents[0], srgb(0, 0, 0)),
                        &FrameInputs::new(surfaces[0]),
                    )
                    .is_err();
                let report = backend.recover().expect("the device recovers");
                (failed, report, backend.generation())
            });
            assert!(failed, "a frame on the lost device fails");
            assert_eq!(
                report.rebuilt_surfaces, 2,
                "both windows keep their surfaces"
            );
            assert_eq!(generation, 1);
            for index in 0..2 {
                let surface = surfaces[index];
                let [r, g, b] = after[index];
                let plan = solid(extents[index], srgb(r, g, b));
                let read = gpu.run(move |backend| {
                    backend.wait_for_frame_slot(surface).expect("a frame slot");
                    backend
                        .execute(&plan, &FrameInputs::new(surface))
                        .expect("the recovered device renders");
                    let read = backend.read_surface(surface).expect("readback");
                    backend
                        .present(surface, PresentHint::Vsync)
                        .expect("present");
                    read
                });
                assert_eq!(read.extent, extents[index]);
                assert!(
                    close(
                        [read.rgba8[0], read.rgba8[1], read.rgba8[2]],
                        after[index],
                        1
                    ),
                    "window {index} renders on the new device: {:?}",
                    &read.rgba8[..4]
                );
            }
            recovered = true;
            return Flow::Wait(Duration::from_millis(10));
        }
        surfaces
            .iter()
            .zip(extents)
            .zip(after)
            .for_each(|((surface, extent), [r, g, b])| {
                gpu.frame(*surface, solid(extent, srgb(r, g, b)));
            });
        let windows = [left, right];
        let captured: Option<Vec<[u8; 3]>> = windows
            .iter()
            .zip(extents)
            .map(|(window, extent)| {
                capture::window(*window).map(|image| {
                    let (x, y) = content(&image, extent);
                    pixel(&image, x + extent.width / 2, y + extent.height / 2)
                })
            })
            .collect();
        let Some(captured) = captured else {
            return Flow::Done;
        };
        let matches = captured
            .iter()
            .zip(after)
            .all(|(seen, wanted)| close(*seen, wanted, 3));
        match matches {
            true => {
                gpu.assert_clean();
                Flow::Done
            }
            false if surfaces.iter().all(|surface| shown(&gpu, *surface) < 200) => {
                Flow::Wait(Duration::from_millis(20))
            }
            false => panic!("after recovery the screen shows {captured:?}, not {after:?}"),
        }
    });
}
