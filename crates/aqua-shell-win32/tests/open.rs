use aqua_shell_win32::open::{OpenError, OpenTarget};

#[test]
fn external_urls_are_normalized_and_keep_arguments_as_url_data() {
    let target = OpenTarget::url("HTTPS://Example.COM/a%20b?q=%22%26%20calc.exe").unwrap();
    assert_eq!(
        target,
        OpenTarget::url("https://example.com/a%20b?q=%22%26%20calc.exe").unwrap()
    );
    assert!(OpenTarget::url("mailto:help@example.com?subject=A%26B").is_ok());
    for value in [
        "javascript:alert(1)",
        "data:text/plain,hello",
        "file:///C:/Windows/test.exe",
        "artisan://thread/one",
    ] {
        assert!(matches!(
            OpenTarget::url(value),
            Err(OpenError::UnsupportedScheme(_))
        ));
    }
    for value in [
        "",
        "not a URL",
        "https://example.com/\0file",
        "https://example.com/\n",
    ] {
        assert!(matches!(OpenTarget::url(value), Err(OpenError::InvalidUrl)));
    }
}

#[test]
fn file_targets_resolve_existing_paths_at_the_boundary() {
    let directory = std::env::current_dir().unwrap();
    assert_eq!(
        OpenTarget::path(&directory).unwrap(),
        OpenTarget::path(std::path::Path::new(".")).unwrap()
    );
    assert!(matches!(
        OpenTarget::path(std::path::Path::new("")),
        Err(OpenError::InvalidPath)
    ));
    assert!(matches!(
        OpenTarget::reveal(&directory.join("aqua-nonexistent-shell-target")),
        Err(OpenError::Io(_))
    ));
}
