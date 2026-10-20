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
