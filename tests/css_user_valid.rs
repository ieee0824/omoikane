//! User validity follows submission, reset, ownership and real interaction state.
use omoikane::{html::TreeBuilder, js::JsRuntime};

fn evaluate(script: &str) -> String {
    let mut runtime =
        JsRuntime::with_document(TreeBuilder::parse("<head></head><body></body>").document())
            .unwrap();
    runtime
        .eval(script)
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped()
}

#[test]
fn submission_and_reset_update_user_validity_before_author_events() {
    assert_eq!(
        evaluate(
            r#"(()=>{try{
if(!CSS.supports('selector(:user-valid)')||!CSS.supports('selector(:user-invalid)'))return 'support';
const form=document.createElement('form');form.innerHTML='<fieldset><input id="invalid" required><input id="valid"><input id="readonly" readonly><input id="disabled" disabled><select id="select" required><option value="">Empty</option><option value="ok">OK</option></select><textarea id="textarea" required></textarea><button id="submit" type="submit">Submit</button></fieldset>';
document.body.append(form);const invalid=form.querySelector('#invalid'),valid=form.querySelector('#valid');
const controls=[...form.querySelectorAll('input,textarea,select')];
const neither=e=>!e.matches(':user-valid')&&!e.matches(':user-invalid');
if(controls.some(e=>!neither(e)))return 'initial';
invalid.value='x';invalid.dispatchEvent(new InputEvent('input',{bubbles:true}));invalid.dispatchEvent(new Event('change',{bubbles:true}));
if(!neither(invalid))return 'synthetic';invalid.value='';
form.checkValidity();if(controls.some(e=>!neither(e)))return 'checkValidity';
let invalidSawFlag=false,submitSawFlag=false;
invalid.addEventListener('invalid',()=>{invalidSawFlag=invalid.matches(':user-invalid');});
form.addEventListener('submit',e=>{e.preventDefault();submitSawFlag=invalid.matches(':user-invalid')&&valid.matches(':user-valid');});
form.requestSubmit();
if(!invalidSawFlag||!invalid.matches(':user-invalid')||!valid.matches(':user-valid')||!form.querySelector('#select').matches(':user-invalid')||!form.querySelector('#textarea').matches(':user-invalid'))return 'submission';
for(const e of [form,form.firstElementChild,form.querySelector('#readonly'),form.querySelector('#disabled'),form.querySelector('#submit')])if(!neither(e))return 'candidate';
invalid.value='valid';if(!invalid.matches(':user-valid')||invalid.matches(':user-invalid'))return 'reactive-validity';
invalid.disabled=true;if(!neither(invalid))return 'disabled';invalid.disabled=false;if(!invalid.matches(':user-valid'))return 'restored';
form.reset();if(controls.some(e=>!neither(e)))return 'reset';
form.noValidate=true;form.requestSubmit();if(!submitSawFlag||!invalid.matches(':user-invalid'))return 'novalidate';
const cancel=e=>e.preventDefault();form.addEventListener('reset',cancel);form.reset();if(!invalid.matches(':user-invalid'))return 'cancelled-reset';
form.removeEventListener('reset',cancel);form.reset();if(controls.some(e=>!neither(e)))return 'second-reset';
return 'ok';}catch(e){return e.name+': '+e.message;}})()
"#
        ),
        "ok"
    );
}

#[test]
fn user_validity_follows_form_ownership_and_adoption_but_not_node_copy() {
    assert_eq!(
        evaluate(
            r#"(()=>{try{
if(!CSS.supports('selector(:user-valid)')||!CSS.supports('selector(:user-invalid)'))return 'support';
const form=document.createElement('form');form.id='owned-validity';form.innerHTML='<fieldset disabled><legend><input id="legend-validity" required></legend><legend><input id="later-validity" required></legend><input id="disabled-validity" required></fieldset>';
const external=document.createElement('input');external.required=true;external.setAttribute('form',form.id);document.body.append(form,external);
const neither=e=>!e.matches(':user-valid')&&!e.matches(':user-invalid');
form.requestSubmit();
if(!external.matches(':user-invalid')||!form.querySelector('#legend-validity').matches(':user-invalid'))return 'owned-submission';
for(const id of ['later-validity','disabled-validity'])if(!neither(form.querySelector('#'+id)))return 'disabled-eligibility';
form.firstElementChild.disabled=false;
for(const id of ['later-validity','disabled-validity'])if(!form.querySelector('#'+id).matches(':user-invalid'))return 'retained-disabled-flag';
for(const copy of [external.cloneNode(true),document.importNode(external,true)]){
 copy.removeAttribute('form');document.body.append(copy);
 if(!copy.matches(':invalid')||!neither(copy))return 'copy-flag';
}

const frame=document.createElement('iframe');document.body.append(frame);const view=frame.contentWindow,other=frame.contentDocument;
other.body.append(other.adoptNode(external));
if(external.ownerDocument!==other||!external.matches(':user-invalid'))return 'adopt-flag';
external.value='filled';if(!external.matches(':user-valid')||external.matches(':user-invalid'))return 'adopt-reactivity';
document.body.append(document.adoptNode(external));frame.remove();
if(!external.matches(':user-valid'))return 'returned-flag';
form.reset();if(!external.matches(':invalid')||!neither(external))return 'external-reset';
return 'ok';}catch(e){return e.name+': '+e.message;}})()"#
        ),
        "ok"
    );
}

#[test]
fn host_input_commits_history_before_change_without_trusting_script_fields() {
    assert_eq!(
        evaluate(
            r#"(()=>{try{
const input=document.createElement('input');input.required=true;
const checkbox=document.createElement('input');checkbox.type='checkbox';checkbox.required=true;
document.body.append(input,checkbox);
const neither=e=>!e.matches(':user-valid')&&!e.matches(':user-invalid');
input.focus();input.value='scripted';input.__textEditChanged=true;input.__focusValue='';input.blur();
if(!neither(input))return 'forged-history';
input.value='';input.focus();let changeSaw=false;
input.addEventListener('change',()=>{changeSaw=input.matches(':user-valid');});
__omoikane_dispatch_keyboard_input('keydown',{key:'a',text:'a'});
if(input.value!=='a'||!neither(input))return 'uncommitted-input';
input.__textEditChanged=false;input.__focusValue=input.value;input.blur();
if(!input.matches(':user-valid')||!changeSaw)return 'committed-input';
input.value='';if(!input.matches(':user-invalid'))return 'reactive-input';
let checkboxSaw=false;checkbox.addEventListener('input',()=>{checkboxSaw=checkbox.matches(':user-valid');});
__omoikane_dispatch_mouse_input(checkbox.__id,'click',{button:0,buttons:0},false);
if(!checkbox.checked||!checkboxSaw||!checkbox.matches(':user-valid'))return 'checkable-input';
const cancelled=document.createElement('input');cancelled.type='checkbox';document.body.append(cancelled);
cancelled.addEventListener('click',e=>e.preventDefault());
__omoikane_dispatch_mouse_input(cancelled.__id,'click',{button:0,buttons:0},false);
if(cancelled.checked||!neither(cancelled))return 'cancelled-activation';
return 'ok';}catch(e){return e.name+': '+e.message;}})()"#
        ),
        "ok"
    );
}

#[test]
fn cancelled_edits_and_focused_reset_do_not_commit_stale_history() {
    assert_eq!(
        evaluate(
            r#"(()=>{try{
const form=document.createElement('form');form.innerHTML='<input required value="default">';document.body.append(form);
const input=form.firstElementChild;const neither=()=>!input.matches(':user-valid')&&!input.matches(':user-invalid');
input.focus();const cancel=e=>e.preventDefault();input.addEventListener('beforeinput',cancel);
__omoikane_dispatch_keyboard_input('keydown',{key:'a',text:'a'});input.blur();
if(input.value!=='default'||!neither())return 'cancelled-beforeinput';
input.removeEventListener('beforeinput',cancel);input.focus();
__omoikane_dispatch_keyboard_input('keydown',{key:'a',text:'a'});form.reset();input.value='scripted';input.blur();
if(!neither())return 'reset-pending-history';
input.focus();__omoikane_dispatch_keyboard_input('keydown',{key:'b',text:'b'});input.blur();
if(!input.matches(':user-valid'))return 'next-edit';
if(typeof globalThis.__omoikane_set_user_validity!=='undefined')return 'private-binding';
return 'ok';}catch(e){return e.name+': '+e.message;}})()"#
        ),
        "ok"
    );
}

#[test]
fn editing_back_to_the_initial_value_still_commits_user_validity() {
    assert_eq!(
        evaluate(
            r#"(()=>{try{
for(const tag of ['input','textarea']){
 const control=document.createElement(tag);control.required=true;document.body.append(control);control.focus();
 let changes=0;control.addEventListener('change',()=>changes++);
 __omoikane_dispatch_keyboard_input('keydown',{key:'a',text:'a'});
 __omoikane_dispatch_keyboard_input('keydown',{key:'Backspace'});
 if(control.value!==''||control.matches(':user-invalid'))return 'before-commit';
 control.blur();if(!control.matches(':user-invalid')||changes!==0)return tag+'-commit';
}
return 'ok';}catch(e){return e.name+': '+e.message;}})()"#
        ),
        "ok"
    );
}

#[test]
fn temporal_input_keeps_partial_components_private_and_commits_complete_values() {
    assert_eq!(
        evaluate(
            r#"(()=>{try{
const key=k=>__omoikane_dispatch_keyboard_input('keydown',{key:k,text:k.length===1?k:''});
for(const [type,keys,value] of [
 ['date',['1','Tab','2','Tab','2','0','2','4'],'2024-01-02'],
 ['time',['1','Tab','2','Tab','p'],'13:02'],
 ['datetime-local',['1','Tab','2','Tab','2','0','2','4','Tab','1','Tab','2','Tab','a'],'2024-01-02T01:02']
]){
 const input=document.createElement('input');input.type=type;input.required=true;document.body.append(input);input.focus();
 key(keys[0]);if(input.value!==''||!input.validity.badInput||input.matches(':user-invalid'))return type+'-partial';
 for(const k of keys.slice(1))key(k);
 if(input.value!==value||input.validity.badInput||!input.matches(':user-valid'))return type+'-complete';
 input.blur();input.focus();key('Backspace');
 if(input.value!==''||!input.validity.badInput||!input.matches(':user-invalid'))return type+'-delete';
 input.value=value;if(input.validity.badInput||!input.matches(':user-valid'))return type+'-script-reset';
 input.remove();
}
const input=document.createElement('input');input.type='date';document.body.append(input);input.focus();
input.dispatchEvent(new KeyboardEvent('keydown',{key:'1',bubbles:true}));
if(input.value!==''||input.validity.badInput)return 'synthetic-key';
const cancel=e=>e.preventDefault();input.addEventListener('keydown',cancel);key('1');
if(input.value!==''||input.validity.badInput)return 'cancelled-key';
input.removeEventListener('keydown',cancel);input.readOnly=true;key('1');
if(input.value!==''||input.validity.badInput)return 'readonly';
return 'ok';}catch(e){return e.name+': '+e.message;}})()"#
        ),
        "ok"
    );
}

#[test]
fn original_submission_reftest_runs_pending_animation_and_matches_pixels() {
    use omoikane::{
        cdp::CdpSession,
        frame::{paint_browser_frame, render_browser_frame},
    };
    use serde_json::json;
    let root =
        std::path::PathBuf::from(std::env::var("WPT_ROOT").unwrap_or_else(|_| "target/wpt".into()));
    let source = root.join("css/selectors/user-invalid-form-submission-invalidation.html");
    if !source.is_file() {
        assert_ne!(
            std::env::var("WPT_REQUIRED").as_deref(),
            Ok("1"),
            "WPT required"
        );
        eprintln!("WPT checkout missing; original submission reftest skipped");
        return;
    }
    let page = |path: std::path::PathBuf| {
        let original = std::fs::read(path).unwrap();
        let encoded = original
            .iter()
            .map(|byte| format!("%{byte:02X}"))
            .collect::<String>();
        let mut session = CdpSession::new().unwrap();
        session.set_viewport(800, 600);
        session
            .dispatch(
                "Page.navigate",
                json!({"url":format!("data:text/html,{encoded}")}),
            )
            .unwrap();
        session
    };
    let mut session = page(source);
    let before = paint_browser_frame(&mut session).unwrap();
    assert_eq!((before.width(), before.height()), (800, 600));
    let actual = render_browser_frame(&mut session, 800, 600, 16).unwrap();
    let result=session.dispatch("Runtime.evaluate",json!({"expression":"!document.documentElement.classList.contains('reftest-wait') && document.querySelectorAll('input:user-invalid').length === 2 && document.querySelectorAll('input:user-valid').length === 1","returnByValue":true})).unwrap();
    assert_eq!(result["result"]["value"], true, "{result}");
    let mut reference =
        page(root.join("css/selectors/user-invalid-form-submission-invalidation-ref.html"));
    let expected = render_browser_frame(&mut reference, 800, 600, 16).unwrap();
    assert!(
        before.pixels() != actual.pixels(),
        "submission must visibly invalidate warm styles"
    );
    let differences = actual
        .pixels()
        .chunks_exact(4)
        .zip(expected.pixels().chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    eprintln!("user-invalid-form-submission-invalidation: {differences} differing pixels");
    assert_eq!(differences, 0, "original reference");
}

#[test]
fn nested_user_validity_selectors_invalidate_warm_styles_and_reject_function_syntax() {
    assert_eq!(
        evaluate(
            r#"(()=>{try{
const style=document.createElement('style');style.textContent='form{width:10px}form:has(input:user-invalid){width:20px}input{height:10px}input:nth-child(1 of :is(:user-valid,:user-invalid)){height:30px}';document.head.append(style);
const form=document.createElement('form');form.innerHTML='<input required><input>';document.body.append(form);
const input=form.firstElementChild;
if(getComputedStyle(form).width!=='10px'||getComputedStyle(input).height!=='10px')return 'initial';
form.requestSubmit();if(getComputedStyle(form).width!=='20px'||getComputedStyle(input).height!=='30px')return 'submission-style';
input.value='filled';if(getComputedStyle(form).width!=='10px'||getComputedStyle(input).height!=='30px')return 'value-style';
input.disabled=true;if(getComputedStyle(input).height!=='10px')return 'candidate-style';
input.disabled=false;form.reset();if(getComputedStyle(input).height!=='10px')return 'reset-style';
for(const selector of [':user-valid()',':user-invalid(x)']){
 if(CSS.supports('selector('+selector+')'))return 'support-function';
 try{document.querySelector(selector);return 'query-function';}catch(e){if(e.name!=='SyntaxError')return 'query-error';}
}
const foreign=document.createElementNS('http://www.w3.org/2000/svg','input');form.append(foreign);form.noValidate=true;form.addEventListener('submit',e=>e.preventDefault());form.requestSubmit();
if(foreign.matches(':user-valid')||foreign.matches(':user-invalid')||document.querySelector('form').matches(':user-valid'))return 'non-candidates';
return 'ok';}catch(e){return e.name+': '+e.message;}})()"#
        ),
        "ok"
    );
}

#[test]
fn select_keyboard_commits_user_validity_before_input_and_change() {
    assert_eq!(
        evaluate(
            r#"(()=>{try{
const select=document.createElement('select');select.required=true;select.innerHTML='<option value="">Empty</option><option disabled value="skip">Skip</option><optgroup disabled><option value="group">Group</option></optgroup><option value="ok">OK</option>';
document.body.append(select);select.focus();let seen=[];
for(const type of ['input','change'])select.addEventListener(type,()=>seen.push(type+':'+select.matches(':user-valid')));
select.value='ok';select.value='';select.dispatchEvent(new Event('change',{bubbles:true}));seen=[];
if(select.matches(':user-invalid'))return 'script-input';
__omoikane_dispatch_keyboard_input('keydown',{key:'ArrowDown'});
if(select.value!=='ok'||seen.join()!=='input:true,change:true')return 'keyboard-selection';
seen=[];__omoikane_dispatch_keyboard_input('keydown',{key:'Home'});
if(select.value!==''||!select.matches(':user-invalid')||seen.join()!=='input:false,change:false')return 'home-selection';
seen=[];const cancel=e=>e.preventDefault();select.addEventListener('keydown',cancel);
__omoikane_dispatch_keyboard_input('keydown',{key:'ArrowDown'});select.removeEventListener('keydown',cancel);
if(select.value!==''||seen.length)return 'cancelled-key';
select.disabled=true;__omoikane_dispatch_keyboard_input('keydown',{key:'ArrowDown'});
if(select.value!==''||seen.length)return 'disabled-key';
return 'ok';}catch(e){return e.name+': '+e.message;}})()"#
        ),
        "ok"
    );
}

#[test]
fn temporal_component_state_survives_incomplete_seconds_and_resets_on_script_values() {
    assert_eq!(
        evaluate(
            r#"(()=>{try{
const key=k=>__omoikane_dispatch_keyboard_input('keydown',{key:k,text:k.length===1?k:''});
for(const [type,keys,value] of [
 ['month',['2','Tab','2','0','2','4'],'2024-02'],
 ['week',['5','Tab','2','0','2','4'],'2024-W05']
]){
 const control=document.createElement('input');control.type=type;document.body.append(control);control.focus();
 for(const k of keys)key(k);
 if(control.value!==value||!control.matches(':user-valid'))return type+'-value';
 control.remove();
}
const control=document.createElement('input');control.type='time';control.value='01:02:03';document.body.append(control);control.focus();
key('Backspace');if(control.value!==''||!control.validity.badInput)return 'partial-seconds';
key('1');if(control.value!=='01:02:03')return 'retained-components';
key('ArrowUp');if(control.value!=='02:02:03')return 'arrow-step';
control.valueAsNumber=3723000;key('1');
if(control.value!=='01:02:03'||control.validity.badInput)return 'numeric-reset';
const form=document.createElement('form');form.innerHTML='<input required>';form.action='javascript:void(0)';document.body.append(form);form.submit();
if(form.firstElementChild.matches(':user-valid')||form.firstElementChild.matches(':user-invalid'))return 'submit-method';
return 'ok';}catch(e){return e.name+': '+e.message;}})()"#
        ),
        "ok"
    );
}

#[test]
fn adopted_user_validity_survives_originating_frame_retirement_and_collection() {
    let mut runtime =
        JsRuntime::with_document(TreeBuilder::parse("<body></body>").document()).unwrap();
    runtime
        .eval(
            r#"
      var frame=document.createElement('iframe');document.body.append(frame);
      var child=frame.contentDocument;child.body.innerHTML='<form><input required></form>';
      child.querySelector('form').requestSubmit();
      var retained=child.querySelector('input');document.body.append(document.adoptNode(retained));
      frame.remove();frame=null;child=null;
    "#,
        )
        .unwrap();
    boa_gc::force_collect();
    assert_eq!(
        runtime
            .eval(
                r#"
      retained.matches(':user-invalid') && !retained.cloneNode(true).matches(':user-invalid')
    "#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    runtime.eval("retained.value='filled'").unwrap();
    assert_eq!(
        runtime
            .eval("retained.matches(':user-valid') && !retained.matches(':user-invalid')")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn single_select_navigation_tracks_current_selection_after_scripts_and_reset() {
    assert_eq!(
        evaluate(
            r#"(()=>{try{
const form=document.createElement('form');form.innerHTML='<select><option value="a" selected>A</option><option value="b">B</option><option value="c">C</option></select>';document.body.append(form);
const select=form.firstElementChild;select.focus();const down=()=>__omoikane_dispatch_keyboard_input('keydown',{key:'ArrowDown'});
down();if(select.value!=='b')return 'initial-position';
select.value='a';down();if(select.value!=='b')return 'script-position';
select.selectedIndex=0;down();if(select.value!=='b')return 'index-position';
form.reset();if(select.matches(':user-valid')||select.matches(':user-invalid'))return 'reset-flag';
down();if(select.value!=='b')return 'reset-position';
select.options[0].selected=true;down();if(select.value!=='b')return 'option-position';
return 'ok';}catch(e){return e.name+': '+e.message;}})()"#
        ),
        "ok"
    );
}

#[test]
fn originating_user_validity_updates_direct_and_nested_pseudo_element_styles() {
    assert_eq!(
        evaluate(
            r#"(()=>{try{
const style=document.createElement('style');style.textContent="input::before{content:'x';color:blue}input:user-invalid::before{color:red}input:user-valid::before{color:green}input::after{content:'y';color:blue}input:is(:user-invalid)::after{color:red}";document.head.append(style);
const form=document.createElement('form');form.innerHTML='<input type="checkbox" required>';document.body.append(form);const control=form.firstElementChild;
if(getComputedStyle(control,'::before').color!=='rgb(0, 0, 255)')return 'initial-pseudo';
form.requestSubmit();if(!control.matches(':user-invalid'))return 'owner-state';
if(getComputedStyle(control,'::before').color!=='rgb(255, 0, 0)'||getComputedStyle(control,'::after').color!=='rgb(255, 0, 0)')return 'invalid-pseudo';
control.checked=true;
if(getComputedStyle(control,'::before').color!=='rgb(0, 128, 0)'||getComputedStyle(control,'::after').color!=='rgb(0, 0, 255)')return 'valid-pseudo';
form.reset();if(getComputedStyle(control,'::before').color!=='rgb(0, 0, 255)')return 'reset-pseudo';
return 'ok';}catch(e){return e.name+': '+e.message;}})()"#
        ),
        "ok"
    );
}

#[test]
fn rust_created_legacy_html_controls_acquire_submission_history() {
    use omoikane::dom::NodeHandle;
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let form = NodeHandle::element("form");
    let input = NodeHandle::element("input");
    input.set_attribute("required", "");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(form.clone());
    form.append_child(input);
    let mut runtime = JsRuntime::with_document(document).unwrap();
    assert_eq!(runtime.eval("document.querySelector('form').requestSubmit(); document.querySelector('input').matches(':user-invalid')").unwrap().as_boolean(),Some(true));
}
