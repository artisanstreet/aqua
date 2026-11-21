use std::ptr::NonNull;

use aqua_graph::Extent;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSString;
use objc2_metal::{
    MTLBuffer, MTLDevice, MTLPixelFormat, MTLResource, MTLResourceOptions, MTLStorageMode,
    MTLTexture, MTLTextureDescriptor, MTLTextureUsage,
};

use super::device::Device;
use super::error::Error;
use crate::ledger::{record, Kind, Token};

#[derive(Clone, Copy)]
pub struct View<'a> {
    pub raw: &'a ProtocolObject<dyn MTLTexture>,
    pub format: MTLPixelFormat,
    pub extent: Extent,
}

impl View<'_> {
    pub fn same_texture(&self, other: &View<'_>) -> bool {
        std::ptr::eq(self.raw, other.raw)
    }
}

pub struct TextureDesc {
    pub extent: Extent,
    pub format: MTLPixelFormat,
    pub usage: MTLTextureUsage,
    pub label: &'static str,
    pub kind: Kind,
    pub bytes: u64,
}

pub struct Texture {
    raw: Retained<ProtocolObject<dyn MTLTexture>>,
    format: MTLPixelFormat,
    extent: Extent,
    _token: Token,
}

unsafe impl Send for Texture {}

impl Texture {
    pub fn new(device: &Device, desc: TextureDesc) -> Result<Self, Error> {
        let descriptor = MTLTextureDescriptor::new();
        descriptor.setPixelFormat(desc.format);
        unsafe {
            descriptor.setWidth(desc.extent.width as usize);
            descriptor.setHeight(desc.extent.height as usize);
        }
        descriptor.setUsage(desc.usage);
        descriptor.setStorageMode(MTLStorageMode::Private);
        let raw = device
            .raw
            .newTextureWithDescriptor(&descriptor)
            .ok_or(Error::Unavailable {
                call: "newTextureWithDescriptor",
            })?;
        raw.setLabel(Some(&NSString::from_str(desc.label)));
        Ok(Self {
            raw,
            format: desc.format,
            extent: desc.extent,
            _token: record(desc.label, desc.kind, desc.bytes),
        })
    }

    pub fn view(&self) -> View<'_> {
        View {
            raw: &self.raw,
            format: self.format,
            extent: self.extent,
        }
    }

    pub fn extent(&self) -> Extent {
        self.extent
    }
}

pub struct Buffer {
    raw: Retained<ProtocolObject<dyn MTLBuffer>>,
    capacity: u64,
    _token: Token,
}

unsafe impl Send for Buffer {}

impl Buffer {
    pub fn shared(
        device: &Device,
        capacity: u64,
        label: &'static str,
        kind: Kind,
    ) -> Result<Self, Error> {
        let raw = device
            .raw
            .newBufferWithLength_options(capacity as usize, MTLResourceOptions::StorageModeShared)
            .ok_or(Error::Unavailable {
                call: "newBufferWithLength:options:",
            })?;
        raw.setLabel(Some(&NSString::from_str(label)));
        Ok(Self {
            raw,
            capacity,
            _token: record(label, kind, capacity),
        })
    }

    pub fn raw(&self) -> &ProtocolObject<dyn MTLBuffer> {
        &self.raw
    }

    pub fn capacity(&self) -> u64 {
        self.capacity
    }

    fn contents(&self) -> NonNull<u8> {
        self.raw.contents().cast()
    }

    pub fn write(&self, bytes: &[u8]) {
        assert!(
            bytes.len() as u64 <= self.capacity,
            "invariant: {} bytes are written into a buffer of {} bytes",
            bytes.len(),
            self.capacity
        );
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.contents().as_ptr(), bytes.len())
        };
    }

    pub fn read(&self, length: usize) -> Vec<u8> {
        assert!(
            length as u64 <= self.capacity,
            "invariant: {length} bytes are read from a buffer of {} bytes",
            self.capacity
        );
        unsafe { std::slice::from_raw_parts(self.contents().as_ptr(), length) }.to_vec()
    }
}
