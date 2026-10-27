//! InputEvent target range snapshots and dispatch lifetime.

use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;

#[test]
fn constructed_input_event_owns_a_snapshot_and_clears_it_after_dispatch() {
    let document =
        TreeBuilder::parse("<!doctype html><body><p id='text'>hello</p></body>").document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
                  const text = document.getElementById('text').firstChild;
                  const range = new StaticRange({startContainer:text,startOffset:1,endContainer:text,endOffset:4});
                  const source = [range];
                  let iteratorReads = 0;
                  const sequence = { get [Symbol.iterator]() { iteratorReads++; return source[Symbol.iterator].bind(source); } };
                  const event = new InputEvent('beforeinput', {targetRanges:sequence});
                  source.length = 0;
                  const first = event.getTargetRanges();
                  if (iteratorReads !== 1 || first.length !== 1 || first[0] !== range || first === event.getTargetRanges()) return false;
                  first.length = 0;
                  let during = false;
                  document.body.addEventListener('beforeinput', e => {
                    const ranges = e.getTargetRanges();
                    during = ranges.length === 1 && ranges[0].startOffset === 1 && ranges[0].endOffset === 4;
                  }, {once:true});
                  document.body.dispatchEvent(event);
                  return during && event.getTargetRanges().length === 0 &&
                    Object.getOwnPropertyDescriptor(InputEvent.prototype, 'getTargetRanges').enumerable;
                })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn input_event_target_ranges_reject_invalid_values_and_receivers() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
                  for (const value of [null, 3, {}, [document.body]]) {
                    try { new InputEvent('x', {targetRanges:value}); return false; }
                    catch (error) { if (!(error instanceof TypeError)) return false; }
                  }
                  try { InputEvent.prototype.getTargetRanges.call({}); return false; }
                  catch (error) { return error instanceof TypeError && new InputEvent('x').getTargetRanges().length === 0; }
                })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn text_control_beforeinput_exposes_no_dom_target_ranges() {
    let document =
        TreeBuilder::parse("<!doctype html><body><textarea id='editor'>ab</textarea></body>")
            .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
                  const editor = document.getElementById('editor');
                  let count = -1, type = '';
                  editor.addEventListener('beforeinput', event => {
                    count = event.getTargetRanges().length;
                    type = event.inputType;
                  });
                  editor.focus();
                  __omoikane_dispatch_keyboard_input('keydown', {key:'x', text:'x'});
                  return count === 0 && type === 'insertText' && editor.value.includes('x');
                })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn input_event_accepts_static_ranges_from_another_same_origin_realm() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
                  const frame = document.createElement('iframe');
                  document.body.append(frame);
                  const child = frame.contentWindow;
                  const text = child.document.createTextNode('text');
                  child.document.body.append(text);
                  const range = new child.StaticRange({startContainer:text,startOffset:0,endContainer:text,endOffset:2});
                  const event = new InputEvent('beforeinput', {targetRanges:[range]});
                  if (event.getTargetRanges()[0] !== range) return false;
                  const childEvent = new child.InputEvent('beforeinput', {targetRanges:[range]});
                  document.body.dispatchEvent(childEvent);
                  return childEvent.getTargetRanges().length === 0;
                })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}
