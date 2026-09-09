use super::*;
use crate::html::TreeBuilder;

fn runtime() -> JsRuntime {
    JsRuntime::with_document(
        TreeBuilder::parse("<iframe id=f srcdoc='<p id=old data-value=kept>old</p>'></iframe>")
            .document(),
    )
    .unwrap()
}

fn collect(runtime: &mut JsRuntime) {
    runtime.context.clear_kept_objects();
    boa_gc::force_collect();
    runtime.host_state.borrow_mut().sweep_node_lifetimes();
}

#[test]
fn retained_iframe_document_and_nodes_survive_navigation_and_removal() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"
        var frame = document.getElementById('f');
        var oldDoc = frame.contentDocument;
        var oldNode = oldDoc.getElementById('old');
        oldNode.marker = { value: 42 };
        frame.srcdoc = '<p id=new>new</p>';
        var newDoc = frame.contentDocument;
    "#,
        )
        .unwrap();
    collect(&mut runtime);
    assert_eq!(
        runtime
            .eval(
                r#"
        oldDoc.nodeType === 9 && oldDoc.nodeName === '#document' &&
        oldNode.nodeName === 'P' && oldNode.textContent === 'old' &&
        oldNode.getAttribute('data-value') === 'kept' && oldNode.ownerDocument === oldDoc &&
        oldNode.parentNode === oldDoc.body && oldNode.isConnected &&
        oldDoc.getElementById('old') === oldNode && oldNode.marker.value === 42 && oldDoc !== newDoc
    "#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    runtime
        .eval(
            r#"
        var added = oldDoc.createElement('span'); added.textContent = 'written';
        oldNode.appendChild(added); oldNode.setAttribute('data-value', 'changed');
        frame.remove();
    "#,
        )
        .unwrap();
    collect(&mut runtime);
    assert_eq!(
        runtime
            .eval(
                r#"
        oldNode.textContent === 'oldwritten' && oldNode.getAttribute('data-value') === 'changed' &&
        added.ownerDocument === oldDoc && added.parentNode === oldNode &&
        newDoc.nodeType === 9 && newDoc.body.textContent === 'new' && newDoc.defaultView === null
    "#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn retained_old_node_keeps_document_ancestors_and_expandos_alive() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"
        var frame = document.getElementById('f');
        var held = frame.contentDocument.getElementById('old');
        held.ownerDocument.body.marker = 123;
        frame.srcdoc = '<p>new</p>'; void frame.contentDocument;
    "#,
        )
        .unwrap();
    collect(&mut runtime);
    assert_eq!(runtime.eval("held.ownerDocument.body === held.parentNode && held.parentNode.marker === 123 && held.ownerDocument.getElementById('old') === held").unwrap().as_boolean(), Some(true));
}

#[test]
fn old_document_generations_are_kept_by_references_without_an_eviction_limit() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"
        var frame = document.getElementById('f'); var held = [];
        for (var i = 0; i < 80; i++) {
            frame.srcdoc = '<p>' + i + '</p>';
            held.push(frame.contentDocument);
        }
    "#,
        )
        .unwrap();
    collect(&mut runtime);
    assert_eq!(
        runtime
            .eval("held.every((doc, i) => doc.body.textContent === String(i))")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    runtime.eval("held = null").unwrap();
    collect(&mut runtime);
    let state = runtime.host_state.borrow();
    assert!(
        state.node_lifetimes.document_count() <= 2,
        "only main and current iframe documents remain"
    );
    assert!(
        state.node_lifetimes.node_count() < 30,
        "retired nodes must be reclaimed"
    );
    assert!(state.document_styles.len() <= 2);
}

#[test]
fn inert_and_cloned_document_style_caches_follow_javascript_lifetime() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"
        var keep = document.implementation.createHTMLDocument('kept');
        keep.body.innerHTML = '<style>p {color:rgb(1, 2, 3)}</style><p>kept</p>';
        getComputedStyle(keep.querySelector('p')).color;
        for (var i = 0; i < 80; i++) {
            let detached = keep.cloneNode(true);
            getComputedStyle(detached.querySelector('p')).color;
            detached.open(); detached.write('<p>stream</p>'); detached.close();
        }
    "#,
        )
        .unwrap();
    collect(&mut runtime);
    assert_eq!(runtime.eval("keep.body.textContent.includes('kept') && getComputedStyle(keep.querySelector('p')).color === 'rgb(1, 2, 3)'").unwrap().as_boolean(), Some(true));
    {
        let state = runtime.host_state.borrow();
        assert!(
            state.node_lifetimes.document_count() <= 2,
            "remaining groups: {}",
            state.node_lifetimes.document_count()
        );
        assert!(state.document_styles.len() <= 2);
        assert!(state.write_parsers.len() <= 1);
    }
    runtime.eval("keep = null").unwrap();
    collect(&mut runtime);
    assert_eq!(runtime.host_state.borrow().document_styles.len(), 1);
}

#[test]
fn adopting_retained_old_node_releases_its_previous_document() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"
        var frame = document.getElementById('f');
        var held = frame.contentDocument.getElementById('old'); held.marker = 42;
        frame.srcdoc = '<p>new</p>'; void frame.contentDocument;
        document.body.appendChild(held);
    "#,
        )
        .unwrap();
    collect(&mut runtime);
    assert_eq!(runtime.eval("held.ownerDocument === document && held.marker === 42 && document.getElementById('old') === held").unwrap().as_boolean(), Some(true));
    assert!(runtime.host_state.borrow().node_lifetimes.document_count() <= 2);
}

#[test]
fn mutating_retired_document_does_not_restart_resources_or_layout() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"
        var frame = document.getElementById('f'); var oldDoc = frame.contentDocument;
        frame.remove();
        var nested = oldDoc.createElement('iframe'); nested.srcdoc = '<p>inactive</p>';
        oldDoc.body.appendChild(nested);
        var script = oldDoc.createElement('script'); script.src = 'http://127.0.0.1:9/inactive.js';
        oldDoc.body.appendChild(script);
    "#,
        )
        .unwrap();
    assert_eq!(runtime.eval("nested.contentDocument === null && oldDoc.documentElement.clientWidth === 0 && oldDoc.getElementById('old').textContent === 'old'").unwrap().as_boolean(), Some(true));
    let state = runtime.host_state.borrow();
    assert!(state.iframe_documents.is_empty());
    assert!(state.pending_resource_loads.is_empty());
}

#[test]
fn template_owner_documents_and_uninserted_contents_follow_their_creator() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"
        var template = document.createElement('template');
        template.innerHTML = '<b>kept</b>';
        var templateOwnerId = template.content.ownerDocument.__id;
        for (var i = 0; i < 80; i++) {
            let doc = document.implementation.createHTMLDocument('temporary');
            doc.createElement('template');
        }
    "#,
        )
        .unwrap();
    collect(&mut runtime);
    assert_eq!(runtime.eval("template.content.ownerDocument.__id === templateOwnerId && template.content.firstChild.textContent === 'kept'").unwrap().as_boolean(), Some(true));
    let state = runtime.host_state.borrow();
    assert!(
        state.node_lifetimes.document_count() <= 2,
        "remaining groups: {}",
        state.node_lifetimes.document_count()
    );
    assert!(state.node_lifetimes.node_count() < 30);
    assert!(
        state.nodes.len() < 30,
        "unwrapped native template contents must not leak"
    );
}

#[test]
fn nested_iframe_realms_and_detached_nodes_have_bounded_lifetimes() {
    let mut runtime = runtime();
    runtime.eval(r#"
        var frame = document.getElementById('f');
        for (var i = 0; i < 12; i++) {
            frame.srcdoc = '<body data-generation=' + i + '><iframe></iframe><p>old</p>';
            frame.contentWindow.eval("document.body.marker = 42; document.querySelector('iframe').contentWindow.eval('document.body.marker = 43'); let node = document.createElement('p'); node.textContent = 'detached';");
        }
        frame.srcdoc = '<p>new</p>'; void frame.contentDocument;
    "#).unwrap();
    runtime.run_jobs().unwrap();
    collect(&mut runtime);
    let state = runtime.host_state.borrow();
    assert_eq!(state.iframe_documents.len(), 1);
    assert!(
        state.node_lifetimes.document_count() <= 2,
        "remaining groups: {}",
        state.node_lifetimes.document_count()
    );
    assert!(state.node_lifetimes.node_count() < 40);
    assert!(state.nodes.len() < 40);
    assert!(state.document_styles.len() <= 2);
}

#[test]
fn retained_document_keeps_adopted_styles_until_its_references_are_collected() {
    let mut runtime = runtime();
    runtime.eval(r#"
        var frame = document.getElementById('f');
        frame.contentWindow.eval("var sheet = new CSSStyleSheet(); sheet.replaceSync('p { color: rgb(1, 2, 3) }'); document.adoptedStyleSheets = [sheet]; getComputedStyle(document.querySelector('p')).color;");
        var oldDoc = frame.contentDocument;
        frame.srcdoc = '<p>new</p>'; void frame.contentDocument;
    "#).unwrap();
    collect(&mut runtime);
    assert_eq!(
        runtime
            .eval("getComputedStyle(oldDoc.querySelector('p')).color === 'rgb(1, 2, 3)'")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    runtime.eval("oldDoc = null").unwrap();
    collect(&mut runtime);
    assert!(runtime.host_state.borrow().adopted_stylesheets.is_empty());
}

#[test]
fn retired_realm_references_cannot_restart_network_timers_or_focus() {
    let mut runtime = runtime();
    runtime.eval(r#"
        var frame = document.getElementById('f');
        var oldDoc = frame.contentDocument;
        var request = frame.contentWindow.eval("() => { fetch('http://127.0.0.1:9/retired').then(() => parent.networkResult = 'allowed', error => parent.networkResult = error.name); setTimeout(() => parent.timerRan = true, 0); setInterval(() => parent.intervalRan = true, 0); requestAnimationFrame(() => parent.animationRan = true); }");
        frame.remove(); request();
        var input = oldDoc.createElement('input'); oldDoc.body.appendChild(input); input.focus();
    "#).unwrap();
    runtime.run_jobs().unwrap();
    assert_eq!(runtime.eval("networkResult === 'ReferenceError' && typeof timerRan === 'undefined' && typeof intervalRan === 'undefined' && typeof animationRan === 'undefined' && !oldDoc.hasFocus() && document.hasFocus()").unwrap().as_boolean(), Some(true));
    assert!(!runtime.has_pending_timers());
}

#[test]
fn retained_document_groups_trace_new_wrappers_after_minor_collection() {
    let mut runtime = runtime();
    runtime.eval("var frame = document.getElementById('f'); var oldDoc = frame.contentDocument; void oldDoc.body; frame.remove();").unwrap();
    collect(&mut runtime);
    for index in 0..8 {
        runtime.eval(&format!("(() => {{ const node = oldDoc.createElement('p'); node.id = 'young{index}'; node.marker = {{ value: {index} }}; oldDoc.body.appendChild(node); }})()")).unwrap();
        runtime.context.clear_kept_objects();
        boa_gc::force_minor_collect();
        assert_eq!(
            runtime
                .eval(&format!(
                    "oldDoc.getElementById('young{index}').marker.value"
                ))
                .unwrap()
                .as_number(),
            Some(index as f64)
        );
    }
    runtime.eval("oldDoc = null").unwrap();
    collect(&mut runtime);
    assert_eq!(
        runtime.host_state.borrow().node_lifetimes.document_count(),
        1
    );
}

#[test]
fn resource_tasks_already_taken_from_the_queue_cannot_revive_retired_documents() {
    let mut runtime = runtime();
    runtime.eval(r#"
        var frame = document.getElementById('f'); var oldDoc = frame.contentDocument;
        var nested = oldDoc.createElement('iframe'); nested.srcdoc = '<p>nested</p>'; oldDoc.body.appendChild(nested);
        var script = oldDoc.createElement('script'); script.src = 'data:text/javascript,globalThis.revived=1'; oldDoc.body.appendChild(script);
        frame.remove();
    "#).unwrap();
    let nested_id = runtime.eval("nested.__id").unwrap().as_number().unwrap() as usize;
    let script_id = runtime.eval("script.__id").unwrap().as_number().unwrap() as usize;
    runtime
        .run_timer_payload(TimerPayload::ResourceLoad { node_id: nested_id })
        .unwrap();
    runtime
        .run_timer_payload(TimerPayload::ResourceLoad { node_id: script_id })
        .unwrap();
    {
        let mut future = Box::pin(runtime.run_dynamic_script_resource_async(script_id));
        let mut context = TaskContext::from_waker(std::task::Waker::noop());
        assert!(matches!(
            future.as_mut().poll(&mut context),
            Poll::Ready(Ok(()))
        ));
    }
    assert!(runtime.host_state.borrow().iframe_documents.is_empty());
    assert_eq!(
        runtime
            .eval("typeof revived")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "undefined"
    );
}

#[test]
fn adoption_updates_all_realm_aliases_across_minor_collection() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"
        var frame = document.getElementById('f');
        var mainAlias = frame.contentDocument.getElementById('old');
        var childAlias = frame.contentWindow.eval("document.getElementById('old')");
        mainAlias.marker = { value: 41 }; childAlias.marker = { value: 42 };
        frame.srcdoc = '<p>new</p>'; void frame.contentDocument;
    "#,
        )
        .unwrap();
    collect(&mut runtime);
    runtime
        .eval("document.body.appendChild(mainAlias)")
        .unwrap();
    runtime.context.clear_kept_objects();
    boa_gc::force_minor_collect();
    assert_eq!(runtime.eval("mainAlias.__id === childAlias.__id && childAlias.ownerDocument.__id === document.__id && mainAlias.marker.value === 41 && childAlias.marker.value === 42").unwrap().as_boolean(), Some(true));
    runtime.eval("childAlias = null").unwrap();
    collect(&mut runtime);
    assert_eq!(runtime.eval("document.getElementById('old') === mainAlias && mainAlias.ownerDocument === document").unwrap().as_boolean(), Some(true));
    assert!(runtime.host_state.borrow().node_lifetimes.document_count() <= 2);
}
