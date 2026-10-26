//! Directionality selectors follow HTML directionality rather than CSS `direction`.
use omoikane::{html::TreeBuilder, js::JsRuntime};

fn runtime() -> JsRuntime {
    JsRuntime::with_document(TreeBuilder::parse("<head></head><body></body>").document()).unwrap()
}

fn string(runtime: &mut JsRuntime, script: &str) -> String {
    runtime
        .eval(script)
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped()
}

#[test]
fn dir_selector_grammar_accepts_one_identifier() {
    let mut runtime = runtime();
    assert_eq!(
        string(
            &mut runtime,
            r#"(()=>{try{
const valid=[':dir(ltr)',':DIR( RTL )',':dir(auto)',':dir(foo)'];
const invalid=[':dir()',':dir(ltr rtl)',':dir("ltr")',':dir'];
if(valid.some(selector=>!CSS.supports('selector('+selector+')')))return 'valid';
if(invalid.some(selector=>CSS.supports('selector('+selector+')')))return 'invalid';
if(valid.some(selector=>{try{document.querySelector(selector);return false}catch{return true}}))return 'query-valid';
if(invalid.some(selector=>{try{document.querySelector(selector);return false}catch(e){return e.name!=='SyntaxError'}}))return 'query-invalid';
if(document.documentElement.matches(':dir(auto)')||document.documentElement.matches(':dir(foo)'))return 'unknown-match';
return 'ok';}catch(e){return e.name+': '+e.message;}})()"#,
        ),
        "ok"
    );
}

#[test]
fn dir_matches_inherited_and_auto_text_directionality() {
    let mut runtime = runtime();
    assert_eq!(
        string(
            &mut runtime,
            r#"(()=>{try{
document.body.innerHTML='<section dir=rtl><span id=child></span></section><div id=auto dir=auto><script>א</script><bdi>א</bdi><i>ABC</i></div><div id=dynamic dir=auto>א</div>';
const child=document.querySelector('#child'),auto=document.querySelector('#auto'),dynamic=document.querySelector('#dynamic');
if(!child.matches(':dir(rtl)')||child.matches(':dir(ltr)'))return 'inherit';
if(!auto.matches(':dir(ltr)'))return 'excluded';
if(!dynamic.matches(':dir(rtl)'))return 'auto-rtl';
dynamic.firstChild.data='ABC';
if(!dynamic.matches(':dir(ltr)')||dynamic.matches(':dir(rtl)'))return 'auto-live';
const foreign=document.createElementNS('urn:test','node');document.querySelector('section').append(foreign);
if(!foreign.matches(':dir(rtl)'))return 'foreign';
return 'ok';}catch(e){return e.name+': '+e.message;}})()"#,
        ),
        "ok"
    );
}

#[test]
fn dir_auto_uses_live_form_values_and_invalidates_dependent_styles() {
    let mut runtime = runtime();
    assert_eq!(
        string(
            &mut runtime,
            r#"(()=>{try{
const style=document.createElement('style');style.textContent='section{width:10px}section:has(input:dir(rtl)){width:20px}textarea:dir(rtl)+span{height:30px}textarea+span{height:10px}';document.head.append(style);
const section=document.createElement('section'),input=document.createElement('input'),textarea=document.createElement('textarea'),span=document.createElement('span');input.dir='auto';textarea.dir='auto';section.append(input,textarea,span);document.body.append(section);
if(!input.matches(':dir(ltr)')||!textarea.matches(':dir(ltr)')||getComputedStyle(section).width!=='10px')return 'initial';
input.value='א';textarea.value='א';
if(!input.matches(':dir(rtl)')||!textarea.matches(':dir(rtl)')||getComputedStyle(section).width!=='20px'||getComputedStyle(span).height!=='30px')return 'rtl';
const clone=input.cloneNode();if(!clone.matches(':dir(rtl)'))return 'clone-dirty-value';
const textareaClone=textarea.cloneNode();if(!textareaClone.matches(':dir(rtl)'))return 'textarea-clone-dirty-value';
const resetForm=document.createElement('form'),resetInput=document.createElement('input');resetInput.dir='auto';resetInput.defaultValue='ABC';resetForm.append(resetInput);document.body.append(resetForm);resetInput.value='א';resetForm.reset();
if(!resetInput.matches(':dir(ltr)'))return 'reset-value';resetInput.defaultValue='א';if(!resetInput.matches(':dir(rtl)'))return 'reset-default-update';
input.value='ABC';textarea.value='ABC';
if(!input.matches(':dir(ltr)')||!textarea.matches(':dir(ltr)')||getComputedStyle(section).width!=='10px'||getComputedStyle(span).height!=='10px')return 'ltr';
input.type='number';input.value='א';if(!input.matches(':dir(ltr)'))return 'unsupported-type';
return 'ok';}catch(e){return e.name+': '+e.message;}})()"#,
        ),
        "ok"
    );
}

#[test]
fn manual_slot_assignment_updates_directionality_and_enforces_mode() {
    let mut runtime = runtime();
    assert_eq!(
        string(
            &mut runtime,
            r#"(()=>{try{
if('__omoikane_assign_slot' in globalThis)return 'binding-visible';
const host=document.createElement('div'),container=document.createElement('div');container.textContent='א';host.append(container);document.body.append(host);
const root=host.attachShadow({mode:'open',slotAssignment:'manual'});root.innerHTML='<slot dir=auto></slot>';const slot=root.querySelector('slot');
if(!slot.matches(':dir(ltr)')||slot.assignedNodes().length!==0)return 'initial';
slot.assign(container);if(!slot.matches(':dir(rtl)')||slot.assignedNodes()[0]!==container||container.assignedSlot!==slot)return 'assigned';
container.remove();if(slot.assignedNodes().length!==0||container.assignedSlot!==null||!slot.matches(':dir(ltr)'))return 'detached';
host.append(container);if(slot.assignedNodes()[0]!==container||container.assignedSlot!==slot||!slot.matches(':dir(rtl)'))return 'reattached';
slot.assign();if(!slot.matches(':dir(ltr)')||slot.assignedNodes().length!==0||container.assignedSlot!==null)return 'cleared';
let invalid='';try{slot.assign(document)}catch(e){invalid=e.name}if(invalid!=='TypeError')return 'invalid-node-'+invalid;
let brand='';try{HTMLSlotElement.prototype.assign.call(host,container)}catch(e){brand=e.name}if(brand!=='TypeError')return 'brand-'+brand;
const namedHost=document.createElement('div');document.body.append(namedHost);const named=namedHost.attachShadow({mode:'open'});named.innerHTML='<slot></slot>';
let disallowed='';try{named.querySelector('slot').assign(container)}catch(e){disallowed=e.name}if(disallowed!=='NotAllowedError')return 'named-mode-'+disallowed;
return 'ok';}catch(e){return e.name+': '+e.message;}})()"#,
        ),
        "ok"
    );
}

#[test]
fn dir_auto_follows_shadow_host_and_assigned_slot_text() {
    let mut runtime = runtime();
    assert_eq!(
        string(
            &mut runtime,
            r#"(()=>{try{
const host=document.createElement('div');host.dir='rtl';const light=document.createTextNode('ABC');host.append(light);const root=host.attachShadow({mode:'open'});root.innerHTML='<p id=inside></p><slot dir=auto></slot>';document.body.append(host);
const inside=root.querySelector('#inside'),slot=root.querySelector('slot');
if(!inside.matches(':dir(rtl)'))return 'shadow-parent';
if(!slot.matches(':dir(ltr)'))return 'slot-ltr';
light.data='א';if(!slot.matches(':dir(rtl)'))return 'slot-rtl';
const auto=document.createElement('div');auto.dir='auto';auto.attachShadow({mode:'open'}).innerHTML='<span>א</span>';auto.append(document.createElement('slot'));document.body.append(auto);
if(!auto.matches(':dir(ltr)'))return 'light-only';
return 'ok';}catch(e){return e.name+': '+e.message;}})()"#,
        ),
        "ok"
    );
}

#[test]
fn html_cdata_bogus_comment_does_not_consume_following_elements() {
    let document = TreeBuilder::parse(
        r#"<!doctype html><body><div dir=auto><![CDATA[foo]]>اختبر</div><iframe src="child.xhtml"></iframe><script>globalThis.after=true</script></body>"#,
    )
    .document();
    assert!(document.query_selector("iframe").is_some());
    assert!(document.query_selector("script").is_some());
}
