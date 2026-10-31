//! Long Tasks and Event Timing (Issue #1293).

use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;

fn runtime() -> JsRuntime {
    JsRuntime::with_document_and_url(
        TreeBuilder::parse("<!doctype html><body><button id=b>b</button></body>").document(),
        "https://example.com/",
    )
    .unwrap()
}

/// Evaluates `script`, which returns `true` or a failure description.
fn check(runtime: &mut JsRuntime, script: &str) {
    let result = runtime.eval(&format!("({script})()")).unwrap();
    assert!(result.as_boolean() == Some(true), "{}", result.display());
}

#[test]
fn tasks_over_fifty_milliseconds_are_long_tasks() {
    let mut runtime = runtime();
    check(
        &mut runtime,
        r#"() => {
          globalThis.longtasks = [];
          new PerformanceObserver(list => longtasks.push(...list.getEntries()))
            .observe({ type: 'longtask' });
          setTimeout(() => { const end = performance.now() + 60; while (performance.now() < end); }, 0);
          setTimeout(() => {}, 0);
          return true;
        }"#,
    );
    runtime.tick(0).unwrap();
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        r#"() => {
          if (longtasks.length !== 1) return 'entries ' + longtasks.length;
          const [entry] = longtasks;
          if (!(entry instanceof PerformanceLongTaskTiming) || entry.name !== 'self') return 'entry';
          if (!(entry.duration > 50 && Number.isInteger(entry.duration) && Number.isInteger(entry.startTime)))
            return 'duration ' + entry.duration;
          const [attribution] = entry.attribution;
          if (!(attribution instanceof TaskAttributionTiming) ||
              attribution.containerType !== 'window' || attribution.name !== 'unknown')
            return 'attribution';
          return performance.getEntriesByType('longtask').length === 0 ||
            'long tasks are not in the performance timeline';
        }"#,
    );
}

#[test]
fn trusted_input_produces_event_timing_entries() {
    let mut runtime = runtime();
    check(
        &mut runtime,
        r#"() => {
          globalThis.events = [];
          globalThis.trusted = [];
          new PerformanceObserver(list => events.push(...list.getEntries()))
            .observe({ type: 'event', durationThreshold: 16 });
          const button = document.getElementById('b');
          button.addEventListener('mousedown', event => {
            trusted.push(event.isTrusted);
            globalThis.lastEvent = event;
            const end = performance.now() + 40; while (performance.now() < end);
          });
          button.dispatchEvent(new MouseEvent('mousedown'));
          __omoikane_dispatch_mouse_input(button.__id, 'mousedown', { button: 0 }, false);
          __omoikane_dispatch_keyboard_input('keydown', { key: 'a' });
          __omoikane_dispatch_keyboard_input('keyup', { key: 'a' });
          return true;
        }"#,
    );
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        r#"() => {
          if (trusted.join() !== 'false,true') return 'isTrusted ' + trusted;
          if (Object.getOwnPropertyNames(lastEvent).some(name => /timing/i.test(name)))
            return 'measurement state leaked onto the event';
          const slow = events.filter(entry => entry.name === 'mousedown');
          if (slow.length !== 1) return 'observed slow events ' + events.map(entry => entry.name);
          const [entry] = slow;
          if (!(entry instanceof PerformanceEventTiming)) return 'interface';
          if (!(entry.duration >= 40 && entry.duration % 8 === 0)) return 'duration ' + entry.duration;
          if (!(entry.processingStart >= entry.startTime && entry.processingEnd >= entry.processingStart))
            return 'processing times';
          if (entry.target !== document.getElementById('b') || !entry.cancelable) return 'target';
          const [first] = performance.getEntriesByType('first-input');
          if (!first || first.name !== 'mousedown') return 'first input';
          if (performance.eventCounts.get('mousedown') !== 1 || performance.eventCounts.get('keydown') !== 1)
            return 'counts ' + [...performance.eventCounts].filter(([, n]) => n).join(';');
          if (performance.interactionCount !== 1) return 'interactions ' + performance.interactionCount;
          return true;
        }"#,
    );
}
