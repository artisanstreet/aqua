use aqua_shell::open::{ExternalTarget, OpenError};

#[test]
fn external_urls_are_normalized_and_keep_arguments_as_url_data() {
    let target = ExternalTarget::url("HTTPS://Example.COM/a%20b?q=%22%26%20calc.exe").unwrap();
    assert_eq!(
        target,
        ExternalTarget::url("https://example.com/a%20b?q=%22%26%20calc.exe").unwrap()
    );
    assert!(ExternalTarget::url("mailto:help@example.com?subject=A%26B").is_ok());
    for value in [
        "javascript:alert(1)",
        "data:text/plain,hello",
        "file:///C:/Windows/test.exe",
        "artisan://thread/one",
    ] {
        assert!(matches!(
            ExternalTarget::url(value),
            Err(OpenError::UnsupportedScheme(_))
        ));
    }
    for value in [
        "",
        "not a URL",
        "https://example.com/\0file",
        "https://example.com/\n",
    ] {
        assert!(matches!(
            ExternalTarget::url(value),
            Err(OpenError::InvalidUrl)
        ));
    }
}

#[test]
fn file_targets_resolve_existing_paths_at_the_boundary() {
    let directory = std::env::current_dir().unwrap();
    assert_eq!(
        ExternalTarget::path(&directory).unwrap(),
        ExternalTarget::path(std::path::Path::new(".")).unwrap()
    );
    assert!(matches!(
        ExternalTarget::path(std::path::Path::new("")),
        Err(OpenError::InvalidPath)
    ));
    assert!(matches!(
        ExternalTarget::reveal(&directory.join("aqua-nonexistent-shell-target")),
        Err(OpenError::Io(_))
    ));
}
