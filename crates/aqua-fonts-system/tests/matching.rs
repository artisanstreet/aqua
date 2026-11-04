use aqua_fonts_system::{system_font_directories, weight_distance_order, FontDatabase};

#[test]
fn regular_prefers_medium_then_lighter_then_heavier() {
    assert_eq!(
        weight_distance_order(400, &[700, 300, 500, 400, 200]),
        vec![400, 500, 300, 200, 700]
    );
    assert_eq!(
        weight_distance_order(400, &[700, 300, 200, 900]),
        vec![300, 200, 700, 900]
    );
}

#[test]
fn light_prefers_lighter_and_bold_prefers_heavier() {
    assert_eq!(
        weight_distance_order(300, &[400, 200, 100, 700]),
        vec![200, 100, 400, 700]
    );
    assert_eq!(
        weight_distance_order(700, &[400, 600, 900, 800]),
        vec![800, 900, 600, 400]
    );
}

#[test]
fn system_directories_load_when_present() {
    let database = FontDatabase::system();
    let present = system_font_directories()
        .iter()
        .any(|directory| directory.is_dir());
    assert_eq!(present, database.face_count() > 0);
}
