use aqua_core::Oklch;
use aqua_runtime::Runtime;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::ui::{Box as UiBox, Text as UiText};
use aqua_ui::style::{px, Axis, Style};
use aqua_verify::heap::{live_bytes, CountingAllocator};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

const ROWS: usize = 2000;
const MOUNT_BYTES_PER_ROW: f64 = 20.0 * 1024.0;
const FIRST_FRAME_BYTES_PER_ROW: f64 = 96.0 * 1024.0;
const PARAGRAPH: &str = "Aqua keeps retained text, layout and scene state between frames so that an unchanged transcript costs nothing, while a streaming message only reshapes the run that changed.";

fn mounted_runtime() -> Runtime {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime.set_viewport(1200.0, 800.0, 1.0);
    runtime
        .mount(|_| {
            UiBox::new()
                .style(Style::new().axis(Axis::Column).width(px(760.0)))
                .children((0..ROWS).map(|index| {
                    UiBox::new()
                        .fill(Oklch::from_hex("#1e1e2e").unwrap())
                        .radius(6.0)
                        .style(Style::new().width(px(760.0)))
                        .child(
                            UiText::builder()
                                .arg(format!(
                                    "Message {index}. {}",
                                    PARAGRAPH.repeat(1 + index % 4)
                                ))
                                .wrap(true)
                                .style(Style::new().width(px(720.0)))
                                .build(),
                        )
                        .build()
                }))
                .build()
        })
        .unwrap();
    runtime
}

#[test]
fn retained_heap_per_mounted_row_stays_bounded() {
    let before = live_bytes();
    let mut runtime = mounted_runtime();
    let mounted = live_bytes();
    runtime.build_frame().unwrap();
    let laid_out = live_bytes();
    std::hint::black_box(&runtime);
    let per_row = |from: u64, to: u64| (to as f64 - from as f64) / ROWS as f64;
    let (mount, frame) = (per_row(before, mounted), per_row(mounted, laid_out));
    eprintln!("retained bytes per row: mount {mount:.0}, first frame {frame:.0}");
    assert!(mount < MOUNT_BYTES_PER_ROW, "mount {mount}");
    assert!(frame < FIRST_FRAME_BYTES_PER_ROW, "first frame {frame}");
}
