#![cfg(target_os = "linux")]

mod support;

use aqua_portal::open::{OpenError, OpenTarget, Origin};
use support::{desktop, file_manager, next, Desktop, Event};

fn scratch_file(bus: &aqua_portal::testing::PrivateBus, name: &str) -> std::path::PathBuf {
    let path = bus.dir().join(name);
    std::fs::write(&path, b"aqua").unwrap();
    std::fs::canonicalize(path).unwrap()
}

#[test]
fn opening_a_url_calls_open_uri_with_the_exported_parent() {
    let Some(bus) = support::bus() else { return };
    let (_portal, events) = desktop(
        &bus,
        Desktop {
            open_uri: Some(true),
            ..Desktop::default()
        },
    );
    let origin = Origin::wayland("exported-7").with_activation_token("token-3");
    OpenTarget::url("https://example.com/a?b=c")
        .unwrap()
        .execute_on(&bus.bus(), &origin)
        .unwrap();
    assert_eq!(
        next(&events),
        Event::OpenUri {
            parent: "wayland:exported-7".into(),
            uri: "https://example.com/a?b=c".into(),
            token: Some("token-3".into()),
        }
    );
}

#[test]
fn opening_a_path_passes_a_descriptor_for_that_file() {
    let Some(bus) = support::bus() else { return };
    let (_portal, events) = desktop(
        &bus,
        Desktop {
            open_uri: Some(true),
            ..Desktop::default()
        },
    );
    let file = scratch_file(&bus, "notes.txt");
    OpenTarget::path(&file)
        .unwrap()
        .execute_on(&bus.bus(), &Origin::default())
        .unwrap();
    assert_eq!(
        next(&events),
        Event::OpenFile {
            parent: String::new(),
            path: file,
        }
    );
}

#[test]
fn revealing_a_file_calls_open_directory_with_an_fd() {
    let Some(bus) = support::bus() else { return };
    let (_portal, events) = desktop(
        &bus,
        Desktop {
            open_uri: Some(true),
            ..Desktop::default()
        },
    );
    let file = scratch_file(&bus, "report.pdf");
    OpenTarget::reveal(&file)
        .unwrap()
        .execute_on(&bus.bus(), &Origin::wayland("parent-1"))
        .unwrap();
    assert_eq!(
        next(&events),
        Event::OpenDirectory {
            parent: "wayland:parent-1".into(),
            path: file,
        }
    );
}

#[test]
fn without_open_directory_revealing_asks_the_file_manager_to_show_the_item() {
    let Some(bus) = support::bus() else { return };
    let (_portal, portal_events) = desktop(
        &bus,
        Desktop {
            open_uri: Some(false),
            ..Desktop::default()
        },
    );
    let (_manager, events) = file_manager(&bus);
    let file = scratch_file(&bus, "with space.txt");
    OpenTarget::reveal(&file)
        .unwrap()
        .execute_on(&bus.bus(), &Origin::default())
        .unwrap();
    assert_eq!(
        next(&events),
        Event::ShowItems {
            uris: vec![url::Url::from_file_path(&file).unwrap().to_string()],
            startup: String::new(),
        }
    );
    assert!(portal_events.try_recv().is_err());
}

#[test]
fn without_any_desktop_service_opening_and_revealing_report_unsupported() {
    let Some(bus) = support::bus() else { return };
    let file = scratch_file(&bus, "lonely.txt");
    let session = bus.bus();
    for target in [
        OpenTarget::url("https://example.com").unwrap(),
        OpenTarget::path(&file).unwrap(),
        OpenTarget::reveal(&file).unwrap(),
    ] {
        assert!(matches!(
            target.execute_on(&session, &Origin::default()),
            Err(OpenError::Unsupported(_))
        ));
    }
}
