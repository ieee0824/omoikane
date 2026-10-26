//! Autofill selectors follow browser-owned fill state until accepted user edits.
use omoikane::{html::TreeBuilder, js::JsRuntime};

fn runtime(markup: &str) -> JsRuntime {
    JsRuntime::with_document(TreeBuilder::parse(markup).document()).unwrap()
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
fn autofill_grammar_alias_and_element_scope_match_the_standard() {
    let mut runtime = runtime("<head></head><body><input><textarea></textarea></body>");
    assert_eq!(
        string(
            &mut runtime,
            r#"(()=>{try{
if(!CSS.supports('selector(:autofill)')||!CSS.supports('selector(:-webkit-autofill)'))return 'support';
for(const selector of [':autofill()',':autofill(x)',':-webkit-autofill()']){
 if(CSS.supports('selector('+selector+')'))return 'functional';
 try{document.querySelector(selector);return 'query'}catch(e){if(e.name!=='SyntaxError')return e.name;}
}
if(document.querySelector('input').matches(':autofill, :-webkit-autofill'))return 'initial';
if(document.querySelector('textarea').matches(':autofill, :-webkit-autofill'))return 'textarea';
const foreign=document.createElementNS('urn:test','input');document.body.append(foreign);
if(foreign.matches(':autofill, :-webkit-autofill'))return 'foreign';
return typeof __omoikane_set_autofilled==='undefined'?'ok':'binding';
}catch(e){return e.name+': '+e.message;}})()"#,
        ),
        "ok"
    );
}

#[test]
fn host_autofill_sets_value_and_invalidates_relational_styles() {
    let mut runtime = runtime(
        "<head><style>section{width:10px}section:has(input:autofill){width:20px}input+span{height:10px}input:-webkit-autofill+span{height:30px}</style></head><body><section><input id=field><span></span></section></body>",
    );
    let input = runtime.document().query_selector("#field").unwrap();
    assert!(runtime.autofill_input(&input, "browser value").unwrap());
    assert_eq!(
        string(
            &mut runtime,
            r#"(()=>{const input=document.querySelector('input'),span=document.querySelector('span'),section=document.querySelector('section');return input.value==='browser value'&&input.matches(':autofill')&&input.matches(':-webkit-autofill')&&getComputedStyle(section).width==='20px'&&getComputedStyle(span).height==='30px'?'ok':'stale'})()"#,
        ),
        "ok"
    );

    // Script changes are not user edits and therefore do not clear the state.
    assert_eq!(
        string(
            &mut runtime,
            "const field=document.querySelector('input');field.value='scripted';field.matches(':autofill')?'ok':'cleared'",
        ),
        "ok"
    );
}

#[test]
fn accepted_user_edit_clears_autofill_but_canceled_edit_does_not() {
    let mut runtime = runtime("<head></head><body><input id=field></body>");
    let input = runtime.document().query_selector("#field").unwrap();
    assert!(runtime.autofill_input(&input, "filled").unwrap());
    assert_eq!(
        string(
            &mut runtime,
            r#"(()=>{const input=document.querySelector('input');input.focus();
const cancel=e=>e.preventDefault();input.addEventListener('beforeinput',cancel,{once:true});
__omoikane_dispatch_keyboard_input('keydown',{key:'x',text:'x'});
if(input.value!=='filled'||!input.matches(':autofill'))return 'canceled';
__omoikane_dispatch_keyboard_input('keydown',{key:'y',text:'y'});
return input.value==='filledy'&&!input.matches(':autofill')?'ok':'accepted';})()"#,
        ),
        "ok"
    );
}

#[test]
fn autofill_state_is_not_copied_or_available_for_non_inputs() {
    let mut runtime = runtime("<head></head><body><input id=field><div id=other></div></body>");
    let input = runtime.document().query_selector("#field").unwrap();
    let other = runtime.document().query_selector("#other").unwrap();
    assert!(runtime.autofill_input(&input, "filled").unwrap());
    assert!(!runtime.autofill_input(&other, "ignored").unwrap());
    assert_eq!(
        string(
            &mut runtime,
            "const clone=document.querySelector('input').cloneNode();clone.value==='filled'&&!clone.matches(':autofill')?'ok':'copied'",
        ),
        "ok"
    );
}
