use std::ffi::CString;

use windows::core::{s, PCSTR};
use windows::Win32::Graphics::Direct3D::Fxc::{D3DCompile, D3DCOMPILE_OPTIMIZATION_LEVEL3};
use windows::Win32::Graphics::Direct3D::{ID3DBlob, ID3DInclude};

use super::Error;

fn blob_bytes(blob: &ID3DBlob) -> Vec<u8> {
    unsafe {
        std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize())
    }
    .to_vec()
}

pub fn compile(source: &str, entry: &str, target: &str) -> Result<Vec<u8>, Error> {
    let entry_name = CString::new(entry).map_err(|_| Error::Shader("entry contains NUL".into()))?;
    let target_name =
        CString::new(target).map_err(|_| Error::Shader("target contains NUL".into()))?;
    let mut code: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    let result = unsafe {
        D3DCompile(
            source.as_ptr() as *const _,
            source.len(),
            s!("aqua-custom"),
            None,
            None::<&ID3DInclude>,
            PCSTR(entry_name.as_ptr() as *const u8),
            PCSTR(target_name.as_ptr() as *const u8),
            D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    match (result, code) {
        (Ok(()), Some(code)) => Ok(blob_bytes(&code)),
        (Err(error), _) => Err(Error::Shader(
            errors
                .map(|blob| String::from_utf8_lossy(&blob_bytes(&blob)).into_owned())
                .unwrap_or_else(|| error.to_string()),
        )),
        (Ok(()), None) => Err(Error::Shader("compiler returned no code".into())),
    }
}
