//! Read-only native media smoke check, running AppKit capture on the main thread.
use omoikane::platform_media::{
    ContrastPreference, MediaPreferenceMonitor, capture_media_preferences,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let captured = capture_media_preferences();
    #[cfg(target_os = "macos")]
    assert!(
        captured.reduced_motion.is_some()
            && captured.contrast.is_some()
            && captured.reduced_transparency.is_some()
            && captured.inverted_colors.is_some(),
        "main-thread AppKit capture must return accessibility settings: {captured:?}"
    );
    #[cfg(target_os = "windows")]
    assert!(
        captured.reduced_motion.is_some() && captured.forced_colors.is_some(),
        "Win32 capture must return accessibility settings: {captured:?}"
    );
    let notifications = Arc::new(AtomicUsize::new(0));
    let wake = notifications.clone();
    // The deliberately stale owned sample checks real native polling delivery.
    // No operating-system preference is modified by this probe.
    let mut stale = captured.clone();
    stale.contrast = if captured.contrast == Some(ContrastPreference::Less) {
        Some(ContrastPreference::More)
    } else {
        Some(ContrastPreference::Less)
    };
    let monitor = MediaPreferenceMonitor::start(stale, Duration::from_millis(20), move || {
        wake.fetch_add(1, Ordering::SeqCst);
    })?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let delivered = loop {
        pump_native_events();
        if let Some(snapshot) = monitor.take_latest() {
            break snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "native monitor did not deliver a fresh snapshot"
        );
    };
    assert!(notifications.load(Ordering::SeqCst) > 0);
    drop(monitor);
    let after_drop = notifications.load(Ordering::SeqCst);
    for _ in 0..5 {
        pump_native_events();
    }
    assert_eq!(
        notifications.load(Ordering::SeqCst),
        after_drop,
        "native monitor must not wake the host after cancellation"
    );
    println!(
        "{}",
        serde_json::json!({
            "status":"PASS", "os":std::env::consts::OS,
            "captured":format!("{captured:?}"), "delivered":format!("{delivered:?}"),
            "notifications":after_drop,
            "scope":"native capture, real monitor delivery and cancellation; no OS settings changed"
        })
    );
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn pump_native_events() {
    std::thread::sleep(Duration::from_millis(20));
}

#[cfg(target_os = "macos")]
fn pump_native_events() {
    use std::ffi::c_void;
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFRunLoopDefaultMode: *const c_void;
        fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, return_after_source: u8) -> i32;
    }
    // SAFETY: main() owns this main-thread probe; CoreFoundation's immutable
    // mode constant stays live, and the call runs the native timer/notification loop.
    unsafe {
        CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.02, 1);
    }
}
