//! CSSOM insertion positions backed by the painted text layout.

use omoikane::{html::TreeBuilder, js::JsRuntime};

fn check(script: &str) {
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse(
        "<!doctype html><style>body{margin:0}p{margin:0;font:20px monospace;line-height:40px}</style><p id='text'>hello world</p>",
    ).document()).unwrap();
    assert_eq!(runtime.eval(script).unwrap().as_boolean(), Some(true));
}

#[test]
fn caret_positions_resolve_utf16_boundaries_and_current_geometry() {
    check(
        r#"(() => {
      const p = document.getElementById('text'), text = p.firstChild;
      const start = document.caretPositionFromPoint(0, 20);
      if (!(start instanceof CaretPosition) || start.offsetNode !== text || start.offset !== 0) return false;
      const end = document.caretPositionFromPoint(1000, 20);
      if (end.offsetNode !== text || end.offset !== text.length) return false;
      const rect = start.getClientRect();
      if (!(rect instanceof DOMRect) || rect.x !== 0 || rect.width !== 0 || rect.height <= 0 || rect.height >= 40) return false;
      const range = document.createRange();
      range.setStart(text, 0); range.collapse(true);
      const rangeRect = range.getBoundingClientRect();
      if (rect.x !== rangeRect.x || rect.y !== rangeRect.y || rect.height !== rangeRect.height) return false;
      p.style.marginLeft = '50px';
      if (start.getClientRect().x !== 50) return false;
      p.remove();
      const removed = start.getClientRect();
      return removed.x === 0 && removed.y === 0 && removed.width === 0 && removed.height === 0;
    })()"#,
    );
}

#[test]
fn caret_positions_validate_arguments_and_ignore_inactive_documents() {
    check(
        r#"(() => {
      for (const args of [[], [0], [NaN, 0], [0, Infinity], [0, 0, 2], [0, 0, {shadowRoots: [document]}]]) {
        try { document.caretPositionFromPoint(...args); return false; } catch (error) { if (!(error instanceof TypeError)) return false; }
      }
      for (const call of [() => new CaretPosition(), () => CaretPosition.prototype.getClientRect.call({}),
        () => Document.prototype.caretPositionFromPoint.call({}, 0, 0)]) {
        try { call(); return false; } catch (error) { if (!(error instanceof TypeError)) return false; }
      }
      const detached = document.implementation.createHTMLDocument('');
      detached.body.textContent = 'detached';
      return detached.caretPositionFromPoint(0, 0) === null &&
        document.caretPositionFromPoint(-1, 0) === null && document.caretPositionFromPoint(0, -1) === null;
    })()"#,
    );
}

#[test]
fn caret_positions_rescope_shadow_text_unless_explicitly_allowed() {
    check(
        r#"(() => {
      document.body.replaceChildren();
      const host = document.createElement('div');
      document.body.append(host);
      const root = host.attachShadow({mode:'closed'});
      root.innerHTML = '<p style="margin:0;font:20px monospace;line-height:40px">shadow text</p>';
      const outer = document.caretPositionFromPoint(0, 20);
      if (!outer || outer.offsetNode !== document.body || outer.offset !== 0) return false;
      let reads = 0;
      const roots = {get [Symbol.iterator]() { reads++; return function*() {yield root;}; }};
      const inner = document.caretPositionFromPoint(0, 20, {shadowRoots: roots});
      return reads === 1 && inner.offsetNode === root.firstChild.firstChild && inner.offset === 0;
    })()"#,
    );
}

#[test]
fn caret_positions_use_text_control_value_and_wrapped_lines() {
    check(
        r#"(() => {
      document.body.innerHTML = '<input style="display:block;font:20px monospace;border:0;padding:0;width:200px;height:40px" value="abc"><textarea style="font:20px/40px monospace;border:0;padding:0;width:200px;height:120px">abc\ndef</textarea>';
      const input = document.querySelector('input'), area = document.querySelector('textarea');
      const inputBox = input.getBoundingClientRect();
      const first = document.caretPositionFromPoint(inputBox.left, inputBox.top + 20);
      if (!first || first.offsetNode !== input || first.offset !== 0 || !first.getClientRect()) return false;
      const box = area.getBoundingClientRect();
      const second = document.caretPositionFromPoint(box.left, box.top + 60);
      if (!second || second.offsetNode !== area || second.offset !== 4) return false;
      const rect = second.getClientRect();
      return rect && rect.x === box.left && rect.y >= box.top + 40;
    })()"#,
    );
}

#[test]
fn caret_positions_use_following_visual_line_at_textarea_soft_wrap() {
    check(
        r#"(() => {
      document.body.innerHTML = '<textarea style="font:20px/40px monospace;border:0;padding:0;width:25px;height:160px">abcdef</textarea>';
      const area = document.querySelector('textarea'), box = area.getBoundingClientRect();
      const caret = document.caretPositionFromPoint(box.left + 1, box.top + 60);
      return caret && caret.offsetNode === area && caret.offset > 0 && caret.offset < area.value.length &&
        caret.getClientRect().x === box.left && caret.getClientRect().y >= box.top + 40;
    })()"#,
    );
}

#[test]
fn caret_position_is_not_exposed_in_dedicated_workers() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            r#"globalThis.caretWorkerValues = [];
      const worker = new Worker('data:text/javascript,postMessage(typeof%20CaretPosition)');
      worker.onmessage = event => caretWorkerValues.push(event.data);"#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("caretWorkerValues[0]")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "undefined"
    );
}

#[test]
fn caret_positions_preserve_graphemes_and_follow_transforms() {
    check(
        r#"(() => {
      const p = document.getElementById('text');
      p.textContent = 'A😀B';
      p.style.transform = 'translateX(20px)';
      const start = document.caretPositionFromPoint(20, 20);
      if (start.offset !== 0 || start.getClientRect().x !== 20) return false;
      const end = document.caretPositionFromPoint(1000, 20);
      if (end.offsetNode !== p.firstChild || end.offset !== 4) return false;
      for (let x = 20; x < 100; x += 2) {
        if (document.caretPositionFromPoint(x, 20).offset === 2) return false;
      }
      p.style.transform = 'translateX(60px)';
      return start.getClientRect().x === 60;
    })()"#,
    );
}

#[test]
fn range_geometry_covers_wrapped_text_and_selected_elements() {
    check(
        r#"(() => {
      const p = document.getElementById('text');
      p.style.width = '40px';
      p.textContent = 'ab cd ef';
      const range = document.createRange();
      range.selectNodeContents(p);
      const rects = range.getClientRects(), bounds = range.getBoundingClientRect();
      if (!(rects instanceof DOMRectList) || rects.length < 2 || bounds.height <= rects[0].height) return false;
      const snapshot = bounds.height;
      p.style.width = '400px';
      if (bounds.height !== snapshot || range.getBoundingClientRect().height >= snapshot) return false;
      range.selectNode(p);
      if (range.getBoundingClientRect().height < p.getBoundingClientRect().height) return false;
      for (const name of ['getClientRects', 'getBoundingClientRect']) {
        try { Range.prototype[name].call({}); return false; } catch (error) { if (!(error instanceof TypeError)) return false; }
      }
      return true;
    })()"#,
    );
}

#[test]
fn caret_positions_accept_same_origin_cross_realm_receivers() {
    check(
        r#"(() => {
      const frame = document.createElement('iframe'); document.body.append(frame);
      const doc = frame.contentDocument;
      doc.body.style.cssText = 'margin:0;font:20px/40px monospace';
      doc.body.textContent = 'child text';
      const caret = doc.caretPositionFromPoint(0, 20);
      const offsetNode = Object.getOwnPropertyDescriptor(CaretPosition.prototype, 'offsetNode').get;
      const offset = Object.getOwnPropertyDescriptor(CaretPosition.prototype, 'offset').get;
      const rect = CaretPosition.prototype.getClientRect.call(caret);
      return CaretPosition.length === 0 && offsetNode.call(caret) === doc.body.firstChild &&
        offset.call(caret) === 0 && rect.height > 0 && rect.x === caret.getClientRect().x;
    })()"#,
    );
}

#[test]
fn range_geometry_preserves_hidden_and_inert_layout_boxes() {
    check(
        r#"(() => {
      const p = document.getElementById('text');
      const caret = document.caretPositionFromPoint(0, 20);
      const range = document.createRange(); range.selectNodeContents(p);
      const width = range.getBoundingClientRect().width;
      p.style.visibility = 'hidden';
      if (width <= 0 || range.getBoundingClientRect().width !== width || caret.getClientRect().height <= 0 ||
          document.caretPositionFromPoint(0, 20) !== null) return false;
      p.style.visibility = 'visible'; p.inert = true;
      return range.getBoundingClientRect().width === width && document.caretPositionFromPoint(0, 20) === null;
    })()"#,
    );
}

#[test]
fn range_geometry_includes_length_changing_text_transforms() {
    check(
        r#"(() => {
      document.body.innerHTML = '<span style="font:20px/40px monospace;text-transform:uppercase">ß</span>';
      const span = document.body.firstChild, range = document.createRange();
      range.selectNodeContents(span);
      const transformed = range.getBoundingClientRect().width;
      span.textContent = 'SS'; range.selectNodeContents(span);
      return transformed > 0 && transformed === range.getBoundingClientRect().width;
    })()"#,
    );
}

#[test]
fn caret_svg_boundaries_respect_explicit_shadow_root_access() {
    check(
        r#"(() => {
      document.body.innerHTML = '<div></div>';
      const host = document.body.firstChild, root = host.attachShadow({mode:'closed'});
      root.innerHTML = '<svg width="100" height="100"><circle cx="50" cy="50" r="50" /></svg>';
      const inner = document.caretPositionFromPoint(50, 50, {shadowRoots:[root]});
      const outer = document.caretPositionFromPoint(50, 50);
      return inner && inner.offsetNode === root.querySelector('circle') && inner.offset === 0 &&
        outer && outer.offsetNode === document.body && outer.offset === 0;
    })()"#,
    );
}

#[test]
fn geometry_uses_captured_platform_methods_after_author_overrides() {
    check(
        r#"(() => {
      const p = document.getElementById('text'), range = document.createRange();
      range.selectNode(p);
      const expected = range.getBoundingClientRect().width;
      const original = Element.prototype.getClientRects, finite = Number.isFinite;
      try {
        Element.prototype.getClientRects = () => {throw new Error('author element method');};
        Number.isFinite = () => {throw new Error('author finite predicate');};
        range.__nodeRelation = () => {throw new Error('author range helper');};
        return range.getBoundingClientRect().width === expected && document.caretPositionFromPoint(0, 20).offset === 0;
      } finally { Element.prototype.getClientRects = original; Number.isFinite = finite; }
    })()"#,
    );
}
