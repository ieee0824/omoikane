//! requestIdleCallback and Prioritized Task Scheduling (Issue #1293).

use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;

fn runtime() -> JsRuntime {
    JsRuntime::with_document_and_url(
        TreeBuilder::parse("<!doctype html><body></body>").document(),
        "https://example.com/",
    )
    .unwrap()
}

/// Evaluates `script`, which returns `true` or a failure description.
fn check(runtime: &mut JsRuntime, script: &str) {
    let result = runtime.eval(&format!("({script})()")).unwrap();
    assert!(result.as_boolean() == Some(true), "{}", result.display());
}

fn run_ticks(runtime: &mut JsRuntime, count: usize) {
    for _ in 0..count {
        runtime.tick(0).unwrap();
    }
}

fn log(runtime: &mut JsRuntime) -> String {
    runtime
        .eval("globalThis.log.join()")
        .unwrap()
        .display()
        .to_string()
        .trim_matches('"')
        .to_string()
}

#[test]
fn idle_callbacks_run_after_tasks_in_their_own_turns() {
    let mut runtime = runtime();
    check(
        &mut runtime,
        r#"() => {
          globalThis.log = [];
          if (typeof requestIdleCallback !== 'function' || typeof IdleDeadline !== 'function')
            return 'interfaces';
          try { new IdleDeadline(); return 'IdleDeadline is not constructible'; } catch (e) {}
          try { requestIdleCallback(1); return 'callback type'; } catch (e) {
            if (!(e instanceof TypeError)) return 'TypeError';
          }
          const first = requestIdleCallback(deadline => {
            log.push('idle1:' + (deadline instanceof IdleDeadline) + ':' + deadline.didTimeout +
              ':' + (deadline.timeRemaining() > 0 && deadline.timeRemaining() <= 50));
            Promise.resolve().then(() => log.push('microtask1'));
            setTimeout(() => log.push('timeout-from-idle'), 0);
            requestIdleCallback(() => log.push('reposted'));
          });
          const cancelled = requestIdleCallback(() => log.push('cancelled'));
          requestIdleCallback(() => log.push('idle2'));
          cancelIdleCallback(cancelled);
          setTimeout(() => log.push('timeout'), 0);
          return typeof first === 'number' && first > 0 && cancelled === first + 1 ||
            'handles ' + first + ' ' + cancelled;
        }"#,
    );
    // The timer queued by the first callback is due, which ends the period.
    runtime.tick(0).unwrap();
    assert_eq!(
        log(&mut runtime),
        "timeout,idle1:true:false:true,microtask1"
    );
    // The next period runs the rest, then the callback posted meanwhile.
    runtime.tick(0).unwrap();
    assert_eq!(
        log(&mut runtime),
        "timeout,idle1:true:false:true,microtask1,timeout-from-idle,idle2,reposted"
    );
}

#[test]
fn idle_timeout_runs_the_callback_once_with_did_timeout() {
    let mut runtime = runtime();
    check(
        &mut runtime,
        r#"() => {
          globalThis.log = [];
          // The clock passes the timeout before any idle period starts.
          requestIdleCallback(deadline => log.push(
            'idle:' + deadline.didTimeout + ':' + deadline.timeRemaining()), { timeout: 5 });
          return true;
        }"#,
    );
    runtime.tick(10).unwrap();
    assert_eq!(log(&mut runtime), "idle:true:0");
    // The timed-out callback is removed from the idle list.
    runtime.tick(100).unwrap();
    assert_eq!(log(&mut runtime), "idle:true:0");
}

#[test]
fn idle_callback_exceptions_are_reported() {
    let mut runtime = runtime();
    check(
        &mut runtime,
        r#"() => {
          globalThis.log = [];
          addEventListener('error', event => log.push('error:' + event.error.message));
          requestIdleCallback(() => { throw new Error('boom'); });
          requestIdleCallback(() => log.push('next'));
          return true;
        }"#,
    );
    runtime.tick(0).unwrap();
    assert_eq!(log(&mut runtime), "error:boom,next");
}

#[test]
fn post_task_orders_by_priority_and_delay() {
    let mut runtime = runtime();
    check(
        &mut runtime,
        r#"() => {
          globalThis.log = [];
          if (!(scheduler instanceof Scheduler)) return 'scheduler';
          try { new Scheduler(); return 'Scheduler is not constructible'; } catch (e) {}
          const post = (name, options) =>
            scheduler.postTask(() => { log.push(name); return name; }, options);
          post('background', { priority: 'background' });
          post('visible');
          post('blocking', { priority: 'user-blocking' });
          post('delayed', { priority: 'user-blocking', delay: 5 });
          post('blocking2', { priority: 'user-blocking' }).then(value => log.push('resolved:' + value));
          scheduler.postTask(() => { throw new Error('task error'); })
            .catch(error => log.push('rejected:' + error.message));
          scheduler.postTask(() => {}, { priority: 'urgent' })
            .catch(error => log.push('invalid:' + error.name));
          scheduler.postTask(1).catch(error => log.push('callback:' + error.name));
          return true;
        }"#,
    );
    runtime.tick(0).unwrap();
    assert_eq!(
        log(&mut runtime),
        "invalid:TypeError,callback:TypeError,blocking,blocking2,resolved:blocking2,\
         visible,rejected:task error,background"
    );
    runtime.tick(5).unwrap();
    assert!(log(&mut runtime).ends_with(",background,delayed"));
}

#[test]
fn task_controller_aborts_and_reprioritizes_tasks() {
    let mut runtime = runtime();
    check(
        &mut runtime,
        r#"() => {
          globalThis.log = [];
          const controller = new TaskController({ priority: 'background' });
          const signal = controller.signal;
          if (!(controller instanceof AbortController) || !(signal instanceof TaskSignal) ||
              !(signal instanceof AbortSignal)) return 'interfaces';
          if (signal.priority !== 'background') return 'initial priority';
          try { new TaskSignal(); return 'TaskSignal is not constructible'; } catch (e) {}
          signal.addEventListener('prioritychange', event => {
            log.push(`change:${event.previousPriority}->${signal.priority}`);
            try { controller.setPriority('background'); } catch (e) { log.push('nested:' + e.name); }
          });
          scheduler.postTask(() => log.push('visible'));
          scheduler.postTask(() => log.push('signal1'), { signal });
          scheduler.postTask(() => log.push('fixed'), { signal, priority: 'background' });
          scheduler.postTask(() => log.push('signal2'), { signal });
          const aborter = new TaskController();
          scheduler.postTask(() => log.push('aborted'), { signal: aborter.signal })
            .catch(reason => log.push('abort:' + reason.name));
          aborter.abort();
          scheduler.postTask(() => log.push('never'), { signal: aborter.signal })
            .catch(reason => log.push('already:' + reason.name));
          controller.setPriority('user-blocking');
          controller.setPriority('user-blocking');
          const dependent = TaskSignal.any([], { priority: signal });
          if (dependent.priority !== 'user-blocking') return 'dependent priority';
          dependent.onprioritychange = event => log.push('dependent:' + event.previousPriority);
          controller.setPriority('user-visible');
          try { controller.setPriority('high'); return 'invalid priority'; } catch (e) {}
          try { new TaskPriorityChangeEvent('x', {}); return 'previousPriority required'; } catch (e) {}
          const event = new TaskPriorityChangeEvent('x', { previousPriority: 'background' });
          return event.previousPriority === 'background' || 'event';
        }"#,
    );
    runtime.tick(0).unwrap();
    assert_eq!(
        log(&mut runtime),
        "change:background->user-blocking,nested:NotAllowedError,\
         change:user-blocking->user-visible,nested:NotAllowedError,dependent:user-blocking,\
         abort:AbortError,already:AbortError,visible,signal1,signal2,fixed"
    );
}

#[test]
fn yield_continues_before_tasks_of_the_same_priority() {
    let mut runtime = runtime();
    check(
        &mut runtime,
        r#"() => {
          globalThis.log = [];
          scheduler.postTask(async () => {
            log.push('start');
            scheduler.postTask(() => log.push('blocking-task'), { priority: 'user-blocking' });
            scheduler.postTask(() => log.push('background-task'), { priority: 'background' });
            await scheduler.yield();
            log.push('after-yield-1');
            await scheduler.yield();
            log.push('after-yield-2');
          }, { priority: 'background' });
          const controller = new TaskController();
          scheduler.postTask(async () => {
            controller.abort();
            await scheduler.yield().catch(reason => log.push('yield-abort:' + reason.name));
          }, { signal: controller.signal });
          return true;
        }"#,
    );
    run_ticks(&mut runtime, 8);
    assert_eq!(
        log(&mut runtime),
        "yield-abort:AbortError,start,blocking-task,after-yield-1,after-yield-2,background-task"
    );
}

#[test]
fn workers_have_scheduler_but_no_idle_callbacks() {
    let mut runtime = runtime();
    runtime
        .eval("__omoikane_install_worker_global('https://example.com/worker.js', '1')")
        .unwrap();
    check(
        &mut runtime,
        r#"() => typeof requestIdleCallback === 'undefined' &&
          typeof cancelIdleCallback === 'undefined' && typeof IdleDeadline === 'undefined' &&
          scheduler instanceof Scheduler && typeof TaskController === 'function' ||
          'worker globals'"#,
    );
}

#[test]
fn deadlines_shrink_and_yield_inherits_idle_and_aborted_state() {
    let mut runtime = runtime();
    check(
        &mut runtime,
        r#"() => {
          globalThis.log = [];
          requestIdleCallback(async deadline => {
            setTimeout(() => {}, 0);
            log.push('remaining:' + deadline.timeRemaining());
            scheduler.postTask(() => log.push('visible'));
            scheduler.postTask(() => log.push('background'), { priority: 'background' });
            await scheduler.yield();
            log.push('idle-yield');
          });
          const controller = new TaskController();
          scheduler.postTask(() => controller.abort(), { signal: controller.signal })
            .catch(reason => log.push('aborted-while-running:' + reason.name));
          const source = new TaskController();
          const middle = TaskSignal.any([], { priority: source.signal });
          const leaf = TaskSignal.any([], { priority: middle });
          leaf.onprioritychange = () => log.push('leaf');
          middle.onprioritychange = () => log.push('middle');
          source.setPriority('background');
          return true;
        }"#,
    );
    run_ticks(&mut runtime, 8);
    assert_eq!(
        log(&mut runtime),
        "middle,leaf,aborted-while-running:AbortError,remaining:0,visible,idle-yield,background"
    );
}
