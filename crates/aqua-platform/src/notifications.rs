use std::collections::HashSet;
use std::hash::{Hash, Hasher};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppIdentity {
    identifier: String,
    name: String,
}

impl AppIdentity {
    pub fn new(
        identifier: impl Into<String>,
        name: impl Into<String>,
    ) -> Result<Self, NotificationError> {
        let identifier = identifier.into();
        let name = name.into();
        if identifier.is_empty()
            || identifier.len() > 128
            || !identifier
                .bytes()
                .all(|value| value.is_ascii_alphanumeric() || b"._-".contains(&value))
            || name.trim().is_empty()
            || name.chars().any(char::is_control)
        {
            return Err(NotificationError::InvalidIdentity);
        }
        Ok(Self { identifier, name })
    }

    pub fn identifier(&self) -> &str {
        &self.identifier
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotificationAction {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notification {
    pub tag: String,
    pub title: String,
    pub body: String,
    pub actions: Vec<NotificationAction>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotificationResponse {
    pub tag: String,
    pub action_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotificationError {
    InvalidIdentity,
    InvalidContent,
    TooManyActive,
    Unsupported,
    Native(String),
}

impl std::fmt::Display for NotificationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "system notification error: {self:?}")
    }
}

impl std::error::Error for NotificationError {}

impl Notification {
    pub fn xml(&self) -> Result<String, NotificationError> {
        let valid_text = |value: &str| {
            value
                .chars()
                .all(|value| !value.is_control() || matches!(value, '\n' | '\r' | '\t'))
        };
        let mut ids = HashSet::new();
        if self.tag.is_empty()
            || self.title.trim().is_empty()
            || ![&self.tag, &self.title, &self.body]
                .into_iter()
                .all(|value| valid_text(value))
            || self.actions.len() > 5
            || self.actions.iter().any(|action| {
                action.id.is_empty()
                    || action.label.trim().is_empty()
                    || !valid_text(&action.id)
                    || !valid_text(&action.label)
                    || !ids.insert(&action.id)
            })
        {
            return Err(NotificationError::InvalidContent);
        }
        let mut writer = xmlwriter::XmlWriter::new(xmlwriter::Options {
            indent: xmlwriter::Indent::None,
            ..Default::default()
        });
        writer.start_element("toast");
        writer.start_element("visual");
        writer.start_element("binding");
        writer.write_attribute("template", "ToastGeneric");
        for text in [&self.title, &self.body] {
            writer.start_element("text");
            writer.write_text(&xml_content(text));
            writer.end_element();
        }
        writer.end_element();
        writer.end_element();
        if !self.actions.is_empty() {
            writer.start_element("actions");
            for action in &self.actions {
                writer.start_element("action");
                writer.write_attribute("content", &xml_content(&action.label));
                writer.write_attribute("arguments", &xml_content(&action.id));
                writer.end_element();
            }
            writer.end_element();
        }
        writer.start_element("audio");
        writer.write_attribute("silent", "true");
        writer.end_element();
        writer.end_element();
        Ok(writer.end_document())
    }

    pub fn native_tag(&self) -> String {
        native_tag(&self.tag)
    }
}

fn xml_content(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\"', "&quot;")
        .replace('\r', "&#13;")
        .replace('\n', "&#10;")
        .replace('\t', "&#9;")
}

fn native_tag(tag: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    tag.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

pub type ResponseHandler = std::sync::Arc<dyn Fn(NotificationResponse) + Send + Sync>;

#[cfg(windows)]
mod native;

#[cfg(windows)]
pub use native::NotificationCenter;

#[cfg(not(windows))]
pub struct NotificationCenter;

#[cfg(not(windows))]
impl NotificationCenter {
    pub fn new(_: AppIdentity, _: ResponseHandler) -> Result<Self, NotificationError> {
        Err(NotificationError::Unsupported)
    }

    pub fn show(&mut self, _: Notification) -> Result<(), NotificationError> {
        Err(NotificationError::Unsupported)
    }

    pub fn withdraw(&mut self, _: &str) -> Result<(), NotificationError> {
        Err(NotificationError::Unsupported)
    }
}
