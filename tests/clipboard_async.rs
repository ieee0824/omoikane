//! Async Clipboard argument conversion and native representation ownership.

use omoikane::{html::TreeBuilder, js::JsRuntime};
use std::sync::Mutex;

static CLIPBOARD_TEST_LOCK: Mutex<()> = Mutex::new(());

fn runtime() -> JsRuntime {
    JsRuntime::with_document_and_url(
        TreeBuilder::parse("<!doctype html><body>").document(),
        "https://clipboard.example.test/",
    )
    .unwrap()
}

#[test]
fn clipboard_interface_is_readonly_same_object_and_an_event_target() {
    let mut runtime = runtime();
    assert_eq!(runtime.eval(r#"(() => {
      const clipboard = navigator.clipboard;
      const descriptor = Object.getOwnPropertyDescriptor(Navigator.prototype, 'clipboard');
      if (!(clipboard instanceof Clipboard) || !(clipboard instanceof EventTarget) ||
          navigator.hasOwnProperty('clipboard') || !descriptor.enumerable || descriptor.set !== undefined ||
          descriptor.get.name !== 'get clipboard' || descriptor.get.length !== 0 ||
          Clipboard.length !== 0 || Object.prototype.toString.call(clipboard) !== '[object Clipboard]') return false;
      navigator.clipboard = {};
      if (navigator.clipboard !== clipboard) return false;
      for (const create of [() => new Clipboard(), () => descriptor.get.call({})]) {
        try {create(); return false;} catch (error) {if (!(error instanceof TypeError)) return false;}
      }
      let called = false;
      clipboard.addEventListener('test', () => {called = true;});
      return clipboard.dispatchEvent(new Event('test')) && called;
    })()"#).unwrap().as_boolean(), Some(true));
}

#[test]
fn clipboard_write_text_converts_its_argument_before_returning() {
    let _lock = CLIPBOARD_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut runtime = runtime();
    runtime
        .eval(
            r#"
      globalThis.clipboardResult = 'pending';
      (async () => {
        let conversions = 0;
        const value = {toString() {conversions++; return 'initial あ😀';}};
        const writing = navigator.clipboard.writeText(value);
        const convertedAtCall = conversions === 1;
        value.toString = () => 'changed after return';
        await writing;
        const text = await navigator.clipboard.readText();
        clipboardResult = convertedAtCall && conversions === 1 && text === 'initial あ😀';
      })().catch(error => {clipboardResult = error.name + ': ' + error.message;})
        .finally(() => navigator.clipboard.writeText(''));
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime.eval("clipboardResult").unwrap().as_boolean(),
        Some(true)
    );
}

#[test]
fn clipboard_argument_and_receiver_errors_return_rejected_promises() {
    let _lock = CLIPBOARD_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut runtime = runtime();
    runtime.eval(r#"
      globalThis.clipboardResult = 'pending';
      (async () => {
        const clipboard = navigator.clipboard;
        for (const invoke of [
          () => clipboard.writeText(),
          () => clipboard.writeText(Symbol('not a DOMString')),
          () => Clipboard.prototype.readText.call({}),
          () => Clipboard.prototype.writeText.call({}, 'invalid receiver'),
        ]) {
          let promise;
          try {promise = invoke();} catch (error) {clipboardResult = 'synchronous ' + error.name; return;}
          if (!(promise instanceof Promise)) {clipboardResult = 'not a Promise'; return;}
          try {await promise; clipboardResult = 'unexpected fulfillment'; return;}
          catch (error) {if (!(error instanceof TypeError)) {clipboardResult = error.name; return;}}
        }
        clipboardResult = true;
      })().catch(error => {clipboardResult = error.name + ': ' + error.message;})
        .finally(() => navigator.clipboard.writeText(''));
    "#).unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime.eval("clipboardResult").unwrap().as_boolean(),
        Some(true)
    );
}

#[test]
fn clipboard_native_items_preserve_multiple_formats_and_binary_bytes() {
    let _lock = CLIPBOARD_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut runtime = runtime();
    runtime.eval(r#"
      globalThis.clipboardResult = 'pending';
      (async () => {
        const bytes = new Uint8Array([0, 128, 255, 13, 10]);
        await navigator.clipboard.write([new ClipboardItem({
          'text/plain': 'plain あ😀',
          'web application/example': new Blob([bytes], {type:'application/example'}),
        })]);
        bytes.fill(1);
        const items = await navigator.clipboard.read();
        if (items.length !== 1 || !(items[0] instanceof ClipboardItem) ||
            items[0].types.join('|') !== 'text/plain|web application/example') {
          clipboardResult = 'incorrect item metadata'; return;
        }
        const payload = await items[0].getType('web application/example');
        const actualBytes = Array.from(new Uint8Array(await payload.arrayBuffer())).join(',');
        const itemText = await (await items[0].getType('text/plain')).text();
        const clipboardText = await navigator.clipboard.readText();
        globalThis.clipboardDebug = JSON.stringify({actualBytes, itemText, clipboardText, payloadType:payload.type});
        clipboardResult = actualBytes === '0,128,255,13,10' && itemText === 'plain あ😀' &&
          clipboardText === 'plain あ😀';
      })().catch(error => {clipboardResult = error.name + ': ' + error.message;})
        .finally(() => navigator.clipboard.writeText(''));
    "#).unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime.eval("clipboardResult").unwrap().as_boolean(),
        Some(true),
        "{}",
        runtime
            .eval("clipboardDebug")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped()
    );
}
