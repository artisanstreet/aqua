use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr::{null, null_mut, NonNull};
use std::time::{Duration, Instant};

use objc2_application_services::{
    AXError, AXIsProcessTrusted, AXObserver, AXUIElement, AXValue, AXValueType,
};
use objc2_core_foundation::{
    kCFRunLoopDefaultMode, CFArray, CFBoolean, CFRange, CFRetained, CFRunLoop, CFString, CFType,
    CGPoint, CGSize,
};

const FLAG: &str = "AQUA_REQUIRE_AX";

#[derive(Debug, thiserror::Error)]
#[error("{call} {name} returned AXError {code}")]
pub struct AxError {
    pub call: &'static str,
    pub name: String,
    pub code: i32,
}

fn checked(call: &'static str, name: &str, error: AXError) -> Result<(), AxError> {
    match error {
        AXError::Success => Ok(()),
        AXError(code) => Err(AxError {
            call,
            name: name.to_owned(),
            code,
        }),
    }
}

pub fn available() -> bool {
    let trusted = unsafe { AXIsProcessTrusted() };
    let required = std::env::var(FLAG).is_ok_and(|value| value == "1");
    match (trusted, required) {
        (true, _) => true,
        (false, true) => panic!("{FLAG}=1 but this process is not trusted for accessibility"),
        (false, false) => {
            println!("SKIP-NO-AX: this process is not trusted for accessibility");
            false
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Frame {
    pub fn center(self) -> (f64, f64) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    pub fn contains(self, inner: Frame) -> bool {
        inner.x >= self.x
            && inner.y >= self.y
            && inner.x + inner.width <= self.x + self.width
            && inner.y + inner.height <= self.y + self.height
    }
}

#[derive(Clone, PartialEq)]
pub struct Element(CFRetained<AXUIElement>);

impl std::fmt::Debug for Element {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}({:?})",
            self.role().unwrap_or_default(),
            self.title().unwrap_or_default()
        )
    }
}

impl Element {
    pub fn application(pid: u32) -> Element {
        let pid = i32::try_from(pid).expect("invariant: process ids fit in pid_t");
        Element(unsafe { AXUIElement::new_application(pid) })
    }

    pub fn attribute(&self, name: &str) -> Result<CFRetained<CFType>, AxError> {
        let key = CFString::from_str(name);
        let mut value: *const CFType = null();
        let error = unsafe { self.0.copy_attribute_value(&key, NonNull::from(&mut value)) };
        checked("AXUIElementCopyAttributeValue", name, error)?;
        let value = NonNull::new(value.cast_mut()).ok_or_else(|| AxError {
            call: "AXUIElementCopyAttributeValue",
            name: name.to_owned(),
            code: AXError::NoValue.0,
        })?;
        Ok(unsafe { CFRetained::from_raw(value) })
    }

    pub fn string(&self, name: &str) -> Option<String> {
        let value = self.attribute(name).ok()?;
        value
            .downcast_ref::<CFString>()
            .map(|text| text.to_string())
    }

    pub fn flag(&self, name: &str) -> Option<bool> {
        let value = self.attribute(name).ok()?;
        value.downcast_ref::<CFBoolean>().map(CFBoolean::as_bool)
    }

    pub fn element(&self, name: &str) -> Option<Element> {
        let value = self.attribute(name).ok()?;
        value.downcast::<AXUIElement>().ok().map(Element)
    }

    pub fn elements(&self, name: &str) -> Vec<Element> {
        let Ok(value) = self.attribute(name) else {
            return Vec::new();
        };
        let Ok(array) = value.downcast::<CFArray>() else {
            return Vec::new();
        };
        let array: CFRetained<CFArray<CFType>> = unsafe { CFRetained::cast_unchecked(array) };
        array
            .iter()
            .filter_map(|item| item.downcast::<AXUIElement>().ok().map(Element))
            .collect()
    }

    fn structure<T>(&self, name: &str, kind: AXValueType, mut out: T) -> Option<T> {
        let value = self.attribute(name).ok()?;
        let value = value.downcast::<AXValue>().ok()?;
        let read = unsafe { value.value(kind, NonNull::from(&mut out).cast::<c_void>()) };
        read.then_some(out)
    }

    pub fn role(&self) -> Option<String> {
        self.string("AXRole")
    }

    pub fn title(&self) -> Option<String> {
        self.string("AXTitle")
    }

    pub fn value(&self) -> Option<String> {
        self.string("AXValue")
    }

    pub fn children(&self) -> Vec<Element> {
        self.elements("AXChildren")
    }

    pub fn windows(&self) -> Vec<Element> {
        self.elements("AXWindows")
    }

    pub fn focused(&self) -> Option<Element> {
        self.element("AXFocusedUIElement")
    }

    pub fn parent(&self) -> Option<Element> {
        self.element("AXParent")
    }

    pub fn frame(&self) -> Option<Frame> {
        let origin: CGPoint =
            self.structure("AXPosition", AXValueType::CGPoint, CGPoint::default())?;
        let size: CGSize = self.structure("AXSize", AXValueType::CGSize, CGSize::default())?;
        Some(Frame {
            x: origin.x,
            y: origin.y,
            width: size.width,
            height: size.height,
        })
    }

    pub fn selected_range(&self) -> Option<(usize, usize)> {
        let range: CFRange = self.structure(
            "AXSelectedTextRange",
            AXValueType::CFRange,
            CFRange {
                location: 0,
                length: 0,
            },
        )?;
        let start = usize::try_from(range.location).ok()?;
        let length = usize::try_from(range.length).ok()?;
        Some((start, start + length))
    }

    pub fn actions(&self) -> Vec<String> {
        let mut names: *const CFArray = null();
        let error = unsafe { self.0.copy_action_names(NonNull::from(&mut names)) };
        if checked("AXUIElementCopyActionNames", "", error).is_err() {
            return Vec::new();
        }
        let Some(names) = NonNull::new(names.cast_mut()) else {
            return Vec::new();
        };
        let names: CFRetained<CFArray<CFString>> =
            unsafe { CFRetained::cast_unchecked(CFRetained::from_raw(names)) };
        names.iter().map(|name| name.to_string()).collect()
    }

    pub fn perform(&self, action: &str) -> Result<(), AxError> {
        let name = CFString::from_str(action);
        checked("AXUIElementPerformAction", action, unsafe {
            self.0.perform_action(&name)
        })
    }

    pub fn press(&self) -> Result<(), AxError> {
        self.perform("AXPress")
    }

    pub fn set_value(&self, text: &str) -> Result<(), AxError> {
        let key = CFString::from_static_str("AXValue");
        let value = CFString::from_str(text);
        checked("AXUIElementSetAttributeValue", "AXValue", unsafe {
            self.0.set_attribute_value(&key, &value)
        })
    }

    pub fn set_focused(&self, focused: bool) -> Result<(), AxError> {
        let key = CFString::from_static_str("AXFocused");
        checked("AXUIElementSetAttributeValue", "AXFocused", unsafe {
            self.0.set_attribute_value(&key, CFBoolean::new(focused))
        })
    }

    pub fn element_at(&self, point: (f64, f64)) -> Result<Element, AxError> {
        let mut found: *const AXUIElement = null();
        let error = unsafe {
            self.0.copy_element_at_position(
                point.0 as f32,
                point.1 as f32,
                NonNull::from(&mut found),
            )
        };
        checked("AXUIElementCopyElementAtPosition", "", error)?;
        let found = NonNull::new(found.cast_mut()).ok_or(AxError {
            call: "AXUIElementCopyElementAtPosition",
            name: String::new(),
            code: AXError::NoValue.0,
        })?;
        Ok(Element(unsafe { CFRetained::from_raw(found) }))
    }

    pub fn descendants(&self) -> Vec<Element> {
        let mut found = Vec::new();
        let mut pending = self.children();
        while let Some(next) = pending.pop() {
            pending.extend(next.children());
            found.push(next);
        }
        found
    }

    pub fn find(&self, role: &str, title: &str) -> Option<Element> {
        self.descendants().into_iter().find(|element| {
            element.role().as_deref() == Some(role)
                && (element.title().as_deref() == Some(title)
                    || element.value().as_deref() == Some(title))
        })
    }
}

type Received = RefCell<Vec<(String, Element)>>;

unsafe extern "C-unwind" fn observed(
    _observer: NonNull<AXObserver>,
    element: NonNull<AXUIElement>,
    notification: NonNull<CFString>,
    refcon: *mut c_void,
) {
    let received = unsafe { &*refcon.cast::<Received>() };
    let element = Element(unsafe { CFRetained::retain(element) });
    let name = unsafe { notification.as_ref() }.to_string();
    received.borrow_mut().push((name, element));
}

pub struct Observer {
    observer: CFRetained<AXObserver>,
    received: Box<Received>,
}

impl Observer {
    pub fn new(pid: u32) -> Result<Observer, AxError> {
        let pid = i32::try_from(pid).expect("invariant: process ids fit in pid_t");
        let mut observer: *mut AXObserver = null_mut();
        let error =
            unsafe { AXObserver::create(pid, Some(observed), NonNull::from(&mut observer)) };
        checked("AXObserverCreate", "", error)?;
        let observer =
            NonNull::new(observer).expect("invariant: AXObserverCreate succeeded with an observer");
        let observer = unsafe { CFRetained::from_raw(observer) };
        let source = unsafe { observer.run_loop_source() };
        let run_loop = CFRunLoop::current().expect("invariant: every thread has a run loop");
        run_loop.add_source(Some(&source), unsafe { kCFRunLoopDefaultMode });
        Ok(Observer {
            observer,
            received: Box::new(RefCell::new(Vec::new())),
        })
    }

    pub fn watch(&self, element: &Element, notification: &str) -> Result<(), AxError> {
        let name = CFString::from_str(notification);
        let refcon = std::ptr::from_ref::<Received>(&self.received)
            .cast_mut()
            .cast::<c_void>();
        checked("AXObserverAddNotification", notification, unsafe {
            self.observer.add_notification(&element.0, &name, refcon)
        })
    }

    pub fn wait(&self, notification: &str, limit: Duration) -> Option<Element> {
        let deadline = Instant::now() + limit;
        loop {
            let position = self
                .received
                .borrow()
                .iter()
                .position(|(name, _)| name == notification);
            if let Some(position) = position {
                return Some(self.received.borrow_mut().remove(position).1);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            CFRunLoop::run_in_mode(
                unsafe { kCFRunLoopDefaultMode },
                remaining.as_secs_f64(),
                true,
            );
        }
    }

    pub fn clear(&self) {
        self.received.borrow_mut().clear();
    }
}
