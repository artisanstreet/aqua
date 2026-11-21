use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

pub(crate) const RUN_COMMAND: u32 = 0;
pub(crate) const GET_OUTPUTS: u32 = 3;
pub(crate) const GET_TREE: u32 = 4;
const MAGIC: &[u8; 6] = b"i3-ipc";

#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("sway IPC socket: {0}")]
    Io(#[from] std::io::Error),
    #[error("sway IPC reply is not JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("sway IPC reply has a bad header")]
    Header,
    #[error("sway refused {command:?}: {reply}")]
    Refused { command: String, reply: String },
}

pub(crate) fn request(
    socket: &Path,
    kind: u32,
    payload: &str,
) -> Result<serde_json::Value, IpcError> {
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let length = u32::try_from(payload.len()).map_err(|_| IpcError::Header)?;
    let mut message = Vec::with_capacity(14 + payload.len());
    message.extend_from_slice(MAGIC);
    message.extend_from_slice(&length.to_ne_bytes());
    message.extend_from_slice(&kind.to_ne_bytes());
    message.extend_from_slice(payload.as_bytes());
    stream.write_all(&message)?;
    let mut header = [0u8; 14];
    stream.read_exact(&mut header)?;
    if &header[..6] != MAGIC {
        return Err(IpcError::Header);
    }
    let size = u32::from_ne_bytes(header[6..10].try_into().map_err(|_| IpcError::Header)?);
    let mut body = vec![0u8; usize::try_from(size).map_err(|_| IpcError::Header)?];
    stream.read_exact(&mut body)?;
    Ok(serde_json::from_slice(&body)?)
}
