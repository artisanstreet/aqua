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

#[cfg(windows)]
pub use host::{handle_launch, register_protocol, unregister_protocol, Launch, Receiver};

#[cfg(windows)]
mod host {
    use super::{parse_activation_url, Activation, ACTIVATION_FLAG, SCHEME};
    use crate::notifications::{NotificationError, ResponseHandler};
    use std::path::Path;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::DataExchange::COPYDATASTRUCT;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, FindWindowExW, GetWindowLongPtrW,
        RegisterClassW, SendMessageTimeoutW, SetWindowLongPtrW, CW_USEDEFAULT, GWLP_USERDATA,
        HWND_MESSAGE, SMTO_ABORTIFHUNG, WINDOW_EX_STYLE, WINDOW_STYLE, WM_COPYDATA, WNDCLASSW,
    };

    const CLASS: &str = "AquaToastReceiver";
    const MAGIC: usize = 0x4151_5441;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn native_error(error: impl std::fmt::Display) -> NotificationError {
        NotificationError::Native(error.to_string())
    }

    pub fn register_protocol(exe: &Path) -> Result<(), NotificationError> {
        let root = windows_registry::CURRENT_USER
            .create(format!(r"Software\Classes\{SCHEME}"))
            .map_err(native_error)?;
        root.set_string("", "URL:Aqua toast activation")
            .map_err(native_error)?;
        root.set_string("URL Protocol", "").map_err(native_error)?;
        let command = windows_registry::CURRENT_USER
            .create(format!(r"Software\Classes\{SCHEME}\shell\open\command"))
            .map_err(native_error)?;
        command
            .set_string(
                "",
                format!("\"{}\" {ACTIVATION_FLAG} \"%1\"", exe.display()),
            )
            .map_err(native_error)
    }

    pub fn unregister_protocol() -> Result<(), NotificationError> {
        windows_registry::CURRENT_USER
            .remove_tree(format!(r"Software\Classes\{SCHEME}"))
            .map_err(native_error)
    }

    struct Sink {
        identifier: String,
        handler: ResponseHandler,
    }

    unsafe extern "system" fn receive(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message != WM_COPYDATA {
            return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
        }
        let sink = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Sink;
        let data = lparam.0 as *const COPYDATASTRUCT;
        if sink.is_null() || data.is_null() {
            return LRESULT(0);
        }
        let (sink, data) = unsafe { (&*sink, &*data) };
        if data.dwData != MAGIC || data.lpData.is_null() {
            return LRESULT(0);
        }
        let bytes =
            unsafe { std::slice::from_raw_parts(data.lpData as *const u8, data.cbData as usize) };
        let activation = std::str::from_utf8(bytes)
            .ok()
            .and_then(|text| parse_activation_url(text).ok())
            .filter(|activation| activation.identifier == sink.identifier);
        match activation {
            Some(activation) => {
                (sink.handler)(activation.response);
                LRESULT(1)
            }
            None => LRESULT(0),
        }
    }

    pub struct Receiver {
        hwnd: HWND,
        sink: *mut Sink,
    }

    impl Receiver {
        pub fn create(
            identifier: &str,
            handler: ResponseHandler,
        ) -> Result<Self, NotificationError> {
            let instance = unsafe { GetModuleHandleW(None) }.map_err(native_error)?;
            let class = wide(CLASS);
            let description = WNDCLASSW {
                lpfnWndProc: Some(receive),
                hInstance: instance.into(),
                lpszClassName: PCWSTR(class.as_ptr()),
                ..Default::default()
            };
            unsafe { RegisterClassW(&description) };
            let title = wide(identifier);
            let hwnd = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    PCWSTR(class.as_ptr()),
                    PCWSTR(title.as_ptr()),
                    WINDOW_STYLE(0),
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    0,
                    0,
                    Some(HWND_MESSAGE),
                    None,
                    Some(instance.into()),
                    None,
                )
            }
            .map_err(native_error)?;
            let sink = Box::into_raw(Box::new(Sink {
                identifier: identifier.to_owned(),
                handler,
            }));
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, sink as isize) };
            Ok(Self { hwnd, sink })
        }
    }

    impl Drop for Receiver {
        fn drop(&mut self) {
            let _ = unsafe { DestroyWindow(self.hwnd) };
            drop(unsafe { Box::from_raw(self.sink) });
        }
    }

    fn forward(activation: &Activation, url: &str) -> bool {
        let class = wide(CLASS);
        let title = wide(&activation.identifier);
        let Ok(target) = (unsafe {
            FindWindowExW(
                Some(HWND_MESSAGE),
                None,
                PCWSTR(class.as_ptr()),
                PCWSTR(title.as_ptr()),
            )
        }) else {
            return false;
        };
        let payload = COPYDATASTRUCT {
            dwData: MAGIC,
            cbData: url.len() as u32,
            lpData: url.as_ptr() as *mut _,
        };
        let mut result = 0usize;
        let sent = unsafe {
            SendMessageTimeoutW(
                target,
                WM_COPYDATA,
                WPARAM(0),
                LPARAM(&payload as *const _ as isize),
                SMTO_ABORTIFHUNG,
                3000,
                Some(&mut result),
            )
        };
        sent.0 != 0 && result == 1
    }

    #[derive(Debug, PartialEq, Eq)]
    pub enum Launch {
        Ordinary,
        Handled,
        Cold(Activation),
    }

    pub fn handle_launch() -> Launch {
        let mut args = std::env::args().skip(1);
        if args.next().as_deref() != Some(ACTIVATION_FLAG) {
            return Launch::Ordinary;
        }
        let Some(url) = args.next() else {
            return Launch::Handled;
        };
        match parse_activation_url(&url) {
            Ok(activation) if forward(&activation, &url) => Launch::Handled,
            Ok(activation) => Launch::Cold(activation),
            Err(_) => Launch::Handled,
        }
    }
}
