use aqua_protocol::{DialogFilter, DialogKind, DialogSpec};
use aqua_shell_win32::dialog_protocol::*;
use std::path::PathBuf;

fn filter(label: &str, extensions: &[&str]) -> DialogFilter {
    DialogFilter {
        label: label.into(),
        extensions: extensions.iter().map(|e| e.to_string()).collect(),
    }
}

fn sample() -> HelperRequest {
    HelperRequest {
        kind: HelperKind::SaveFile,
        title: Some("Save\tas\nnow \\ done".into()),
        start_dir: Some(PathBuf::from("C:\\Users\\a b")),
        file_name: Some("untitled.txt".into()),
        filters: vec![filter("Images", &["png", "jpg"]), filter("Any", &[])],
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

#[test]
fn spec_maps_to_helper_kinds_and_drops_empty_names() {
    let spec = |kind| DialogSpec {
        kind,
        title: String::new(),
        filters: vec![],
    };
    let open = HelperRequest::from_spec(&spec(DialogKind::OpenFile { multiple: false }), None);
    assert_eq!((open.kind, open.title), (HelperKind::OpenFile, None));
    let many = HelperRequest::from_spec(&spec(DialogKind::OpenFile { multiple: true }), Some(7));
    assert_eq!((many.kind, many.owner), (HelperKind::OpenFiles, Some(7)));
    let save = HelperRequest::from_spec(
        &spec(DialogKind::SaveFile {
            suggested_name: "a.txt".into(),
        }),
        None,
    );
    assert_eq!(
        (save.kind, save.file_name.as_deref()),
        (HelperKind::SaveFile, Some("a.txt"))
    );
    let blank = HelperRequest::from_spec(
        &spec(DialogKind::SaveFile {
            suggested_name: String::new(),
        }),
        None,
    );
    assert_eq!(blank.file_name, None);
    assert_eq!(
        HelperRequest::from_spec(&spec(DialogKind::PickFolder), None).kind,
        HelperKind::PickFolder
    );
}
