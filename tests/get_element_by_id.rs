//! ID lookup uses the DOM tree directly without traversing JavaScript wrappers.

use omoikane::{
    html::TreeBuilder,
    js::{JsRuntime, SandboxConfig},
};

#[test]
fn large_tree_lookup_does_not_spend_the_script_loop_budget() {
    let html = format!(
        "<html><body>{}<span id='target'></span></body></html>",
        "<div><span></span></div>".repeat(512)
    );
    let document = TreeBuilder::parse(&html).document();
    let mut runtime = JsRuntime::with_document_and_sandbox(
        document,
        SandboxConfig {
            max_loop_iterations: 128,
            ..SandboxConfig::default()
        },
    )
    .unwrap();
    let result = runtime
        .eval("document.getElementById('target').tagName === 'SPAN'")
        .expect("a DOM lookup must not execute a JavaScript loop per descendant");
    assert_eq!(result.as_boolean(), Some(true));
    assert_eq!(
        runtime
            .eval("document.getElementById('absent') === null")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn lookup_does_not_invoke_overridden_dom_accessors() {
    let document =
        TreeBuilder::parse("<html><body><div><span id='target'></span></div></body></html>")
            .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let result = runtime
        .eval(
            r#"(() => {
                const target = document.getElementById('target');
                const fail = () => { throw new Error('observable DOM traversal'); };
                Object.defineProperty(document.body, 'childNodes', {get: fail});
                target.getAttribute = fail;
                return document.getElementById('target') === target &&
                    document.getElementById('absent') === null;
            })()"#,
        )
        .expect("ID lookup must inspect internal DOM data");
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn lookup_preserves_tree_order_after_mutation_and_respects_tree_boundaries() {
    let document = TreeBuilder::parse("<html><body></body></html>").document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let result = runtime
        .eval(
            r#"(() => {
                const a = document.createElement('div');
                const b = document.createElement('span');
                a.id = b.id = 'same';
                document.body.append(a, b);
                if (document.getElementById('same') !== a) return false;
                document.body.insertBefore(b, a);
                if (document.getElementById('same') !== b) return false;
                b.id = 'other';
                if (document.getElementById('same') !== a) return false;
                const fragment = document.createDocumentFragment();
                fragment.append(a);
                if (document.getElementById('same') !== null ||
                    fragment.getElementById('same') !== a) return false;
                const shadow = b.attachShadow({mode: 'open'});
                const hidden = document.createElement('i');
                hidden.id = 'same';
                shadow.append(hidden);
                if (document.getElementById('same') !== null ||
                    shadow.getElementById('same') !== hidden) return false;
                const template = document.createElement('template');
                template.innerHTML = '<i id="template-only"></i>';
                document.body.append(template);
                if (document.getElementById('template-only') !== null ||
                    template.content.getElementById('template-only') === null) return false;
                const detached = document.implementation.createDocument(null, 'root');
                const detachedChild = detached.createElement('leaf');
                detachedChild.id = 'other';
                detached.documentElement.appendChild(detachedChild);
                if (detached.getElementById('other') !== detachedChild ||
                    document.getElementById('other') !== b) return false;
                b.id = '';
                return document.getElementById('') === null;
            })()"#,
        )
        .unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn range_removal_uses_internal_sibling_indices() {
    let document = TreeBuilder::parse(
        "<html><body><p id='before'></p><p id='removed'></p><p id='after'></p></body></html>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let result = runtime
        .eval(
            r#"(() => {
                const removed = document.getElementById('removed');
                const range = document.createRange();
                range.setStart(document.body, 2);
                range.setEnd(document.body, 3);
                Object.defineProperty(document.body, 'childNodes', {
                    get() { throw new Error('observable sibling enumeration'); }
                });
                document.body.removeChild(removed);
                return range.startContainer === document.body && range.startOffset === 1 &&
                    range.endContainer === document.body && range.endOffset === 2;
            })()"#,
        )
        .expect("range updates must read internal sibling positions");
    assert_eq!(result.as_boolean(), Some(true));
}
