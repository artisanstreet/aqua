use aqua_shell_appkit::model::presence::{Change, Occlusion, Presence, Reports};

fn run(changes: &[Change]) -> Vec<Reports> {
    changes
        .iter()
        .scan(Presence::Hidden, |presence, change| {
            let (next, reports) = presence.apply(*change);
            *presence = next;
            Some(reports)
        })
        .collect()
}

fn covered(value: bool) -> Reports {
    Reports {
        covered: Some(value),
        minimized: None,
    }
}

#[test]
fn hiding_and_showing_a_window_covers_and_uncovers_it_once() {
    let reports = run(&[
        Change::Shown(Occlusion::Covered),
        Change::Occluded(Occlusion::Exposed),
        Change::Hidden,
        Change::Occluded(Occlusion::Covered),
        Change::Shown(Occlusion::Exposed),
    ]);
    assert_eq!(
        reports,
        vec![
            Reports::default(),
            covered(false),
            covered(true),
            Reports::default(),
            covered(false),
        ]
    );
}

#[test]
fn a_minimized_window_stays_covered_until_it_is_restored_on_screen() {
    let reports = run(&[
        Change::Shown(Occlusion::Exposed),
        Change::Miniaturized,
        Change::Occluded(Occlusion::Exposed),
        Change::Deminiaturized(Occlusion::Exposed),
    ]);
    assert_eq!(
        reports,
        vec![
            covered(false),
            Reports {
                covered: Some(true),
                minimized: Some(true),
            },
            Reports::default(),
            Reports {
                covered: Some(false),
                minimized: Some(false),
            },
        ]
    );
}
