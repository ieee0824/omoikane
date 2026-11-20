//! Same-origin auxiliary WindowProxy property existence regressions.
use omoikane::html::TreeBuilder;
use omoikane::http::{HttpParseError, HttpRequest, HttpResponse, HttpTransport};
use omoikane::js::JsRuntime;
use serde_json::{Value, json};
use std::{io::Cursor, sync::Arc, time::Duration};

// Columns: in, intrinsic Reflect.has, hasOwnProperty, own descriptor kind,
// typeof value, primitive value (undefined is represented by null plus its type).
// Builtins assert descriptor presence; author properties also assert its kind.
const SAMPLE: &str = r#"
const sampleHas = (() => {
  const reflectHas = Reflect.has;
  const own = Function.prototype.call.bind(Object.prototype.hasOwnProperty);
  const descriptor = Object.getOwnPropertyDescriptor;
  return (target, key, exactDescriptorKind = true) => {
    const present = key in target;
    const reflected = reflectHas(target, key);
    const owned = own(target, key);
    const d = descriptor(target, key);
    const value = target[key];
    return [present, reflected, owned, d ? (exactDescriptorKind ? ('value' in d ? 'data' : 'accessor') : true) : null,
      typeof value, typeof value === 'number' || typeof value === 'boolean' ||
        typeof value === 'string' ? value : null];
  };
})();
const sampleBuiltin = (target, key) => sampleHas(target, key, false);
"#;

fn runtime() -> JsRuntime {
    let mut runtime = JsRuntime::with_document_and_url(
        TreeBuilder::parse("<html><body></body></html>").document(),
        "http://localhost/main",
    )
    .unwrap();
    runtime.eval(SAMPLE).unwrap();
    runtime
}

fn value(runtime: &mut JsRuntime, source: &str) -> Value {
    let result = runtime.eval(source).expect("author script");
    serde_json::from_str(&result.as_string().unwrap().to_std_string_escaped()).unwrap()
}

fn child_value(runtime: &mut JsRuntime, source: &str) -> Value {
    value(
        runtime,
        &format!(
            "const popup=window.open('','eval-child');popup.eval({})",
            json!(format!("{SAMPLE}\n{source}"))
        ),
    )
}

fn drain(runtime: &mut JsRuntime) {
    runtime.run_until_idle().unwrap();
    for _ in 0..8 {
        runtime.tick(16).unwrap();
    }
}

#[test]
fn child_eval_has_own_inherited_missing_and_symbol_properties() {
    let mut runtime = runtime();
    let actual = child_value(
        &mut runtime,
        r#"
      var ownUndefined; var ownZero=0; function ownFunction() {}
      var getterCalls=0;
      Object.defineProperty(globalThis,'onceAccessor',{configurable:true,get(){getterCalls++;return undefined;}});
      Object.getPrototypeOf(globalThis).inheritedFalse=false;
      Object.defineProperty(globalThis,Symbol.for('own-has-key'),{value:0,configurable:true});
      Object.defineProperty(Object.getPrototypeOf(globalThis),Symbol.for('inherited-has-key'),{value:undefined,configurable:true});
      const result={document:sampleBuiltin(globalThis,'document'),location:sampleBuiltin(globalThis,'location'),
        array:sampleBuiltin(globalThis,'Array'),listener:sampleBuiltin(globalThis,'addEventListener'),
        undefinedValue:sampleHas(globalThis,'ownUndefined'),zero:sampleHas(globalThis,'ownZero'),
        fn:sampleHas(globalThis,'ownFunction'),inherited:sampleHas(globalThis,'inheritedFalse'),
        accessor:sampleHas(globalThis,'onceAccessor'),
        ownSymbol:sampleHas(globalThis,Symbol.for('own-has-key')),
        inheritedSymbol:sampleHas(globalThis,Symbol.for('inherited-has-key')),
        missing:sampleHas(globalThis,'notDefinedHere'),missingSymbol:sampleHas(globalThis,Symbol.for('missing-has-key'))};
      result.getterCalls=getterCalls;JSON.stringify(result)
    "#,
    );
    assert_eq!(
        actual,
        json!({
        "document":[true,true,true,true,"object",null],
        "location":[true,true,true,true,"object",null],
        "array":[true,true,true,true,"function",null],
        "listener":[true,true,true,true,"function",null],
        "undefinedValue":[true,true,true,"data","undefined",null],
        "zero":[true,true,true,"data","number",0],"fn":[true,true,true,"data","function",null],
        "inherited":[true,true,false,null,"boolean",false],
        "accessor":[true,true,true,"accessor","undefined",null],"getterCalls":1,
        "ownSymbol":[true,true,true,"data","number",0],
        "inheritedSymbol":[true,true,false,null,"undefined",null],
        "missing":[false,false,false,null,"undefined",null],
        "missingSymbol":[false,false,false,null,"undefined",null]})
    );
}

#[test]
fn parent_proxy_has_uses_the_child_global_and_prototype() {
    let mut runtime = runtime();
    let actual = value(
        &mut runtime,
        r#"
      var parentQueryOnly=1;const popup=window.open('','parent-query');
      popup.eval(`var childUndefined;function childFunction(){} Object.getPrototypeOf(globalThis).childInherited=0;
        Object.defineProperty(globalThis,Symbol.for('parent-has-key'),{value:false,configurable:true});`);
      JSON.stringify({document:sampleBuiltin(popup,'document'),location:sampleBuiltin(popup,'location'),array:sampleBuiltin(popup,'Array'),listener:sampleBuiltin(popup,'addEventListener'),fn:sampleHas(popup,'childFunction'),
        own:sampleHas(popup,'childUndefined'),inherited:sampleHas(popup,'childInherited'),
        symbol:sampleHas(popup,Symbol.for('parent-has-key')),missing:sampleHas(popup,'missingChildName'),
        parentOnly:sampleHas(popup,'parentQueryOnly')})
    "#,
    );
    assert_eq!(
        actual,
        json!({"document":[true,true,true,true,"object",null],
        "location":[true,true,true,true,"object",null],"listener":[true,true,true,true,"function",null],"fn":[true,true,true,"data","function",null],
        "array":[true,true,true,true,"function",null],
        "own":[true,true,true,"data","undefined",null],"inherited":[true,true,false,null,"number",0],
        "symbol":[true,true,true,"data","boolean",false],
        "missing":[false,false,false,null,"undefined",null],"parentOnly":[false,false,false,null,"undefined",null]})
    );
}

struct Pages;
impl HttpTransport for Pages {
    fn send(
        &self,
        request: &HttpRequest,
        _: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        let script=match request.url().path(){
            "/scripted"=>format!("{SAMPLE}\n{}",r#"
              var scriptUndefined;function scriptFunction(){}
              Object.getPrototypeOf(globalThis).scriptInherited=false;
              Object.defineProperty(globalThis,Symbol.for('script-has-key'),{value:0,configurable:true});
              opener.postMessage({document:sampleBuiltin(globalThis,'document'),location:sampleBuiltin(globalThis,'location'),
                array:sampleBuiltin(globalThis,'Array'),listener:sampleBuiltin(globalThis,'addEventListener'),
                fn:sampleHas(globalThis,'scriptFunction'),own:sampleHas(globalThis,'scriptUndefined'),
                inherited:sampleHas(globalThis,'scriptInherited'),symbol:sampleHas(globalThis,Symbol.for('script-has-key')),
                missing:sampleHas(globalThis,'scriptMissing')},'*');
            "#),
            "/next"=>"var newDocumentZero=0;opener.postMessage('navigated','*');".into(),
            "/cross"=>"var crossChildSecret=0;opener.postMessage({ready:true,own:Object.prototype.hasOwnProperty.call(globalThis,'crossChildSecret')},'*');".into(),
            _=>panic!("unexpected auxiliary fixture path"),
        };
        let body = format!("<html><body><script>{script}</script></body></html>");
        HttpResponse::parse(&mut Cursor::new(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .into_bytes(),
        ))
    }
}

#[test]
fn ordinary_child_script_observes_its_own_global_properties() {
    let mut runtime = runtime();
    runtime.set_http_resource_transport(Some(Arc::new(Pages)));
    runtime.eval("globalThis.replies=[];addEventListener('message',e=>replies.push(e.data));window.open('/scripted','normal-script');").unwrap();
    drain(&mut runtime);
    assert_eq!(
        value(&mut runtime, "JSON.stringify(replies)"),
        json!([{
        "document":[true,true,true,true,"object",null],"location":[true,true,true,true,"object",null],"array":[true,true,true,true,"function",null],"listener":[true,true,true,true,"function",null],"fn":[true,true,true,"data","function",null],
        "own":[true,true,true,"data","undefined",null],"inherited":[true,true,false,null,"boolean",false],
        "symbol":[true,true,true,"data","number",0],"missing":[false,false,false,null,"undefined",null]}])
    );
}

#[test]
fn navigation_queries_the_new_global_and_reopening_uses_a_fresh_proxy() {
    let mut runtime = runtime();
    runtime.set_http_resource_transport(Some(Arc::new(Pages)));
    runtime
        .eval(
            r#"
      globalThis.popup=window.open('','navigation');globalThis.originalProxy=popup;
      globalThis.originalDocument=popup.document;popup.eval('var oldDocumentOnly=1');
      globalThis.oldSnapshot=sampleHas(popup,'oldDocumentOnly');
      globalThis.replies=[];addEventListener('message',e=>replies.push(e.data));
      popup.location.href='http://localhost/next';
    "#,
        )
        .unwrap();
    drain(&mut runtime);
    let actual = value(
        &mut runtime,
        r#"
      const navigated={oldSnapshot,ready:replies[0]==='navigated',sameProxy:popup===originalProxy,
        namedProxy:window.open('','navigation')===popup,newDocument:popup.document!==originalDocument,
        oldName:sampleHas(popup,'oldDocumentOnly'),newName:sampleHas(popup,'newDocumentZero')};
      popup.close();const replacement=window.open('','navigation');replacement.eval('var reopenedUndefined;function reopenedFunction(){}');
      JSON.stringify({...navigated,freshProxy:replacement!==originalProxy,
        reopened:sampleHas(replacement,'reopenedUndefined'),reopenedFunction:sampleHas(replacement,'reopenedFunction'),reopenedDocument:sampleBuiltin(replacement,'document'),reopenedLocation:sampleBuiltin(replacement,'location'),reopenedArray:sampleBuiltin(replacement,'Array'),reopenedListener:sampleBuiltin(replacement,'addEventListener'),stale:sampleHas(replacement,'newDocumentZero')})
    "#,
    );
    assert_eq!(
        actual,
        json!({"oldSnapshot":[true,true,true,"data","number",1],"ready":true,
        "sameProxy":true,"namedProxy":true,"newDocument":true,
        "oldName":[false,false,false,null,"undefined",null],"newName":[true,true,true,"data","number",0],
        "freshProxy":true,"reopened":[true,true,true,"data","undefined",null],
        "reopenedFunction":[true,true,true,"data","function",null],"reopenedDocument":[true,true,true,true,"object",null],"reopenedLocation":[true,true,true,true,"object",null],"reopenedArray":[true,true,true,true,"function",null],"reopenedListener":[true,true,true,true,"function",null],
        "stale":[false,false,false,null,"undefined",null]})
    );
}

#[test]
fn has_uses_the_intrinsic_captured_before_author_reflect_mutation() {
    let mut runtime = runtime();
    let actual = value(
        &mut runtime,
        &format!(
            r#"
      const popup=window.open('','intrinsics');
      const originalHas=Reflect.has;let calls=0;
      Reflect.has=()=>{{calls++;throw new Error('author Reflect.has');}};
      let result;try{{
        result={{parent:sampleBuiltin(popup,'document'),child:JSON.parse(popup.eval({})),
          missing:sampleHas(popup,'hasMissingName'),calls}};
      }}finally{{Reflect.has=originalHas;}}JSON.stringify(result)
    "#,
            json!(format!(
                "{SAMPLE}\nReflect.has=()=>{{throw new Error('child Reflect.has');}};JSON.stringify(sampleBuiltin(globalThis,'Array'))"
            ))
        ),
    );
    assert_eq!(
        actual,
        json!({"parent":[true,true,true,true,"object",null],
        "child":[true,true,true,true,"function",null],
        "missing":[false,false,false,null,"undefined",null],"calls":0})
    );
}

#[test]
fn has_can_be_the_first_access_to_a_fresh_auxiliary_window() {
    let mut runtime = runtime();
    let actual = value(
        &mut runtime,
        r#"
      const popup=window.open('','first-has');
      JSON.stringify({document:sampleBuiltin(popup,'document'),array:sampleBuiltin(popup,'Array'),
        inherited:sampleHas(popup,'toString'),missing:sampleHas(popup,'freshMissingName')})
    "#,
    );
    assert_eq!(
        actual,
        json!({"document":[true,true,true,true,"object",null],
        "array":[true,true,true,true,"function",null],"inherited":[true,true,false,null,"function",null],
        "missing":[false,false,false,null,"undefined",null]})
    );
}

#[test]
fn cross_origin_parent_does_not_learn_child_property_existence() {
    let mut runtime = runtime();
    runtime.set_http_resource_transport(Some(Arc::new(Pages)));
    runtime.eval("globalThis.replies=[];addEventListener('message',e=>replies.push(e.data));globalThis.popup=window.open('http://other.test/cross','cross-query');").unwrap();
    drain(&mut runtime);
    assert_eq!(
        value(&mut runtime, "JSON.stringify(replies)"),
        json!([{"ready":true,"own":true}])
    );
    let actual = value(
        &mut runtime,
        r#"
      const capture=fn=>{try{return {status:'ok',value:fn()};}catch(error){return {status:'throw',name:error.name};}};
      JSON.stringify({secretIn:capture(()=>'crossChildSecret' in popup),missingIn:capture(()=>'definitelyMissingName' in popup),
        secretReflect:capture(()=>Reflect.has(popup,'crossChildSecret')),missingReflect:capture(()=>Reflect.has(popup,'definitelyMissingName')),
        own:capture(()=>Object.prototype.hasOwnProperty.call(popup,'crossChildSecret')),
        descriptor:capture(()=>Object.getOwnPropertyDescriptor(popup,'document')),
        value:capture(()=>popup.document)})
    "#,
    );
    // Existence probes must not reveal which name the cross-origin child owns.
    // The eventual cross-origin has contract may return a value or throw.
    assert_eq!(actual["secretIn"], actual["missingIn"]);
    assert_eq!(actual["secretReflect"], actual["missingReflect"]);
    for access in ["own", "descriptor", "value"] {
        assert_eq!(
            actual[access],
            json!({"status":"throw","name":"SecurityError"})
        );
    }
}
