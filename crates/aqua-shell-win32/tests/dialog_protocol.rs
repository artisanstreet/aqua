use aqua_platform::dialogs::{DialogKind, FileFilter};
use aqua_shell_win32::dialog_protocol::*;
use std::path::PathBuf;

fn sample() -> HelperRequest {
    HelperRequest {
        kind: DialogKind::SaveFile,
        title: Some("Save\tas\nnow \\ done".into()),
        start_dir: Some(PathBuf::from("C:\\Users\\a b")),
        file_name: Some("untitled.txt".into()),
        filters: vec![
            FileFilter::new("Images", &["png", "jpg"]),
            FileFilter::new("Any", &[]),
        ],
        owner: Some(65934),
    }
}

#[test]
fn request_round_trips_with_hostile_text() {
    let request = sample();
    assert_eq!(decode_request(&encode_request(&request)), Ok(request));
}

#[test]
fn request_without_kind_is_rejected() {
    assert_eq!(decode_request("title\tx"), Err(DecodeError::MissingKind));
    assert_eq!(
        decode_request("kind\tbogus"),
        Err(DecodeError::UnknownKind("bogus".into()))
    );
}

#[test]
fn response_round_trips_paths_and_errors() {
    let selected = HelperResponse::Selected(vec![
        PathBuf::from("C:\\a\\b.txt"),
        PathBuf::from("D:\\x\ty"),
    ]);
    assert_eq!(decode_response(&encode_response(&selected)), Ok(selected));
    let none = HelperResponse::Selected(vec![]);
    assert_eq!(decode_response(&encode_response(&none)), Ok(none));
    let failed = HelperResponse::Failed("no\nparent".into());
    assert_eq!(decode_response(&encode_response(&failed)), Ok(failed));
    assert_eq!(decode_response(""), Err(DecodeError::EmptyResponse));
}
