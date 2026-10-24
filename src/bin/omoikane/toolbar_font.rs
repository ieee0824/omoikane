//! GUI-lifetime font I/O; drawing never waits for discovery or decoding.

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::mpsc::TryRecvError;
use std::task::{Wake, Waker};

use omoikane::font::Font;
use omoikane::resource_worker::{
    ResourceKind, ResourceOutcome, ResourcePriority, ResourceQueue, ResourceScope, ResourceTask,
};
use winit::event_loop::EventLoopProxy;

use super::page_worker::AsyncEvent;

struct FontWake(EventLoopProxy<AsyncEvent>);

impl Wake for FontWake {
    fn wake(self: Arc<Self>) {
        let _ = self.0.send_event(AsyncEvent::ToolbarFontReady);
    }
}

/// Owns one scoped chrome job; page navigation keeps its separate queue policy.
pub(super) struct ToolbarFont {
    _queue: ResourceQueue,
    _scope: ResourceScope,
    task: Option<ResourceTask<Option<Font>>>,
    font: Option<Font>,
}

impl ToolbarFont {
    pub(super) fn start(proxy: EventLoopProxy<AsyncEvent>) -> Self {
        Self::with_loader(
            ResourceQueue::new(NonZeroUsize::new(1).unwrap()),
            Waker::from(Arc::new(FontWake(proxy))),
            super::load_toolbar_font,
        )
    }

    fn with_loader(
        queue: ResourceQueue,
        waker: Waker,
        load: impl FnOnce() -> Option<Font> + Send + 'static,
    ) -> Self {
        let scope = ResourceScope::default();
        let task = queue
            .submit_with_waker(
                &scope,
                ResourcePriority::Critical,
                ResourceKind::Font,
                waker,
                move |_| load(),
            )
            .map_err(|error| eprintln!("toolbar font job unavailable: {error}"))
            .ok();
        Self {
            _queue: queue,
            _scope: scope,
            task,
            font: None,
        }
    }

    /// Settles an already-committed result without blocking or running I/O.
    pub(super) fn poll(&mut self) -> bool {
        let Some(task) = self.task.as_ref() else {
            return false;
        };
        let font = match task.try_recv() {
            Err(TryRecvError::Empty) => return false,
            Ok(ResourceOutcome::Complete(font)) => font,
            Ok(ResourceOutcome::Cancelled) => None,
            Ok(ResourceOutcome::Panicked) | Err(TryRecvError::Disconnected) => {
                eprintln!("toolbar font job did not complete; drawing without text");
                None
            }
        };
        self.task = None;
        self.font = font;
        true
    }

    /// Reads the font available for this frame; it never starts loading.
    pub(super) fn get(&self) -> Option<&Font> {
        self.font.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    const WAIT: Duration = Duration::from_secs(5);

    struct Notify(mpsc::Sender<()>);
    impl Wake for Notify {
        fn wake(self: Arc<Self>) {
            let _ = self.0.send(());
        }
    }

    fn notification() -> (Waker, mpsc::Receiver<()>) {
        let (sender, receiver) = mpsc::channel();
        (Waker::from(Arc::new(Notify(sender))), receiver)
    }

    fn queue() -> ResourceQueue {
        ResourceQueue::new(NonZeroUsize::new(1).unwrap())
    }

    #[test]
    fn blocked_font_io_leaves_the_owner_available_and_completion_wakes_it() {
        let (waker, notified) = notification();
        let (started, observed) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let mut font = ToolbarFont::with_loader(queue(), waker, move || {
            started.send(()).unwrap();
            gate.recv_timeout(WAIT).unwrap();
            Some(Font::load_from_bytes(include_bytes!(
                "../../../tests/fixtures/anonymized-font-selection/OmoikaneFixture-Regular.ttf"
            ).to_vec()).unwrap())
        });
        observed.recv_timeout(WAIT).unwrap();
        assert!(font.get().is_none());
        assert!(!font.poll());
        release.send(()).unwrap();
        notified.recv_timeout(WAIT).unwrap();
        assert!(font.poll());
        assert!(font.get().is_some());
        assert!(!font.poll());
    }

    #[test]
    fn a_missing_or_panicking_font_loader_settles_without_retrying() {
        for panic in [false, true] {
            let (waker, notified) = notification();
            let mut font = ToolbarFont::with_loader(queue(), waker, move || {
                if panic {
                    panic!("injected font failure");
                }
                None
            });
            notified.recv_timeout(WAIT).unwrap();
            assert!(font.poll());
            assert!(font.get().is_none());
            assert!(!font.poll());
        }
    }

    #[test]
    fn dropping_the_gui_owner_does_not_wait_for_blocked_font_io() {
        let (waker, notified) = notification();
        let (started, observed) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let font = ToolbarFont::with_loader(queue(), waker, move || {
            started.send(()).unwrap();
            gate.recv_timeout(WAIT).unwrap();
            None
        });
        observed.recv_timeout(WAIT).unwrap();
        drop(font);
        release.send(()).unwrap();
        notified.recv_timeout(WAIT).unwrap();
    }
}
