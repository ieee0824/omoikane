use omoikane::{html::TreeBuilder, js::JsRuntime};

#[test]
fn opened_blank_window_has_a_separate_document_and_self_parent() {
    let document = TreeBuilder::parse("<html><body></body></html>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "http://example.test/page").unwrap();
    assert!(
        runtime
            .eval(
                r#"
            const popup = window.open('', 'child');
            popup !== null && popup !== window && popup.document !== document &&
              popup.document.URL === 'about:blank' &&
              popup.opener === window && popup.parent === popup && popup.top === popup &&
              document.querySelectorAll('iframe').length === 0;
        "#,
            )
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn opened_window_can_reply_to_its_opener() {
    let document = TreeBuilder::parse("<html><body></body></html>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "http://example.test/page").unwrap();
    runtime
        .eval(
            r#"
            globalThis.replies = [];
            window.addEventListener('message', event => replies.push({
              data: event.data, source: event.source === popup,
            }));
            globalThis.popup = window.open('', 'popup');
            popup.eval("opener.postMessage('reply', '*')");
        "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .eval("replies.length === 1 && replies[0].data === 'reply' && replies[0].source")
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn opener_posts_to_popup_realm_asynchronously() {
    let document = TreeBuilder::parse("<html><body></body></html>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "http://example.test/page").unwrap();
    assert!(
        runtime
            .eval(
                r#"
            const popup = window.open('', 'receiver');
            popup.received = [];
            popup.addEventListener('message', event => popup.received.push({
              data: event.data, source: event.source === window,
              origin: event.origin,
            }));
            popup.postMessage({ value: 7 }, '*');
            popup.received.length === 0;
        "#,
            )
            .unwrap()
            .to_boolean()
    );
    runtime.run_until_idle().unwrap();
    assert!(runtime
        .eval("popup.received.length === 1 && popup.received[0].data.value === 7 && popup.received[0].source && popup.received[0].origin === 'http://example.test'")
        .unwrap()
        .to_boolean());
}

#[test]
fn opener_transfers_message_port_to_popup_realm() {
    let document = TreeBuilder::parse("<html><body></body></html>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "http://example.test/page").unwrap();
    runtime
        .eval(
            r#"
            globalThis.replies = [];
            const popup = window.open('', 'port');
            popup.eval(`addEventListener('message', event => {
              if (event.data.port === event.ports[0]) event.ports[0].postMessage('popup-reply');
            })`);
            const channel = new MessageChannel();
            channel.port1.onmessage = event => replies.push(event.data);
            popup.postMessage({ port: channel.port2 }, '*', [channel.port2]);
        "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .eval("replies.length === 1 && replies[0] === 'popup-reply'")
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn named_popup_reuses_its_window_proxy_and_close_retires_it() {
    let document = TreeBuilder::parse("<html><body></body></html>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "http://example.test/page").unwrap();
    assert!(
        runtime
            .eval(
                r#"
            const first = window.open('', 'reused');
            const second = window.open('', 'reused');
            const same = first === second && !first.closed;
            first.close();
            const retired = first.closed;
            const third = window.open('', 'reused');
            same && retired && third !== first && !third.closed;
        "#,
            )
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn popup_navigation_keeps_proxy_and_runs_new_document_scripts() {
    let document = TreeBuilder::parse("<html><body></body></html>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "http://example.test/page").unwrap();
    runtime
        .eval(
            r#"
            globalThis.replies = [];
            addEventListener('message', event => replies.push(event.data));
            globalThis.popup = window.open('', 'navigated');
            globalThis.oldDocument = popup.document;
            popup.location.href = 'data:text/html,<script>onload=()=>opener.postMessage("navigated","*")</script>';
            globalThis.initialBlank = oldDocument.URL === 'about:blank';
        "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime
        .eval("initialBlank && window.open('', 'navigated') === popup && replies.includes('navigated') && popup.closed === false")
        .unwrap()
        .to_boolean());
}

#[test]
fn closing_popup_fires_hidden_visibility_change_before_retirement() {
    let document = TreeBuilder::parse("<html><body></body></html>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "http://example.test/page").unwrap();
    runtime
        .eval(
            r#"
            globalThis.replies = [];
            addEventListener('message', event => replies.push(event.data));
            const popup = window.open('', 'closing');
            popup.eval(`document.addEventListener('visibilitychange', event => {
              opener.postMessage({state: document.visibilityState, bubbles: event.bubbles}, '*');
            })`);
            popup.close();
        "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime
        .eval("replies.length === 1 && replies[0].state === 'hidden' && replies[0].bubbles === true")
        .unwrap()
        .to_boolean());
}

#[test]
fn cross_origin_popup_exposes_only_safe_window_access_and_can_reply() {
    let document = TreeBuilder::parse("<html><body></body></html>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "http://example.test/page").unwrap();
    runtime
        .eval(
            r#"
            globalThis.replies = [];
            addEventListener('message', event => replies.push({
              data: event.data, origin: event.origin, source: event.source === popup,
            }));
            globalThis.popup = window.open(
              'data:text/html,<script>addEventListener("message", e => opener.postMessage({sourceIsOpener:e.source===opener,openerDocumentHidden:opener.document===undefined}, "*"))</script>',
              'cross');
        "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .eval(
                r#"
            let blocked = false;
            try { void popup.document; } catch (error) { blocked = error.name === 'SecurityError'; }
            blocked && popup.closed === false && typeof popup.postMessage === 'function';
        "#,
            )
            .unwrap()
            .to_boolean()
    );
    runtime.eval("popup.postMessage('ping', '*')").unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime
        .eval("replies.length === 1 && replies[0].data.sourceIsOpener && replies[0].data.openerDocumentHidden && replies[0].origin === 'null' && replies[0].source")
        .unwrap()
        .to_boolean());
}

#[test]
fn reserved_window_targets_reuse_the_current_context() {
    let document = TreeBuilder::parse("<html><body></body></html>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "http://example.test/page").unwrap();
    assert!(runtime
        .eval(
            r#"
            const popup = window.open('', 'reserved');
            window.open('', '_self') === window &&
              window.open('', '_top') === top &&
              popup.eval("open('', '_self') === window && open('', '_parent') === parent && open('', '_top') === top");
        "#,
        )
        .unwrap()
        .to_boolean());
}

#[test]
fn closing_popup_discards_its_queued_message() {
    let document = TreeBuilder::parse("<html><body></body></html>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "http://example.test/page").unwrap();
    runtime
        .eval(
            r#"
            globalThis.received = [];
            const popup = window.open('', 'stale');
            popup.addEventListener('message', event => received.push(event.data));
            popup.postMessage('stale');
            popup.close();
        "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .eval("received.length === 0 && popup.closed")
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn closing_popup_cancels_its_timer() {
    let document = TreeBuilder::parse("<html><body></body></html>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "http://example.test/page").unwrap();
    runtime
        .eval(
            r#"
            globalThis.closedTimerFired = false;
            const popup = window.open('', 'timer');
            popup.eval('setTimeout(() => opener.closedTimerFired = true, 1)');
            popup.close();
        "#,
        )
        .unwrap();
    runtime.run_timers(10, 1, 100);
    assert!(!runtime.eval("closedTimerFired").unwrap().to_boolean());
    assert!(runtime.take_task_errors().is_empty());
}

#[test]
fn popup_navigation_runs_external_classic_script() {
    let document = TreeBuilder::parse("<html><body></body></html>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "http://example.test/page").unwrap();
    runtime
        .eval(
            r#"
            globalThis.replies = [];
            addEventListener('message', event => replies.push(event.data));
            window.open(
              'data:text/html,<script src="data:text/javascript,opener.postMessage(%27external%27,%27*%27)"></script>',
              'external-script');
        "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .eval("replies.length === 1 && replies[0] === 'external'")
            .unwrap()
            .to_boolean()
    );
}
