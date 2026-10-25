//! Default controls across content attributes, form owners and live styles.
use omoikane::{html::TreeBuilder, js::JsRuntime};

#[test]
fn default_uses_checked_and_selected_content_attributes() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
        for(const type of ['checkbox','radio']) {
            const n=document.createElement('input');n.type=type;n.checked=true;
            if(n.matches(':default'))return false;
            n.defaultChecked=true;n.checked=false;
            if(!n.matches(':default'))return false;
            n.defaultChecked=false;if(n.matches(':default'))return false;
        }
        for(const type of ['text','submit','image','button','hidden']) {
            const n=document.createElement('input');n.type=type;n.setAttribute('checked','');
            if(n.matches(':default'))return false;
        }
        const option=document.createElement('option');option.selected=true;
        if(option.matches(':default'))return false;
        option.defaultSelected=true;option.selected=false;
        if(!option.matches(':default'))return false;
        option.defaultSelected=false;
        return !option.matches(':default');
    })()"#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn default_submit_buttons_follow_form_owner_and_tree_order() {
    let mut runtime=JsRuntime::with_document(TreeBuilder::parse("<input id=external type=submit form=f><form id=f><button id=a>A</button><input id=b type=image><input id=c type=submit></form><button id=orphan type=submit>orphan</button><span id=blocked></span><form id=blocked><button id=other>other</button></form><input id=bad form=blocked type=submit>").document()).unwrap();
    assert_eq!(runtime.eval(r#"(() => {
        const x=document.getElementById('external'),a=document.getElementById('a');
        const b=document.getElementById('b'),c=document.getElementById('c');
        if(!x.matches(':default')||a.matches(':default')||b.matches(':default')||c.matches(':default'))return false;
        if(document.getElementById('orphan').matches(':default')||document.getElementById('bad').matches(':default'))return false;
        if(!document.getElementById('other').matches(':default'))return false;
        x.removeAttribute('form');if(x.matches(':default')||!a.matches(':default'))return false;
        x.setAttribute('form','f');x.disabled=true;
        if(!x.matches(':default')||a.matches(':default'))return false;
        x.type='button';if(!a.matches(':default'))return false;
        a.type='reset';if(!b.matches(':default'))return false;
        b.remove();return c.matches(':default');
    })()"#).unwrap().as_boolean(),Some(true));
}

#[test]
fn default_changes_update_relational_and_filtered_sibling_styles() {
    let mut runtime=JsRuntime::with_document(TreeBuilder::parse("<style>input,button{background-color:transparent;color:green}:default{color:red}form:has(input:default){background-color:blue}:nth-child(2 of :default){background-color:yellow}</style><form id=f><input id=a type=checkbox checked><input id=b type=checkbox><button id=c>submit</button></form>").document()).unwrap();
    assert_eq!(runtime.eval(r#"(() => {
        const a=document.getElementById('a'),b=document.getElementById('b'),c=document.getElementById('c');
        const f=document.getElementById('f'),bg=n=>getComputedStyle(n).backgroundColor;
        if(bg(c)!=='rgb(255, 255, 0)'||bg(f)!=='rgb(0, 0, 255)')return false;
        b.defaultChecked=true;
        if(bg(b)!=='rgb(255, 255, 0)'||bg(c)!=='rgba(0, 0, 0, 0)')return false;
        a.checked=false;
        if(!a.matches(':default')||getComputedStyle(a).color!=='rgb(255, 0, 0)')return false;
        a.defaultChecked=false;
        if(bg(c)!=='rgb(255, 255, 0)')return false;
        b.defaultChecked=false;
        return bg(f)==='rgba(0, 0, 0, 0)'&&bg(c)==='rgba(0, 0, 0, 0)'&&c.matches(':default');
    })()"#).unwrap().as_boolean(),Some(true));
}

#[test]
fn default_respects_namespaces_and_nonfunctional_grammar() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(runtime.eval(r#"(() => {
        for(const ns of [null,'http://www.w3.org/2000/svg']) {
            const n=document.createElementNS(ns,'input');n.setAttribute('type','checkbox');n.setAttribute('checked','');
            if(n.matches(':default'))return false;
        }
        const html='http://www.w3.org/1999/xhtml',doc=document.implementation.createDocument(html,'html',null);
        const n=doc.createElementNS(html,'h:input');n.setAttributeNS(null,'checked','');
        n.setAttributeNS('urn:other','type','checkbox');if(n.matches(':default'))return false;
        n.setAttributeNS(null,'type','checkbox');if(!n.matches(':default'))return false;
        n.removeAttributeNS(null,'checked');n.setAttributeNS('urn:other','checked','');if(n.matches(':default'))return false;
        const o=doc.createElementNS(html,'h:option');o.setAttributeNS('urn:other','selected','');
        if(o.matches(':default'))return false;o.setAttributeNS(null,'selected','');if(!o.matches(':default'))return false;
        if(!CSS.supports('selector(:DEFAULT)'))return false;
        for(const selector of [':default()',':default(value)']) {
            if(CSS.supports(`selector(${selector})`))return false;
            const sheet=new CSSStyleSheet();sheet.replaceSync(`${selector}{color:red}`);if(sheet.cssRules.length)return false;
            try{document.querySelector(selector);return false;}catch(e){if(e.name!=='SyntaxError')return false;}
        }
        return true;
    })()"#).unwrap().as_boolean(),Some(true));
}

#[test]
fn default_submit_classification_honors_auto_commands_and_select_parent() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(runtime.eval(r#"(() => {
        const form=document.createElement('form'),a=document.createElement('button'),b=document.createElement('button');
        form.append(a,b);document.body.append(form);
        if(!a.matches(':default')||b.matches(':default'))return false;
        a.setAttribute('command','show-modal');
        if(a.type!=='button'||a.matches(':default')||!b.matches(':default'))return false;
        a.type='submit';if(a.type!=='submit'||!a.matches(':default'))return false;
        a.removeAttribute('type');a.removeAttribute('command');a.setAttribute('commandfor','missing');
        if(a.type!=='button'||a.matches(':default'))return false;
        a.removeAttribute('commandfor');a.setAttribute('popovertarget','missing');
        if(a.type!=='submit'||!a.matches(':default'))return false;
        const select=document.createElement('select');form.prepend(select);select.append(a);
        if(a.type!=='button'||a.matches(':default')||!b.matches(':default'))return false;
        a.type='submit';
        return a.type==='submit'&&a.matches(':default')&&!b.matches(':default');
    })()"#).unwrap().as_boolean(),Some(true));
}

#[test]
fn default_submit_style_updates_after_type_order_and_form_id_changes() {
    let mut runtime=JsRuntime::with_document(TreeBuilder::parse("<style>button,input{color:green}:default{color:red}form{background-color:transparent}form:has(:default){background-color:blue}</style><input id=x type=submit form=f><form id=f><button id=a>A</button><button id=b>B</button></form>").document()).unwrap();
    assert_eq!(runtime.eval(r#"(() => {
        const x=document.getElementById('x'),a=document.getElementById('a'),b=document.getElementById('b'),f=document.getElementById('f');
        const red=n=>getComputedStyle(n).color==='rgb(255, 0, 0)';
        const blue=()=>getComputedStyle(f).backgroundColor==='rgb(0, 0, 255)';
        if(!red(x)||red(a)||red(b)||blue())return false;
        x.type='button';if(!red(a)||red(b)||!blue())return false;
        a.setAttribute('command','show-modal');if(red(a)||!red(b))return false;
        a.removeAttribute('command');if(!red(a)||red(b))return false;
        a.type='button';if(!red(b))return false;
        const c=document.createElement('button');f.prepend(c);
        if(!red(c)||red(b))return false;c.remove();if(!red(b))return false;
        x.type='submit';if(!red(x)||red(b)||blue())return false;
        f.id='g';if(red(x)||!red(b)||!blue())return false;
        x.setAttribute('form','g');return red(x)&&!red(b)&&!blue();
    })()"#).unwrap().as_boolean(),Some(true));
}

#[test]
fn default_form_owners_keep_exact_utf16_ids_distinct() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(runtime.eval(r#"(() => {
        const a=document.createElement('form'),b=document.createElement('form'),r=document.createElement('form');
        a.id='\ud800';b.id='\ud801';r.id='\ufffd';
        const ca=document.createElement('button'),cb=document.createElement('button'),cr=document.createElement('button');
        a.append(ca);b.append(cb);r.append(cr);
        const x=document.createElement('input'),y=document.createElement('input');x.type=y.type='submit';
        x.setAttribute('form','\ud801');y.setAttribute('form','\ufffd');
        document.body.append(x,y,a,b,r);
        if(document.getElementById('\ud800')!==a||document.getElementById('\ud801')!==b||document.getElementById('\ufffd')!==r)return false;
        if(x.form!==b||y.form!==r)return false;
        return x.matches(':default')&&y.matches(':default')&&ca.matches(':default')&&
            !cb.matches(':default')&&!cr.matches(':default');
    })()"#).unwrap().as_boolean(),Some(true));
}

#[test]
fn default_reflections_and_form_owners_ignore_foreign_namespaces() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(runtime.eval(r#"(() => {
        const n=document.createElement('input');n.type='checkbox';n.setAttributeNS('urn:other','checked','foreign');
        if(n.defaultChecked||n.matches(':default'))return false;
        n.defaultChecked=true;if(!n.defaultChecked||!n.matches(':default'))return false;
        n.defaultChecked=false;if(n.defaultChecked||n.matches(':default')||n.getAttributeNS('urn:other','checked')!=='foreign')return false;
        const o=document.createElement('option');o.setAttributeNS('urn:other','selected','foreign');
        if(o.defaultSelected||o.matches(':default'))return false;
        o.defaultSelected=true;if(!o.defaultSelected||!o.matches(':default'))return false;
        o.defaultSelected=false;if(o.defaultSelected||o.matches(':default'))return false;
        const outer=document.createElement('form'),target=document.createElement('form');target.id='target';
        const a=document.createElement('button'),x=document.createElement('button');
        x.setAttributeNS('urn:other','form','target');outer.append(a,x);
        const svg=document.createElementNS('http://www.w3.org/2000/svg','form');svg.id='foreign-form';
        const y=document.createElement('input');y.type='submit';y.setAttribute('form','foreign-form');
        document.body.append(outer,target,svg,y);
        return x.form===outer&&y.form===null&&a.matches(':default')&&!x.matches(':default')&&!y.matches(':default');
    })()"#).unwrap().as_boolean(),Some(true));
}

#[test]
fn default_form_owners_stay_within_shadow_and_detached_trees() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(runtime.eval(r#"(() => {
        const outer=document.createElement('form');outer.id='f';
        const o=document.createElement('button');outer.append(o);
        const host=document.createElement('div');document.body.append(outer,host);
        const root=host.attachShadow({mode:'open'});
        root.innerHTML='<form id=f><button id=first>A</button><button id=second>B</button></form>';
        const inner=root.querySelector('form'),first=root.querySelector('#first'),second=root.querySelector('#second');
        if(!o.matches(':default')||!first.matches(':default')||second.matches(':default'))return false;
        const external=document.createElement('button');external.setAttribute('form','f');root.prepend(external);
        if(external.form!==inner||!external.matches(':default')||first.matches(':default')||!o.matches(':default'))return false;
        const detached=document.createElement('form'),a=document.createElement('button'),b=document.createElement('button');
        detached.append(a,b);
        if(a.form!==detached||!a.matches(':default')||b.matches(':default'))return false;
        a.type='button';return b.matches(':default');
    })()"#).unwrap().as_boolean(),Some(true));
}
