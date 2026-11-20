use aqua_shell::{AppId, AppIdError};

#[test]
fn app_ids_follow_the_desktop_file_naming_rules() {
    assert_eq!(
        AppId::new("com.artisanstreet.editor").unwrap().as_str(),
        "com.artisanstreet.editor"
    );
    assert!(AppId::new("org.example.My_App-2").is_ok());
    assert!(matches!(
        AppId::new("editor"),
        Err(AppIdError::SingleElement { .. })
    ));
    assert!(matches!(
        AppId::new("com..editor"),
        Err(AppIdError::EmptyElement { .. })
    ));
    assert!(matches!(
        AppId::new("com.artisanstreet."),
        Err(AppIdError::EmptyElement { .. })
    ));
    assert!(matches!(
        AppId::new("com.9patch.editor"),
        Err(AppIdError::LeadingDigit { element, .. }) if element == "9patch"
    ));
    assert!(matches!(
        AppId::new("com.artisan street.editor"),
        Err(AppIdError::InvalidCharacter { character: ' ', .. })
    ));
    assert!(matches!(
        AppId::new("com.artisanstreet.redigér"),
        Err(AppIdError::InvalidCharacter {
            character: 'é', ..
        })
    ));
    assert!(matches!(
        AppId::new(format!("com.{}", "a".repeat(252))),
        Err(AppIdError::TooLong { length: 256, .. })
    ));
}
