//! Read-only and read-write selectors use real form and editing state.
use omoikane::{html::TreeBuilder, js::JsRuntime};

#[test]
fn original_type_change_reftests_run_scripts_and_match_reference_pixels() {
    use omoikane::layout::Rect;
    use omoikane::paint::render_document;
    let root =
        std::path::PathBuf::from(std::env::var("WPT_ROOT").unwrap_or_else(|_| "target/wpt".into()));
    let directory = root.join("css/selectors");
    if !directory
        .join("selector-read-write-type-change-001.html")
        .is_file()
    {
        assert_ne!(
            std::env::var("WPT_REQUIRED").as_deref(),
            Ok("1"),
            "WPT required"
        );
        eprintln!("WPT checkout missing; read-write original reftests skipped");
        return;
    }
    for index in 1..=2 {
        let name = format!("selector-read-write-type-change-{index:03}");
        let actual_html = std::fs::read_to_string(directory.join(format!("{name}.html"))).unwrap();
        let reference_html =
            std::fs::read_to_string(directory.join(format!("{name}-ref.html"))).unwrap();
        let document = TreeBuilder::parse(&actual_html).document();
        let mut runtime = JsRuntime::with_document(document.clone()).unwrap();
        assert!(
            runtime.execute_document_scripts(None).is_empty(),
            "{name}: original scripts"
        );
        let before = runtime
            .eval("getComputedStyle(document.querySelector('span')).color")
            .unwrap();
        assert_eq!(
            before.as_string().unwrap().to_std_string_escaped(),
            "rgb(255, 0, 0)"
        );
        runtime.fire_load().unwrap();
        runtime.run_timers(100, 10, 100);
        let after = runtime
            .eval("getComputedStyle(document.querySelector('span')).color")
            .unwrap();
        assert_eq!(
            after.as_string().unwrap().to_std_string_escaped(),
            "rgb(0, 128, 0)"
        );
        assert_eq!(
            runtime
                .eval("document.documentElement.classList.contains('reftest-wait')")
                .unwrap()
                .as_boolean(),
            Some(false)
        );
        let viewport = Rect {
            width: 800.0,
            height: 600.0,
            ..Default::default()
        };
        let actual = render_document(&document, viewport).unwrap();
        let expected =
            render_document(&TreeBuilder::parse(&reference_html).document(), viewport).unwrap();
        let different = actual
            .pixels()
            .chunks_exact(4)
            .zip(expected.pixels().chunks_exact(4))
            .filter(|(a, b)| a != b)
            .count();
        eprintln!("{name}: {different} differing pixels");
        assert_eq!(different, 0, "{name}: original reference");
    }
}

#[test]
fn design_mode_is_owned_by_each_document_and_not_copied_or_adopted() {
    assert_eq!(
        evaluate(
            r#"(()=>{try{
const other=document.implementation.createHTMLDocument('other');
const child=other.createElement('span');other.body.append(child);
other.designMode='on';
if(other.designMode!=='on'||!child.isContentEditable||document.designMode!=='off'||document.body.isContentEditable)return 'document-isolation';
other.documentElement.contentEditable='false';
if(!other.documentElement.isContentEditable||!child.isContentEditable)return 'document-root-host';
const clone=other.cloneNode(true);
if(clone.designMode!=='off'||clone.body.isContentEditable)return 'clone-mode';
document.body.append(document.adoptNode(child));
if(child.isContentEditable||child.matches(':read-write')||other.designMode!=='on')return 'adopt-mode';
other.open();other.write('<!doctype html><p id="written">new</p>');other.close();
if(other.designMode!=='on'||!other.getElementById('written').isContentEditable)return 'document-open';
return 'ok';}catch(e){return e.name+': '+e.message;}})()"#
        ),
        "ok"
    );
}

#[test]
fn editing_metadata_survives_iframe_retirement_and_collection() {
    let mut runtime =
        JsRuntime::with_document(TreeBuilder::parse("<body></body>").document()).unwrap();
    runtime
        .eval(
            r#"(()=>{
const frame=document.createElement('iframe');document.body.append(frame);
globalThis.retainedEditingDocument=frame.contentDocument;
retainedEditingDocument.designMode='on';
globalThis.retainedEditingChild=retainedEditingDocument.createElement('span');
retainedEditingDocument.body.append(retainedEditingChild);
frame.remove();
})()"#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    boa_gc::force_collect();
    let result = runtime.eval(r#"(()=>{try{
if(retainedEditingDocument.designMode!=='on'||!retainedEditingChild.isContentEditable)return 'retained';
retainedEditingDocument.designMode='off';
if(retainedEditingChild.isContentEditable||!retainedEditingChild.matches(':read-only'))return 'disabled';
retainedEditingChild.contentEditable='true';
document.body.append(retainedEditingChild);
if(!retainedEditingChild.isContentEditable||retainedEditingChild.ownerDocument!==document)return 'adopt';
return 'ok';}catch(e){return e.name+': '+e.message;}})()"#).unwrap();
    assert_eq!(result.as_string().unwrap().to_std_string_escaped(), "ok");
}

#[test]
fn host_and_part_editability_updates_shadow_and_element_backed_styles() {
    assert_eq!(
        evaluate(
            r#"(()=>{try{
const host=document.createElement('div');host.id='editing-part';
const shadow=host.attachShadow({mode:'open'});
shadow.innerHTML='<style>:host{color:red}:host(:read-write){color:green}input{opacity:1}</style><input part="control">';
const style=document.createElement('style');style.textContent='#editing-part::part(control):read-write{opacity:.5}#editing-part::part(control):read-only{opacity:.25}';
document.head.append(style);document.body.append(host);const input=shadow.querySelector('input');
if(getComputedStyle(host).color!=='rgb(255, 0, 0)'||getComputedStyle(input).opacity!=='0.5')return 'before';
host.contentEditable='plaintext-only';
if(getComputedStyle(host).color!=='rgb(0, 128, 0)')return 'host';
input.type='button';if(getComputedStyle(input).opacity!=='0.25')return 'part-type';
input.type='text';if(getComputedStyle(input).opacity!=='0.5')return 'part-restored';
input.readOnly=true;if(getComputedStyle(input).opacity!=='0.25')return 'part-readonly';
host.contentEditable='false';if(getComputedStyle(host).color!=='rgb(255, 0, 0)')return 'host-restored';
return 'ok';}catch(e){return e.name+': '+e.message;}})()"#
        ),
        "ok"
    );
}

#[test]
fn editing_changes_invalidate_relational_and_filtered_sibling_selectors() {
    assert_eq!(
        evaluate(
            r#"(()=>{try{
const rows=document.createElement('div');rows.id='editing-rows';rows.innerHTML='<span></span><span></span>';document.body.append(rows);
const [first,second]=rows.children;const style=document.createElement('style');
style.textContent='#editing-rows{color:red}#editing-rows:has(> :read-write){color:green}#editing-rows>span{opacity:1}#editing-rows>span:nth-child(2 of :read-write){opacity:.5}';document.head.append(style);
const state=()=>getComputedStyle(rows).color+':'+getComputedStyle(second).opacity;
if(state()!=='rgb(255, 0, 0):1')return 'before';
first.contentEditable='true';if(state()!=='rgb(0, 128, 0):1')return 'has';
second.contentEditable='true';if(state()!=='rgb(0, 128, 0):0.5')return 'filtered';
first.contentEditable='false';if(state()!=='rgb(0, 128, 0):1')return 'filtered-invalidation';
first.contentEditable='inherit';second.contentEditable='inherit';if(state()!=='rgb(255, 0, 0):1')return 'removed';
document.designMode='on';if(state()!=='rgb(0, 128, 0):0.5')return 'document-invalidation';
document.designMode='off';if(state()!=='rgb(255, 0, 0):1')return 'document-restored';
return 'ok';}catch(e){return e.name+': '+e.message;}})()"#
        ),
        "ok"
    );
}

#[test]
fn borrowed_design_mode_setters_reset_the_target_documents_selection() {
    assert_eq!(
        evaluate(
            r#"(()=>{try{
const frame=document.createElement('iframe');document.body.append(frame);
const child=frame.contentWindow,doc=frame.contentDocument;
child.eval(`globalThis.editingRange=document.createRange();const text=document.createTextNode('abc');document.body.append(text);editingRange.setStart(text,1);editingRange.setEnd(text,2);globalThis.editingSelection=getSelection();editingSelection.removeAllRanges();editingSelection.addRange(editingRange);document.getSelection=()=>{throw Error('public selection')};editingRange.setStart=editingRange.setEnd=()=>{throw Error('public range')};`);
const setter=Object.getOwnPropertyDescriptor(Document.prototype,'designMode').set;
setter.call(doc,'on');
if(!child.eval(`editingSelection.rangeCount===1&&editingSelection.getRangeAt(0)===editingRange&&editingRange.startContainer===document&&editingRange.endContainer===document&&editingRange.startOffset===0&&editingRange.endOffset===0`))return 'parent-setter-child-selection';
if(doc.activeElement!==doc.documentElement)return 'child-focus';
const text=document.createTextNode('xyz');document.body.append(text);const range=document.createRange();range.setStart(text,1);range.setEnd(text,2);const selection=getSelection();selection.removeAllRanges();selection.addRange(range);
child.eval(`Object.getOwnPropertyDescriptor(Document.prototype,'designMode').set.call(parent.document,'on')`);
if(selection.rangeCount!==1||selection.getRangeAt(0)!==range||range.startContainer!==document||range.endContainer!==document||range.startOffset!==0||range.endOffset!==0)return 'child-setter-parent-selection';
if(document.activeElement!==document.documentElement||document.designMode!=='on'||doc.designMode!=='on')return 'parent-focus';
return 'ok';}catch(e){return e.name+': '+e.message;}})()"#
        ),
        "ok"
    );
}

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
fn input_mutability_controls_read_only_read_write_and_type_invalidation() {
    assert_eq!(
        evaluate(
            r#"(()=>{try{
if(!CSS.supports('selector(:read-only)')||!CSS.supports('selector(:read-write)'))return 'support';
const editable=['text','search','url','tel','email','password','date','month','week','time','datetime-local','number','unknown'];
const fixed=['hidden','range','color','checkbox','radio','file','submit','image','reset','button'];
for(const type of [...editable,...fixed]) {
 const element=document.createElement('input');element.type=type;document.body.append(element);
 const expected=editable.includes(type);
 if(element.matches(':read-write')!==expected||element.matches(':read-only')===expected)return 'type:'+type;
 element.readOnly=true;
 if(element.matches(':read-write')||!element.matches(':read-only'))return 'readonly:'+type;
 element.readOnly=false;element.disabled=true;
 if(element.matches(':read-write')||!element.matches(':read-only'))return 'disabled:'+type;
 element.disabled=false;
 if(element.matches(':read-write')!==expected)return 'restored:'+type;
}
const fieldset=document.createElement('fieldset');fieldset.disabled=true;
fieldset.innerHTML='<legend><input id="first-legend"></legend><legend><input id="later-legend"></legend><input id="fieldset-input">';
document.body.append(fieldset);
if(!fieldset.querySelector('#first-legend').matches(':read-write')||fieldset.querySelector('#later-legend').matches(':read-write')||fieldset.querySelector('#fieldset-input').matches(':read-write'))return 'fieldset';
const style=document.createElement('style');style.textContent='input+span{color:red}input:read-write+span{color:green}';document.head.append(style);
const input=document.createElement('input'),sibling=document.createElement('span');input.type='hidden';document.body.append(input,sibling);
if(getComputedStyle(sibling).color!=='rgb(255, 0, 0)')return 'before';
input.type='text';if(getComputedStyle(sibling).color!=='rgb(0, 128, 0)')return 'type-invalidation';
input.readOnly=true;if(getComputedStyle(sibling).color!=='rgb(255, 0, 0)')return 'readonly-invalidation';
return 'ok';
}catch(e){return e.name+': '+e.message;}})()"#
        ),
        "ok"
    );
}

#[test]
fn contenteditable_reflects_keywords_and_inherits_after_tree_moves() {
    assert_eq!(
        evaluate(
            r#"(()=>{try{
const parent=document.createElement('div'),child=document.createElement('span');parent.append(child);document.body.append(parent);
if(parent.contentEditable!=='inherit'||parent.isContentEditable!==false||!parent.matches(':read-only'))return 'initial';
for(const [source,state] of [['TrUe','true'],['PLAINTEXT-ONLY','plaintext-only'],['FaLsE','false']]) {
 parent.contentEditable=source;
 if(parent.contentEditable!==state||parent.getAttribute('contenteditable')!==state)return 'reflect:'+source;
 const expected=state!=='false';
 if(parent.isContentEditable!==expected||child.isContentEditable!==expected||child.matches(':read-write')!==expected)return 'inherit:'+source;
}

parent.contentEditable='true';child.contentEditable='false';
if(child.isContentEditable||child.matches(':read-write'))return 'false-boundary';
child.contentEditable='INHERIT';
if(child.hasAttribute('contenteditable')||!child.isContentEditable)return 'remove-inherit';
for(const value of ['', 'yes', ' true ', 'typing']) {
 try{parent.contentEditable=value;return 'accepted:'+value;}catch(e){if(e.name!=='SyntaxError')return 'error:'+value;}
 if(parent.contentEditable!=='true')return 'changed-after-error';
}
parent.setAttribute('contenteditable','invalid');
if(parent.contentEditable!=='inherit'||parent.isContentEditable||child.isContentEditable)return 'invalid-attribute';
parent.setAttribute('contenteditable','');
if(parent.contentEditable!=='true'||!child.matches(':read-write'))return 'empty-attribute';
const elsewhere=document.createElement('div');document.body.append(elsewhere);elsewhere.append(child);
if(child.isContentEditable||!child.matches(':read-only'))return 'reparent';
return 'ok';
}catch(e){return e.name+': '+e.message;}})()"#
        ),
        "ok"
    );
}

#[test]
fn design_mode_resets_active_range_before_focus_and_ignores_repeated_on() {
    assert_eq!(
        evaluate(
            r#"(()=>{try{
if(document.designMode!=='off')return 'default';
const input=document.createElement('input'),text=document.createTextNode('abc');
document.body.append(input,text);input.focus();
const selection=document.getSelection(),range=document.createRange();
range.setStart(text,1);range.setEnd(text,2);selection.addRange(range);
let observed='missing';
input.onblur=()=>{observed=document.designMode+':'+(range.startContainer===document)+':'+range.startOffset;};
document.designMode='ON';
if(document.designMode!=='on'||!document.body.isContentEditable||!document.body.matches(':read-write'))return 'enabled';
if(selection.rangeCount!==1||selection.getRangeAt(0)!==range||range.startContainer!==document||range.endContainer!==document||range.startOffset!==0||range.endOffset!==0)return 'active-range';
if(observed!=='on:true:0'||document.activeElement!==document.documentElement)return 'focus:'+observed+':'+document.activeElement.tagName;
selection.collapse(text,2);input.focus();document.designMode='on';
if(selection.anchorNode!==text||selection.anchorOffset!==2||document.activeElement!==input)return 'repeat-on';
for(const value of ['invalid',' on ','']){document.designMode=value;if(document.designMode!=='on')return 'invalid';}
document.designMode='OFF';
if(document.designMode!=='off'||document.body.isContentEditable||selection.anchorNode!==text||selection.anchorOffset!==2)return 'off';
return 'ok';}catch(e){return e.name+': '+e.message;}})()"#
        ),
        "ok"
    );
}

#[test]
fn editing_uses_ordinary_parents_and_preserves_control_precedence() {
    assert_eq!(
        evaluate(
            r#"(()=>{try{
const host=document.createElement('div');host.contentEditable='plaintext-only';
host.innerHTML='<span id="child"></span><div contenteditable="false"><b id="blocked"></b></div><input readonly><textarea disabled></textarea><input type="checkbox">';
document.body.append(host);
if(!host.querySelector('#child').isContentEditable||host.querySelector('#blocked').isContentEditable)return 'inheritance';
for(const control of host.querySelectorAll('input,textarea'))if(!control.isContentEditable||control.matches(':read-write')||!control.matches(':read-only'))return 'control-precedence';
const root=host.attachShadow({mode:'open'}),shadowChild=document.createElement('span');root.append(shadowChild);
if(shadowChild.isContentEditable||shadowChild.matches(':read-write'))return 'shadow-boundary';
shadowChild.contentEditable='true';if(!shadowChild.isContentEditable)return 'shadow-host';
const svg=document.createElementNS('http://www.w3.org/2000/svg','svg'),g=document.createElementNS(svg.namespaceURI,'g');svg.append(g);host.append(svg);
const math=document.createElementNS('http://www.w3.org/1998/Math/MathML','math'),mi=document.createElementNS(math.namespaceURI,'mi');math.append(mi);host.append(math);
const foreign=document.createElementNS('urn:example','foreign');host.append(foreign);
if(!svg.matches(':read-write')||!math.matches(':read-write')||g.matches(':read-write')||mi.matches(':read-write')||foreign.matches(':read-write'))return 'node-kinds';
const child=host.querySelector('#child');const clone=host.cloneNode(true);
if(!clone.querySelector('#child').isContentEditable)return 'clone';
host.remove();if(!host.isContentEditable||!child.isContentEditable)return 'detached';
const other=document.implementation.createHTMLDocument('other');other.body.append(other.adoptNode(child));
if(child.isContentEditable||!child.matches(':read-only'))return 'adopt';
return 'ok';}catch(e){return e.name+': '+e.message;}})()"#
        ),
        "ok"
    );
}

#[test]
fn editing_idl_uses_private_brand_attributes_and_dom_string_conversion() {
    assert_eq!(
        evaluate(
            r#"(()=>{try{
const element=document.createElement('div'),span=document.createElement('span');element.append(span);document.body.append(element);
const content=Object.getOwnPropertyDescriptor(HTMLElement.prototype,'contentEditable');
const editable=Object.getOwnPropertyDescriptor(HTMLElement.prototype,'isContentEditable');
const mode=Object.getOwnPropertyDescriptor(Document.prototype,'designMode');
for(const descriptor of [content,editable,mode])if(!descriptor.enumerable||!descriptor.configurable)return 'idl-descriptor';
for(const [descriptor,receiver] of [[content,{}],[content,document],[editable,{}],[mode,{}],[mode,element]]){
 try{descriptor.get.call(receiver);return 'getter-brand';}catch(e){if(e.name!=='TypeError')return 'getter-error';}
}
let converted=false;try{content.set.call({}, {toString(){converted=true;return 'true'}});return 'setter-brand';}catch(e){if(e.name!=='TypeError'||converted)return 'brand-order';}
for(const setter of [v=>{element.contentEditable=v},v=>{document.designMode=v}]){
 try{setter(Symbol());return 'symbol';}catch(e){if(e.name!=='TypeError')return 'symbol-error';}
}
Object.defineProperties(element,{getAttribute:{value(){throw 'public-get'}},setAttribute:{value(){throw 'public-set'}},removeAttributeNS:{value(){throw 'public-remove'}}});
element.contentEditable='TRUE';if(element.contentEditable!=='true'||!span.isContentEditable)return 'private-read';
element.contentEditable='inherit';if(element.contentEditable!=='inherit'||span.isContentEditable)return 'private-remove';
for(const selector of [':read-only(x)',':read-write()',':read-write(1)'])if(CSS.supports('selector('+selector+')'))return 'function-support';
if(CSS.supports('selector(::before:read-write)'))return 'pseudo-placement';
if(typeof globalThis.__omoikane_editability!=='undefined'||typeof globalThis.__omoikane_document_design_mode!=='undefined')return 'bridge-leak';
return 'ok';}catch(e){return e.name+': '+e.message;}})()"#
        ),
        "ok"
    );
}
