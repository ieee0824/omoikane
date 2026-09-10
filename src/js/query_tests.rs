use super::*;
use std::cell::Cell;

thread_local! {
    static ROOT_STEPS: Cell<usize> = const { Cell::new(0) };
}

pub(super) fn record_root_step() {
    ROOT_STEPS.with(|steps| steps.set(steps.get() + 1));
}

#[test]
fn enrolling_a_connected_subtree_resolves_its_document_once() {
    for depth in [32, 64, 128] {
        let runtime = JsRuntime::new().unwrap();
        let outer = NodeHandle::element("div");
        let mut inner = outer.clone();
        for _ in 1..depth {
            let child = NodeHandle::element("div");
            inner.append_child(child.clone());
            inner = child;
        }
        runtime
            .document()
            .query_selector("body")
            .unwrap()
            .append_child(outer.clone());
        let mut state = runtime.host_state.borrow_mut();
        state.node_lifetimes.enrollment_visits = 0;
        ROOT_STEPS.with(|steps| steps.set(0));
        state.register_tree(&outer);
        let visits = state.node_lifetimes.enrollment_visits;
        let root_steps = ROOT_STEPS.with(Cell::get);
        eprintln!("registration depth={depth} enrollment_visits={visits} root_steps={root_steps}");
        assert_eq!(visits, depth);
        assert!(
            root_steps <= 4,
            "only the outer node needs an ancestor walk"
        );
        assert_eq!(state.get_node(inner.identity()), Some(inner));
    }
}

#[test]
fn repeated_queries_do_not_reenroll_registered_descendants() {
    for depth in [32, 64, 128] {
        let html = format!(
            "<main>{}{}</main>",
            "<div>".repeat(depth),
            "</div>".repeat(depth)
        );
        let mut runtime =
            JsRuntime::with_document(crate::html::TreeBuilder::parse(&html).document()).unwrap();
        runtime
            .eval("var previous = document.querySelectorAll('div');")
            .unwrap();
        runtime
            .host_state
            .borrow_mut()
            .node_lifetimes
            .enrollment_visits = 0;
        ROOT_STEPS.with(|steps| steps.set(0));
        let result = runtime.eval(
            "var current = document.querySelectorAll('div'); current.length === previous.length && Array.from(current).every((node, i) => node === previous[i]);"
        ).unwrap();
        assert_eq!(result.as_boolean(), Some(true));
        let visits = runtime.host_state.borrow().node_lifetimes.enrollment_visits;
        let root_steps = ROOT_STEPS.with(Cell::get);
        eprintln!("query depth={depth} enrollment_visits={visits} root_steps={root_steps}");
        assert_eq!(
            visits, 0,
            "unchanged query results must not re-enroll subtrees"
        );
        assert!(
            root_steps <= depth,
            "a query must not walk ancestors per result"
        );
    }
}

#[test]
fn queries_register_new_native_subtrees_and_keep_static_result_order() {
    let document = crate::html::TreeBuilder::parse("<main id='scope'></main>").document();
    let scope = document.query_selector("#scope").unwrap();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    // Embedders may add native nodes between evaluations, without JS wrappers.
    let outer = NodeHandle::element("div");
    let inner = NodeHandle::element("div");
    outer.set_attribute("id", "outer");
    inner.set_attribute("id", "inner");
    outer.append_child(inner);
    scope.append_child(outer);
    assert_eq!(
        runtime
            .eval(
                r#"
        var before = document.querySelectorAll('div, #outer');
        var added = document.createElement('div'); added.id = 'added';
        document.getElementById('inner').appendChild(added);
        var after = document.querySelectorAll('div');
        before.length === 2 && before[0].id === 'outer' && before[1].id === 'inner' &&
        after.length === 3 && after[2] === added && after[0] === before[0] &&
        before[1].ownerDocument === document
    "#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn query_results_preserve_template_shadow_and_adopted_owners_after_collection() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime.eval(r#"
        var inert = document.implementation.createHTMLDocument('inert');
        inert.body.innerHTML = '<section><div id="plain"></div><template><div id="inert"></div></template></section>';
        var section = inert.querySelector('section');
        var template = section.querySelector('template');
        var contentOwner = template.content.ownerDocument;
        var templateNode = template.content.querySelectorAll('div')[0];
        var root = section.attachShadow({mode: 'open'});
        root.innerHTML = '<div id="shadow"></div>';
        var shadowNode = root.querySelectorAll('div')[0];
        var plain = section.querySelectorAll('div')[0];
        // Insertion adopts the subtree, including its shadow and template trees.
        document.body.appendChild(section);
        var adoptedContentOwner = template.content.ownerDocument;
        var found = document.querySelectorAll('div');
        section.remove();
    "#).unwrap();
    runtime.context.clear_kept_objects();
    boa_gc::force_collect();
    runtime.host_state.borrow_mut().sweep_node_lifetimes();
    assert_eq!(
        runtime
            .eval(
                r#"
        found.length === 1 && found[0] === plain &&
        section.querySelectorAll('div')[0] === plain && plain.ownerDocument === document &&
        root.querySelectorAll('div')[0] === shadowNode && shadowNode.ownerDocument === document &&
        template.content.querySelectorAll('div')[0] === templateNode &&
        templateNode.ownerDocument === adoptedContentOwner && adoptedContentOwner !== document &&
        contentOwner !== adoptedContentOwner
    "#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn query_results_retain_retired_documents_without_resurrecting_host_roots() {
    let mut runtime = JsRuntime::with_document(
        crate::html::TreeBuilder::parse(
            "<iframe id='frame' srcdoc='<div id=outer><div id=inner></div></div>'></iframe>",
        )
        .document(),
    )
    .unwrap();
    runtime
        .eval(
            r#"
        var frame = document.getElementById('frame');
        var held = frame.contentDocument.querySelectorAll('div');
        held[1].marker = 42;
        frame.srcdoc = '<p>new document</p>'; void frame.contentDocument;
        frame.remove(); frame = null;
    "#,
        )
        .unwrap();
    runtime.context.clear_kept_objects();
    boa_gc::force_collect();
    runtime.host_state.borrow_mut().sweep_node_lifetimes();
    assert_eq!(
        runtime
            .eval(
                r#"
        held.length === 2 && held[0].id === 'outer' && held[1].marker === 42 &&
        held[0].querySelectorAll('div')[0] === held[1] &&
        held[1].ownerDocument.querySelectorAll('div')[0] === held[0]
    "#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    runtime.eval("held = null").unwrap();
    runtime.context.clear_kept_objects();
    boa_gc::force_collect();
    runtime.host_state.borrow_mut().sweep_node_lifetimes();
    assert_eq!(
        runtime.host_state.borrow().node_lifetimes.document_count(),
        1
    );
}
