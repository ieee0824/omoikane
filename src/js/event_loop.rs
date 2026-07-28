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
}

#[derive(Debug, Clone)]
pub(crate) enum Task {
    Timer(TimerPayload),
    Navigation(NavigationRequest),
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
}
