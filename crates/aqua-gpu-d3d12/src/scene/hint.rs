use std::collections::BTreeSet;

const MAGIC: &[u8; 4] = b"AQH1";
const HEADER: usize = 8;

pub fn encode(used: &BTreeSet<String>, library: &[u8]) -> Vec<u8> {
    let names = used
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("\n");
    let mut bytes = Vec::with_capacity(HEADER + names.len() + library.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&(names.len() as u32).to_le_bytes());
    bytes.extend_from_slice(names.as_bytes());
    bytes.extend_from_slice(library);
    bytes
}

pub fn decode(cache: Option<&[u8]>) -> (Option<BTreeSet<String>>, Option<&[u8]>) {
    let Some(bytes) = cache else {
        return (None, None);
    };
    let framed = bytes
        .strip_prefix(MAGIC)
        .and_then(|rest| rest.split_first_chunk::<4>())
        .and_then(|(length, rest)| {
            let length = u32::from_le_bytes(*length) as usize;
            let names = rest.get(..length)?;
            let library = rest.get(length..)?;
            Some((std::str::from_utf8(names).ok()?, library))
        });
    match framed {
        Some((names, library)) => {
            let used: BTreeSet<String> = names
                .split('\n')
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .collect();
            (Some(used).filter(|used| !used.is_empty()), Some(library))
        }
        None => (None, Some(bytes)),
    }
}

#[cfg(test)]
#[path = "../../tests/scene/hint.rs"]
mod tests;
