#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Bounds {
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    fn is_empty(self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }

    fn clip(self, to: Bounds) -> Bounds {
        Bounds {
            left: self.left.max(to.left),
            top: self.top.max(to.top),
            right: self.right.min(to.right),
            bottom: self.bottom.min(to.bottom),
        }
    }
}

pub fn covered(target: Bounds, above: &[Bounds]) -> bool {
    if target.is_empty() {
        return false;
    }
    let clipped: Vec<Bounds> = above
        .iter()
        .map(|rect| rect.clip(target))
        .filter(|rect| !rect.is_empty())
        .collect();
    if clipped.is_empty() {
        return false;
    }
    let mut xs: Vec<i32> = clipped
        .iter()
        .flat_map(|rect| [rect.left, rect.right])
        .chain([target.left, target.right])
        .collect();
    let mut ys: Vec<i32> = clipped
        .iter()
        .flat_map(|rect| [rect.top, rect.bottom])
        .chain([target.top, target.bottom])
        .collect();
    xs.sort_unstable();
    xs.dedup();
    ys.sort_unstable();
    ys.dedup();
    xs.windows(2).all(|x| {
        ys.windows(2).all(|y| {
            clipped.iter().any(|rect| {
                rect.left <= x[0] && x[1] <= rect.right && rect.top <= y[0] && y[1] <= rect.bottom
            })
        })
    })
}

#[cfg(windows)]
pub(crate) use native::{is_covered, pump, watch};

#[cfg(windows)]
mod native {
    use super::{Bounds, covered};
    use crate::event::ShellEvent;
    use crate::state;
    use std::cell::Cell;
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Dwm::{
        DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
    };
    use windows::Win32::Graphics::Gdi::{GetWindowRgnBox, RGN_ERROR};
    use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
    use windows::Win32::UI::WindowsAndMessaging::{
        GW_HWNDPREV, GWL_EXSTYLE, GetWindow, GetWindowLongW, GetWindowRect, IsIconic,
        IsWindowVisible, WS_EX_LAYERED, WS_EX_TRANSPARENT,
    };

    const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
    const EVENT_SYSTEM_MINIMIZESTART: u32 = 0x0016;
    const EVENT_SYSTEM_MINIMIZEEND: u32 = 0x0017;
    const EVENT_OBJECT_SHOW: u32 = 0x8002;
    const EVENT_OBJECT_HIDE: u32 = 0x8003;
    const EVENT_OBJECT_LOCATIONCHANGE: u32 = 0x800B;
    const EVENT_OBJECT_CLOAKED: u32 = 0x8017;
    const EVENT_OBJECT_UNCLOAKED: u32 = 0x8018;
    const WINEVENT_OUTOFCONTEXT: u32 = 0;
    const OBJID_WINDOW: i32 = 0;

    thread_local! {
        static DIRTY: Cell<bool> = const { Cell::new(true) };
        static HOOK: Cell<bool> = const { Cell::new(false) };
    }

    const RELEVANT: [u32; 8] = [
        EVENT_SYSTEM_FOREGROUND,
        EVENT_SYSTEM_MINIMIZESTART,
        EVENT_SYSTEM_MINIMIZEEND,
        EVENT_OBJECT_SHOW,
        EVENT_OBJECT_HIDE,
        EVENT_OBJECT_LOCATIONCHANGE,
        EVENT_OBJECT_CLOAKED,
        EVENT_OBJECT_UNCLOAKED,
    ];

    unsafe extern "system" fn on_event(
        _hook: HWINEVENTHOOK,
        event: u32,
        _hwnd: HWND,
        object: i32,
        child: i32,
        _thread: u32,
        _time: u32,
    ) {
        if object == OBJID_WINDOW && child == 0 && RELEVANT.contains(&event) {
            DIRTY.set(true);
        }
    }

    pub(crate) fn watch() {
        if HOOK.replace(true) {
            return;
        }
        let _ = unsafe {
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_OBJECT_UNCLOAKED,
                None,
                Some(on_event),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        };
        DIRTY.set(true);
    }

    fn cloaked(hwnd: HWND) -> bool {
        let mut value = 0u32;
        unsafe {
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED,
                (&mut value as *mut u32).cast(),
                std::mem::size_of::<u32>() as u32,
            )
        }
        .is_ok()
            && value != 0
    }

    fn frame_bounds(hwnd: HWND) -> Option<Bounds> {
        let mut rect = RECT::default();
        let dwm = unsafe {
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                (&mut rect as *mut RECT).cast(),
                std::mem::size_of::<RECT>() as u32,
            )
        }
        .is_ok();
        (dwm || unsafe { GetWindowRect(hwnd, &mut rect) }.is_ok()).then_some(Bounds::new(
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
        ))
    }

    fn opaque_in_flow(hwnd: HWND) -> bool {
        let ex = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32;
        unsafe { IsWindowVisible(hwnd) }.as_bool()
            && !unsafe { IsIconic(hwnd) }.as_bool()
            && ex & (WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0) == 0
            && !cloaked(hwnd)
            && unsafe { GetWindowRgnBox(hwnd, &mut RECT::default()) }.0 == RGN_ERROR.0
    }

    pub(crate) fn is_covered(hwnd: HWND) -> bool {
        if cloaked(hwnd) {
            return true;
        }
        let Some(target) = frame_bounds(hwnd) else {
            return false;
        };
        let mut above = Vec::new();
        let mut cursor = hwnd;
        while let Ok(previous) = unsafe { GetWindow(cursor, GW_HWNDPREV) } {
            if previous.0.is_null() {
                break;
            }
            cursor = previous;
            if opaque_in_flow(previous) {
                above.extend(frame_bounds(previous));
            }
        }
        covered(target, &above)
    }

    pub(crate) fn pump() {
        if !DIRTY.replace(false) {
            return;
        }
        let known: Vec<(isize, bool)> = state::with(|s| {
            s.windows
                .iter()
                .filter(|(_, w)| !w.minimized)
                .map(|(raw, w)| (*raw, w.covered))
                .collect()
        });
        let changes: Vec<(isize, bool)> = known
            .into_iter()
            .filter_map(|(raw, was)| {
                let hwnd = crate::wndproc::hwnd_from(raw);
                let now = unsafe { IsWindowVisible(hwnd) }.as_bool() && is_covered(hwnd);
                (now != was).then_some((raw, now))
            })
            .collect();
        changes.into_iter().for_each(|(raw, now)| {
            state::window(raw, |w| w.covered = now);
            state::emit(raw, ShellEvent::Covered(now));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: Bounds = Bounds::new(0, 0, 100, 100);

    #[test]
    fn a_single_larger_window_covers() {
        assert!(covered(TARGET, &[Bounds::new(-10, -10, 120, 120)]));
    }

    #[test]
    fn no_windows_above_leaves_it_visible() {
        assert!(!covered(TARGET, &[]));
        assert!(!covered(TARGET, &[Bounds::new(200, 0, 300, 100)]));
    }

    #[test]
    fn two_halves_cover_together_but_a_gap_does_not() {
        let left = Bounds::new(0, 0, 50, 100);
        let right = Bounds::new(50, 0, 100, 100);
        assert!(covered(TARGET, &[left, right]));
        assert!(!covered(TARGET, &[left, Bounds::new(51, 0, 100, 100)]));
    }

    #[test]
    fn a_hole_in_the_middle_keeps_it_visible() {
        let ring = [
            Bounds::new(0, 0, 100, 40),
            Bounds::new(0, 60, 100, 100),
            Bounds::new(0, 40, 40, 60),
            Bounds::new(60, 40, 100, 60),
        ];
        assert!(!covered(TARGET, &ring));
    }

    #[test]
    fn an_empty_target_is_never_covered() {
        assert!(!covered(Bounds::new(5, 5, 5, 9), &[TARGET]));
    }
}
