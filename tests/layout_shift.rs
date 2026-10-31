//! Layout Instability (Issue #1293).

use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;

fn runtime(body: &str) -> JsRuntime {
    let markup = format!(
        "<!doctype html><html><head><style>body {{ margin: 0 }}</style></head><body>{body}</body></html>"
    );
    let mut runtime = JsRuntime::with_document_and_url(
        TreeBuilder::parse(&markup).document(),
        "https://example.com/",
    )
    .unwrap();
    runtime.set_viewport(800.0, 600.0);
    runtime
}

/// Evaluates `script`, which returns `true` or a failure description.
fn check(runtime: &mut JsRuntime, script: &str) {
    let result = runtime.eval(&format!("({script})()")).unwrap();
    assert!(result.as_boolean() == Some(true), "{}", result.display());
}

/// Renders a frame and delivers queued observer callbacks.
fn frame(runtime: &mut JsRuntime) {
    runtime.run_animation_frame(16).unwrap();
    runtime.tick(0).unwrap();
}

#[test]
fn moved_boxes_produce_layout_shift_entries() {
    let mut runtime = runtime(
        "<div id=shifter style='position:relative;width:300px;height:200px;background:blue'></div>",
    );
    check(
        &mut runtime,
        r#"() => {
          globalThis.shifts = [];
          new PerformanceObserver(list => shifts.push(...list.getEntries()))
            .observe({ type: 'layout-shift', buffered: true });
          return PerformanceObserver.supportedEntryTypes.includes('layout-shift') || 'supported';
        }"#,
    );
    frame(&mut runtime);
    runtime
        .eval("document.getElementById('shifter').style.top = '160px'")
        .unwrap();
    frame(&mut runtime);
    // A frame without layout changes reports nothing more.
    frame(&mut runtime);
    check(
        &mut runtime,
        r#"() => {
          if (shifts.length !== 1) return 'entries ' + shifts.length;
          const [entry] = shifts;
          if (!(entry instanceof LayoutShift) || entry.entryType !== 'layout-shift') return 'entry';
          const expected = (300 * 360) / (800 * 600) * (160 / 800);
          if (Math.abs(entry.value - expected) > 1e-6) return 'value ' + entry.value;
          if (entry.hadRecentInput || entry.duration !== 0) return 'flags';
          const [source] = entry.sources;
          if (entry.sources.length !== 1 || !(source instanceof LayoutShiftAttribution)) return 'sources';
          if (source.node !== document.getElementById('shifter')) return 'node';
          const text = r => [r.x, r.y, r.width, r.height].join();
          if (text(source.previousRect) !== '0,0,300,200' || text(source.currentRect) !== '0,160,300,200')
            return 'rects ' + text(source.previousRect) + ' ' + text(source.currentRect);
          if (JSON.stringify(entry.toJSON().sources[0].currentRect) === undefined) return 'toJSON';
          try { new LayoutShift(); return 'constructible'; } catch (e) {}
          return performance.getEntriesByType('layout-shift').length === 0 ||
            'layout shifts are not in the performance timeline';
        }"#,
    );
}

#[test]
fn fixed_transparent_and_recent_input_shifts() {
    let mut runtime = runtime(
        "<div id=fixed style='position:fixed;top:0;width:100px;height:100px;background:red'></div>\
         <div id=clear style='position:relative;opacity:0;width:100px;height:100px'></div>\
         <div id=moved style='position:relative;width:100px;height:100px;background:blue'></div>",
    );
    check(
        &mut runtime,
        r#"() => {
          globalThis.shifts = [];
          new PerformanceObserver(list => shifts.push(...list.getEntries()))
            .observe({ type: 'layout-shift' });
          return true;
        }"#,
    );
    frame(&mut runtime);
    runtime
        .eval(
            "document.getElementById('fixed').style.top = '200px'; \
             document.getElementById('clear').style.top = '200px';",
        )
        .unwrap();
    frame(&mut runtime);
    check(
        &mut runtime,
        "() => shifts.length === 0 || 'untracked boxes shifted'",
    );
    runtime
        .eval(
            "__omoikane_dispatch_keyboard_input('keydown', { key: 'a' }); \
             document.getElementById('moved').style.top = '50px';",
        )
        .unwrap();
    frame(&mut runtime);
    check(
        &mut runtime,
        r#"() => {
          if (shifts.length !== 1) return 'entries ' + shifts.length;
          const [entry] = shifts;
          return entry.hadRecentInput && entry.lastInputTime > 0 &&
            entry.sources[0].node === document.getElementById('moved') || 'recent input';
        }"#,
    );
}
