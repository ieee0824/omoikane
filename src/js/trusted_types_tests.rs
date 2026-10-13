use super::*;
use crate::html::TreeBuilder;

fn runtime(markup: &str) -> JsRuntime {
    JsRuntime::with_document_and_url(
        TreeBuilder::parse(markup).document(),
        "https://example.test/page",
    )
    .unwrap()
}

fn assert_js_true(runtime: &mut JsRuntime, source: &str) {
    assert_eq!(runtime.eval(source).unwrap().as_boolean(), Some(true));
}

#[test]
fn trusted_values_have_unforgeable_brands_and_policy_callbacks() {
    let mut runtime = runtime("<body>");
    assert_js_true(
        &mut runtime,
        r#"
        const policy = trustedTypes.createPolicy('app', {
          createHTML: (s, suffix) => s + suffix,
          createScript: s => s,
          createScriptURL: s => s,
        });
        const html = policy.createHTML('<b>', '</b>');
        const script = policy.createScript('1 + 1');
        const url = policy.createScriptURL('/app.js');
        const forged = Object.create(TrustedHTML.prototype);
        forged.toString = () => '<script>forged</script>';
        const checks = [
          policy.name === 'app', String(html) === '<b></b>', html.toJSON() === '<b></b>',
          trustedTypes.isHTML(html), trustedTypes.isScript(script), trustedTypes.isScriptURL(url),
          !trustedTypes.isHTML(forged), !trustedTypes.isHTML({toString: () => '<b>fake</b>'}),
          !trustedTypes.isHTML(Object.create(html)),
          trustedTypes.isHTML(trustedTypes.emptyHTML),
          trustedTypes.isScript(trustedTypes.emptyScript),
          (() => { try { new TrustedHTML(); return false; } catch(e) { return e instanceof TypeError; } })(),
          (() => { try { new TrustedTypePolicyFactory(); return false; } catch(e) { return e instanceof TypeError; } })(),
          (() => { try { policy.createHTML.call(forged, 'x'); return false; } catch(e) { return e instanceof TypeError; } })(),
          (() => { try { trustedTypes.isHTML.call({}, html); return false; } catch(e) { return e instanceof TypeError; } })(),
          (() => { try { trustedTypes.createPolicy(Symbol(), {}); return false; } catch(e) { return e instanceof TypeError; } })(),
          (() => { try { policy.createHTML(Symbol()); return false; } catch(e) { return e instanceof TypeError; } })(),
          (() => { try { policy.createHTML(); return false; } catch(e) { return e instanceof TypeError; } })(),
          (() => { try { policy.createScript(); return false; } catch(e) { return e instanceof TypeError; } })(),
          (() => { try { policy.createScriptURL(); return false; } catch(e) { return e instanceof TypeError; } })(),
          trustedTypes.createPolicy('app', {}).name === 'app',
        ];
        checks.every(Boolean)
        "#,
    );
}

#[test]
fn enforced_inner_html_rejects_before_dom_changes_and_reports() {
    let mut runtime = runtime("<body><div id=target><i id=original>old</i></div>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.violations = [];
        document.addEventListener('securitypolicyviolation', e => violations.push(e));
        const target = document.getElementById('target');
        const original = target.firstChild;
        let rejected = false;
        try { target.innerHTML = '<b id=blocked>bad</b>'; }
        catch (e) { rejected = e instanceof TypeError; }
        const preserved = target.firstChild === original && original.id === 'original' &&
          document.getElementById('blocked') === null;
        const policy = trustedTypes.createPolicy('app', {createHTML: s => s});
        target.innerHTML = policy.createHTML('<b id=allowed>safe</b>');
        rejected && preserved &&
          target.firstChild.id === 'allowed'
        "#,
    );
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "violations.length === 1 && violations[0].effectiveDirective === 'require-trusted-types-for' && violations[0].blockedURI === 'trusted-types-sink' && violations[0].sample === 'Element innerHTML|<b id=blocked>bad</b>' && violations[0].disposition === 'enforce'",
    );
}

#[test]
fn report_only_allows_strings_but_emits_a_sink_violation() {
    let mut runtime = runtime("<body><div id=target></div>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy-Report-Only".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.violations = [];
        document.addEventListener('securitypolicyviolation', e => violations.push(e));
        document.getElementById('target').innerHTML = '<b id=inserted>allowed</b>';
        document.getElementById('inserted').textContent === 'allowed'
        "#,
    );
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "violations.length === 1 && violations[0].disposition === 'report' && violations[0].blockedURI === 'trusted-types-sink'",
    );
}

#[test]
fn report_only_trusted_types_directive_reports_disallowed_policy_names() {
    let mut runtime = runtime("<body>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy-Report-Only".into(),
        "trusted-types approved".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.violations = [];
        document.addEventListener('securitypolicyviolation', e => violations.push(e));
        const policy = trustedTypes.createPolicy('other', {createHTML: s => s});
        policy.createHTML('<b>allowed by report-only</b>').toString() ===
          '<b>allowed by report-only</b>'
        "#,
    );
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "violations.length === 1 && violations[0].disposition === 'report' && violations[0].blockedURI === 'trusted-types-policy' && violations[0].sample === 'other'",
    );
}

#[test]
fn default_policy_converts_plain_html_and_rejected_names_report() {
    let mut runtime = runtime("<body><div id=target></div>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "trusted-types default app; require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.violations = [];
        document.addEventListener('securitypolicyviolation', e => violations.push(e));
        let args;
        const fallback = trustedTypes.createPolicy('default', {
          createHTML: (input, type, sink) => {
            args = [input, type, sink];
            return input.replace('unsafe', 'safe');
          }
        });
        const target = document.getElementById('target');
        target.innerHTML = '<b>unsafe</b>';
        let duplicate = false, denied = false;
        try { trustedTypes.createPolicy('default', {}); }
        catch (e) { duplicate = e instanceof TypeError; }
        try { trustedTypes.createPolicy('other', {}); }
        catch (e) { denied = e instanceof TypeError; }
        fallback === trustedTypes.defaultPolicy && target.innerHTML === '<b>safe</b>' &&
          args.join('|') === '<b>unsafe</b>|TrustedHTML|Element innerHTML' &&
          duplicate && denied
        "#,
    );
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "violations.length === 2 && violations.every(e => e.effectiveDirective === 'trusted-types' && e.blockedURI === 'trusted-types-policy') && violations[1].sample === 'other'",
    );
}

#[test]
fn rejected_default_policy_preserves_dom_before_throwing() {
    let mut runtime = runtime("<body><div id=target><i id=original>old</i></div>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.violations = [];
        document.addEventListener('securitypolicyviolation', e => violations.push(e));
        trustedTypes.createPolicy('default', {createHTML: () => null});
        const target = document.getElementById('target');
        const old = target.firstChild;
        let rejected = false;
        try { target.innerHTML = '<b id=blocked>blocked</b>'; }
        catch (e) { rejected = e instanceof TypeError; }
        rejected && target.firstChild === old && document.getElementById('blocked') === null
        "#,
    );
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "violations.length === 1 && violations[0].blockedURI === 'trusted-types-sink'",
    );
}

#[test]
fn sink_uses_owning_document_policy_and_accepts_cross_realm_trusted_html() {
    let mut runtime = runtime("<body><iframe id=frame></iframe>");
    assert_js_true(
        &mut runtime,
        r#"
        const frame = document.getElementById('frame');
        frame.srcdoc = `<head><meta http-equiv="Content-Security-Policy" content="require-trusted-types-for 'script'"></head><body><div id="target"><i id="original">old</i></div>`;
        const child = frame.contentWindow;
        const target = frame.contentDocument.getElementById('target');
        const original = target.firstChild;
        const parentSetter = Object.getOwnPropertyDescriptor(Element.prototype, 'innerHTML').set;
        let rejected = false;
        try { parentSetter.call(target, '<b id="blocked">bad</b>'); }
        catch (e) { rejected = e instanceof TypeError; }
        const preserved = target.firstChild === original && original.id === 'original';
        const trusted = trustedTypes.createPolicy('parent', {createHTML: s => s}).createHTML('<b id="safe">safe</b>');
        const crossRealmBrand = child.trustedTypes.isHTML(trusted);
        parentSetter.call(target, trusted);
        rejected && preserved && crossRealmBrand &&
          target.firstChild.id === 'safe' && !frame.contentDocument.getElementById('blocked')
        "#,
    );
}

#[test]
fn dom_parser_gates_html_in_the_calling_document_before_creating_a_document() {
    let mut runtime = runtime("<body>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.violations = [];
        document.addEventListener('securitypolicyviolation', e => violations.push(e));
        const parser = new DOMParser();
        const implementationGetter = Object.getOwnPropertyDescriptor(Document.prototype, 'implementation').get;
        let documentCreations = 0;
        Object.defineProperty(document, 'implementation', { configurable: true, get() {
          documentCreations++;
          return implementationGetter.call(this);
        }});
        let rejected = false;
        try { parser.parseFromString('<p id="blocked">bad</p>', 'text/html'); }
        catch (e) { rejected = e instanceof TypeError; }
        const rejectedBeforeCreation = rejected && documentCreations === 0;
        const policy = trustedTypes.createPolicy('parser', {createHTML: s => s});
        const trustedDocument = parser.parseFromString(policy.createHTML('<b id="trusted">safe</b>'), 'text/html');
        const xmlDocument = parser.parseFromString('<root/>', 'text/xml');
        let callbackArguments;
        trustedTypes.createPolicy('default', {createHTML: (input, type, sink) => {
          callbackArguments = [input, type, sink];
          return input.replace('raw', 'safe');
        }});
        const convertedDocument = parser.parseFromString('<i>raw</i>', 'text/html');
        delete document.implementation;
        rejectedBeforeCreation && documentCreations === 2 &&
          trustedDocument.body.firstChild.id === 'trusted' &&
          xmlDocument.documentElement !== null &&
          convertedDocument.body.innerHTML === '<i>safe</i>' &&
          callbackArguments.join('|') === '<i>raw</i>|TrustedHTML|DOMParser parseFromString'
        "#,
    );
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "violations.length === 1 && violations[0].sample === 'DOMParser parseFromString|<p id=\"blocked\">bad</p>' && violations[0].blockedURI === 'trusted-types-sink'",
    );
}

#[test]
fn dom_parser_report_only_allows_html_and_reports() {
    let mut runtime = runtime("<body>");
    runtime.install_response_security_headers(&[(
        "Content-Security-Policy-Report-Only".into(),
        "require-trusted-types-for 'script'".into(),
    )]);
    assert_js_true(
        &mut runtime,
        r#"
        globalThis.violations = [];
        document.addEventListener('securitypolicyviolation', e => violations.push(e));
        new DOMParser().parseFromString('<p id="allowed">plain</p>', 'text/html')
          .body.firstChild.id === 'allowed'
        "#,
    );
    runtime.run_until_idle().unwrap();
    assert_js_true(
        &mut runtime,
        "violations.length === 1 && violations[0].disposition === 'report' && violations[0].sample === 'DOMParser parseFromString|<p id=\"allowed\">plain</p>'",
    );
}
