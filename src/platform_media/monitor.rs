//! Sampling lives on a cancellable worker; the GUI receives owned snapshots.
use super::PlatformMediaPreferences;
#[cfg(not(all(target_os = "macos", feature = "gui")))]
use super::capture_media_preferences;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

/// Owns a background native-preference sampler and its pending updates.
/// Dropping the monitor cancels future samples. An in-flight OS request may
/// finish after drop, but cannot publish another update or retain the GUI.
pub struct MediaPreferenceMonitor {
    updates: Receiver<PlatformMediaPreferences>,
    stop: Sender<()>,
    #[cfg(all(target_os = "macos", feature = "gui"))]
    _native: Option<super::macos::Monitor>,
}

impl MediaPreferenceMonitor {
    /// Samples native preferences at the given interval, notifying only when
    /// the snapshot changes. The callback runs on the sampler thread and must
    /// only wake the presentation host; it must not invoke JavaScript or layout.
    /// Intervals shorter than ten milliseconds are clamped. macOS uses native
    /// accessibility notifications on the main operation queue and an input
    /// inventory timer on the main run loop;
    /// its monitor must be created and dropped on the main thread.
    pub fn start(
        initial: PlatformMediaPreferences,
        interval: Duration,
        notify: impl Fn() + Send + 'static,
    ) -> std::io::Result<Self> {
        #[cfg(all(target_os = "macos", feature = "gui"))]
        {
            let (send, updates) = mpsc::channel();
            let (stop, _) = mpsc::channel();
            let native = super::macos::Monitor::new(initial, interval, send, notify)?;
            Ok(Self {
                updates,
                stop,
                _native: Some(native),
            })
        }
        #[cfg(not(all(target_os = "macos", feature = "gui")))]
        {
            Self::with_capture(initial, interval, capture_media_preferences, notify)
        }
    }

    /// Drains queued updates and returns the newest owned snapshot, if any.
    pub fn take_latest(&self) -> Option<PlatformMediaPreferences> {
        self.updates.try_iter().last()
    }

    #[cfg(any(not(all(target_os = "macos", feature = "gui")), test))]
    fn with_capture(
        initial: PlatformMediaPreferences,
        interval: Duration,
        mut capture: impl FnMut() -> PlatformMediaPreferences + Send + 'static,
        notify: impl Fn() + Send + 'static,
    ) -> std::io::Result<Self> {
        let (send, updates) = mpsc::channel();
        let (stop, stopping) = mpsc::channel();
        let interval = interval.max(Duration::from_millis(10));
        std::thread::Builder::new()
            .name("media-preferences".into())
            .spawn(move || {
                let mut previous = initial;
                loop {
                    match stopping.recv_timeout(interval) {
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        _ => return,
                    }
                    let next = capture();
                    // Cancellation during capture must not wake the closed host.
                    if !matches!(stopping.try_recv(), Err(mpsc::TryRecvError::Empty)) {
                        return;
                    }
                    if next != previous {
                        previous = next.clone();
                        if send.send(next).is_err() {
                            return;
                        }
                        notify();
                    }
                }
            })?;
        Ok(Self {
            updates,
            stop,
            #[cfg(all(target_os = "macos", feature = "gui"))]
            _native: None,
        })
    }
}

impl Drop for MediaPreferenceMonitor {
    fn drop(&mut self) {
        let _ = self.stop.send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_publish_owned_snapshots_but_identical_samples_do_not_notify() {
        let initial = PlatformMediaPreferences::default();
        let changed = PlatformMediaPreferences {
            reduced_motion: Some(true),
            ..initial.clone()
        };
        assert_snapshot_delivery(initial, changed);
    }

    #[test]
    fn palette_only_changes_notify_without_repeating_identical_samples() {
        let initial = PlatformMediaPreferences {
            forced_colors: Some(true),
            forced_color_palette: Some(crate::css::ForcedColorPalette::for_color_scheme(false)),
            ..Default::default()
        };
        let changed = PlatformMediaPreferences {
            forced_color_palette: Some(crate::css::ForcedColorPalette::for_color_scheme(true)),
            ..initial.clone()
        };
        assert_snapshot_delivery(initial, changed);
    }

    fn assert_snapshot_delivery(
        initial: PlatformMediaPreferences,
        changed: PlatformMediaPreferences,
    ) {
        let (samples, receiving) = mpsc::channel();
        let (notifications, notified) = mpsc::channel();
        let (requests, requested) = mpsc::channel();
        let monitor = MediaPreferenceMonitor::with_capture(
            initial.clone(),
            Duration::ZERO,
            move || {
                requests.send(()).unwrap();
                receiving.recv().unwrap()
            },
            move || {
                notifications.send(()).unwrap();
            },
        )
        .unwrap();
        requested.recv_timeout(Duration::from_secs(2)).unwrap();
        samples.send(initial).unwrap();
        requested.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(monitor.take_latest().is_none());
        assert!(notified.try_recv().is_err());
        samples.send(changed.clone()).unwrap();
        notified.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(monitor.take_latest(), Some(changed.clone()));
        requested.recv_timeout(Duration::from_secs(2)).unwrap();
        samples.send(changed.clone()).unwrap();
        requested.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(monitor.take_latest().is_none());
        assert!(notified.try_recv().is_err());
        drop(monitor);
        samples
            .send(PlatformMediaPreferences {
                reduced_motion: Some(false),
                ..changed
            })
            .unwrap();
        assert!(matches!(
            notified.recv_timeout(Duration::from_secs(2)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));
        assert!(matches!(
            requested.recv_timeout(Duration::from_secs(2)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));
    }
}
