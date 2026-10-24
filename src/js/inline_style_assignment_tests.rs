use super::*;

fn runtime() -> JsRuntime {
    JsRuntime::with_document(
        crate::html::TreeBuilder::parse(
            "<div id=t style='position:absolute;width:10px;height:10px;top:0px;left:0px'></div>",
        )
        .document(),
    )
    .unwrap()
}

#[test]
fn repeated_independent_inline_property_assignment_keeps_attribute_and_layout_fresh() {
    let mut runtime = runtime();
    runtime.eval("globalThis.t=document.getElementById('t');t.getBoundingClientRect();globalThis.before=t.getAttribute('style');globalThis.observer=new MutationObserver(()=>{});observer.observe(t,{attributes:true});").unwrap();
    let generation = runtime.host_state.borrow().style_generation;
    let records = runtime.eval("for(let i=0;i<20;i++){t.style.width='10px';t.style.top='0px'}observer.takeRecords().length").unwrap();
    assert_eq!(records.as_number(), Some(0.0));
    assert!(runtime.eval("t.getAttribute('style')===before && t.style.item(1)==='width' && t.style.item(3)==='top'").unwrap().to_boolean());
    assert_eq!(runtime.host_state.borrow().style_generation, generation);
}

#[test]
fn changed_independent_value_and_priority_update_in_place_and_notify() {
    let mut runtime = runtime();
    assert!(runtime.eval("(()=>{const t=document.getElementById('t');const observer=new MutationObserver(()=>{});observer.observe(t,{attributes:true});t.style.width='20px';const valueRecords=observer.takeRecords().length;t.style.setProperty('width','20px','important');const priorityRecords=observer.takeRecords().length;return valueRecords===1 && priorityRecords===1 && t.style.item(1)==='width' && t.style.getPropertyPriority('width')==='important' && t.getBoundingClientRect().width===20})()").unwrap().to_boolean());
}

#[test]
fn direct_same_style_attribute_set_still_delivers_a_mutation() {
    let mut runtime = runtime();
    assert!(runtime.eval("(()=>{const t=document.getElementById('t');const observer=new MutationObserver(()=>{});observer.observe(t,{attributes:true});const before=t.getAttribute('style');t.setAttribute('style',before);return observer.takeRecords().length===1 && t.getAttribute('style')===before})()").unwrap().to_boolean());
}

#[test]
fn conflicting_logical_and_shorthand_declarations_keep_assignment_precedence() {
    let mut runtime = runtime();
    assert!(runtime.eval("(()=>{const t=document.getElementById('t');t.style.cssText='left:1px;inset-inline-start:2px';t.style.left='1px';const logical=Array.from({length:t.style.length},(_,i)=>t.style.item(i));if(logical.indexOf('left')<logical.indexOf('inset-inline-start'))return false;t.style.cssText='width:3px;all:initial';t.style.width='3px';const reset=Array.from({length:t.style.length},(_,i)=>t.style.item(i));return reset.indexOf('width')>reset.indexOf('all')})()").unwrap().to_boolean());
}

#[test]
fn duplicate_independent_properties_collapse_even_when_one_value_matches() {
    let mut runtime = runtime();
    assert!(runtime.eval("(()=>{const t=document.getElementById('t');for(const value of ['red','blue']){t.style.cssText='color:red;color:blue';const observer=new MutationObserver(()=>{});observer.observe(t,{attributes:true});t.style.setProperty('color',value);if(t.style.length!==1 || t.style.getPropertyValue('color')!==value || observer.takeRecords().length!==1)return false;observer.disconnect()}return true})()").unwrap().to_boolean());
}

#[test]
fn background_image_cssom_updates_fifteen_hidden_subtrees_without_global_cache_clear() {
    let mut html = String::from(
        "<html><head><style>.target{display:none;background-image:none}.target span{background-image:inherit}\
         #unrelated::before{content:'cached'}</style></head><body>",
    );
    for index in 0..15 {
        html.push_str(&format!(
            "<div class='target' id='target-{index}'><span></span></div>"
        ));
    }
    html.push_str("<aside id='unrelated'></aside></body></html>");
    let mut runtime = JsRuntime::with_document(crate::html::TreeBuilder::parse(&html).document())
        .expect("background-image fixture should bootstrap");
    assert!(
        runtime
            .eval(
                r#"globalThis.targets=Array.from(document.querySelectorAll('.target'));
                   globalThis.unrelated=document.getElementById('unrelated');
                   unrelated.getBoundingClientRect();
                   globalThis.unrelatedPseudoContent=getComputedStyle(unrelated,'::before').content;
                   targets.length===15&&targets.every(target =>
                     getComputedStyle(target).display==='none'&&
                     getComputedStyle(target.firstChild).backgroundImage==='none')"#,
            )
            .unwrap()
            .to_boolean(),
    );
    let layout_generation = runtime.render_generations().layout;
    let unrelated = runtime
        .host_state
        .borrow()
        .document
        .query_selector("#unrelated")
        .unwrap();
    let unrelated_styles = |runtime: &JsRuntime| {
        let mut state = runtime.host_state.borrow_mut();
        let document_id = state.document.identity();
        let resolver = state
            .document_styles
            .get_mut(&document_id)
            .and_then(|entry| entry.resolver.as_mut())
            .unwrap();
        (
            resolver.computed_style_shared(&unrelated),
            resolver
                .computed_pseudo_style_shared(&unrelated, crate::css::PseudoElement::Before)
                .unwrap(),
        )
    };
    let before = unrelated_styles(&runtime);

    assert!(
        runtime
            .eval(
                r#"targets.every((target,index) => {
                  target.style.backgroundImage='linear-gradient(red, blue)';
                  const image=getComputedStyle(target).backgroundImage;
                  return target.style.backgroundImage.includes('linear-gradient')&&
                    image.includes('linear-gradient')&&
                    getComputedStyle(target.firstChild).backgroundImage===image;
                })"#,
            )
            .unwrap()
            .to_boolean(),
    );
    assert_eq!(runtime.render_generations().layout, layout_generation);
    assert!(
        runtime
            .eval("unrelated.getBoundingClientRect().width>=0")
            .unwrap()
            .to_boolean(),
    );
    assert_eq!(runtime.render_generations().layout, layout_generation);
    let after = unrelated_styles(&runtime);
    assert!(std::sync::Arc::ptr_eq(&before.0, &after.0));
    assert!(std::sync::Arc::ptr_eq(&before.1, &after.1));
}

#[test]
fn inline_display_none_focus_fixup_refreshes_external_selector_and_paint() {
    let html = "<style>body{margin:0}#target{width:20px;height:20px;background:red}\
        #observer{height:20px;background:green;color:blue}\
        body:has(#target:focus) #observer{color:red}</style>\
        <div id=target tabindex=0 style='display:block'></div><aside id=observer></aside>";
    let mut runtime =
        JsRuntime::with_document(crate::html::TreeBuilder::parse(html).document()).unwrap();
    assert!(runtime.eval("globalThis.target=document.getElementById('target');globalThis.observer=document.getElementById('observer');globalThis.cold=getComputedStyle(observer).color;target.focus();document.activeElement===target&&getComputedStyle(observer).color!==cold").unwrap().to_boolean());
    let before = runtime
        .paint_current_document()
        .unwrap()
        .pixel(5, 5)
        .unwrap();
    assert_eq!((before.r, before.g, before.b), (255, 0, 0));
    let generation = runtime.render_generations().layout;
    assert!(runtime.eval("target.style.display='none';document.activeElement!==target&&getComputedStyle(observer).color===cold").unwrap().to_boolean());
    let after = runtime
        .paint_current_document()
        .unwrap()
        .pixel(5, 5)
        .unwrap();
    assert_eq!((after.r, after.g, after.b), (0, 128, 0));
    assert!(runtime.render_generations().layout > generation);
    assert!(runtime.eval("target.style.display='block';target.focus();document.activeElement===target&&getComputedStyle(observer).color!==cold").unwrap().to_boolean());
    let shown = runtime
        .paint_current_document()
        .unwrap()
        .pixel(5, 5)
        .unwrap();
    assert_eq!((shown.r, shown.g, shown.b), (255, 0, 0));
}

#[test]
fn repeated_inline_assignment_with_unrelated_declarations_preserves_layout() {
    for extra in [
        "font-size:16px;overflow:hidden",
        "margin:0;padding:0",
        "background:red;box-sizing:border-box",
    ] {
        let mut runtime = runtime();
        runtime.eval(&format!(
            "globalThis.t=document.getElementById('t');t.style.cssText='position:absolute;width:10px;height:10px;top:0px;left:0px;{extra}';t.getBoundingClientRect();globalThis.before=t.getAttribute('style');globalThis.observer=new MutationObserver(()=>{{}});observer.observe(t,{{attributes:true}});"
        )).unwrap();
        let generation = runtime.host_state.borrow().style_generation;
        let records = runtime.eval("for(let i=0;i<20;i++){t.style.position='absolute';t.style.width='10px';t.getBoundingClientRect()}observer.takeRecords().length").unwrap();
        assert_eq!(records.as_number(), Some(0.0), "{extra}");
        assert!(
            runtime
                .eval("t.getAttribute('style')===before")
                .unwrap()
                .to_boolean(),
            "{extra}"
        );
        assert_eq!(
            runtime.host_state.borrow().style_generation,
            generation,
            "{extra}"
        );
    }
}

#[test]
fn repeated_inline_assignment_preserves_later_conflicting_declaration_precedence() {
    for (property, value, later) in [
        ("width", "10px", "inline-size:20px"),
        ("height", "10px", "block-size:20px"),
        ("left", "1px", "inset:2px"),
        ("left", "1px", "inset-inline-start:2px"),
        ("top", "1px", "inset-block-start:2px"),
        ("background-color", "red", "background:blue"),
        (
            "background-image",
            "none",
            "background:linear-gradient(red,blue)",
        ),
        ("color", "red", "all:initial"),
    ] {
        let mut runtime = runtime();
        let setup = format!("{property}:{value};{later};overflow:hidden");
        let script = format!(
            r#"(()=>{{
              const t=document.getElementById('t');
              t.style.cssText={setup};
              const property={property}, value={value}, later={later};
              if(!t.style.getPropertyValue(later))throw new Error('fixture declaration rejected');
              const reference=document.createElement('div');
              reference.style.cssText={setup}+';'+property+':'+value;
              document.body.appendChild(reference);
              const observer=new MutationObserver(()=>{{}});
              observer.observe(t,{{attributes:true}});
              t.style.setProperty(property,value);
              const names=Array.from({{length:t.style.length}},(_,i)=>t.style.item(i));
              return observer.takeRecords().length===1 &&
                names.indexOf(property)>names.indexOf(later) &&
                t.style.getPropertyValue(property)===value &&
                getComputedStyle(t).getPropertyValue(property)===
                  getComputedStyle(reference).getPropertyValue(property);
            }})()"#,
            setup = serde_json::to_string(&setup).unwrap(),
            property = serde_json::to_string(property).unwrap(),
            value = serde_json::to_string(value).unwrap(),
            later = serde_json::to_string(later.split(':').next().unwrap()).unwrap(),
        );
        assert!(runtime.eval(&script).unwrap().to_boolean(), "{setup}");
    }
}

#[test]
fn mixed_inline_changed_values_and_priorities_remain_observable() {
    let mut runtime = runtime();
    assert!(runtime.eval(r#"(()=>{
      const t=document.getElementById('t');
      t.style.cssText='width:10px;overflow:hidden';
      const observer=new MutationObserver(()=>{});
      observer.observe(t,{attributes:true});
      t.style.width='20px';
      if(observer.takeRecords().length!==1 || t.getBoundingClientRect().width!==20)return false;
      t.style.setProperty('width','20px','important');
      if(observer.takeRecords().length!==1 || t.style.getPropertyPriority('width')!=='important')return false;
      t.style.setProperty('width','20px','important');
      if(observer.takeRecords().length!==0)return false;
      t.style.setProperty('width','20px','');
      return observer.takeRecords().length===1 && t.style.getPropertyPriority('width')==='';
    })()"#).unwrap().to_boolean());
}
