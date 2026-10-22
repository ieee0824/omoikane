//! Read-only CSSOM snapshots share styles; resolved-value edits stay isolated.
use super::*;

fn runtime() -> JsRuntime {
    let document = crate::html::TreeBuilder::parse(
        "<style>body{margin:0}section{width:200px}#box{width:50%;height:40px;\
         transform:translateX(50%);--label:original}#box::before{content:'label';\
         color:blue}</style><section><div id='box'></div></section>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime.set_viewport(400.0, 300.0);
    runtime
}

fn assert_read_reuses_cache(pseudo: Option<PseudoElement>) {
    let runtime = runtime();
    let mut state = runtime.host_state.borrow_mut();
    let document = state.document.clone();
    let node = document.query_selector("#box").unwrap();
    state.ensure_style_resolver(&document);
    let resolver = document_style_resolver(&mut state, &document).unwrap();
    let cached = match pseudo {
        Some(pseudo) => resolver
            .computed_pseudo_style_shared(&node, pseudo)
            .unwrap(),
        None => resolver.computed_style_shared(&node),
    };
    let snapshot = resolve_cssom_style(&mut state, &document, &node, pseudo, false).unwrap();
    assert!(
        std::ptr::eq(
            std::borrow::Borrow::<ComputedStyle>::borrow(&snapshot),
            cached.as_ref(),
        ),
        "read-only CSSOM must reuse the existing immutable style allocation"
    );
}

#[test]
fn cssom_scalar_snapshot_reuses_ordinary_style() {
    assert_read_reuses_cache(None);
}

#[test]
fn cssom_scalar_snapshot_reuses_pseudo_style() {
    assert_read_reuses_cache(Some(PseudoElement::Before));
}

fn cached_style(runtime: &JsRuntime) -> Arc<ComputedStyle> {
    let mut state = runtime.host_state.borrow_mut();
    let document = state.document.clone();
    let node = document.query_selector("#box").unwrap();
    document_style_resolver(&mut state, &document)
        .unwrap()
        .computed_style_shared(&node)
}

#[test]
fn cssom_used_sizes_and_logical_aliases_leave_cascade_snapshot_unchanged() {
    let mut runtime = runtime();
    runtime
        .eval("document.body.getBoundingClientRect()")
        .unwrap();
    let before = cached_style(&runtime);
    let expected = before.as_ref().clone();
    let values = runtime
        .eval(
            "globalThis.box=document.getElementById('box');\
             globalThis.style=getComputedStyle(box);\
             [style.width,style.inlineSize,style.transform,style.borderInlineStartColor,\
              style.cssText.length>0,style.getPropertyValue('--label')].join('|')",
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(
        values,
        "100px|100px|matrix(1, 0, 0, 1, 50, 0)|rgb(0, 0, 0)|true|original"
    );
    assert_eq!(cached_style(&runtime).as_ref(), &expected);
    assert_eq!(before.as_ref(), &expected);
    runtime.eval("box.style.width='25%'").unwrap();
    assert_eq!(
        runtime
            .eval("style.width")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "50px"
    );
    assert_eq!(
        before.as_ref(),
        &expected,
        "retained snapshots stay immutable"
    );
}
