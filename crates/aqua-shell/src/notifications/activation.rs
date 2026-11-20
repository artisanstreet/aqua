use super::{AppIdentity, NotificationResponse};
use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};

pub const SCHEME: &str = "aqua-toast";
pub const ACTIVATION_FLAG: &str = "--aqua-toast-activation";
const AUTHORITY: &str = "app";
const UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Activation {
    pub identifier: String,
    pub response: NotificationResponse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivationUrlError {
    WrongScheme,
    Malformed,
}

fn encode(segment: &str) -> String {
    utf8_percent_encode(segment, UNRESERVED).to_string()
}

fn decode(segment: &str) -> Result<String, ActivationUrlError> {
    percent_decode_str(segment)
        .decode_utf8()
        .map(|text| text.into_owned())
        .map_err(|_| ActivationUrlError::Malformed)
}

pub fn activation_url(identity: &AppIdentity, tag: &str, action: Option<&str>) -> String {
    let mut url = format!(
        "{SCHEME}://{AUTHORITY}/{}/{}",
        encode(identity.identifier()),
        encode(tag)
    );
    if let Some(action) = action {
        url.push('/');
        url.push_str(&encode(action));
    }
    url
}

pub fn parse_activation_url(text: &str) -> Result<Activation, ActivationUrlError> {
    let rest = text
        .strip_prefix(SCHEME)
        .and_then(|rest| rest.strip_prefix("://"))
        .ok_or(ActivationUrlError::WrongScheme)?;
    let path = rest
        .strip_prefix(AUTHORITY)
        .and_then(|path| path.strip_prefix('/'))
        .ok_or(ActivationUrlError::Malformed)?;
    let segments: Vec<&str> = path.trim_end_matches('/').split('/').collect();
    match segments.as_slice() {
        [identifier, tag] if !identifier.is_empty() && !tag.is_empty() => Ok(Activation {
            identifier: decode(identifier)?,
            response: NotificationResponse {
                tag: decode(tag)?,
                action_id: None,
            },
        }),
        [identifier, tag, action]
            if !identifier.is_empty() && !tag.is_empty() && !action.is_empty() =>
        {
            Ok(Activation {
                identifier: decode(identifier)?,
                response: NotificationResponse {
                    tag: decode(tag)?,
                    action_id: Some(decode(action)?),
                },
            })
        }
        _ => Err(ActivationUrlError::Malformed),
    }
}
