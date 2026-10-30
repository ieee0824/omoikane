//! CSS Custom Highlight API: `Highlight`, `CSS.highlights` and
//! `::highlight()` (Issue #1297).

use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;

fn runtime(body: &str) -> JsRuntime {
    let markup = format!("<!doctype html><html><body>{body}</body></html>");
    JsRuntime::with_document_and_url(
        TreeBuilder::parse(&markup).document(),
        "https://example.com/",
    )
    .unwrap()
}

/// Evaluates `script`, which returns `true` or a failure description.
fn check(runtime: &mut JsRuntime, script: &str) {
    let result = runtime
        .eval(&format!(
            "(() => {{ try {{ return ({script})(); }} catch (error) {{ return String(error) + error.stack; }} }})()"
        ))
        .unwrap();
    if result.as_boolean() != Some(true) {
        panic!("{}", result.display());
    }
}

#[test]
fn highlight_is_a_setlike_of_ranges_with_priority_and_type() {
    let mut runtime = runtime("<p id=p>abc</p>");
    check(
        &mut runtime,
        r#"() => {
          const text = document.getElementById('p').firstChild;
          const live = new Range();
          const fixed = new StaticRange({ startContainer: text, startOffset: 0, endContainer: text, endOffset: 1 });
          // Tampering with Set.prototype must not affect the setlike members.
          Set.prototype.add = null;
          const highlight = new Highlight(live, live, fixed);
          if (highlight.size !== 2 || !highlight.has(fixed)) return 'constructor ' + highlight.size;
          if (highlight.add(live) !== highlight || highlight.size !== 2) return 'add';
          if (!highlight.delete(live) || highlight.delete(live)) return 'delete';
          if ([...highlight][0] !== fixed || [...highlight.entries()][0][1] !== fixed) return 'iteration';
          highlight.priority = 3.7;
          highlight.type = 'spelling-error';
          highlight.type = 'bogus';
          if (highlight.priority !== 3 || highlight.type !== 'spelling-error') return 'attributes';
          try { highlight.add({}); return 'accepted a non-range'; } catch (error) {
            if (!(error instanceof TypeError)) return 'wrong error';
          }
          highlight.clear();
          return highlight.size === 0 && Highlight.length === 0 &&
            Object.prototype.toString.call(highlight) === '[object Highlight]' || 'clear';
        }"#,
    );
}

#[test]
fn highlight_registry_is_a_maplike_exposed_as_css_highlights() {
    let mut runtime = runtime("");
    check(
        &mut runtime,
        r#"() => {
          const registry = CSS.highlights;
          if (!(registry instanceof HighlightRegistry) || registry !== CSS.highlights) return 'identity';
          try { new HighlightRegistry(); return 'constructible'; } catch (_) {}
          const a = new Highlight(), b = new Highlight();
          if (registry.set('a', a) !== registry) return 'set return';
          registry.set('b', b);
          try { registry.set('c', {}); return 'accepted a non-highlight'; } catch (_) {}
          if (registry.size !== 2 || registry.get('a') !== a || !registry.has('b')) return 'lookup';
          if ([...registry.keys()].join() !== 'a,b') return 'order ' + [...registry.keys()];
          const seen = [];
          registry.forEach((value, key, map) => seen.push(key + (value === a) + (map === registry)));
          if (seen.join() !== 'atruetrue,bfalsetrue') return 'forEach ' + seen;
          if (!registry.delete('a') || registry.delete('a')) return 'delete';
          registry.clear();
          return registry.size === 0 || 'clear';
        }"#,
    );
}

#[test]
fn highlight_pseudo_elements_parse_and_compute() {
    let mut runtime = runtime(
        r#"<style>#t::highlight(foo){background-color:green;color:lime} #t::highlight(bar){color:fuchsia}</style><div id=t></div>"#,
    );
    check(
        &mut runtime,
        r#"() => {
          const target = document.getElementById('t');
          const foo = getComputedStyle(target, '::highlight(foo)');
          if (foo.backgroundColor !== 'rgb(0, 128, 0)' || foo.color !== 'rgb(0, 255, 0)') {
            return 'foo ' + foo.backgroundColor + ' ' + foo.color;
          }
          if (getComputedStyle(target, '::highlight(bar)').color !== 'rgb(255, 0, 255)') return 'bar';
          for (const invalid of ['::highlight(foo))', '::highlight(foo,bar)', '::highlight', ':::highlight(foo)']) {
            if (getComputedStyle(target, invalid).length !== 0) return 'invalid computed ' + invalid;
          }
          const sheet = new CSSStyleSheet();
          const valid = ['::highlight(foo)', 'div ::highlight(foo)', '::part(p)::highlight(foo)', '::highlight(\\*)'];
          for (const selector of valid) {
            if (!CSS.supports(`selector(${selector})`)) return 'unsupported ' + selector;
          }
          const invalid = ['::before::highlight(foo)', '::highlight(foo).a', '::highlight(foo) div',
            '::highlight(foo)::after', '::highlight(foo):hover', ':not(::highlight(foo))'];
          for (const selector of invalid) {
            if (CSS.supports(`selector(${selector})`)) return 'accepted ' + selector;
          }
          for (const selector of ['::highlight', ...invalid]) {
            sheet.replaceSync(`${selector} { color: red }`);
            if (sheet.cssRules.length !== 0) return 'rule accepted ' + selector;
          }
          sheet.replaceSync('::highlight(\\31\\32\\33) { color: red }');
          return sheet.cssRules[0].selectorText === '::highlight(\\31 23)' ||
            'serialized ' + sheet.cssRules[0].selectorText;
        }"#,
    );
}

fn pixel(runtime: &JsRuntime, x: u32, y: u32) -> omoikane::paint::Color {
    let canvas = omoikane::paint::render_document(
        &runtime.document(),
        omoikane::layout::Rect {
            width: 300.0,
            height: 100.0,
            ..Default::default()
        },
    )
    .unwrap();
    canvas.pixel(x, y).unwrap()
}

#[test]
fn registered_highlights_paint_their_pseudo_styles_by_priority() {
    let mut runtime = runtime(
        r#"<style>body{margin:0;background:white} p{margin:0;font:20px/30px monospace}
          ::highlight(low){background-color:rgb(0,0,255)} ::highlight(high){background-color:rgb(0,128,0)}</style><p id=p>MMMMMMMMMM</p>"#,
    );
    assert_eq!(
        pixel(&runtime, 5, 5),
        omoikane::paint::Color::rgb(255, 255, 255)
    );
    check(
        &mut runtime,
        r#"() => {
          const text = document.getElementById('p').firstChild;
          const range = (start, end) => {
            const value = new Range();
            value.setStart(text, start);
            value.setEnd(text, end);
            return value;
          };
          const low = new Highlight(range(0, 10));
          const high = new Highlight(range(0, 2));
          globalThis.low = low;
          // Registered later but with lower priority.
          CSS.highlights.set('high', high);
          CSS.highlights.set('low', low);
          high.priority = 1;
          return true;
        }"#,
    );
    runtime.run_until_idle().unwrap();
    // The higher-priority highlight paints over the first two characters.
    assert_eq!(
        pixel(&runtime, 2, 3),
        omoikane::paint::Color::rgb(0, 128, 0)
    );
    // The remaining characters show the low highlight's background.
    let width = runtime
        .eval("document.getElementById('p').firstChild && (() => { const r = new Range(); r.selectNodeContents(document.getElementById('p')); return r.getBoundingClientRect().width; })()")
        .unwrap()
        .as_number()
        .unwrap() as u32;
    assert_eq!(
        pixel(&runtime, width - 2, 3),
        omoikane::paint::Color::rgb(0, 0, 255)
    );
    assert_eq!(
        pixel(&runtime, width + 5, 3),
        omoikane::paint::Color::rgb(255, 255, 255)
    );
    // Live ranges follow DOM edits: deleting text shrinks the low highlight.
    runtime
        .eval("CSS.highlights.delete('high'); document.getElementById('p').firstChild.appendData('MMMMM');")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        pixel(&runtime, 2, 3),
        omoikane::paint::Color::rgb(0, 0, 255)
    );
    assert_eq!(
        pixel(&runtime, width + 5, 3),
        omoikane::paint::Color::rgb(255, 255, 255)
    );
    runtime
        .eval("document.getElementById('p').firstChild.deleteData(0, 9)")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        pixel(&runtime, 2, 3),
        omoikane::paint::Color::rgb(0, 0, 255)
    );
    assert_eq!(
        pixel(&runtime, width / 10 + 3, 3),
        omoikane::paint::Color::rgb(255, 255, 255)
    );
    runtime.eval("low.clear()").unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        pixel(&runtime, 2, 3),
        omoikane::paint::Color::rgb(255, 255, 255)
    );
}

#[test]
fn highlight_color_repaints_only_the_covered_glyphs() {
    let mut runtime = runtime(
        r#"<style>body{margin:0;background:white} p{margin:0;font:40px/50px monospace;color:black}
          ::highlight(red){color:rgb(255,0,0)}</style><p id=p>MMMM</p>"#,
    );
    runtime
        .eval(
            "const t = document.getElementById('p').firstChild; const r = new Range(); \
             r.setStart(t, 0); r.setEnd(t, 2); CSS.highlights.set('red', new Highlight(r));",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    let canvas = omoikane::paint::render_document(
        &runtime.document(),
        omoikane::layout::Rect {
            width: 300.0,
            height: 60.0,
            ..Default::default()
        },
    )
    .unwrap();
    let width = runtime
        .eval("(() => { const r = new Range(); r.selectNodeContents(document.getElementById('p')); return r.getBoundingClientRect().width; })()")
        .unwrap()
        .as_number()
        .unwrap() as u32;
    let count = |from: u32, to: u32, red: bool| {
        (0..50)
            .flat_map(|y| (from..to).map(move |x| (x, y)))
            .filter(|(x, y)| {
                let pixel = canvas.pixel(*x, *y).unwrap();
                if red {
                    pixel.r > 200 && pixel.g < 60 && pixel.b < 60
                } else {
                    pixel.r < 60 && pixel.g < 60 && pixel.b < 60
                }
            })
            .count()
    };
    let half = width / 2;
    assert!(count(0, half, true) > 50, "highlighted glyphs must be red");
    assert_eq!(
        count(half, width, true),
        0,
        "unhighlighted glyphs stay black"
    );
    assert!(
        count(half, width, false) > 50,
        "unhighlighted glyphs are painted"
    );
}

#[test]
fn window_and_highlights_survive_tampered_collection_prototypes() {
    for (collection, member) in [
        ("Set", "has"),
        ("Set", "add"),
        ("Set", "delete"),
        ("Set", "forEach"),
        ("Set", "[Symbol.iterator]"),
        ("Map", "has"),
        ("Map", "get"),
        ("Map", "set"),
        ("Map", "forEach"),
        ("Map", "[Symbol.iterator]"),
    ] {
        let mut runtime = runtime("<p id=p>text</p>");
        let target = if member.starts_with('[') {
            format!("{collection}.prototype{member}")
        } else {
            format!("{collection}.prototype.{member}")
        };
        let result = runtime
            .eval(&format!(
                r#"(() => {{
                    {target} = 1;
                    try {{
                        window.addEventListener('error', () => {{}});
                        const range = new Range();
                        range.selectNodeContents(document.getElementById('p'));
                        const highlight = new Highlight(range);
                        highlight.add(range);
                        CSS.highlights.set('h', highlight);
                        return [highlight.size, highlight.has(range), CSS.highlights.size,
                                CSS.highlights.get('h') === highlight].join();
                    }} catch (e) {{ return String(e); }}
                }})()"#
            ))
            .unwrap();
        assert_eq!(
            result.display().to_string(),
            "\"1,true,1,true\"",
            "{target} tampering"
        );
    }
}
