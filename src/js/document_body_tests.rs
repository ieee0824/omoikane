use super::*;
use crate::html::TreeBuilder;

fn runtime() -> JsRuntime {
    JsRuntime::with_document(TreeBuilder::parse("<body></body>").document()).unwrap()
}

fn assert_contract(runtime: &mut JsRuntime, source: &str) {
    let result = runtime.eval(source).unwrap_or_else(|error| {
        let message = error
            .to_opaque(&mut runtime.context)
            .to_string(&mut runtime.context)
            .unwrap()
            .to_std_string_escaped();
        panic!("Document body contract failed: {message}");
    });
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn direct_children_framesets_and_nested_bodies() {
    assert_contract(
        &mut runtime(),
        r#"(() => {
    const ns = 'http://www.w3.org/1999/xhtml';
    const doc = document.implementation.createHTMLDocument('body order');
    const root = doc.documentElement, first = doc.body;
    const nested = doc.createElement('body');
    doc.head.appendChild(nested);
    const foreign = doc.createElementNS('urn:foreign', 'body');
    const noNamespace = doc.createElementNS(null, 'body');
    root.insertBefore(foreign, first); root.insertBefore(noNamespace, first);
    if (doc.body !== first) throw new Error('nested/foreign body selected');
    const frameset = doc.createElement('frameset'), second = doc.createElement('body');
    root.insertBefore(frameset, first); root.appendChild(second);
    if (doc.body !== frameset) throw new Error('frameset child order');
    root.removeChild(frameset);
    if (doc.body !== first) throw new Error('body after frameset removal');
    root.insertBefore(second, first);
    if (doc.body !== second) throw new Error('duplicate body reorder');
    root.removeChild(second); root.removeChild(first);
    return doc.body === null;
})()"#,
    );
}

#[test]
fn prefixed_xhtml_namespace_case_and_non_html_root() {
    assert_contract(
        &mut runtime(),
        r#"(() => {
    const html = 'http://www.w3.org/1999/xhtml';
    const cases = [
        ['<h:html xmlns:h="'+html+'"><h:body/></h:html>', 'body', 'h'],
        ['<h:html xmlns:h="'+html+'"><h:frameset/><h:body/></h:html>', 'frameset', 'h'],
        ['<html xmlns="'+html+'"><body/><frameset/></html>', 'body', null],
        ['<html xmlns="'+html+'"><head><body/></head></html>', null, null],
        ['<html xmlns="'+html+'"><BODY/></html>', null, null],
        ['<HTML xmlns="'+html+'"><body/></HTML>', null, null],
        ['<html><body/></html>', null, null],
        ['<svg xmlns="http://www.w3.org/2000/svg"><body xmlns="'+html+'"/></svg>', null, null]
    ];
    for (const [source,name,prefix] of cases) {
        const doc = new DOMParser().parseFromString(source, 'application/xhtml+xml');
        const body = doc.body;
        if (name === null ? body !== null : !body || body.localName !== name || body.prefix !== prefix)
            throw new Error(source + ': ' + (body && body.nodeName));
    }
    return new Document().body === null;
})()"#,
    );
}

#[test]
fn body_root_replacement_adoption_and_readonly_observers() {
    assert_contract(
        &mut runtime(),
        r#"(() => {
    const source = document.implementation.createHTMLDocument('source');
    const target = document.implementation.createHTMLDocument('target');
    const body = source.body, previous = target.body;
    body.textContent = 'adopted';
    const observer = new MutationObserver(() => {});
    observer.observe(target.documentElement, {childList:true});
    target.documentElement.replaceChild(body, previous);
    if (source.body !== null || target.body !== body || body.ownerDocument !== target ||
        body.textContent !== 'adopted' || previous.parentNode !== null) return false;
    const records = observer.takeRecords();
    if (records.length !== 1 || records[0].removedNodes[0] !== previous ||
        records[0].addedNodes[0] !== body) throw new Error('replacement mutation record');
    if (target.body !== body || target.body !== body || observer.takeRecords().length !== 0)
        throw new Error('getter queued mutation');
    const oldRoot = target.documentElement;
    const newRoot = target.createElement('html'), frameset = target.createElement('frameset');
    newRoot.appendChild(frameset);
    target.replaceChild(newRoot, oldRoot);
    if (target.body !== frameset || oldRoot.parentNode !== null) return false;
    target.documentElement.removeChild(frameset);
    source.documentElement.appendChild(source.adoptNode(body));
    return target.body === null && source.body === body && body.ownerDocument === source;
})()"#,
    );
}

#[test]
fn body_getter_native_state_author_overrides_and_receiver_brand() {
    assert_contract(
        &mut runtime(),
        r#"(() => {
    const doc = document.implementation.createHTMLDocument('private state');
    const root = doc.documentElement, body = doc.body;
    const getter = Object.getOwnPropertyDescriptor(Document.prototype, 'body').get;
    const poison = () => { throw new Error('author accessor must not run'); };
    for (const name of ['documentElement','childNodes','querySelector','getElementsByTagName'])
        Object.defineProperty(doc, name, {get:poison,configurable:true});
    for (const node of [root,body]) for (const name of ['childNodes','nodeType','localName','namespaceURI','prefix'])
        Object.defineProperty(node, name, {get:poison,configurable:true});
    if (getter.call(doc) !== body || getter.call(doc) !== body) return false;
    Object.defineProperty(doc, 'body', {get:poison,configurable:true});
    if (getter.call(doc) !== body) return false;
    const fake = Object.create(Document.prototype);
    Object.defineProperty(fake, '__id', {get:poison});
    for (const receiver of [null,undefined,{},fake,body]) {
        try { getter.call(receiver); return false; }
        catch (error) { if (error.name !== 'TypeError') throw error; }
    }
    return typeof globalThis.__omoikane_document_body === 'undefined';
})()"#,
    );
}

#[test]
fn body_getter_accepts_other_realm_document_and_preserves_wrapper_identity() {
    assert_contract(
        &mut runtime(),
        r#"(() => {
    const frame = document.createElement('iframe');
    document.body.appendChild(frame);
    try {
        const child = frame.contentWindow, doc = child.document, body = doc.body;
        const getter = Object.getOwnPropertyDescriptor(Document.prototype,'body').get;
        const childGetter = Object.getOwnPropertyDescriptor(child.Document.prototype,'body').get;
        const extra = doc.implementation.createHTMLDocument('other Realm');
        const extraBody = extra.body;
        return body !== null && getter.call(doc) === body && childGetter.call(document) === document.body &&
            getter.call(extra) === extraBody && extraBody.ownerDocument === extra &&
            typeof child.__omoikane_document_body === 'undefined';
    } finally { frame.remove(); }
})()"#,
    );
}

#[test]
fn body_getter_enrolls_native_replacement_and_its_descendants() {
    let mut runtime = runtime();
    let document = runtime.document();
    let root = document.query_selector("html").unwrap();
    let previous = document.document_body().unwrap();
    let replacement = NodeHandle::element("body");
    replacement.append_child(NodeHandle::text("native replacement"));
    root.remove_child(&previous).unwrap();
    root.append_child(replacement.clone());
    assert!(
        runtime
            .host_state
            .borrow()
            .get_node(replacement.identity())
            .is_none()
    );
    assert_contract(
        &mut runtime,
        r#"(() => {
        const body = document.body;
        return body === document.body && body.ownerDocument === document &&
            body.firstChild.data === 'native replacement' && body.firstChild.ownerDocument === document;
    })()"#,
    );
    assert!(
        runtime
            .host_state
            .borrow()
            .get_node(replacement.identity())
            .is_some()
    );
}
