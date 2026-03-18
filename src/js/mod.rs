//! JavaScript engine embedding and event loop primitives.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use boa_engine::native_function::NativeFunction;
use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue, Source, js_string};

thread_local! {
    static ACTIVE_EVENT_LOOP: RefCell<Option<Rc<RefCell<EventLoopState>>>> = const { RefCell::new(None) };
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TimerTask {
    id: u64,
    source: String,
    next_run_at: u64,
    interval_ms: u64,
    repeat: bool,
}

#[derive(Debug, Default)]
struct EventLoopState {
    next_timer_id: u64,
    now_ms: u64,
    macrotasks: VecDeque<String>,
    timers: Vec<TimerTask>,
}

impl EventLoopState {
    fn schedule_timer(&mut self, source: String, delay_ms: u64, repeat: bool) -> u64 {
        let id = self.next_timer_id;
        self.next_timer_id += 1;
        self.timers.push(TimerTask {
            id,
            source,
            next_run_at: self.now_ms.saturating_add(delay_ms),
            interval_ms: delay_ms,
            repeat,
        });
        id
    }

    fn clear_timer(&mut self, id: u64) {
        self.timers.retain(|timer| timer.id != id);
    }

    fn advance(&mut self, elapsed_ms: u64) {
        self.now_ms = self.now_ms.saturating_add(elapsed_ms);

        let mut ready = Vec::new();
        for timer in &mut self.timers {
            if timer.next_run_at <= self.now_ms {
                ready.push((
                    timer.id,
                    timer.source.clone(),
                    timer.repeat,
                    timer.interval_ms,
                ));
                if timer.repeat {
                    timer.next_run_at = self.now_ms.saturating_add(timer.interval_ms);
                }
            }
        }

        self.timers
            .retain(|timer| timer.repeat || timer.next_run_at > self.now_ms);

        for (_, source, _, _) in ready {
            self.macrotasks.push_back(source);
        }
    }

    fn drain_macrotasks(&mut self) -> Vec<String> {
        self.macrotasks.drain(..).collect()
    }
}

/// Embedded JavaScript runtime backed by Boa.
#[derive(Debug)]
pub struct JsRuntime {
    context: Context,
    event_loop: Rc<RefCell<EventLoopState>>,
}

impl Default for JsRuntime {
    fn default() -> Self {
        Self::new().expect("default JS runtime should be constructible")
    }
}

impl JsRuntime {
    /// Creates a JavaScript runtime with global timer functions.
    pub fn new() -> JsResult<Self> {
        let event_loop = Rc::new(RefCell::new(EventLoopState::default()));
        let mut context = Context::default();

        context.register_global_builtin_callable(
            js_string!("setTimeout"),
            2,
            NativeFunction::from_copy_closure(set_timeout_native),
        )?;
        context.register_global_builtin_callable(
            js_string!("setInterval"),
            2,
            NativeFunction::from_copy_closure(set_interval_native),
        )?;
        context.register_global_builtin_callable(
            js_string!("clearTimeout"),
            1,
            NativeFunction::from_copy_closure(clear_timer_native),
        )?;
        context.register_global_builtin_callable(
            js_string!("clearInterval"),
            1,
            NativeFunction::from_copy_closure(clear_timer_native),
        )?;

        Ok(Self {
            context,
            event_loop,
        })
    }

    /// Evaluates JavaScript source code.
    pub fn eval(&mut self, source: &str) -> JsResult<JsValue> {
        self.with_active_event_loop(|context| context.eval(Source::from_bytes(source)))
    }

    /// Runs pending promise jobs.
    pub fn run_jobs(&mut self) -> JsResult<()> {
        self.with_active_event_loop(|context| context.run_jobs())
    }

    /// Schedules a timeout task from Rust.
    pub fn set_timeout(&mut self, source: impl Into<String>, delay_ms: u64) -> u64 {
        self.event_loop
            .borrow_mut()
            .schedule_timer(source.into(), delay_ms, false)
    }

    /// Schedules an interval task from Rust.
    pub fn set_interval(&mut self, source: impl Into<String>, interval_ms: u64) -> u64 {
        self.event_loop
            .borrow_mut()
            .schedule_timer(source.into(), interval_ms, true)
    }

    /// Clears a previously scheduled timer.
    pub fn clear_timer(&mut self, id: u64) {
        self.event_loop.borrow_mut().clear_timer(id);
    }

    /// Advances the event loop clock and runs due macrotasks and pending jobs.
    pub fn tick(&mut self, elapsed_ms: u64) -> JsResult<()> {
        self.event_loop.borrow_mut().advance(elapsed_ms);
        self.run_until_idle()
    }

    /// Runs queued macrotasks and pending promise jobs until idle.
    pub fn run_until_idle(&mut self) -> JsResult<()> {
        loop {
            let tasks = self.event_loop.borrow_mut().drain_macrotasks();
            if tasks.is_empty() {
                break;
            }

            for task in tasks {
                self.eval(&task)?;
                self.run_jobs()?;
            }
        }

        self.run_jobs()
    }

    fn with_active_event_loop<T>(
        &mut self,
        f: impl FnOnce(&mut Context) -> JsResult<T>,
    ) -> JsResult<T> {
        let event_loop = Rc::clone(&self.event_loop);
        ACTIVE_EVENT_LOOP.with(|slot| {
            let previous = slot.replace(Some(event_loop));
            let result = f(&mut self.context);
            slot.replace(previous);
            result
        })
    }
}

fn set_timeout_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    schedule_timer_from_js(args, context, false)
}

fn set_interval_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    schedule_timer_from_js(args, context, true)
}

fn clear_timer_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_u32(context)
        .unwrap_or(0) as u64;

    with_event_loop_state(|state| {
        state.borrow_mut().clear_timer(id);
        Ok(JsValue::undefined())
    })
}

fn schedule_timer_from_js(
    args: &[JsValue],
    context: &mut Context,
    repeat: bool,
) -> JsResult<JsValue> {
    let source = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let delay_ms = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_u32(context)
        .unwrap_or(0) as u64;

    with_event_loop_state(|state| {
        let id = state.borrow_mut().schedule_timer(source, delay_ms, repeat);
        Ok(JsValue::from(id as i32))
    })
}

fn with_event_loop_state<T>(
    f: impl FnOnce(&Rc<RefCell<EventLoopState>>) -> JsResult<T>,
) -> JsResult<T> {
    ACTIVE_EVENT_LOOP.with(|slot| {
        let state = slot.borrow().clone().ok_or_else(|| {
            JsError::from(JsNativeError::error().with_message("event loop is not active"))
        })?;
        f(&state)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_runtime_and_evaluates_scripts() {
        let mut runtime = JsRuntime::new().unwrap();
        let value = runtime.eval("1 + 2 + 3").unwrap();

        assert_eq!(value.as_number(), Some(6.0));
    }

    #[test]
    fn can_register_and_clear_timeout_from_rust() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval("globalThis.counter = 0;")
            .expect("script should evaluate");

        let id = runtime.set_timeout("globalThis.counter += 1;", 10);
        runtime.clear_timer(id);
        runtime.tick(10).unwrap();

        let value = runtime.eval("globalThis.counter").unwrap();
        assert_eq!(value.as_number(), Some(0.0));
    }

    #[test]
    fn runs_timeout_and_interval_tasks() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime.eval("globalThis.counter = 0;").unwrap();
        runtime.set_timeout("globalThis.counter += 1;", 0);
        runtime.set_interval("globalThis.counter += 2;", 5);

        runtime.tick(0).unwrap();
        assert_eq!(
            runtime.eval("globalThis.counter").unwrap().as_number(),
            Some(1.0)
        );

        runtime.tick(5).unwrap();
        assert_eq!(
            runtime.eval("globalThis.counter").unwrap().as_number(),
            Some(3.0)
        );
    }

    #[test]
    fn exposes_timer_functions_to_javascript() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime.eval("globalThis.counter = 0;").unwrap();
        runtime
            .eval(r#"setTimeout("globalThis.counter = 7", 0);"#)
            .unwrap();
        runtime.tick(0).unwrap();

        assert_eq!(
            runtime.eval("globalThis.counter").unwrap().as_number(),
            Some(7.0)
        );
    }

    #[test]
    fn runs_promise_jobs_via_job_queue() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime.eval("globalThis.value = 0;").unwrap();
        runtime
            .eval("Promise.resolve(21).then(v => { globalThis.value = v * 2; });")
            .unwrap();
        runtime.run_jobs().unwrap();

        let value = runtime.eval("globalThis.value").unwrap();
        assert_eq!(value.as_number(), Some(42.0));
    }
}
