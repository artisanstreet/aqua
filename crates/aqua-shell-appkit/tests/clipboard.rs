#[cfg(target_os = "macos")]
mod support;

fn main() {
    #[cfg(target_os = "macos")]
    match std::env::var(cases::PEER) {
        Ok(role) => cases::peer(&role),
        Err(_) => support::main(&cases::ALL),
    }
    #[cfg(not(target_os = "macos"))]
    println!("clipboard: skipped; requires macOS");
}

#[cfg(target_os = "macos")]
mod cases {
    use super::support::{self, case, Case};
    use aqua_shell_appkit::services::{
        app_clipboard, Clipboard, ClipboardBackend, ClipboardImage, PasteboardName,
    };
    use aqua_shell_appkit::testing::pasteboard;
    use image::ImageEncoder;
    use std::path::PathBuf;
    use std::process::Command;

    pub const PEER: &str = "AQUA_PASTEBOARD_PEER";
    const ARGUMENT: &str = "AQUA_PASTEBOARD_PEER_ARGUMENT";

    pub const ALL: [Case; 4] = [
        case("copied_text_is_read_back_by_another_process_and_back", text),
        case(
            "an_image_round_trips_through_another_process_as_straight_rgba",
            image_round_trip,
        ),
        case("a_tiff_only_image_from_another_app_is_readable", tiff_only),
        case(
            "copied_files_paste_as_paths_in_another_process_and_back",
            files,
        ),
    ];

    const TEXT: &str = "pasteboard h\u{e9}llo \u{4e16}\u{754c} \u{1f600}";

    fn system() -> Box<dyn Clipboard> {
        let (clipboard, backend) = app_clipboard();
        assert_eq!(backend, ClipboardBackend::System);
        assert!(
            matches!(PasteboardName::from_environment(), PasteboardName::Named(_)),
            "cargo runs tests on the private test pasteboard"
        );
        clipboard
    }

    fn peer_run(role: &str, argument: &str) -> String {
        let exe = std::env::current_exe().expect("the test binary knows its path");
        let output = Command::new(exe)
            .env(PEER, role)
            .env(ARGUMENT, argument)
            .output()
            .expect("the peer process runs");
        let stdout = String::from_utf8(output.stdout).expect("the peer prints UTF-8");
        assert!(
            output.status.success(),
            "peer {role} failed: {stdout}{}",
            String::from_utf8_lossy(&output.stderr)
        );
        stdout.trim_end_matches('\n').to_owned()
    }

    fn pixels() -> ClipboardImage {
        let rgba = vec![
            255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 1, 200, 100, 50, 77, 10, 20, 30, 0, 255,
            255, 255, 255,
        ];
        ClipboardImage::new(3, 2, rgba).expect("six RGBA pixels")
    }

    fn opaque_png() -> Vec<u8> {
        let rgba = [
            255u8, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 40, 108, 220, 255,
        ];
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(&rgba, 2, 2, image::ExtendedColorType::Rgba8)
            .expect("the test PNG encodes");
        bytes
    }

    fn encode(image: &ClipboardImage) -> String {
        format!(
            "{}x{}:{}",
            image.width(),
            image.height(),
            image
                .rgba()
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(",")
        )
    }

    fn same_files(printed: &str, expected: &[PathBuf]) -> bool {
        use std::os::unix::fs::MetadataExt;
        let identity = |path: &std::path::Path| {
            let metadata = std::fs::metadata(path).expect("the pasted path exists");
            (metadata.dev(), metadata.ino())
        };
        let printed: Vec<PathBuf> = printed.lines().map(PathBuf::from).collect();
        printed.len() == expected.len()
            && printed
                .iter()
                .zip(expected)
                .all(|(left, right)| identity(left) == identity(right))
    }

    fn joined(paths: &[PathBuf]) -> String {
        paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn peer(role: &str) {
        let argument = std::env::var(ARGUMENT).unwrap_or_default();
        let mut clipboard = system();
        match role {
            "read-text" => println!(
                "{}",
                clipboard
                    .get_text()
                    .expect("text reads")
                    .unwrap_or_default()
            ),
            "write-text" => clipboard.set_text(argument).expect("text writes"),
            "read-image" => match clipboard.get_image().expect("the image reads") {
                Some(image) => println!("{}", encode(&image)),
                None => println!("no image"),
            },
            "write-tiff" => {
                pasteboard::write_tiff_only(&PasteboardName::from_environment(), &opaque_png())
            }
            "read-files" => match clipboard.get_files().expect("files read") {
                Some(paths) => println!("{}", joined(&paths)),
                None => println!("no files"),
            },
            "write-files" => clipboard
                .set_files(argument.lines().map(PathBuf::from).collect())
                .expect("files write"),
            other => panic!("no peer role {other}"),
        }
    }

    fn text() {
        if !support::appkit() {
            return;
        }
        let mut clipboard = system();
        clipboard.set_text(TEXT.into()).expect("the text is copied");
        assert_eq!(peer_run("read-text", ""), TEXT);
        peer_run("write-text", "from the peer");
        assert_eq!(
            clipboard.get_text().expect("the text reads"),
            Some("from the peer".into())
        );
        clipboard.clear().expect("the pasteboard clears");
        assert_eq!(peer_run("read-text", ""), "");
    }

    fn image_round_trip() {
        if !support::appkit() {
            return;
        }
        let mut clipboard = system();
        clipboard.set_image(pixels()).expect("the image is copied");
        assert_eq!(
            peer_run("read-image", ""),
            encode(&pixels()),
            "translucent pixels keep their straight colour"
        );
        assert_eq!(
            clipboard.get_text().expect("text reads"),
            None,
            "copying an image replaces the text"
        );
    }

    fn tiff_only() {
        if !support::appkit() {
            return;
        }
        let mut clipboard = system();
        peer_run("write-tiff", "");
        let image = clipboard
            .get_image()
            .expect("the TIFF reads")
            .expect("a TIFF counts as an image");
        let expected = image::load_from_memory(&opaque_png())
            .expect("the test PNG decodes")
            .to_rgba8()
            .into_raw();
        assert_eq!((image.width(), image.height()), (2, 2));
        assert_eq!(image.rgba(), expected.as_slice());
    }

    fn files() {
        if !support::appkit() {
            return;
        }
        let root = std::env::temp_dir().join(format!("aqua-pasteboard-{}", std::process::id()));
        std::fs::create_dir_all(root.join("folder")).expect("the temp folder exists");
        std::fs::write(root.join("notes \u{e9}.txt"), "notes").expect("the temp file exists");
        let paths = vec![root.join("notes \u{e9}.txt"), root.join("folder")];
        let mut clipboard = system();
        clipboard
            .set_files(paths.clone())
            .expect("the files are copied");
        let pasted = peer_run("read-files", "");
        assert!(
            same_files(&pasted, &paths),
            "the other process pastes the same files: {pasted:?}"
        );
        let back = vec![root.join("folder")];
        peer_run("write-files", &joined(&back));
        assert_eq!(clipboard.get_files().expect("files read"), Some(back));
        std::fs::remove_dir_all(&root).expect("the temp folder is removed");
    }
}
