//! Native tree queries must not consume the JavaScript loop budget per ancestor.

use omoikane::{
    dom::{NodeHandle, ShadowRootMode},
    html::TreeBuilder,
    js::{JsRuntime, SandboxConfig},
};

fn deep_runtime() -> JsRuntime {
    let html = format!(
        "<html><body>{}<span id='leaf'>text</span>{}<aside id='sibling'></aside></body></html>",
        "<div>".repeat(256),
        "</div>".repeat(256),
    );
    JsRuntime::with_document_and_sandbox(
        TreeBuilder::parse(&html).document(),
        SandboxConfig {
            max_loop_iterations: 128,
            ..SandboxConfig::default()
        },
    )
    .unwrap()
}

#[test]
fn fallback_notification_includes_a_slot_that_was_not_previously_wrapped() {
    let document = TreeBuilder::parse("<html><body><div id='host'></div></body></html>").document();
    let host = document.query_selector("#host").unwrap();
    let shadow = host.attach_shadow(ShadowRootMode::Open).unwrap();
    let slot = NodeHandle::element("slot");
    let fallback = NodeHandle::element("span");
    fallback.append_child(NodeHandle::text("before"));
    slot.append_child(fallback);
    shadow.append_child(slot);
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime
        .eval(
            r#"const root = document.getElementById('host').shadowRoot;
               const fallback = root.querySelector('span');
               fallback.textContent = 'after';
               globalThis.fallbackChanges = 0;
               root.querySelector('slot').addEventListener('slotchange', () => fallbackChanges++);"#,
        )
        .unwrap();
    runtime.run_jobs().unwrap();
    assert_eq!(
        runtime.eval("fallbackChanges").unwrap().as_number(),
        Some(1.0)
    );
}

#[test]
fn node_identity_uses_captured_call_and_collection_methods() {
    let document = TreeBuilder::parse("<html><body><p id='target'></p></body></html>").document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
                    const target = document.getElementById('target');
                    const methods = [
                        [Function.prototype, 'call'], [Function.prototype, 'bind'],
                        [Map.prototype, 'get'], [WeakMap.prototype, 'get'],
                        [WeakRef.prototype, 'deref'], [Reflect, 'apply']
                    ];
                    const originals = methods.map(([object, key]) => object[key]);
                    const fail = () => { throw new Error('observable collection lookup'); };
                    try {
                        for (const [object, key] of methods) object[key] = fail;
                        return document.getElementById('target') === target &&
                            target.ownerDocument === document && target.isConnected;
                    } finally {
                        for (let i = 0; i < methods.length; i++) {
                            methods[i][0][methods[i][1]] = originals[i];
                        }
                    }
                })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn node_ids_stay_immutable_through_adoption_and_prototype_changes() {
    let document = TreeBuilder::parse("<html><body></body></html>").document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
                    const other = document.implementation.createHTMLDocument('other');
                    const nodes = [document.createElement('p'), document.createTextNode('text'),
                        document.createComment('comment'), document.createDocumentFragment(), document];
                    const same = Node.prototype.isSameNode;
                    const owner = Object.getOwnPropertyDescriptor(Node.prototype, 'ownerDocument').get;
                    for (const node of nodes) {
                        const id = node.__id;
                        const descriptor = Object.getOwnPropertyDescriptor(node, '__id');
                        if (typeof descriptor.get !== 'function' || descriptor.set !== undefined ||
                            'value' in descriptor || !descriptor.enumerable || descriptor.configurable ||
                            descriptor.get.call(other) !== other.__id ||
                            descriptor.get.call({}) !== undefined) return false;
                        if (Reflect.set(node, '__id', -1) || Reflect.deleteProperty(node, '__id') ||
                            Reflect.defineProperty(node, '__id', { value: -1 })) return false;
                        if (node !== document && node.nodeType !== 11) {
                            other.body.appendChild(node);
                            if (node.__id !== id || owner.call(node) !== other) return false;
                            other.body.removeChild(node);
                        }
                        const forged = Object.create(Node.prototype);
                        forged.__id = id;
                        try { same.call(forged, node); return false; }
                        catch (error) { if (error.name !== 'TypeError') return false; }
                        Object.setPrototypeOf(node, { __id: -1 });
                        if (node.__id !== id || !same.call(node, node)) return false;
                    }
                    return true;
                })()"#,
            )
            .expect("node identity must be immutable and separately branded")
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn deep_connectivity_does_not_spend_the_script_loop_budget() {
    let mut runtime = deep_runtime();
    assert_eq!(
        runtime
            .eval("document.getElementById('leaf').isConnected")
            .expect("connectivity must walk native ancestors")
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn unrelated_removal_preserves_deep_range_without_spending_the_loop_budget() {
    let mut runtime = deep_runtime();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
                    const text = document.getElementById('leaf').firstChild;
                    const range = document.createRange();
                    range.selectNodeContents(text);
                    document.body.removeChild(document.getElementById('sibling'));
                    return range.startContainer === text && range.startOffset === 0 &&
                        range.endContainer === text && range.endOffset === 4;
                })()"#,
            )
            .expect("range ancestry must walk native ancestors")
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn connectivity_reads_internal_tree_state_and_tracks_shadow_host_moves() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
                    const host = document.createElement('div');
                    const shadow = host.attachShadow({mode: 'closed'});
                    const child = document.createElement('span');
                    shadow.appendChild(child);
                    const connected = Object.getOwnPropertyDescriptor(Node.prototype, 'isConnected').get;
                    Object.defineProperty(child, 'parentNode', {
                        get() { throw new Error('observable ancestor lookup'); }
                    });
                    if (connected.call(child)) return false;
                    document.body.appendChild(host);
                    if (!connected.call(child) || !shadow.isConnected) return false;
                    document.body.removeChild(host);
                    if (connected.call(child) || shadow.isConnected) return false;
                    const template = document.createElement('template');
                    template.innerHTML = '<b>inert</b>';
                    document.body.appendChild(template);
                    const inert = document.implementation.createHTMLDocument('inert');
                    return !template.content.firstChild.isConnected && inert.body.isConnected &&
                        document.isConnected &&
                        !('__omoikane_node_is_connected' in globalThis) &&
                        !('__omoikane_node_is_inclusive_descendant' in globalThis) &&
                        !('__omoikane_node_has_slot_ancestor' in globalThis);
                })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn removal_adjusts_only_ranges_in_the_ordinary_removed_subtree() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
                    const parent = document.createElement('section');
                    parent.innerHTML = '<i></i><div>removed</div><b></b>';
                    document.body.appendChild(parent);
                    const removed = parent.childNodes[1];
                    const text = removed.firstChild;
                    const inside = document.createRange();
                    inside.setStart(text, 1);
                    inside.setEnd(text, 3);
                    const after = document.createRange();
                    after.setStart(parent, 2);
                    after.setEnd(parent, 3);
                    const shadow = removed.attachShadow({mode: 'closed'});
                    shadow.innerHTML = '<span>shadow</span>';
                    const shadowText = shadow.firstChild.firstChild;
                    const shadowRange = document.createRange();
                    shadowRange.setStart(shadowText, 1);
                    shadowRange.setEnd(shadowText, 3);
                    const documentRange = document.createRange();
                    parent.removeChild(removed);
                    return inside.startContainer === parent && inside.endContainer === parent &&
                        inside.startOffset === 1 && inside.endOffset === 1 &&
                        after.startContainer === parent && after.endContainer === parent &&
                        after.startOffset === 1 && after.endOffset === 2 &&
                        shadowRange.startContainer === shadowText && shadowRange.startOffset === 1 &&
                        shadowRange.endContainer === shadowText && shadowRange.endOffset === 3 &&
                        documentRange.startContainer === document && documentRange.endContainer === document &&
                        documentRange.startOffset === 0 && documentRange.endOffset === 0;
                })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn removing_document_child_adjusts_document_range_offsets() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
                    const doc = document.implementation.createDocument(null, 'root', null);
                    const range = doc.createRange();
                    range.setStart(doc, 1);
                    range.setEnd(doc, 1);
                    doc.removeChild(doc.documentElement);
                    return range.startContainer === doc && range.endContainer === doc &&
                        range.startOffset === 0 && range.endOffset === 0;
                })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn range_boundary_move_invalidates_document_only_removal_cache() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
                    const parent = document.createElement('section');
                    parent.innerHTML = '<i></i><b>target</b>';
                    document.body.appendChild(parent);
                    const range = document.createRange();
                    parent.removeChild(parent.firstChild); // primes the document-boundary cache
                    const removed = parent.firstChild;
                    range.setStart(removed.firstChild, 1);
                    range.setEnd(removed.firstChild, 4);
                    parent.removeChild(removed);
                    return range.startContainer === parent && range.endContainer === parent &&
                        range.startOffset === 0 && range.endOffset === 0;
                })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn insertion_cycle_checks_do_not_spend_the_js_loop_budget_per_ancestor() {
    let mut runtime = deep_runtime();
    let result = runtime
        .eval(
            r#"(() => {
                const leaf = document.getElementById('leaf');
                Object.defineProperty(leaf, 'parentNode', {
                    get() { throw new Error('observable ancestor lookup'); }
                });
                leaf.insertBefore(document.createTextNode('first'), null);
                leaf.appendChild(document.createTextNode('second'));
                try { leaf.insertBefore(leaf, null); return false; }
                catch (error) { if (error.name !== 'HierarchyRequestError') return false; }
                try { leaf.appendChild(leaf); return false; }
                catch (error) { if (error.name !== 'HierarchyRequestError') return false; }
                try { leaf.insertBefore(document.body, null); return false; }
                catch (error) { if (error.name !== 'HierarchyRequestError') return false; }
                try { leaf.appendChild(document.body); return false; }
                catch (error) { if (error.name !== 'HierarchyRequestError') return false; }
                return leaf.textContent === 'textfirstsecond' &&
                    document.body.parentNode === document.documentElement;
            })()"#,
        )
        .expect("native ancestry checks must handle a tree deeper than the JS loop budget");
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn insertion_includes_shadow_hosts_while_observer_ancestry_stops_at_roots() {
    let mut runtime = deep_runtime();
    let result = runtime
        .eval(
            r#"(() => {
                const host = document.createElement('div');
                const shadow = host.attachShadow({mode: 'open'});
                const observer = new MutationObserver(() => {});
                observer.observe(host, {childList: true, subtree: true});
                const inner = document.createElement('span');
                shadow.appendChild(inner);
                if (observer.takeRecords().length !== 0) return false;
                host.appendChild(document.createElement('p'));
                if (observer.takeRecords().length !== 1) return false;
                try { inner.insertBefore(host, null); return false; }
                catch (error) { if (error.name !== 'HierarchyRequestError') return false; }
                return host.parentNode === null && inner.parentNode === shadow;
            })()"#,
        )
        .expect("insertion and observer ancestry must keep their distinct shadow-root boundaries");
    assert_eq!(result.as_boolean(), Some(true));
}
