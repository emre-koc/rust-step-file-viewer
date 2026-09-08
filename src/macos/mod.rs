//! macOS integration: receive Finder "Open With" / double-click file events.
//!
//! winit installs its own `NSApplicationDelegate`, which does not implement
//! `application:openURLs:`. AppKit checks `respondsToSelector:` at dispatch time, so we add that
//! method to the delegate's class at runtime, during `applicationWillFinishLaunching` (after winit
//! has installed its delegate, before AppKit delivers the initial open-document event). Received
//! paths are queued and drained by the app each frame.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, NSObject, NSObjectProtocol, Sel};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSApplication, NSApplicationWillFinishLaunchingNotification};
use objc2_foundation::{NSArray, NSNotification, NSNotificationCenter, NSURL};

static PENDING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static WAKE: OnceLock<egui::Context> = OnceLock::new();
static PATCHED: Mutex<bool> = Mutex::new(false);

/// Paths delivered by the system since the last call.
pub fn take_pending() -> Vec<PathBuf> {
    std::mem::take(&mut *PENDING.lock().unwrap())
}

/// Register the egui context so a late-arriving open event repaints the UI.
pub fn set_wake(ctx: egui::Context) {
    let _ = WAKE.set(ctx);
}

unsafe extern "C-unwind" fn open_urls_imp(_this: *mut AnyObject, _cmd: Sel, _app: *mut AnyObject, urls: *mut AnyObject) {
    if urls.is_null() {
        return;
    }
    // SAFETY: AppKit passes an NSArray<NSURL> for this selector.
    let urls: &NSArray<NSURL> = unsafe { &*(urls as *const NSArray<NSURL>) };
    let mut paths = Vec::new();
    for u in urls.iter() {
        if let Some(p) = u.path() {
            paths.push(PathBuf::from(p.to_string()));
        }
    }
    if paths.is_empty() {
        return;
    }
    PENDING.lock().unwrap().extend(paths);
    if let Some(ctx) = WAKE.get() {
        ctx.request_repaint();
    }
}

/// Add `application:openURLs:` to the current application delegate's class (once).
fn patch_delegate() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let mut patched = PATCHED.lock().unwrap();
    if *patched {
        return;
    }
    let app = NSApplication::sharedApplication(mtm);
    let Some(delegate) = app.delegate() else { return };
    // SAFETY: `-class` is valid on every object and returns a class pointer.
    let cls: *const AnyClass = unsafe { msg_send![&*delegate, class] };
    if cls.is_null() {
        return;
    }
    let cls: &AnyClass = unsafe { &*cls };
    let selector = sel!(application:openURLs:);
    if cls.responds_to(selector) {
        *patched = true;
        return;
    }
    // SAFETY: standard Objective-C runtime call; the IMP signature matches the type encoding
    // "v@:@@" (void return, self, _cmd, two object arguments).
    let ok = unsafe {
        let imp: objc2::runtime::Imp = std::mem::transmute(open_urls_imp as unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject));
        objc2::ffi::class_addMethod(cls as *const AnyClass as *mut AnyClass, selector, imp, c"v@:@@".as_ptr())
    };
    *patched = ok.as_bool();
    tracing::debug!(patched = *patched, class = %cls.name().to_string_lossy(), "installed application:openURLs: handler");
}

struct Ivars;

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "StepViewLaunchObserver"]
    #[ivars = Ivars]
    struct LaunchObserver;

    unsafe impl NSObjectProtocol for LaunchObserver {}

    impl LaunchObserver {
        #[unsafe(method(applicationWillFinishLaunching:))]
        fn will_finish_launching(&self, _notification: &NSNotification) {
            patch_delegate();
        }
    }
);

impl LaunchObserver {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars);
        // SAFETY: plain NSObject init.
        unsafe { msg_send![super(this), init] }
    }
}

/// Call once on the main thread before starting the event loop.
pub fn install_open_handler() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let observer = LaunchObserver::new(mtm);
    let center = NSNotificationCenter::defaultCenter();
    // SAFETY: observer outlives the process (leaked below); selector exists on the class.
    unsafe {
        center.addObserver_selector_name_object(&observer, sel!(applicationWillFinishLaunching:), Some(NSApplicationWillFinishLaunchingNotification), None);
    }
    std::mem::forget(observer);
    // In case the application already finished launching (e.g. handler installed late).
    patch_delegate();
}
