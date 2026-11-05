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

    pub(crate) fn is_empty(self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }

    pub(crate) fn clip(self, to: Bounds) -> Bounds {
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

pub const QUIET_BEFORE_UNHOOK: std::time::Duration = std::time::Duration::from_millis(1500);
pub const REFRESH_WHILE_ACTIVE: std::time::Duration = std::time::Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Level {
    #[default]
    Off,
    Narrow,
    Full,
}

#[derive(Debug, Default)]
pub struct Watch {
    level: Level,
    covered: bool,
    quiet_since: Option<aqua_protocol::Instant>,
    refreshed: Option<aqua_protocol::Instant>,
}

impl Watch {
    pub fn step(
        &mut self,
        now: aqua_protocol::Instant,
        active: bool,
        covered: bool,
    ) -> Option<Level> {
        self.covered = covered;
        let target = match active {
            true => {
                self.quiet_since = None;
                Level::Full
            }
            false => {
                let since = *self.quiet_since.get_or_insert(now);
                match now.saturating_duration_since(since) >= QUIET_BEFORE_UNHOOK {
                    true if covered => Level::Narrow,
                    true => Level::Off,
                    false => self.level,
                }
            }
        };
        (target != self.level).then(|| {
            self.level = target;
            target
        })
    }

    pub fn refresh_due(&mut self, now: aqua_protocol::Instant) -> bool {
        let due = self.level == Level::Full
            && self
                .refreshed
                .is_none_or(|last| now.saturating_duration_since(last) >= REFRESH_WHILE_ACTIVE);
        if due {
            self.refreshed = Some(now);
        }
        due
    }

    pub fn next_check(&self) -> Option<aqua_protocol::Instant> {
        let settling = match self.level {
            Level::Full => true,
            Level::Narrow => !self.covered,
            Level::Off => false,
        };
        settling
            .then_some(self.quiet_since)
            .flatten()
            .map(|since| since + QUIET_BEFORE_UNHOOK)
    }
}

pub fn reconcile<K: Eq + Clone>(installed: &[K], desired: &[K]) -> (Vec<K>, Vec<K>) {
    let stale = installed
        .iter()
        .filter(|key| !desired.contains(key))
        .cloned()
        .collect();
    let missing = desired
        .iter()
        .enumerate()
        .filter(|(index, key)| !installed.contains(key) && !desired[..*index].contains(key))
        .map(|(_, key)| key.clone())
        .collect();
    (stale, missing)
}

#[cfg(windows)]
pub(crate) use native::{is_covered, pump, refresh, set_level};

#[cfg(windows)]
mod native {
    use super::{covered, Bounds};
    use crate::event::ShellEvent;
    use crate::state;
    use std::cell::{Cell, RefCell};
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Dwm::{
        DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
    };
    use windows::Win32::Graphics::Gdi::{GetWindowRgnBox, RGN_ERROR};
    use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindow, GetWindowLongW, GetWindowRect, GetWindowThreadProcessId, IsIconic,
        IsWindowVisible, GWL_EXSTYLE, GW_HWNDPREV, WS_EX_LAYERED, WS_EX_TRANSPARENT,
    };

    const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
    const EVENT_SYSTEM_MINIMIZESTART: u32 = 0x0016;
    const EVENT_SYSTEM_MINIMIZEEND: u32 = 0x0017;
    const EVENT_OBJECT_DESTROY: u32 = 0x8001;
    const EVENT_OBJECT_SHOW: u32 = 0x8002;
    const EVENT_OBJECT_HIDE: u32 = 0x8003;
    const EVENT_OBJECT_CLOAKED: u32 = 0x8017;
    const EVENT_OBJECT_UNCLOAKED: u32 = 0x8018;
    const WINEVENT_OUTOFCONTEXT: u32 = 0;
    const OBJID_WINDOW: i32 = 0;

    thread_local! {
        static DIRTY: Cell<bool> = const { Cell::new(true) };
        static HOOKS: RefCell<Vec<(HookKey, HWINEVENTHOOK)>> = const { RefCell::new(Vec::new()) };
        static LEVEL: Cell<super::Level> = const { Cell::new(super::Level::Off) };
    }

    type HookKey = (u32, u32, u32, u32);

    const FULL: [(u32, u32); 5] = [
        (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
        (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND),
        (EVENT_OBJECT_DESTROY, EVENT_OBJECT_DESTROY),
        (EVENT_OBJECT_SHOW, EVENT_OBJECT_HIDE),
        (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
    ];

    const SYSTEM_NARROW: [(u32, u32); 2] = [
        (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
        (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND),
    ];

    const OCCLUDER_NARROW: [(u32, u32); 3] = [
        (EVENT_OBJECT_DESTROY, EVENT_OBJECT_DESTROY),
        (EVENT_OBJECT_HIDE, EVENT_OBJECT_HIDE),
        (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
    ];

    const RELEVANT: [u32; 8] = [
        EVENT_SYSTEM_FOREGROUND,
        EVENT_SYSTEM_MINIMIZESTART,
        EVENT_SYSTEM_MINIMIZEEND,
        EVENT_OBJECT_DESTROY,
        EVENT_OBJECT_SHOW,
        EVENT_OBJECT_HIDE,
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
        let relevant = object == OBJID_WINDOW && child == 0 && RELEVANT.contains(&event);
        crate::wake_trace::record(crate::wake_trace::Source::WinEvent { event, relevant });
        if relevant {
            DIRTY.set(true);
        }
    }

    fn hook(first: u32, last: u32, process: u32, thread: u32) -> Option<HWINEVENTHOOK> {
        let hook = unsafe {
            SetWinEventHook(
                first,
                last,
                None,
                Some(on_event),
                process,
                thread,
                WINEVENT_OUTOFCONTEXT,
            )
        };
        (!hook.is_invalid()).then_some(hook)
    }

    fn occluder_threads() -> Vec<(u32, u32)> {
        let own = std::process::id();
        let targets: Vec<isize> = state::with(|s| s.windows.keys().copied().collect());
        let mut occluders: Vec<(u32, u32)> = targets
            .into_iter()
            .flat_map(|raw| {
                let target = crate::wndproc::hwnd_from(raw);
                let bounds = frame_bounds(target);
                let rough = outer_bounds(target);
                let mut found = Vec::new();
                let mut cursor = target;
                while let Ok(previous) = unsafe { GetWindow(cursor, GW_HWNDPREV) } {
                    if previous.0.is_null() {
                        break;
                    }
                    cursor = previous;
                    let candidate = rough.is_some_and(|rough| may_occlude(previous, rough));
                    let overlaps = candidate
                        && bounds.zip(frame_bounds(previous)).is_some_and(|(a, b)| {
                            !super::Bounds::new(a.left, a.top, a.right, a.bottom)
                                .clip(b)
                                .is_empty()
                        });
                    if overlaps && occludes(previous) {
                        let mut process = 0u32;
                        let thread =
                            unsafe { GetWindowThreadProcessId(previous, Some(&mut process)) };
                        found.push((process, thread));
                    }
                }
                found
            })
            .filter(|(process, thread)| *process != 0 && *thread != 0 && *process != own)
            .collect();
        occluders.sort_unstable();
        occluders.dedup();
        occluders
    }

    pub(crate) fn refresh() {
        DIRTY.set(true);
    }

    fn desired_hooks(level: super::Level) -> Vec<HookKey> {
        match level {
            super::Level::Off => Vec::new(),
            super::Level::Full => FULL
                .iter()
                .map(|(first, last)| (*first, *last, 0, 0))
                .collect(),
            super::Level::Narrow => SYSTEM_NARROW
                .iter()
                .map(|(first, last)| (*first, *last, 0, 0))
                .chain(
                    occluder_threads()
                        .into_iter()
                        .flat_map(|(process, thread)| {
                            OCCLUDER_NARROW
                                .iter()
                                .map(move |(first, last)| (*first, *last, process, thread))
                        }),
                )
                .collect(),
        }
    }

    pub(crate) fn set_level(level: super::Level) {
        LEVEL.set(level);
        let desired = crate::wake_trace::timed("occluders", || desired_hooks(level));
        HOOKS.with(|hooks| {
            let mut hooks = hooks.borrow_mut();
            let installed: Vec<HookKey> = hooks.iter().map(|(key, _)| *key).collect();
            let (stale, missing) = super::reconcile(&installed, &desired);
            crate::wake_trace::timed("unhook", || {
                hooks.retain(|(key, hook)| {
                    let keep = !stale.contains(key);
                    if !keep {
                        let _ = unsafe { UnhookWinEvent(*hook) };
                    }
                    keep
                })
            });
            crate::wake_trace::timed("hook", || {
                hooks.extend(missing.into_iter().filter_map(|key| {
                    hook(key.0, key.1, key.2, key.3).map(|installed| (key, installed))
                }))
            });
            if level != super::Level::Off {
                DIRTY.set(true);
            }
        });
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

    fn outer_bounds(hwnd: HWND) -> Option<Bounds> {
        let mut rect = RECT::default();
        unsafe { GetWindowRect(hwnd, &mut rect) }
            .is_ok()
            .then_some(Bounds::new(rect.left, rect.top, rect.right, rect.bottom))
    }

    fn may_occlude(hwnd: HWND, rough: Bounds) -> bool {
        let ex = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32;
        unsafe { IsWindowVisible(hwnd) }.as_bool()
            && !unsafe { IsIconic(hwnd) }.as_bool()
            && ex & (WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0) == 0
            && outer_bounds(hwnd).is_some_and(|outer| !outer.clip(rough).is_empty())
    }

    fn occludes(hwnd: HWND) -> bool {
        !cloaked(hwnd) && unsafe { GetWindowRgnBox(hwnd, &mut RECT::default()) }.0 == RGN_ERROR.0
    }

    pub(crate) fn is_covered(hwnd: HWND) -> bool {
        if cloaked(hwnd) {
            return true;
        }
        let (Some(target), Some(rough)) = (frame_bounds(hwnd), outer_bounds(hwnd)) else {
            return false;
        };
        let mut above = Vec::new();
        let mut cursor = hwnd;
        while let Ok(previous) = unsafe { GetWindow(cursor, GW_HWNDPREV) } {
            if previous.0.is_null() {
                break;
            }
            cursor = previous;
            if may_occlude(previous, rough) && occludes(previous) {
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
        let changes: Vec<(isize, bool)> = crate::wake_trace::timed("covered", || {
            known
                .into_iter()
                .filter_map(|(raw, was)| {
                    let hwnd = crate::wndproc::hwnd_from(raw);
                    let now = unsafe { IsWindowVisible(hwnd) }.as_bool() && is_covered(hwnd);
                    (now != was).then_some((raw, now))
                })
                .collect()
        });
        changes.into_iter().for_each(|(raw, now)| {
            state::window(raw, |w| w.covered = now);
            state::emit(raw, ShellEvent::Covered(now));
        });
        if LEVEL.get() == super::Level::Narrow {
            set_level(super::Level::Narrow);
            DIRTY.set(false);
        }
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
