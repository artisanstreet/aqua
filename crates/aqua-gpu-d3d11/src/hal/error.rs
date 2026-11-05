use super::recovery::{LossCause, classify};

#[derive(Debug)]
pub enum Error {
    Windows(windows::core::Error),
    NoAdapter,
    DeviceLost { cause: LossCause, detail: String },
}

impl Error {
    pub fn loss_cause(&self) -> Option<LossCause> {
        match self {
            Error::DeviceLost { cause, .. } => Some(*cause),
            _ => None,
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Windows(error) => write!(f, "windows error: {error}"),
            Error::NoAdapter => write!(f, "no usable D3D11 adapter"),
            Error::DeviceLost { cause, detail } => write!(f, "device lost ({cause:?}): {detail}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<windows::core::Error> for Error {
    fn from(error: windows::core::Error) -> Self {
        match classify(error.code().0) {
            Some(cause) => Error::DeviceLost {
                cause,
                detail: error.message(),
            },
            None => Error::Windows(error),
        }
    }
}
