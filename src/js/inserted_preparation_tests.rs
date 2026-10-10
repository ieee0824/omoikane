//! Behavioral contracts for leaf wiring and empty pending-script collections.

use super::*;

fn runtime() -> JsRuntime {
    let document =
        crate::html::TreeBuilder::parse("<html><head></head><body></body></html>").document();
    JsRuntime::with_document(document).unwrap()
}

fn check(runtime: &mut JsRuntime, source: &str) {
    let value = runtime.eval(source).unwrap_or_else(|error| {
        let message = error
            .to_opaque(&mut runtime.context)
            .to_string(&mut runtime.context)
            .map(|value| value.to_std_string_escaped())
            .unwrap_or_else(|_| format!("{error:?}"));
        panic!("inserted preparation contract: {message}");
    });
    assert_eq!(value.as_boolean(), Some(true));
}

#[test]
fn inserted_handler_wiring_keeps_fragment_shadow_and_native_leaf_contracts() {
    let mut runtime = runtime();
    check(
        &mut runtime,
        r#"(() => {
        const poison = () => { throw new Error('author tree getter'); };
        const poisonTreeAccessors = nodes => {
            const saved = [];
            for (const node of nodes) {
                for (const name of ['nodeType', 'childNodes']) {
                    saved.push([node, name, Object.getOwnPropertyDescriptor(node, name)]);
                    Object.defineProperty(node, name, {get:poison, configurable:true});
                }
            }
            return () => {
                for (const [node, name, descriptor] of saved) {
                    if (descriptor) Object.defineProperty(node, name, descriptor);
                    else delete node[name];
                }
            };
        };
        const range = document.createRange();
        range.selectNodeContents(document.body);
        const fragment = range.createContextualFragment(
            '<button onclick="globalThis.fragmentClicks=(globalThis.fragmentClicks||0)+1">text</button><!--leaf-->');
        const button = fragment.firstChild, text = button.firstChild;
        const restoreFragment = poisonTreeAccessors([button, text, fragment.lastChild]);
        document.body.appendChild(fragment);
        restoreFragment();
        button.dispatchEvent(new Event('click'));
        if (globalThis.fragmentClicks !== 1) return false;
        let idlClicks = 0;
        button.onclick = () => { idlClicks++; };
        button.remove();
        const restoreButton = poisonTreeAccessors([button, text]);
        document.body.appendChild(button);
        restoreButton();
        button.dispatchEvent(new Event('click'));
        if (globalThis.fragmentClicks !== 1 || idlClicks !== 1) return false;
        const host = document.createElement('div');
        const shadow = host.attachShadow({mode:'open'});
        shadow.innerHTML = '<button onclick="globalThis.shadowClicks=(globalThis.shadowClicks||0)+1">shadow</button>';
        const shadowButton = shadow.firstChild;
        const restoreShadow = poisonTreeAccessors([shadowButton]);
        document.body.appendChild(host);
        restoreShadow();
        shadowButton.dispatchEvent(new Event('click'));
        const xml = document.implementation.createDocument(null, 'root', null);
        const cdata = xml.createCDATASection('a😀\uD800x');
        const pi = xml.createProcessingInstruction('target', 'value');
        for (const leaf of [cdata, pi]) {
            Object.defineProperty(leaf, 'nodeType', {get:poison});
            Object.defineProperty(leaf, 'childNodes', {get:poison});
            xml.documentElement.appendChild(leaf);
        }
        return globalThis.shadowClicks === 1 && xml.documentElement.firstChild === cdata &&
            xml.documentElement.lastChild === pi && text.parentNode === button;
        })()"#,
    );
}

#[test]
fn pending_scripts_cross_empty_collections_and_keep_order_once_and_csp() {
    let mut runtime = runtime();
    check(
        &mut runtime,
        r#"(() => {
        const addOrdinary = () => {
            const element = document.createElement('a');
            element.appendChild(document.createTextNode('ordinary'));
            document.body.appendChild(element); element.remove();
        };
        addOrdinary();
        const pending = document.createElement('script');
        pending.textContent = 'globalThis.pendingRuns=(globalThis.pendingRuns||0)+1';
        addOrdinary();
        if (globalThis.pendingRuns !== undefined) return false;
        document.body.appendChild(pending);
        if (globalThis.pendingRuns !== 1) return false;
        pending.remove(); document.body.appendChild(pending);
        addOrdinary();
        const range = document.createRange(); range.selectNodeContents(document.body);
        globalThis.orderedRuns = [];
        document.body.appendChild(range.createContextualFragment(
            '<div><script>orderedRuns.push(1)</script><script>orderedRuns.push(2)</script></div>'));
        return globalThis.pendingRuns === 1 && orderedRuns.join(',') === '1,2';
        })()"#,
    );
    let mut blocked = self::runtime();
    blocked.install_csp_policy(&["script-src 'none'".to_owned()]);
    check(
        &mut blocked,
        r#"(() => {
        const script = document.createElement('script');
        script.textContent = 'globalThis.blockedPreparationRan=true';
        document.body.appendChild(script);
        const leaf = document.createTextNode('after blocked script');
        document.body.appendChild(leaf);
        return globalThis.blockedPreparationRan === undefined && leaf.parentNode === document.body;
        })()"#,
    );
    assert_eq!(blocked.host_state.borrow().csp_violations.len(), 1);
}

#[test]
fn pending_script_collection_preserves_child_realm_and_sandbox_boundaries() {
    let mut runtime = runtime();
    check(
        &mut runtime,
        r#"(() => {
        globalThis.allowedFrame = document.createElement('iframe');
        globalThis.blockedFrame = document.createElement('iframe');
        blockedFrame.setAttribute('sandbox', 'allow-same-origin');
        document.body.append(allowedFrame, blockedFrame);
        return true;
        })()"#,
    );
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        r#"(() => {
        for (const frame of [allowedFrame, blockedFrame]) {
            const doc = frame.contentDocument;
            doc.body.appendChild(doc.createTextNode('empty collection'));
            const script = doc.createElement('script');
            script.textContent = 'globalThis.preparationChildRuns=(globalThis.preparationChildRuns||0)+1';
            doc.body.appendChild(script);
            script.remove(); doc.body.appendChild(script);
        }
        return allowedFrame.contentWindow.preparationChildRuns === 1 &&
            blockedFrame.contentWindow.preparationChildRuns === undefined &&
            globalThis.preparationChildRuns === undefined;
        })()"#,
    );
}
