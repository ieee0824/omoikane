//! Scoped custom element registries select definitions per element.
use omoikane::{html::TreeBuilder, js::JsRuntime};

fn evaluate(script: &str) -> String {
    let mut runtime =
        JsRuntime::with_document(TreeBuilder::parse("<body></body>").document()).unwrap();
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
fn scoped_creation_keeps_equal_names_in_independent_registries() {
    assert_eq!(
        evaluate(
            r#"
        const first=new CustomElementRegistry(), second=new CustomElementRegistry();
        class First extends HTMLElement {constructor(){super();this.kind='first';}}
        class Second extends HTMLElement {constructor(){super();this.kind='second';}}
        first.define('x-scoped',First);second.define('x-scoped',Second);
        const a=document.createElement('x-scoped',{customElementRegistry:first});
        const b=document.createElement('x-scoped',{customElementRegistry:second});
        const global=document.createElement('x-scoped');
        if(a.kind!=='first'||b.kind!=='second'||!a.matches(':defined')||!b.matches(':defined'))return 'creation';
        if(a.customElementRegistry!==first||b.customElementRegistry!==second)return 'association';
        if(global.matches(':defined')||global.customElementRegistry!==customElements)return 'global';
        try{new First();return 'direct';}catch(e){if(e.name!=='TypeError')return 'direct-error';}
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn initialize_adopts_only_null_associations_and_upgrades_inert_document_elements() {
    assert_eq!(
        evaluate(
            r#"
        const doc=new Document(), registry=new CustomElementRegistry();
        const plain=doc.createElement('plain');
        const pending=doc.createElementNS('http://www.w3.org/1999/xhtml','x-inert');
        const root=doc.createElement('root');root.append(plain,pending);doc.append(root);
        registry.define('x-inert',class extends HTMLElement{});
        if(pending.matches(':defined')||plain.customElementRegistry!==null)return 'before';
        registry.initialize(root);
        if(!pending.matches(':defined')||plain.customElementRegistry!==registry)return 'initialize';
        const global=document.createElement('x-inert');registry.initialize(global);
        if(global.customElementRegistry!==customElements||global.matches(':defined'))return 'preserve';
        try{customElements.initialize(doc);return 'foreign-global';}catch(e){if(e.name!=='NotSupportedError')return 'foreign-error';}
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn explicit_null_and_registry_brand_are_checked_without_author_fields() {
    assert_eq!(
        evaluate(
            r#"
        const node=document.createElement('x-null',{customElementRegistry:null});
        if(node.customElementRegistry!==null||node.matches(':defined'))return 'null';
        const registry=new CustomElementRegistry();registry.define('x-null',class extends HTMLElement{});
        registry.initialize(node);
        if(node.customElementRegistry!==registry||!node.matches(':defined'))return 'upgrade';
        try{document.createElement('x-fake',{customElementRegistry:{}});return 'brand';}catch(e){if(e.name!=='TypeError')return 'brand-error';}
        try{registry.define('x-builtin',class extends HTMLParagraphElement{},{extends:'p'});return 'builtin';}catch(e){if(e.name!=='NotSupportedError')return 'builtin-error';}
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn definitions_upgrade_connected_candidates_and_invalidate_styles() {
    assert_eq!(
        evaluate(
            r#"
        const registry=new CustomElementRegistry();
        const style=document.createElement('style');style.textContent='x-late{color:red}x-late:defined{color:green}';document.body.append(style);
        const node=document.createElement('x-late',{customElementRegistry:registry});document.body.append(node);
        if(node.matches(':defined')||getComputedStyle(node).color!=='rgb(255, 0, 0)')return 'before';
        registry.define('x-late',class extends HTMLElement{});
        return node.matches(':defined')&&getComputedStyle(node).color==='rgb(0, 128, 0)'?'ok':'after';
    "#
        ),
        "ok"
    );
}

#[test]
fn nested_constructor_failure_restores_the_outer_registry_and_global_lookup() {
    assert_eq!(
        evaluate(
            r#"
        const first=new CustomElementRegistry(), second=new CustomElementRegistry();
        let nesting=false, inner;
        class Shared extends HTMLElement {
            constructor(){
                if(!nesting){nesting=true;inner=document.createElement('x-inner',{customElementRegistry:second});}
                super();
                if(this.localName==='x-inner')throw Error('inner failure');
            }
        }
        first.define('x-outer',Shared);second.define('x-inner',Shared);
        const outer=document.createElement('x-outer',{customElementRegistry:first});
        if(!outer.matches(':defined')||inner.matches(':defined'))return 'state';
        if(outer.customElementRegistry!==first||inner.customElementRegistry!==second)return 'association';
        customElements.define('x-global-shared',Shared);
        const global=new Shared();
        return global.localName==='x-global-shared'&&global.customElementRegistry===customElements&&global.matches(':defined')?'ok':'global';
    "#
        ),
        "ok"
    );
}

#[test]
fn shadow_roots_select_the_document_registry_by_default_and_preserve_explicit_null() {
    assert_eq!(
        evaluate(
            r#"
        const registry=new CustomElementRegistry();
        const host=document.createElement('div',{customElementRegistry:registry});
        const root=host.attachShadow({mode:'open'});
        if(root.customElementRegistry!==customElements)return 'default';
        const other=document.createElement('div');
        const scoped=other.attachShadow({mode:'closed',customElementRegistry:registry});
        if(scoped.customElementRegistry!==registry)return 'scoped';
        const empty=document.createElement('div').attachShadow({mode:'open',customElementRegistry:null});
        if(empty.customElementRegistry!==null)return 'null';
        registry.initialize(empty);
        if(empty.customElementRegistry!==registry)return 'initialize';
        customElements.initialize(empty);
        if(empty.customElementRegistry!==registry)return 'preserve';
        const getter=Object.getOwnPropertyDescriptor(ShadowRoot.prototype,'customElementRegistry').get;
        try{getter.call(document.createDocumentFragment());return 'brand';}catch(e){if(e.name!=='TypeError')return 'brand-error';}
        try{document.createElement('div').attachShadow({mode:'open',customElementRegistry:{}});return 'fake';}catch(e){if(e.name!=='TypeError')return 'fake-error';}
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn shadow_fragment_parsing_uses_its_registry_and_initialize_stops_at_shadow_boundaries() {
    assert_eq!(
        evaluate(
            r#"
        const registry=new CustomElementRegistry();
        registry.define('x-shadow-scope',class extends HTMLElement{constructor(){super();this.kind='scoped';}});
        customElements.define('x-shadow-scope',class extends HTMLElement{constructor(){super();this.kind='global';}});
        const host=document.createElement('div');
        const root=host.attachShadow({mode:'open',customElementRegistry:registry});
        root.innerHTML='<section><x-shadow-scope></x-shadow-scope></section>';
        const child=root.querySelector('x-shadow-scope');
        if(child.kind!=='scoped'||child.customElementRegistry!==registry||!child.matches(':defined'))return 'parser';
        const emptyHost=document.createElement('div',{customElementRegistry:null});
        const emptyRoot=emptyHost.attachShadow({mode:'open',customElementRegistry:null});
        emptyRoot.innerHTML='<x-shadow-scope></x-shadow-scope>';
        const pending=emptyRoot.firstChild;
        if(pending.customElementRegistry!==null||pending.matches(':defined'))return 'pending';
        registry.initialize(emptyHost);
        if(emptyRoot.customElementRegistry!==null||pending.matches(':defined'))return 'boundary';
        registry.initialize(emptyRoot);
        if(pending.customElementRegistry!==registry||pending.kind!=='scoped'||!pending.matches(':defined'))return 'initialize';
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn clonable_shadow_roots_keep_their_settings_and_descendants_even_on_shallow_host_clones() {
    assert_eq!(
        evaluate(
            r#"
        const host=document.createElement('div');host.append(document.createElement('i'));
        const root=host.attachShadow({mode:'open',clonable:true,serializable:true,delegatesFocus:true,customElementRegistry:null});
        root.innerHTML='<span>inside</span>';
        for(const deep of [false,true]) {
            const clone=host.cloneNode(deep), shadow=clone.shadowRoot;
            if(!shadow)return 'missing';
            if(!shadow.clonable||!shadow.serializable||!shadow.delegatesFocus)return 'settings';
            if(shadow.firstChild.localName!=='span'||shadow.textContent!=='inside')return 'children';
            if(shadow.customElementRegistry!==null||shadow.firstChild.customElementRegistry!==null)return 'registry';
            if(clone.childNodes.length!==(deep?1:0))return 'light';
            if(shadow.firstChild===root.firstChild||shadow.host!==clone)return 'identity';
        }
        const opaque=document.createElement('div');opaque.attachShadow({mode:'open'});
        if(opaque.cloneNode(true).shadowRoot!==null)return 'unclonable';
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn import_uses_native_brands_and_explicit_fallback_without_replacing_existing_associations() {
    assert_eq!(
        evaluate(
            r#"
        const frame=document.createElement('iframe');document.body.append(frame);
        const destination=frame.contentDocument;
        const host=document.createElement('div');host.append(document.createElement('b'));
        const cross=destination.importNode(host,{selfOnly:false});
        if(cross.ownerDocument!==destination||cross.customElementRegistry!==frame.contentWindow.customElements||cross.childNodes.length!==1)return 'cross-realm';
        const fallback=new CustomElementRegistry(), own=new CustomElementRegistry();
        fallback.define('x-import-fallback',class extends HTMLElement{});
        const outer=document.createElement('div',{customElementRegistry:null});
        const inner=document.createElement('div',{customElementRegistry:own});
        inner.append(document.createElement('x-import-fallback',{customElementRegistry:null}));outer.append(inner);
        const copy=document.importNode(outer,{customElementRegistry:fallback});
        if(copy.customElementRegistry!==fallback||copy.firstChild.customElementRegistry!==own)return 'associations';
        if(copy.firstChild.firstChild.customElementRegistry!==fallback||!copy.firstChild.firstChild.matches(':defined'))return 'fallback';
        if(document.importNode(host,{selfOnly:true}).hasChildNodes())return 'self-only';
        try{document.importNode(host,{customElementRegistry:null});return 'null';}catch(e){if(e.name!=='TypeError')return 'null-error';}
        try{destination.importNode(host,{customElementRegistry:customElements});return 'foreign';}catch(e){if(e.name!=='NotSupportedError')return 'foreign-error';}
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn adoption_rebinds_global_registries_preserves_scoped_registries_and_updates_late_definitions() {
    assert_eq!(
        evaluate(
            r#"
        const frame=document.createElement('iframe');document.body.append(frame);
        const dest=frame.contentDocument, global=frame.contentWindow.customElements;
        const host=document.createElement('div');const shadow=host.attachShadow({mode:'open'});
        shadow.innerHTML='<span></span>';dest.body.append(host);
        if(host.customElementRegistry!==global||shadow.customElementRegistry!==global||shadow.firstChild.customElementRegistry!==global)return 'global';
        const inert=document.implementation.createHTMLDocument();
        const scoped=new CustomElementRegistry();
        const root=inert.createElement('div');
        const child=inert.createElement('x-adopt-scope',{customElementRegistry:scoped});
        const pending=inert.createElement('x-adopt-null',{customElementRegistry:null});
        child.append(pending);root.append(child);dest.body.append(root);
        if(root.customElementRegistry!==global||child.customElementRegistry!==scoped||pending.customElementRegistry!==null)return 'scope';
        scoped.define('x-adopt-scope',class extends HTMLElement{});
        if(!child.matches(':defined'))return 'late';
        const explicit=document.createElement('div');dest.adoptNode(explicit);
        if(explicit.customElementRegistry!==global||explicit.ownerDocument!==dest)return 'explicit';
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn registry_rejects_null_callbacks_and_observed_sequences_and_recovers_after_errors() {
    assert_eq!(
        evaluate(
            r#"
        let index=0;
        for(const registry of [customElements,new CustomElementRegistry()]) {
            for(const name of ['connectedCallback','disconnectedCallback','connectedMoveCallback','adoptedCallback','attributeChangedCallback']) {
                const tag='x-null-callback-'+index++;
                class Candidate extends HTMLElement{};
                Object.defineProperty(Candidate.prototype,name,{value:null,configurable:true});
                try{registry.define(tag,Candidate);return 'accepted:'+name;}catch(e){if(e.name!=='TypeError')return 'error:'+name;}
                if(registry.get(tag)!==undefined)return 'registered';
                Object.defineProperty(Candidate.prototype,name,{value(){}});
                registry.define(tag,Candidate);
                if(registry.get(tag)!==Candidate)return 'recovery';
            }
            class Observed extends HTMLElement{static observedAttributes=null;attributeChangedCallback(){}}
            try{registry.define('x-null-observed',Observed);return 'observed';}catch(e){if(e.name!=='TypeError')return 'observed-error';}
        }
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn html_context_parsing_restores_document_parts_and_keeps_declarative_registry_null() {
    assert_eq!(
        evaluate(
            r#"
        const frame=document.createElement('iframe');document.body.append(frame);
        const doc=frame.contentDocument;
        doc.documentElement.setHTMLUnsafe('<div id="host"><template shadowrootmode="open" shadowrootclonable shadowrootcustomelementregistry><x-html-context></x-html-context></template></div>');
        if(!doc.head||!doc.body||doc.body.firstElementChild!==doc.getElementById('host'))return 'document-parts';
        const host=doc.getElementById('host'), root=host.shadowRoot;
        if(!root||root.customElementRegistry!==null||root.firstChild.customElementRegistry!==null)return 'shadow';
        const clone=host.cloneNode(true);doc.body.append(clone);
        if(clone.shadowRoot.customElementRegistry!==null||clone.shadowRoot.firstChild.customElementRegistry!==null)return 'clone';
        doc.documentElement.setHTMLUnsafe('<title>title</title><p>body</p>');
        if(doc.head.firstElementChild.localName!=='title'||doc.body.firstElementChild.localName!=='p')return 'head-placement';
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn written_scripts_see_named_hosts_and_scoped_upgrades_before_the_next_script() {
    assert_eq!(
        evaluate(
            r#"
        const frame=document.createElement('iframe');document.body.append(frame);
        const doc=frame.contentDocument, win=frame.contentWindow;
        const first=`window.prefixNamed=typeof writeHost!=='undefined';
            if(prefixNamed){window.scope=new CustomElementRegistry();scope.initialize(writeHost.shadowRoot);
            window.count=0;window.Written=class extends HTMLElement{constructor(){super();count++;}};
            scope.define('x-written-scope',Written);}`;
        const last=`window.beforeNext=window.count===1&&writeHost.shadowRoot.querySelector('x-written-scope') instanceof Written;`;
        doc.open();
        win.__omoikane_parser_update=()=>{throw Error('author hook');};
        doc.write('<!doctype html><html><body><div id="writeHost"><template shadowrootmode="open" shadowrootcustomelementregistry><script>'+first+'</scr'+'ipt><x-written-scope></x-written-scope><script>'+last+'</scr'+'ipt></template></div></body></html>');
        doc.close();
        if(!win.prefixNamed)return 'named';
        if(!win.beforeNext||win.count!==1)return 'parser-timing';
        const element=doc.getElementById('writeHost').shadowRoot.querySelector('x-written-scope');
        if(element.customElementRegistry!==win.scope||!element.matches(':defined'))return 'state';
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn written_parser_nodes_keep_the_http_document_owner_at_private_native_checks() {
    let mut runtime = JsRuntime::with_document_sandbox_and_url(
        TreeBuilder::parse("<body></body>").document(),
        Default::default(),
        "http://same.test/page",
    )
    .unwrap();
    let result = runtime.eval(r#"(()=>{
        const frame=document.createElement('iframe');document.body.append(frame);
        const doc=frame.contentDocument;
        doc.open();
        doc.write('<!doctype html><html><body><div id="httpHost"><template shadowrootmode="open" shadowrootcustomelementregistry><script>window.namedSeen=httpHost.id;</scr'+'ipt><span></span></template></div></body></html>');
        doc.close();
        return frame.contentWindow.namedSeen==='httpHost'&&doc.getElementById('httpHost').shadowRoot.firstChild!==null;
    })()"#).unwrap();
    let errors = runtime.take_task_errors();
    assert!(errors.is_empty(), "unexpected parser errors: {errors:?}");
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn written_template_contents_remain_inert_until_imported_into_a_registry() {
    assert_eq!(
        evaluate(
            r#"
        globalThis.writeUpgrades=0;
        customElements.define('x-inert-written',class extends HTMLElement{constructor(){super();writeUpgrades++;}});
        document.open();
        document.write('<!doctype html><html><body><template id="writtenTemplate"><x-inert-written></x-inert-written></template></body></html>');
        document.close();
        const template=document.getElementById('writtenTemplate'), candidate=template.content.firstChild;
        if(writeUpgrades!==0)return 'early-upgrade';
        if(candidate.customElementRegistry!==null||candidate.matches(':defined'))return 'inert';
        const copy=document.importNode(template.content,true).firstChild;
        if(writeUpgrades!==1||!copy.matches(':defined')||copy.customElementRegistry!==customElements)return 'import';
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn template_registry_reflection_uses_private_attribute_operations() {
    assert_eq!(
        evaluate(
            r#"
        const template=document.createElement('template');
        const original=Element.prototype.setAttribute;
        let calls=0;
        Element.prototype.setAttribute=function(){calls++;throw new Error('author hook');};
        try {
            template.shadowRootCustomElementRegistry='scoped';
        } finally {
            Element.prototype.setAttribute=original;
        }
        if(calls!==0)return 'author-hook';
        if(template.getAttribute('shadowrootcustomelementregistry')!=='scoped')return 'attribute';
        if(template.shadowRootCustomElementRegistry!=='scoped')return 'reflection';
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn registry_string_sequences_reject_primitives_and_array_like_objects() {
    assert_eq!(
        evaluate(
            r#"
        let index=0;
        for(const registry of [customElements,new CustomElementRegistry()]) {
            for(const property of ['observedAttributes','disabledFeatures']) {
                for(const value of ['', 'data-x', 0, true, null, Symbol(), {0:'data-x',length:1}]) {
                    class Candidate extends HTMLElement {attributeChangedCallback(){}}
                    Object.defineProperty(Candidate,property,{value,configurable:true});
                    const name='x-sequence-invalid-'+index++;
                    try{registry.define(name,Candidate);return 'accepted:'+property+':'+typeof value;}
                    catch(e){if(e.name!=='TypeError')return 'error:'+property+':'+e.name;}
                    Object.defineProperty(Candidate,property,{value:[]});
                    registry.define(name,Candidate);
                    if(registry.get(name)!==Candidate)return 'recovery';
                }
            }
        }
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn registry_string_sequences_read_iterator_and_next_once() {
    assert_eq!(
        evaluate(
            r#"
        let index=0;
        for(const registry of [customElements,new CustomElementRegistry()]) {
            for(const property of ['observedAttributes','disabledFeatures']) {
                let getters=0,nextGetters=0,conversions=0,changes=0;
                const sequence={get [Symbol.iterator](){
                    getters++;
                    return function(){
                        if(this!==sequence)throw Error('iterator receiver');
                        let yielded=false;
                        const iterator={get next(){nextGetters++;return function(){
                            if(this!==iterator)throw Error('next receiver');
                            if(yielded)return {done:true};
                            yielded=true;
                            return {done:false,value:{toString(){conversions++;return 'data-observed';}}};
                        };}};
                        return iterator;
                    };
                }};
                class Candidate extends HTMLElement {attributeChangedCallback(){changes++;}}
                Object.defineProperty(Candidate,property,{value:sequence});
                const name='x-sequence-once-'+index++;
                registry.define(name,Candidate);
                const node=document.createElement(name,{customElementRegistry:registry});
                node.setAttribute('data-observed','value');
                if(getters!==1||nextGetters!==1||conversions!==1)return property+':'+[getters,nextGetters,conversions];
                if(changes!==(property==='observedAttributes'?1:0))return 'attribute-callback';
            }
        }
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn registry_string_sequences_apply_dom_string_conversion_to_symbols() {
    assert_eq!(
        evaluate(
            r#"
        let index=0;
        for(const registry of [customElements,new CustomElementRegistry()]) {
            for(const property of ['observedAttributes','disabledFeatures']) {
                class Candidate extends HTMLElement {attributeChangedCallback(){}}
                Object.defineProperty(Candidate,property,{value:[Symbol('invalid')],configurable:true});
                const name='x-sequence-symbol-'+index++;
                try{registry.define(name,Candidate);return 'accepted:'+property;}
                catch(e){if(e.name!=='TypeError')return 'error:'+property+':'+e.name;}
                Object.defineProperty(Candidate,property,{value:[]});
                registry.define(name,Candidate);
                if(registry.get(name)!==Candidate)return 'recovery';
            }
        }
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn cloned_shadow_scripts_preserve_pending_started_and_inert_states() {
    assert_eq!(
        evaluate(
            r#"
        globalThis.cloneScopeRuns=0;
        for(const deep of [false,true]) {
            const host=document.createElement('div');
            const root=host.attachShadow({mode:'open',clonable:true});
            const pending=document.createElement('script');
            pending.text='globalThis.cloneScopeRuns++';
            root.append(pending);
            const before=cloneScopeRuns;
            if(before!==0&&before!==2)return 'detached';
            const copy=host.cloneNode(deep);
            if(!copy.shadowRoot||copy.shadowRoot.firstChild.text!==pending.text)return 'clone';
            document.body.append(copy);
            if(cloneScopeRuns!==before+1)return 'pending:'+cloneScopeRuns;
            document.body.append(host);
            if(cloneScopeRuns!==before+2)return 'original';
            const started=host.cloneNode(deep);
            document.body.append(started);
            const moved=started.shadowRoot.firstChild;
            moved.text='globalThis.cloneScopeRuns+=100';
            document.body.append(moved);
            if(cloneScopeRuns!==before+2)return 'started';
            const inertHost=document.createElement('div');
            const inertRoot=inertHost.attachShadow({mode:'open',clonable:true});
            inertRoot.innerHTML='<script>globalThis.cloneScopeRuns+=1000</scr'+'ipt>';
            const inert=inertHost.cloneNode(deep);
            document.body.append(inert);
            document.body.append(inert.shadowRoot.firstChild);
            if(cloneScopeRuns!==before+2)return 'inert';
        }
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn registry_lookups_accept_callable_names_and_validate_dom_string_arguments() {
    assert_eq!(
        evaluate(
            r#"
        for(const registry of [customElements,new CustomElementRegistry()]) {
            if(registry.getName(()=>{})!==null||registry.getName(Math.max)!==null)return 'callable';
            try{registry.get();return 'missing-name';}catch(e){if(e.name!=='TypeError')return 'missing-error';}
            try{registry.get(Symbol());return 'get-symbol';}catch(e){if(e.name!=='TypeError')return 'get-error';}
            try{registry.define(Symbol(),class extends HTMLElement{});return 'define-symbol';}
            catch(e){if(e.name!=='TypeError')return 'define-error:'+e.name;}
            if(registry.define.length!==2)return 'define-arity';
        }
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn when_defined_rejects_argument_and_receiver_errors_through_promises() {
    let mut runtime =
        JsRuntime::with_document(TreeBuilder::parse("<body></body>").document()).unwrap();
    let initial = runtime
        .eval(
            r#"(()=>{
        globalThis.lookupErrors=[];
        const cases=[
            ()=>customElements.whenDefined(),
            ()=>customElements.whenDefined(Symbol()),
            ()=>CustomElementRegistry.prototype.whenDefined.call({},'x-invalid'),
            ()=>(new CustomElementRegistry()).whenDefined(Symbol()),
        ];
        for(const call of cases){
            let promise;
            try{promise=call();}catch(e){return 'synchronous:'+e.name;}
            if(!(promise instanceof Promise))return 'not-promise';
            promise.then(()=>lookupErrors.push('resolved'),e=>lookupErrors.push(e.name));
        }
        return 'ok';
    })()"#,
        )
        .unwrap();
    assert_eq!(initial.as_string().unwrap().to_std_string_escaped(), "ok");
    runtime.run_jobs().unwrap();
    let settled = runtime.eval("JSON.stringify(lookupErrors)").unwrap();
    assert_eq!(
        settled.as_string().unwrap().to_std_string_escaped(),
        r#"["TypeError","TypeError","TypeError","TypeError"]"#
    );
}

#[test]
fn cloned_documents_keep_scoped_registries_and_upgrade_their_own_candidates() {
    assert_eq!(
        evaluate(
            r#"
        const namespace='http://www.w3.org/1999/xhtml';
        const registry=new CustomElementRegistry();
        registry.define('x-document-clone',class extends HTMLElement{constructor(){super();this.kind='scoped';}});
        for(const source of [document.implementation.createHTMLDocument(),new Document()]) {
            registry.initialize(source);
            if(source.customElementRegistry!==registry)return 'source-registry';
            const root=source.documentElement||source.appendChild(source.createElementNS(namespace,'root'));
            root.append(source.createElementNS(namespace,'x-document-clone'));
            root.append(source.createElementNS(namespace,'x-document-clone',{customElementRegistry:null}));
            for(const deep of [false,true]) {
                const copy=source.cloneNode(deep);
                if(copy.customElementRegistry!==registry)return 'document-registry';
                if(copy.contentType!==source.contentType||copy.defaultView!==null)return 'metadata';
                const created=copy.createElementNS(namespace,'x-document-clone');
                if(created.customElementRegistry!==registry||created.kind!=='scoped'||!created.matches(':defined'))return 'creation';
                if(!deep){if(copy.hasChildNodes())return 'shallow';continue;}
                const children=copy.querySelectorAll('x-document-clone');
                if(children.length!==2)return 'children';
                if(children[0].ownerDocument!==copy||children[0].customElementRegistry!==registry)return 'owner';
                if(children[0].kind!=='scoped'||!children[0].matches(':defined'))return 'upgrade';
                if(children[1].customElementRegistry!==null||children[1].matches(':defined'))return 'explicit-null';
            }
        }
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn cloned_documents_do_not_copy_global_registries_or_upgrade_null_candidates() {
    assert_eq!(
        evaluate(
            r#"
        customElements.define('x-global-document-clone',class extends HTMLElement{});
        document.body.append(document.createElement('x-global-document-clone'));
        const copy=document.cloneNode(true);
        if(copy.customElementRegistry!==null||copy.defaultView!==null)return 'document-registry';
        const child=copy.querySelector('x-global-document-clone');
        if(child.ownerDocument!==copy||child.customElementRegistry!==null||child.matches(':defined'))return 'candidate';
        const created=copy.createElement('x-global-document-clone');
        if(created.customElementRegistry!==null||created.matches(':defined'))return 'creation';
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn cloning_uses_native_children_and_preserves_cross_realm_template_registries() {
    assert_eq!(
        evaluate(
            r#"
        const registry=new CustomElementRegistry();
        registry.define('x-private-clone',class extends HTMLElement{constructor(){super();this.kind='scoped';}});
        const root=document.createElement('div');
        root.append(document.createElement('x-private-clone',{customElementRegistry:registry}));
        Object.defineProperty(root,'childNodes',{get(){throw Error('author tree getter');}});
        const cloned=root.cloneNode(true);
        if(cloned.firstChild.customElementRegistry!==registry||cloned.firstChild.kind!=='scoped')return 'private-tree';
        const frame=document.createElement('iframe');document.body.append(frame);
        const foreign=frame.contentDocument;
        const template=foreign.createElement('template');
        template.append(foreign.createElement('b'));
        template.content.append(foreign.createElement('x-private-clone',{customElementRegistry:registry}));
        Object.defineProperty(template,'content',{get(){throw Error('author content getter');}});
        Object.defineProperty(template,'ownerDocument',{get(){throw Error('author owner getter');}});
        const imported=document.importNode(template,true);
        if(imported.firstChild.localName!=='b'||imported.firstChild.customElementRegistry!==customElements)return 'template-light';
        if(imported.content.firstChild.customElementRegistry!==registry)return 'template-registry';
        if(imported.content.firstChild.kind!=='scoped'||!imported.content.firstChild.matches(':defined'))return 'template-upgrade';
        if(imported.content.ownerDocument===document||imported.content.firstChild.ownerDocument!==imported.content.ownerDocument)return 'template-owner';
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn scoped_definition_internals_are_shared_across_realms_and_copied_controls_submit() {
    assert_eq!(
        evaluate(
            r#"
        const registry=new CustomElementRegistry();
        registry.define('x-foreign-scoped-control',class extends HTMLElement {
            static formAssociated=true;
            constructor(){super();this.i=this.attachInternals();this.i.setFormValue('scoped-value');}
        });
        const frame=document.createElement('iframe');document.body.append(frame);
        const foreign=frame.contentDocument;
        const foreignForm=foreign.createElement('form');foreign.body.append(foreignForm);
        const control=foreign.createElement('x-foreign-scoped-control',{customElementRegistry:registry});
        control.setAttribute('name','answer');foreignForm.append(control);
        if(control.customElementRegistry!==registry||control.i.form!==foreignForm)return 'foreign-internals';
        if(new FormData(foreignForm).get('answer')!=='scoped-value')return 'foreign-submission';
        const form=document.createElement('form');document.body.append(form);
        const copy=document.importNode(control,true);form.append(copy);
        if(copy.customElementRegistry!==registry||copy.i.form!==form||!copy.matches(':defined'))return 'copy-internals';
        if(new FormData(form).get('answer')!=='scoped-value')return 'copy-submission';
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn definition_getters_follow_web_idl_and_html_order_before_registration() {
    assert_eq!(
        evaluate(
            r#"
        for(const registry of [customElements,new CustomElementRegistry()]) {
            const reads=[];
            class Target extends HTMLElement{}
            for(const name of ['connectedCallback','disconnectedCallback','connectedMoveCallback','adoptedCallback','attributeChangedCallback']) {
                Object.defineProperty(Target.prototype,name,{get(){reads.push(name);return name==='attributeChangedCallback'?()=>{}:undefined;}});
            }
            for(const name of ['observedAttributes','disabledFeatures','formAssociated']) {
                Object.defineProperty(Target,name,{get(){reads.push(name);return name==='formAssociated'?false:[];}});
            }
            const constructor=new Proxy(Target,{get(target,name,receiver){if(name==='prototype')reads.push('prototype');return Reflect.get(target,name,receiver);}});
            registry.define('x-definition-order',constructor,{get extends(){reads.push('extends');return undefined;}});
            const expected=['extends','prototype','connectedCallback','disconnectedCallback','connectedMoveCallback','adoptedCallback','attributeChangedCallback','observedAttributes','disabledFeatures','formAssociated'];
            if(reads.join(',')!==expected.join(','))return reads.join(',');
        }
        const scoped=new CustomElementRegistry();
        const constructor=new Proxy(class extends HTMLParagraphElement{}, {get(){throw Error('prototype read');}});
        try{scoped.define('x-no-prototype-read',constructor,{extends:'p'});return 'accepted-extends';}
        catch(e){if(e.name!=='NotSupportedError')return 'extends:'+e.name;}
        try{scoped.define('x-invalid-dictionary',class extends HTMLElement{},1);return 'accepted-dictionary';}
        catch(e){if(e.name!=='TypeError')return 'dictionary:'+e.name;}
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn customized_builtins_keep_native_form_submission_with_form_associated_static_property() {
    assert_eq!(
        evaluate(
            r#"
        const reads=[];
        class NativeInput extends HTMLInputElement {
            static get formAssociated(){reads.push('formAssociated');return true;}
            static get disabledFeatures(){reads.push('disabledFeatures');return [];}
        }
        customElements.define('x-native-form-input',NativeInput,{extends:'input'});
        if(reads.join(',')!=='disabledFeatures,formAssociated')return 'getters';
        const form=document.createElement('form');document.body.append(form);
        const input=document.createElement('input',{is:'x-native-form-input'});
        input.name='answer';input.value='native-value';form.append(input);
        if(new FormData(form).get('answer')!=='native-value')return 'native-submission';
        try{input.attachInternals();return 'internals';}catch(e){if(e.name!=='NotSupportedError')return 'internals-error';}
        return 'ok';
    "#
        ),
        "ok"
    );
}
