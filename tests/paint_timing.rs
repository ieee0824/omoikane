//! Paint Timing and Largest Contentful Paint (Issue #1293).

use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;

fn runtime(body: &str) -> JsRuntime {
    let markup = format!("<!doctype html><html><body>{body}</body></html>");
    let mut runtime = JsRuntime::with_document_and_url(
        TreeBuilder::parse(&markup).document(),
        "https://example.com/",
    )
    .unwrap();
    runtime.set_viewport(400.0, 300.0);
    runtime
}

/// Evaluates `script`, which returns `true` or a failure description.
fn check(runtime: &mut JsRuntime, script: &str) {
    let result = runtime.eval(&format!("({script})()")).unwrap();
    assert!(result.as_boolean() == Some(true), "{}", result.display());
}

fn paint_names(runtime: &mut JsRuntime) -> String {
    runtime
        .eval("performance.getEntriesByType('paint').map(entry => entry.name).join()")
        .unwrap()
        .display()
        .to_string()
        .trim_matches('"')
        .to_string()
}

#[test]
fn background_is_a_first_paint_and_text_a_first_contentful_paint() {
    let mut runtime = runtime(
        "<div id=box style='width:50px;height:50px;background:red'></div>\
         <p id=text style='opacity:0'>hidden</p>",
    );
    assert_eq!(paint_names(&mut runtime), "");
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(paint_names(&mut runtime), "first-paint");
    check(
        &mut runtime,
        r#"() => {
          const [entry] = performance.getEntriesByType('paint');
          if (!(entry instanceof PerformancePaintTiming) || entry.duration !== 0) return 'entry';
          try { new PerformancePaintTiming(); return 'constructible'; } catch (e) {}
          document.getElementById('text').style.opacity = '1';
          return PerformanceObserver.supportedEntryTypes.includes('paint') || 'supported';
        }"#,
    );
    runtime.run_animation_frame(16).unwrap();
    assert_eq!(
        paint_names(&mut runtime),
        "first-paint,first-contentful-paint"
    );
}

#[test]
fn largest_contentful_paint_grows_until_input() {
    let mut runtime = runtime("<p id=small>small</p>");
    runtime.run_animation_frame(16).unwrap();
    check(
        &mut runtime,
        r#"() => {
          globalThis.lcp = [];
          new PerformanceObserver(list => lcp.push(...list.getEntries()))
            .observe({ type: 'largest-contentful-paint', buffered: true });
          if (performance.getEntriesByType('largest-contentful-paint').length !== 0)
            return 'LCP is not in the performance timeline';
          const big = document.createElement('h1');
          big.id = 'big';
          big.textContent = 'a much larger heading';
          document.body.append(big);
          return true;
        }"#,
    );
    runtime.run_animation_frame(16).unwrap();
    check(
        &mut runtime,
        r#"() => {
          const ids = lcp.map(entry => entry.id).join();
          if (ids !== 'small,big') return 'candidates ' + ids;
          const [first, second] = lcp;
          if (!(second instanceof LargestContentfulPaint)) return 'interface';
          if (!(second.size > first.size)) return 'sizes ' + first.size + ' ' + second.size;
          if (second.element !== document.getElementById('big') || second.url !== '') return 'element';
          if (second.startTime !== second.renderTime || second.loadTime !== 0) return 'times';
          if (second.toJSON().size !== second.size) return 'toJSON';
          __omoikane_dispatch_keyboard_input('keydown', { key: 'a' });
          const huge = document.createElement('div');
          huge.style.fontSize = '60px';
          huge.textContent = 'after input';
          document.body.append(huge);
          return true;
        }"#,
    );
    runtime.run_animation_frame(16).unwrap();
    check(
        &mut runtime,
        r#"() => lcp.length === 2 || 'input stops LCP: ' + lcp.map(entry => entry.id)"#,
    );
}
