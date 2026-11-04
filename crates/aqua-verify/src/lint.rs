use std::fs;
use std::path::{Path, PathBuf};

use crate::policy::FORBIDDEN_SOURCE_TOKENS;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub path: PathBuf,
    pub line: usize,
    pub token: &'static str,
}

#[derive(Clone, Copy)]
enum Mode {
    Code,
    LineComment,
    BlockComment(u32),
    Str,
    RawStr(usize),
}

fn blank(ch: char) -> char {
    match ch {
        '\n' => '\n',
        _ => ' ',
    }
}

fn is_identifier_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

fn prior_is_boundary(chars: &[char], index: usize) -> bool {
    index == 0 || !is_identifier_char(chars[index - 1])
}

fn raw_string_opening(chars: &[char], at: usize) -> Option<(usize, usize)> {
    let start = match chars.get(at) {
        Some('r') => at + 1,
        Some('b') if chars.get(at + 1) == Some(&'r') => at + 2,
        _ => return None,
    };
    let hashes = chars[start..].iter().take_while(|c| **c == '#').count();
    (chars.get(start + hashes) == Some(&'"')).then_some((hashes, start + hashes + 1))
}

fn char_literal_len(chars: &[char], at: usize) -> Option<usize> {
    match (chars.get(at + 1), chars.get(at + 2)) {
        (Some('\\'), _) => chars[at + 2..]
            .iter()
            .take(10)
            .position(|c| *c == '\'')
            .map(|offset| offset + 3),
        (Some(c), Some('\'')) if *c != '\'' => Some(3),
        _ => None,
    }
}

pub fn blank_comments_and_strings(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len());
    let mut mode = Mode::Code;
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        let next = chars.get(index + 1).copied();
        match mode {
            Mode::Code => match (ch, next) {
                ('/', Some('/')) => {
                    mode = Mode::LineComment;
                    out.push_str("  ");
                    index += 2;
                }
                ('/', Some('*')) => {
                    mode = Mode::BlockComment(1);
                    out.push_str("  ");
                    index += 2;
                }
                ('"', _) => {
                    mode = Mode::Str;
                    out.push(' ');
                    index += 1;
                }
                ('\'', _) => match char_literal_len(&chars, index) {
                    Some(len) => {
                        out.extend(std::iter::repeat_n(' ', len));
                        index += len;
                    }
                    None => {
                        out.push(ch);
                        index += 1;
                    }
                },
                ('r' | 'b', _) if prior_is_boundary(&chars, index) => {
                    match raw_string_opening(&chars, index) {
                        Some((hashes, body_start)) => {
                            mode = Mode::RawStr(hashes);
                            out.extend(std::iter::repeat_n(' ', body_start - index));
                            index = body_start;
                        }
                        None => {
                            out.push(ch);
                            index += 1;
                        }
                    }
                }
                _ => {
                    out.push(ch);
                    index += 1;
                }
            },
            Mode::LineComment => {
                out.push(blank(ch));
                index += 1;
                if ch == '\n' {
                    mode = Mode::Code;
                }
            }
            Mode::BlockComment(depth) => match (ch, next) {
                ('/', Some('*')) => {
                    mode = Mode::BlockComment(depth + 1);
                    out.push_str("  ");
                    index += 2;
                }
                ('*', Some('/')) => {
                    mode = match depth {
                        1 => Mode::Code,
                        _ => Mode::BlockComment(depth - 1),
                    };
                    out.push_str("  ");
                    index += 2;
                }
                _ => {
                    out.push(blank(ch));
                    index += 1;
                }
            },
            Mode::Str => match (ch, next) {
                ('\\', Some(escaped)) => {
                    out.push(blank(ch));
                    out.push(blank(escaped));
                    index += 2;
                }
                ('"', _) => {
                    mode = Mode::Code;
                    out.push(' ');
                    index += 1;
                }
                _ => {
                    out.push(blank(ch));
                    index += 1;
                }
            },
            Mode::RawStr(hashes) => {
                let closes = ch == '"'
                    && chars[index + 1..]
                        .iter()
                        .take(hashes)
                        .filter(|c| **c == '#')
                        .count()
                        == hashes;
                match closes {
                    true => {
                        mode = Mode::Code;
                        out.extend(std::iter::repeat_n(' ', hashes + 1));
                        index += hashes + 1;
                    }
                    false => {
                        out.push(blank(ch));
                        index += 1;
                    }
                }
            }
        }
    }
    out
}

pub fn blank_test_items(source: &str) -> String {
    const MARKER: &str = "#[cfg(test)]";
    let marker: Vec<char> = MARKER.chars().collect();
    let mut chars: Vec<char> = source.chars().collect();
    let mut cursor = 0;
    while cursor < chars.len() {
        if !chars[cursor..].starts_with(&marker) {
            cursor += 1;
            continue;
        }
        let start = cursor;
        cursor += marker.len();
        let mut depth = 0usize;
        let mut entered = false;
        while cursor < chars.len() {
            let ch = chars[cursor];
            cursor += 1;
            match ch {
                '{' => {
                    depth += 1;
                    entered = true;
                }
                '}' => depth = depth.saturating_sub(1),
                ';' if !entered => break,
                _ => {}
            }
            if entered && depth == 0 {
                break;
            }
        }
        (start..cursor).for_each(|i| chars[i] = blank(chars[i]));
    }
    chars.into_iter().collect()
}

pub fn scan_source(path: &Path, source: &str) -> Vec<Hit> {
    let cleaned = blank_test_items(&blank_comments_and_strings(source));
    let chars: Vec<char> = cleaned.chars().collect();
    let lines: Vec<usize> = chars
        .iter()
        .scan(1usize, |line, ch| {
            let current = *line;
            if *ch == '\n' {
                *line += 1;
            }
            Some(current)
        })
        .collect();
    FORBIDDEN_SOURCE_TOKENS
        .iter()
        .flat_map(|token| {
            let needle: Vec<char> = token.chars().collect();
            (0..chars.len())
                .filter(|index| chars[*index..].starts_with(&needle))
                .filter(|index| prior_is_boundary(&chars, *index))
                .map(|index| Hit {
                    path: path.to_path_buf(),
                    line: lines[index],
                    token,
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

pub fn rust_sources(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    paths.sort();
    paths
        .into_iter()
        .flat_map(|path| {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            match (path.is_dir(), name.as_str()) {
                (true, "tests") => Vec::new(),
                (true, _) => rust_sources(&path),
                (false, "tests.rs") => Vec::new(),
                (false, _) if path.extension().is_some_and(|ext| ext == "rs") => vec![path],
                (false, _) => Vec::new(),
            }
        })
        .collect()
}

pub fn scan_crate(crate_root: &Path) -> Vec<Hit> {
    rust_sources(&crate_root.join("src"))
        .into_iter()
        .filter_map(|path| fs::read_to_string(&path).ok().map(|source| (path, source)))
        .flat_map(|(path, source)| scan_source(&path, &source))
        .collect()
}
