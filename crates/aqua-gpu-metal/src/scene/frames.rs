use std::sync::Arc;
use std::time::Duration;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLBlitCommandEncoder, MTLBuffer, MTLClearColor, MTLCommandBuffer, MTLCommandEncoder,
    MTLLoadAction, MTLOrigin, MTLRenderCommandEncoder, MTLRenderPassDescriptor, MTLSize,
    MTLStoreAction,
};

use crate::hal::{Buffer, Device, Error, InFlight, View};
use crate::ledger::Kind;

pub const SLOTS: usize = 2;
const MIN_RING_BYTES: u64 = 1 << 16;
const FRAME_LABEL: &str = "aqua frame";
const READBACK_LABEL: &str = "aqua readback";

#[derive(Default)]
pub struct Stream {
    bytes: Vec<u8>,
}

impl Stream {
    pub fn push(&mut self, align: u64, data: &[u8]) -> u64 {
        let offset = (self.bytes.len() as u64).next_multiple_of(align.max(1));
        self.bytes.resize(offset as usize, 0);
        self.bytes.extend_from_slice(data);
        offset
    }
}

#[derive(Default)]
struct Slot {
    ring: Option<Buffer>,
    in_flight: Option<InFlight>,
}

pub struct Frames {
    device: Arc<Device>,
    slots: [Slot; SLOTS],
    next: usize,
    stream: Stream,
    latest: Option<Duration>,
    readback: Option<Buffer>,
}

impl Frames {
    pub fn new(device: &Arc<Device>) -> Self {
        Self {
            device: Arc::clone(device),
            slots: Default::default(),
            next: 0,
            stream: Stream::default(),
            latest: None,
            readback: None,
        }
    }

    pub fn gpu_time(&self) -> Option<Duration> {
        self.latest
    }

    pub fn ring_bytes(&self) -> u64 {
        self.slots
            .iter()
            .filter_map(|slot| slot.ring.as_ref())
            .map(Buffer::capacity)
            .sum()
    }

    pub fn shrink(&mut self) {
        self.slots.iter_mut().for_each(|slot| slot.ring = None);
        self.readback = None;
    }

    fn settle(
        device: &Device,
        slot: &mut Slot,
        latest: &mut Option<Duration>,
    ) -> Result<(), Error> {
        match slot.in_flight.take() {
            None => Ok(()),
            Some(InFlight(command)) => {
                *latest = device.settle(FRAME_LABEL, &command)?;
                Ok(())
            }
        }
    }

    pub fn begin(&mut self, ring_bytes: u64) -> Result<Frame<'_>, Error> {
        self.device.check("frame begin")?;
        let index = self.next;
        self.next = (self.next + 1) % SLOTS;
        let device = &self.device;
        let slot = &mut self.slots[index];
        Self::settle(device, slot, &mut self.latest)?;
        if slot
            .ring
            .as_ref()
            .is_none_or(|ring| ring.capacity() < ring_bytes)
        {
            slot.ring = Some(Buffer::shared(
                device,
                ring_bytes.max(MIN_RING_BYTES).next_power_of_two(),
                "frame_ring",
                Kind::DynamicBuffer,
            )?);
        }
        let command = device.command_buffer(FRAME_LABEL)?;
        self.stream.bytes.clear();
        Ok(Frame {
            slot,
            command,
            stream: &mut self.stream,
        })
    }

    pub fn read_texture(&mut self, view: View<'_>, texel_bytes: u64) -> Result<Vec<u8>, Error> {
        self.device.check("surface readback")?;
        let row = view.extent.width as u64 * texel_bytes;
        let bytes = row * view.extent.height as u64;
        if self
            .readback
            .as_ref()
            .is_none_or(|buffer| buffer.capacity() < bytes)
        {
            self.readback = Some(Buffer::shared(
                &self.device,
                bytes.next_power_of_two(),
                "readback_buffer",
                Kind::StagingTexture,
            )?);
        }
        let buffer = self
            .readback
            .as_ref()
            .expect("invariant: the readback buffer was just ensured");
        let command = self.device.command_buffer(READBACK_LABEL)?;
        let blit = command.blitCommandEncoder().ok_or(Error::Unavailable {
            call: "blitCommandEncoder",
        })?;
        unsafe {
            blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
                view.raw,
                0,
                0,
                MTLOrigin { x: 0, y: 0, z: 0 },
                MTLSize {
                    width: view.extent.width as usize,
                    height: view.extent.height as usize,
                    depth: 1,
                },
                buffer.raw(),
                0,
                row as usize,
                bytes as usize,
            )
        };
        blit.endEncoding();
        command.commit();
        self.device.settle(READBACK_LABEL, &command)?;
        Ok(buffer.read(bytes as usize))
    }
}

pub struct Frame<'a> {
    slot: &'a mut Slot,
    command: Retained<ProtocolObject<dyn MTLCommandBuffer>>,
    pub stream: &'a mut Stream,
}

impl Frame<'_> {
    pub fn command(&self) -> &ProtocolObject<dyn MTLCommandBuffer> {
        &self.command
    }

    pub fn ring(&self) -> &ProtocolObject<dyn MTLBuffer> {
        self.slot
            .ring
            .as_ref()
            .expect("invariant: a frame always holds its ring")
            .raw()
    }

    pub fn render_pass(
        &self,
        out: View<'_>,
        clear: Option<[f32; 4]>,
    ) -> Result<Retained<ProtocolObject<dyn MTLRenderCommandEncoder>>, Error> {
        let descriptor = MTLRenderPassDescriptor::new();
        let attachment = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
        attachment.setTexture(Some(out.raw));
        attachment.setStoreAction(MTLStoreAction::Store);
        match clear {
            Some([red, green, blue, alpha]) => {
                attachment.setLoadAction(MTLLoadAction::Clear);
                attachment.setClearColor(MTLClearColor {
                    red: red as f64,
                    green: green as f64,
                    blue: blue as f64,
                    alpha: alpha as f64,
                });
            }
            None => attachment.setLoadAction(MTLLoadAction::Load),
        }
        self.command
            .renderCommandEncoderWithDescriptor(&descriptor)
            .ok_or(Error::Unavailable {
                call: "renderCommandEncoderWithDescriptor",
            })
    }

    pub fn clear(&self, out: View<'_>) -> Result<(), Error> {
        self.render_pass(out, Some([0.0; 4]))?.endEncoding();
        Ok(())
    }

    pub fn blit(&self) -> Result<Retained<ProtocolObject<dyn MTLBlitCommandEncoder>>, Error> {
        self.command.blitCommandEncoder().ok_or(Error::Unavailable {
            call: "blitCommandEncoder",
        })
    }

    pub fn submit(self) {
        self.slot
            .ring
            .as_ref()
            .expect("invariant: a frame always holds its ring")
            .write(&self.stream.bytes);
        self.command.commit();
        self.slot.in_flight = Some(InFlight(self.command));
    }
}

impl Drop for Frames {
    fn drop(&mut self) {
        self.slots
            .iter_mut()
            .filter_map(|slot| slot.in_flight.take())
            .for_each(|InFlight(command)| command.waitUntilCompleted());
    }
}
