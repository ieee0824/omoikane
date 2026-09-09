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
                    parent.removeChild(removed);
                    return inside.startContainer === parent && inside.endContainer === parent &&
                        inside.startOffset === 1 && inside.endOffset === 1 &&
                        after.startContainer === parent && after.endContainer === parent &&
                        after.startOffset === 1 && after.endOffset === 2 &&
                        shadowRange.startContainer === shadowText && shadowRange.startOffset === 1 &&
                        shadowRange.endContainer === shadowText && shadowRange.endOffset === 3;
                })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}
