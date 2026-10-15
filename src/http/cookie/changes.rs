//! Owned, coalesced host wakeups for a shared cookie jar.

use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, Ordering},
};
use std::task::Waker;

/// Keeps a host subscribed to script-visible cookie changes.
///
/// Dropping the last clone stops notifications. The waker must schedule host
/// work without synchronously re-entering the jar: a mutation can wake it while
/// the caller holds the shared jar's mutex. Notifications carry no cookie data;
/// the receiving Window still applies its normal origin and policy checks.
#[derive(Debug, Clone)]
#[must_use = "keep the subscription alive to receive cookie change wakeups"]
pub struct CookieChangeSubscription(Arc<ChangeSignal>);

impl CookieChangeSubscription {
    /// Acknowledges pending changes and allows the next mutation to wake again.
    /// Returns whether at least one mutation arrived since the last acknowledgement.
    pub fn take_changed(&self) -> bool {
        self.0.pending.swap(false, Ordering::AcqRel)
    }
}

#[derive(Debug)]
struct ChangeSignal {
    pending: AtomicBool,
    waker: Waker,
}

#[derive(Debug, Default)]
pub(super) struct ChangeSubscriptions(Vec<Weak<ChangeSignal>>);

impl Clone for ChangeSubscriptions {
    fn clone(&self) -> Self {
        // A copied jar owns an independent cookie history. Its mutations must
        // not notify hosts subscribed to the original jar.
        Self::default()
    }
}

impl ChangeSubscriptions {
    pub(super) fn subscribe(&mut self, waker: Waker) -> CookieChangeSubscription {
        let signal = Arc::new(ChangeSignal {
            pending: AtomicBool::new(false),
            waker,
        });
        self.0.retain(|entry| entry.strong_count() != 0);
        self.0.push(Arc::downgrade(&signal));
        CookieChangeSubscription(signal)
    }

    pub(super) fn notify(&mut self) {
        self.0.retain(|entry| {
            let Some(signal) = entry.upgrade() else {
                return false;
            };
            if !signal.pending.swap(true, Ordering::AcqRel) {
                signal.waker.wake_by_ref();
            }
            true
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::task::Wake;

    #[derive(Default)]
    struct Counter(AtomicUsize);

    impl Wake for Counter {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn cookie_wakeups_coalesce_until_acknowledged_and_stop_on_drop() {
        let mut jar = super::super::CookieJar::new();
        let counter = Arc::new(Counter::default());
        let subscription = jar.subscribe_changes(Waker::from(Arc::clone(&counter)));
        assert!(!subscription.take_changed());
        jar.add_from_header("first=1; Path=/", "example.test");
        jar.add_from_header("second=2; Path=/", "example.test");
        assert_eq!(counter.0.load(Ordering::SeqCst), 1);
        assert!(subscription.take_changed());
        assert!(!subscription.take_changed());
        jar.add_from_header("third=3; Path=/", "example.test");
        assert_eq!(counter.0.load(Ordering::SeqCst), 2);
        assert!(subscription.take_changed());
        jar.add_from_header("secret=1; HttpOnly; Path=/", "example.test");
        assert!(!subscription.take_changed());
        drop(subscription);
        jar.add_from_header("fourth=4; Path=/", "example.test");
        assert_eq!(counter.0.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn cloned_jar_does_not_wake_original_subscribers() {
        let mut jar = super::super::CookieJar::new();
        let counter = Arc::new(Counter::default());
        let subscription = jar.subscribe_changes(Waker::from(Arc::clone(&counter)));
        let mut copied = jar.clone();
        copied.add_from_header("copy=1; Path=/", "example.test");
        assert_eq!(counter.0.load(Ordering::SeqCst), 0);
        assert!(!subscription.take_changed());
        jar.add_from_header("original=1; Path=/", "example.test");
        assert_eq!(counter.0.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn hosts_acknowledge_independently_and_cloned_subscription_keeps_listening() {
        let mut jar = super::super::CookieJar::new();
        let first = Arc::new(Counter::default());
        let second = Arc::new(Counter::default());
        let first_subscription = jar.subscribe_changes(Waker::from(Arc::clone(&first)));
        let second_subscription = jar.subscribe_changes(Waker::from(Arc::clone(&second)));
        let retained = first_subscription.clone();
        drop(first_subscription);
        jar.add_from_header("first=1; Path=/", "example.test");
        assert!(retained.take_changed());
        jar.add_from_header("second=2; Path=/", "example.test");
        assert_eq!(first.0.load(Ordering::SeqCst), 2);
        assert_eq!(second.0.load(Ordering::SeqCst), 1);
        assert!(second_subscription.take_changed());
        drop(retained);
        jar.add_from_header("third=3; Path=/", "example.test");
        assert_eq!(first.0.load(Ordering::SeqCst), 2);
        assert_eq!(second.0.load(Ordering::SeqCst), 2);
    }
}
