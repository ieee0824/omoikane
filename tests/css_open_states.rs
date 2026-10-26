//! Open selectors follow semantic disclosure and actual picker state.
use omoikane::{cdp::CdpSession, html::TreeBuilder, js::JsRuntime};
use serde_json::json;

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
fn disclosures_match_semantic_open_state_and_reject_foreign_and_popover_states() {
    assert_eq!(
        evaluate(
            r#"(()=>{try{
if(!CSS.supports('selector(:open)'))return 'support';
const dialog=document.createElement('dialog'),details=document.createElement('details');document.body.append(dialog,details);
if(dialog.matches(':open')||details.matches(':open'))return 'initial';
dialog.show();if(!dialog.matches(':open'))return 'show';dialog.close();
dialog.showModal();if(!dialog.matches(':open')||!dialog.matches(':modal'))return 'modal';dialog.close();
let cancelSeen=false;const cancel=e=>{cancelSeen=true;e.preventDefault();};dialog.addEventListener('cancel',cancel);dialog.showModal();dialog.requestClose('blocked');
if(!cancelSeen||!dialog.open||dialog.returnValue==='blocked')return 'request-close-cancel';dialog.removeEventListener('cancel',cancel);dialog.requestClose('accepted');
if(dialog.open||dialog.matches(':modal')||dialog.returnValue!=='accepted')return 'request-close';
details.setAttribute('open','false');details.style.display='none';
if(!details.open||!details.matches(':open'))return 'semantic-not-rendered';
details.open=false;if(details.matches(':open'))return 'close';
const foreign=document.createElementNS('http://www.w3.org/2000/svg','details');foreign.setAttribute('open','');document.body.append(foreign);
const arbitrary=document.createElement('div');arbitrary.setAttribute('open','');arbitrary.popover='manual';document.body.append(arbitrary);arbitrary.showPopover();
if(foreign.matches(':open')||arbitrary.matches(':open')||!arbitrary.matches(':popover-open'))return 'non-disclosures';
for(const selector of [':open()',':open(x)',':closed']){
 if(CSS.supports('selector('+selector+')'))return 'grammar';
 try{document.querySelector(selector);return 'query';}catch(e){if(e.name!=='SyntaxError')return 'query-error';}
}
return 'ok';}catch(e){return e.name+': '+e.message;}})()"#
        ),
        "ok"
    );
}

#[test]
fn disclosure_changes_invalidate_has_siblings_and_named_group_styles() {
    assert_eq!(
        evaluate(
            r#"(()=>{try{
if(!CSS.supports('selector(:open)'))return 'support';
const style=document.createElement('style');style.textContent='section{width:10px}section:has(details:open){width:20px}span{height:10px}details:open+span{height:30px}';document.head.append(style);
const section=document.createElement('section');section.innerHTML='<details name="group"><summary>One</summary></details><span></span><details name="group"><summary>Two</summary></details>';document.body.append(section);
const [first,second]=section.querySelectorAll('details'),span=section.querySelector('span');
if(getComputedStyle(section).width!=='10px'||getComputedStyle(span).height!=='10px')return 'initial-style';
first.open=true;if(!first.matches(':open')||getComputedStyle(section).width!=='20px'||getComputedStyle(span).height!=='30px')return 'open-style';
second.open=true;if(first.open||first.matches(':open')||!second.matches(':open')||getComputedStyle(span).height!=='10px')return 'group-style';
second.open=false;if(getComputedStyle(section).width!=='10px')return 'close-style';
first.querySelector('summary').click();if(!first.open||!first.matches(':open'))return 'summary-activation';
first.querySelector('summary').click();if(first.matches(':open'))return 'summary-close';
return 'ok';}catch(e){return e.name+': '+e.message;}})()"#
        ),
        "ok"
    );
}

fn page_value(session: &mut CdpSession, expression: &str) -> serde_json::Value {
    let response = session
        .dispatch(
            "Runtime.evaluate",
            json!({"expression":expression,"returnByValue":true}),
        )
        .unwrap();
    assert!(response.get("exceptionDetails").is_none(), "{response}");
    response["result"]["value"].clone()
}

#[test]
fn actual_select_picker_opens_commits_and_closes_before_notifications() {
    let html = "<style>body{margin:0}select{position:absolute;left:0;width:120px;height:24px}select:open{width:140px}#first{top:0}#second{top:30px}button{position:absolute;top:60px;left:0;width:80px;height:24px}</style><select id=first><option value=a>A</option><option value=b>B</option></select><select id=second><option>C</option></select><button>outside</button>";
    let encoded = html
        .bytes()
        .map(|byte| format!("%{byte:02X}"))
        .collect::<String>();
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch(
            "Page.navigate",
            json!({"url":format!("data:text/html,{encoded}")}),
        )
        .unwrap();
    assert_eq!(
        page_value(&mut session, "CSS.supports('selector(:open)')"),
        true
    );
    assert_eq!(
        page_value(
            &mut session,
            "document.querySelector('select').matches(':open')"
        ),
        false
    );
    assert_eq!(
        page_value(
            &mut session,
            "(()=>{const s=document.querySelector('select');s.disabled=true;try{s.showPicker();return 'disabled-missed'}catch(e){if(e.name!=='InvalidStateError')return e.name}s.disabled=false;try{s.showPicker();return 'activation-missed'}catch(e){return e.name==='NotAllowedError'}})()"
        ),
        true
    );
    let click = |session: &mut CdpSession, y: u32| {
        for kind in ["mousePressed", "mouseReleased"] {
            session
                .dispatch(
                    "Input.dispatchMouseEvent",
                    json!({"type":kind,"x":5,"y":y,"button":"left","clickCount":1}),
                )
                .unwrap();
        }
    };
    click(&mut session, 5);
    assert_eq!(
        page_value(
            &mut session,
            "document.querySelector('select').matches(':open') && getComputedStyle(document.querySelector('select')).width==='140px'"
        ),
        true
    );
    click(&mut session, 35);
    assert_eq!(
        page_value(
            &mut session,
            "!document.querySelector('#first').matches(':open') && document.querySelector('#second').matches(':open')"
        ),
        true
    );
    click(&mut session, 65);
    assert_eq!(
        page_value(
            &mut session,
            "!document.querySelector('#second').matches(':open')"
        ),
        true
    );
    click(&mut session, 5);
    session
        .dispatch(
            "Input.dispatchKeyEvent",
            json!({"type":"keyDown","key":"Escape"}),
        )
        .unwrap();
    assert_eq!(
        page_value(
            &mut session,
            "document.querySelector('select').matches(':open')"
        ),
        false
    );
    page_value(
        &mut session,
        "var pickerChangedClosed=false; document.querySelector('select').addEventListener('change',e=>pickerChangedClosed=!e.target.matches(':open'));true",
    );
    click(&mut session, 5);
    for key in ["ArrowDown", "Enter"] {
        session
            .dispatch(
                "Input.dispatchKeyEvent",
                json!({"type":"keyDown","key":key}),
            )
            .unwrap();
    }
    assert_eq!(
        page_value(
            &mut session,
            "document.querySelector('select').value==='b' && !document.querySelector('select').matches(':open') && pickerChangedClosed"
        ),
        true
    );
    click(&mut session, 5);
    assert_eq!(
        page_value(
            &mut session,
            "document.querySelector('#first').matches(':open')"
        ),
        true
    );
    assert_eq!(
        page_value(
            &mut session,
            "globalThis.detachedPicker=document.querySelector('#first');globalThis.clonedPicker=detachedPicker.cloneNode(true);!clonedPicker.matches(':open')"
        ),
        true
    );
    assert_eq!(
        page_value(
            &mut session,
            "detachedPicker.remove();!detachedPicker.matches(':open')"
        ),
        true
    );
    assert_eq!(
        page_value(
            &mut session,
            "typeof __omoikane_get_picker_open==='undefined' && typeof __omoikane_set_picker_open==='undefined'"
        ),
        true
    );
}
