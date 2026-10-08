//! Modern DOM mutation and serialization contracts for Issue #1284.
use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;

fn check(script: &str) {
    let document = TreeBuilder::parse("<!doctype html><html><body></body></html>").document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let result = runtime.eval(&format!("try {{ {script} }} catch (error) {{ globalThis.__p1TestError = String(error); throw error; }}"));
    match result {
        Ok(value) => assert_eq!(value.as_boolean(), Some(true)),
        Err(error) => panic!("{error}: {:?}", runtime.eval("globalThis.__p1TestError")),
    }
}

#[test]
fn mouse_text_selection_uses_original_nodes_across_coalesced_text() {
    check(
        r#"(() => {
        document.body.style.margin = '0';
        const p = document.createElement('p');
        p.style.cssText = 'margin:0;width:300px;font-size:20px';
        const first = document.createTextNode('Alpha'), last = document.createTextNode('β😀');
        p.append(first, document.createComment('retain shaping across nodes'), last);
        document.body.append(p);
        const rect = p.getBoundingClientRect(), y = rect.top + rect.height / 2;
        __omoikane_dispatch_mouse_input(p.__id, 'mousedown', {button:0,buttons:1,clientX:rect.left + .1,clientY:y}, true);
        __omoikane_dispatch_mouse_input(p.__id, 'mousemove', {button:0,buttons:1,clientX:rect.right - .1,clientY:y}, false);
        __omoikane_dispatch_mouse_input(p.__id, 'mouseup', {button:0,buttons:0,clientX:rect.right - .1,clientY:y}, false);
        const selection = getSelection();
        if (selection.anchorNode !== first || selection.anchorOffset !== 0 ||
            selection.focusNode !== last || selection.focusOffset !== 3 || selection.toString() !== 'Alphaβ😀') {
            throw new Error(JSON.stringify({anchor:selection.anchorNode?.data,anchorOffset:selection.anchorOffset,
                focus:selection.focusNode?.data,focusOffset:selection.focusOffset,text:selection.toString(),x:rect.x,y,height:rect.height}));
        }
        return selection.getRangeAt(0).intersectsNode(first) && selection.getRangeAt(0).intersectsNode(last);
    })()"#,
    );
}

#[test]
fn mouse_text_selection_respects_canceled_input_and_synthetic_dispatch() {
    check(
        r#"(() => {
        const p = document.createElement('p'); p.textContent = 'selectable'; document.body.append(p);
        const text = p.firstChild, selection = getSelection(); selection.collapse(text, 2);
        const rect = p.getBoundingClientRect(), init = {button:0,buttons:1,clientX:rect.left + .1,clientY:rect.top + rect.height / 2};
        p.dispatchEvent(new MouseEvent('mousedown', init));
        if (selection.anchorNode !== text || selection.anchorOffset !== 2) return false;
        const cancel = event => event.preventDefault(); p.addEventListener('mousedown', cancel);
        if (__omoikane_dispatch_mouse_input(p.__id, 'mousedown', init, true) !== false) return false;
        __omoikane_dispatch_mouse_input(p.__id, 'mousemove', {...init,clientX:rect.right - .1}, false);
        __omoikane_dispatch_mouse_input(p.__id, 'mouseup', {...init,buttons:0}, false);
        p.removeEventListener('mousedown', cancel);
        if (selection.anchorNode !== text || selection.anchorOffset !== 2 || !selection.isCollapsed) return false;
        __omoikane_dispatch_mouse_input(p.__id, 'mousedown', {...init,button:2,buttons:2}, true);
        __omoikane_dispatch_mouse_input(p.__id, 'mouseup', {...init,button:2,buttons:0}, false);
        return selection.anchorNode === text && selection.anchorOffset === 2 && selection.isCollapsed;
    })()"#,
    );
}

#[test]
fn mouse_text_selection_handles_backward_drags_and_preserves_unmoved_text() {
    check(
        r#"(() => {
        const p = document.createElement('p'); p.style.cssText = 'width:300px;margin:0'; p.textContent = 'backward';
        const moved = document.createElement('span'); moved.textContent = 'other';
        const destination = document.createElement('div'); document.body.append(p, moved, destination);
        const rect = p.getBoundingClientRect(), y = rect.top + rect.height / 2;
        __omoikane_dispatch_mouse_input(p.__id, 'mousedown', {button:0,buttons:1,clientX:rect.right - .1,clientY:y}, true);
        __omoikane_dispatch_mouse_input(p.__id, 'mousemove', {button:0,buttons:1,clientX:rect.left + .1,clientY:y}, false);
        __omoikane_dispatch_mouse_input(p.__id, 'mouseup', {button:0,buttons:0,clientX:rect.left + .1,clientY:y}, false);
        const selection = getSelection();
        if (selection.anchorNode !== p.firstChild || selection.anchorOffset !== 8 || selection.focusOffset !== 0) {
            throw new Error(JSON.stringify({anchor:selection.anchorNode?.data,anchorOffset:selection.anchorOffset,
                focus:selection.focusNode?.data,focusOffset:selection.focusOffset,text:selection.toString(),x:rect.x,y,height:rect.height}));
        }
        destination.moveBefore(moved, null);
        return selection.anchorNode === p.firstChild && selection.focusNode === p.firstChild &&
            selection.anchorOffset === 8 && selection.focusOffset === 0 && selection.toString() === 'backward';
    })()"#,
    );
}

#[test]
fn modern_methods_are_exposed_on_their_standard_interfaces() {
    check(
        r#"(() => {
        const parents = [Element.prototype, DocumentFragment.prototype, Document.prototype];
        const children = [Element.prototype, CharacterData.prototype, DocumentType.prototype];
        for (const prototype of parents) {
            for (const name of ['prepend', 'replaceChildren', 'moveBefore']) {
                if (typeof prototype[name] !== 'function') throw new Error(name + ' missing on ParentNode');
            }
        }
        for (const prototype of children) {
            for (const name of ['before', 'after', 'replaceWith']) {
                if (typeof prototype[name] !== 'function') throw new Error(name + ' missing on ChildNode');
            }
        }
        for (const name of ['before', 'after', 'replaceWith', 'prepend', 'replaceChildren', 'moveBefore']) {
            if (Object.hasOwn(Node.prototype, name)) throw new Error(name + ' leaked onto Node');
        }
        for (const prototype of [Element.prototype, ShadowRoot.prototype]) {
            if (typeof prototype.getHTML !== 'function' || typeof prototype.setHTMLUnsafe !== 'function') return false;
        }
        return typeof Document.prototype.adoptNode === 'function' &&
            typeof Document.parseHTMLUnsafe === 'function';
    })()"#,
    );
}

#[test]
fn sibling_mutators_convert_text_and_preserve_node_identity() {
    check(
        r#"(() => {
        const parent = document.createElement('div');
        const a = document.createElement('a'), b = document.createElement('b');
        parent.append(a, b);
        b.before('before', a);
        b.after('after', null, undefined);
        const nodes = [...parent.childNodes];
        if (nodes.length !== 6 || nodes[0].data !== 'before' || nodes[1] !== a || nodes[2] !== b ||
            nodes[3].data !== 'after' || nodes[4].data !== 'null' || nodes[5].data !== 'undefined') return false;
        a.replaceWith(a, 'replacement', a);
        return parent.childNodes[1].data === 'replacement' && parent.childNodes[2] === a &&
            a.nextSibling === b && a.ownerDocument === document;
    })()"#,
    );
}

#[test]
fn detached_child_mutators_still_perform_web_idl_conversion() {
    check(
        r#"(() => {
        const detached = document.createElement('div');
        for (const method of ['before', 'after', 'replaceWith']) {
            let conversions = 0;
            detached[method]({toString() { conversions++; return 'text'; }});
            if (conversions !== 1 || detached.parentNode !== null) return false;
            try { detached[method](Symbol()); return false; }
            catch (error) { if (!(error instanceof TypeError)) throw error; }
        }
        return true;
    })()"#,
    );
}

#[test]
fn parent_mutators_validate_before_replacing_and_aggregate_mutation_records() {
    check(
        r#"(() => {
        const parent = document.createElement('div');
        const old = document.createElement('span');
        parent.append(old);
        const observer = new MutationObserver(() => {});
        observer.observe(parent, {childList:true});
        try { parent.replaceChildren(parent); return false; }
        catch (error) { if (error.name !== 'HierarchyRequestError') throw error; }
        if (parent.firstChild !== old || observer.takeRecords().length) return false;
        const fresh = document.createElement('b');
        parent.replaceChildren(fresh, 'tail');
        const records = observer.takeRecords();
        if (records.length !== 1 || records[0].target !== parent ||
            records[0].removedNodes.length !== 1 || records[0].removedNodes[0] !== old ||
            records[0].addedNodes.length !== 2 || records[0].addedNodes[0] !== fresh ||
            records[0].addedNodes[1].data !== 'tail' ||
            records[0].previousSibling !== null || records[0].nextSibling !== null) return false;
        parent.prepend('head');
        return parent.firstChild.data === 'head' && parent.childNodes[1] === fresh;
    })()"#,
    );
}

#[test]
fn replace_children_updates_live_ranges_and_empty_calls_remove_children() {
    check(
        r#"(() => {
        const parent = document.createElement('div');
        parent.append('one', 'two');
        const range = document.createRange();
        range.selectNodeContents(parent);
        parent.replaceChildren('new');
        if (range.startContainer !== parent || range.endContainer !== parent ||
            range.startOffset !== 0 || range.endOffset !== 0) return false;
        parent.replaceChildren();
        return parent.childNodes.length === 0 && range.collapsed;
    })()"#,
    );
}

#[test]
fn document_mutators_validate_the_resulting_order() {
    check(
        r#"(() => {
        const doc = document.implementation.createHTMLDocument('title');
        const root = doc.documentElement, doctype = doc.doctype;
        try { doc.prepend(root); return false; }
        catch (error) { if (error.name !== 'HierarchyRequestError') throw error; }
        if (doc.documentElement !== root || doc.firstChild !== doctype) return false;
        const replacement = document.createElement('html');
        doc.replaceChildren(replacement);
        if (doc.documentElement !== replacement || doc.doctype !== null ||
            replacement.ownerDocument !== doc) return false;
        try { doc.replaceChildren('text'); return false; }
        catch (error) { if (error.name !== 'HierarchyRequestError') throw error; }
        return doc.documentElement === replacement;
    })()"#,
    );
}

#[test]
fn sibling_viability_handles_self_and_neighbor_arguments() {
    check(
        r#"(() => {
        const parent = document.createElement('div');
        const a = document.createElement('a'), b = document.createElement('b'), c = document.createElement('c');
        parent.append(a,b,c);
        b.before(a,b,'x');
        if (parent.childNodes[0] !== a || parent.childNodes[1] !== b ||
            parent.childNodes[2].data !== 'x' || parent.lastChild !== c) return false;
        b.after(c,b,'y');
        if (parent.childNodes[0] !== a || parent.childNodes[1] !== c ||
            parent.childNodes[2] !== b || parent.childNodes[3].data !== 'y') return false;
        b.replaceWith();
        return b.parentNode === null && parent.childNodes.length === 4;
    })()"#,
    );
}

#[test]
fn move_before_preserves_custom_element_connection_and_reports_two_records() {
    check(
        r#"(() => {
        const log=[];
        class Moving extends HTMLElement {
            connectedCallback() { log.push('connected'); }
            disconnectedCallback() { log.push('disconnected'); }
            connectedMoveCallback() { log.push('moved'); }
        }
        customElements.define('p1-moving',Moving);
        const a=document.createElement('div'), b=document.createElement('div');
        document.body.append(a,b);
        const node=document.createElement('p1-moving');
        a.append(node);
        log.length=0;
        const observer=new MutationObserver(()=>{});
        observer.observe(document.body,{subtree:true,childList:true});
        b.moveBefore(node,null);
        const records=observer.takeRecords();
        return log.join(',')==='moved' && node.parentNode===b &&
            records.length===2 && records[0].target===a && records[0].removedNodes[0]===node &&
            records[1].target===b && records[1].addedNodes[0]===node;
    })()"#,
    );
}

#[test]
fn move_before_requires_a_shared_root_and_valid_reference() {
    check(
        r#"(() => {
        const parent=document.createElement('div'), node=document.createElement('span');
        document.body.append(parent);
        try { parent.moveBefore(node,null); return false; }
        catch(error) { if(error.name!=='HierarchyRequestError') throw error; }
        parent.append(node);
        try { parent.moveBefore(node,document.body); return false; }
        catch(error) { if(error.name!=='NotFoundError') throw error; }
        parent.moveBefore(node,node);
        return parent.firstChild===node && parent.lastChild===node;
    })()"#,
    );
}

#[test]
fn move_before_preserves_iframe_document_and_realm_state() {
    check(
        r#"(() => {
        const a=document.createElement('div'), b=document.createElement('div');
        document.body.append(a,b);
        const frame=document.createElement('iframe');
        frame.srcdoc='<p>child</p>';
        a.append(frame);
        const win=frame.contentWindow, doc=frame.contentDocument;
        win.eval('globalThis.moveMarker={value:42}; document.body.marker=43;');
        b.moveBefore(frame,null);
        return frame.contentWindow===win && frame.contentDocument===doc &&
            win.eval('moveMarker.value===42 && document.body.marker===43');
    })()"#,
    );
}

#[test]
fn move_before_updates_ranges_at_removal_and_insertion_boundaries() {
    check(
        r#"(() => {
        const a=document.createElement('div'), b=document.createElement('div');
        document.body.append(a,b);
        const node=document.createElement('span');
        node.append('abc'); a.append(node,'tail'); b.append('first','second');
        const inside=document.createRange(); inside.selectNodeContents(node.firstChild);
        const destination=document.createRange(); destination.setStart(b,2); destination.collapse(true);
        b.moveBefore(node,b.childNodes[1]);
        return inside.startContainer===a && inside.endContainer===a && inside.startOffset===0 &&
            destination.startContainer===b && destination.startOffset===3 && destination.endOffset===3;
    })()"#,
    );
}

#[test]
fn adoption_preserves_identity_updates_shadow_attributes_and_detaches() {
    check(
        r#"(() => {
        const target=document.implementation.createHTMLDocument('target');
        const node=document.createElement('div'); node.setAttribute('data-x','value');
        const attr=node.getAttributeNode('data-x');
        node.append('text'); const child=node.firstChild;
        const shadow=node.attachShadow({mode:'closed'}); shadow.append('shadow');
        document.body.append(node);
        const observer=new MutationObserver(()=>{}); observer.observe(document.body,{childList:true});
        const range=document.createRange(); range.selectNodeContents(child);
        const index=[...document.body.childNodes].indexOf(node);
        if(target.adoptNode(node)!==node || node.parentNode!==null || node.firstChild!==child) return false;
        const records=observer.takeRecords();
        return node.ownerDocument===target && child.ownerDocument===target && attr.ownerDocument===target &&
            shadow.ownerDocument===target && shadow.firstChild.ownerDocument===target &&
            records.length===1 && records[0].removedNodes[0]===node &&
            range.startContainer===document.body && range.startOffset===index;
    })()"#,
    );
}

#[test]
fn adoption_calls_custom_element_reactions_with_old_and_new_documents() {
    check(
        r#"(() => {
        const log=[]; const target=document.implementation.createHTMLDocument('target');
        class Adopted extends HTMLElement {
            connectedCallback() { log.push('connected'); }
            disconnectedCallback() { log.push('disconnected'); }
            adoptedCallback(oldDoc,newDoc) { log.push(oldDoc===document && newDoc===target ? 'adopted':'bad'); }
        }
        customElements.define('p1-adopted',Adopted);
        const node=document.createElement('p1-adopted'); document.body.append(node); log.length=0;
        target.adoptNode(node);
        if(log.join(',')!=='disconnected,adopted') return false;
        log.length=0; target.adoptNode(node);
        return log.length===0;
    })()"#,
    );
}

#[test]
fn adoption_validates_nodes_and_handles_template_contents() {
    check(
        r#"(() => {
        const target=document.implementation.createHTMLDocument('target');
        try { target.adoptNode(document); return false; }
        catch(error) { if(error.name!=='NotSupportedError') throw error; }
        const host=document.createElement('div'), root=host.attachShadow({mode:'open'});
        try { target.adoptNode(root); return false; }
        catch(error) { if(error.name!=='HierarchyRequestError') throw error; }
        try { target.adoptNode({nodeType:1}); return false; }
        catch(error) { if(!(error instanceof TypeError)) throw error; }
        const template=document.createElement('template'); template.innerHTML='<span>text</span>';
        const contents=template.content, child=contents.firstChild;
        if(target.adoptNode(contents)!==contents || contents.ownerDocument!==target || child.ownerDocument!==target) return false;
        return template.ownerDocument===document && contents.firstChild===child &&
            template.content===contents && template.content.ownerDocument===target;
    })()"#,
    );
}

#[test]
fn get_html_selects_serializable_and_explicit_closed_shadow_roots() {
    check(
        r#"(() => {
        const host=document.createElement('div'); host.append('light');
        const closed=host.attachShadow({mode:'closed',serializable:true,clonable:true,delegatesFocus:true});
        closed.innerHTML='<b>shadow</b>';
        const expected='<template shadowrootmode="closed" shadowrootdelegatesfocus="" shadowrootserializable="" shadowrootclonable=""><b>shadow</b></template>light';
        if(host.getHTML()!=='light' || host.getHTML({serializableShadowRoots:true})!==expected ||
            host.getHTML({shadowRoots:[closed]})!==expected || closed.getHTML()!=='<b>shadow</b>') return false;
        return closed.serializable && closed.clonable && closed.delegatesFocus && closed.slotAssignment==='named';
    })()"#,
    );
}

#[test]
fn get_html_handles_nested_selected_roots_raw_text_voids_and_templates() {
    check(
        r#"(() => {
        const host=document.createElement('div');
        const outer=host.attachShadow({mode:'open'});
        const nested=document.createElement('section'); outer.append(nested);
        const inner=nested.attachShadow({mode:'closed',serializable:true}); inner.innerHTML='<i>inner</i>';
        const expected='<template shadowrootmode="open"><section><template shadowrootmode="closed" shadowrootserializable=""><i>inner</i></template></section></template>';
        if(host.getHTML({serializableShadowRoots:true,shadowRoots:[outer]})!==expected) return false;
        const markup=document.createElement('div');
        markup.innerHTML='<br><script>if (a < b && c > d) {}</script><template><p>A &amp; B</p></template>';
        return markup.getHTML()==='<br><script>if (a < b && c > d) {}</script><template><p>A &amp; B</p></template>';
    })()"#,
    );
}

#[test]
fn get_html_retains_declarative_shadow_root_serialization_flags() {
    let document=TreeBuilder::parse(r#"<div id="host"><template shadowrootmode="closed" shadowrootserializable shadowrootclonable><span>inside</span></template>outside</div>"#).document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    assert_eq!(runtime.eval(r#"document.getElementById('host').getHTML({serializableShadowRoots:true}) === '<template shadowrootmode="closed" shadowrootserializable="" shadowrootclonable=""><span>inside</span></template>outside'"#).unwrap().as_boolean(),Some(true));
}

#[test]
fn unsafe_html_parses_declarative_roots_and_aggregates_replacement_records() {
    check(
        r#"(() => {
        const target=document.createElement('div'); target.append('old'); document.body.append(target);
        const old=target.firstChild;
        const observer=new MutationObserver(()=>{}); observer.observe(target,{childList:true});
        target.setHTMLUnsafe('<section><template shadowrootmode="closed" shadowrootserializable><b>shadow</b></template>light</section>');
        const records=observer.takeRecords();
        return records.length===1 && records[0].removedNodes[0]===old && records[0].addedNodes[0]===target.firstChild &&
            target.firstChild.getHTML({serializableShadowRoots:true})===
            '<template shadowrootmode="closed" shadowrootserializable=""><b>shadow</b></template>light';
    })()"#,
    );
}

#[test]
fn unsafe_html_handles_direct_host_shadow_root_and_template_targets() {
    check(
        r#"(() => {
        const host=document.createElement('div');
        host.setHTMLUnsafe('<template shadowrootmode="open"><span>direct</span></template>light');
        const root=host.shadowRoot;
        if(!root || root.firstChild.textContent!=='direct' || host.textContent!=='light') throw new Error('direct: '+host.getHTML({serializableShadowRoots:true})+' / '+(root && root.textContent));
        root.setHTMLUnsafe('<section><template shadowrootmode="open"><i>nested</i></template></section>');
        if(root.firstChild.shadowRoot.firstChild.textContent!=='nested') throw new Error('nested');
        host.setHTMLUnsafe('<template shadowrootmode="closed">duplicate</template>');
        if(host.shadowRoot!==root || host.firstChild.localName!=='template') throw new Error('existing root');
        const template=document.createElement('template'); template.setHTMLUnsafe('<p>contents</p>');
        return template.childNodes.length===0 && template.content.firstChild.textContent==='contents';
    })()"#,
    );
}

#[test]
fn unsafe_document_parse_is_inert_and_has_an_independent_document() {
    check(
        r#"(() => {
        const parsed=Document.parseHTMLUnsafe('<!doctype html><title>title</title><div id="host"><template shadowrootmode="open" shadowrootserializable><span>shadow</span></template></div><script>globalThis.unsafeRan=true</script>');
        if(parsed===document || parsed.defaultView!==null || parsed.location!==null || parsed.URL!=='about:blank' ||
            parsed.contentType!=='text/html' || parsed.title!=='title' || parsed.documentElement.ownerDocument!==parsed) return false;
        if(parsed.getElementById('host').shadowRoot.firstChild.ownerDocument!==parsed) return false;
        document.body.append(parsed.querySelector('script'));
        return globalThis.unsafeRan===undefined;
    })()"#,
    );
}

#[test]
fn unsafe_html_uses_raw_text_context_and_preserves_legacy_inner_html() {
    check(
        r#"(() => {
        const script=document.createElement('script'); script.setHTMLUnsafe('if (a < b) {}');
        if(script.textContent!=='if (a < b) {}') return false;
        const legacy=document.createElement('div'); legacy.innerHTML='<section><template shadowrootmode="open">plain</template></section>';
        if(legacy.firstChild.shadowRoot!==null || legacy.firstChild.firstChild.localName!=='template') return false;
        const target=document.createElement('div');
        target.setHTMLUnsafe('<script>globalThis.unsafeRan=true</script>'); document.body.append(target);
        return globalThis.unsafeRan===undefined;
    })()"#,
    );
}

#[test]
fn unsafe_document_parsing_disables_scripting_and_reports_mode_namespace_and_base() {
    check(
        r#"(() => {
        const parsed=Document.parseHTMLUnsafe('<body><noscript><p id="a">A<p id="b">B</noscript>');
        if(parsed.compatMode!=='BackCompat' || parsed.documentElement.namespaceURI!=='http://www.w3.org/1999/xhtml') return false;
        if(parsed.querySelector('noscript').childNodes.length!==2 || parsed.getElementById('b').textContent!=='B') return false;
        if(parsed.baseURI!=='about:blank' || parsed.body.baseURI!=='about:blank') return false;
        const standard=Document.parseHTMLUnsafe('<!doctype html><base href="https://example.test/base/">');
        return standard.compatMode==='CSS1Compat' && standard.baseURI==='https://example.test/base/';
    })()"#,
    );
}

#[test]
fn unsafe_document_mode_uses_initial_doctype_and_legacy_identifiers() {
    check(
        r#"(() => {
        for(const markup of ['<!DOCTYPE foo>','<!DOCTYPE html PUBLIC "HTML">',
            '<!DOCTYPE html PUBLIC "-//W3C//DTD HTML 4.01 Transitional//EN">',
            '<!DOCTYPE html SYSTEM "http://www.ibm.com/data/dtd/v11/ibmxhtml1-transitional.dtd">',
            '<!DOCTYPE html PUBLIC >']) {
            if(Document.parseHTMLUnsafe(markup).compatMode!=='BackCompat') return false;
        }
        for(const markup of ['<!DOCTYPE html>', '<!DOCTYPE html><!DOCTYPE foo>',
            '<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.0 Transitional//EN">',
            '<!DOCTYPE html PUBLIC "-//W3C//DTD HTML 4.01 Transitional//EN" "legacy.dtd">']) {
            if(Document.parseHTMLUnsafe(markup).compatMode!=='CSS1Compat') return false;
        }
        return true;
    })()"#,
    );
}

#[path = "support/http_fixture.rs"]
mod http_fixture;

#[test]
fn unsafe_document_parsing_is_available_in_an_iframe_external_script() {
    use http_fixture::{
        FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback, read_request_headers,
    };
    use std::io::Write;
    let listener = bind_loopback().unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let worker = FixtureWorker::spawn(move || {
        let mut requests = 0;
        for _ in 0..2 {
            let Ok(mut stream) = accept_with_timeout(&listener, http_fixture::ACCEPT_TIMEOUT)
            else {
                break;
            };
            let request = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
            let path = request.split_whitespace().nth(1).unwrap();
            let (mime, body) = match path {
                "/frame.html" => (
                    "text/html",
                    "<!doctype html><script src='/parse.js'></script>",
                ),
                "/parse.js" => (
                    "text/javascript",
                    "window.doParse = html => Document.parseHTMLUnsafe(html); window.externalScriptDocument = document;",
                ),
                _ => panic!("unexpected request: {path}"),
            };
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            requests += 1;
        }
        requests
    });
    let document =
        TreeBuilder::parse("<!doctype html><iframe src='/frame.html'></iframe>").document();
    let mut runtime = JsRuntime::with_document_and_url(document, &origin).unwrap();
    let base = origin.parse().unwrap();
    assert!(runtime.execute_document_scripts(Some(&base)).is_empty());
    runtime.run_until_idle().unwrap();
    for _ in 0..8 {
        runtime.run_timers(16, 16, 128);
        runtime.run_until_idle().unwrap();
    }
    let result = runtime
        .eval(
            r#"JSON.stringify((() => {
        const frame=document.querySelector('iframe');
        if(typeof frame.contentWindow.doParse!=='function') return {missing: true};
        const parsed=frame.contentWindow.doParse('<body><p>child</p>');
        return {url: parsed.URL, base: parsed.baseURI, view: parsed.defaultView,
          text: parsed.body.firstChild.textContent,
          sameDocument: frame.contentWindow.externalScriptDocument===frame.contentDocument,
          parentLeak: globalThis.doParse!==undefined};
    })())"#,
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    let result: serde_json::Value = serde_json::from_str(&result).unwrap();
    let errors = runtime.take_task_errors();
    let detail = runtime
        .eval("document.querySelector('iframe').contentDocument.documentElement.outerHTML")
        .unwrap();
    assert_eq!(
        worker.join(),
        2,
        "external script must actually be fetched: errors={errors:?}, child={detail:?}"
    );
    assert_eq!(
        result,
        serde_json::json!({
            "url": "about:blank", "base": "about:blank", "view": null,
            "text": "child", "sameDocument": true, "parentLeak": false,
        })
    );
}

#[test]
fn unsafe_document_parsing_uses_utf8_for_string_input_and_ignores_meta_encoding() {
    check(
        r#"(() => {
        for (const html of ['', '<meta charset="latin2">é', '<?xml version="1.0" encoding="latin2"?><x/>']) {
            const parsed=Document.parseHTMLUnsafe(html);
            if(parsed.charset!=='UTF-8' || parsed.characterSet!=='UTF-8' || parsed.inputEncoding!=='UTF-8') return false;
        }
        return true;
    })()"#,
    );
}

#[test]
fn encoding_metadata_comes_from_the_decoder_and_survives_meta_changes() {
    let parsed =
        TreeBuilder::parse_bytes(b"<meta charset=windows-1252><p>\xe9</p>", None).document();
    assert_eq!(
        parsed.document_character_encoding().as_deref(),
        Some("windows-1252")
    );
    let mut runtime = JsRuntime::with_document(parsed).unwrap();
    let result = runtime.eval(r#"JSON.stringify((() => {
        const original = {charset: document.charset, characterSet: document.characterSet,
          inputEncoding: document.inputEncoding, text: document.querySelector('p').textContent};
        document.querySelector('meta').setAttribute('charset', 'utf-8');
        const generated=Document.parseHTMLUnsafe('<meta charset=latin2>é');
        return {original, after: document.characterSet, generated: generated.characterSet,
          generatedInputEncoding: generated.inputEncoding, generatedText: generated.body.textContent};
    })())"#).unwrap().as_string().unwrap().to_std_string_escaped();
    let result: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(
        result,
        serde_json::json!({
            "original": {"charset": "windows-1252", "characterSet": "windows-1252",
              "inputEncoding": "windows-1252", "text": "é"},
            "after": "windows-1252", "generated": "UTF-8",
            "generatedInputEncoding": "UTF-8", "generatedText": "é",
        })
    );
}

#[test]
fn encoding_metadata_preserves_transport_precedence_and_document_clones() {
    let document = TreeBuilder::parse_bytes(
        b"<!doctype html><meta charset=utf-8><p>\xe9</p>",
        Some("text/html; charset=windows-1252"),
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
        const clone=document.cloneNode(true);
        return document.characterSet==='windows-1252' && clone.characterSet==='windows-1252' &&
          clone.querySelector('p').textContent==='é' && new Document().characterSet==='UTF-8';
    })()"#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn range_intersection_tracks_move_boundaries_and_distinct_roots() {
    check(
        r#"(() => {
        const parent=document.createElement('div');
        parent.innerHTML='<span>a</span><span>b</span><span>c</span>';
        document.body.append(parent);
        const [a,b,c]=parent.children;
        const range=new Range(); range.setStart(a,0); range.setEnd(c,0);
        if (!range.intersectsNode(b) || !range.intersectsNode(document) ||
            range.intersectsNode(document.createElement('div'))) return false;
        parent.moveBefore(a,null);
        if (range.startContainer!==parent || range.endContainer!==c || !range.intersectsNode(b)) return false;
        const point=new Range(); point.setStart(parent,1); point.collapse(true);
        if (point.intersectsNode(parent.children[0]) || point.intersectsNode(parent.children[1])) return false;
        for(const value of [null, {}, {nodeType:1}]) {
            try { range.intersectsNode(value); return false; } catch(e) { if(!(e instanceof TypeError)) return false; }
        }
        try { range.intersectsNode(); return false; } catch(e) { if(!(e instanceof TypeError)) return false; }
        return true;
    })()"#,
    );
}

#[test]
fn moved_focus_fixup_runs_later_and_respects_subsequent_focus_changes() {
    for subsequent in ["", "visible.moveBefore(button,null)", "other.focus()"] {
        let document = TreeBuilder::parse("<!doctype html><div id=visible><button id=button>A</button><button id=other>B</button></div><div id=hidden hidden></div>").document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        assert_eq!(
            runtime
                .eval(
                    r#"globalThis.events=[];
            for(const id of ['button','other','visible','hidden']) globalThis[id]=document.getElementById(id);
            button.addEventListener('blur',()=>events.push('blur'));
            button.addEventListener('focusout',()=>events.push('focusout'));
            button.focus(); hidden.moveBefore(button,null);
            document.activeElement===button && events.length===0"#
                )
                .unwrap()
                .as_boolean(),
            Some(true)
        );
        if !subsequent.is_empty() {
            runtime.eval(subsequent).unwrap();
        }
        runtime.run_until_idle().unwrap();
        let result = runtime
            .eval("JSON.stringify({active:document.activeElement===document.body?'body':document.activeElement===button?'button':document.activeElement===other?'other':'unexpected',events})")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        let result: serde_json::Value = serde_json::from_str(&result).unwrap();
        let (active, events) = match subsequent {
            "" => ("body", vec!["blur", "focusout"]),
            "visible.moveBefore(button,null)" => ("button", vec![]),
            _ => ("other", vec!["blur", "focusout"]),
        };
        assert_eq!(
            result,
            serde_json::json!({"active":active,"events":events}),
            "subsequent={subsequent}"
        );
    }
}

#[test]
fn public_character_data_constructors_produce_canonical_mutation_nodes() {
    check(
        r#"(() => {
        const parent=document.createElement('div'); document.body.append(parent);
        for(const [Ctor,type] of [[Text,3],[Comment,8]]) {
            for(const [input,expected] of [[undefined,''],[null,'null'],[42,'42'],['data','data']]) {
                const node=new Ctor(input);
                if(!(node instanceof Ctor) || node.nodeType!==type || node.data!==expected || node.ownerDocument!==document) return false;
                parent.prepend(node);
                if(parent.firstChild!==node) return false;
                document.body.moveBefore(node,null);
                if(document.body.lastChild!==node || node.parentNode!==document.body) return false;
                node.remove();
            }
            try { new Ctor(Symbol()); return false; } catch(e) { if(!(e instanceof TypeError)) return false; }
            class Derived extends Ctor {}
            const derived=new Derived('subclass'); parent.append(derived);
            if(!(derived instanceof Derived) || parent.lastChild!==derived || derived.data!=='subclass') return false;
        }
        return true;
    })()"#,
    );
}

#[test]
fn modern_parent_mutators_accept_foreign_realm_nodes_without_accepting_forged_nodes() {
    check(
        r#"(() => {
        const frame=document.createElement('iframe'); document.body.append(frame);
        const foreign=frame.contentWindow.Function('return document.createTextNode("foreign")')();
        const parent=document.createElement('div'); document.body.append(parent);
        parent.append(foreign);
        if(parent.firstChild!==foreign || parent.textContent!=='foreign') return false;
        document.body.moveBefore(foreign,null);
        if(document.body.lastChild!==foreign) return false;
        const fake={__id:foreign.__id,nodeType:3};
        Object.setPrototypeOf(fake,frame.contentWindow.Text.prototype);
        try { document.body.moveBefore(fake,null); return false; } catch(e) { if(!(e instanceof TypeError)) return false; }
        return true;
    })()"#,
    );
}

#[test]
fn iframe_evaluation_and_function_creation_use_the_child_document() {
    let document =
        TreeBuilder::parse("<!doctype html><iframe srcdoc='<p>child</p>'></iframe>").document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let result = runtime
        .eval(
            r#"JSON.stringify((() => {
        const frame=document.querySelector('iframe');
        const child=frame.contentDocument;
        return {evalDocument:frame.contentWindow.eval('document')===child,
          functionDocument:frame.contentWindow.Function('return document')()===child,
          evalView:frame.contentWindow.eval('document.defaultView===window'),
          evalThis:frame.contentWindow.eval('this===document.defaultView'),
          functionThis:frame.contentWindow.Function('return this===document.defaultView')(),
          functionView:frame.contentWindow.Function('return document.defaultView===globalThis')(),
          externalView:child.defaultView===frame.contentWindow,
          childPrototype:Object.getPrototypeOf(child)===frame.contentWindow.Document.prototype};
    })())"#,
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    let result: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(
        result,
        serde_json::json!({"evalDocument":true,"functionDocument":true,
      "evalView":true,"evalThis":true,"functionThis":true,"functionView":true,
      "externalView":true,"childPrototype":true})
    );
}

#[test]
fn iframe_window_proxy_uses_its_own_prototype_and_accessor_receivers() {
    check(
        r#"(() => {
        const frame=document.createElement('iframe'); document.body.append(frame);
        return frame.contentWindow.eval(`(() => {
            globalThis.counter=1; window.counter++;
            let setterReceiver;
            Object.defineProperty(window,'receiverProbe', {
                configurable:true,
                get() { return this; },
                set(value) { setterReceiver=this; this.assignedValue=value; }
            });
            window.receiverProbe=42;
            return counter===2 && assignedValue===42 && setterReceiver===window &&
                window.receiverProbe===window && Object.getPrototypeOf(window)===Window.prototype &&
                Object.getPrototypeOf(window).constructor.constructor('return this')()===window;
        })()`);
    })()"#,
    );
}

#[test]
fn nested_iframe_scripts_observe_the_parent_window_proxy() {
    check(
        r#"(() => {
        const outer=document.createElement('iframe'); document.body.append(outer);
        const inner=outer.contentDocument.createElement('iframe'); outer.contentDocument.body.append(inner);
        const parentSeenByChild=inner.contentWindow.eval('parent');
        return parentSeenByChild===outer.contentWindow &&
            inner.contentWindow.eval('parent === window.parent') &&
            inner.contentWindow.eval('parent.document.defaultView === parent');
    })()"#,
    );
}

#[test]
fn mouse_action_client_dimensions_are_integer_without_rounding_dom_rects() {
    check(
        r#"(() => {
        const target = document.createElement('div');
        target.style.cssText = 'width:10.75px;height:20.25px;padding:0;border:0;margin:0';
        document.body.append(target);
        const rect = target.getBoundingClientRect();
        if (rect.width !== 10.75 || rect.height !== 20.25)
            throw new Error('DOMRect lost fractional CSS pixels: ' + rect.width + ',' + rect.height);
        if (target.clientWidth !== 11 || target.clientHeight !== 20)
            throw new Error('client dimensions must be rounded integers: ' + target.clientWidth + ',' + target.clientHeight);
        target.style.width = '10.25px';
        target.style.height = '20.75px';
        return target.clientWidth === 10 && target.clientHeight === 21 &&
            target.getBoundingClientRect().width === 10.25 &&
            target.getBoundingClientRect().height === 20.75;
    })()"#,
    );
}

#[test]
fn unsafe_html_in_xml_documents_keeps_html_namespaces_and_document_mime() {
    check(
        r#"(() => {
        const html = 'http://www.w3.org/1999/xhtml';
        const svg = 'http://www.w3.org/2000/svg';
        for (const [namespace, mime] of [[null, 'application/xml'],
            [svg, 'image/svg+xml'], [html, 'application/xhtml+xml']]) {
            const doc = document.implementation.createDocument(namespace, 'root');
            if (doc.contentType !== mime) throw new Error('wrong XML MIME: ' + doc.contentType);
            doc.documentElement.setHTMLUnsafe('<p><foo><b><i>test</b></i>');
            const paragraph = doc.documentElement.firstChild;
            if (paragraph.namespaceURI !== html || paragraph.ownerDocument !== doc) return false;
            if (doc.documentElement.innerHTML !==
                '<p xmlns="http://www.w3.org/1999/xhtml"><foo><b><i>test</i></b></foo></p>')
                throw new Error('XML serialization lost HTML namespace');
            const template = doc.createElementNS(html, 'template');
            template.setHTMLUnsafe('<br>');
            if (template.innerHTML !== '<br xmlns="http://www.w3.org/1999/xhtml" />')
                throw new Error('XML template contents missing: ' + template.innerHTML);
        }
        return true;
    })()"#,
    );
}

#[test]
fn html_serialization_reads_sequence_iterator_once_and_closes_on_conversion_error() {
    check(
        r#"(() => {
        const host = document.createElement('div');
        const root = host.attachShadow({mode:'closed'});
        root.textContent = 'closed';
        let reads = 0;
        const order = [];
        const sequence = {
            get [Symbol.iterator]() {
                if (++reads !== 1) throw new Error('iterator getter read twice');
                return function() {
                    if (this !== sequence) throw new Error('iterator receiver lost');
                    return [root][Symbol.iterator]();
                };
            }
        };
        const options = {
            get serializableShadowRoots() { order.push('serializable'); return false; },
            get shadowRoots() { order.push('roots'); return sequence; }
        };
        if (host.getHTML(options) !== '<template shadowrootmode="closed">closed</template>' ||
            reads !== 1 || order.join(',') !== 'serializable,roots') return false;
        let closed = 0;
        const invalid = {
            [Symbol.iterator]() {
                return {next() { return {done:false,value:document.body}; },
                    return() { closed++; return {done:true}; }};
            }
        };
        try { host.getHTML({shadowRoots:invalid}); return false; }
        catch (error) { if (!(error instanceof TypeError)) return false; }
        return closed === 1;
    })()"#,
    );
}

#[test]
fn unsafe_html_run_scripts_observe_complete_tree_before_custom_reactions() {
    check(
        r#"(() => {
        const host = document.createElement('div'); host.id='unsafe-host'; document.body.append(host);
        globalThis.unsafeOperations = [];
        customElements.define('unsafe-first', class extends HTMLElement {
            constructor() { super(); unsafeOperations.push('constructor'); }
            connectedCallback() { unsafeOperations.push('connected'); }
        });
        host.setHTMLUnsafe('<script>unsafeOperations.push("inert");</script>');
        if (unsafeOperations.length) return false;
        host.setHTMLUnsafe('<unsafe-first></unsafe-first><script>' +
            'if (!document.getElementById("unsafe-after")) throw new Error("partial tree");' +
            'if (document.currentScript.parentNode !== document.getElementById("unsafe-host")) throw new Error("currentScript");' +
            'unsafeOperations.push("script");</script><div id="unsafe-after"></div>',
            {get runScripts() { unsafeOperations.push('option'); return true; }});
        return unsafeOperations.join(',') === 'option,script,constructor,connected';
    })()"#,
    );
}

#[test]
fn unsafe_html_run_scripts_include_shadow_trees_and_skip_removed_scripts() {
    check(
        r#"(() => {
        const host = document.createElement('div'); document.body.append(host);
        const root = host.attachShadow({mode:'closed'}); globalThis.unsafeRoot=root;
        globalThis.unsafeRuns = [];
        root.setHTMLUnsafe('<div><template shadowrootmode="open"><script>' +
            'unsafeRuns.push("shadow");</script></template></div>' +
            '<script>unsafeRuns.push("first");unsafeRoot.getElementById("unsafe-next").remove();</script>' +
            '<script id="unsafe-next">unsafeRuns.push("removed");</script>', {runScripts:true});
        if (unsafeRuns.join(',') !== 'shadow,first') return false;
        const script = root.querySelector('script');
        script.remove(); root.append(script);
        return unsafeRuns.join(',') === 'shadow,first';
    })()"#,
    );
}

#[test]
fn unsafe_html_external_scripts_stay_inert_by_default_and_run_once_when_enabled() {
    let document = TreeBuilder::parse("<!doctype html><html><body></body></html>").document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime.eval(r#"globalThis.externalRuns=0;globalThis.externalLoads=0;
        globalThis.externalHost=document.createElement('div');document.body.append(externalHost);
        externalHost.setHTMLUnsafe('<script src="data:text/javascript,externalRuns%2B%2B" onload="externalLoads++"></script>');
    "#).unwrap();
    runtime.run_timers(100, 1, 100);
    assert_eq!(
        runtime
            .eval("externalRuns + externalLoads")
            .unwrap()
            .as_number(),
        Some(0.0)
    );
    for expected in 1..=3 {
        runtime.eval(r#"externalHost.setHTMLUnsafe('<div><template shadowrootmode="open"><script src="data:text/javascript,externalRuns%2B%2B" onload="externalLoads++"></script></template></div>', {runScripts:true});"#).unwrap();
        runtime.run_timers(100, 1, 100);
        assert_eq!(
            runtime.eval("externalRuns").unwrap().as_number(),
            Some(f64::from(expected))
        );
        assert_eq!(
            runtime.eval("externalLoads").unwrap().as_number(),
            Some(f64::from(expected))
        );
        runtime.eval("{const original=externalHost.firstChild.shadowRoot.querySelector('script');original.remove();externalHost.firstChild.shadowRoot.append(original)}").unwrap();
        runtime.run_timers(100, 1, 100);
        assert_eq!(
            runtime.eval("externalRuns").unwrap().as_number(),
            Some(f64::from(expected))
        );
    }
}

#[test]
fn unsafe_html_nested_scripts_restore_current_script_and_hide_shadow_execution() {
    check(
        r#"(() => {
        const host = document.createElement('div'); document.body.append(host);
        globalThis.nestedScripts=[];
        host.setHTMLUnsafe('<script id="unsafe-outer">' +
            'const activeOuter=document.currentScript;' +
            'document.getElementById("unsafe-inner").setHTMLUnsafe("<script id=unsafe-inner-script>nestedScripts.push(document.currentScript.id);</"+"script>",{runScripts:true});' +
            'nestedScripts.push(document.currentScript === activeOuter);</script><div id="unsafe-inner"></div>', {runScripts:true});
        if (nestedScripts.join(',') !== 'unsafe-inner-script,true' || document.currentScript !== null) return false;
        const root=host.attachShadow({mode:'open'});
        root.setHTMLUnsafe('<script>nestedScripts.push(document.currentScript === null);</script>', {runScripts:true});
        return nestedScripts.join(',') === 'unsafe-inner-script,true,true' && document.currentScript === null;
    })()"#,
    );
}

#[test]
fn unsafe_html_current_script_uses_native_root_despite_parent_override() {
    check(
        r#"(() => {
        const host=document.createElement('div'); document.body.append(host);
        const root=host.attachShadow({mode:'closed'});
        Object.defineProperty(root, 'parentNode', {get() { throw new Error('public parent read'); }});
        globalThis.privateRootScriptRan=false;
        root.setHTMLUnsafe('<script>privateRootScriptRan=document.currentScript === null;</script>', {runScripts:true});
        return privateRootScriptRan && document.currentScript === null;
    })()"#,
    );
}

#[test]
fn html_sanitizer_filters_templates_shadow_trees_and_namespaced_attributes_before_execution() {
    check(
        r#"(() => {
        const host=document.createElement('div');document.body.append(host);
        globalThis.sanitizedScriptRuns=0;
        host.setHTMLUnsafe('<template shadowrootmode="closed" shadowrootserializable><script>sanitizedScriptRuns++;</script><b secret="1">shadow</b></template><template><script>sanitizedScriptRuns++;</script><b secret="2">template</b></template><svg><a xlink:href="javascript:alert(1)" href="https://example.test/"/></svg>',
            {runScripts:true,sanitizer:{removeElements:['script'],removeAttributes:['secret'],javascriptURLs:false}});
        if (sanitizedScriptRuns) return false;
        const html=host.getHTML({serializableShadowRoots:true});
        const template=host.querySelector('template');
        const a=host.querySelector('a');
        return html.includes('shadow</b>') && !html.includes('<script') && !html.includes('secret=') &&
            !template.content.querySelector('script') && !a.hasAttributeNS('http://www.w3.org/1999/xlink','href') &&
            a.getAttribute('href') === 'https://example.test/';
    })()"#,
    );
}

#[test]
fn html_sanitizer_invalid_configuration_leaves_existing_tree_and_shadow_root_untouched() {
    check(
        r#"(() => {
        const host=document.createElement('div');host.innerHTML='<b>original</b>';document.body.append(host);
        let rejected=0;
        for (const sanitizer of [
            {elements:['b'],removeElements:['script']},
            {elements:['b','b']},
            {replaceWithChildrenElements:['html']},
            {attributes:['title'],elements:[{name:'b',attributes:['title']}]},
            {removeAttributes:[],dataAttributes:false},
            {processingInstructions:[],removeProcessingInstructions:[]},
        ]) {
            try { host.setHTMLUnsafe('<template shadowrootmode="open"><i>changed</i></template>',{sanitizer}); }
            catch(error) { if (!(error instanceof TypeError)) throw error; rejected++; }
            if (host.innerHTML !== '<b>original</b>' || host.shadowRoot !== null) return false;
        }
        return rejected===6;
    })()"#,
    );
}

#[test]
fn html_sanitizer_config_modifiers_and_safe_parsing_preserve_owned_state() {
    check(
        r#"(() => {
        const input={elements:[{name:'b',attributes:['title']}],removeAttributes:['secret']};
        const sanitizer=new Sanitizer(input);input.elements[0].name='script';
        const snapshot=sanitizer.get();snapshot.elements[0].name='script';
        if (sanitizer.get().elements[0].name !== 'b') return false;
        if (!sanitizer.allowElement({name:'i',removeAttributes:['class','class','secret']})) return false;
        if (!sanitizer.replaceElementWithChildren('b') || sanitizer.replaceElementWithChildren('html')) return false;
        const host=document.createElement('div');document.body.append(host);
        host.setHTMLUnsafe('<b><i class="x" title="kept">value</i></b>',{sanitizer});
        if (host.innerHTML !== '<i title="kept">value</i>') return false;
        const script=document.createElement('script');script.textContent='original';
        script.setHTML('changed');if (script.textContent !== 'original') return false;
        const unsafe=new Sanitizer({});
        if (!unsafe.removeUnsafe() || unsafe.removeUnsafe()) return false;
        const safe=Document.parseHTML('<p onclick="alert(1)">safe<script>alert(1);</script><a href="JaVaScRiPt:alert(1)">link</a>',{sanitizer:{}});
        return !safe.querySelector('script') && !safe.querySelector('p').hasAttribute('onclick') &&
            !safe.querySelector('a').hasAttribute('href') && safe.defaultView===null;
    })()"#,
    );
}

#[test]
fn html_sanitizer_preserves_foreign_attribute_names_and_html_outer_serialization() {
    let document = TreeBuilder::parse("<!doctype html><html><body></body></html>").document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let result = runtime.eval(r#"JSON.stringify((() => {
        const host=document.createElement('div');
        host.setHTMLUnsafe('<svg viewbox="0 0 1 1" xml:space="preserve" xlink:href="about:blank" xmlns:foo="urn:custom"><animate attributeName="href"/></svg>',{sanitizer:{}});
        const svg=host.firstChild;
        const attributes=[svg.getAttribute('viewBox'),svg.getAttributeNS('http://www.w3.org/XML/1998/namespace','space'),
            svg.getAttributeNS('http://www.w3.org/1999/xlink','href'),svg.getAttributeNS(null,'xmlns:foo'),svg.firstChild.getAttribute('attributeName')];
        host.setHTML('<svg><animate attributeName="href"/><animate attributeName="href:x"/></svg>',{sanitizer:{}});
        const safeAttributes=[host.firstChild.firstChild.hasAttribute('attributeName'),host.firstChild.lastChild.getAttribute('attributeName')];
        const template=document.createElement('template');template.setHTML('<p title="hello">x</p>',{sanitizer:{}});
        return {attributes,safeAttributes,outerHTML:template.content.firstChild.outerHTML};
    })())"#).unwrap();
    let observed: serde_json::Value =
        serde_json::from_str(&result.as_string().unwrap().to_std_string_escaped()).unwrap();
    assert_eq!(
        observed,
        serde_json::json!({
            "attributes": ["0 0 1 1", "preserve", "about:blank", "urn:custom", "href"],
            "safeAttributes": [false, "href:x"],
            "outerHTML": "<p title=\"hello\">x</p>"
        })
    );
}

#[test]
fn html_sanitizer_get_uses_code_unit_order_and_exposes_javascript_urls_spelling() {
    check(
        r#"(() => {
        const sanitizer=new Sanitizer({elements:['\uE000','\u{10000}'],attributes:[],javascriptURLs:false});
        const config=sanitizer.get();
        return config.elements[0].name === '\u{10000}' && config.elements[1].name === '\uE000' &&
            config.javascriptURLs===false && !('javascriptUrls' in config) &&
            new Sanitizer().get().javascriptURLs===false;
    })()"#,
    );
}

#[test]
fn html_sanitizer_filters_actual_processing_instructions_by_case_sensitive_target() {
    check(
        r#"(() => {
        const host=document.createElement('div');
        host.setHTMLUnsafe('<?keep literal &amp;?><?Keep ignored?><?drop ignored?><p><?keep inner?></p>',
            {sanitizer:{processingInstructions:['keep']}});
        if (host.childNodes.length !== 2 || host.firstChild.nodeType !== 7 || host.firstChild.target !== 'keep' || host.firstChild.data !== 'literal &amp;') return false;
        if (host.lastChild.firstChild.nodeType !== 7 || host.lastChild.firstChild.data !== 'inner') return false;
        const safe=Document.parseHTML('<!doctype html><p><?keep removed?></p>',{sanitizer:{}});
        return safe.querySelector('p').childNodes.length === 0;
    })()"#,
    );
}

#[test]
fn html_sanitizer_sequence_conversion_closes_iterators_and_ignores_public_get_override() {
    check(
        r#"(() => {
        let iteratorReads=0,closed=0;
        const elements={get [Symbol.iterator]() { iteratorReads++;return function() {
            return {next() {return {done:false,value:{name:Symbol('invalid')}};},return() {closed++;return {};}};
        };}};
        let rejected=false;
        try {new Sanitizer({elements});} catch(error) {rejected=error instanceof TypeError;}
        if (!rejected || iteratorReads!==1 || closed!==1) return false;
        const sanitizer=new Sanitizer({removeElements:['script']});
        sanitizer.get=() => {throw new Error('public configuration getter');};
        globalThis.brandCheckedRuns=0;
        const host=document.createElement('div');document.body.append(host);
        host.setHTMLUnsafe('<script>brandCheckedRuns++;</script><b>kept</b>',{sanitizer,runScripts:true});
        return brandCheckedRuns===0 && host.innerHTML==='<b>kept</b>';
    })()"#,
    );
}

#[test]
fn html_sanitizer_preserves_adjacent_text_after_comment_removal() {
    check(
        r#"(() => {
        const host=document.createElement('div');
        host.setHTMLUnsafe('a <!-- comment --> b',{sanitizer:{comments:false}});
        return host.childNodes.length===2 && host.firstChild.nodeType===3 && host.firstChild.data==='a ' &&
            host.lastChild.nodeType===3 && host.lastChild.data===' b';
    })()"#,
    );
}

#[test]
fn append_child_validates_native_brands_and_whole_fragment_before_mutation() {
    check(
        r#"(() => {
        const parent = document.createElement('div');
        const doctype = document.implementation.createDocumentType('html', '', '');
        const expect = (operation, name) => {
            try { operation(); } catch (error) { if (error.name === name) return; throw error; }
            throw new Error('Missing ' + name);
        };
        expect(() => parent.appendChild(doctype), 'HierarchyRequestError');
        expect(() => parent.insertBefore(doctype, null), 'HierarchyRequestError');
        expect(() => parent.appendChild({__id: parent.__id}), 'TypeError');
        expect(() => Node.prototype.appendChild.call({}, parent), 'TypeError');
        const doc = document.implementation.createHTMLDocument('title');
        const fragment = document.createDocumentFragment();
        const comment = document.createComment('retained');
        const secondRoot = document.createElement('html');
        fragment.append(comment, secondRoot);
        expect(() => doc.appendChild(fragment), 'HierarchyRequestError');
        expect(() => doc.insertBefore(fragment, null), 'HierarchyRequestError');
        if (fragment.firstChild !== comment || fragment.lastChild !== secondRoot ||
            comment.parentNode !== fragment || doc.lastChild !== doc.documentElement) return false;
        const child = document.createElement('span');
        Object.defineProperty(child, 'nodeType', {get() {throw new Error('author nodeType');}});
        Object.defineProperty(child, 'isConnected', {get() {throw new Error('author isConnected');}});
        return parent.appendChild(child) === child && parent.firstChild === child;
    })()"#,
    );
}

#[test]
fn replace_child_validates_document_replacement_and_aggregates_observers() {
    check(
        r#"(() => {
        const doc = document.implementation.createHTMLDocument('title');
        const original = doc.documentElement;
        const oldDoctype = doc.doctype;
        const newDoctype = document.implementation.createDocumentType('html', '', '');
        if (doc.replaceChild(newDoctype, oldDoctype) !== oldDoctype ||
            doc.firstChild !== newDoctype || newDoctype.nextSibling !== original) return false;
        const replacement = document.createElement('html');
        const observer = new MutationObserver(() => {});
        observer.observe(doc, {childList:true});
        Object.defineProperty(doc, 'insertBefore', {value() {throw new Error('author insertBefore');}});
        Object.defineProperty(doc, 'removeChild', {value() {throw new Error('author removeChild');}});
        if (doc.replaceChild(replacement, original) !== original ||
            doc.documentElement !== replacement || original.parentNode !== null ||
            replacement.ownerDocument !== doc) return false;
        const records = observer.takeRecords();
        if (records.length !== 1 || records[0].addedNodes[0] !== replacement ||
            records[0].removedNodes[0] !== original || records[0].previousSibling !== doc.doctype ||
            records[0].nextSibling !== null) return false;
        const text = document.createTextNode('invalid');
        try {doc.replaceChild(text, replacement); return false;}
        catch(error) {if (error.name !== 'HierarchyRequestError') throw error;}
        if (doc.documentElement !== replacement || observer.takeRecords().length !== 0) return false;
        try {doc.replaceChild(original, document.createElement('absent')); return false;}
        catch(error) {if(error.name !== 'NotFoundError') throw error;}
        return doc.replaceChild(replacement, replacement) === replacement &&
            doc.documentElement === replacement;
    })()"#,
    );
}

#[test]
fn fragment_insertion_reports_one_removal_record_before_parent_insertion() {
    check(
        r#"(() => {
        for (const operation of ['appendChild', 'insertBefore', 'prepend', 'replaceChildren']) {
            const fragment = document.createDocumentFragment();
            const a = document.createElement('a'), b = document.createElement('b');
            fragment.append(a, b);
            const parent = document.createElement('div');
            const observer = new MutationObserver(() => {});
            observer.observe(fragment, {childList:true});
            observer.observe(parent, {childList:true});
            if (operation === 'insertBefore') parent.insertBefore(fragment, null);
            else parent[operation](fragment);
            const records = observer.takeRecords();
            if (records.length !== 2 || records[0].target !== fragment ||
                records[0].removedNodes.length !== 2 || records[0].removedNodes[0] !== a ||
                records[0].removedNodes[1] !== b || records[0].addedNodes.length !== 0 ||
                records[0].previousSibling !== null || records[0].nextSibling !== null ||
                records[1].target !== parent || records[1].addedNodes.length !== 2 ||
                records[1].addedNodes[0] !== a || records[1].addedNodes[1] !== b ||
                fragment.firstChild !== null || parent.firstChild !== a || parent.lastChild !== b) {
                throw new Error(operation + ' fragment mutation records');
            }
        }
        return true;
    })()"#,
    );
}

#[test]
fn custom_element_registry_uses_its_owner_realm_when_called_from_parent() {
    let document =
        TreeBuilder::parse("<!doctype html><body><iframe id=f></iframe></body>").document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let observed = runtime
        .eval(
            r#"(() => {
        try {
            const frame = document.getElementById('f'), view = frame.contentWindow;
            const doc = frame.contentDocument, registry = view.customElements;
            const before = doc.createElement('x-own-realm');
            class ChildElement extends view.HTMLElement {}
            registry.define('x-own-realm', ChildElement);
            const undefinedBeforeUpgrade = !(before instanceof ChildElement);
            registry.upgrade(before);
            const after = doc.createElement('x-own-realm');
            return JSON.stringify({undefinedBeforeUpgrade, upgraded:before instanceof ChildElement,
                created:after instanceof ChildElement, owner:after.ownerDocument===doc,
                isolated:customElements.get('x-own-realm')===undefined});
        } catch(error) {return JSON.stringify({error:error.name,message:error.message});}
    })()"#,
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(
        observed,
        r#"{"undefinedBeforeUpgrade":true,"upgraded":true,"created":true,"owner":true,"isolated":true}"#
    );
}

#[test]
fn sanitized_markup_does_not_treat_unicode_case_folds_as_event_handler_names() {
    check(
        r#"(() => {
        globalThis.unicodeHandlerCalls = [];
        const host = document.createElement('div');
        document.body.append(host);
        for (const method of ['setHTML', 'setHTMLUnsafe']) {
            const sanitizer = new Sanitizer({});
            sanitizer.removeUnsafe();
            host[method]('<button onKeydown="unicodeHandlerCalls.push(1)" onkeydown="unicodeHandlerCalls.push(2)">go</button>', {sanitizer});
            const button = host.firstChild;
            if (!button.getAttributeNames().includes('onKeydown') || button.hasAttribute('onkeydown')) return false;
            button.dispatchEvent(new KeyboardEvent('keydown'));
        }
        if (unicodeHandlerCalls.length !== 0) throw new Error('Unicode event handler name became executable');
        host.setHTMLUnsafe('<button ONKEYDOWN="unicodeHandlerCalls.push(3)">go</button>');
        host.firstChild.dispatchEvent(new KeyboardEvent('keydown'));
        return unicodeHandlerCalls.join(',') === '3';
    })()"#,
    );
}

#[test]
fn mouse_selection_keeps_associated_live_range_when_selected_node_moves() {
    check(
        r#"(() => {
        const parent = document.createElement('div'), target = document.createElement('div');
        const span = document.createElement('span'); span.textContent = 'selected';
        parent.append(document.createTextNode('prefix'), span);
        document.body.append(parent, target);
        const rect = span.getBoundingClientRect(), y = rect.top + rect.height / 2;
        __omoikane_dispatch_mouse_input(span.__id, 'mousedown', {button:0,buttons:1,clientX:rect.left + .1,clientY:y}, true);
        __omoikane_dispatch_mouse_input(span.__id, 'mousemove', {button:0,buttons:1,clientX:rect.right - .1,clientY:y}, false);
        __omoikane_dispatch_mouse_input(span.__id, 'mouseup', {button:0,buttons:0,clientX:rect.right - .1,clientY:y}, false);
        const selection = getSelection(), range = selection.getRangeAt(0);
        if (selection.toString() !== 'selected') throw new Error('actual pointer selection missing');
        target.moveBefore(span, null);
        return selection.getRangeAt(0) === range && range.startContainer === parent &&
            range.endContainer === parent && range.startOffset === 1 && range.endOffset === 1 &&
            selection.anchorNode === parent && selection.focusNode === parent && selection.isCollapsed;
    })()"#,
    );
}

#[test]
fn mouse_selection_selectstart_uses_text_boundary_and_honors_cancellation() {
    check(
        r#"(() => {
        const p = document.createElement('p'); p.textContent = 'selection'; document.body.append(p);
        const text = p.firstChild, selection = getSelection(); selection.collapse(text, 2);
        const before = selection.getRangeAt(0), rect = p.getBoundingClientRect();
        let calls = 0;
        p.addEventListener('selectstart', event => {
            if (event.target !== text || !event.bubbles || !event.cancelable || !event.isTrusted) throw new Error('wrong selectstart');
            calls++; event.preventDefault();
        });
        __omoikane_dispatch_mouse_input(p.__id, 'mousedown', {button:0,buttons:1,clientX:rect.left + .1,clientY:rect.top + rect.height / 2}, true);
        return calls === 1 && selection.getRangeAt(0) === before && selection.anchorNode === text && selection.anchorOffset === 2;
    })()"#,
    );
}

#[test]
fn mouse_selection_ignores_author_range_and_selection_setter_overrides() {
    check(
        r#"(() => {
        const p = document.createElement('p'); p.textContent = 'native selection'; document.body.append(p);
        const text = p.firstChild, selection = getSelection();
        for (const name of ['setStart', 'setEnd', '__validate']) {
            Range.prototype[name] = () => {throw new Error('author Range.' + name);};
        }
        Selection.prototype.setBaseAndExtent = () => {throw new Error('author Selection');};
        text.dispatchEvent = () => {throw new Error('author dispatchEvent');};
        const rect = p.getBoundingClientRect(), y = rect.top + rect.height / 2;
        __omoikane_dispatch_mouse_input(p.__id, 'mousedown', {button:0,buttons:1,clientX:rect.left + .1,clientY:y}, true);
        __omoikane_dispatch_mouse_input(p.__id, 'mousemove', {button:0,buttons:1,clientX:rect.right - .1,clientY:y}, false);
        __omoikane_dispatch_mouse_input(p.__id, 'mouseup', {button:0,buttons:0,clientX:rect.right - .1,clientY:y}, false);
        return selection.anchorNode === text && selection.anchorOffset === 0 &&
            selection.focusNode === text && selection.focusOffset === text.length && selection.toString() === 'native selection';
    })()"#,
    );
}

#[test]
fn range_boundaries_use_native_character_data_and_tree_lengths() {
    check(
        r#"(() => {
        const host = document.createElement('div'), text = document.createTextNode('a😀b');
        host.append(text); document.body.append(host);
        Object.defineProperty(text, 'length', {get() {throw new Error('author length');}});
        Object.defineProperty(text, 'nodeType', {get() {throw new Error('author nodeType');}});
        Object.defineProperty(host, 'childNodes', {get() {throw new Error('author childNodes');}});
        Object.defineProperty(text, 'parentNode', {get() {throw new Error('author parentNode');}});
        const range = document.createRange();
        range.setStart(text, 1); range.setEnd(text, 3);
        if (range.startContainer !== text || range.startOffset !== 1 || range.endOffset !== 3) return false;
        range.setEnd(host, 1);
        if (range.endContainer !== host || range.endOffset !== 1) return false;
        try {range.setStart(text, 5); return false;}
        catch(error) {if (error.name !== 'IndexSizeError') throw error;}
        const xml = document.implementation.createDocument(null, 'root', null);
        const cdata = xml.createCDATASection('a😀b'); xml.documentElement.append(cdata);
        const xmlRange = xml.createRange(); xmlRange.setStart(cdata, 1); xmlRange.setEnd(cdata, 3);
        return xmlRange.startContainer === cdata && xmlRange.startOffset === 1 && xmlRange.endOffset === 3;
    })()"#,
    );
}

#[test]
fn mouse_selection_drags_inside_shadow_root_using_composed_boundaries() {
    check(
        r#"(() => {
        const host = document.createElement('div'); document.body.append(host);
        const root = host.attachShadow({mode:'closed'}), p = document.createElement('p');
        p.textContent = 'shadow selection'; root.append(p);
        const text = p.firstChild, rect = p.getBoundingClientRect(), y = rect.top + rect.height / 2;
        __omoikane_dispatch_mouse_input(p.__id, 'mousedown', {button:0,buttons:1,clientX:rect.left + .1,clientY:y}, true);
        __omoikane_dispatch_mouse_input(p.__id, 'mousemove', {button:0,buttons:1,clientX:rect.right - .1,clientY:y}, false);
        __omoikane_dispatch_mouse_input(p.__id, 'mouseup', {button:0,buttons:0,clientX:rect.right - .1,clientY:y}, false);
        const selection = getSelection(), exposed = selection.getComposedRanges({shadowRoots:[root]});
        if (exposed.length !== 1 || exposed[0].startContainer !== text || exposed[0].startOffset !== 0 ||
            exposed[0].endContainer !== text || exposed[0].endOffset !== text.length) return false;
        const rescaled = selection.getComposedRanges();
        return selection.anchorNode === null && selection.focusNode === null &&
            rescaled.length === 1 && rescaled[0].startContainer === document.body &&
            rescaled[0].endContainer === document.body && rescaled[0].endOffset === rescaled[0].startOffset + 1;
    })()"#,
    );
}

#[test]
fn mouse_selection_routes_to_child_document_without_changing_parent_selection() {
    check(
        r#"(() => {
        const parentText = document.createTextNode('parent'); document.body.append(parentText);
        const selection = getSelection(); selection.collapse(parentText, 2);
        const parentRange = selection.getRangeAt(0);
        const frame = document.createElement('iframe'); document.body.append(frame);
        const doc = frame.contentDocument, p = doc.createElement('p');
        p.textContent = 'child selection'; doc.body.append(p);
        const text = p.firstChild, rect = p.getBoundingClientRect(), y = rect.top + rect.height / 2;
        __omoikane_dispatch_mouse_input(p.__id, 'mousedown', {button:0,buttons:1,clientX:rect.left + .1,clientY:y}, true);
        __omoikane_dispatch_mouse_input(p.__id, 'mousemove', {button:0,buttons:1,clientX:rect.right - .1,clientY:y}, false);
        __omoikane_dispatch_mouse_input(p.__id, 'mouseup', {button:0,buttons:0,clientX:rect.right - .1,clientY:y}, false);
        const child = frame.contentWindow.getSelection();
        return child !== selection && child.anchorNode === text && child.anchorOffset === 0 &&
            child.focusNode === text && child.focusOffset === text.length && child.toString() === 'child selection' &&
            selection.getRangeAt(0) === parentRange && selection.anchorNode === parentText && selection.anchorOffset === 2;
    })()"#,
    );
}

#[test]
fn get_html_serializes_qualified_names_and_namespace_attribute_names() {
    check(
        r#"(() => {
        const host = document.createElement('div'), node = document.createElementNS('urn:example', 'p:item');
        node.setAttributeNS('http://www.w3.org/XML/1998/namespace', 'alternate:lang', 'en');
        node.setAttributeNS('http://www.w3.org/1999/xlink', 'alternate:href', '&');
        host.append(node);
        const html = host.getHTML();
        return html.startsWith('<p:item ') && html.endsWith('</p:item>') &&
            html.includes('xml:lang="en"') && html.includes('xlink:href="&amp;"');
    })()"#,
    );
}

#[test]
fn get_html_serializes_processing_instruction_terminator() {
    check(
        r#"(() => {
        const host = document.createElement('div'); host.append(document.createProcessingInstruction('target', 'data'));
        return host.getHTML() === '<?target data?>';
    })()"#,
    );
}

#[test]
fn get_html_distinguishes_unqualified_xml_from_html_void_and_raw_text() {
    check(
        r#"(() => {
        const xml = document.implementation.createDocument(null, 'root', null);
        const br = xml.createElement('br'), script = xml.createElement('script');
        br.append(xml.createTextNode('content')); script.append(xml.createTextNode('<&>'));
        xml.documentElement.append(br, script);
        if (xml.documentElement.getHTML() !== '<br>content</br><script>&lt;&amp;&gt;</script>') return false;
        const host = document.createElement('div'), htmlBr = document.createElement('br');
        htmlBr.append(document.createTextNode('omitted'));
        const htmlScript = document.createElement('script'); htmlScript.append(document.createTextNode('<&>'));
        host.append(htmlBr, htmlScript);
        return host.getHTML() === '<br><script><&></script>';
    })()"#,
    );
}

#[test]
fn get_html_noscript_escaping_uses_the_owner_document_scripting_state() {
    check(
        r#"(() => {
        const frame = document.createElement('iframe'); frame.setAttribute('sandbox', 'allow-same-origin'); document.body.append(frame);
        const inert = Document.parseHTMLUnsafe('<!doctype html><body></body>');
        for (const doc of [document, inert, frame.contentDocument]) {
            const noscript = doc.createElement('noscript'); noscript.append(doc.createTextNode('<b>&</b>')); doc.body.append(noscript);
            const expected = doc === document ? '<b>&</b>' : '&lt;b&gt;&amp;&lt;/b&gt;';
            if (noscript.getHTML() !== expected) throw new Error('owner scripting: ' + noscript.getHTML());
        }
        return true;
    })()"#,
    );
}

#[test]
fn get_html_noscript_in_template_uses_inert_contents_document() {
    check(
        r#"(() => {
        const template = document.createElement('template');
        const noscript = document.createElement('noscript'); noscript.append(document.createTextNode('<b>&</b>'));
        template.content.append(noscript);
        return noscript.ownerDocument !== document &&
            template.getHTML() === '<noscript>&lt;b&gt;&amp;&lt;/b&gt;</noscript>';
    })()"#,
    );
}

#[test]
fn created_text_retains_unpaired_surrogates_and_literal_escapes() {
    check(
        r#"(() => {
        for (const value of ['\ud800', '\udfff', '\\uD800', '😀']) {
            const node = document.createTextNode(value);
            if (node.data !== value || node.textContent !== value || node.length !== value.length) {
                throw new Error('createTextNode changed UTF-16 data');
            }
        }
        return true;
    })()"#,
    );
}

#[test]
fn text_mutation_cloning_and_descendants_retain_utf16() {
    check(
        r#"(() => {
        const p = document.createElement('p');
        const high = document.createTextNode('\ud800');
        const span = document.createElement('span');
        span.append(document.createTextNode('\udc00'));
        p.append(high, document.createComment('excluded'), span);
        if (p.textContent !== '\ud800\udc00' || p.cloneNode(true).textContent !== p.textContent) throw new Error('descendants/clone');
        high.data = '\udfff';
        if (high.cloneNode().data !== '\udfff') throw new Error('mutated clone');
        p.textContent = '\ud800x';
        if (p.firstChild.data !== '\ud800x' || p.cloneNode(true).textContent !== '\ud800x') throw new Error('replacement');
        p.firstChild.data = 'ordinary';
        if (p.cloneNode(true).textContent !== 'ordinary') throw new Error('stale original units');
        return true;
    })()"#,
    );
}

#[test]
fn get_html_retains_text_utf16_in_escaped_raw_and_shadow_contexts() {
    check(
        r#"(() => {
        const value = '\ud800<&>\u00a0\udfff😀\\uD800';
        const escaped = '\ud800&lt;&amp;&gt;&nbsp;\udfff😀\\uD800';
        const p = document.createElement('p');
        p.append(document.createTextNode(value));
        if (p.getHTML() !== escaped || p.outerHTML !== '<p>' + escaped + '</p>') throw new Error('escaped text');
        const script = document.createElement('script');
        script.append(document.createTextNode(value));
        if (script.getHTML() !== value) throw new Error('raw text');
        const host = document.createElement('div');
        const root = host.attachShadow({mode:'open',serializable:true});
        root.append(p);
        if (!host.getHTML({serializableShadowRoots:true}).includes('<p>' + escaped + '</p>')) throw new Error('shadow text');
        return true;
    })()"#,
    );
}

#[test]
fn character_data_creation_mutation_and_clone_preserve_utf16() {
    check(
        r#"(() => {
        const xml = document.implementation.createDocument(null, 'root');
        for (const node of [document.createComment('\ud800'), document.createProcessingInstruction('probe', '\ud800'), xml.createCDATASection('\ud800')]) {
            if (node.data !== '\ud800' || node.cloneNode().data !== node.data) throw new Error('creation/clone');
            node.appendData('\udfff');
            if (node.cloneNode().data !== '\ud800\udfff') throw new Error('mutation/clone');
            node.data = 'plain';
            if (node.cloneNode().data !== 'plain') throw new Error('reset');
        }
        const host = document.createElement('div');
        host.append(document.createComment('\ud800'), document.createProcessingInstruction('probe','\udfff'));
        if (host.getHTML() !== '<!--\ud800--><?probe \udfff?>') throw new Error('serialization');
        return true;
    })()"#,
    );
}

#[test]
fn unqualified_attribute_bridge_retains_utf16_and_clears_old_units() {
    check(
        r#"(() => {
        const node = document.createElement('div');
        for (const value of ['\ud800', '\udfff', '\\uD800', '😀', 'ordinary']) {
            node.setAttribute('DATA-X', value);
            if (node.getAttribute('data-x') !== value || node.getAttribute('DATA-X') !== value) throw new Error('attribute bridge changed data');
        }
        node.removeAttribute('data-x');
        return node.getAttribute('data-x') === null;
    })()"#,
    );
}

#[test]
fn attribute_records_namespaces_clones_and_html_preserve_utf16() {
    check(
        r#"(() => {
        const node = document.createElement('div'), value = '\ud800&"\u00a0\udfff';
        node.setAttribute('x',value);
        node.setAttributeNS('urn:probe','p:y',value);
        for (const target of [node,node.cloneNode(true)]) {
            if (target.getAttributeNS('urn:probe','y') !== value || target.attributes[0].value !== value || target.attributes[1].value !== value) throw new Error('records/ns/clone');
            if (target.getHTML() !== '' || !target.outerHTML.includes('x="\ud800&amp;&quot;&nbsp;\udfff"')) throw new Error('attribute serialization');
        }
        node.setAttributeNS('urn:probe','q:y','plain');
        return node.getAttributeNS('urn:probe','y') === 'plain';
    })()"#,
    );
}

#[test]
fn html_attribute_escaping_includes_angle_brackets_and_surrogates() {
    check(
        r#"(() => {
        const parent = document.createElement('div'), node = document.createElement('span');
        node.setAttribute('x', '\ud800<>'); parent.append(node);
        return parent.getHTML() === '<span x="\ud800&lt;&gt;"></span>';
    })()"#,
    );
}

#[test]
fn unsafe_html_parsing_retains_exact_text_utf16() {
    check(
        r#"(() => {
        const value = '\ud800\ufffd\udfff😀\\uD800';
        const parsed = Document.parseHTMLUnsafe('<p>' + value + '</p>');
        if (parsed.body.textContent !== value || parsed.body.firstChild.firstChild.data !== value || parsed.body.getHTML() !== '<p>' + value + '</p>') throw new Error('document text');
        for (const tag of ['div','title','textarea','style','script','plaintext']) {
            const node = document.createElement(tag);
            node.setHTMLUnsafe(value);
            if (node.textContent !== value || node.getHTML() !== value) throw new Error('fragment text ' + tag);
        }
        const foreign = document.createElementNS('http://www.w3.org/2000/svg','svg');
        foreign.setHTMLUnsafe('<text>' + value + '</text>');
        if (foreign.textContent !== value) throw new Error('foreign text');
        const table = document.createElement('div');
        table.setHTMLUnsafe('<table>' + value + '<tr><td>x</td></tr></table>');
        return table.firstChild.data === value && table.textContent === value + 'x';
    })()"#,
    );
}

#[test]
fn unsafe_html_parsed_attributes_preserve_exact_utf16() {
    check(
        r#"(() => {
        const value = '\ud800\ufffd\udfff😀\\uD800';
        const markup = `<p a="${value}" b='${value}' c=${value}></p>`;
        const parsed = Document.parseHTMLUnsafe(markup);
        const target = document.createElement('div'); target.setHTMLUnsafe(markup);
        for (const node of [parsed.body.firstChild,target.firstChild]) {
            for (const name of ['a','b','c']) if (node.getAttribute(name) !== value || node.getAttributeNode(name).value !== value) throw new Error('parsed attribute ' + name);
            if (node.cloneNode().getAttribute('a') !== value || !node.outerHTML.includes('a="' + value + '"')) throw new Error('cloning/serialization');
        }
        target.setHTMLUnsafe('<svg><text xlink:title="' + value + '"></text></svg>');
        return target.firstChild.firstChild.getAttributeNS('http://www.w3.org/1999/xlink','title') === value;
    })()"#,
    );
}

#[test]
fn unsafe_html_parsed_comments_and_instructions_preserve_utf16() {
    check(
        r#"(() => {
        const value = '\ud800\ufffd\udfff😀\\uD800';
        const markup = '<!--' + value + '--><?probe ' + value + '?>';
        const parsed = Document.parseHTMLUnsafe('<div>' + markup + '</div>');
        const host = document.createElement('div'); host.setHTMLUnsafe(markup);
        for (const node of [parsed.body.firstChild, host]) {
            if (node.childNodes.length !== 2 || node.childNodes[0].data !== value || node.childNodes[1].data !== value || node.getHTML() !== markup) throw new Error('leaf data');
            if (node.cloneNode(true).getHTML() !== markup) throw new Error('cloned leaf');
        }
        host.setHTMLUnsafe('<?9probe ' + value + '?>');
        return host.firstChild.data === '?9probe ' + value + '?';
    })()"#,
    );
}

#[test]
fn document_write_preserves_utf16_between_calls_and_reentrant_scripts() {
    check(
        r#"(() => {
        document.open();
        document.write('<p id="split">\ud83d');
        const node = document.getElementById('split').firstChild;
        if (node.data !== '\ud83d') throw new Error('first write not observable');
        document.write('\ude00\ud800</p>');
        if (document.getElementById('split').firstChild !== node || node.data !== '😀\ud800') throw new Error('split write');
        const value = '\ud800\ufffd\udfff';
        document.write('<script>document.write("<b id=reentered>r</b>");</script><p id="tail" x="' + value + '">' + value + '<!--' + value + '--></p>');
        document.close();
        const tail = document.getElementById('tail');
        return !!document.getElementById('reentered') && tail.firstChild.data === value && tail.getAttribute('x') === value && tail.lastChild.data === value;
    })()"#,
    );
}

#[test]
fn xml_serializer_and_xml_inner_html_preserve_utf16() {
    check(
        r#"(() => {
        const doc = document.implementation.createDocument(null,'root');
        const root = doc.documentElement, value = '\ud800<&>\udfff';
        root.setAttribute('x',value);
        root.append(doc.createTextNode(value),doc.createComment(value),doc.createCDATASection(value),doc.createProcessingInstruction('probe',value));
        const escaped = '\ud800&lt;&amp;&gt;\udfff';
        const content = escaped + '<!--' + value + '--><![CDATA[' + value + ']]><?probe ' + value + '?>';
        const expected = '<root x="' + escaped + '">' + content + '</root>';
        const serialized = new XMLSerializer().serializeToString(root);
        if (serialized !== expected || root.innerHTML !== content || root.outerHTML !== expected) throw new Error('XML data lost');
        return true;
    })()"#,
    );
}

#[test]
fn html_inner_html_uses_exact_units_and_owner_raw_text_context() {
    check(
        r#"(() => {
        const value = '\ud800<&>\udfff', escaped = '\ud800&lt;&amp;&gt;\udfff';
        const host = document.createElement('div'), span = document.createElement('span');
        span.setAttribute('x',value); span.append(document.createTextNode(value)); host.append(span);
        if (host.innerHTML !== host.getHTML() || span.innerHTML !== escaped) throw new Error('text/attribute units');
        for (const tag of ['script','style','noscript']) {
            const node = document.createElement(tag); node.textContent = value;
            if (node.innerHTML !== value) throw new Error('active raw text ' + tag);
        }
        const inert = Document.parseHTMLUnsafe('<noscript></noscript>');
        inert.body.firstChild.textContent = value;
        if (inert.body.firstChild.innerHTML !== escaped) throw new Error('inert noscript');
        const template = document.createElement('template'); template.content.append(document.createTextNode(value));
        return template.innerHTML === escaped;
    })()"#,
    );
}
