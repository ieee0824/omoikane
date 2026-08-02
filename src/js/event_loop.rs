//! HTML event-loop scheduling primitives.
//!
//! Tasks are stored in FIFO queues per task source.  `order` records the
//! globally observable enqueue order used by this single browsing-context
//! implementation; keeping the queues separate makes source ownership
//! explicit without changing the deterministic ordering embedders relied on.

use std::collections::{HashMap, VecDeque};

use boa_engine::JsValue;
use boa_gc::{Finalize, Trace, Tracer};

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
    /// The geolocation task source, used to deliver position/error callbacks.
    Geolocation,
}

#[derive(Debug, Clone)]
pub(crate) enum Task {
    Timer(TimerPayload),
    /// A geolocation request delivered after the current script task.
    Geolocation { request_id: u64 },
    Navigation(NavigationRequest),
    PostedMessage { port: JsValue, data: JsValue },
    /// A message delivered to a page-owned `BroadcastChannel` endpoint.
    ///
    /// The payload stays in the context-independent structured-clone wire
    /// format until the target runtime's task turn, where it is rehydrated
    /// with that realm's constructors and prototypes.
    BroadcastChannelMessage {
        channel_id: u64,
        data: String,
        origin: String,
    },
    /// A message sent from a page-owned `Worker` to its dedicated worker.
    WorkerMessage { worker_id: u64, data: String },
    /// A message sent from a dedicated worker back to its owner page.
    WorkerOwnerMessage {
        worker_id: u64,
        owner: JsValue,
        data: String,
    },
    /// A message sent from a page-owned `SharedWorkerPort` to the shared
    /// worker runtime.  The endpoint is identified by a process-local id;
    /// the structured-clone wire is decoded in the target realm.
    SharedWorkerMessage { connection_id: u64, data: String },
    /// A message sent from a shared worker runtime to a page-owned port.
    SharedWorkerOwnerMessage {
        connection_id: u64,
        port: JsValue,
        data: String,
        origin: String,
    },
    /// A worker startup/runtime failure reported to its owner page.
    WorkerError {
        worker_id: u64,
        owner: Option<JsValue>,
        message: String,
    },
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

impl Finalize for EventLoop {}

// JavaScript callbacks and message values live in the host-side event loop rather
// than in Boa's VM stack. They therefore need to be exposed through the runtime's
// host root provider while the event loop retains them.
unsafe impl Trace for EventLoop {
    unsafe fn trace(&self, tracer: &mut Tracer) {
        for queue in self.queues.values() {
            for (_, task) in queue {
                unsafe { trace_task(task, tracer) };
            }
        }
        for timer in &self.timers {
            unsafe { trace_timer_payload(&timer.payload, tracer) };
        }
        for callback in self.animation_frame_callbacks.values() {
            unsafe { callback.trace(tracer) };
        }
    }

    fn run_finalizer(&self) {}
}

unsafe fn trace_timer_payload(payload: &TimerPayload, tracer: &mut Tracer) {
    match payload {
        TimerPayload::Callback { callback, args } => {
            unsafe { callback.trace(tracer) };
            for arg in args {
                unsafe { arg.trace(tracer) };
            }
        }
        TimerPayload::Realm { payload, .. } => unsafe { trace_timer_payload(payload, tracer) },
        TimerPayload::Source(_)
        | TimerPayload::ResourceLoad { .. }
        | TimerPayload::GeolocationTimeout { .. } => {}
    }
}

unsafe fn trace_task(task: &Task, tracer: &mut Tracer) {
    match task {
        Task::Timer(payload) => unsafe { trace_timer_payload(payload, tracer) },
        Task::PostedMessage { port, data } => {
            unsafe { port.trace(tracer) };
            unsafe { data.trace(tracer) };
        }
        Task::WorkerOwnerMessage { owner, .. } => unsafe { owner.trace(tracer) },
        Task::SharedWorkerOwnerMessage { port, .. } => unsafe { port.trace(tracer) },
        Task::WorkerError { owner: Some(owner), .. } => unsafe { owner.trace(tracer) },
        Task::Geolocation { .. }
        | Task::Navigation(_)
        | Task::BroadcastChannelMessage { .. }
        | Task::WorkerMessage { .. }
        | Task::SharedWorkerMessage { .. }
        | Task::WorkerError { owner: None, .. } => {}
    }
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
            TimerPayload::GeolocationTimeout { .. } => TaskSource::Geolocation,
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

    /// Queues a callback on the DOM manipulation task source.
    pub(crate) fn enqueue_dom_manipulation(&mut self, payload: TimerPayload) {
        self.enqueue(TaskSource::DomManipulation, Task::Timer(payload));
    }

    /// Queues a geolocation delivery on its own task source.
    pub(crate) fn enqueue_geolocation(&mut self, request_id: u64) {
        self.enqueue(TaskSource::Geolocation, Task::Geolocation { request_id });
    }

    /// Queues a port and cloned data on the posted message task source.
    /// Both values stay live until their event-loop turn runs.
    pub(crate) fn enqueue_posted_message(&mut self, port: JsValue, data: JsValue) {
        self.enqueue(TaskSource::PostedMessage, Task::PostedMessage { port, data });
    }

    /// Queues a message on a target `BroadcastChannel`'s posted-message task
    /// source.  The receiver channel is identified by a per-runtime id so a
    /// `JsValue` never crosses Boa realms.
    pub(crate) fn enqueue_broadcast_channel_message(
        &mut self,
        channel_id: u64,
        data: String,
        origin: String,
    ) {
        self.enqueue(
            TaskSource::PostedMessage,
            Task::BroadcastChannelMessage {
                channel_id,
                data,
                origin,
            },
        );
    }

    pub(crate) fn enqueue_worker_message(&mut self, worker_id: u64, data: String) {
        self.enqueue(TaskSource::PostedMessage, Task::WorkerMessage { worker_id, data });
    }

    pub(crate) fn enqueue_worker_owner_message(
        &mut self,
        worker_id: u64,
        owner: JsValue,
        data: String,
    ) {
        self.enqueue(
            TaskSource::PostedMessage,
            Task::WorkerOwnerMessage {
                worker_id,
                owner,
                data,
            },
        );
    }

    pub(crate) fn enqueue_shared_worker_message(&mut self, connection_id: u64, data: String) {
        self.enqueue(
            TaskSource::PostedMessage,
            Task::SharedWorkerMessage { connection_id, data },
        );
    }

    pub(crate) fn enqueue_shared_worker_owner_message(
        &mut self,
        connection_id: u64,
        port: JsValue,
        data: String,
        origin: String,
    ) {
        self.enqueue(
            TaskSource::PostedMessage,
            Task::SharedWorkerOwnerMessage {
                connection_id,
                port,
                data,
                origin,
            },
        );
    }

    pub(crate) fn enqueue_worker_error(
        &mut self,
        worker_id: u64,
        owner: Option<JsValue>,
        message: String,
    ) {
        self.enqueue(
            TaskSource::PostedMessage,
            Task::WorkerError { worker_id, owner, message },
        );
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

    pub(crate) fn now_ms(&self) -> u64 {
        self.now_ms
    }

    pub(crate) fn has_pending_timers(&self) -> bool {
        !self.timers.is_empty()
            || self
                .queues
                .values()
                .any(|queue| queue.iter().any(|(_, task)| matches!(task, Task::Timer(_))))
    }

    pub(crate) fn has_pending_geolocation_tasks(&self) -> bool {
        self.queues
            .get(&TaskSource::Geolocation)
            .is_some_and(|queue| queue.iter().any(|(_, task)| matches!(task, Task::Geolocation { .. })))
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
    fn geolocation_tasks_use_their_own_source() {
        let mut event_loop = EventLoop::default();
        event_loop.enqueue_geolocation(7);
        assert!(matches!(
            event_loop.pop_task(),
            Some((TaskSource::Geolocation, Task::Geolocation { request_id: 7 }))
        ));
    }

    #[test]
    fn posted_message_callbacks_keep_fifo_order() {
        let mut event_loop = EventLoop::default();
        event_loop.enqueue_posted_message(JsValue::from(1), JsValue::undefined());
        event_loop.enqueue_posted_message(JsValue::from(2), JsValue::undefined());

        for expected in [1.0, 2.0] {
            let Some((source, Task::PostedMessage { port: callback, .. })) =
                event_loop.pop_task()
            else {
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
        event_loop.enqueue_posted_message(JsValue::from(7), JsValue::undefined());
        event_loop.enqueue_navigation(NavigationRequest::Reload);

        assert!(matches!(
            event_loop.pop_task(),
            Some((TaskSource::Timer, Task::Timer(_)))
        ));
        assert!(matches!(
            event_loop.pop_task(),
            Some((TaskSource::PostedMessage, Task::PostedMessage { .. }))
        ));
        assert!(matches!(
            event_loop.pop_task(),
            Some((TaskSource::Navigation, Task::Navigation(_)))
        ));
    }
}
