use crate::access::AccessHooks;
use crate::lease::{self, LayerGeometry, LayerLease};
use crate::looping::{emit, Waker};
use crate::model::presence::{Change, Occlusion};
use crate::output::{monitor_of, primary_height, refresh_of, Monitor};
use crate::state::{self, Pointer, Reported, Spec, WindowState};
use crate::text_input::TextInputClient;
use crate::unsupported::Unsupported;
use crate::view::AquaView;
use crate::window_class::{occlusion_of, AquaWindow, WindowDelegate};
use aqua_core::{ClickSettings, CursorIcon, Point};
use aqua_protocol::{DragRegions, MaterialCapability, WindowId, WindowMaterial};
use aqua_shell::{Chrome, ShellError, ShellEvent, WindowKind, WindowOptions};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::AnyThread;
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSColor, NSEvent, NSTrackingArea, NSTrackingAreaOptions, NSWindowCollectionBehavior,
    NSWindowLevel, NSWindowOrderingMode, NSWindowStyleMask, NSWindowTabbingMode,
    NSWindowTitleVisibility,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSString};
use objc2_quartz_core::{kCAGravityTopLeft, CAMetalLayer};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const FLOATING_LEVEL: NSWindowLevel = 3;
const NORMAL_LEVEL: NSWindowLevel = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Window {
    id: WindowId,
}

fn main_thread(call: &'static str) -> Result<MainThreadMarker, ShellError> {
    MainThreadMarker::new()
        .ok_or_else(|| ShellError::AppKit(call, "AppKit windows live on the main thread".into()))
}

fn marker() -> MainThreadMarker {
    MainThreadMarker::new().expect("invariant: Window handles are used on the main thread")
}

fn style_of(options: &WindowOptions) -> NSWindowStyleMask {
    let mut style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable;
    if options.minimizable {
        style |= NSWindowStyleMask::Miniaturizable;
    }
    if options.resizable && options.kind == WindowKind::Normal {
        style |= NSWindowStyleMask::Resizable;
    }
    if options.chrome == Chrome::Custom {
        style |= NSWindowStyleMask::FullSizeContentView;
    }
    style
}

fn metal_layer(view: &AquaView) -> Retained<CAMetalLayer> {
    view.setWantsLayer(true);
    view.layer()
        .expect("invariant: a layer-backed view has a layer")
        .downcast::<CAMetalLayer>()
        .expect("invariant: AquaView backs itself with a CAMetalLayer")
}

fn track_pointer(view: &AquaView) {
    let options = NSTrackingAreaOptions::MouseMoved
        | NSTrackingAreaOptions::MouseEnteredAndExited
        | NSTrackingAreaOptions::CursorUpdate
        | NSTrackingAreaOptions::ActiveAlways
        | NSTrackingAreaOptions::InVisibleRect;
    let area = unsafe {
        NSTrackingArea::initWithRect_options_owner_userInfo(
            NSTrackingArea::alloc(),
            NSRect::ZERO,
            options,
            Some(view),
            None,
        )
    };
    view.addTrackingArea(&area);
}

fn appearance(dark: bool) -> Option<Retained<NSAppearance>> {
    let name = match dark {
        true => unsafe { NSAppearanceNameDarkAqua },
        false => unsafe { NSAppearanceNameAqua },
    };
    NSAppearance::appearanceNamed(name)
}

fn point_of_physical(mtm: MainThreadMarker, (x, y): (i32, i32), scale: f64) -> NSPoint {
    NSPoint::new(
        f64::from(x) / scale,
        primary_height(mtm) - f64::from(y) / scale,
    )
}

fn top_left_physical(mtm: MainThreadMarker, frame: NSRect, scale: f64) -> (i32, i32) {
    let top = primary_height(mtm) - frame.origin.y - frame.size.height;
    (
        (frame.origin.x * scale).round() as i32,
        (top * scale).round() as i32,
    )
}

pub(crate) fn occlusion(id: WindowId) -> Occlusion {
    state::native(id).map_or(Occlusion::Covered, |native| occlusion_of(&native))
}

pub(crate) fn refresh_metrics(id: WindowId) {
    let Some((native, view, layer, reported, geometry)) = state::window(id, |window| {
        (
            window.native.clone(),
            window.view.clone(),
            window.layer.clone(),
            window.reported,
            window.geometry.clone(),
        )
    }) else {
        return;
    };
    let scale = native.backingScaleFactor();
    let size = view.bounds().size;
    let physical = (
        (size.width * scale).round().max(1.0) as u32,
        (size.height * scale).round().max(1.0) as u32,
    );
    let refresh = native.screen().and_then(|screen| refresh_of(&screen));
    layer.setContentsScale(scale);
    lease::update(&geometry, |geometry| {
        geometry.logical = (size.width, size.height);
        geometry.physical = physical;
        geometry.scale = scale;
        geometry.refresh = refresh;
    });
    state::window(id, |window| {
        window.reported.scale = Some(scale);
        window.reported.physical = Some(physical);
    });
    if reported.scale != Some(scale) {
        emit(id, ShellEvent::ScaleChanged { scale });
    }
    if reported.physical != Some(physical) || reported.scale != Some(scale) {
        emit(
            id,
            ShellEvent::Resized {
                width: physical.0,
                height: physical.1,
            },
        );
    }
    report_maximized(id);
}

pub(crate) fn report_position(id: WindowId) {
    let Some(native) = state::native(id) else {
        return;
    };
    let position = top_left_physical(marker(), native.frame(), native.backingScaleFactor());
    let changed = state::window(id, |window| {
        let changed = window.reported.position != Some(position);
        window.reported.position = Some(position);
        changed
    });
    if changed == Some(true) {
        emit(
            id,
            ShellEvent::Moved {
                x: position.0,
                y: position.1,
            },
        );
    }
}

fn maximized_now(native: &AquaWindow) -> bool {
    native.styleMask().contains(NSWindowStyleMask::FullScreen) || native.isZoomed()
}

pub(crate) fn report_maximized(id: WindowId) {
    let Some(native) = state::native(id) else {
        return;
    };
    let maximized = maximized_now(&native);
    let changed = state::window(id, |window| {
        let changed = window.reported.maximized != maximized;
        window.reported.maximized = maximized;
        changed
    });
    if changed == Some(true) {
        emit(id, ShellEvent::Maximized(maximized));
    }
}

impl Window {
    pub fn create(options: &WindowOptions) -> Result<Window, ShellError> {
        let mtm = main_thread("NSWindow.init")?;
        crate::app::ensure(mtm);
        let id = state::next_id();
        let (width, height) = options.logical_size;
        let frame = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(width.max(1.0), height.max(1.0)),
        );
        let native = AquaWindow::new(mtm, frame, style_of(options));
        native.setTitle(&NSString::from_str(&options.title));
        native.setAcceptsMouseMovedEvents(true);
        native.setTabbingMode(NSWindowTabbingMode::Disallowed);
        native.setMovableByWindowBackground(false);
        if options.chrome == Chrome::Custom {
            native.setTitlebarAppearsTransparent(true);
            native.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        }
        if options.maximizable && options.kind == WindowKind::Normal {
            native.setCollectionBehavior(NSWindowCollectionBehavior::FullScreenPrimary);
        }
        if let Some((min_width, min_height)) = options.min_logical_size {
            native.setContentMinSize(NSSize::new(min_width, min_height));
        }
        if options.topmost {
            native.setLevel(FLOATING_LEVEL);
        }
        native.setAppearance(appearance(options.dark).as_deref());
        if options.alpha_surface {
            native.setOpaque(false);
            native.setBackgroundColor(Some(&NSColor::clearColor()));
        }
        let view = AquaView::new(mtm, id, frame);
        native.setContentView(Some(&view));
        native.makeFirstResponder(Some(&view));
        track_pointer(&view);
        let layer = metal_layer(&view);
        layer.setContentsGravity(unsafe { kCAGravityTopLeft });
        layer.setOpaque(!options.alpha_surface);
        layer.setContentsScale(native.backingScaleFactor());
        let delegate = WindowDelegate::new(mtm, id);
        let geometry = Arc::new(Mutex::new(LayerGeometry {
            logical: (width, height),
            physical: (width.round() as u32, height.round() as u32),
            scale: 1.0,
            refresh: None,
            covered: true,
        }));
        state::insert(
            id,
            WindowState {
                native: native.clone(),
                view,
                _delegate: delegate.clone(),
                layer,
                spec: Spec {
                    chrome: options.chrome,
                    kind: options.kind,
                    titlebar_height: options.titlebar_height,
                    caption_buttons: options.caption_buttons,
                    resizable: options.resizable,
                    minimizable: options.minimizable,
                    maximizable: options.maximizable,
                    alpha_surface: options.alpha_surface,
                },
                presence: crate::model::presence::Presence::Hidden,
                reported: Reported::default(),
                regions: DragRegions::default(),
                cursor: CursorIcon::Default,
                pointer: Pointer::default(),
                raw_mouse: false,
                redraw_pending: true,
                geometry,
                text_client: None,
                access: None,
            },
        );
        native.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        if let Some(owner) = options.owner.and_then(state::native) {
            unsafe { owner.addChildWindow_ordered(&native, NSWindowOrderingMode::Above) };
        }
        let window = Window { id };
        match options.position {
            Some(position) => {
                let scale = native.backingScaleFactor();
                native.setFrameTopLeftPoint(point_of_physical(mtm, position, scale));
            }
            None => native.center(),
        }
        crate::chrome::configure_buttons(id);
        refresh_metrics(id);
        report_position(id);
        if options.visible {
            window.show()?;
        }
        Ok(window)
    }

    pub fn id(self) -> WindowId {
        self.id
    }

    pub fn waker(self) -> Waker {
        Waker::new()
    }

    pub fn scale(self) -> f64 {
        state::window(self.id, |window| window.native.backingScaleFactor()).unwrap_or(1.0)
    }

    pub fn inner_size(self) -> (u32, u32) {
        state::window(self.id, |window| window.reported.physical)
            .flatten()
            .unwrap_or((0, 0))
    }

    pub fn logical_size(self) -> (u32, u32) {
        state::window(self.id, |window| {
            let size = window.view.bounds().size;
            (size.width.round() as u32, size.height.round() as u32)
        })
        .unwrap_or((0, 0))
    }

    pub fn outer_rect(self) -> Option<(i32, i32, i32, i32)> {
        let native = state::native(self.id)?;
        let scale = native.backingScaleFactor();
        let frame = native.frame();
        let (left, top) = top_left_physical(marker(), frame, scale);
        Some((
            left,
            top,
            left + (frame.size.width * scale).round() as i32,
            top + (frame.size.height * scale).round() as i32,
        ))
    }

    pub fn monitor(self) -> Option<Monitor> {
        let screen = state::native(self.id)?.screen()?;
        Some(monitor_of(marker(), &screen))
    }

    pub fn refresh_interval(self) -> Option<Duration> {
        self.monitor().and_then(|monitor| monitor.refresh)
    }

    pub fn click_settings(self) -> ClickSettings {
        let interval = Duration::from_secs_f64(NSEvent::doubleClickInterval().max(0.0));
        ClickSettings::new(interval, Point::new(5.0, 5.0)).unwrap_or_default()
    }

    pub fn set_title(self, title: &str) {
        if let Some(native) = state::native(self.id) {
            native.setTitle(&NSString::from_str(title));
        }
    }

    pub fn title(self) -> String {
        state::native(self.id).map_or_else(String::new, |native| native.title().to_string())
    }

    pub fn request_redraw(self) {
        state::window(self.id, |window| window.redraw_pending = true);
    }

    pub fn set_drag_regions(self, regions: &DragRegions) {
        state::window(self.id, |window| window.regions = regions.clone());
    }

    pub fn set_cursor(self, cursor: CursorIcon) {
        state::window(self.id, |window| window.cursor = cursor);
        crate::cursor::apply(self.id);
    }

    pub fn set_material(self, material: WindowMaterial) -> MaterialCapability {
        crate::capability::material_capability(material)
    }

    pub fn set_dark(self, dark: bool) -> bool {
        match state::native(self.id) {
            Some(native) => {
                native.setAppearance(appearance(dark).as_deref());
                true
            }
            None => false,
        }
    }

    pub fn set_topmost(self, topmost: bool) {
        if let Some(native) = state::native(self.id) {
            native.setLevel(match topmost {
                true => FLOATING_LEVEL,
                false => NORMAL_LEVEL,
            });
        }
    }

    pub fn set_position(self, x: i32, y: i32) {
        if let Some(native) = state::native(self.id) {
            let scale = native.backingScaleFactor();
            native.setFrameTopLeftPoint(point_of_physical(marker(), (x, y), scale));
        }
    }

    pub fn center(self) {
        if let Some(native) = state::native(self.id) {
            native.center();
        }
    }

    pub fn send_to_back(self) {
        if let Some(native) = state::native(self.id) {
            native.orderBack(None);
        }
    }

    pub fn set_raw_mouse(self, enabled: bool) {
        state::window(self.id, |window| window.raw_mouse = enabled);
    }

    pub fn set_logical_size(self, width: f64, height: f64) -> Result<(), Unsupported> {
        let Some(native) = state::native(self.id) else {
            return Ok(());
        };
        match native.styleMask().contains(NSWindowStyleMask::FullScreen) {
            true => Err(Unsupported::FullscreenSize),
            false => {
                native.setContentSize(NSSize::new(width.max(1.0), height.max(1.0)));
                Ok(())
            }
        }
    }

    fn spec(self) -> Option<Spec> {
        state::window(self.id, |window| window.spec)
    }

    pub fn minimize(self) -> Result<(), Unsupported> {
        match (self.spec(), state::native(self.id)) {
            (Some(spec), Some(_)) if !spec.minimizable => Err(Unsupported::Minimize),
            (_, Some(native)) => {
                native.miniaturize(None);
                Ok(())
            }
            (_, None) => Ok(()),
        }
    }

    pub fn restore_minimized(self) {
        if let Some(native) = state::native(self.id) {
            native.deminiaturize(None);
        }
    }

    pub fn toggle_maximize(self) -> Result<(), Unsupported> {
        match (self.spec(), state::native(self.id)) {
            (Some(spec), Some(_)) if !spec.maximizable => Err(Unsupported::Maximize),
            (_, Some(native)) => {
                native.zoom(None);
                Ok(())
            }
            (_, None) => Ok(()),
        }
    }

    pub fn set_fullscreen(self, fullscreen: bool) {
        if let Some(native) = state::native(self.id)
            && native.styleMask().contains(NSWindowStyleMask::FullScreen) != fullscreen
        {
            native.toggleFullScreen(None);
        }
    }

    pub fn is_maximized(self) -> bool {
        state::native(self.id).is_some_and(|native| maximized_now(&native))
    }

    pub fn is_fullscreen(self) -> bool {
        state::native(self.id)
            .is_some_and(|native| native.styleMask().contains(NSWindowStyleMask::FullScreen))
    }

    pub fn is_foreground(self) -> bool {
        state::native(self.id)
            .is_some_and(|native| native.isKeyWindow() && crate::app::shared(marker()).isActive())
    }

    pub fn is_visible(self) -> bool {
        state::window(self.id, |window| {
            matches!(
                window.presence,
                crate::model::presence::Presence::Visible(_)
            )
        })
        .unwrap_or(false)
    }

    pub fn is_covered(self) -> bool {
        state::window(self.id, |window| window.presence.covered()).unwrap_or(true)
    }

    pub fn has_alpha_surface(self) -> bool {
        self.spec().is_some_and(|spec| spec.alpha_surface)
    }

    pub fn activate(self) -> Result<(), Unsupported> {
        let Some(native) = state::native(self.id) else {
            return Ok(());
        };
        let app = crate::app::shared(marker());
        let was_active = app.isActive();
        if !was_active {
            app.activate();
        }
        native.makeKeyAndOrderFront(None);
        match was_active {
            true => Ok(()),
            false => Err(Unsupported::Activation),
        }
    }

    pub fn engaged() -> Option<Window> {
        let app = crate::app::shared(MainThreadMarker::new()?);
        let key = app.keyWindow().or_else(|| app.mainWindow())?;
        state::window_ids()
            .into_iter()
            .find(|id| {
                state::native(*id).is_some_and(|native| {
                    Retained::as_ptr(&native).cast::<c_void>()
                        == Retained::as_ptr(&key).cast::<c_void>()
                })
            })
            .map(|id| Window { id })
    }

    pub fn show(self) -> Result<(), ShellError> {
        let Some(native) = state::native(self.id) else {
            return Err(ShellError::AppKit(
                "NSWindow.makeKeyAndOrderFront",
                format!("window {:?} was destroyed", self.id),
            ));
        };
        native.makeKeyAndOrderFront(None);
        state::change_presence(self.id, Change::Shown(occlusion_of(&native)));
        crate::chrome::place_buttons(self.id);
        Ok(())
    }

    pub fn hide(self) {
        if let Some(native) = state::native(self.id) {
            native.orderOut(None);
            state::change_presence(self.id, Change::Hidden);
        }
    }

    pub fn destroy(self) {
        let Some((window, remaining)) = state::remove(self.id) else {
            return;
        };
        window.native.setDelegate(None);
        window.native.orderOut(None);
        window.native.close();
        crate::cursor::reset();
        emit(self.id, ShellEvent::Destroyed);
        if remaining == 0 {
            crate::looping::quit();
        }
    }

    pub fn layer_lease(self) -> Option<LayerLease> {
        state::window(self.id, |window| {
            LayerLease::new(self.id, window.layer.clone(), window.geometry.clone())
        })
    }

    pub fn view_pointer(self) -> Option<NonNull<c_void>> {
        state::window(self.id, |window| {
            NonNull::from(&*window.view).cast::<c_void>()
        })
    }

    pub fn chrome(self) -> Option<Chrome> {
        self.spec().map(|spec| spec.chrome)
    }

    pub fn set_text_input_client(self, client: Option<Box<dyn TextInputClient>>) {
        state::window(self.id, |window| {
            window.text_client =
                client.map(|client| std::rc::Rc::new(std::cell::RefCell::new(client)))
        });
    }

    pub fn discard_marked_text(self) {
        let view = state::window(self.id, |window| window.view.clone());
        if let Some(context) = view.and_then(|view| view.inputContext()) {
            context.discardMarkedText();
        }
    }

    pub fn invalidate_character_coordinates(self) {
        let view = state::window(self.id, |window| window.view.clone());
        if let Some(context) = view.and_then(|view| view.inputContext()) {
            context.invalidateCharacterCoordinates();
        }
    }

    pub fn set_access_hooks(self, hooks: Option<Box<dyn AccessHooks>>) {
        state::window(self.id, |window| {
            window.access = hooks.map(|hooks| std::rc::Rc::new(std::cell::RefCell::new(hooks)))
        });
    }
}
