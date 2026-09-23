use omoikane::{html::TreeBuilder, js::JsRuntime};

fn runtime() -> JsRuntime {
    let document = TreeBuilder::parse("<html><body></body></html>").document();
    JsRuntime::with_document_and_url(document, "http://example.test/page").unwrap()
}

#[test]
fn document_and_open_shadow_root_report_their_own_active_element() {
    let mut runtime = runtime();
    assert!(
        runtime
            .eval(
                r#"
            const host = document.createElement('div');
            document.body.append(host);
            const root = host.attachShadow({mode: 'open'});
            const button = document.createElement('button');
            root.append(button);
            button.focus();
            document.activeElement === host && root.activeElement === button;
        "#,
            )
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn closed_and_nested_shadow_roots_retarget_each_boundary() {
    let mut runtime = runtime();
    assert!(
        runtime
            .eval(
                r#"
            const outerHost = document.createElement('div');
            document.body.append(outerHost);
            const outerRoot = outerHost.attachShadow({mode: 'closed'});
            const innerHost = document.createElement('div');
            outerRoot.append(innerHost);
            const innerRoot = innerHost.attachShadow({mode: 'open'});
            const button = document.createElement('button');
            innerRoot.append(button);
            button.focus();
            outerHost.shadowRoot === null &&
              document.activeElement === outerHost &&
              outerRoot.activeElement === innerHost &&
              innerRoot.activeElement === button;
        "#,
            )
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn slotted_light_dom_focus_stays_in_the_document_tree() {
    let mut runtime = runtime();
    assert!(
        runtime
            .eval(
                r#"
            const host = document.createElement('div');
            const button = document.createElement('button');
            button.slot = 'content';
            host.append(button);
            document.body.append(host);
            const root = host.attachShadow({mode: 'open'});
            root.innerHTML = '<slot name="content"></slot>';
            button.focus();
            document.activeElement === button && root.activeElement === null;
        "#,
            )
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn iframe_and_shadow_focus_are_retargeted_independently() {
    let mut runtime = runtime();
    assert!(
        runtime
            .eval(
                r#"
            const frame = document.createElement('iframe');
            document.body.append(frame);
            const child = frame.contentDocument;
            const host = child.createElement('div');
            child.body.append(host);
            const root = host.attachShadow({mode: 'open'});
            const button = child.createElement('button');
            root.append(button);
            button.focus();
            document.activeElement === frame &&
              child.activeElement === host && root.activeElement === button;
        "#,
            )
            .unwrap()
            .to_boolean()
    );
}
