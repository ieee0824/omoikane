//! ClipboardItem construction, representations, and exposure boundaries.

use omoikane::{html::TreeBuilder, js::JsRuntime};

fn runtime() -> JsRuntime {
    JsRuntime::with_document_and_url(
        TreeBuilder::parse("<!doctype html><body>").document(),
        "https://clipboard.example.test/",
    )
    .unwrap()
}

#[test]
fn clipboard_item_does_not_construct_private_maps_through_author_globals() {
    let mut runtime = runtime();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
      const OriginalMap = Map, OriginalString = String;
      let maps = 0, strings = 0, item;
      globalThis.Map = class extends OriginalMap {constructor(...args) {super(...args); maps++;}};
      globalThis.String = value => {strings++; return OriginalString(value);};
      try {item = new ClipboardItem({'text/plain':'value'});}
      finally {globalThis.Map = OriginalMap; globalThis.String = OriginalString;}
      return maps === 0 && strings === 0 && item.types[0] === 'text/plain';
    })()"#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn clipboard_item_types_are_canonical_frozen_snapshots() {
    let mut runtime = runtime();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
      const source = {' TEXT/PLAIN ; Charset="UTF-8" ': 'text', 'web application/abc': 'custom'};
      const item = new ClipboardItem(source, {presentationStyle:'attachment'});
      delete source['web application/abc'];
      const types = item.types;
      item.presentationStyle = 'inline'; item.__types = [];
      return types === item.types && Object.isFrozen(types) &&
        JSON.stringify(types) === '["text/plain;charset=UTF-8","web application/abc"]' &&
        item.presentationStyle === 'attachment' && ClipboardItem.length === 1 &&
        Object.prototype.toString.call(item) === '[object ClipboardItem]';
    })()"#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn clipboard_item_constructor_rejects_invalid_records_and_duplicate_mime_types() {
    let mut runtime = runtime();
    assert_eq!(runtime.eval(r#"(() => {
      for (const create of [() => new ClipboardItem(), () => new ClipboardItem(null),
        () => new ClipboardItem({}), () => new ClipboardItem(new Blob(['x'])),
        () => new ClipboardItem({'not a/real type':'x'}), () => new ClipboardItem({'web notatype':'x'}),
        () => new ClipboardItem({'TEXT/PLAIN':'x', 'text/plain':'y'}),
        () => new ClipboardItem({'text/plain':'x'}, {presentationStyle:'bad'}),
        () => new ClipboardItem({[Symbol()]:'x'}),
        () => Object.getOwnPropertyDescriptor(ClipboardItem.prototype,'types').get.call({})]) {
        try {create(); return false;} catch (error) {if (!(error instanceof TypeError)) return false;}
      }
      return new ClipboardItem({'application/abc':'x'}).types[0] === 'application/abc' &&
        new ClipboardItem({'text/plain':'x'}, null).presentationStyle === 'unspecified';
    })()"#).unwrap().as_boolean(), Some(true));
}

#[test]
fn clipboard_item_reads_record_entries_and_options_once() {
    let mut runtime = runtime();
    assert_eq!(runtime.eval(r#"(() => {
      const reads = [], target = {'text/plain':'x'};
      const source = new Proxy(target, {
        ownKeys(object) {reads.push('keys'); return Reflect.ownKeys(object);},
        getOwnPropertyDescriptor(object, key) {reads.push('descriptor:' + key); return Object.getOwnPropertyDescriptor(object,key);},
        get(object,key) {reads.push('get:' + key); return object[key];}
      });
      const item = new ClipboardItem(source, {get presentationStyle() {reads.push('style'); return 'inline';}});
      return reads.join(',') === 'keys,descriptor:text/plain,get:text/plain,style' && item.presentationStyle === 'inline';
    })()"#).unwrap().as_boolean(), Some(true));
}

#[test]
fn clipboard_item_supports_distinguishes_writable_formats_from_constructor_types() {
    let mut runtime = runtime();
    assert_eq!(runtime.eval(r#"(() => {
      for (const type of ['text/plain','text/html','image/png','text/uri-list','image/svg+xml','web foo/bar']) {
        if (!ClipboardItem.supports(type)) return false;
      }
      for (const type of ['foo/bar','text/plain;foo=1','web foo/bar;foo=1','web','web ','not a/real type','']) {
        if (ClipboardItem.supports(type)) return false;
      }
      for (const invoke of [() => ClipboardItem.supports(), () => ClipboardItem.supports(Symbol())]) {
        try {invoke(); return false;} catch(error) {if (!(error instanceof TypeError)) return false;}
      }
      return new ClipboardItem({'foo/bar':'x'}).types[0] === 'foo/bar';
    })()"#).unwrap().as_boolean(), Some(true));
}

#[test]
fn clipboard_item_get_type_awaits_data_and_preserves_blob_identity() {
    let mut runtime = runtime();
    runtime.eval(r#"globalThis.clipboardItemResult = false;
      (async () => {
        const blob = new Blob(['different type'], {type:'application/abc'});
        const item = new ClipboardItem({'text/plain':Promise.resolve('あ😀'), 'text/html':Promise.resolve(blob)});
        const text = await item.getType('text/PLAIN');
        clipboardItemResult = text instanceof Blob && text.type === 'text/plain' &&
          await text.text() === 'あ😀' && await item.getType('text/html') === blob;
      })();"#).unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime.eval("clipboardItemResult").unwrap().as_boolean(),
        Some(true)
    );
}

#[test]
fn clipboard_item_missing_and_invalid_types_reject_promises() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"globalThis.clipboardItemResult = false;
      (async () => {
        const item = new ClipboardItem({'text/plain':'x'}), errors = [];
        for (const args of [[], ['not a/real type'], ['text/html']]) {
          const promise = item.getType(...args);
          if (!(promise instanceof Promise)) return;
          try {await promise; return;} catch (error) {errors.push(error.name);}
        }
        clipboardItemResult = errors.join(',') === 'TypeError,TypeError,NotFoundError';
      })();"#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime.eval("clipboardItemResult").unwrap().as_boolean(),
        Some(true)
    );
}

#[test]
fn clipboard_item_getters_accept_same_origin_cross_realm_instances() {
    let mut runtime = runtime();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
      const frame = document.createElement('iframe'); document.body.append(frame);
      const item = new frame.contentWindow.ClipboardItem({'text/plain':'x'});
      const getter = Object.getOwnPropertyDescriptor(ClipboardItem.prototype,'types').get;
      return getter.call(item)[0] === 'text/plain';
    })()"#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn clipboard_item_rejected_data_is_reported_without_unhandled_internal_promises() {
    let mut runtime = runtime();
    runtime.eval(r#"globalThis.clipboardItemFailure = null; globalThis.clipboardItemUnhandled = 0;
      addEventListener('unhandledrejection', event => {clipboardItemUnhandled++; event.preventDefault();});
      globalThis.unreadClipboardItem = new ClipboardItem({'text/plain':Promise.reject(new Error('unread data'))});
      const rejectedItem = new ClipboardItem({'text/plain':Promise.reject(new Error('rejected data'))});
      rejectedItem.getType('text/plain').catch(error => {clipboardItemFailure = error.name;});"#).unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("clipboardItemFailure === 'NotFoundError' && clipboardItemUnhandled === 0")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn clipboard_item_does_not_expose_internal_records_to_author_thenables() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"globalThis.clipboardItemInternalRecords = 0; globalThis.clipboardItemResult = false;
      const originalThen = Object.getOwnPropertyDescriptor(Object.prototype, 'then');
      Object.defineProperty(Object.prototype, 'then', {configurable:true, value:function(resolve) {
        if (!(this instanceof Blob)) clipboardItemInternalRecords++;
        delete Object.prototype.then;
        resolve(this);
      }});
      new ClipboardItem({'text/plain':'x'}).getType('text/plain').then(blob => {
        clipboardItemResult = blob.type === 'text/plain' && clipboardItemInternalRecords === 0;
      }).finally(() => {
        delete Object.prototype.then;
        if (originalThen) Object.defineProperty(Object.prototype,'then',originalThen);
      });"#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime.eval("clipboardItemResult").unwrap().as_boolean(),
        Some(true)
    );
}

#[test]
fn clipboard_item_is_only_exposed_in_secure_windows() {
    let mut insecure = JsRuntime::with_document_and_url(
        TreeBuilder::parse("<!doctype html><body>").document(),
        "http://clipboard.example.test/",
    )
    .unwrap();
    assert_eq!(
        insecure
            .eval("typeof ClipboardItem")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "undefined"
    );
    let mut secure = runtime();
    secure
        .eval(
            r#"globalThis.clipboardItemWorker = null;
      const worker = new Worker('data:text/javascript,postMessage(typeof%20ClipboardItem)');
      worker.onmessage = event => clipboardItemWorker = event.data;"#,
        )
        .unwrap();
    secure.run_until_idle().unwrap();
    assert_eq!(
        secure
            .eval("clipboardItemWorker")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "undefined"
    );
}
