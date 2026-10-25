//! Indeterminate controls across live state, radio grouping and style changes.
use omoikane::{html::TreeBuilder, js::JsRuntime};

#[test]
fn checkbox_indeterminate_survives_reset_type_changes_and_clone() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(runtime.eval(r#"(() => {
      const f=document.createElement('form'),n=document.createElement('input');n.type='checkbox';f.append(n);document.body.append(f);
      if(n.indeterminate!==false||n.matches(':indeterminate'))return false;
      n.indeterminate=true;n.checked=true;
      if(!n.matches(':checked:indeterminate'))return false;
      f.reset();if(!n.indeterminate||n.checked||!n.matches(':indeterminate'))return false;
      n.type='text';if(!n.indeterminate||n.matches(':indeterminate'))return false;
      n.type='checkbox';if(!n.matches(':indeterminate'))return false;n.checked=true;
      const c=n.cloneNode(true),d=document.importNode(n,true);
      if(!c.checked||!d.checked||!c.indeterminate||!c.matches(':checked:indeterminate')||!d.indeterminate||!d.matches(':checked:indeterminate'))return false;
      c.defaultChecked=true;c.defaultChecked=false;if(!c.checked)return false;
      n.indeterminate=false;return !n.matches(':indeterminate')&&c.matches(':indeterminate');
    })()"#).unwrap().as_boolean(),Some(true));
}

#[test]
fn checkbox_pre_activation_and_cancellation_restore_both_states() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(runtime.eval(r#"(() => {
      const n=document.createElement('input');n.type='checkbox';document.body.append(n);n.indeterminate=true;
      let observed=false;n.addEventListener('click',e=>{observed=n.checked&&!n.indeterminate&&!n.matches(':indeterminate');e.preventDefault();},{once:true});
      n.click();if(!observed||n.checked||!n.indeterminate||!n.matches(':indeterminate'))return false;
      n.click();if(!n.checked||n.indeterminate||n.matches(':indeterminate'))return false;
      n.indeterminate=true;
      n.dispatchEvent(new MouseEvent('click',{bubbles:true,cancelable:true}));
      return !n.checked&&!n.indeterminate&&!n.matches(':indeterminate');
    })()"#).unwrap().as_boolean(),Some(true));
}

#[test]
fn radio_indeterminate_tracks_names_form_owners_and_live_checkedness() {
    let mut runtime=JsRuntime::with_document(TreeBuilder::parse("<form id=f><input id=a type=radio name=g><input id=b type=radio name=g><input id=u type=radio></form><form id=h><input id=c type=radio name=g></form><input id=x type=radio name=g form=f>").document()).unwrap();
    assert_eq!(runtime.eval(r#"(() => {
      const a=document.getElementById('a'),b=document.getElementById('b'),c=document.getElementById('c'),u=document.getElementById('u'),x=document.getElementById('x');
      if(![a,b,c,u,x].every(n=>n.matches(':indeterminate')))return false;
      a.checked=true;if([a,b,x].some(n=>n.matches(':indeterminate'))||!c.matches(':indeterminate')||!u.matches(':indeterminate'))return false;
      a.disabled=true;if(b.matches(':indeterminate'))return false;
      u.checked=true;if(u.matches(':indeterminate'))return false;
      b.name='other';b.indeterminate=true;if(!b.matches(':indeterminate'))return false;
      x.setAttribute('form','h');if(!x.matches(':indeterminate'))return false;
      c.checked=true;if(x.matches(':indeterminate'))return false;
      a.checked=false;return a.matches(':indeterminate');
    })()"#).unwrap().as_boolean(),Some(true));
}

#[test]
fn progress_indeterminate_uses_null_namespace_attribute_presence() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(runtime.eval(r#"(() => {
      const p=document.createElement('progress');if(!p.matches(':indeterminate'))return false;
      for(const value of ['','bad','0','-1','1']){p.setAttribute('value',value);if(p.matches(':indeterminate'))return false;}
      p.removeAttribute('value');if(!p.matches(':indeterminate'))return false;
      p.setAttributeNS('urn:other','value','1');if(!p.matches(':indeterminate'))return false;
      for(const ns of [null,'http://www.w3.org/2000/svg'])if(document.createElementNS(ns,'progress').matches(':indeterminate'))return false;
      const doc=document.implementation.createDocument('http://www.w3.org/1999/xhtml','html',null),q=doc.createElementNS('http://www.w3.org/1999/xhtml','h:progress');
      if(!q.matches(':indeterminate'))return false;q.setAttributeNS(null,'value','0');return !q.matches(':indeterminate');
    })()"#).unwrap().as_boolean(),Some(true));
}

#[test]
fn indeterminate_updates_relational_and_filtered_sibling_styles() {
    let mut runtime=JsRuntime::with_document(TreeBuilder::parse("<style>input{color:green;background-color:transparent}:indeterminate{color:red}:nth-child(2 of :indeterminate){background-color:yellow}form{background-color:transparent}form:has(:indeterminate){background-color:blue}</style><form id=f><input id=a type=checkbox><input id=b type=checkbox></form>").document()).unwrap();
    assert_eq!(runtime.eval(r#"(() => {
      const a=document.getElementById('a'),b=document.getElementById('b'),f=document.getElementById('f'),bg=n=>getComputedStyle(n).backgroundColor;
      if(bg(f)!=='rgba(0, 0, 0, 0)')return false;
      a.indeterminate=true;b.indeterminate=true;
      if(getComputedStyle(a).color!=='rgb(255, 0, 0)'||bg(b)!=='rgb(255, 255, 0)'||bg(f)!=='rgb(0, 0, 255)')return false;
      a.indeterminate=false;if(bg(b)!=='rgba(0, 0, 0, 0)')return false;
      b.indeterminate=false;return bg(f)==='rgba(0, 0, 0, 0)';
    })()"#).unwrap().as_boolean(),Some(true));
}

#[test]
fn indeterminate_nonfunctional_grammar_and_html_input_brand() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(runtime.eval(r#"(() => {
      if(!CSS.supports('selector(:INDETERMINATE)'))return false;
      for(const selector of [':indeterminate()',':indeterminate(value)']){
        if(CSS.supports(`selector(${selector})`))return false;
        const sheet=new CSSStyleSheet();sheet.replaceSync(`${selector}{color:red}`);if(sheet.cssRules.length)return false;
        try{document.querySelector(selector);return false;}catch(e){if(e.name!=='SyntaxError')return false;}
      }
      const descriptor=Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'indeterminate');if(!descriptor)return false;
      for(const n of [document.createElement('div'),document.createElementNS(null,'input'),{}]){
        try{descriptor.get.call(n);return false;}catch(e){if(!(e instanceof TypeError))return false;}
      }
      const n=document.createElement('input');n.type='radio';n.checked=true;n.indeterminate=true;
      return n.indeterminate&&!n.matches(':indeterminate');
    })()"#).unwrap().as_boolean(),Some(true));
}

#[test]
fn checkable_activation_emits_events_and_preserves_disabled_and_plain_event_rules() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(runtime.eval(r#"(() => {
      const n=document.createElement('input');n.type='checkbox';document.body.append(n);const events=[];
      for(const type of ['click','input','change'])n.addEventListener(type,e=>events.push([type,n.checked,n.indeterminate,e.isTrusted]));
      n.indeterminate=true;n.addEventListener('click',e=>e.preventDefault(),{once:true});n.click();
      if(JSON.stringify(events.splice(0))!==JSON.stringify([['click',true,false,false]])||n.checked||!n.indeterminate)return false;
      n.click();if(JSON.stringify(events.splice(0))!==JSON.stringify([['click',true,false,false],['input',true,false,true],['change',true,false,true]]))return false;
      n.indeterminate=true;n.disabled=true;n.click();if(events.length||!n.checked||!n.indeterminate)return false;
      n.dispatchEvent(new MouseEvent('click',{bubbles:true,cancelable:true}));
      if(JSON.stringify(events.splice(0))!==JSON.stringify([['click',false,false,false],['input',false,false,true],['change',false,false,true]]))return false;
      n.disabled=false;n.indeterminate=true;n.dispatchEvent(new Event('click',{bubbles:true,cancelable:true}));
      if(JSON.stringify(events.splice(0))!==JSON.stringify([['click',false,true,false]])||n.checked||!n.indeterminate)return false;
      n.type='radio';n.checked=true;n.click();if(events.splice(0).map(e=>e[0]).join(',')!=='click')return false;
      n.checked=false;n.click();return events.map(e=>e[0]).join(',')==='click,input,change';
    })()"#).unwrap().as_boolean(),Some(true));
}

#[test]
fn canceled_radio_activation_revalidates_the_previous_group() {
    let mut runtime = JsRuntime::with_document(
        TreeBuilder::parse(
            "<form><input id=a type=radio name=g><input id=b type=radio name=g></form>",
        )
        .document(),
    )
    .unwrap();
    assert_eq!(runtime.eval(r#"(() => {
      const a=document.getElementById('a'),b=document.getElementById('b');a.checked=true;let observed=false;
      b.addEventListener('click',e=>{observed=!a.checked&&b.checked;e.preventDefault();},{once:true});b.click();
      if(!observed||!a.checked||b.checked||a.matches(':indeterminate')||b.matches(':indeterminate'))return false;
      b.addEventListener('click',e=>{a.name='other';e.preventDefault();},{once:true});b.click();
      return !a.checked&&!b.checked&&a.matches(':indeterminate')&&b.matches(':indeterminate');
    })()"#).unwrap().as_boolean(),Some(true));
}

#[test]
fn checkable_click_uses_cross_realm_mouse_brands_and_blocks_recursive_clicks() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
      const n=document.createElement('input');n.type='checkbox';document.body.append(n);
      const frame=document.createElement('iframe');document.body.append(frame);n.indeterminate=true;
      n.dispatchEvent(new frame.contentWindow.MouseEvent('click',{bubbles:true,cancelable:true}));
      if(!n.checked||n.indeterminate||n.matches(':indeterminate'))return false;
      let calls=0;n.addEventListener('click',()=>{calls++;n.click();});n.click();
      return calls===1&&!n.checked&&!n.indeterminate;
    })()"#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn radio_peer_updates_and_reset_preserve_default_checkedness_rules() {
    let mut runtime=JsRuntime::with_document(TreeBuilder::parse("<form id=f><input id=a type=radio name=g checked><input id=b type=radio name=g></form>").document()).unwrap();
    let result=runtime.eval(r#"(() => {
      const a=document.getElementById('a'),b=document.getElementById('b'),f=document.getElementById('f');
      b.checked=true;b.checked=false;a.setAttribute('checked','');
      if(a.checked)return 'existing-attribute';
      a.removeAttribute('checked');a.setAttribute('checked','');
      if(!a.checked||a.matches(':indeterminate'))return 'peer-default';
      a.checked=false;f.reset();a.removeAttribute('checked');
      if(a.checked||!a.matches(':indeterminate'))return 'reset-dirty';
      const n=document.createElement('input');n.type='text';n.checked=true;
      if(!n.checked||n.matches(':checked'))return 'noncheckable-checkedness';
      n.type='checkbox';return n.matches(':checked')?'ok':'type-change';
    })()"#).unwrap().as_string().unwrap().to_std_string_escaped();
    assert_eq!(result, "ok");
}

#[test]
fn checked_radios_resynchronize_after_attribute_and_tree_changes() {
    let mut runtime=JsRuntime::with_document(TreeBuilder::parse("<form id=f><input id=a type=radio name=g checked><input id=b type=radio name=h checked><input id=c type=checkbox name=g checked></form><form id=q><input id=d type=radio name=g checked></form>").document()).unwrap();
    let result=runtime.eval(r#"(() => {
      const a=document.getElementById('a'),b=document.getElementById('b'),c=document.getElementById('c'),d=document.getElementById('d'),f=document.getElementById('f');
      b.name='g';if(a.checked||!b.checked)return 'name';
      b.checked=false;if(!a.matches(':indeterminate')||!b.matches(':indeterminate'))return 'name-state';
      a.checked=true;c.type='radio';if(a.checked||!c.checked)return 'type';
      c.checked=false;if(!a.matches(':indeterminate'))return 'type-state';
      a.checked=true;d.setAttribute('form','f');if(a.checked||!d.checked)return 'form';
      d.checked=false;if(!a.matches(':indeterminate'))return 'form-state';
      const x=document.createElement('input');x.type='radio';x.name='g';x.checked=true;a.checked=true;f.prepend(x);
      if(a.checked||!x.checked)return 'connected';
      x.checked=false;return a.matches(':indeterminate')?'ok':'connected-state';
    })()"#).unwrap().as_string().unwrap().to_std_string_escaped();
    assert_eq!(result, "ok");
}

#[test]
fn radio_context_keeps_namespace_shadow_and_utf16_groups_distinct() {
    let mut runtime = JsRuntime::new().unwrap();
    let result=runtime.eval(r#"(() => {
      const H='http://www.w3.org/1999/xhtml',xml=document.implementation.createDocument(H,'html',null);
      const f=xml.createElementNS(H,'h:form'),a=xml.createElementNS(H,'h:input'),b=xml.createElementNS(H,'h:input'),c=xml.createElementNS(H,'h:input');
      for(const n of [a,b,c])n.type='radio';a.name=b.name='\ud800';c.name='\ud801';f.append(a,b,c);xml.documentElement.append(f);
      c.checked=true;a.checked=true;b.checked=true;b.checked=false;
      if(a.checked||!a.matches(':indeterminate')||!c.checked||c.matches(':indeterminate')||a.name!=='\ud800'||c.name!=='\ud801')
        return JSON.stringify({stage:'prefix-utf16',checked:[a.checked,b.checked,c.checked],names:[a.name,b.name,c.name],owner:[a.form===f,b.form===f,c.form===f],inputCount:xml.querySelectorAll('input').length,states:[a.matches(':indeterminate'),b.matches(':indeterminate'),c.matches(':indeterminate')]});
      const parent=document.createElement('input'),child=document.createElement('input');parent.type=child.type='radio';parent.name=child.name='root';
      parent.checked=true;parent.append(child);child.checked=true;child.checked=false;
      if(parent.checked||!parent.matches(':indeterminate'))return 'root-member';
      const outer=document.createElement('form'),o=document.createElement('input'),host=document.createElement('div');outer.id='f';o.type='radio';o.name='g';o.checked=true;outer.append(o);document.body.append(outer,host);
      const s=host.attachShadow({mode:'open'});s.innerHTML='<form id=f><input id=inside type=radio name=g checked></form>';
      const inside=s.querySelector('#inside'),x=document.createElement('input');x.type='radio';x.name='g';x.setAttribute('form','f');x.checked=true;s.prepend(x);
      if(inside.checked||!x.checked||!o.checked)return 'shadow-owner';
      x.checked=false;if(!inside.matches(':indeterminate')||o.matches(':indeterminate'))return 'shadow-empty';
      const named=document.createElement('form'),n1=document.createElement('input'),n2=document.createElement('input');
      named.setAttributeNS('urn:other','id','foreign');named.id='real';
      for(const n of [n1,n2]){n.type='radio';n.setAttributeNS('urn:other','name','foreign');n.name='\ud800';}
      named.append(n1,n2);document.body.append(named);
      if(named.id!=='real'||n1.name!=='\ud800'||named.getAttributeNS('urn:other','id')!=='foreign')return 'null-namespace-reflection';
      n1.checked=true;n2.checked=true;n2.checked=false;if(n1.checked||!n1.matches(':indeterminate'))return 'foreign-name-group';
      const q=document.createElement('input'),r=document.createElement('input');q.type=r.type='radio';q.name=r.name='\ud800';r.setAttribute('form','real');q.checked=r.checked=true;document.body.prepend(q,r);
      named.id='other';if(q.checked||!r.checked)return 'id-reassociation';
      r.checked=false;return q.matches(':indeterminate')?'ok':'id-state';
    })()"#).unwrap().as_string().unwrap().to_std_string_escaped();
    assert_eq!(result, "ok");
}

#[test]
fn input_context_canonical_type_uses_ascii_keywords() {
    let mut runtime = JsRuntime::new().unwrap();
    let result=runtime.eval(r#"(() => {
      const n=document.createElement('input');document.body.append(n);
      for(const kind of ['chec\u212abox','wee\u212a']) {
        n.setAttribute('type',kind);n.indeterminate=true;n.checked=false;
        if(n.type!=='text'||n.getAttribute('type')!==kind||n.matches(':indeterminate'))return 'unicode-type';
        n.click();if(n.checked||!n.indeterminate)return 'unknown-activation';
      }
      n.type='CHECKBOX';if(n.type!=='checkbox'||!n.matches(':indeterminate'))return 'ascii-type';
      n.click();if(!n.checked||n.indeterminate||!n.matches(':checked'))return 'ascii-activation';
      const d=Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'type');
      try{d.get.call(document.createElement('div'));return 'brand';}catch(e){if(!(e instanceof TypeError))return 'error-class';}
      return 'ok';
    })()"#).unwrap().as_string().unwrap().to_std_string_escaped();
    assert_eq!(result, "ok");
}
