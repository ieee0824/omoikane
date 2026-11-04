//! Contracts for per-invocation WPT input capabilities.
use super::automation_input::InputScope;
use super::*;

fn runtime() -> JsRuntime {
    let mut runtime =
        JsRuntime::with_document_and_url(default_document(), "https://input.example.test/")
            .unwrap();
    runtime.enable_test_automation().unwrap();
    runtime.eval(r#"
      globalThis.deliveries = [];
      document.body.onkeydown = e => deliveries.push(e.key);
      globalThis.keyData = key => ({key,text:'',repeat:false,shiftKey:false,ctrlKey:false,altKey:false,metaKey:false});
      globalThis.mouseData = () => ({clientX:0,clientY:0,pageX:0,pageY:0,screenX:0,screenY:0,movementX:0,movementY:0,buttons:0,button:0,pointerId:1});
    "#).unwrap();
    runtime
}

fn assert_script(runtime: &mut JsRuntime, source: &str) {
    let value = runtime.eval(source).unwrap();
    assert_eq!(value.as_boolean(), Some(true), "{}", value.display());
}

#[test]
fn ordinary_runtime_cannot_create_automation_input_scope() {
    let mut runtime = JsRuntime::new().unwrap();
    let document = runtime.document().identity();
    assert!(InputScope::new(&mut runtime, document).is_err());
    assert_script(
        &mut runtime,
        "typeof automationInput === 'undefined' && typeof __omoikane_automation_register_driver === 'undefined'",
    );
}

#[test]
fn execution_uses_polled_driver_snapshot_and_revokes_before_jobs_and_gc() {
    let mut runtime = runtime();
    runtime.eval(r#"
      globalThis.pollArgs = []; globalThis.executed = []; globalThis.expired = false;
      __omoikane_automation_register_driver(function(execute, input) {
        if (!execute) {
          pollArgs.push([arguments.length, typeof input]);
          Promise.resolve().then(() => __omoikane_automation_register_driver(() => { executed.push('replacement'); return null; }));
          return true;
        }
        executed.push('original'); globalThis.savedInput = input;
        input('keyboard', ['keydown', keyData('a')]);
        Promise.resolve().then(() => {
          try { input('keyboard', ['keydown', keyData('late')]); }
          catch (error) { expired = String(error).includes('expired'); }
        });
        return 7;
      });
    "#).unwrap();
    assert_eq!(runtime.advance_test_automation().unwrap(), Some(7));
    assert_script(
        &mut runtime,
        "JSON.stringify(pollArgs)==='[[1,\"undefined\"]]' && executed.join()==='original' && deliveries.join()==='a' && expired && typeof automationInput==='undefined'",
    );
    runtime.context.clear_kept_objects();
    boa_gc::force_collect();
    assert_script(
        &mut runtime,
        "try { savedInput('keyboard',['keydown',keyData('late')]); false; } catch(error) { String(error).includes('expired') && deliveries.join()==='a'; }",
    );
    assert_eq!(runtime.advance_test_automation().unwrap(), None);
    assert_script(&mut runtime, "executed.join()==='original,replacement'");
}

#[test]
fn throwing_driver_revokes_retained_capability() {
    let mut runtime = runtime();
    runtime.eval("__omoikane_automation_register_driver((execute,input)=>{if(!execute)return true;globalThis.savedInput=input;throw new Error('driver failed');});").unwrap();
    assert!(runtime.advance_test_automation().is_err());
    runtime.context.clear_kept_objects();
    boa_gc::force_collect();
    assert_script(
        &mut runtime,
        "try { savedInput('keyboard',['keydown',keyData('late')]); false; } catch(error) { String(error).includes('expired') && deliveries.length===0; }",
    );
}

#[test]
fn dictionary_getters_are_read_once_without_borrow_and_reentry_is_rejected() {
    let mut runtime = runtime();
    let document = runtime.document().identity();
    let scope = InputScope::new(&mut runtime, document).unwrap();
    let result = runtime.call_function_with_value(r#"input => {
      let getterCalls=0, rejected=0, delivered=0, codeUnit=0;
      const data=keyData('a');
      Object.defineProperty(data,'key',{get(){
        getterCalls++; document.body.setAttribute('data-getter','called');
        try { input('keyboard',['keydown',keyData('nested')]); }
        catch(error){if(String(error).includes('reentrant'))rejected++;}
        return '\ud800';
      }});
      document.body.onkeydown=e=>{
        delivered++; codeUnit=e.key.charCodeAt(0);
        if(!e.isTrusted)throw new Error('untrusted');
        try { input('keyboard',['keydown',keyData('nested')]); }
        catch(error){if(String(error).includes('reentrant'))rejected++;}
      };
      input('keyboard',['keydown',data]);
      return getterCalls===1 && rejected===2 && delivered===1 && codeUnit===0xd800 && document.body.dataset.getter==='called';
    }"#, scope.value()).unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn malformed_input_is_rejected_before_delivery_and_scope_remains_usable() {
    let mut runtime = runtime();
    let document = runtime.document().identity();
    let scope = InputScope::new(&mut runtime, document).unwrap();
    let result = runtime.call_function_with_value(r#"input => {
      const detached=document.createElement('button');
      const frame=document.createElement('iframe');document.body.append(frame);
      const foreign=frame.contentDocument.body;
      const calls=[
        ()=>input('touch',[]),
        ()=>input('keyboard',['keypress',keyData('x')]),
        ()=>input('keyboard',['keydown',{...keyData('x'),repeat:'false'}]),
        ()=>input('mouse',[document.body.__id,'mousemove',{...mouseData(),clientX:NaN},false]),
        ()=>input('mouse',[document.body.__id,'mousemove',{...mouseData(),button:5},false]),
        ()=>input('mouse',[document.body.__id,'mousemove',{...mouseData(),pointerId:0},false]),
        ()=>input('mouse',[document.body.__id,'mousedown',mouseData(),false]),
        ()=>input('mouse',[detached.__id,'mousemove',mouseData(),false]),
        ()=>input('mouse',[foreign.__id,'mousemove',mouseData(),false]),
      ];
      let rejected=0;
      for(const call of calls){try{call();}catch(error){if(error instanceof TypeError)rejected++;}}
      if(rejected!==calls.length || deliveries.length!==0 || navigator.userActivation.hasBeenActive)return false;
      input('keyboard',['keydown',keyData('ok')]);
      return deliveries.join()==='ok';
    }"#, scope.value()).unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn capability_rejects_other_runtime_and_author_document() {
    let mut owner = runtime();
    let document = owner.document().identity();
    let scope = InputScope::new(&mut owner, document).unwrap();
    let mut other = runtime();
    let caught = "input => {try{input('keyboard',['keydown',keyData('x')]);return 'unexpected';}catch(error){return String(error);}}";
    let value = other
        .call_function_with_value(caught, scope.value())
        .unwrap();
    assert!(
        value
            .as_string()
            .unwrap()
            .to_std_string_escaped()
            .contains("runtime mismatch")
    );
    owner.eval("globalThis.frame=document.createElement('iframe');document.body.append(frame);frame.contentDocument;").unwrap();
    let frame = owner.document().query_selector("iframe").unwrap();
    let child = owner.frame_content_document(&frame).unwrap();
    let realm = owner.realm_for_document(child.identity()).unwrap().unwrap();
    let previous = owner.context.enter_realm(realm);
    let value = owner.call_function_with_value("input=>{try{input('keyboard',[]);return 'unexpected';}catch(error){return String(error);}}", scope.value()).unwrap();
    owner.context.enter_realm(previous);
    assert!(
        value
            .as_string()
            .unwrap()
            .to_std_string_escaped()
            .contains("author Document mismatch")
    );
    assert_script(&mut owner, "deliveries.length===0");
}

#[test]
fn navigation_during_dictionary_getter_invalidates_entry_before_dispatch() {
    let mut runtime = runtime();
    runtime.eval("globalThis.frame=document.createElement('iframe');document.body.append(frame);frame.contentDocument.body.onkeydown=e=>deliveries.push(e.key);frame.focus();").unwrap();
    let frame = runtime.document().query_selector("iframe").unwrap();
    let child = runtime.frame_content_document(&frame).unwrap();
    let realm = runtime
        .realm_for_document(child.identity())
        .unwrap()
        .unwrap();
    let previous = runtime.context.enter_realm(realm);
    let scope = InputScope::new(&mut runtime, child.identity()).unwrap();
    let result = runtime.call_function_with_value(r#"input => {
      const data={key:'before',text:'',repeat:false,shiftKey:false,ctrlKey:false,altKey:false,metaKey:false};
      input('keyboard',['keydown',data]);
      Object.defineProperty(data,'key',{get(){
        parent.frame.srcdoc='<p>replacement</p>'; void parent.frame.contentDocument; return 'after';
      }});
      try{input('keyboard',['keydown',data]);return 'unexpected';}catch(error){return String(error);}
    }"#, scope.value()).unwrap();
    runtime.context.enter_realm(previous);
    assert!(
        result
            .as_string()
            .unwrap()
            .to_std_string_escaped()
            .contains("Document is inactive")
    );
    assert!(
        !runtime
            .host_state
            .borrow()
            .document_is_active(child.identity())
    );
    assert_script(&mut runtime, "deliveries.join()==='before'");
}

#[test]
fn throwing_dictionary_getter_releases_busy_guard_for_same_scope() {
    let mut runtime = runtime();
    let document = runtime.document().identity();
    let scope = InputScope::new(&mut runtime, document).unwrap();
    let result = runtime.call_function_with_value(r#"input => {
      const data=keyData('bad');
      Object.defineProperty(data,'key',{get(){throw new Error('getter failed');}});
      let failed=false;
      try{input('keyboard',['keydown',data]);}catch(error){failed=String(error).includes('getter failed');}
      if(!failed || deliveries.length!==0)return false;
      input('keyboard',['keydown',keyData('after')]);
      return deliveries.join()==='after';
    }"#, scope.value()).unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}
