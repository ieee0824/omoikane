//! Long Tasks (Issue #1293).
//!
//! Each outermost task turn (a queued task, a parser-run script or the
//! animation frame callbacks of a rendering opportunity) is timed on the
//! performance clock. Turns longer than 50ms are reported to the main
//! Document's script as `longtask` entries.

use super::*;

/// The long task threshold from Long Tasks §3.
pub(super) const LONG_TASK_THRESHOLD_MS: f64 = 50.0;

/// Returns the `(startTime, duration)` of a turn that ran from `start_ms` to
/// `end_ms`, when it is a long task. Both are whole milliseconds, which
/// keeps the clock coarse as Long Tasks §4 asks.
pub(super) fn long_task(start_ms: f64, end_ms: f64) -> Option<(f64, f64)> {
    (end_ms - start_ms > LONG_TASK_THRESHOLD_MS)
        .then(|| (start_ms.floor(), end_ms.ceil() - start_ms.floor()))
}

impl JsRuntime {
    /// The performance clock in milliseconds.
    fn performance_clock_ms(&self) -> f64 {
        self.host_state
            .borrow()
            .performance_start
            .elapsed()
            .as_secs_f64()
            * 1000.0
    }

    /// Starts timing a task turn. Returns `None` inside another turn, whose
    /// duration already includes this one.
    pub(super) fn begin_task_turn(&mut self) -> Option<f64> {
        self.task_turn_depth += 1;
        (self.task_turn_depth == 1).then(|| self.performance_clock_ms())
    }

    /// Ends the turn begun by [`Self::begin_task_turn`] and reports it when
    /// it is a long task.
    pub(super) fn end_task_turn(&mut self, start_ms: Option<f64>) {
        self.task_turn_depth = self.task_turn_depth.saturating_sub(1);
        let Some(start_ms) = start_ms else {
            return;
        };
        let Some((start, duration)) = long_task(start_ms, self.performance_clock_ms()) else {
            return;
        };
        let document_id = self.host_state.borrow().document.identity();
        let result = self.eval_in_document_realm(
            document_id,
            &format!(
                "if (typeof globalThis.__omoikane_record_long_task === 'function') \
                 globalThis.__omoikane_record_long_task({start}, {duration});"
            ),
        );
        self.record_error_from("long task timing", result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_turns_over_fifty_milliseconds_are_long_tasks() {
        assert_eq!(long_task(10.0, 60.0), None);
        assert_eq!(long_task(10.0, 60.5), Some((10.0, 51.0)));
        assert_eq!(long_task(10.4, 70.2), Some((10.0, 61.0)));
    }
}
