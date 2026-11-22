use std::path::Path;
use std::ptr::NonNull;

use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSModalResponseOK,
    NSRunningApplication, NSWindow, NSWindowWillBeginSheetNotification, NSWorkspace,
};
use objc2_foundation::{
    MainThreadMarker, NSBundle, NSDistributedNotificationCenter, NSNotification,
    NSNotificationCenter, NSObjectProtocol, NSString, NSURL,
};

fn main_thread() -> MainThreadMarker {
    MainThreadMarker::new().expect("tests drive AppKit from the main thread")
}

pub fn set_app_appearance(dark: Option<bool>) {
    let name = dark.map(|dark| match dark {
        true => unsafe { NSAppearanceNameDarkAqua },
        false => unsafe { NSAppearanceNameAqua },
    });
    let appearance = name.and_then(NSAppearance::appearanceNamed);
    crate::app::shared(main_thread()).setAppearance(appearance.as_deref());
}

pub fn post_distributed(name: &str) {
    unsafe {
        NSDistributedNotificationCenter::defaultCenter()
            .postNotificationName_object_userInfo_deliverImmediately(
                &NSString::from_str(name),
                None,
                None,
                true,
            );
    }
}

pub fn default_application_for(path: &Path) -> Option<String> {
    let url = NSURL::from_file_path(path)?;
    let app = NSWorkspace::sharedWorkspace().URLForApplicationToOpenURL(&url)?;
    NSBundle::bundleWithURL(&app)?
        .bundleIdentifier()
        .map(|identifier| identifier.to_string())
}

fn running_with(bundle: &str) -> Vec<Retained<NSRunningApplication>> {
    NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(bundle))
        .to_vec()
}

pub fn running(bundle: &str) -> bool {
    running_with(bundle).iter().any(|app| !app.isTerminated())
}

pub fn terminate(bundle: &str) {
    for app in running_with(bundle) {
        app.forceTerminate();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnsweredSheet {
    pub parent_title: String,
    pub sheet_class: Option<String>,
}

pub struct SheetAnswer {
    _observer: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

struct Parent(Retained<NSWindow>);

unsafe impl Send for Parent {}

fn accept(parent: &NSWindow) {
    let sheet = parent.attachedSheet();
    let answered = AnsweredSheet {
        parent_title: parent.title().to_string(),
        sheet_class: sheet
            .as_ref()
            .map(|sheet| sheet.class().name().to_string_lossy().into_owned()),
    };
    println!("sheet {answered:?}");
    if let Some(sheet) = sheet {
        parent.endSheet_returnCode(&sheet, NSModalResponseOK);
    }
}

pub fn accept_next_sheet() -> SheetAnswer {
    let _mtm = main_thread();
    let block = RcBlock::new(move |notification: NonNull<NSNotification>| {
        let notification = unsafe { notification.as_ref() };
        let parent = notification
            .object()
            .and_then(|object| object.downcast::<NSWindow>().ok())
            .expect("NSWindowWillBeginSheetNotification names its parent window");
        let parent = Parent(parent);
        DispatchQueue::main().exec_async(move || {
            let parent = parent;
            accept(&parent.0);
        });
    });
    let observer = unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(NSWindowWillBeginSheetNotification),
            None,
            None,
            &block,
        )
    };
    SheetAnswer {
        _observer: observer,
    }
}
