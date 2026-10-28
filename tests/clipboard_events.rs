//! Clipboard event payload conversion and synthetic dispatch isolation.

use omoikane::{html::TreeBuilder, js::JsRuntime};

fn check(source: &str) {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(runtime.eval(source).unwrap().as_boolean(), Some(true));
}

#[test]
fn clipboard_events_preserve_the_supplied_data_transfer() {
    check(
        r#"(() => {
      const data = new DataTransfer(); data.setData('text/plain', 'copied');
      const event = new ClipboardEvent('copy', {clipboardData:data, bubbles:true, cancelable:true});
      if (!(event instanceof Event) || event.clipboardData !== data || !event.bubbles || !event.cancelable || event.composed || event.isTrusted) return false;
      event.clipboardData = null; event.__clipboardData = null;
      document.addEventListener('copy', e => {e.clipboardData.setData('text/plain', 'updated'); e.preventDefault();}, {once:true});
      return !document.dispatchEvent(event) && event.clipboardData === data && data.getData('text/plain') === 'updated' &&
        new ClipboardEvent('paste').clipboardData === null && Object.prototype.toString.call(event) === '[object ClipboardEvent]';
    })()"#,
    );
}

#[test]
fn clipboard_events_validate_dictionary_members_once_in_idl_order() {
    check(
        r#"(() => {
      const reads = [], data = new DataTransfer();
      const init = new Proxy({bubbles:true,cancelable:true,clipboardData:data,composed:true}, {
        get(target, name) { reads.push(name); return target[name]; }
      });
      const event = new ClipboardEvent('paste', init);
      if (reads.join(',') !== 'bubbles,cancelable,clipboardData,composed' || !event.composed) return false;
      for (const create of [() => new ClipboardEvent(), () => new ClipboardEvent(Symbol()),
        () => new ClipboardEvent('copy', 1), () => new ClipboardEvent('copy', {clipboardData:{}}),
        () => new ClipboardEvent('copy', {clipboardData:Object.create(DataTransfer.prototype)}),
        () => Object.getOwnPropertyDescriptor(ClipboardEvent.prototype,'clipboardData').get.call({})]) {
        try {create(); return false;} catch (error) {if (!(error instanceof TypeError)) return false;}
      }
      return new ClipboardEvent('copy', null).clipboardData === null;
    })()"#,
    );
}

#[test]
fn clipboard_events_accept_same_origin_cross_realm_data_and_getters() {
    check(
        r#"(() => {
      const frame = document.createElement('iframe'); document.body.append(frame);
      const child = frame.contentWindow, data = new child.DataTransfer();
      const event = new ClipboardEvent('paste', {clipboardData:data});
      const childEvent = new child.ClipboardEvent('copy', {clipboardData:new DataTransfer()});
      const getter = Object.getOwnPropertyDescriptor(ClipboardEvent.prototype, 'clipboardData').get;
      return event.clipboardData === data && getter.call(childEvent) === childEvent.clipboardData;
    })()"#,
    );
}

#[test]
fn clipboard_event_constructor_is_not_exposed_in_workers() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            r#"globalThis.clipboardWorkerValues = [];
      const worker = new Worker('data:text/javascript,postMessage(typeof%20ClipboardEvent)');
      worker.onmessage = event => clipboardWorkerValues.push(event.data);"#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("clipboardWorkerValues[0]")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "undefined"
    );
}

#[test]
fn synthetic_clipboard_events_do_not_edit_or_write_the_host_clipboard() {
    let document = TreeBuilder::parse("<!doctype html><input value='keep'>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "https://clipboard.example.test/").unwrap();
    runtime.eval(r#"
      globalThis.syntheticClipboardIsolation = false;
      navigator.clipboard.writeText('host clipboard').then(() => {
        const input = document.querySelector('input'), data = new DataTransfer();
        data.setData('text/plain', 'synthetic payload');
        input.select();
        input.addEventListener('copy', event => event.clipboardData.setData('text/plain', 'overridden'));
        for (const type of ['copy', 'cut', 'paste']) {
          input.dispatchEvent(new ClipboardEvent(type, {clipboardData:data,bubbles:true,cancelable:true}));
        }
        return navigator.clipboard.readText().then(text => {
          syntheticClipboardIsolation = text === 'host clipboard' && input.value === 'keep';
        });
      }).finally(() => navigator.clipboard.writeText(''));
    "#).unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("syntheticClipboardIsolation")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}
