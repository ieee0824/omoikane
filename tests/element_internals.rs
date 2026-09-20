use omoikane::{html::TreeBuilder, js::JsRuntime};

fn runtime() -> JsRuntime {
    JsRuntime::with_document(
        TreeBuilder::parse("<body><form id=f><x-field name=answer></x-field></form></body>")
            .document(),
    )
    .unwrap()
}

fn check(runtime: &mut JsRuntime, expression: &str) {
    assert!(
        runtime.eval(expression).unwrap().to_boolean(),
        "{expression}"
    );
}

#[test]
fn internals_are_private_single_attachment_and_require_a_custom_element() {
    let mut runtime = runtime();
    check(
        &mut runtime,
        "typeof HTMLElement.prototype.attachInternals === 'function' && typeof ElementInternals === 'function'",
    );
    check(
        &mut runtime,
        r#"(() => {
        function error(fn, name) { try { fn(); return false; } catch(e) { return e.name === name; } }
        if (!error(() => new ElementInternals(), 'TypeError')) return false;
        if (!error(() => document.createElement('div').attachInternals(), 'NotSupportedError')) return false;
        if (!error(() => document.createElement('not-defined').attachInternals(), 'NotSupportedError')) return false;
        class Plain extends HTMLElement { constructor() { super(); this.internals=this.attachInternals(); } }
        customElements.define('plain-element', Plain);
        const plain = new Plain();
        if (!(plain.internals instanceof ElementInternals)) return false;
        if (!error(() => plain.attachInternals(), 'NotSupportedError')) return false;
        plain.__customElementDefinition.formAssociated = true;
        if (!error(() => plain.internals.setFormValue('x'), 'NotSupportedError')) return false;
        class Disabled extends HTMLElement { static disabledFeatures = ['internals']; }
        customElements.define('disabled-internals', Disabled);
        return error(() => new Disabled().attachInternals(), 'NotSupportedError');
    })()"#,
    );
}

#[test]
fn custom_control_contributes_values_and_blocks_invalid_form_submission() {
    let mut runtime = runtime();
    runtime.eval(r#"
        globalThis.events=[];
        class Field extends HTMLElement {
            static formAssociated = true;
            constructor() { super(); this.internals=this.attachInternals(); this.internals.setFormValue('forty-two'); }
        }
        customElements.define('x-field', Field);
        globalThis.form=document.getElementById('f');
        globalThis.field=form.querySelector('x-field');
        field.addEventListener('invalid', e => events.push('invalid'));
        form.onsubmit=e=>{events.push('submit');e.preventDefault()};
    "#).unwrap();
    check(
        &mut runtime,
        "field.internals.form === form && field.internals.willValidate && form.elements[0] === field && new FormData(form).get('answer') === 'forty-two'",
    );
    runtime
        .eval("field.internals.setValidity({customError:true},'Fix this'); form.requestSubmit()")
        .unwrap();
    check(
        &mut runtime,
        "!field.internals.validity.valid && field.internals.validationMessage === 'Fix this' && events.join() === 'invalid'",
    );
    runtime
        .eval("field.internals.setValidity({}); form.requestSubmit()")
        .unwrap();
    check(
        &mut runtime,
        "field.internals.validity.valid && events.join() === 'invalid,submit'",
    );
}

#[test]
fn form_callbacks_follow_association_disabled_changes_and_reset() {
    let mut runtime = runtime();
    runtime.eval(r#"
        globalThis.events=[];
        class Field extends HTMLElement {
            static formAssociated = true;
            constructor() { super(); this.internals=this.attachInternals(); }
            formAssociatedCallback(form) { events.push('form:'+ (form ? form.id : 'null')); }
            formDisabledCallback(disabled) { events.push('disabled:'+disabled); }
            formResetCallback() { events.push('reset'); this.internals.setFormValue('reset value'); }
        }
        customElements.define('x-field', Field);
        globalThis.form=document.getElementById('f');
        globalThis.field=form.querySelector('x-field');
    "#).unwrap();
    check(&mut runtime, "events.join() === 'form:f'");
    runtime.eval("field.setAttribute('disabled',''); field.removeAttribute('disabled'); form.reset(); field.remove()").unwrap();
    check(
        &mut runtime,
        "events.join() === 'form:f,disabled:true,disabled:false,reset,form:null' && field.internals.form === null",
    );
}

#[test]
fn explicit_owners_track_id_collisions_moves_and_detached_forms() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"
        globalThis.events=[];
        customElements.define('x-field', class extends HTMLElement {
            static formAssociated = true;
            constructor() { super(); this.i=this.attachInternals(); this.i.setFormValue('v'); }
            formAssociatedCallback(form) { events.push(form ? form.id : null); }
        });
        globalThis.field=document.querySelector('x-field');
        globalThis.first=document.getElementById('f');
        globalThis.second=document.createElement('form'); second.id='second';
        document.body.append(second);
        second.append(field);
    "#,
        )
        .unwrap();
    check(
        &mut runtime,
        "JSON.stringify(events) === '[\"f\",null,\"second\"]' && field.i.form === second",
    );
    runtime.eval("field.setAttribute('form','f')").unwrap();
    check(
        &mut runtime,
        "field.i.form === first && first.elements[0] === field && second.elements.length === 0",
    );
    runtime.eval("globalThis.collision=document.createElement('div');collision.id='f';document.body.insertBefore(collision,document.body.firstChild)").unwrap();
    check(
        &mut runtime,
        "field.i.form === null && first.elements.length === 0",
    );
    runtime
        .eval("collision.remove();first.id='renamed'")
        .unwrap();
    check(&mut runtime, "field.i.form === null");
    runtime.eval("first.id='f';second.remove()").unwrap();
    check(
        &mut runtime,
        "field.i.form === second && second.elements[0] === field && new FormData(second).get('answer') === 'v'",
    );
}

#[test]
fn disabled_fieldsets_and_first_legend_reordering_update_callbacks() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"
        customElements.define('x-field', class extends HTMLElement {
            static formAssociated = true;
            constructor() { super(); this.i=this.attachInternals(); this.disabledEvents=[]; }
            formDisabledCallback(disabled) { this.disabledEvents.push(disabled); }
        });
        globalThis.field=document.querySelector('x-field');
        globalThis.set=document.createElement('fieldset'); set.disabled=true;
        globalThis.legend=document.createElement('legend');
        set.append(legend); legend.append(field); document.body.append(set);
    "#,
        )
        .unwrap();
    check(
        &mut runtime,
        "field.i.willValidate && field.disabledEvents.length === 0",
    );
    runtime.eval("globalThis.other=document.createElement('legend'); set.insertBefore(other,set.firstChild)").unwrap();
    check(
        &mut runtime,
        "!field.i.willValidate && field.disabledEvents.join() === 'true'",
    );
    runtime
        .eval("field.setAttribute('disabled','');other.remove();field.removeAttribute('disabled')")
        .unwrap();
    check(
        &mut runtime,
        "field.i.willValidate && field.disabledEvents.join() === 'true,false'",
    );
    runtime.eval("set.append(field); field.remove()").unwrap();
    check(
        &mut runtime,
        "field.i.willValidate && field.disabledEvents.join() === 'true,false,true,false'",
    );
}

#[test]
fn submission_clones_formdata_and_preserves_file_and_null_values() {
    let mut runtime = runtime();
    runtime.eval(r#"
        customElements.define('x-field', class extends HTMLElement {
            static formAssociated=true;
            constructor() { super(); this.i=this.attachInternals(); }
        });
        globalThis.field=document.querySelector('x-field');
        globalThis.form=document.getElementById('f');
        globalThis.file=new File(['abc'],'sample.txt',{type:'text/plain',lastModified:1});
        globalThis.data=new FormData(); data.append('repeated','one'); data.append('repeated','two'); data.append('file',file);
        field.removeAttribute('name'); field.i.setFormValue(data); data.set('repeated','later');
    "#).unwrap();
    check(
        &mut runtime,
        "new FormData(form).getAll('repeated').join() === 'one,two' && new FormData(form).get('file') === file",
    );
    runtime.eval("field.i.setFormValue('unnamed')").unwrap();
    check(&mut runtime, "Array.from(new FormData(form)).length === 0");
    runtime
        .eval("field.setAttribute('name','answer');field.i.setFormValue(file)")
        .unwrap();
    check(&mut runtime, "new FormData(form).get('answer') === file");
    runtime.eval("field.i.setFormValue(null)").unwrap();
    check(&mut runtime, "Array.from(new FormData(form)).length === 0");
    runtime.eval(r"field.i.setFormValue('\ud800')").unwrap();
    check(
        &mut runtime,
        r"new FormData(form).get('answer') === '\ufffd'",
    );
}

#[test]
fn labels_are_live_and_shadow_roots_do_not_leak_form_ownership() {
    let mut runtime = runtime();
    runtime.eval(r#"
        customElements.define('x-field', class extends HTMLElement {
            static formAssociated=true;
            constructor() { super(); this.i=this.attachInternals(); }
        });
        globalThis.field=document.querySelector('x-field'); field.id='control';
        globalThis.labels=field.i.labels;
        globalThis.label=document.createElement('label'); label.htmlFor='control'; document.body.append(label);
    "#).unwrap();
    check(
        &mut runtime,
        "labels === field.i.labels && labels instanceof NodeList && labels.length === 1 && labels.item(0) === label && label.control === field",
    );
    runtime
        .eval("label.remove();label.removeAttribute('for');label.append(field)")
        .unwrap();
    check(
        &mut runtime,
        "labels.length === 1 && labels[0] === label && field.i.form === null",
    );
    runtime.eval("globalThis.host=document.createElement('div');document.getElementById('f').append(host);globalThis.shadow=host.attachShadow({mode:'closed'});shadow.append(field)").unwrap();
    check(&mut runtime, "field.i.form === null && labels.length === 0");
    runtime.eval("globalThis.inner=document.createElement('form');inner.id='inside';shadow.append(inner);field.setAttribute('form','inside')").unwrap();
    check(
        &mut runtime,
        "field.i.form === inner && inner.elements[0] === field",
    );
}

#[test]
fn canceled_reset_skips_callbacks_and_reentrant_reset_is_guarded() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"
        globalThis.resets=0;
        customElements.define('x-field', class extends HTMLElement {
            static formAssociated=true;
            constructor() { super(); this.i=this.attachInternals(); this.i.setFormValue('old'); }
            formResetCallback() { resets++; this.i.setFormValue('new'); }
        });
        globalThis.form=document.getElementById('f');
        form.addEventListener('reset',()=>form.reset());
        form.addEventListener('reset',e=>e.preventDefault(),{once:true}); form.reset();
    "#,
        )
        .unwrap();
    check(
        &mut runtime,
        "resets === 0 && new FormData(form).get('answer') === 'old'",
    );
    runtime.eval("form.reset()").unwrap();
    check(
        &mut runtime,
        "resets === 1 && new FormData(form).get('answer') === 'new'",
    );
}

#[test]
fn internals_and_submission_values_keep_their_brand_across_realms() {
    let mut runtime = runtime();
    runtime.eval(r#"
        customElements.define('x-field', class extends HTMLElement {
            static formAssociated=true;
            constructor() { super(); this.i=this.attachInternals(); }
        });
        globalThis.field=document.querySelector('x-field');
        globalThis.form=document.getElementById('f');
        globalThis.frame=document.createElement('iframe'); document.body.append(frame);
        globalThis.other=frame.contentWindow;
        globalThis.foreignFile=other.eval("new File(['payload'],'foreign.txt',{type:'text/plain',lastModified:7})");
        globalThis.foreignData=new other.FormData(); foreignData.append('file',foreignFile);
        other.ElementInternals.prototype.setFormValue.call(field.i,foreignData);
        foreignData.set('file','changed');
    "#).unwrap();
    check(
        &mut runtime,
        "new FormData(form).get('file') === foreignFile && new FormData(form).get('file').size === 7",
    );
    runtime.eval("field.i.setFormValue(foreignFile)").unwrap();
    check(
        &mut runtime,
        "new FormData(form).get('answer') === foreignFile",
    );
    check(
        &mut runtime,
        r#"(() => {
        const read=Object.getOwnPropertyDescriptor(other.ElementInternals.prototype,'form').get;
        return read.call(field.i) === form;
    })()"#,
    );
    check(
        &mut runtime,
        r#"(() => {
        const fake=document.createElement('fake-field');
        fake.__customElementState='custom';fake.__customElementDefinition=field.__customElementDefinition;
        try { fake.attachInternals(); return false; } catch(e) { return e.name==='NotSupportedError'; }
    })()"#,
    );
}

#[test]
fn live_form_collections_preserve_duplicate_names_and_reset_reaction_timing() {
    let mut runtime = runtime();
    runtime.eval(r#"
        globalThis.events=[];
        customElements.define('x-field', class extends HTMLElement {
            static formAssociated=true;
            constructor() { super(); this.i=this.attachInternals(); }
            formResetCallback() { events.push(document.querySelector('output').value); }
        });
        globalThis.form=document.getElementById('f');
        form.insertAdjacentHTML('beforeend','<x-field name=answer></x-field><output>default</output><input type=reset>');
        globalThis.collection=form.elements;
        globalThis.group=collection.namedItem('answer');
    "#).unwrap();
    check(
        &mut runtime,
        "group instanceof RadioNodeList && group instanceof NodeList && group.length === 2 && collection.absent === undefined && collection.namedItem('absent') === null",
    );
    runtime.eval("form.querySelector('output').value='changed';form.querySelector('input').click();events.push('click returned')").unwrap();
    check(&mut runtime, "events.join() === 'click returned'");
    runtime.run_jobs().unwrap();
    check(
        &mut runtime,
        "events.join() === 'click returned,default,default'",
    );
    runtime
        .eval("form.querySelector('x-field').remove()")
        .unwrap();
    check(
        &mut runtime,
        "group.length === 1 && collection.namedItem('answer') === group[0]",
    );
}

#[test]
fn custom_disabled_state_is_available_to_native_css_matching() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"
        customElements.define('x-field', class extends HTMLElement { static formAssociated=true; });
        globalThis.field=document.querySelector('x-field');
        globalThis.set=document.createElement('fieldset');
        set.append(field);document.body.append(set);
    "#,
        )
        .unwrap();
    check(
        &mut runtime,
        "field.matches(':enabled') && !field.matches(':disabled')",
    );
    runtime.eval("set.disabled=true").unwrap();
    check(
        &mut runtime,
        "field.matches(':disabled') && !field.matches(':enabled')",
    );
    runtime
        .eval(
            "set.insertAdjacentHTML('afterbegin','<legend></legend>');set.firstChild.append(field)",
        )
        .unwrap();
    check(
        &mut runtime,
        "field.matches(':enabled') && !field.matches(':disabled')",
    );
    check(
        &mut runtime,
        "!document.createElement('undefined-field').matches(':enabled') && typeof __omoikane_set_form_associated_custom === 'undefined'",
    );
}

#[test]
fn multipart_submission_sends_cross_realm_file_bytes_and_cloned_entries() {
    use omoikane::js::NavigationRequest;

    let mut runtime = runtime();
    runtime.eval(r#"
        customElements.define('x-field', class extends HTMLElement {
            static formAssociated=true;
            constructor() { super(); this.i=this.attachInternals(); }
        });
        globalThis.form=document.getElementById('f');
        form.action='https://example.test/upload'; form.method='post'; form.enctype='multipart/form-data';
        const frame=document.createElement('iframe');document.body.append(frame);
        const other=frame.contentWindow;
        const values=new other.FormData();
        values.append('note','one');values.append('note','two');
        values.append('file',other.eval("new File([new Uint8Array([0,1,255])],'data.bin',{type:'application/octet-stream'})"));
        form.querySelector('x-field').i.setFormValue(values);
        values.set('note','changed'); form.requestSubmit();
    "#).unwrap();
    runtime.run_until_idle().unwrap();
    let requests = runtime.take_navigation_requests();
    let [
        NavigationRequest::FormSubmit {
            url,
            method,
            body: Some(body),
            content_type: Some(content_type),
        },
    ] = requests.as_slice()
    else {
        panic!("expected one multipart request, got {requests:?}");
    };
    assert_eq!(url, "https://example.test/upload");
    assert_eq!(method, "POST");
    let boundary = content_type
        .strip_prefix("multipart/form-data; boundary=")
        .unwrap();
    let mut expected = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"note\"\r\n\r\none\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"note\"\r\n\r\ntwo\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"data.bin\"\r\n\
         Content-Type: application/octet-stream\r\n\r\n"
    )
    .into_bytes();
    expected.extend_from_slice(&[0, 1, 255]);
    expected.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    assert_eq!(body, &expected);
}

#[test]
fn adjacent_html_keeps_iframe_and_form_siblings_for_targeted_submission() {
    let mut runtime = runtime();
    runtime.eval(r#"
        document.body.insertAdjacentHTML('afterbegin',
          '<iframe name="form-target"></iframe><form action="/blank" target="form-target"><x-field></x-field></form>');
        globalThis.frame=document.getElementsByName('form-target')[0];
    "#).unwrap();
    check(
        &mut runtime,
        "frame instanceof HTMLIFrameElement && frame.nextSibling instanceof HTMLFormElement && frame.nextSibling.previousSibling === frame",
    );
}

#[test]
fn parser_form_owner_survives_table_construction_but_not_dom_reparenting() {
    let mut runtime = JsRuntime::with_document(
        TreeBuilder::parse(
            "<body><table><form id=outside><tr><td><select id=s></select></td></tr>\
         <tr><td><x-field id=c></x-field></td></tr><tr><td><input id=i></td></tr>\
         </form></table><form id=other></form></body>",
        )
        .document(),
    )
    .unwrap();
    runtime
        .eval(
            r#"
        globalThis.form=document.getElementById('outside');
        globalThis.other=document.getElementById('other');
        globalThis.input=document.getElementById('i');
        globalThis.select=document.getElementById('s');
        customElements.define('x-field', class extends HTMLElement {
            static formAssociated=true;
            constructor() { super(); this.i=this.attachInternals(); }
        });
    "#,
        )
        .unwrap();
    for expression in [
        "form.elements.length === 2",
        "form.elements[0] === select",
        "form.elements[1] === input",
        "input.form === form",
        "select.form === form",
        "document.getElementById('c').i.form === null",
    ] {
        check(&mut runtime, expression);
    }
    runtime
        .eval("input.parentElement.remove();document.querySelector('table').append(input)")
        .unwrap();
    check(
        &mut runtime,
        "input.form === null && form.elements.length === 1",
    );
    runtime.eval("select.setAttribute('form','other')").unwrap();
    check(
        &mut runtime,
        "select.form === other && other.elements[0] === select",
    );
    runtime.eval("select.removeAttribute('form')").unwrap();
    check(
        &mut runtime,
        "select.form === null && form.elements.length === 0",
    );
}

#[test]
fn moving_a_table_away_and_back_clears_its_parser_form_associations() {
    let mut runtime = JsRuntime::with_document(
        TreeBuilder::parse(
            "<body><div id=box><table><form id=f><tr><td><input id=i></table></div></body>",
        )
        .document(),
    )
    .unwrap();
    runtime.eval("globalThis.input=document.getElementById('i');globalThis.box=document.getElementById('box');globalThis.boxParent=box.parentNode").unwrap();
    check(&mut runtime, "input.form === document.getElementById('f')");
    runtime.eval("box.remove();boxParent.append(box)").unwrap();
    check(
        &mut runtime,
        "input.form === null && document.getElementById('f').elements.length === 0",
    );
}

#[test]
fn saved_state_restores_after_the_old_runtime_is_dropped_and_a_late_upgrade() {
    use omoikane::js::FormStateRestoreMode;

    let mut old = runtime();
    old.eval(
        r#"
        customElements.define('x-field', class extends HTMLElement {
            static formAssociated=true;
            constructor() { super(); this.i=this.attachInternals(); }
        });
        document.querySelector('x-field').i.setFormValue('sent value','restoration value');
    "#,
    )
    .unwrap();
    let snapshot = old.capture_form_state().unwrap();
    drop(old);
    boa_gc::force_collect();

    let mut restored = runtime();
    restored
        .restore_form_state(&snapshot, FormStateRestoreMode::Restore)
        .unwrap();
    restored.run_until_idle().unwrap();
    restored.eval(r#"
        globalThis.restorations=[];
        customElements.define('x-field', class extends HTMLElement {
            static formAssociated=true;
            constructor() { super(); this.i=this.attachInternals(); this.i.setFormValue('initial'); }
            formStateRestoreCallback(value,mode) {
                restorations.push([value,mode,this.i.form.id]);
                this.i.setFormValue('restored:'+value);
            }
        });
    "#).unwrap();
    check(&mut restored, "restorations.length === 0");
    restored.run_until_idle().unwrap();
    check(
        &mut restored,
        "JSON.stringify(restorations) === '[[\"restoration value\",\"restore\",\"f\"]]' && new FormData(document.getElementById('f')).get('answer') === 'restored:restoration value'",
    );
    restored.run_until_idle().unwrap();
    check(&mut restored, "restorations.length === 1");
}

#[test]
fn restoration_preserves_files_and_formdata_and_reports_autocomplete_mode() {
    use omoikane::js::FormStateRestoreMode;

    let mut old = runtime();
    old.eval(r#"
        customElements.define('x-field', class extends HTMLElement {
            static formAssociated=true;
            constructor() { super(); this.i=this.attachInternals(); }
        });
        const data=new FormData();data.append('text','first');data.append('text','second');
        data.append('file',new File([new Uint8Array([0,1,255])],'saved.bin',{type:'application/octet-stream',lastModified:42}));
        document.querySelector('x-field').i.setFormValue('submission',data);
        data.set('text','changed after setFormValue');
    "#).unwrap();
    let snapshot = old.capture_form_state().unwrap();
    drop(old);
    boa_gc::force_collect();
    let mut restored = runtime();
    restored
        .eval(
            r#"
        globalThis.restorations=[];
        customElements.define('x-field', class extends HTMLElement {
            static formAssociated=true;
            constructor() { super(); this.i=this.attachInternals(); }
            formStateRestoreCallback(value,mode) { restorations.push({value,mode}); }
        });
        document.querySelector('x-field').setAttribute('disabled','');
    "#,
        )
        .unwrap();
    restored
        .restore_form_state(&snapshot, FormStateRestoreMode::Autocomplete)
        .unwrap();
    restored.run_until_idle().unwrap();
    check(&mut restored, "restorations.length === 0");
    restored
        .eval("document.querySelector('x-field').removeAttribute('disabled')")
        .unwrap();
    restored.run_until_idle().unwrap();
    check(
        &mut restored,
        r#"(() => {
        if (restorations.length !== 1 || restorations[0].mode !== 'autocomplete') return false;
        const value=restorations[0].value;
        if (!(value instanceof FormData) || value.getAll('text').join() !== 'first,second') return false;
        const file=value.get('file');
        return file instanceof File && file.name === 'saved.bin' && file.type === 'application/octet-stream' &&
            file.lastModified === 42 && Array.from(file.__bytes).join() === '0,1,255';
    })()"#,
    );
}

#[test]
fn multipart_normalizes_field_names_but_preserves_file_name_newlines_and_bytes() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"
        customElements.define('x-field', class extends HTMLElement {
            static formAssociated = true;
            constructor() { super(); this.i = this.attachInternals(); }
        });
        const entries = new FormData();
        entries.append('a\nb', 'text');
        entries.append('c\rd', new File([new Uint8Array([0, 1, 255])], 'raw\nname'));
        document.querySelector('x-field').i.setFormValue(entries);
        const form = document.querySelector('form');
        form.method = 'post';
        form.enctype = 'multipart/form-data';
        form.submit();
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    let requests = runtime.take_navigation_requests();
    let [
        omoikane::js::NavigationRequest::FormSubmit {
            body: Some(body), ..
        },
    ] = requests.as_slice()
    else {
        panic!("one form submission required: {requests:?}");
    };
    let text = String::from_utf8_lossy(body);
    assert!(text.contains("name=\"a%0D%0Ab\"\r\n\r\ntext\r\n"));
    assert!(text.contains("name=\"c%0D%0Ad\"; filename=\"raw%0Aname\""));
    assert!(body.windows(7).any(|part| part == b"\r\n\r\n\x00\x01\xff"));
}

#[test]
fn iframe_history_restores_custom_control_state_after_recreating_the_document() {
    let mut runtime = runtime();
    let html = r#"<!doctype html><form><x-restored name=answer></x-restored></form><script>
        globalThis.restorations=[];
        customElements.define('x-restored', class extends HTMLElement {
            static formAssociated=true;
            constructor() { super(); this.i=this.attachInternals(); this.i.setFormValue('initial'); }
            formStateRestoreCallback(value,mode) { restorations.push([value,mode]); this.i.setFormValue('restored:'+value,value); }
        });
        globalThis.control=document.querySelector('x-restored');
    </script>"#;
    runtime.eval(&format!("globalThis.frame=document.createElement('iframe'); frame.srcdoc={}; document.body.appendChild(frame); globalThis.proxy=frame.contentWindow", serde_json::to_string(html).unwrap())).unwrap();
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        "proxy.control.i.form !== null && proxy.restorations.length === 0",
    );
    runtime
        .eval("proxy.control.i.setFormValue('submitted','saved'); frame.srcdoc='<p>other</p>'")
        .unwrap();
    runtime.run_until_idle().unwrap();
    check(&mut runtime, "proxy.document.body.textContent === 'other'");
    runtime.eval("proxy.history.back()").unwrap();
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        "JSON.stringify(proxy.restorations) === '[[\"saved\",\"restore\"]]' && new proxy.FormData(proxy.control.i.form).get('answer') === 'restored:saved'",
    );
    runtime
        .eval("proxy.control.i.setFormValue('sent again','saved again'); proxy.location.reload()")
        .unwrap();
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        "JSON.stringify(proxy.restorations) === '[[\"saved again\",\"restore\"]]'",
    );
}

#[test]
fn iframe_same_document_history_restores_each_entries_control_state() {
    for from_child in [false, true] {
        let mut runtime = runtime();
        let html = r#"<form><x-restored name=answer></x-restored></form><script>
        globalThis.restorations=[];
        customElements.define('x-restored', class extends HTMLElement {
            static formAssociated=true;
            constructor() { super(); this.i=this.attachInternals(); this.i.setFormValue('initial'); }
            formStateRestoreCallback(value,mode) { restorations.push([value,mode]); this.i.setFormValue(value); }
        });
        globalThis.control=document.querySelector('x-restored');
        globalThis.pushEntry=()=>history.pushState({step:2},'');
        globalThis.backEntry=()=>history.back();
        globalThis.forwardEntry=()=>history.forward();
    </script>"#;
        runtime.eval(&format!("globalThis.frame=document.createElement('iframe'); frame.srcdoc={}; document.body.appendChild(frame); globalThis.proxy=frame.contentWindow", serde_json::to_string(html).unwrap())).unwrap_or_else(|error| panic!("{error}"));
        runtime
            .run_until_idle()
            .unwrap_or_else(|error| panic!("{error}"));
        runtime.eval(if from_child {
        "proxy.control.i.setFormValue('first'); proxy.pushEntry(); proxy.control.i.setFormValue('second'); proxy.backEntry()"
    } else {
        "proxy.control.i.setFormValue('first'); proxy.history.pushState({step:2},''); proxy.control.i.setFormValue('second'); proxy.history.back()"
    }).unwrap_or_else(|error| panic!("{error}"));
        runtime
            .run_until_idle()
            .unwrap_or_else(|error| panic!("{error}"));
        check(
            &mut runtime,
            "JSON.stringify(proxy.restorations) === '[[\"first\",\"restore\"]]' && proxy.history.state === null",
        );
        runtime
            .eval(if from_child {
                "proxy.forwardEntry()"
            } else {
                "proxy.history.forward()"
            })
            .unwrap_or_else(|error| panic!("{error}"));
        runtime
            .run_until_idle()
            .unwrap_or_else(|error| panic!("{error}"));
        check(
            &mut runtime,
            "JSON.stringify(proxy.restorations) === '[[\"first\",\"restore\"],[\"second\",\"restore\"]]' && proxy.history.state.step === 2",
        );
    }
}

#[test]
fn iframe_script_reload_restores_its_own_form_state_without_navigating_the_parent() {
    let mut runtime = runtime();
    let html = r#"<form><x-restored name=answer></x-restored></form><script>
        globalThis.restorations=[];
        customElements.define('x-restored', class extends HTMLElement {
            static formAssociated=true;
            constructor() { super(); this.i=this.attachInternals(); this.i.setFormValue('initial'); }
            formStateRestoreCallback(value,mode) { restorations.push([value,mode]); this.i.setFormValue(value); }
        });
        globalThis.control=document.querySelector('x-restored');
        globalThis.reloadSelf=()=>{ control.i.setFormValue('child saved'); location.reload(); };
    </script>"#;
    runtime.eval(&format!("globalThis.frame=document.createElement('iframe'); frame.srcdoc={}; document.body.appendChild(frame); globalThis.proxy=frame.contentWindow", serde_json::to_string(html).unwrap())).unwrap();
    runtime.run_until_idle().unwrap();
    runtime
        .eval("globalThis.oldReload=proxy.reloadSelf; proxy.reloadSelf()")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime.take_navigation_requests().is_empty(),
        "child reload must not enter the top-level navigation queue"
    );
    check(
        &mut runtime,
        "JSON.stringify(proxy.restorations) === '[[\"child saved\",\"restore\"]]'",
    );
    runtime.eval("try { oldReload(); } catch (_) {}").unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_navigation_requests().is_empty());
    check(&mut runtime, "proxy.restorations.length === 1");
}

#[test]
fn discarding_a_restoring_iframe_cancels_its_pending_form_state_callback() {
    let mut runtime = runtime();
    let html = r#"<form><x-restored name=answer></x-restored></form><script>
        customElements.define('x-restored', class extends HTMLElement {
            static formAssociated=true;
            constructor() { super(); this.i=this.attachInternals(); this.i.setFormValue('saved'); }
            formStateRestoreCallback(value,mode) { parent.restorations.push([value,mode]); }
        });
        if(sessionStorage.getItem('restoring') === 'yes') frameElement.remove();
        else sessionStorage.setItem('restoring','yes');
    </script>"#;
    runtime.eval(&format!("globalThis.restorations=[];globalThis.frame=document.createElement('iframe'); frame.srcdoc={}; document.body.appendChild(frame); globalThis.proxy=frame.contentWindow", serde_json::to_string(html).unwrap())).unwrap();
    runtime.run_until_idle().unwrap();
    runtime.eval("proxy.location.reload()").unwrap();
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        "document.querySelector('iframe') === null && restorations.length === 0",
    );
    assert!(runtime.take_task_errors().is_empty());
}
