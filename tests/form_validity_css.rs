//! CSS validity uses the same candidates and state as form validation.
use omoikane::{html::TreeBuilder, js::JsRuntime};

fn check(runtime: &mut JsRuntime, source: &str) {
    assert!(
        runtime
            .eval(source)
            .unwrap_or_else(|error| panic!("{error}"))
            .to_boolean(),
        "{source}"
    );
}

#[test]
fn custom_validity_updates_owners_fieldsets_and_computed_style() {
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse(
        "<style>x-field { display:block; width:20px } x-field:invalid { width:40px } form:invalid { color:red }</style>\
         <form id=f><fieldset id=s><x-field id=c></x-field></fieldset></form><form id=g></form>"
    ).document()).unwrap();
    runtime.eval("customElements.define('x-field', class extends HTMLElement { static formAssociated=true; constructor(){super();this.i=this.attachInternals()} }); globalThis.c=document.getElementById('c'); globalThis.f=document.getElementById('f'); globalThis.s=document.getElementById('s'); globalThis.g=document.getElementById('g');").unwrap();
    check(
        &mut runtime,
        "c.matches(':valid') && f.matches(':valid') && s.matches(':valid') && !document.body.matches(':valid,:invalid')",
    );
    runtime
        .eval("c.i.setValidity({customError:true}, 'invalid')")
        .unwrap();
    check(
        &mut runtime,
        "c.matches(':invalid') && f.matches(':invalid') && s.matches(':invalid') && getComputedStyle(c).width === '40px'",
    );
    runtime.eval("c.setAttribute('form', 'g')").unwrap();
    check(
        &mut runtime,
        "f.matches(':valid') && g.matches(':invalid') && s.matches(':invalid')",
    );
    runtime.eval("c.setAttribute('readonly','')").unwrap();
    check(
        &mut runtime,
        "!c.matches(':valid,:invalid') && g.matches(':valid') && s.matches(':valid') && getComputedStyle(c).width === '20px'",
    );
    runtime
        .eval("c.removeAttribute('readonly'); c.i.setValidity({})")
        .unwrap();
    check(
        &mut runtime,
        "c.matches(':valid') && g.matches(':valid') && s.matches(':valid')",
    );
}

#[test]
fn native_controls_and_external_radio_groups_participate_in_css_validity() {
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse(
        "<form id=f><fieldset id=s><input id=t required><input id=r type=radio name=g required>\
         <select id=q required><option value=''>Choose</option><option value=yes>Yes</option></select>\
         <input id=h type=hidden required></fieldset></form><input id=e type=radio name=g form=f>"
    ).document()).unwrap();
    check(
        &mut runtime,
        "document.querySelectorAll('input:invalid').length === 3 && document.querySelector('form').matches(':invalid') && !document.getElementById('h').matches(':valid,:invalid')",
    );
    runtime.eval("document.getElementById('t').value='filled'; document.getElementById('e').checked=true; document.getElementById('q').value='yes'").unwrap();
    check(
        &mut runtime,
        "document.querySelector('form').matches(':valid') && document.querySelectorAll(':invalid').length === 0",
    );
    runtime
        .eval("document.getElementById('t').setCustomValidity('problem')")
        .unwrap();
    check(
        &mut runtime,
        "document.getElementById('t').matches(':invalid') && document.getElementById('s').matches(':invalid')",
    );
    runtime
        .eval("document.getElementById('s').disabled=true")
        .unwrap();
    check(
        &mut runtime,
        "document.querySelector('form').matches(':valid') && document.getElementById('s').matches(':valid') && !document.getElementById('t').matches(':valid,:invalid')",
    );
}

#[test]
fn detached_and_shadow_trees_have_independent_validity_aggregation() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime.eval("globalThis.detached=document.createElement('form'); detached.innerHTML='<input required>'; globalThis.host=document.createElement('div'); document.body.appendChild(host); globalThis.shadow=host.attachShadow({mode:'closed'}); shadow.innerHTML='<form><input required></form>'").unwrap();
    check(
        &mut runtime,
        "detached.matches(':invalid') && shadow.querySelector('form').matches(':invalid') && !document.body.matches(':has(:invalid)')",
    );
    runtime
        .eval("detached.firstChild.value='ok'; shadow.querySelector('input').value='ok'")
        .unwrap();
    check(
        &mut runtime,
        "detached.matches(':valid') && shadow.querySelector('form').matches(':valid')",
    );
}

#[test]
fn unnamed_radios_readonly_checkbox_and_foreign_elements_keep_validation_semantics() {
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse(
        "<form><input id=r type=radio required><input id=c type=checkbox required readonly><svg><form id=foreign></form></svg></form>"
    ).document()).unwrap();
    check(
        &mut runtime,
        "!document.getElementById('r').validity.valueMissing && document.getElementById('r').matches(':valid') && !document.getElementById('c').willValidate && !document.getElementById('c').matches(':valid,:invalid') && !document.getElementById('foreign').matches(':valid,:invalid')",
    );
}

#[test]
fn validation_and_disabled_states_reach_painted_pixels() {
    use omoikane::{cdp::CdpSession, frame::render_browser_frame};
    use serde_json::json;
    let html = include_str!("fixtures/anonymized-form-validity/states.html");
    let url = format!(
        "data:text/html,{}",
        html.bytes()
            .map(|byte| format!("%{byte:02X}"))
            .collect::<String>()
    );
    let mut session = CdpSession::new().unwrap();
    session
        .dispatch("Page.navigate", json!({"url": url}))
        .unwrap();
    for (phase, script, expected) in [
        (
            "initial",
            "void 0",
            [
                [220, 60, 40, 255],
                [40, 160, 80, 255],
                [220, 60, 40, 255],
                [220, 60, 40, 255],
            ],
        ),
        (
            "changed",
            "document.getElementById('native').value='filled'; document.getElementById('custom').internals.setValidity({customError:true}, 'invalid')",
            [
                [220, 60, 40, 255],
                [220, 60, 40, 255],
                [40, 160, 80, 255],
                [220, 60, 40, 255],
            ],
        ),
        (
            "disabled",
            "document.getElementById('group').disabled=true",
            [
                [40, 160, 80, 255],
                [150, 150, 150, 255],
                [150, 150, 150, 255],
                [40, 160, 80, 255],
            ],
        ),
    ] {
        let result = session
            .dispatch("Runtime.evaluate", json!({"expression":script}))
            .unwrap();
        assert!(result.get("exceptionDetails").is_none(), "{result}");
        let frame = render_browser_frame(&mut session, 320, 180, 16).unwrap();
        if let Some(root) = std::env::var_os("OMOIKANE_FORM_VALIDITY_IMAGES") {
            let root = std::path::PathBuf::from(root);
            std::fs::create_dir_all(&root).unwrap();
            let file = std::fs::File::create(
                root.join(format!("anonymized-form-validity.{phase}.actual.png")),
            )
            .unwrap();
            let mut encoder = png::Encoder::new(file, 320, 180);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(frame.pixels())
                .unwrap();
        }
        for ((x, y), color) in [(24, 24), (240, 60), (240, 110), (32, 32)]
            .into_iter()
            .zip(expected)
        {
            let index = (y * 320 + x) * 4;
            assert_eq!(
                &frame.pixels()[index..index + 4],
                color,
                "{phase}: pixel {x},{y}"
            );
        }
    }
}

#[test]
fn radio_validation_and_css_keep_groups_within_each_tree() {
    let mut runtime = JsRuntime::with_document(
        TreeBuilder::parse("<input type=radio name=group checked><div id=host></div>").document(),
    )
    .unwrap();
    runtime.eval("globalThis.detached=document.createElement('div'); detached.innerHTML='<input type=radio name=group required><input type=radio name=group>'; globalThis.shadow=document.getElementById('host').attachShadow({mode:'open'}); shadow.innerHTML='<input type=radio name=group required><input type=radio name=group>'").unwrap();
    for root in ["detached", "shadow"] {
        check(
            &mut runtime,
            &format!(
                "Array.from({root}.querySelectorAll('input')).every(input => input.validity.valueMissing && input.matches(':invalid'))"
            ),
        );
        runtime
            .eval(&format!("{root}.lastChild.checked=true"))
            .unwrap();
        check(&mut runtime, "document.querySelector('input').checked");
        check(
            &mut runtime,
            &format!(
                "Array.from({root}.querySelectorAll('input')).every(input => !input.validity.valueMissing && input.matches(':valid'))"
            ),
        );
    }
}

#[test]
fn select_selectedness_updates_validity_and_computed_style_without_other_mutations() {
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse(
        "<style>select{width:20px}select:invalid{width:40px}</style><form><select required><option value=''>Choose</option><option value=yes>Yes</option></select></form>"
    ).document()).unwrap();
    runtime
        .eval("globalThis.select=document.querySelector('select')")
        .unwrap();
    check(
        &mut runtime,
        "select.matches(':invalid') && getComputedStyle(select).width === '40px'",
    );
    runtime.eval("select.value='yes'").unwrap();
    check(
        &mut runtime,
        "select.matches(':valid') && getComputedStyle(select).width === '20px' && document.querySelector('form').matches(':valid')",
    );
    runtime.eval("select.selectedIndex=0").unwrap();
    check(
        &mut runtime,
        "select.matches(':invalid') && getComputedStyle(select).width === '40px'",
    );
    runtime.eval("select.options[1].selected=true").unwrap();
    check(
        &mut runtime,
        "select.matches(':valid') && getComputedStyle(select).width === '20px'",
    );
}

#[test]
fn radio_selection_separates_form_owners_and_unnamed_controls() {
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse(
        "<form id=a><input id=a1 name=g type=radio required checked><input id=a2 name=g type=radio></form><form id=b><input id=b1 name=g type=radio required checked></form><input id=u1 type=radio checked><input id=u2 type=radio>"
    ).document()).unwrap();
    runtime.eval("document.getElementById('a2').checked=true; document.getElementById('u2').checked=true").unwrap();
    check(
        &mut runtime,
        "!document.getElementById('a1').checked && document.getElementById('a2').checked && document.getElementById('b1').checked && document.getElementById('u1').checked && document.getElementById('u2').checked && document.getElementById('b').matches(':valid')",
    );
}
