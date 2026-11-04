use aqua_text::font::select_cluster;

#[test]
fn primary_and_ascii_do_not_probe_fallback() {
    assert_eq!(
        select_cluster("ascii", 2, |_| false, |_, _| true, |_| panic!()),
        None
    );
    assert_eq!(
        select_cluster("é", 2, |_| true, |_, _| true, |_| panic!()),
        None
    );
}

#[test]
fn full_cluster_and_formation_determine_native_first_fallback() {
    let flag = "🇳🇴";
    let covers = |_: usize, _: char| true;
    assert_eq!(
        select_cluster(flag, 2, |_| false, covers, |index| index == 0),
        Some(0)
    );
    assert_eq!(
        select_cluster(flag, 2, |_| false, covers, |index| index == 1),
        Some(1)
    );
    assert_eq!(
        select_cluster(flag, 2, |_| false, covers, |_| false),
        Some(0)
    );
    assert_eq!(
        select_cluster("🎉", 2, |_| false, covers, |_| panic!()),
        Some(0)
    );
}

#[test]
fn partial_coverage_keeps_cluster_whole_and_uses_first_scalar_rule() {
    let cluster = "👩\u{200d}👧";
    let covers = |index: usize, ch: char| index == 1 && ch == '👩';
    assert_eq!(
        select_cluster(cluster, 2, |_| false, covers, |_| panic!()),
        Some(1)
    );
    assert_eq!(
        select_cluster(cluster, 2, |ch| ch == '👩', covers, |_| panic!()),
        None
    );
    assert_eq!(
        select_cluster(cluster, 2, |_| false, |_, _| false, |_| panic!()),
        None
    );
}
