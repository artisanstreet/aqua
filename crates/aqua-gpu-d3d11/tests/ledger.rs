use aqua_gpu_d3d11::ledger::{record, snapshot, Kind};

#[test]
fn tokens_count_while_alive_and_release_on_drop() {
    let first = record("ledger_test_alive", Kind::Texture, 1000);
    let second = record("ledger_test_alive", Kind::Texture, 500);
    let line = snapshot().find("ledger_test_alive").unwrap().line;
    assert_eq!((line.count, line.bytes), (2, 1500));
    drop(first);
    let line = snapshot().find("ledger_test_alive").unwrap().line;
    assert_eq!((line.count, line.bytes, line.peak_bytes), (1, 500, 1500));
    drop(second);
    let line = snapshot().find("ledger_test_alive").unwrap().line;
    assert_eq!((line.count, line.bytes, line.created), (0, 0, 2));
}

#[test]
fn peaks_survive_release() {
    let token = record("ledger_test_peak", Kind::TransientTexture, 4096);
    drop(token);
    let entry = snapshot().find("ledger_test_peak").unwrap().clone();
    assert_eq!((entry.line.bytes, entry.line.peak_bytes), (0, 4096));
}

#[test]
fn render_lists_the_largest_peak_first() {
    let small = record("ledger_test_render_small", Kind::DynamicBuffer, 10);
    let large = record("ledger_test_render_large", Kind::DynamicBuffer, 10_000_000);
    let text = snapshot().render();
    let large_at = text.find("ledger_test_render_large").unwrap();
    let small_at = text.find("ledger_test_render_small").unwrap();
    assert!(large_at < small_at);
    drop((small, large));
}
