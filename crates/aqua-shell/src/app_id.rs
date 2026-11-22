const MAX_LENGTH: usize = 255;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AppId(String);

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AppIdError {
    #[error("app id {id:?} is {length} bytes long; the limit is {MAX_LENGTH}")]
    TooLong { id: String, length: usize },
    #[error("app id {id:?} needs at least two dot-separated elements, like com.example.App")]
    SingleElement { id: String },
    #[error("app id {id:?} has an empty element")]
    EmptyElement { id: String },
    #[error("app id {id:?} has the element {element:?}, which starts with a digit")]
    LeadingDigit { id: String, element: String },
    #[error(
        "app id {id:?} contains {character:?}; only ASCII letters, digits, '_' and '-' are allowed"
    )]
    InvalidCharacter { id: String, character: char },
}

impl AppId {
    pub fn new(id: impl Into<String>) -> Result<Self, AppIdError> {
        let id = id.into();
        match problem(&id) {
            Some(error) => Err(error),
            None => Ok(Self(id)),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AppId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn problem(id: &str) -> Option<AppIdError> {
    let owned = || id.to_owned();
    let elements: Vec<&str> = id.split('.').collect();
    let invalid = id
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')));
    let leading_digit = elements
        .iter()
        .find(|element| element.starts_with(|c: char| c.is_ascii_digit()));
    match (id.len(), elements.len(), invalid, leading_digit) {
        (length, _, _, _) if length > MAX_LENGTH => Some(AppIdError::TooLong {
            id: owned(),
            length,
        }),
        (_, _, Some(character), _) => Some(AppIdError::InvalidCharacter {
            id: owned(),
            character,
        }),
        (_, count, _, _) if count < 2 => Some(AppIdError::SingleElement { id: owned() }),
        _ if elements.iter().any(|element| element.is_empty()) => {
            Some(AppIdError::EmptyElement { id: owned() })
        }
        (_, _, _, Some(element)) => Some(AppIdError::LeadingDigit {
            id: owned(),
            element: (*element).to_owned(),
        }),
        _ => None,
    }
}
