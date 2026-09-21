use omoikane::{html::TreeBuilder, js::JsRuntime};

#[test]
fn parser_created_declarative_shadow_roots_follow_js_visibility() {
    let document = TreeBuilder::parse(
        "<div id='open'><template shadowrootmode='open'><span id='target'></span></template></div>\
         <section id='closed'><template shadowrootmode='closed'><b id='secret'></b></template></section>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let result = runtime
        .eval(
            "(() => {\
               const open = document.getElementById('open');\
               const closed = document.getElementById('closed');\
               return open.shadowRoot.getElementById('target') !== null &&\
                 open.querySelector('template') === null &&\
                 document.getElementById('target') === null &&\
                 closed.shadowRoot === null && closed.childNodes.length === 0;\
             })()",
        )
        .unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn inner_html_keeps_declarative_shadow_template_inert() {
    let document = TreeBuilder::parse("<div id='host'></div>").document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let result = runtime
        .eval(
            "(() => {\
               const host = document.getElementById('host');\
               host.innerHTML = '<template shadowrootmode=open><span id=inside></span></template>';\
               return host.shadowRoot === null &&\
                 host.querySelector('template').content.getElementById('inside') !== null;\
             })()",
        )
        .unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}
