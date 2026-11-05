use std::collections::BTreeSet;

use super::{decode, encode};

fn names(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

#[test]
fn framed_cache_round_trips_the_used_set_and_the_library() {
    let used = names(&["Quad/91", "Slug/91", "Shadow/91"]);
    let framed = encode(&used, &[9, 8, 7]);
    let (decoded, library) = decode(Some(&framed));
    assert_eq!(decoded, Some(used));
    assert_eq!(library, Some(&[9u8, 8, 7][..]));
}

#[test]
fn unframed_blob_is_a_bare_library_with_no_hint() {
    let (hint, library) = decode(Some(&[1, 2, 3, 4, 5]));
    assert_eq!(hint, None);
    assert_eq!(library, Some(&[1u8, 2, 3, 4, 5][..]));
    assert_eq!(decode(None), (None, None));
}

#[test]
fn truncated_frame_degrades_to_a_bare_library() {
    let mut framed = encode(&names(&["Quad/91"]), &[1]);
    framed.truncate(10);
    let (hint, library) = decode(Some(&framed));
    assert_eq!(hint, None);
    assert_eq!(library, Some(&framed[..]));
}

#[test]
fn empty_used_set_gives_no_hint_so_everything_is_warmed() {
    let framed = encode(&BTreeSet::new(), &[5, 5]);
    let (hint, library) = decode(Some(&framed));
    assert_eq!(hint, None);
    assert_eq!(library, Some(&[5u8, 5][..]));
}
