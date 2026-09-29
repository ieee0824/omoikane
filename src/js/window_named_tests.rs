use super::*;

#[test]
fn window_named_properties_follow_connected_dom_mutations() {
    let mut runtime = JsRuntime::new().unwrap();
    let result = runtime
        .eval(
            r#"(() => {
                const host = document.createElement('section');
                host.innerHTML = '<div id="outer"></div>';
                document.body.appendChild(host);
                const inserted = outer === host.firstElementChild && window.outer === outer;
                host.innerHTML = '<div id="renamed"></div>';
                const replaced = typeof outer === 'undefined' && window.renamed === host.firstElementChild;
                host.firstElementChild.id = 'newName';
                const renamed = typeof window.renamed === 'undefined' && window.newName === host.firstElementChild;
                const second = document.createElement('div');
                second.id = 'newName';
                host.appendChild(second);
                const collection = window.newName;
                const duplicate = collection.length === 2 && collection[0] === host.firstElementChild && collection[1] === second;
                second.remove();
                const liveCollection = collection.length === 1 && window.newName === host.firstElementChild;
                const builtin = document.createElement('div');
                builtin.id = 'document';
                host.appendChild(builtin);
                const priority = window.document !== builtin && window.document === document;
                window.newName = 42;
                host.remove();
                const override = window.newName === 42 && typeof window.outer === 'undefined';
                return [inserted, replaced, renamed, duplicate, liveCollection, priority, override].join('|');
            })()"#,
        )
        .unwrap()
        .to_string(&mut runtime.context)
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(result, "true|true|true|true|true|true|true");
}

#[test]
fn window_named_properties_follow_nested_fragment_insertions_and_removals() {
    let mut runtime = JsRuntime::new().unwrap();
    let result = runtime
        .eval(
            r#"(() => {
                const fragment = document.createDocumentFragment();
                const outer = document.createElement('section');
                const inner = document.createElement('div');
                inner.id = 'fragmentDescendant';
                outer.appendChild(inner);
                fragment.appendChild(outer);
                const detached = !('fragmentDescendant' in window);
                document.body.appendChild(fragment);
                const inserted = window.fragmentDescendant === inner;
                outer.remove();
                return detached && inserted && !('fragmentDescendant' in window);
            })()"#,
        )
        .unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn window_named_properties_include_selected_name_attributes_and_same_origin_frames() {
    let mut runtime = JsRuntime::new().unwrap();
    let result = runtime
        .eval(
            r#"(() => {
                const form = document.createElement('form');
                form.name = 'namedForm';
                document.body.appendChild(form);
                const input = document.createElement('input');
                input.name = 'notWindowNamed';
                document.body.appendChild(input);
                const frame = document.createElement('iframe');
                frame.name = 'childFrame';
                frame.srcdoc = '<div id="insideChild"></div>';
                document.body.appendChild(frame);
                return [
                    window.namedForm === form,
                    !('notWindowNamed' in window),
                    window.childFrame === frame.contentWindow,
                    window.childFrame.insideChild?.__id === frame.contentDocument.getElementById('insideChild').__id,
                    'insideChild' in window.childFrame,
                    Object.getOwnPropertyDescriptor(window.childFrame, 'insideChild')?.enumerable === true
                ].join('|');
            })()"#,
        )
        .unwrap()
        .to_string(&mut runtime.context)
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(result, "true|true|true|true|true|true");
}

#[test]
fn cross_origin_named_frame_does_not_expose_its_window() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime.set_base_url("https://example.test/index.html".parse().unwrap());
    runtime
        .eval(
            r#"globalThis.crossFrame = document.createElement('iframe');
               crossFrame.name = 'otherOrigin';
               crossFrame.src = 'data:text/html,<div id="secret"></div>';
               crossFrame.id = 'crossOriginElement';
               document.body.appendChild(crossFrame);
               globalThis.laterFrame = document.createElement('iframe');
               laterFrame.name = 'otherOrigin';
               laterFrame.srcdoc = '<p>same origin</p>';
               document.body.appendChild(laterFrame);"#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval(
                "crossFrame.contentDocument === null && window.otherOrigin === undefined && !('otherOrigin' in window) && window.crossOriginElement === crossFrame"
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn named_frame_visibility_tracks_navigation_across_origins() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime.set_base_url("https://example.test/index.html".parse().unwrap());
    assert_eq!(
        runtime
            .eval(
                r#"globalThis.movingFrame = document.createElement('iframe');
                   movingFrame.name = 'movingFrameName';
                   document.body.appendChild(movingFrame);
                   window.movingFrameName === movingFrame.contentWindow"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    runtime
        .eval("movingFrame.src = 'data:text/html,<p>cross origin</p>'")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("movingFrame.contentDocument === null")
            .unwrap()
            .as_boolean(),
        Some(true),
        "the navigation must have a cross-origin document"
    );
    assert_eq!(
        runtime
            .eval("!('movingFrameName' in window)")
            .unwrap()
            .as_boolean(),
        Some(true),
        "the named property must be removed after navigation"
    );
}

#[test]
fn window_named_refresh_does_not_call_mutable_string_helpers() {
    let mut runtime = JsRuntime::new().unwrap();
    let result = runtime
        .eval(
            r#"(() => {
                const original = String.prototype.toLowerCase;
                const node = document.createElement('div');
                node.id = 'safeNamed';
                try {
                    String.prototype.toLowerCase = () => { throw new Error('poisoned'); };
                    document.body.appendChild(node);
                    return window.safeNamed === node;
                } finally {
                    String.prototype.toLowerCase = original;
                    node.remove();
                }
            })()"#,
        )
        .unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn window_named_refresh_ignores_author_owner_document_override() {
    let mut runtime = JsRuntime::new().unwrap();
    let result = runtime
        .eval(
            r#"(() => {
                const node = document.createElement('div');
                node.id = 'safeNamed';
                Object.defineProperty(node, 'ownerDocument', {
                    get() { throw new Error('author accessor must not affect named access'); }
                });
                document.body.appendChild(node);
                const found = window.safeNamed === node;
                node.remove();
                return found && !('safeNamed' in window);
            })()"#,
        )
        .unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn window_named_refresh_ignores_author_child_nodes_override() {
    let mut runtime = JsRuntime::new().unwrap();
    let result = runtime
        .eval(
            r#"(() => {
                const node = document.createElement('div');
                node.id = 'safeNamed';
                Object.defineProperty(document.body, 'childNodes', {
                    get() { throw new Error('author accessor must not affect named access'); }
                });
                document.body.appendChild(node);
                const found = window.safeNamed === node;
                node.remove();
                return found && !('safeNamed' in window);
            })()"#,
        )
        .unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}
