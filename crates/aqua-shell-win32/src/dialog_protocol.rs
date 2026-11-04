use aqua_platform::dialogs::{DialogKind, DialogRequest, FileFilter};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperRequest {
    pub kind: DialogKind,
    pub title: Option<String>,
    pub start_dir: Option<PathBuf>,
    pub file_name: Option<String>,
    pub filters: Vec<FileFilter>,
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
    pub fn new(request: &DialogRequest, owner: Option<isize>) -> Option<Self> {
        Some(Self {
            kind: request.kind?,
            title: request.title.clone(),
            start_dir: request.start_dir.clone(),
            file_name: request.file_name.clone(),
            filters: request.filters.clone(),
            owner,
        })
    }

    pub fn dialog(&self) -> DialogRequest {
        DialogRequest {
            kind: Some(self.kind),
            title: self.title.clone(),
            start_dir: self.start_dir.clone(),
            file_name: self.file_name.clone(),
            filters: self.filters.clone(),
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

fn kind_name(kind: DialogKind) -> &'static str {
    match kind {
        DialogKind::OpenFile => "open-file",
        DialogKind::OpenFiles => "open-files",
        DialogKind::SaveFile => "save-file",
        DialogKind::PickFolder => "pick-folder",
    }
}

fn kind_from(name: &str) -> Result<DialogKind, DecodeError> {
    match name {
        "open-file" => Ok(DialogKind::OpenFile),
        "open-files" => Ok(DialogKind::OpenFiles),
        "save-file" => Ok(DialogKind::SaveFile),
        "pick-folder" => Ok(DialogKind::PickFolder),
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
            escape(&filter.name),
            escape(&filter.extensions.join(","))
        )
    }));
    lines.join("\n")
}

pub fn decode_request(text: &str) -> Result<HelperRequest, DecodeError> {
    let mut kind = None;
    let mut request = HelperRequest {
        kind: DialogKind::OpenFile,
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
                request.filters.push(FileFilter {
                    name: first,
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
