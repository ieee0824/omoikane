use omoikane::{html::TreeBuilder, js::JsRuntime};

#[test]
fn window_post_message_queues_a_cloned_message_with_source_and_origin() {
    let document = TreeBuilder::parse("<html><body></body></html>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "http://example.test/page").unwrap();
    assert!(
        runtime
            .eval(
                r#"
            globalThis.received = [];
            window.addEventListener('message', event => {
                received.push({ data: event.data.value, origin: event.origin,
                    source: event.source === window, ports: event.ports.length });
            });
            const payload = { value: 1 };
            window.postMessage(payload, '*');
            payload.value = 2;
            received.length === 0;
        "#,
            )
            .unwrap()
            .to_boolean()
    );
    runtime.run_until_idle().unwrap();
    assert!(runtime
        .eval(
            "received.length === 1 && received[0].data === 1 && received[0].origin === 'http://example.test' && received[0].source && received[0].ports === 0"
        )
        .unwrap()
        .to_boolean());
}

#[test]
fn window_onmessage_handler_receives_posted_message() {
    let document = TreeBuilder::parse("<html><body></body></html>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "http://example.test/page").unwrap();
    runtime
        .eval("globalThis.received = []; window.onmessage = event => received.push(event.data); window.postMessage('handler', '*');")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .eval("received.length === 1 && received[0] === 'handler'")
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn window_post_message_enforces_target_origin() {
    let document = TreeBuilder::parse("<html><body></body></html>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "http://example.test/page").unwrap();
    runtime
        .eval(
            r#"
            globalThis.received = [];
            window.addEventListener('message', event => received.push(event.data));
            window.postMessage('blocked', 'https://other.test');
            window.postMessage('same-default');
            window.postMessage('same-explicit', 'http://example.test');
            globalThis.invalidTargetSyntaxError = false;
            try { window.postMessage('invalid', 'relative-path'); }
            catch (error) { invalidTargetSyntaxError = error.name === 'SyntaxError'; }
        "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .eval("invalidTargetSyntaxError && received.join(',') === 'same-default,same-explicit'")
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn iframe_post_message_errors_use_the_target_window_realm() {
    let document = TreeBuilder::parse("<html><body><iframe></iframe></body></html>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "http://example.test/page").unwrap();
    assert!(
        runtime
            .eval(
                r#"
            const child = document.querySelector('iframe').contentWindow;
            const errors = [];
            for (const [message, origin, transfer, expected] of [
              ['data', 'invalid-origin', undefined, 'SyntaxError'],
              [() => {}, '*', undefined, 'DataCloneError'],
            ]) {
              try { child.postMessage(message, origin, transfer); errors.push(false); }
              catch (error) {
                errors.push(error instanceof child.DOMException &&
                  !(error instanceof DOMException) && error.name === expected);
              }
            }
            errors.length === 2 && errors.every(Boolean);
        "#,
            )
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn iframe_and_parent_exchange_messages_with_window_sources() {
    let document = TreeBuilder::parse(
        r#"<html><body><iframe id="child" srcdoc="<script>
            addEventListener('message', event => {
                parent.postMessage({ data: event.data, sourceIsParent: event.source === parent }, '*');
            });
        </script>"></iframe></body></html>"#,
    )
    .document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "http://example.test/page").unwrap();
    runtime
        .eval(
            r#"
            globalThis.replies = [];
            const child = document.getElementById('child').contentWindow;
            addEventListener('message', event => replies.push({
                data: event.data.data,
                sourceIsParent: event.data.sourceIsParent,
                sourceIsChild: event.source === child,
                origin: event.origin,
            }));
            child.postMessage('hello', '*');
        "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    let replies = runtime.eval("JSON.stringify(replies)").unwrap();
    assert!(runtime
        .eval(
            "replies.length === 1 && replies[0].data === 'hello' && replies[0].sourceIsParent && replies[0].sourceIsChild && replies[0].origin === 'http://example.test'"
        )
        .unwrap()
        .to_boolean(), "{replies:?}");
}

#[test]
fn opaque_iframe_can_reply_without_exposing_parent_document() {
    let document = TreeBuilder::parse(
        r#"<html><body><iframe id="child" sandbox="allow-scripts" srcdoc="<script>
            addEventListener('message', event => {
                parent.postMessage({ data: event.data, sourceIsParent: event.source === parent,
                    parentDocumentHidden: parent.document === undefined }, '*');
            });
        </script>"></iframe></body></html>"#,
    )
    .document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "http://example.test/page").unwrap();
    runtime
        .eval(
            r#"
            globalThis.replies = [];
            const child = document.getElementById('child').contentWindow;
            addEventListener('message', event => replies.push({
                data: event.data.data,
                sourceIsParent: event.data.sourceIsParent,
                parentDocumentHidden: event.data.parentDocumentHidden,
                sourceIsChild: event.source === child,
                origin: event.origin,
            }));
            child.postMessage('opaque', '*');
        "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    let replies = runtime.eval("JSON.stringify(replies)").unwrap();
    assert!(runtime
        .eval(
            "replies.length === 1 && replies[0].data === 'opaque' && replies[0].sourceIsParent && replies[0].parentDocumentHidden && replies[0].sourceIsChild && replies[0].origin === 'null'"
        )
        .unwrap()
        .to_boolean(), "{replies:?}");
}

#[test]
fn opaque_iframe_default_target_origin_reaches_itself() {
    let document = TreeBuilder::parse(
        r#"<html><body><iframe sandbox="allow-scripts" srcdoc="<script>
            addEventListener('message', event => {
                parent.postMessage({ data: event.data, origin: event.origin }, '*');
            });
            window.postMessage('self');
        </script>"></iframe></body></html>"#,
    )
    .document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "http://example.test/page").unwrap();
    runtime
        .eval("globalThis.replies = []; addEventListener('message', event => replies.push(event.data));")
        .unwrap();
    runtime.run_until_idle().unwrap();
    let replies = runtime.eval("JSON.stringify(replies)").unwrap();
    assert!(
        runtime
            .eval(
                "replies.length === 1 && replies[0].data === 'self' && replies[0].origin === 'null'"
            )
            .unwrap()
            .to_boolean(),
        "replies: {replies:?}; errors: {:?}",
        runtime.take_task_errors()
    );
}

#[test]
fn window_message_transfers_array_buffer_and_rejects_uncloneable_data() {
    let document = TreeBuilder::parse("<html><body></body></html>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "http://example.test/page").unwrap();
    assert!(runtime
        .eval(
            r#"
            globalThis.received = [];
            addEventListener('message', event => received.push(Array.from(new Uint8Array(event.data.buffer))));
            const buffer = new ArrayBuffer(2);
            new Uint8Array(buffer).set([3, 7]);
            window.postMessage({ buffer }, { targetOrigin: '/', transfer: [buffer] });
            let cloneError = false;
            try { window.postMessage(() => {}, '*'); }
            catch (error) { cloneError = error.name === 'DataCloneError'; }
            cloneError && buffer.byteLength === 0 && received.length === 0;
        "#,
        )
        .unwrap()
        .to_boolean());
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .eval("received.length === 1 && received[0].join(',') === '3,7'")
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn window_message_transfers_a_message_port() {
    let document = TreeBuilder::parse("<html><body></body></html>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "http://example.test/page").unwrap();
    runtime
        .eval(
            r#"
            globalThis.received = [];
            const channel = new MessageChannel();
            channel.port1.onmessage = event => received.push(event.data);
            window.addEventListener('message', event => {
                received.push(event.data.port === event.ports[0] ? 'identity' : 'wrong-port');
                event.ports[0].postMessage('reply');
            });
            window.postMessage({ port: channel.port2 }, '*', [channel.port2]);
        "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .eval("received.join(',') === 'identity,reply'")
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn failed_window_message_clone_keeps_message_port_usable() {
    let document = TreeBuilder::parse("<html><body></body></html>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "http://example.test/page").unwrap();
    assert!(runtime
        .eval(
            r#"
            globalThis.received = [];
            const channel = new MessageChannel();
            channel.port1.onmessage = event => received.push(event.data);
            let cloneError = false;
            try { window.postMessage({ port: channel.port2, bad: () => {} }, '*', [channel.port2]); }
            catch (error) { cloneError = error.name === 'DataCloneError'; }
            channel.port2.postMessage('still-live');
            cloneError;
        "#,
        )
        .unwrap()
        .to_boolean());
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .eval("received.length === 1 && received[0] === 'still-live'")
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn iframe_receives_a_transferred_message_port_in_its_realm() {
    let document = TreeBuilder::parse(
        r#"<html><body><iframe id="child" srcdoc="<script>
            addEventListener('message', event => {
                if (event.data.port === event.ports[0]) event.ports[0].postMessage({ value: 'child-reply' });
            });
        </script>"></iframe></body></html>"#,
    )
    .document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "http://example.test/page").unwrap();
    runtime
        .eval(
            r#"
            globalThis.received = [];
            const channel = new MessageChannel();
            channel.port1.onmessage = event => received.push(
                event.data.value === 'child-reply' &&
                Object.getPrototypeOf(event.data) === Object.prototype);
            document.getElementById('child').contentWindow.postMessage(
                { port: channel.port2 }, '*', [channel.port2]);
        "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .eval("received.length === 1 && received[0] === true")
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn detached_iframe_does_not_receive_a_queued_message() {
    let document =
        TreeBuilder::parse("<html><body><iframe id='child'></iframe></body></html>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "http://example.test/page").unwrap();
    runtime
        .eval(
            r#"
            globalThis.received = [];
            const iframe = document.getElementById('child');
            const child = iframe.contentWindow;
            child.addEventListener('message', () => received.push('stale'));
            child.postMessage('before detach', '*');
            iframe.remove();
        "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .eval("received.length === 0 && child.closed")
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn navigated_iframe_does_not_receive_its_previous_documents_message() {
    let document =
        TreeBuilder::parse("<html><body><iframe id='child'></iframe></body></html>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "http://example.test/page").unwrap();
    runtime
        .eval(
            r#"
            globalThis.received = [];
            const iframe = document.getElementById('child');
            const child = iframe.contentWindow;
            child.addEventListener('message', () => received.push('stale'));
            child.postMessage('before navigation', '*');
            // Location.replace commits the new Document before the queued task.
            child.location.replace('data:text/html,replacement');
        "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .eval("received.length === 0 && iframe.contentWindow === child")
            .unwrap()
            .to_boolean()
    );
}
