//! Retain pipeline task failures without stopping later work.

use super::{JsResult, JsRuntime};

impl JsRuntime {
    /// Retains a raw failure before subsequent jobs or collection can run.
    /// Script diagnostics additionally print the existing timer log format.
    pub(super) fn record_host_task_result(
        &mut self,
        kind: &str,
        phase: &str,
        result: JsResult<()>,
    ) {
        let Err(error) = result else { return };
        if std::env::var_os("OMOIKANE_LOG_SCRIPTS").is_some() {
            let category = if phase == "jobs" {
                "timer-job-error"
            } else {
                "timer-error"
            };
            eprintln!("[omoikane][{category}] {error}");
        }
        self.record_error_from::<()>(&format!("host task {kind} {phase}"), Err(error));
    }
}

#[cfg(test)]
mod tests {
    use crate::js::JsRuntime;
    use boa_engine::JsNativeError;
    use boa_engine::job::GenericJob;

    #[test]
    fn host_resource_failure_is_retained_while_later_timers_continue() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                "globalThis.frame=document.createElement('iframe'); \
             frame.srcdoc='<p>first</p>'; document.body.appendChild(frame); \
             globalThis.afterFailure=false;",
            )
            .unwrap();
        runtime.run_timers(100, 1, 10);
        assert!(runtime.take_task_errors().is_empty());
        runtime.eval("frame.srcdoc='<p>second</p>'").unwrap();
        runtime.host_state.borrow_mut().next_iframe_generation = u64::MAX;
        runtime.set_timeout("globalThis.afterFailure=true", 0);
        runtime.run_timers(100, 1, 10);
        assert_eq!(
            runtime.eval("afterFailure").unwrap().as_boolean(),
            Some(true)
        );
        let errors = runtime.take_task_errors();
        assert_eq!(
            errors.len(),
            1,
            "host failure must be retained exactly once: {errors:?}"
        );
        assert!(
            errors[0].contains("iframe document generation"),
            "{errors:?}"
        );
        assert!(runtime.take_task_errors().is_empty());
    }

    #[test]
    fn host_job_failure_is_retained_while_later_timers_continue() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                "globalThis.afterFailure=false; setTimeout(()=>{},0); \
             setTimeout(()=>{afterFailure=true},0);",
            )
            .unwrap();
        let realm = runtime.context.realm().clone();
        runtime.context.enqueue_job(
            GenericJob::new(
                |_| {
                    Err(JsNativeError::error()
                        .with_message("host-job-failure")
                        .into())
                },
                realm,
            )
            .into(),
        );
        runtime.run_timers(100, 1, 10);
        assert_eq!(
            runtime.eval("afterFailure").unwrap().as_boolean(),
            Some(true)
        );
        let errors = runtime.take_task_errors();
        assert_eq!(
            errors.len(),
            1,
            "host job failure must be retained exactly once: {errors:?}"
        );
        assert!(errors[0].contains("host-job-failure"), "{errors:?}");
    }

    #[test]
    fn author_timer_failure_is_not_recorded_twice() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                "globalThis.afterFailure=false; \
             setTimeout(()=>{throw new Error('author-callback')},0); \
             setTimeout(()=>{afterFailure=true},0);",
            )
            .unwrap();
        runtime.run_timers(100, 1, 10);
        assert_eq!(
            runtime.eval("afterFailure").unwrap().as_boolean(),
            Some(true)
        );
        let errors = runtime.take_task_errors();
        assert_eq!(
            errors.len(),
            1,
            "author failure must be retained exactly once: {errors:?}"
        );
        assert!(errors[0].contains("author-callback"), "{errors:?}");
    }
}
