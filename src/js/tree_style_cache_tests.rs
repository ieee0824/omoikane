//! Tree mutations refresh selectors/geometry while retaining unchanged rules.
use super::*;

fn runtime() -> JsRuntime {
    let doc = crate::html::TreeBuilder::parse(
        "<html><head><style>#host{width:40px;height:10px}#host:empty{width:10px}.item{display:block;width:20px;height:1px}.item:first-child{width:30px}.item:last-child{width:40px}</style></head><body><div id=host></div></body></html>",
    ).document();
    let mut runtime = JsRuntime::with_document(doc).unwrap();
    runtime
        .eval("globalThis.host=document.getElementById('host');host.getBoundingClientRect().width")
        .unwrap();
    runtime
}

fn generation(runtime: &JsRuntime) -> u64 {
    runtime.host_state.borrow().style_resolver_generation
}

#[test]
fn ordinary_tree_append_remove_retains_rules_and_updates_empty_selector() {
    let mut runtime = runtime();
    let initial = generation(&runtime);
    assert!(runtime.eval("(() => {for(let i=0;i<4;i++){const child=document.createElement('span');host.appendChild(child);if(host.getBoundingClientRect().width!==40)return false;host.removeChild(child);if(host.getBoundingClientRect().width!==10)return false}return true})()").unwrap().to_boolean());
    assert_eq!(
        generation(&runtime),
        initial,
        "ordinary child changes must retain parsed rules"
    );
}

#[test]
fn ordinary_tree_reorder_retains_rules_and_refreshes_structural_selectors() {
    let mut runtime = runtime();
    runtime.eval("globalThis.a=document.createElement('div');a.className='item';globalThis.b=document.createElement('div');b.className='item';host.appendChild(a);host.appendChild(b);host.getBoundingClientRect()").unwrap();
    let initial = generation(&runtime);
    assert!(
        runtime
            .eval("a.getBoundingClientRect().width===30 && b.getBoundingClientRect().width===40")
            .unwrap()
            .to_boolean()
    );
    runtime.eval("host.insertBefore(b,a)").unwrap();
    assert!(runtime.eval("b.getBoundingClientRect().width===30 && a.getBoundingClientRect().width===40 && host.firstElementChild===b").unwrap().to_boolean());
    assert_eq!(
        generation(&runtime),
        initial,
        "reordering ordinary children must retain parsed rules"
    );
}

#[test]
fn stylesheet_subtree_insertion_and_removal_rebuild_rules() {
    let mut runtime = runtime();
    let initial = generation(&runtime);
    runtime.eval("globalThis.sheet=document.createElement('style');sheet.textContent='#host{width:70px}';host.appendChild(sheet)").unwrap();
    assert_eq!(
        runtime
            .eval("host.getBoundingClientRect().width")
            .unwrap()
            .as_number(),
        Some(70.0)
    );
    assert!(generation(&runtime) > initial);
    let inserted = generation(&runtime);
    runtime.eval("host.removeChild(sheet)").unwrap();
    assert_eq!(
        runtime
            .eval("host.getBoundingClientRect().width")
            .unwrap()
            .as_number(),
        Some(10.0)
    );
    assert!(generation(&runtime) > inserted);
}

#[test]
fn inserted_inline_styles_still_observe_csp() {
    let mut runtime = runtime();
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "style-src-elem 'unsafe-inline'; style-src-attr 'none'".into(),
    )]);
    runtime.eval("host.getBoundingClientRect()").unwrap();
    let initial = generation(&runtime);
    runtime.eval("globalThis.child=document.createElement('div');child.className='item';child.setAttribute('style','width:99px');host.appendChild(child)").unwrap();
    assert_eq!(
        runtime
            .eval("child.getBoundingClientRect().width")
            .unwrap()
            .as_number(),
        Some(40.0)
    );
    assert!(
        generation(&runtime) > initial,
        "inline-style insertion must retain the conservative CSP refresh"
    );
}

#[test]
fn ordinary_element_text_replacement_retains_rules_and_refreshes_empty_state() {
    let mut runtime = runtime();
    let initial = generation(&runtime);
    assert!(runtime.eval("host.textContent='hello';const full=host.getBoundingClientRect().width===40;host.textContent='';full && host.getBoundingClientRect().width===10").unwrap().to_boolean());
    assert_eq!(generation(&runtime), initial);
}

#[test]
fn ordinary_inner_html_replacement_retains_rules_and_refreshes_geometry() {
    let mut runtime = runtime();
    let initial = generation(&runtime);
    assert!(runtime.eval("host.innerHTML='<div class=item></div>';const full=host.getBoundingClientRect().width===40 && host.firstElementChild.getBoundingClientRect().width===40;host.innerHTML='';full && host.getBoundingClientRect().width===10").unwrap().to_boolean());
    assert_eq!(generation(&runtime), initial);
}

#[test]
fn replacement_inline_styles_still_observe_csp() {
    let mut runtime = runtime();
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "style-src-elem 'unsafe-inline'; style-src-attr 'none'".into(),
    )]);
    runtime.eval("host.getBoundingClientRect()").unwrap();
    let initial = generation(&runtime);
    runtime
        .eval("host.innerHTML='<div class=item style=\"width:99px\"></div>'")
        .unwrap();
    assert_eq!(
        runtime
            .eval("host.firstElementChild.getBoundingClientRect().width")
            .unwrap()
            .as_number(),
        Some(40.0)
    );
    assert!(generation(&runtime) > initial);
}

#[test]
fn shadow_tree_changes_preserve_scoped_rules_and_use_the_conservative_path() {
    let mut runtime = runtime();
    runtime.eval("globalThis.shadow=host.attachShadow({mode:'open'});shadow.innerHTML='<style>.thing{display:block;width:66px;height:1px}</style>';host.getBoundingClientRect()").unwrap();
    let initial = generation(&runtime);
    runtime.eval("globalThis.child=document.createElement('div');child.className='thing';shadow.appendChild(child)").unwrap();
    assert_eq!(
        runtime
            .eval("child.getBoundingClientRect().width")
            .unwrap()
            .as_number(),
        Some(66.0)
    );
    assert!(generation(&runtime) > initial);
}

#[test]
fn text_inside_a_stylesheet_descendant_refreshes_the_stylesheet_source() {
    let mut runtime = runtime();
    runtime.eval("globalThis.sheet=document.createElement('style');globalThis.source=document.createElement('span');source.textContent='#host:empty{width:70px}';sheet.appendChild(source);document.head.appendChild(sheet)").unwrap();
    assert_eq!(
        runtime
            .eval("host.getBoundingClientRect().width")
            .unwrap()
            .as_number(),
        Some(70.0)
    );
    let initial = generation(&runtime);
    runtime
        .eval("source.firstChild.data='#host:empty{width:80px}'")
        .unwrap();
    assert_eq!(
        runtime
            .eval("host.getBoundingClientRect().width")
            .unwrap()
            .as_number(),
        Some(80.0)
    );
    assert!(generation(&runtime) > initial);
}

#[test]
fn ordinary_tree_changes_refresh_form_constraint_state() {
    let mut runtime = runtime();
    assert!(runtime.eval("globalThis.form=document.createElement('form');host.appendChild(form);form.checkValidity()").unwrap().to_boolean());
    assert!(runtime.eval("globalThis.field=document.createElement('input');field.required=true;form.appendChild(field);!form.checkValidity()").unwrap().to_boolean());
    assert!(
        runtime
            .eval("field.value='ok';form.checkValidity()")
            .unwrap()
            .to_boolean()
    );
}
