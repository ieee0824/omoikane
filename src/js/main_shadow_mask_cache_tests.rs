use std::rc::Rc;

use super::*;

fn shadow_runtime(extra_body: &str) -> JsRuntime {
    let document = crate::html::TreeBuilder::parse(&format!(
        "<html><head><style>html,body{{margin:0;background:white}}\
         .card{{position:absolute;left:20px;top:20px;width:50px;height:30px;\
         border-radius:7px;background:#eef;box-shadow:2px 1px 7px 2px #2348}}</style></head>\
         <body><div class=card></div>{extra_body}</body></html>"
    ))
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime.set_viewport(160.0, 100.0);
    runtime
}

fn main_cache_entries(runtime: &JsRuntime) -> usize {
    runtime
        .host_state
        .borrow()
        .main_shadow_masks
        .retained_entries()
}

#[test]
fn main_document_cache_survives_rasters_with_exact_pixels() {
    let mut runtime = shadow_runtime("");
    let first = runtime.paint_current_document().unwrap();
    let entries = main_cache_entries(&runtime);
    assert!(entries > 0);

    runtime
        .eval("document.querySelector('.card').style.display='none'")
        .unwrap();
    let hidden = runtime.paint_current_document().unwrap();
    assert_ne!(hidden.pixels(), first.pixels());
    assert_eq!(main_cache_entries(&runtime), entries);

    runtime
        .eval("document.querySelector('.card').style.removeProperty('display')")
        .unwrap();
    let restored = runtime.paint_current_document().unwrap();
    assert_eq!(restored.pixels(), first.pixels());
    assert_eq!(main_cache_entries(&runtime), entries);
}

#[test]
fn owner_document_mismatch_replaces_entries_before_the_next_raster() {
    let mut runtime = shadow_runtime("");
    let first = runtime.paint_current_document().unwrap();
    assert!(main_cache_entries(&runtime) > 0);

    {
        let mut state = runtime.host_state.borrow_mut();
        let document_id = state.document.identity();
        let different_document_id = document_id.wrapping_add(1);
        assert_ne!(different_document_id, document_id);
        assert_eq!(
            state
                .main_shadow_masks
                .cache_for(different_document_id)
                .retained_entries(),
            0
        );
    }
    assert_eq!(main_cache_entries(&runtime), 0);

    let regenerated = runtime.paint_current_document().unwrap();
    assert_eq!(regenerated.pixels(), first.pixels());
    assert!(main_cache_entries(&runtime) > 0);
}

#[test]
fn child_raster_keeps_a_distinct_ephemeral_shadow_cache() {
    let mut runtime = shadow_runtime("<iframe id=child style='width:100px;height:70px'></iframe>");
    runtime
        .eval(
            "const d=document.getElementById('child').contentDocument;d.open();\
             d.write('<style>html,body{margin:0}.child{position:absolute;left:8px;top:8px;\
             width:72px;height:34px;border-radius:3px;box-shadow:1px 2px 4px 1px #1238}</style>\
             <div class=child></div>');d.close()",
        )
        .unwrap();

    let first = runtime.paint_current_document().unwrap();
    assert_eq!(main_cache_entries(&runtime), 1);
    let second = runtime.paint_current_document().unwrap();
    assert_eq!(second.pixels(), first.pixels());
    assert_eq!(main_cache_entries(&runtime), 1);
}

#[test]
fn successful_empty_main_document_open_clears_cache_but_child_open_does_not() {
    let mut runtime = shadow_runtime("<iframe id=child></iframe>");
    runtime.paint_current_document().unwrap();
    let entries = main_cache_entries(&runtime);
    assert!(entries > 0);

    runtime
        .eval("document.getElementById('child').contentDocument.open()")
        .unwrap();
    assert_eq!(main_cache_entries(&runtime), entries);

    let document = runtime.document();
    for child in document.child_nodes() {
        document.remove_child(&child).unwrap();
    }
    assert!(document.child_nodes().is_empty());
    assert_eq!(main_cache_entries(&runtime), entries);
    runtime.eval("document.open()").unwrap();
    assert_eq!(main_cache_entries(&runtime), 0);
}

#[test]
fn parser_reentrant_main_document_open_preserves_entries() {
    let mut runtime = shadow_runtime("");
    runtime.paint_current_document().unwrap();
    let entries = main_cache_entries(&runtime);
    assert!(entries > 0);

    runtime
        .eval(
            "document.write('<script>document.open();globalThis.cardSurvivedOpen=\
             !!document.querySelector(\".card\");<\\/script><b id=written>ok</b>')",
        )
        .unwrap();
    assert!(
        runtime
            .eval("cardSurvivedOpen && !!document.getElementById('written')")
            .unwrap()
            .to_boolean()
    );
    assert_eq!(main_cache_entries(&runtime), entries);
}

#[test]
fn runtime_drop_releases_main_cache_with_a_retained_host_state() {
    let mut runtime = shadow_runtime("");
    runtime.paint_current_document().unwrap();
    assert!(main_cache_entries(&runtime) > 0);
    let host_state = Rc::clone(&runtime.host_state);

    drop(runtime);

    assert_eq!(host_state.borrow().main_shadow_masks.retained_entries(), 0);
}
