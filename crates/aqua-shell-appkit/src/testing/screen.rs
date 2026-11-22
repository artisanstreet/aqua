use super::Rect;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_graphics::{CGWindowListCopyWindowInfo, CGWindowListOption};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};

#[derive(Debug, Clone, PartialEq)]
pub struct OnScreen {
    pub number: isize,
    pub title: String,
    pub bounds: Rect,
}

type Info = NSDictionary<NSString, AnyObject>;

fn entry(info: &Info, key: &str) -> Option<Retained<AnyObject>> {
    info.objectForKey(&NSString::from_str(key))
}

fn number(info: &Info, key: &str) -> Option<f64> {
    entry(info, key)?
        .downcast::<NSNumber>()
        .ok()
        .map(|number| number.as_f64())
}

fn bounds(info: &Info) -> Option<Rect> {
    let bounds = entry(info, "kCGWindowBounds")?
        .downcast::<NSDictionary>()
        .ok()?;
    let bounds: &Info = unsafe { &*(Retained::as_ptr(&bounds) as *const Info) };
    Some(Rect {
        x: number(bounds, "X")?,
        y: number(bounds, "Y")?,
        width: number(bounds, "Width")?,
        height: number(bounds, "Height")?,
    })
}

fn on_screen(info: &Info) -> Option<(u32, OnScreen)> {
    let owner = number(info, "kCGWindowOwnerPID")? as u32;
    if number(info, "kCGWindowLayer")? != 0.0 {
        return None;
    }
    let title = entry(info, "kCGWindowName")
        .and_then(|name| name.downcast::<NSString>().ok())
        .map(|name| name.to_string())
        .unwrap_or_default();
    Some((
        owner,
        OnScreen {
            number: number(info, "kCGWindowNumber")? as isize,
            title,
            bounds: bounds(info)?,
        },
    ))
}

pub fn windows_of(pid: u32) -> Vec<OnScreen> {
    let options =
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
    let Some(list) = CGWindowListCopyWindowInfo(options, 0) else {
        return Vec::new();
    };
    let list: &NSArray<Info> = unsafe {
        &*(objc2_core_foundation::CFRetained::as_ptr(&list).as_ptr() as *const NSArray<Info>)
    };
    list.iter()
        .filter_map(|info| on_screen(&info))
        .filter(|(owner, _)| *owner == pid)
        .map(|(_, window)| window)
        .collect()
}
