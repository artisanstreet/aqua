use aqua_protocol::{DialogFilter, DialogKind, DialogSpec};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperKind {
    OpenFile,
    OpenFiles,
    SaveFile,
    PickFolder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperRequest {
    pub kind: HelperKind,
    pub title: Option<String>,
    pub start_dir: Option<PathBuf>,
    pub file_name: Option<String>,
    pub filters: Vec<DialogFilter>,
    pub owner: Option<isize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelperResponse {
    Selected(Vec<PathBuf>),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    MissingKind,
    UnknownKind(String),
    BadOwner(String),
    UnknownField(String),
    EmptyResponse,
}

impl HelperRequest {
    pub fn from_spec(spec: &DialogSpec, owner: Option<isize>) -> Self {
        let (kind, file_name) = match &spec.kind {
            DialogKind::OpenFile { multiple: false } => (HelperKind::OpenFile, None),
            DialogKind::OpenFile { multiple: true } => (HelperKind::OpenFiles, None),
            DialogKind::SaveFile { suggested_name } => {
                (HelperKind::SaveFile, Some(suggested_name.clone()))
            }
            DialogKind::PickFolder => (HelperKind::PickFolder, None),
        };
        Self {
            kind,
            title: Some(spec.title.clone()).filter(|t| !t.is_empty()),
            start_dir: None,
            file_name: file_name.filter(|n| !n.is_empty()),
            filters: spec.filters.clone(),
            owner,
        }
    }
}

fn escape(text: &str) -> String {
    text.chars()
        .flat_map(|c| match c {
            '\\' => vec!['\\', '\\'],
            '\n' => vec!['\\', 'n'],
            '\r' => vec!['\\', 'r'],
            '\t' => vec!['\\', 't'],
            other => vec![other],
        })
        .collect()
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (c, c == '\\') {
            (_, true) => match chars.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            },
            (c, false) => out.push(c),
        }
    }
    out
}

fn kind_name(kind: HelperKind) -> &'static str {
    match kind {
        HelperKind::OpenFile => "open-file",
        HelperKind::OpenFiles => "open-files",
        HelperKind::SaveFile => "save-file",
        HelperKind::PickFolder => "pick-folder",
    }
}

fn kind_from(name: &str) -> Result<HelperKind, DecodeError> {
    match name {
        "open-file" => Ok(HelperKind::OpenFile),
        "open-files" => Ok(HelperKind::OpenFiles),
        "save-file" => Ok(HelperKind::SaveFile),
        "pick-folder" => Ok(HelperKind::PickFolder),
        other => Err(DecodeError::UnknownKind(other.to_owned())),
    }
}

pub fn encode_request(request: &HelperRequest) -> String {
    let mut lines = vec![format!("kind\t{}", kind_name(request.kind))];
    let optional = [
        ("title", request.title.clone()),
        (
            "start_dir",
            request
                .start_dir
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
        ),
        ("file_name", request.file_name.clone()),
        ("owner", request.owner.map(|o| o.to_string())),
    ];
    lines.extend(
        optional
            .into_iter()
            .filter_map(|(key, value)| value.map(|v| format!("{key}\t{}", escape(&v)))),
    );
    lines.extend(request.filters.iter().map(|filter| {
        format!(
            "filter\t{}\t{}",
            escape(&filter.label),
            escape(&filter.extensions.join(","))
        )
    }));
    lines.join("\n")
}

pub fn decode_request(text: &str) -> Result<HelperRequest, DecodeError> {
    let mut kind = None;
    let mut request = HelperRequest {
        kind: HelperKind::OpenFile,
        title: None,
        start_dir: None,
        file_name: None,
        filters: Vec::new(),
        owner: None,
    };
    for line in text.lines().filter(|l| !l.is_empty()) {
        let mut parts = line.splitn(3, '\t');
        let key = parts.next().unwrap_or_default();
        let first = parts.next().map(unescape).unwrap_or_default();
        match key {
            "kind" => kind = Some(kind_from(&first)?),
            "title" => request.title = Some(first),
            "start_dir" => request.start_dir = Some(PathBuf::from(first)),
            "file_name" => request.file_name = Some(first),
            "owner" => {
                request.owner = Some(
                    first
                        .parse()
                        .map_err(|_| DecodeError::BadOwner(first.clone()))?,
                )
            }
            "filter" => {
                let extensions = parts.next().map(unescape).unwrap_or_default();
                request.filters.push(DialogFilter {
                    label: first,
                    extensions: extensions
                        .split(',')
                        .filter(|e| !e.is_empty())
                        .map(str::to_owned)
                        .collect(),
                });
            }
            other => return Err(DecodeError::UnknownField(other.to_owned())),
        }
    }
    request.kind = kind.ok_or(DecodeError::MissingKind)?;
    Ok(request)
}

pub fn encode_response(response: &HelperResponse) -> String {
    match response {
        HelperResponse::Selected(paths) => std::iter::once("ok".to_owned())
            .chain(paths.iter().map(|p| escape(&p.to_string_lossy())))
            .collect::<Vec<_>>()
            .join("\n"),
        HelperResponse::Failed(reason) => format!("error\n{}", escape(reason)),
    }
}

pub fn decode_response(text: &str) -> Result<HelperResponse, DecodeError> {
    let mut lines = text.lines();
    match lines.next() {
        Some("ok") => Ok(HelperResponse::Selected(
            lines
                .filter(|l| !l.is_empty())
                .map(|l| PathBuf::from(unescape(l)))
                .collect(),
        )),
        Some("error") => Ok(HelperResponse::Failed(
            lines.next().map(unescape).unwrap_or_default(),
        )),
        _ => Err(DecodeError::EmptyResponse),
    }
}
