//! Custom element states are shared by DOM queries and style matching.
use omoikane::{html::TreeBuilder, js::JsRuntime};

fn runtime() -> JsRuntime {
    JsRuntime::with_document(TreeBuilder::parse("<head></head><body></body>").document()).unwrap()
}

fn evaluate(script: &str) -> String {
    let mut runtime = runtime();
    runtime
        .eval(&format!(
            "(()=>{{try{{{script}}}catch(e){{return e.name+': '+e.message;}}}})()"
        ))
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped()
}

#[test]
fn custom_states_are_same_object_ordered_dom_string_sets() {
    assert_eq!(
        evaluate(
            r#"
        customElements.define('x-states',class extends HTMLElement{constructor(){super();this.i=this.attachInternals();}});
        const element=document.createElement('x-states'),states=element.i.states;
        if(!(states instanceof CustomStateSet)||states!==element.i.states||states.size!==0||String(states)!=='[object CustomStateSet]')return 'initial';
        if('supports' in states)return 'supports';
        try{new CustomStateSet();return 'construct';}catch(e){if(e.name!=='TypeError')return 'construct-error';}
        for(const value of ['', '--a\tb', 'Mixed', 'mixed', '\uD800', '\uDC00']) {
            if(states.add(value)!==states||!states.has(value))return 'add';
        }
        if(states.size!==6)return 'size';
        states.add('Mixed');states.delete('Mixed');states.add('Mixed');
        if(JSON.stringify([...states])!==JSON.stringify(['','--a\tb','mixed','\uD800','\uDC00','Mixed']))return 'order';
        let calls=0;const value={toString(){calls++;return 'converted';}};
        if(states.add(value)!==states||calls!==1||!states.has(value)||calls!==2||!states.delete(value)||calls!==3)return 'conversion';
        for(const method of ['add','has','delete']) {
            for(const value of [Symbol(),{[Symbol.toPrimitive](){return Symbol();}}]) {
                try{states[method](value);return 'symbol:'+method;}catch(e){if(e.name!=='TypeError')return 'symbol-error';}
            }
            try{CustomStateSet.prototype[method].call({},{toString(){throw Error('conversion');}});return 'brand';}
            catch(e){if(e.name!=='TypeError')return 'brand-order';}
        }
        if(states.add()!==states||!states.has()||!states.delete())return 'undefined-argument';
        if(states.clear()!==undefined||states.size!==0||states.delete('absent')!==false)return 'clear';
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn state_set_iterators_and_for_each_observe_live_mutations() {
    assert_eq!(
        evaluate(
            r#"
        customElements.define('x-live',class extends HTMLElement{constructor(){super();this.i=this.attachInternals();}});
        const states=document.createElement('x-live').i.states;
        if(states.keys!==states.values||states[Symbol.iterator]!==states.values)return 'aliases';
        states.add('a');states.add('b');states.add('c');
        const iterator=states.values();states.delete('a');
        if(iterator.next().value!=='b')return 'first';
        states.delete('b');states.add('d');
        if(iterator.next().value!=='c'||iterator.next().value!=='d')return 'mutation';
        states.clear();if(!iterator.next().done)return 'clear-iterator';
        states.add('e');if(!iterator.next().done)return 'exhausted';
        if(JSON.stringify([...states.entries()])!=='[["e","e"]]')return 'entries';
        const calls=[],receiver={};
        states.forEach(function(a,b,set){if(this!==receiver||a!==b||set!==states)throw Error('arguments');calls.push(a);if(a==='e')states.add('f');},receiver);
        if(calls.join()!=='e,f')return 'forEach-live';
        try{states.forEach(null);return 'callback';}catch(e){if(e.name!=='TypeError')return 'callback-error';}
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn state_selector_grammar_escaping_and_cssom_agree() {
    assert_eq!(
        evaluate(
            r#"
        customElements.define('x-grammar',class extends HTMLElement{constructor(){super();this.i=this.attachInternals();}});
        const element=document.createElement('x-grammar');document.body.append(element);
        for(const name of ['foo','-','--','--16px','Mixed','(escaped state','1leading','space name','emoji-😀','é','initial','inherit','unset','revert','default']) {
            element.i.states.add(name);const selector=':STATE('+CSS.escape(name)+')';
            try{if(!CSS.supports('selector('+selector+')')||!element.matches(selector))return 'valid:'+name;}catch(e){return 'match:'+name+':'+selector+':'+e.name;}
            const sheet=new CSSStyleSheet();
            try{sheet.replaceSync(selector+'{color:red}');}catch(e){return 'replace:'+name+':'+selector+':'+e.name;}
            try{if(sheet.cssRules.length!==1||!element.matches(sheet.cssRules[0].selectorText))return 'cssom:'+name;}catch(e){return 'roundtrip:'+name+':'+sheet.cssRules[0].selectorText+':'+e.name;}
        }
        if(element.matches(':state(mixed)'))return 'case';
        for(const selector of [':state',':state()',':state(-)',':state(=)',':state(foo bar)',':state(foo,bar)',':state(16px)',':state("foo")',':state(foo())']) {
            if(CSS.supports('selector('+selector+')'))return 'invalid-support:'+selector;
            try{document.querySelector(selector);return 'invalid-query:'+selector;}catch(e){if(e.name!=='SyntaxError')return 'invalid-error';}
            const sheet=new CSSStyleSheet();sheet.replaceSync(selector+'{color:red}');
            if(sheet.cssRules.length!==0)return 'invalid-sheet:'+selector;
        }
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn state_changes_invalidate_relational_and_filtered_sibling_styles() {
    assert_eq!(
        evaluate(
            r#"
        const style=document.createElement('style');style.textContent=`
            x-item, #sibling, #parent{color:red}
            x-item:state(active){color:green}
            x-item:state(active)+#sibling{color:blue}
            #parent:has(>x-item:state(active)){color:green}
            x-item:nth-child(1 of :state(active)){background-color:blue}`;
        document.head.append(style);
        customElements.define('x-item',class extends HTMLElement{constructor(){super();this.i=this.attachInternals();}});
        const parent=document.createElement('section');parent.id='parent';
        const first=document.createElement('x-item'),second=document.createElement('x-item'),sibling=document.createElement('span');sibling.id='sibling';
        parent.append(first,second,sibling);document.body.append(parent);
        if(getComputedStyle(second).color!=='rgb(255, 0, 0)'||getComputedStyle(parent).color!=='rgb(255, 0, 0)')return 'before';
        second.i.states.add('active');
        if(!second.matches(':is(:state(active))')||second.matches(':not(:state(active))'))return 'matching';
        if(getComputedStyle(second).color!=='rgb(0, 128, 0)'||getComputedStyle(parent).color!=='rgb(0, 128, 0)'||getComputedStyle(sibling).color!=='rgb(0, 0, 255)')return 'add-style';
        if(getComputedStyle(second).backgroundColor!=='rgb(0, 0, 255)')return 'nth-first';
        first.i.states.add('active');if(getComputedStyle(first).backgroundColor!=='rgb(0, 0, 255)'||getComputedStyle(second).backgroundColor==='rgb(0, 0, 255)')return 'nth-change';
        second.i.states.clear();first.i.states.delete('active');
        if(getComputedStyle(second).color!=='rgb(255, 0, 0)'||getComputedStyle(parent).color!=='rgb(255, 0, 0)'||getComputedStyle(sibling).color!=='rgb(255, 0, 0)')return 'remove-style';
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn retained_state_sets_keep_detached_elements_usable_after_collection() {
    let mut runtime = runtime();
    runtime.eval(r#"
        customElements.define('x-retained-state',class extends HTMLElement{constructor(){super();this.i=this.attachInternals();}});
        globalThis.retainedStates=document.createElement('x-retained-state').i.states;
        retainedStates.add('before');
    "#).unwrap();
    boa_gc::force_collect();
    let result=runtime.eval(r#"(()=>{
        retainedStates.add('after');
        return retainedStates.size===2&&retainedStates.delete('before')&&[...retainedStates].join()==='after';
    })()"#).unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn state_mutations_retain_native_brands_and_captured_set_operations() {
    assert_eq!(
        evaluate(
            r#"
        customElements.define('x-private-states',class extends HTMLElement{constructor(){super();this.i=this.attachInternals();}});
        const element=document.createElement('x-private-states'),states=element.i.states;
        document.body.append(element);
        Object.defineProperty(element.i,'states',{get(){throw Error('author states getter');}});
        const original={has:Set.prototype.has,add:Set.prototype.add,delete:Set.prototype.delete,clear:Set.prototype.clear,values:Set.prototype.values,entries:Set.prototype.entries,forEach:Set.prototype.forEach};
        let callbacks=0;
        for(const name of Object.keys(original))Set.prototype[name]=function(){throw Error('author Set.'+name);};
        try{
            states.add('active');
            if(!states.has('active'))return 'has';
            states.forEach(()=>callbacks++);
            if(!states.delete('active'))return 'delete';
            states.add('active');states.clear();states.add('retained');
        }finally{for(const name of Object.keys(original))Set.prototype[name]=original[name];}
        if(callbacks!==1||!element.matches(':state(retained)')||element.matches(':state(active)'))return 'membership';
        for(const name of ['has','add','delete','clear','values','entries','forEach']) {
            try{Reflect.apply(CustomStateSet.prototype[name],{},['x']);return 'brand:'+name;}
            catch(e){if(e.name!=='TypeError')return 'brand-error:'+name;}
            if(!Object.getOwnPropertyDescriptor(CustomStateSet.prototype,name).enumerable)return 'enumerable:'+name;
        }
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn adopted_state_sets_survive_their_creating_iframe_retirement_and_collection() {
    let mut runtime = runtime();
    runtime.eval(r#"(()=>{
        const frame=document.createElement('iframe');document.body.append(frame);
        const doc=frame.contentDocument,win=frame.contentWindow;
        win.eval(`customElements.define('x-retired-state',class extends HTMLElement{constructor(){super();this.i=this.attachInternals();}})`);
        const element=doc.createElement('x-retired-state');element.id='retained-state';
        globalThis.retiredStates=element.i.states;
        retiredStates.add('before');document.body.append(element);frame.remove();
    })()"#).unwrap();
    runtime.run_until_idle().unwrap();
    boa_gc::force_collect();
    let result = runtime
        .eval(
            r#"(()=>{
        const element=document.getElementById('retained-state');
        retiredStates.add('after');retiredStates.delete('before');
        return element.ownerDocument===document&&element.i.states===retiredStates&&
            element.matches(':state(after)')&&!element.matches(':state(before)');
    })()"#,
        )
        .unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn host_and_part_states_match_their_own_elements_and_update_generated_content() {
    assert_eq!(
        evaluate(
            r#"
        customElements.define('x-state-inner',class extends HTMLElement{constructor(){super();this.i=this.attachInternals();}});
        customElements.define('x-state-host',class extends HTMLElement{constructor(){super();this.i=this.attachInternals();}});
        const host=document.createElement('x-state-host');host.id='state-host';
        const root=host.attachShadow({mode:'open'});
        root.innerHTML='<style>:host{color:red}:host(:state(hostActive)){color:green}x-state-inner{opacity:1}</style><x-state-inner part="inner"></x-state-inner>';
        const inner=root.querySelector('x-state-inner');
        const style=document.createElement('style');style.textContent=`
            #state-host::part(inner):state(innerActive){opacity:0.5}
            #state-host:state(hostActive)::part(inner):state(innerActive){opacity:0.25}
            #state-host::part(inner):not(:state(innerActive)){border-top-style:solid}
            #state-host::part(inner):state(innerActive)::before{content:'active';color:blue}`;
        document.head.append(style);document.body.append(host);
        if(getComputedStyle(host).color!=='rgb(255, 0, 0)'||getComputedStyle(inner).opacity!=='1')return 'before';
        inner.i.states.add('innerActive');
        if(getComputedStyle(inner).opacity!=='0.5'||getComputedStyle(host).color!=='rgb(255, 0, 0)')return 'inner';
        if(getComputedStyle(inner,'::before').color!=='rgb(0, 0, 255)')return 'part-before';
        host.i.states.add('hostActive');
        if(getComputedStyle(host).color!=='rgb(0, 128, 0)'||getComputedStyle(inner).opacity!=='0.25')return 'host';
        inner.i.states.clear();host.i.states.clear();
        if(getComputedStyle(host).color!=='rgb(255, 0, 0)'||getComputedStyle(inner).opacity!=='1'||getComputedStyle(inner).borderTopStyle!=='solid')return 'clear';
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn cloned_and_imported_states_are_independent_of_the_source_state_set() {
    assert_eq!(
        evaluate(
            r#"
        customElements.define('x-copy-state',class extends HTMLElement{constructor(){super();this.i=this.attachInternals();this.i.states.add('constructed');}});
        const source=document.createElement('x-copy-state');source.i.states.add('source-only');
        for(const copy of [source.cloneNode(true),document.importNode(source,true)]) {
            if(copy.i.states===source.i.states||!copy.i.states.has('constructed')||!copy.matches(':state(constructed)'))return 'construction';
            if(copy.i.states.has('source-only')||copy.matches(':state(source-only)'))return 'copied-private-state';
            copy.i.states.add('copy-only');
            if(source.i.states.has('copy-only')||source.matches(':state(copy-only)'))return 'shared-state';
        }
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn connected_element_keeps_its_states_when_no_collection_handle_is_retained() {
    let mut runtime = runtime();
    runtime.eval(r#"(()=>{
        customElements.define('x-connected-state',class extends HTMLElement{constructor(){super();this.i=this.attachInternals();}});
        const element=document.createElement('x-connected-state');element.id='connected-state';
        element.i.states.add('kept');document.body.append(element);
    })()"#).unwrap();
    runtime.run_until_idle().unwrap();
    boa_gc::force_collect();
    let result = runtime
        .eval(
            r#"(()=>{
        const element=document.getElementById('connected-state');
        return element.i.states.has('kept')&&element.matches(':state(kept)');
    })()"#,
        )
        .unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn state_matching_requires_successful_customization_even_when_the_set_is_retained() {
    assert_eq!(
        evaluate(
            r#"
        const pending=document.createElement('x-state-phase');document.body.append(pending);
        let during;
        class Target extends HTMLElement {
            constructor(){super();this.i=this.attachInternals();this.i.states.add('ctor');during=[this.matches(':defined'),this.matches(':state(ctor)')];}
        }
        customElements.define('x-state-phase',Target);
        if(during.join()!=='false,false'||!pending.matches(':state(ctor)'))return 'upgrade';
        const direct=new Target();
        if(during.join()!=='true,true'||!direct.matches(':state(ctor)'))return 'direct';
        const failed=document.createElement('x-state-failed');document.body.append(failed);
        customElements.define('x-state-failed',class extends HTMLElement {
            constructor(){super();this.i=this.attachInternals();this.i.states.add('ctor');throw Error('expected failure');}
        });
        if(!failed.i.states.has('ctor')||failed.matches(':defined')||failed.matches(':state(ctor)'))return 'failed';
        failed.i.states.add('later');
        return failed.matches(':state(later)')?'failed-later':'ok';
    "#,
        ),
        "ok"
    );
}

#[test]
fn state_cssom_canonicalizes_custom_identifiers_and_matches_escaped_host_arguments() {
    assert_eq!(
        evaluate(
            r#"
        const sheet=new CSSStyleSheet();
        sheet.replaceSync(':state( \\(escaped\\ state  ){opacity:.5}');
        if(sheet.cssRules[0].selectorText!==':state(\\(escaped\\ state)')return 'text:'+sheet.cssRules[0].selectorText;
        customElements.define('x-escaped-host',class extends HTMLElement{constructor(){super();this.i=this.attachInternals();}});
        const host=document.createElement('x-escaped-host');
        host.attachShadow({mode:'open'}).innerHTML='<style>:host(:state( \\(escaped\\ state  )){color:blue}</style>';
        document.body.append(host);host.i.states.add('(escaped state');
        if(getComputedStyle(host).color!=='rgb(0, 0, 255)')return 'escaped-host';
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn state_cssom_border_style_reads_follow_host_state_changes() {
    assert_eq!(
        evaluate(
            r#"
        customElements.define('x-border-state',class extends HTMLElement{constructor(){super();this.i=this.attachInternals();}});
        const host=document.createElement('x-border-state');
        host.attachShadow({mode:'open'}).innerHTML='<style>:host{border-style:solid}:host(:state(dotted)){border-style:dotted}</style>';
        document.body.append(host);
        if(getComputedStyle(host).borderTopStyle!=='solid')return 'before-long:'+getComputedStyle(host).borderTopStyle;
        if(getComputedStyle(host).borderStyle!=='solid')return 'before-shorthand:'+getComputedStyle(host).borderStyle;
        host.i.states.add('dotted');
        if(getComputedStyle(host).borderTopStyle!=='dotted'||getComputedStyle(host).getPropertyValue('border-style')!=='dotted')return 'after';
        host.style.borderStyle='solid dotted dashed double';
        if(getComputedStyle(host).borderStyle!=='solid dotted dashed double')return 'four-sides';
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn state_cssom_inputs_replace_surrogates_without_changing_raw_state_values() {
    assert_eq!(
        evaluate(
            r#"return (()=>{try{
customElements.define('x-state-input',class extends HTMLElement{constructor(){super();this.i=this.attachInternals();}});
const element=document.createElement('x-state-input'),control=document.createElement('x-state-input');
element.i.states.add('\uFFFD');control.i.states.add('\uD800');document.body.append(element,control);
let index=0;
for(const selector of [':state(\uD800)',':state(\uDC00)',':state(\0)',':state(\\d800)',':state(\\0)',':state(\\110000)']){
 if(!CSS.supports('selector('+selector+')')||!element.matches(selector)||control.matches(selector))return 'match:'+index;
 if(document.querySelector(selector)!==element||document.querySelectorAll(selector).length!==1)return 'query:'+index;
 const sheet=new CSSStyleSheet();sheet.replaceSync(selector+'{color:blue}');
 if(sheet.cssRules.length!==1||!element.matches(sheet.cssRules[0].selectorText))return 'cssom:'+index;
 document.adoptedStyleSheets=[sheet];if(getComputedStyle(element).color!=='rgb(0, 0, 255)')return 'style:'+index;
 index++;
}
return 'ok';
}catch(e){return e.name+': '+e.message;}})();"#
        ),
        "ok"
    );
}

#[test]
fn part_states_style_supported_pseudo_elements_without_exposing_tree_structure() {
    assert_eq!(
        evaluate(
            r#"
        customElements.define('x-part-states',class extends HTMLElement {
            constructor(){super();this.i=this.attachInternals();}
        });
        const host=document.createElement('div');host.id='part-states';
        const root=host.attachShadow({mode:'open'});
        root.innerHTML='<style>x-part-states{opacity:1}</style><x-part-states part="item">Text</x-part-states>';
        const inner=root.querySelector('x-part-states');
        const style=document.createElement('style');
        for(const pseudo of ['first-line','first-letter','marker','before::marker']) {
            const selector='#part-states::part(item):state(active)::'+pseudo;
            if(!CSS.supports('selector('+selector+')'))return 'syntax:'+pseudo;
        }
        style.textContent=`
            #part-states::part(item):state(active)::first-line{color:blue}
            #part-states::part(item):state(active)::first-letter{color:green}
            #part-states::part(item):state(active)::marker{color:purple}
            #part-states::part(item):state(active)::before{content:'item';display:list-item}
            #part-states::part(item):state(active)::before::marker{color:red}
            #part-states::part(item):state(active):first-child{opacity:.1}
            #part-states::part(item):state(active):has(*){opacity:.2}
            #part-states::part(item):state(active):is(:nth-child(1),:scope){opacity:.3}
            #part-states::part(item):state(active):not(:first-child){border-top-style:solid}
        `;
        document.head.append(style);document.body.append(host);inner.i.states.add('active');
        for(const [pseudo,color] of [['first-line','rgb(0, 0, 255)'],['first-letter','rgb(0, 128, 0)'],
            ['marker','rgb(128, 0, 128)'],['before::marker','rgb(255, 0, 0)']]) {
            if(getComputedStyle(inner,'::'+pseudo).color!==color)return 'style:'+pseudo+':'+getComputedStyle(inner,'::'+pseudo).color;
        }
        if(getComputedStyle(inner).opacity!=='1'||getComputedStyle(inner).borderTopStyle!=='solid')return 'structure:'+getComputedStyle(inner).opacity+':'+getComputedStyle(inner).borderTopStyle;
        inner.i.states.clear();
        if(getComputedStyle(inner,'::first-line').color==='rgb(0, 0, 255)')return 'stale';
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn state_selector_hex_escapes_preprocess_lines_and_preserve_css_whitespace_boundaries() {
    assert_eq!(
        evaluate(
            r#"return (()=>{try{
customElements.define('x-state-lines',class extends HTMLElement{constructor(){super();this.i=this.attachInternals();}});
const element=document.createElement('x-state-lines');element.i.states.add('foo');document.body.append(element);
for(const separator of ['\r\n','\r','\f','\n','\t',' ']) {
 const selector=':state(\\66'+separator+'oo)';
 if(!CSS.supports('selector('+selector+')')||!element.matches(selector))return 'match:'+JSON.stringify(separator);
 if(document.querySelector(selector)!==element||document.querySelectorAll(selector).length!==1)return 'query';
 const sheet=new CSSStyleSheet();sheet.replaceSync(selector+'{color:blue}');
 if(sheet.cssRules.length!==1||!element.matches(sheet.cssRules[0].selectorText))return 'cssom:'+JSON.stringify(separator);
 document.adoptedStyleSheets=[sheet];if(getComputedStyle(element).color!=='rgb(0, 0, 255)')return 'style';
}
for(const selector of [':state(\\66\u000boo)', ':state(f\u000boo)']) {
 if(CSS.supports('selector('+selector+')'))return 'vertical-tab';
 try{element.matches(selector);return 'accepted-vertical-tab';}catch(e){if(e.name!=='SyntaxError')return 'vertical-tab-error';}
}
for(const newline of ['\r\n','\r','\f','\n']) {
 const selector=':state(f\\'+newline+'oo)';
 if(CSS.supports('selector('+selector+')'))return 'invalid-newline';
 try{element.matches(selector);return 'accepted-newline';}catch(e){if(e.name!=='SyntaxError')return 'newline-error';}
}
return 'ok';
}catch(e){return e.name+': '+e.message;}})();"#
        ),
        "ok"
    );
}

#[test]
fn custom_state_set_is_window_only_in_dedicated_shared_and_service_workers() {
    use omoikane::http::{HttpParseError, HttpRequest, HttpResponse, HttpTransport};
    use std::sync::Arc;
    use std::time::Duration;

    struct WorkerTransport;
    impl HttpTransport for WorkerTransport {
        fn send(
            &self,
            request: &HttpRequest,
            _: Option<Duration>,
        ) -> Result<HttpResponse, HttpParseError> {
            let source = match request.url().path() {
                "/dedicated.js" => "postMessage(typeof CustomStateSet)",
                "/shared.js" => "onconnect=e=>e.ports[0].postMessage(typeof CustomStateSet)",
                "/service.js" => "onmessage=e=>e.source.postMessage(typeof CustomStateSet)",
                path => panic!("unexpected worker resource: {path}"),
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/javascript\r\nContent-Length: {}\r\n\r\n{source}",
                source.len()
            );
            HttpResponse::parse(&mut std::io::Cursor::new(response.into_bytes()))
        }
    }

    let mut runtime = JsRuntime::with_document_and_url(
        TreeBuilder::parse("<body></body>").document(),
        "https://custom-state.test/page",
    )
    .unwrap();
    runtime.set_http_resource_transport(Some(Arc::new(WorkerTransport)));
    runtime.eval(r#"
        globalThis.surfaces={window:typeof CustomStateSet,dedicated:null,shared:null,service:null,error:''};
        const dedicated=new Worker('/dedicated.js');
        dedicated.onmessage=e=>surfaces.dedicated=e.data;
        dedicated.onerror=e=>surfaces.error+='dedicated:'+e.message;
        const shared=new SharedWorker('/shared.js','custom-state-exposure');
        shared.port.onmessage=e=>surfaces.shared=e.data;shared.port.start();
        shared.onerror=e=>surfaces.error+='shared:'+e.message;
        navigator.serviceWorker.addEventListener('message',e=>surfaces.service=e.data);
        navigator.serviceWorker.register('/service.js').then(registration=>{
            const worker=registration.installing||registration.waiting||registration.active;
            worker.postMessage('probe');
        },error=>surfaces.error+='service:'+error.name);
    "#).unwrap();
    runtime.run_until_idle().unwrap();
    let observed = runtime
        .eval("JSON.stringify(surfaces)")
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(
        observed,
        r#"{"window":"function","dedicated":"undefined","shared":"undefined","service":"undefined","error":""}"#,
        "task errors: {:?}",
        runtime.take_task_errors()
    );
}

#[test]
fn custom_state_sets_remain_non_serializable_after_prototype_changes() {
    assert_eq!(
        evaluate(
            r#"return (()=>{try{
customElements.define('x-state-serialization',class extends HTMLElement{constructor(){super();this.i=this.attachInternals();}});
const element=document.createElement('x-state-serialization'),states=element.i.states;
states.add('kept');document.body.append(element);
const has=CustomStateSet.prototype.has;
const original=Object.getPrototypeOf(states);
const channel=new MessageChannel();
for(const [name,prototype] of [['original',original],['null',null],['plain',Object.prototype],['set',Set.prototype],['map',Map.prototype]]) {
 Object.setPrototypeOf(states,prototype);
 const buffer=new ArrayBuffer(8);
 try{structuredClone({states},{transfer:[buffer]});return 'accepted-clone:'+name;}
 catch(e){if(e.name!=='DataCloneError')return 'clone-error:'+name+':'+e.name;}
 if(buffer.byteLength!==8)return 'detached:'+name;
 try{channel.port1.postMessage(states);return 'accepted-message:'+name;}
 catch(e){if(e.name!=='DataCloneError')return 'message-error:'+name+':'+e.name;}
 if(!has.call(states,'kept')||!element.matches(':state(kept)'))return 'lost-state:'+name;
}
Object.setPrototypeOf(states,Object.prototype);
let reads=0;Object.defineProperty(states,'authorValue',{enumerable:true,get(){reads++;throw Error('author getter');}});
try{structuredClone(states);return 'accepted-getter';}
catch(e){if(e.name!=='DataCloneError'||reads!==0)return 'author-read';}
const frame=document.createElement('iframe');document.body.append(frame);
frame.contentWindow.eval("customElements.define('x-state-foreign',class extends HTMLElement{constructor(){super();this.i=this.attachInternals();}})");
const foreign=frame.contentDocument.createElement('x-state-foreign').i.states;
Object.setPrototypeOf(foreign,Object.prototype);
try{structuredClone(foreign);return 'accepted-foreign';}
catch(e){if(e.name!=='DataCloneError')return 'foreign-error:'+e.name;}
channel.port1.close();channel.port2.close();Object.setPrototypeOf(states,original);
return 'ok';
}catch(e){return e.name+': '+e.message;}})();"#
        ),
        "ok"
    );
}

#[test]
fn custom_state_sets_cannot_be_transferred_by_spoofing_message_port_prototypes() {
    assert_eq!(
        evaluate(
            r#"return (()=>{try{
customElements.define('x-state-transfer',class extends HTMLElement{constructor(){super();this.i=this.attachInternals();}});
const element=document.createElement('x-state-transfer'),states=element.i.states;
states.add('kept');document.body.append(element);
const has=CustomStateSet.prototype.has;
let reads=0;Object.defineProperty(states,'_closed',{configurable:true,get(){reads++;return false;}});
Object.setPrototypeOf(states,MessagePort.prototype);
const buffer=new ArrayBuffer(8);
try{structuredClone('unrelated',{transfer:[buffer,states]});return 'accepted-transfer';}
catch(e){if(e.name!=='DataCloneError')return 'transfer-error:'+e.name;}
if(buffer.byteLength!==8||reads!==0||!has.call(states,'kept')||!element.matches(':state(kept)'))return 'side-effects';
return 'ok';
}catch(e){return e.name+': '+e.message;}})();"#
        ),
        "ok"
    );
}

#[test]
fn custom_state_set_interface_uses_web_idl_property_descriptors() {
    assert_eq!(
        evaluate(
            r#"return (()=>{try{
const globalDescriptor=Object.getOwnPropertyDescriptor(globalThis,'CustomStateSet');
if(!globalDescriptor||globalDescriptor.value!==CustomStateSet||!globalDescriptor.writable||globalDescriptor.enumerable||!globalDescriptor.configurable)return 'global-descriptor';
if(CustomStateSet.name!=='CustomStateSet'||CustomStateSet.length!==0)return 'constructor-metadata';
const prototypeDescriptor=Object.getOwnPropertyDescriptor(CustomStateSet,'prototype');
if(prototypeDescriptor.writable||prototypeDescriptor.enumerable||prototypeDescriptor.configurable)return 'prototype-descriptor';
const prototype=CustomStateSet.prototype;
const size=Object.getOwnPropertyDescriptor(prototype,'size');
if(typeof size.get!=='function'||size.set!==undefined||!size.enumerable||!size.configurable)return 'size-descriptor';
for(const [name,length] of [['add',1],['has',1],['delete',1],['clear',0],['values',0],['keys',0],['entries',0],['forEach',1]]) {
 const descriptor=Object.getOwnPropertyDescriptor(prototype,name);
 if(!descriptor||typeof descriptor.value!=='function'||descriptor.value.length!==length||!descriptor.writable||!descriptor.enumerable||!descriptor.configurable)return 'method:'+name;
 try{new descriptor.value();return 'constructible:'+name;}catch(e){if(e.name!=='TypeError')return 'construct-error:'+name;}
}
const iterator=Object.getOwnPropertyDescriptor(prototype,Symbol.iterator);
if(!iterator.writable||iterator.enumerable||!iterator.configurable)return 'iterator-descriptor';
const tag=Object.getOwnPropertyDescriptor(prototype,Symbol.toStringTag);
if(tag.value!=='CustomStateSet'||tag.writable||tag.enumerable||!tag.configurable)return 'tag-descriptor';
return 'ok';
}catch(e){return e.name+': '+e.message;}})();"#
        ),
        "ok"
    );
}
