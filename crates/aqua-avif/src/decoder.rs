use std::mem::MaybeUninit;
use std::ptr::NonNull;

use rav1d::include::dav1d::data::Dav1dData;
use rav1d::include::dav1d::dav1d::{Dav1dContext, Dav1dSettings};
use rav1d::include::dav1d::headers::{
    DAV1D_PIXEL_LAYOUT_I400, DAV1D_PIXEL_LAYOUT_I420, DAV1D_PIXEL_LAYOUT_I422,
    DAV1D_PIXEL_LAYOUT_I444,
};
use rav1d::include::dav1d::picture::Dav1dPicture;
use rav1d::src::lib::{
    dav1d_close, dav1d_data_create, dav1d_data_unref, dav1d_default_settings, dav1d_get_picture,
    dav1d_open, dav1d_picture_unref, dav1d_send_data,
};
use rav1d::Dav1dResult;

use crate::AvifError;

const MAX_THREADS: usize = 8;

struct Context(Option<Dav1dContext>);

impl Drop for Context {
    fn drop(&mut self) {
        unsafe { dav1d_close(Some(NonNull::from(&mut self.0))) };
    }
}

struct Owned(Dav1dPicture);

impl Drop for Owned {
    fn drop(&mut self) {
        unsafe { dav1d_picture_unref(Some(NonNull::from(&mut self.0))) };
    }
}

struct Input(Dav1dData);

impl Drop for Input {
    fn drop(&mut self) {
        unsafe { dav1d_data_unref(Some(NonNull::from(&mut self.0))) };
    }
}

fn checked(result: Dav1dResult) -> Result<(), AvifError> {
    match result.0 {
        0 => Ok(()),
        code => Err(AvifError::Decoder(code.unsigned_abs())),
    }
}

fn open(max_pixels: u64) -> Result<Context, AvifError> {
    let mut settings = MaybeUninit::<Dav1dSettings>::uninit();
    let settings = unsafe {
        dav1d_default_settings(NonNull::from(&mut settings).cast());
        settings.assume_init_mut()
    };
    let threads = std::thread::available_parallelism().map_or(4, std::num::NonZero::get);
    settings.n_threads = threads.min(MAX_THREADS) as _;
    settings.max_frame_delay = 1;
    settings.apply_grain = 1;
    settings.frame_size_limit = u32::try_from(max_pixels).unwrap_or(u32::MAX);
    let mut context = Context(None);
    checked(unsafe {
        dav1d_open(
            Some(NonNull::from(&mut context.0)),
            Some(NonNull::from(settings)),
        )
    })?;
    Ok(context)
}

fn next_picture(context: Dav1dContext) -> Option<Owned> {
    let mut picture = Owned(Dav1dPicture::default());
    let result = unsafe { dav1d_get_picture(Some(context), Some(NonNull::from(&mut picture.0))) };
    (result.0 == 0).then_some(picture)
}

fn last_picture(context: Dav1dContext, av1: &[u8]) -> Result<Owned, AvifError> {
    let mut input = Input(Dav1dData::default());
    let buffer = unsafe { dav1d_data_create(Some(NonNull::from(&mut input.0)), av1.len()) };
    let buffer = NonNull::new(buffer).ok_or(AvifError::Decoder(12))?;
    unsafe { std::ptr::copy_nonoverlapping(av1.as_ptr(), buffer.as_ptr(), av1.len()) };
    let mut last = None;
    loop {
        let sent = unsafe { dav1d_send_data(Some(context), Some(NonNull::from(&mut input.0))) };
        if sent.0 == 0 {
            break;
        }
        match next_picture(context) {
            Some(picture) => last = Some(picture),
            None => return checked(sent).and(Err(AvifError::Decoder(0))),
        }
    }
    while let Some(picture) = next_picture(context) {
        last = Some(picture);
    }
    last.ok_or(AvifError::Unsupported("item without a frame"))
}

pub struct Plane<'a> {
    samples: &'a [u8],
    stride: usize,
    wide: bool,
    pub width: usize,
    pub height: usize,
}

impl Plane<'_> {
    pub fn sample(&self, x: usize, y: usize) -> f32 {
        match self.wide {
            false => f32::from(self.samples[y * self.stride + x]),
            true => {
                let at = y * self.stride + x * 2;
                f32::from(u16::from_ne_bytes([self.samples[at], self.samples[at + 1]]))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chroma {
    Monochrome,
    Subsampled { horizontal: bool, vertical: bool },
}

pub struct Picture<'a> {
    picture: &'a Dav1dPicture,
}

impl Picture<'_> {
    pub fn width(&self) -> usize {
        usize::try_from(self.picture.p.w).unwrap_or(0)
    }

    pub fn height(&self) -> usize {
        usize::try_from(self.picture.p.h).unwrap_or(0)
    }

    pub fn depth(&self) -> u32 {
        u32::try_from(self.picture.p.bpc).unwrap_or(8)
    }

    pub fn chroma(&self) -> Result<Chroma, AvifError> {
        match self.picture.p.layout {
            DAV1D_PIXEL_LAYOUT_I400 => Ok(Chroma::Monochrome),
            DAV1D_PIXEL_LAYOUT_I420 => Ok(Chroma::Subsampled {
                horizontal: true,
                vertical: true,
            }),
            DAV1D_PIXEL_LAYOUT_I422 => Ok(Chroma::Subsampled {
                horizontal: true,
                vertical: false,
            }),
            DAV1D_PIXEL_LAYOUT_I444 => Ok(Chroma::Subsampled {
                horizontal: false,
                vertical: false,
            }),
            _ => Err(AvifError::Unsupported("pixel layout")),
        }
    }

    pub fn matrix(&self) -> u32 {
        self.picture
            .seq_hdr
            .map_or(2, |header| unsafe { header.as_ref() }.mtrx)
    }

    pub fn full_range(&self) -> bool {
        self.picture
            .seq_hdr
            .is_some_and(|header| unsafe { header.as_ref() }.color_range != 0)
    }

    pub fn plane(&self, index: usize) -> Result<Plane<'_>, AvifError> {
        let (horizontal, vertical) = match (index, self.chroma()?) {
            (0, _) => (false, false),
            (_, Chroma::Monochrome) => return Err(AvifError::Unsupported("chroma plane")),
            (
                _,
                Chroma::Subsampled {
                    horizontal,
                    vertical,
                },
            ) => (horizontal, vertical),
        };
        let width = (self.width() + usize::from(horizontal)) >> usize::from(horizontal);
        let height = (self.height() + usize::from(vertical)) >> usize::from(vertical);
        let wide = self.depth() > 8;
        let stride = usize::try_from(self.picture.stride[usize::from(index != 0)])
            .map_err(|_| AvifError::Unsupported("plane stride"))?;
        let data = self.picture.data[index].ok_or(AvifError::Unsupported("missing plane"))?;
        if width == 0 || height == 0 || stride < width * (1 + usize::from(wide)) {
            return Err(AvifError::Unsupported("plane extent"));
        }
        let samples =
            unsafe { std::slice::from_raw_parts(data.as_ptr().cast::<u8>(), stride * height) };
        Ok(Plane {
            samples,
            stride,
            wide,
            width,
            height,
        })
    }
}

pub fn with_picture<R>(
    av1: &[u8],
    max_pixels: u64,
    read: impl FnOnce(&Picture<'_>) -> R,
) -> Result<R, AvifError> {
    let context = open(max_pixels)?;
    let handle = context.0.ok_or(AvifError::Decoder(0))?;
    let owned = last_picture(handle, av1)?;
    let result = read(&Picture { picture: &owned.0 });
    drop(owned);
    drop(context);
    Ok(result)
}
