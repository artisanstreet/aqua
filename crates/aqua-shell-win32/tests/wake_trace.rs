use aqua_shell_win32::wake_trace::{Source, Tally};
use std::time::Duration;

#[test]
fn render_orders_by_count_and_scales_to_per_second() {
    let mut tally = Tally::default();
    for _ in 0..10 {
        tally.record(Source::Message(0x0113));
    }
    for _ in 0..2 {
        tally.record(Source::Sent(0x0084));
    }
    tally.record(Source::Message(0xC123));
    let line = tally.render(Duration::from_secs(2));
    assert_eq!(
        line,
        "aqua-shell-wakes: secs=2.00 total=6.5 [timer=5.0,sent:nchittest=1.0,0xc123=0.5]"
    );
}
