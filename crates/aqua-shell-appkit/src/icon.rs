use std::path::Path;

use objc2::AllocAnyThread;
use objc2_app_kit::{
    NSBitmapFormat, NSBitmapImageRep, NSCalibratedRGBColorSpace, NSCompositingOperation,
    NSGraphicsContext, NSWorkspace,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

pub use aqua_shell::icon::FileIcon;

const MAX_EDGE: u32 = 1024;

fn straight(premultiplied: &[u8]) -> Vec<u8> {
    let (pixels, _) = premultiplied.as_chunks::<4>();
    pixels
        .iter()
        .flat_map(|&[red, green, blue, alpha]| {
            let straight = |channel: u8| match alpha {
                0 => 0,
                alpha => (u32::from(channel) * 255 / u32::from(alpha)).min(255) as u8,
            };
            [straight(red), straight(green), straight(blue), alpha]
        })
        .collect()
}

pub fn file_icon(path: &Path, edge: u32) -> Option<FileIcon> {
    if !(1..=MAX_EDGE).contains(&edge) || !path.exists() {
        return None;
    }
    let path = NSString::from_str(path.to_str()?);
    let image = NSWorkspace::sharedWorkspace().iconForFile(&path);
    let side = edge as isize;
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            side,
            side,
            8,
            4,
            true,
            false,
            NSCalibratedRGBColorSpace,
            NSBitmapFormat::empty(),
            side * 4,
            32,
        )
    }?;
    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    let bounds = NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(f64::from(edge), f64::from(edge)),
    );
    image.drawInRect_fromRect_operation_fraction(
        bounds,
        NSRect::ZERO,
        NSCompositingOperation::Copy,
        1.0,
    );
    context.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();
    let stride = usize::try_from(rep.bytesPerRow()).ok()?;
    let data = rep.bitmapData();
    if data.is_null() || stride < edge as usize * 4 {
        return None;
    }
    let rows = unsafe { std::slice::from_raw_parts(data, stride * edge as usize) };
    let packed: Vec<u8> = rows
        .chunks_exact(stride)
        .flat_map(|row| &row[..edge as usize * 4])
        .copied()
        .collect();
    Some(FileIcon {
        width: edge,
        height: edge,
        rgba: straight(&packed),
    })
}
