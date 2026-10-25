//! Placeholder state across type applicability, live values, and selectors.
use omoikane::{html::TreeBuilder, js::JsRuntime};

#[test]
fn placeholder_state_follows_applicable_types_and_empty_values() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(runtime.eval(r#"(() => {
        for (const type of ['text','search','tel','url','email','password','number','unknown']) {
            const n=document.createElement('input'); n.type=type;
            if(n.matches(':placeholder-shown')) return false;
            for(const hint of ['', ' ', 'hint']) {
                n.setAttribute('placeholder',hint); n.value='';
                if(!n.matches(':placeholder-shown')) return false;
                n.value='1'; if(n.matches(':placeholder-shown')) return false;
            }
        }
        for(const type of ['hidden','date','month','week','time','datetime-local','range','color','checkbox','radio','file','button','image','submit','reset']) {
            const n=document.createElement('input'); n.type=type; n.setAttribute('placeholder','hint');
            if(n.matches(':placeholder-shown') !== (n.type==='text')) return false;
        }
        const area=document.createElement('textarea'); area.setAttribute('placeholder','hint');
        if(!area.matches(':placeholder-shown')) return false;
        area.value='1'; return !area.matches(':placeholder-shown');
    })()"#).unwrap().as_boolean(),Some(true));
}

#[test]
fn placeholder_value_changes_update_sibling_and_relational_styles() {
    let mut runtime=JsRuntime::with_document(TreeBuilder::parse("<style>input, textarea {color:green;background-color:transparent} :placeholder-shown {color:red} main:has(:placeholder-shown) {background-color:blue} :nth-child(2 of :placeholder-shown) {background-color:yellow}</style><main id=scope><input id=a placeholder=hint><input id=b placeholder=hint><textarea id=c placeholder=hint></textarea></main>").document()).unwrap();
    assert_eq!(runtime.eval(r#"(() => {
        const a=document.getElementById('a'), b=document.getElementById('b'), c=document.getElementById('c');
        const scope=document.getElementById('scope'), bg=n=>getComputedStyle(n).backgroundColor;
        const initialC=bg(c);
        if(bg(b)!=='rgb(255, 255, 0)' || bg(scope)!=='rgb(0, 0, 255)') return false;
        a.value='filled';
        if(bg(c)!=='rgb(255, 255, 0)' || bg(b)!=='rgba(0, 0, 0, 0)') return false;
        b.type='number'; b.value='12';
        if(bg(c)!==initialC || getComputedStyle(b).color!=='rgb(0, 128, 0)') return false;
        c.value='filled';
        if(bg(scope)!=='rgba(0, 0, 0, 0)') return false;
        b.value='';
        return bg(scope)==='rgb(0, 0, 255)' && b.matches(':placeholder-shown');
    })()"#).unwrap().as_boolean(),Some(true));
}

#[test]
fn placeholder_state_tracks_dirty_values_cloning_and_reset() {
    let mut runtime = JsRuntime::with_document(
        TreeBuilder::parse("<form id=form><input id=a placeholder=hint></form>").document(),
    )
    .unwrap();
    assert_eq!(runtime.eval(r#"(() => {
        const n=document.getElementById('a');
        if(!n.matches(':placeholder-shown')) return false;
        n.setAttribute('value','1'); if(n.matches(':placeholder-shown')) return false;
        n.value=''; n.setAttribute('value','2');
        const clone=n.cloneNode();
        if(!n.matches(':placeholder-shown') || clone.value!=='' || !clone.matches(':placeholder-shown')) return false;
        document.getElementById('form').reset();
        if(n.value!=='2' || n.matches(':placeholder-shown')) return false;
        n.value=''; n.removeAttribute('placeholder');
        return !n.matches(':placeholder-shown');
    })()"#).unwrap().as_boolean(),Some(true));
}

#[test]
fn placeholder_state_obeys_namespaces_and_nonfunctional_grammar() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(runtime.eval(r#"(() => {
        for(const namespace of [null,'http://www.w3.org/2000/svg']) {
            const n=document.createElementNS(namespace,'input'); n.setAttribute('placeholder','hint');
            if(n.matches(':placeholder-shown')) return false;
        }
        const html='http://www.w3.org/1999/xhtml';
        const doc=document.implementation.createDocument(html,'html',null);
        const n=doc.createElementNS(html,'h:input'); n.setAttributeNS('urn:other','placeholder','hint');
        if(n.matches(':placeholder-shown')) return false;
        n.setAttributeNS(null,'placeholder','hint'); if(!n.matches(':placeholder-shown')) return false;
        if(!CSS.supports('selector(:PLACEHOLDER-SHOWN)')) return false;
        for(const selector of [':placeholder-shown()',':placeholder-shown(value)']) {
            if(CSS.supports(`selector(${selector})`)) return false;
            const sheet=new CSSStyleSheet(); sheet.replaceSync(`${selector} { color:red }`);
            if(sheet.cssRules.length) return false;
            try{document.querySelector(selector);return false;}catch(e){if(e.name!=='SyntaxError')return false;}
        }
        return true;
    })()"#).unwrap().as_boolean(),Some(true));
}

#[test]
fn cloned_placeholder_state_uses_internal_values_without_author_getters() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
        const n=document.createElement('input'); n.setAttribute('placeholder','hint');
        n.value='\ud800\r\n';
        Object.defineProperty(n,'__value',{get(){throw new Error('author getter');}});
        Object.defineProperty(HTMLInputElement,Symbol.hasInstance,{
            value(){throw new Error('author hasInstance');},configurable:true
        });
        const clone=n.cloneNode();
        return clone.value.length===1 && clone.value.charCodeAt(0)===0xd800 &&
            !clone.matches(':placeholder-shown');
    })()"#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn placeholder_state_uses_sanitized_input_values_and_type_changes() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(runtime.eval(r#"(() => {
        for(const [type,raw] of [['number','bad'],['number','+1'],['text','\r\n'],
            ['url',' \t\r\n '],['email',' \t\r\n ']]) {
            const n=document.createElement('input'); n.type=type;
            n.setAttribute('placeholder','hint'); n.setAttribute('value',raw);
            if(n.value!=='' || !n.matches(':placeholder-shown')) return false;
            n.value=raw;
            if(n.value!=='' || !n.matches(':placeholder-shown')) return false;
        }
        for(const change of [n=>{n.type='number'},n=>n.setAttribute('type','number'),
            n=>n.setAttributeNS(null,'type','number')]) {
            const n=document.createElement('input'); n.setAttribute('placeholder','hint'); n.value='bad';
            change(n);
            if(n.value!=='' || !n.matches(':placeholder-shown')) return false;
            n.type='text';
            if(n.value!=='' || !n.matches(':placeholder-shown')) return false;
        }
        const n=document.createElement('input');n.type='number';n.setAttribute('placeholder','hint');
        for(const value of ['0','-0','.5','1e2']) {
            n.value=value;
            if(n.value!==value || n.matches(':placeholder-shown')) return false;
        }
        return true;
    })()"#).unwrap().as_boolean(),Some(true));
}

#[test]
fn placeholder_input_value_reflections_ignore_foreign_namespaces() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(runtime.eval(r#"(() => {
        const html='http://www.w3.org/1999/xhtml';
        const doc=document.implementation.createDocument(html,'html',null);
        const n=doc.createElementNS(html,'h:input');
        n.setAttributeNS('urn:other','type','file');
        n.setAttributeNS('urn:other','value','foreign');
        n.setAttributeNS('urn:other','multiple','');
        n.setAttributeNS(null,'placeholder','hint');
        if(n.type!=='text' || n.value!=='' || n.defaultValue!=='' || n.multiple ||
            !n.matches(':placeholder-shown')) return false;
        n.type='number'; n.value='bad'; n.defaultValue='2';
        if(n.type!=='number' || n.value!=='' || n.defaultValue!=='2' ||
            n.getAttributeNS('urn:other','value')!=='foreign' || !n.matches(':placeholder-shown')) return false;
        n.multiple=true; n.type='email'; n.value=' a@b , c@d ';
        if(!n.multiple || n.value!=='a@b,c@d' || n.matches(':placeholder-shown')) return false;
        n.multiple=false;
        return !n.multiple && n.getAttributeNS('urn:other','multiple')==='';
    })()"#).unwrap().as_boolean(),Some(true));
}

#[test]
fn placeholder_textarea_defaults_follow_initial_newline_and_dirty_value_rules() {
    let mut runtime=JsRuntime::with_document(TreeBuilder::parse(
        "<textarea id=a placeholder=hint>\n</textarea><textarea id=b placeholder=hint>\n\n</textarea>"
    ).document()).unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
        const a=document.getElementById('a'),b=document.getElementById('b');
        if(a.value!=='' || !a.matches(':placeholder-shown')) return false;
        if(b.value!=='\n' || b.matches(':placeholder-shown')) return false;
        a.value='\n';
        return a.value==='\n' && !a.matches(':placeholder-shown');
    })()"#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}
