use aqua_text::overflow::{EllipsisPosition, OverflowCluster, OverflowMode, single_line_overflow};

fn clusters(text: &str, advance: f32) -> Vec<OverflowCluster> {
    text.char_indices()
        .map(|(start, ch)| OverflowCluster {
            bytes: start..start + ch.len_utf8(),
            advance,
        })
        .collect()
}

#[test]
fn end_and_start_keep_source_boundaries() {
    let text = "abcde";
    let shaped = clusters(text, 10.0);
    let end = single_line_overflow(text, &shaped, 35.0, 5.0, OverflowMode::EndEllipsis);
    assert_eq!(end.source, 0..2);
    assert_eq!(end.ellipsis, Some(EllipsisPosition::End));
    let start = single_line_overflow(text, &shaped, 35.0, 5.0, OverflowMode::StartEllipsis);
    assert_eq!(start.source, 3..5);
    assert_eq!(start.ellipsis, Some(EllipsisPosition::Start));
}

#[test]
fn end_trims_punctuation_without_splitting_clusters() {
    let text = "ab, cd";
    let shaped = vec![
        OverflowCluster {
            bytes: 0..2,
            advance: 20.0,
        },
        OverflowCluster {
            bytes: 2..3,
            advance: 10.0,
        },
        OverflowCluster {
            bytes: 3..4,
            advance: 10.0,
        },
        OverflowCluster {
            bytes: 4..6,
            advance: 20.0,
        },
    ];
    let result = single_line_overflow(text, &shaped, 45.0, 5.0, OverflowMode::EndEllipsis);
    assert_eq!(&text[result.source], "ab");
}

#[test]
fn fits_and_clip_leave_original_intact() {
    let text = "éclair";
    let shaped = clusters(text, 4.0);
    for mode in [
        OverflowMode::Clip,
        OverflowMode::StartEllipsis,
        OverflowMode::EndEllipsis,
    ] {
        let width = if mode == OverflowMode::Clip {
            1.0
        } else {
            24.0
        };
        let result = single_line_overflow(text, &shaped, width, 4.0, mode);
        assert_eq!(result.source, 0..text.len());
        assert_eq!(result.ellipsis, None);
    }
}
