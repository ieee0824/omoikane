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

#[test]
fn script_insertion_and_removal_retain_rules_and_refresh_structural_selectors() {
    let mut runtime = runtime();
    let initial = generation(&runtime);
    assert!(runtime.eval("(() => {const script=document.createElement('script');script.type='text/plain';script.textContent='inert';host.appendChild(script);if(host.getBoundingClientRect().width!==40)return false;host.removeChild(script);return host.getBoundingClientRect().width===10})()").unwrap().to_boolean());
    assert_eq!(
        generation(&runtime),
        initial,
        "script elements own no author stylesheet"
    );
}

#[test]
fn executed_script_insertion_retains_rules() {
    let mut runtime = runtime();
    let initial = generation(&runtime);
    runtime
        .eval("globalThis.ran=false;const script=document.createElement('script');script.textContent='globalThis.ran=true';host.appendChild(script)")
        .unwrap();
    assert!(
        runtime
            .eval("ran && host.getBoundingClientRect().width===40")
            .unwrap()
            .to_boolean()
    );
    assert_eq!(generation(&runtime), initial);
}

#[test]
fn script_text_changes_retain_rules() {
    let mut runtime = runtime();
    runtime.eval("globalThis.script=document.createElement('script');script.type='text/plain';script.textContent='a';document.head.appendChild(script);host.getBoundingClientRect()").unwrap();
    let initial = generation(&runtime);
    runtime
        .eval("script.firstChild.data='b';script.textContent='c';script.appendChild(document.createTextNode('d'));host.getBoundingClientRect()")
        .unwrap();
    assert_eq!(
        generation(&runtime),
        initial,
        "script source text is not stylesheet input"
    );
}

#[test]
fn stylesheet_inside_a_script_element_still_rebuilds_rules() {
    let mut runtime = runtime();
    runtime.eval("globalThis.script=document.createElement('script');script.type='text/plain';document.head.appendChild(script);host.getBoundingClientRect()").unwrap();
    let initial = generation(&runtime);
    runtime
        .eval("globalThis.sheet=document.createElement('style');sheet.textContent='#host:empty{width:70px}';script.appendChild(sheet)")
        .unwrap();
    assert_eq!(
        runtime
            .eval("host.getBoundingClientRect().width")
            .unwrap()
            .as_number(),
        Some(70.0)
    );
    assert!(generation(&runtime) > initial);
    let inserted = generation(&runtime);
    runtime
        .eval("sheet.textContent='#host:empty{width:80px}'")
        .unwrap();
    assert_eq!(
        runtime
            .eval("host.getBoundingClientRect().width")
            .unwrap()
            .as_number(),
        Some(80.0)
    );
    assert!(generation(&runtime) > inserted);
}

fn shared_style(runtime: &JsRuntime, id: &str) -> Arc<ComputedStyle> {
    let mut state = runtime.host_state.borrow_mut();
    let node = state
        .nodes
        .values()
        .find(|node| node.get_attribute("id").as_deref() == Some(id))
        .unwrap()
        .clone();
    let document_id = state.document.identity();
    state
        .document_styles
        .get_mut(&document_id)
        .unwrap()
        .resolver
        .as_mut()
        .unwrap()
        .computed_style_shared(&node)
}

fn local_runtime(extra_css: &str, body: &str) -> JsRuntime {
    let markup = format!(
        "<html><head><style>.list{{display:block}}.list:empty{{width:10px}}\
         .list>div{{display:block;height:1px;width:20px}}.list>div:last-child{{width:30px}}\
         #observer{{display:block;width:70px;height:1px}}{extra_css}</style></head>\
         <body><section><div class=list id=list></div><div class=list id=other><div id=moving></div></div></section>\
         <aside id=observer></aside>{body}</body></html>"
    );
    let doc = crate::html::TreeBuilder::parse(&markup).document();
    let mut runtime = JsRuntime::with_document(doc).unwrap();
    runtime
        .eval("globalThis.list=document.getElementById('list');globalThis.other=document.getElementById('other');document.getElementById('observer').getBoundingClientRect();list.getBoundingClientRect()")
        .unwrap();
    runtime
}

fn default_width(runtime: &mut JsRuntime) -> bool {
    runtime
        .eval("getComputedStyle(document.getElementById('existing')).width==='50px'")
        .unwrap()
        .to_boolean()
}

fn width(runtime: &mut JsRuntime, expression: &str) -> Option<f64> {
    runtime
        .eval(&format!("{expression}.getBoundingClientRect().width"))
        .unwrap()
        .as_number()
}

#[test]
fn child_list_changes_retain_styles_outside_the_changed_parents() {
    let mut runtime = local_runtime("", "");
    let initial = generation(&runtime);
    let observer = shared_style(&runtime, "observer");
    runtime
        .eval("globalThis.a=document.createElement('div');globalThis.b=document.createElement('div');list.appendChild(a);list.insertBefore(b,a)")
        .unwrap();
    assert_eq!(width(&mut runtime, "a"), Some(30.0));
    assert_eq!(width(&mut runtime, "b"), Some(20.0));
    runtime.eval("list.removeChild(a)").unwrap();
    assert_eq!(width(&mut runtime, "b"), Some(30.0));
    // Moving out of `other` empties it; both parents are refreshed.
    runtime
        .eval("list.appendChild(document.getElementById('moving'))")
        .unwrap();
    assert_eq!(width(&mut runtime, "other"), Some(10.0));
    assert_eq!(width(&mut runtime, "b"), Some(20.0));
    assert_eq!(
        width(&mut runtime, "document.getElementById('moving')"),
        Some(30.0)
    );
    runtime.eval("list.textContent=''").unwrap();
    assert_eq!(width(&mut runtime, "list"), Some(10.0));
    runtime
        .eval("list.innerHTML='<div id=parsed></div>'")
        .unwrap();
    assert_eq!(
        width(&mut runtime, "document.getElementById('parsed')"),
        Some(30.0)
    );
    assert!(
        Arc::ptr_eq(&observer, &shared_style(&runtime, "observer")),
        "styles outside the changed parents must be retained"
    );
    assert_eq!(generation(&runtime), initial);
}

#[test]
fn character_data_changes_refresh_the_parent_and_retain_unrelated_styles() {
    let mut runtime = local_runtime(".list:not(:empty){width:40px}", "");
    runtime
        .eval("globalThis.text=document.createTextNode('x');list.appendChild(text);list.getBoundingClientRect()")
        .unwrap();
    let observer = shared_style(&runtime, "observer");
    runtime.eval("text.textContent='y'").unwrap();
    assert_eq!(width(&mut runtime, "list"), Some(40.0));
    assert!(Arc::ptr_eq(&observer, &shared_style(&runtime, "observer")));
}

#[test]
fn relational_selectors_still_refresh_ancestors_and_unrelated_elements() {
    let mut runtime = local_runtime("body:has(.marker) #observer{width:90px}", "");
    let observer = shared_style(&runtime, "observer");
    runtime
        .eval("const marker=document.createElement('div');marker.className='marker';list.appendChild(marker)")
        .unwrap();
    assert_eq!(
        width(&mut runtime, "document.getElementById('observer')"),
        Some(90.0)
    );
    assert!(!Arc::ptr_eq(&observer, &shared_style(&runtime, "observer")));
}

#[test]
fn form_controls_clear_every_cached_style() {
    let mut runtime = local_runtime(
        "button:default{width:50px;display:block;height:1px}",
        "<form><section id=slot></section><button id=existing></button></form>",
    );
    assert!(default_width(&mut runtime));
    let existing = shared_style(&runtime, "existing");
    runtime
        .eval("document.getElementById('slot').appendChild(document.createElement('button'))")
        .unwrap();
    assert!(
        !Arc::ptr_eq(&existing, &shared_style(&runtime, "existing")),
        "a new default button changes another button outside its parent"
    );
    assert!(!default_width(&mut runtime));
}

#[test]
fn auto_direction_ancestors_clear_every_cached_style() {
    let mut runtime = local_runtime(
        "#dir-observer:dir(rtl){width:60px}#dir-observer{display:block;height:1px;width:5px}",
        "<div dir=auto><p id=dir-parent></p><span id=dir-observer></span></div>",
    );
    assert_eq!(
        width(&mut runtime, "document.getElementById('dir-observer')"),
        Some(5.0)
    );
    runtime
        .eval("document.getElementById('dir-parent').textContent='\u{5d0}'")
        .unwrap();
    assert_eq!(
        width(&mut runtime, "document.getElementById('dir-observer')"),
        Some(60.0)
    );
}

#[test]
fn form_controls_inserted_through_fragments_or_markup_clear_every_cached_style() {
    for insertion in [
        "const fragment=document.createDocumentFragment();fragment.appendChild(document.createElement('button'));document.getElementById('slot').appendChild(fragment)",
        "document.getElementById('slot').innerHTML='<button></button>'",
    ] {
        let mut runtime = local_runtime(
            "button:default{width:50px;display:block;height:1px}",
            "<form><section id=slot></section><button id=existing></button></form>",
        );
        assert!(default_width(&mut runtime));
        let existing = shared_style(&runtime, "existing");
        runtime.eval(insertion).unwrap();
        assert!(
            !Arc::ptr_eq(&existing, &shared_style(&runtime, "existing")),
            "{insertion}"
        );
    }
}

fn animated_runtime(animated: &str) -> JsRuntime {
    let mut runtime = local_runtime(
        &format!(
            "@keyframes fade{{from{{opacity:0}}to{{opacity:1}}}}{animated}{{animation:fade 1s linear forwards}}"
        ),
        "",
    );
    runtime.enable_live_css_animations();
    runtime.eval("getComputedStyle(list).opacity").unwrap();
    runtime
}

#[test]
fn unrelated_paint_only_animation_keeps_child_list_changes_local_and_advancing() {
    let mut runtime = animated_runtime("#observer");
    runtime
        .eval("getComputedStyle(document.getElementById('observer')).opacity")
        .unwrap();
    let observer = shared_style(&runtime, "observer");
    runtime
        .eval("globalThis.a=document.createElement('div');list.appendChild(a)")
        .unwrap();
    assert!(Arc::ptr_eq(&observer, &shared_style(&runtime, "observer")));
    assert_eq!(width(&mut runtime, "a"), Some(30.0));
    runtime.run_animation_frame(500).unwrap();
    assert!(
        runtime
            .eval("getComputedStyle(document.getElementById('observer')).opacity==='0.5'")
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn animated_ancestor_of_the_changed_parent_clears_every_cached_style() {
    let mut runtime = animated_runtime("section");
    let observer = shared_style(&runtime, "observer");
    runtime
        .eval("list.appendChild(document.createElement('div'))")
        .unwrap();
    assert!(!Arc::ptr_eq(&observer, &shared_style(&runtime, "observer")));
}

#[test]
fn animation_inside_the_changed_parent_stays_local_and_resamples() {
    let mut runtime = local_runtime(
        "@keyframes fade{from{opacity:0}to{opacity:1}}.skeleton{animation:fade 1s linear forwards}",
        "",
    );
    runtime.enable_live_css_animations();
    runtime
        .eval("globalThis.skeleton=document.createElement('div');skeleton.className='skeleton';list.appendChild(skeleton);getComputedStyle(skeleton).opacity;document.getElementById('observer').getBoundingClientRect()")
        .unwrap();
    runtime.run_animation_frame(250).unwrap();
    let observer = shared_style(&runtime, "observer");
    runtime
        .eval("globalThis.content=document.createElement('div');list.appendChild(content)")
        .unwrap();
    assert!(Arc::ptr_eq(&observer, &shared_style(&runtime, "observer")));
    assert_eq!(width(&mut runtime, "skeleton"), Some(20.0));
    assert_eq!(width(&mut runtime, "content"), Some(30.0));
    runtime.run_animation_frame(250).unwrap();
    assert!(
        runtime
            .eval("getComputedStyle(skeleton).opacity==='0.5'")
            .unwrap()
            .to_boolean()
    );
}
