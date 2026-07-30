//! HTML event-loop scheduling primitives.
//!
//! Tasks are stored in FIFO queues per task source.  `order` records the
//! globally observable enqueue order used by this single browsing-context
//! implementation; keeping the queues separate makes source ownership
//! explicit without changing the deterministic ordering embedders relied on.

use std::collections::{HashMap, VecDeque};

use boa_engine::JsValue;

use super::{NavigationRequest, TimerPayload};

/// The HTML task source responsible for a queued task.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TaskSource {
    Timer,
    DomManipulation,
    UserInteraction,
    Networking,
    Navigation,
    Rendering,
    /// The posted message task source, used by `MessagePort` delivery.
    PostedMessage,
    /// The file reading task source, used by `FileReader` to deliver its
    /// `loadstart`/`progress`/`load`/`loadend` events.
    FileReading,
}

#[derive(Debug, Clone)]
pub(crate) enum Task {
    Timer(TimerPayload),
    Navigation(NavigationRequest),
    PostedMessage(JsValue),
}

#[derive(Debug, Clone)]
struct TimerTask {
    id: u64,
    payload: TimerPayload,
    next_run_at: u64,
    interval_ms: u64,
    repeat: bool,
}

#[derive(Debug)]
pub(crate) struct EventLoop {
    next_task_id: u64,
    queues: HashMap<TaskSource, VecDeque<(u64, Task)>>,
    order: VecDeque<(u64, TaskSource)>,
    next_timer_id: u64,
    now_ms: u64,
    timers: Vec<TimerTask>,
    next_animation_frame_id: u64,
    animation_frame_order: Vec<u64>,
    animation_frame_callbacks: HashMap<u64, JsValue>,
}

impl Default for EventLoop {
    fn default() -> Self {
        Self {
            next_task_id: 1,
            queues: HashMap::new(),
            order: VecDeque::new(),
            next_timer_id: 0,
            now_ms: 0,
            timers: Vec::new(),
            next_animation_frame_id: 0,
            animation_frame_order: Vec::new(),
            animation_frame_callbacks: HashMap::new(),
        }
    }
}

impl EventLoop {
    fn enqueue(&mut self, source: TaskSource, task: Task) {
        let id = self.next_task_id;
        self.next_task_id = self.next_task_id.saturating_add(1);
        self.queues.entry(source).or_default().push_back((id, task));
        self.order.push_back((id, source));
    }

    pub(crate) fn enqueue_timer(&mut self, payload: TimerPayload) {
        let source = match payload {
            TimerPayload::ResourceLoad { .. } => TaskSource::Networking,
            _ => TaskSource::Timer,
        };
        self.enqueue(source, Task::Timer(payload));
    }

    pub(crate) fn enqueue_navigation(&mut self, request: NavigationRequest) {
        self.enqueue(TaskSource::Navigation, Task::Navigation(request));
    }

    /// Queues `payload` on the file reading task source.
    ///
    /// Unlike [`schedule_timer`](Self::schedule_timer) this does not wait for
    /// virtual time to advance: a read that has already produced its bytes owes
    /// its events to the very next turn of the event loop, not to a delay.
    pub(crate) fn enqueue_file_reading(&mut self, payload: TimerPayload) {
        self.enqueue(TaskSource::FileReading, Task::Timer(payload));
    }

    /// Queues a callback on the networking task source.
    pub(crate) fn enqueue_networking(&mut self, payload: TimerPayload) {
        self.enqueue(TaskSource::Networking, Task::Timer(payload));
    }

    /// Queues a `MessagePort` delivery callback on the posted message task
    /// source. The callback is retained as a live JavaScript value until its
    /// event-loop turn runs.
    pub(crate) fn enqueue_posted_message(&mut self, callback: JsValue) {
        self.enqueue(TaskSource::PostedMessage, Task::PostedMessage(callback));
    }

    pub(crate) fn pop_task(&mut self) -> Option<(TaskSource, Task)> {
        while let Some((expected_id, source)) = self.order.pop_front() {
            let queue = self.queues.get_mut(&source)?;
            let Some((id, task)) = queue.pop_front() else {
                continue;
            };
            debug_assert_eq!(id, expected_id);
            return Some((source, task));
        }
        None
    }

    pub(crate) fn schedule_timer(
        &mut self,
        payload: TimerPayload,
        delay_ms: u64,
        repeat: bool,
    ) -> u64 {
        let id = self.next_timer_id;
        self.next_timer_id = self.next_timer_id.saturating_add(1);
        self.timers.push(TimerTask {
            id,
            payload,
            next_run_at: self.now_ms.saturating_add(delay_ms),
            interval_ms: delay_ms,
            repeat,
        });
        id
    }

    pub(crate) fn clear_timer(&mut self, id: u64) {
        self.timers.retain(|timer| timer.id != id);
    }

    pub(crate) fn advance(&mut self, elapsed_ms: u64) {
        self.now_ms = self.now_ms.saturating_add(elapsed_ms);
        let mut ready = Vec::new();
        for timer in &mut self.timers {
            if timer.next_run_at <= self.now_ms {
                ready.push((timer.next_run_at, timer.id, timer.payload.clone()));
                if timer.repeat {
                    timer.next_run_at = self.now_ms.saturating_add(timer.interval_ms);
                }
            }
        }
        ready.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        self.timers
            .retain(|timer| timer.repeat || timer.next_run_at > self.now_ms);
        for (_, _, payload) in ready {
            self.enqueue_timer(payload);
        }
    }

    pub(crate) fn has_pending_timers(&self) -> bool {
        !self.timers.is_empty()
            || self
                .queues
                .values()
                .any(|queue| queue.iter().any(|(_, task)| matches!(task, Task::Timer(_))))
    }

    pub(crate) fn schedule_animation_frame(&mut self, callback: JsValue) -> u64 {
        self.next_animation_frame_id = self.next_animation_frame_id.saturating_add(1);
        let id = self.next_animation_frame_id;
        self.animation_frame_order.push(id);
        self.animation_frame_callbacks.insert(id, callback);
        id
    }

    pub(crate) fn cancel_animation_frame(&mut self, id: u64) {
        self.animation_frame_callbacks.remove(&id);
    }

    pub(crate) fn begin_animation_frame(&mut self) -> (f64, Vec<u64>) {
        (
            self.now_ms as f64,
            std::mem::take(&mut self.animation_frame_order),
        )
    }

    pub(crate) fn take_animation_frame_callback(&mut self, id: u64) -> Option<JsValue> {
        self.animation_frame_callbacks.remove(&id)
    }

    pub(crate) fn has_pending_animation_frames(&self) -> bool {
        !self.animation_frame_order.is_empty()
    }

    pub(crate) fn rendering_time_ms(&self) -> f64 {
        self.now_ms as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_sources_keep_fifo_enqueue_order() {
        let mut event_loop = EventLoop::default();
        event_loop.enqueue_navigation(NavigationRequest::Reload);
        event_loop.enqueue_timer(TimerPayload::Source("timer".into()));
        assert!(matches!(
            event_loop.pop_task(),
            Some((TaskSource::Navigation, Task::Navigation(_)))
        ));
        assert!(matches!(
            event_loop.pop_task(),
            Some((TaskSource::Timer, Task::Timer(_)))
        ));
    }

    #[test]
    fn file_reading_tasks_run_in_enqueue_order_without_advancing_time() {
        let mut event_loop = EventLoop::default();
        event_loop.enqueue_file_reading(TimerPayload::Source("first".into()));
        event_loop.enqueue_timer(TimerPayload::Source("timer".into()));
        event_loop.enqueue_file_reading(TimerPayload::Source("second".into()));

        let sources: Vec<_> = std::iter::from_fn(|| event_loop.pop_task())
            .map(|(source, task)| {
                let label = match task {
                    Task::Timer(TimerPayload::Source(source)) => source,
                    other => panic!("unexpected task: {other:?}"),
                };
                (source, label)
            })
            .collect();

        assert_eq!(
            sources,
            vec![
                (TaskSource::FileReading, "first".to_string()),
                (TaskSource::Timer, "timer".to_string()),
                (TaskSource::FileReading, "second".to_string()),
            ]
        );
    }

    #[test]
    fn queued_file_reading_tasks_count_as_pending_work() {
        let mut event_loop = EventLoop::default();
        assert!(!event_loop.has_pending_timers());
        event_loop.enqueue_file_reading(TimerPayload::Source("read".into()));
        assert!(event_loop.has_pending_timers());
        assert!(event_loop.pop_task().is_some());
        assert!(!event_loop.has_pending_timers());
    }

    #[test]
    fn networking_callbacks_keep_global_task_enqueue_order() {
        let mut event_loop = EventLoop::default();
        event_loop.enqueue_networking(TimerPayload::Source("open".into()));
        event_loop.enqueue_timer(TimerPayload::Source("timer".into()));
        event_loop.enqueue_networking(TimerPayload::Source("message".into()));
        let labels: Vec<_> = std::iter::from_fn(|| event_loop.pop_task())
            .map(|(_, task)| match task { Task::Timer(TimerPayload::Source(value)) => value, _ => panic!("unexpected task") })
            .collect();
        assert_eq!(labels, ["open", "timer", "message"]);
    }

    #[test]
    fn posted_message_callbacks_keep_fifo_order() {
        let mut event_loop = EventLoop::default();
        event_loop.enqueue_posted_message(JsValue::from(1));
        event_loop.enqueue_posted_message(JsValue::from(2));

        for expected in [1.0, 2.0] {
            let Some((source, Task::PostedMessage(callback))) = event_loop.pop_task() else {
                panic!("expected a posted message task");
            };
            assert_eq!(source, TaskSource::PostedMessage);
            assert_eq!(callback.as_number(), Some(expected));
        }
        assert!(event_loop.pop_task().is_none());
    }

    #[test]
    fn posted_messages_preserve_global_enqueue_order() {
        let mut event_loop = EventLoop::default();
        event_loop.enqueue_timer(TimerPayload::Source("timer".into()));
        event_loop.enqueue_posted_message(JsValue::from(7));
        event_loop.enqueue_navigation(NavigationRequest::Reload);

        assert!(matches!(
            event_loop.pop_task(),
            Some((TaskSource::Timer, Task::Timer(_)))
        ));
        assert!(matches!(
            event_loop.pop_task(),
            Some((TaskSource::PostedMessage, Task::PostedMessage(_)))
        ));
        assert!(matches!(
            event_loop.pop_task(),
            Some((TaskSource::Navigation, Task::Navigation(_)))
        ));
    }
}
