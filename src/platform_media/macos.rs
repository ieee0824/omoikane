//! Main-thread AppKit accessibility capture and owned notification registration.
use super::{ContrastPreference, PlatformMediaPreferences};
use block2::RcBlock;
use objc2::rc::{Retained, autoreleasepool};
use objc2_app_kit::{NSWorkspace, NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification};
use objc2_foundation::{
    NSNotification, NSNotificationCenter, NSObject, NSOperationQueue, NSThread, NSTimer,
};
use std::cell::RefCell;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::mpsc::Sender;

pub(super) fn capture() -> PlatformMediaPreferences {
    if !NSThread::isMainThread_class() {
        return PlatformMediaPreferences::default();
    }
    autoreleasepool(|_| {
        // SAFETY: this caller is on the main thread. The owned shared workspace
        // remains live through these read-only AppKit accessibility calls.
        unsafe {
            let workspace = NSWorkspace::sharedWorkspace();
            PlatformMediaPreferences {
                pointers: super::macos_input::capture(),
                reduced_motion: Some(workspace.accessibilityDisplayShouldReduceMotion()),
                contrast: Some(ContrastPreference::from_higher(
                    workspace.accessibilityDisplayShouldIncreaseContrast(),
                )),
                reduced_transparency: Some(
                    workspace.accessibilityDisplayShouldReduceTransparency(),
                ),
                inverted_colors: Some(workspace.accessibilityDisplayShouldInvertColors()),
                ..PlatformMediaPreferences::default()
            }
        }
    })
}

pub(super) struct Monitor {
    center: Retained<NSNotificationCenter>,
    observer: Retained<NSObject>,
    timer: Retained<NSTimer>,
    _main_thread: std::marker::PhantomData<Rc<()>>,
}

impl Monitor {
    pub(super) fn new(
        initial: PlatformMediaPreferences,
        interval: std::time::Duration,
        updates: Sender<PlatformMediaPreferences>,
        notify: impl Fn() + Send + 'static,
    ) -> std::io::Result<Self> {
        if !NSThread::isMainThread_class() {
            return Err(std::io::Error::other(
                "AppKit preference monitor requires the main thread",
            ));
        }
        let previous = Rc::new(RefCell::new(initial));
        let publish = Rc::new(move || {
            let next = capture();
            if next != *previous.borrow() {
                *previous.borrow_mut() = next.clone();
                if updates.send(next).is_ok() {
                    notify();
                }
            }
        });
        let on_notification = publish.clone();
        let block = RcBlock::new(move |_: NonNull<NSNotification>| on_notification());
        let timer_block = RcBlock::new(move |_: NonNull<NSTimer>| publish());
        autoreleasepool(|_| {
            // SAFETY: registration and reads occur on the main thread. The
            // notification center copies the block and runs it on the main
            // operation queue. Both center and observer are retained by this
            // owner and registration is removed before either can be released.
            unsafe {
                let center = NSWorkspace::sharedWorkspace().notificationCenter();
                let queue = NSOperationQueue::mainQueue();
                let observer = center.addObserverForName_object_queue_usingBlock(
                    Some(NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification),
                    None,
                    Some(&queue),
                    &block,
                );
                // The main-thread run loop owns the scheduled timer; this
                // retained owner invalidates it before releasing the block.
                let timer = NSTimer::scheduledTimerWithTimeInterval_repeats_block(
                    interval
                        .max(std::time::Duration::from_millis(10))
                        .as_secs_f64(),
                    true,
                    &timer_block,
                );
                Ok(Self {
                    center,
                    observer,
                    timer,
                    _main_thread: std::marker::PhantomData,
                })
            }
        })
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        // SAFETY: this non-Send AppKit owner is destroyed on its creating
        // main thread, while both retained center and observer are live.
        unsafe {
            self.timer.invalidate();
            self.center.removeObserver(&self.observer);
        }
    }
}
