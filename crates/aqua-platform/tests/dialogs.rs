use aqua_platform::dialogs::{DialogError, DialogRequest};

#[test]
fn single_result_api_cannot_silently_drop_multiple_selected_files() {
    assert_eq!(
        DialogRequest::open_files().execute(),
        Err(DialogError::MultipleSelectionRequiresMulti)
    );
    assert_eq!(
        DialogRequest::default().execute_multi(),
        Err(DialogError::NoKind)
    );
}
