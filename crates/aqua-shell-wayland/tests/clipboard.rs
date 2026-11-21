#![cfg(target_os = "linux")]

mod support;

use aqua_shell_wayland::services::{
    primary_selection, Clipboard, ClipboardError, ClipboardImage, SystemClipboard,
};
use aqua_shell_wayland::testing::{Compositor, DataControl, Reply, Selection, VirtualKeyboard};
use aqua_shell_wayland::Window;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;
use support::{drive, options, ready, Flow};

const UTF8: &str = "text/plain;charset=utf-8";
const TICK: Duration = Duration::from_millis(10);

struct Session {
    compositor: Rc<Compositor>,
    window: Window,
    peer: DataControl,
    _keyboard: VirtualKeyboard,
    app: &'static str,
}

fn session(app: &'static str) -> Option<Session> {
    let compositor = support::compositor()?;
    let keyboard = VirtualKeyboard::with_layout(&compositor, "us", "").unwrap();
    let peer = DataControl::connect(&compositor).unwrap();
    let window = Window::create(&options(app, (320.0, 240.0))).unwrap();
    Some(Session {
        compositor,
        window,
        peer,
        _keyboard: keyboard,
        app,
    })
}

fn answered(reply: &Reply) -> Option<Option<Vec<u8>>> {
    reply
        .try_recv()
        .ok()
        .map(|answer| answer.expect("the peer reads the selection"))
}

fn copied(result: Result<(), ClipboardError>) -> bool {
    match result {
        Ok(()) => true,
        Err(ClipboardError::Unavailable("no input serial")) => false,
        Err(error) => panic!("copying failed: {error}"),
    }
}

fn png(image: &ClipboardImage) -> Vec<u8> {
    let mut out = Vec::new();
    image::ImageEncoder::write_image(
        image::codecs::png::PngEncoder::new(&mut out),
        image.rgba(),
        image.width() as u32,
        image.height() as u32,
        image::ExtendedColorType::Rgba8,
    )
    .unwrap();
    out
}

fn sample_image() -> ClipboardImage {
    let rgba = [
        [255, 0, 0, 255],
        [0, 255, 0, 128],
        [0, 0, 255, 0],
        [17, 34, 51, 68],
        [200, 100, 50, 25],
        [1, 2, 3, 4],
    ];
    ClipboardImage::new(3, 2, rgba.concat()).unwrap()
}

fn copy_then_peer_reads(
    app: &'static str,
    which: Selection,
    mime: &'static str,
    mut copy: impl FnMut() -> Result<(), ClipboardError> + 'static,
) -> Option<(Vec<String>, Vec<u8>)> {
    let session = session(app)?;
    let outcome = Rc::new(RefCell::new(None));
    let result = outcome.clone();
    let Session {
        compositor,
        window,
        peer,
        ..
    } = session;
    let mut reply = None;
    drive(&[window], Duration::from_secs(10), move |_| {
        if ready(&compositor, app).is_none() {
            return Flow::Wait(TICK);
        }
        match &reply {
            None if copied(copy()) => reply = Some(peer.read(which, mime)),
            None => {}
            Some(pending) => {
                if let Some(bytes) = answered(pending) {
                    match bytes {
                        Some(bytes) => {
                            *result.borrow_mut() =
                                Some((peer.offered(which).unwrap_or_default(), bytes));
                            return Flow::Done;
                        }
                        None => reply = Some(peer.read(which, mime)),
                    }
                }
            }
        }
        Flow::Wait(TICK)
    });
    outcome.take()
}

fn peer_offers_then_we_read<T: 'static>(
    app: &'static str,
    which: Selection,
    entries: Vec<(String, Vec<u8>)>,
    mut read: impl FnMut() -> Option<T> + 'static,
) -> Option<T> {
    let Session {
        compositor,
        window,
        peer,
        _keyboard,
        ..
    } = session(app)?;
    let outcome = Rc::new(RefCell::new(None));
    let result = outcome.clone();
    let mut offered = false;
    drive(&[window], Duration::from_secs(10), move |_| {
        match (offered, ready(&compositor, app).is_some()) {
            (false, true) => {
                peer.offer(which, entries.clone());
                offered = true;
            }
            (true, _) => {
                if let Some(value) = read() {
                    *result.borrow_mut() = Some(value);
                    return Flow::Done;
                }
            }
            (false, false) => {}
        }
        Flow::Wait(TICK)
    });
    outcome.take()
}

#[test]
fn copied_text_is_readable_by_another_client() {
    let mut clipboard = None;
    let Some((mimes, bytes)) = copy_then_peer_reads(
        "com.aqua.test.copy",
        Selection::Clipboard,
        UTF8,
        move || {
            clipboard
                .get_or_insert_with(|| SystemClipboard::new().unwrap())
                .set_text("Grüße fra Aqua ✓".into())
        },
    ) else {
        return;
    };
    assert_eq!(String::from_utf8(bytes).unwrap(), "Grüße fra Aqua ✓");
    for mime in [UTF8, "UTF8_STRING", "text/plain"] {
        assert!(
            mimes.iter().any(|offered| offered == mime),
            "{mime} in {mimes:?}"
        );
    }
}

#[test]
fn text_copied_by_another_client_is_pasted() {
    let mut clipboard = None;
    let Some(text) = peer_offers_then_we_read(
        "com.aqua.test.paste",
        Selection::Clipboard,
        vec![(UTF8.into(), "pasted 日本語 text".as_bytes().to_vec())],
        move || {
            clipboard
                .get_or_insert_with(|| SystemClipboard::new().unwrap())
                .get_text()
                .unwrap()
                .filter(|text| !text.is_empty())
        },
    ) else {
        return;
    };
    assert_eq!(text, "pasted 日本語 text");
}

#[test]
fn reading_our_own_selection_never_waits_on_the_compositor() {
    let Some(Session {
        compositor,
        window,
        peer,
        app,
        _keyboard,
    }) = session("com.aqua.test.own")
    else {
        return;
    };
    let reads = Rc::new(RefCell::new(Vec::new()));
    let timed = reads.clone();
    let mut clipboard = SystemClipboard::new().unwrap();
    let mut copied_at_all = false;
    drive(&[window], Duration::from_secs(10), move |_| {
        if ready(&compositor, app).is_none() {
            return Flow::Wait(TICK);
        }
        if !copied_at_all {
            copied_at_all = copied(clipboard.set_text("mine".into()));
            return Flow::Wait(TICK);
        }
        let started = std::time::Instant::now();
        let text = clipboard.get_text().unwrap();
        timed.borrow_mut().push((text, started.elapsed()));
        let echoed = peer
            .offered(Selection::Clipboard)
            .is_some_and(|mimes| mimes.iter().any(|mime| mime == UTF8));
        match echoed && timed.borrow().len() > 3 {
            true => Flow::Done,
            false => Flow::Wait(TICK),
        }
    });
    let reads = reads.take();
    assert!(!reads.is_empty());
    for (text, elapsed) in reads {
        assert_eq!(text.as_deref(), Some("mine"));
        assert!(
            elapsed < Duration::from_millis(5),
            "a self read took {elapsed:?}"
        );
    }
}

#[test]
fn a_copy_by_another_client_replaces_our_selection() {
    let Some(Session {
        compositor,
        window,
        peer,
        app,
        _keyboard,
    }) = session("com.aqua.test.replaced")
    else {
        return;
    };
    let seen = Rc::new(RefCell::new(None));
    let result = seen.clone();
    let mut clipboard = SystemClipboard::new().unwrap();
    let mut phase = 0;
    drive(&[window], Duration::from_secs(10), move |_| {
        if ready(&compositor, app).is_none() {
            return Flow::Wait(TICK);
        }
        match phase {
            0 if copied(clipboard.set_text("ours".into())) => phase = 1,
            1 if peer
                .offered(Selection::Clipboard)
                .is_some_and(|mimes| mimes.iter().any(|mime| mime == UTF8)) =>
            {
                peer.offer(
                    Selection::Clipboard,
                    vec![(UTF8.into(), b"theirs".to_vec())],
                );
                phase = 2;
            }
            2 => {
                let text = clipboard.get_text().unwrap();
                if text.as_deref() != Some("ours") {
                    *result.borrow_mut() = text;
                    return Flow::Done;
                }
            }
            _ => {}
        }
        Flow::Wait(TICK)
    });
    assert_eq!(seen.take().as_deref(), Some("theirs"));
}

#[test]
fn a_png_image_round_trips_through_the_clipboard() {
    let mut clipboard = None;
    let Some((mimes, bytes)) = copy_then_peer_reads(
        "com.aqua.test.image.copy",
        Selection::Clipboard,
        "image/png",
        move || {
            clipboard
                .get_or_insert_with(|| SystemClipboard::new().unwrap())
                .set_image(sample_image())
        },
    ) else {
        return;
    };
    assert!(mimes.iter().any(|mime| mime == "image/png"), "{mimes:?}");
    let decoded = image::load_from_memory(&bytes).unwrap().into_rgba8();
    assert_eq!(decoded.dimensions(), (3, 2));
    assert_eq!(decoded.into_raw(), sample_image().rgba());
    let mut clipboard = None;
    let Some(pasted) = peer_offers_then_we_read(
        "com.aqua.test.image.paste",
        Selection::Clipboard,
        vec![("image/png".into(), png(&sample_image()))],
        move || {
            clipboard
                .get_or_insert_with(|| SystemClipboard::new().unwrap())
                .get_image()
                .unwrap()
        },
    ) else {
        return;
    };
    assert_eq!(pasted, sample_image());
}

#[test]
fn copied_files_are_offered_as_a_uri_list() {
    let files = vec![
        PathBuf::from("/tmp/aqua clip/Grüße.txt"),
        PathBuf::from("/tmp/plain.txt"),
    ];
    let mut clipboard = None;
    let copy = files.clone();
    let Some((mimes, bytes)) = copy_then_peer_reads(
        "com.aqua.test.files.copy",
        Selection::Clipboard,
        "text/uri-list",
        move || {
            clipboard
                .get_or_insert_with(|| SystemClipboard::new().unwrap())
                .set_files(copy.clone())
        },
    ) else {
        return;
    };
    assert_eq!(
        String::from_utf8(bytes).unwrap(),
        "file:///tmp/aqua%20clip/Gr%C3%BC%C3%9Fe.txt\r\nfile:///tmp/plain.txt\r\n"
    );
    assert!(
        mimes
            .iter()
            .any(|mime| mime == "x-special/gnome-copied-files"),
        "{mimes:?}"
    );
}

#[test]
fn files_copied_by_another_client_are_pasted_as_paths() {
    let mut clipboard = None;
    let Some(files) = peer_offers_then_we_read(
        "com.aqua.test.files.paste",
        Selection::Clipboard,
        vec![(
            "text/uri-list".into(),
            b"# a comment\r\nfile:///tmp/aqua%20drop/%C3%A6%C3%B8%C3%A5.png\r\nhttps://example.com/x\r\nfile:///tmp/b.txt\r\n"
                .to_vec(),
        )],
        move || {
            clipboard
                .get_or_insert_with(|| SystemClipboard::new().unwrap())
                .get_files()
                .unwrap()
        },
    ) else {
        return;
    };
    assert_eq!(
        files,
        vec![
            PathBuf::from("/tmp/aqua drop/æøå.png"),
            PathBuf::from("/tmp/b.txt")
        ]
    );
}

#[test]
fn pasting_with_no_selection_returns_none_without_blocking() {
    let Some(Session {
        compositor,
        window,
        app,
        _keyboard,
        ..
    }) = session("com.aqua.test.empty")
    else {
        return;
    };
    let reads = Rc::new(RefCell::new(Vec::new()));
    let timed = reads.clone();
    let mut clipboard = SystemClipboard::new().unwrap();
    let mut since_ready = 0;
    drive(&[window], Duration::from_secs(10), move |_| {
        if ready(&compositor, app).is_none() {
            return Flow::Wait(TICK);
        }
        since_ready += 1;
        let started = std::time::Instant::now();
        let text = clipboard.get_text().unwrap();
        let image = clipboard.get_image().unwrap();
        timed.borrow_mut().push((text, image, started.elapsed()));
        match since_ready > 5 {
            true => Flow::Done,
            false => Flow::Wait(TICK),
        }
    });
    for (text, image, elapsed) in reads.take() {
        assert_eq!((text, image), (None, None));
        assert!(
            elapsed < Duration::from_millis(50),
            "an empty read took {elapsed:?}"
        );
    }
}

#[test]
fn the_primary_selection_round_trips_with_another_client() {
    let mut primary = None;
    let Some((_, bytes)) = copy_then_peer_reads(
        "com.aqua.test.primary.copy",
        Selection::Primary,
        UTF8,
        move || {
            primary
                .get_or_insert_with(|| primary_selection().unwrap())
                .set_text("selected with the mouse".into())
        },
    ) else {
        return;
    };
    assert_eq!(String::from_utf8(bytes).unwrap(), "selected with the mouse");
    let mut primary = None;
    let Some(text) = peer_offers_then_we_read(
        "com.aqua.test.primary.paste",
        Selection::Primary,
        vec![(UTF8.into(), b"middle-click me".to_vec())],
        move || {
            primary
                .get_or_insert_with(|| primary_selection().unwrap())
                .get_text()
                .unwrap()
        },
    ) else {
        return;
    };
    assert_eq!(text, "middle-click me");
}
