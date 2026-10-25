//! Definition selectors follow custom element construction and upgrade state.
use omoikane::{html::TreeBuilder, js::JsRuntime};

fn evaluate(html: &str, script: &str) -> String {
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse(html).document()).unwrap();
    let program = format!(
        "(() => {{ try {{ {script} }} catch (error) {{ return error.name + ': ' + error.message; }} }})()"
    );
    runtime
        .eval(&program)
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped()
}

#[test]
fn defined_distinguishes_ordinary_foreign_and_unregistered_elements() {
    assert_eq!(
        evaluate(
            "<body><div id=ordinary></div><x-pending id=pending></x-pending></body>",
            r#"
        if (!document.getElementById('ordinary').matches(':defined')) return 'ordinary';
        if (document.getElementById('pending').matches(':defined')) return 'pending';
        for (const name of ['unknown', 'annotation-xml', 'font-face']) {
            if (!document.createElement(name).matches(':defined')) return name;
        }
        for (const ns of [null, 'http://www.w3.org/2000/svg']) {
            if (!document.createElementNS(ns, 'x-foreign').matches(':defined')) return 'foreign';
        }
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn defined_tracks_successful_and_failed_upgrades_and_invalidates_style() {
    assert_eq!(
        evaluate(
            "<style>x-good:not(:defined){color:red} x-good:defined{color:green}</style><body><x-good></x-good><x-bad></x-bad></body>",
            r#"
        const good = document.querySelector('x-good'), bad = document.querySelector('x-bad');
        if (good.matches(':defined') || bad.matches(':defined')) return 'before';
        if (getComputedStyle(good).color !== 'rgb(255, 0, 0)') return 'before-style';
        customElements.define('x-good', class extends HTMLElement {});
        if (!good.matches(':defined')) return 'upgraded';
        if (getComputedStyle(good).color !== 'rgb(0, 128, 0)') return 'after-style';
        customElements.define('x-bad', class extends HTMLElement { constructor(){ super(); throw Error('failed'); } });
        if (bad.matches(':defined')) return 'failed';
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn defined_grammar_agrees_with_supports_and_cssom() {
    assert_eq!(
        evaluate(
            "<body></body>",
            r#"
        if (!CSS.supports('selector(:defined)')) return 'supports';
        const sheet = new CSSStyleSheet(); sheet.replaceSync(':defined{color:red}');
        if (sheet.cssRules.length !== 1) return 'cssom';
        for (const selector of [':defined()', ':defined(x)']) {
            if (CSS.supports(`selector(${selector})`)) return 'functional-support';
            try { document.querySelector(selector); return 'functional-query'; }
            catch (e) { if (e.name !== 'SyntaxError') return 'wrong-error'; }
        }
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn defined_stays_false_during_upgrade_and_true_in_direct_construction() {
    assert_eq!(
        evaluate(
            "<body><x-upgrade></x-upgrade></body>",
            r#"
        const existing = document.querySelector('x-upgrade');
        let upgrading;
        class Upgrade extends HTMLElement {
            constructor() { super(); upgrading = this.matches(':defined'); }
        }
        customElements.define('x-upgrade', Upgrade);
        if (upgrading !== false || !existing.matches(':defined')) return 'upgrade';
        let direct;
        class Direct extends HTMLElement {
            constructor() { super(); direct = this.matches(':defined'); }
        }
        customElements.define('x-direct', Direct);
        const instance = new Direct();
        return direct === true && instance.matches(':defined') ? 'ok' : 'direct';
    "#
        ),
        "ok"
    );
}

#[test]
fn customized_candidates_keep_creation_is_value_separate_from_attributes() {
    assert_eq!(
        evaluate(
            "<body><p id=parsed is=x-para></p><p id=empty is=''></p></body>",
            r#"
        for (const id of ['parsed', 'empty']) {
            const node = document.getElementById(id);
            if (node.matches(':defined')) return 'parsed-'+id;
            node.removeAttribute('is');
            if (node.matches(':defined')) return 'removed-'+id;
        }
        const normal = document.createElement('p');normal.setAttribute('is', 'x-para');
        if (!normal.matches(':defined')) return 'attribute-only';
        for (const is of ['', 'x-para']) {
            const node = document.createElement('p', {is});
            if (node.matches(':defined')) return 'created';
            node.setAttribute('is', 'different');
            if (node.matches(':defined') || node.cloneNode().matches(':defined')) return 'clone';
        }
        const foreign = document.createElementNS('http://www.w3.org/2000/svg', 'p', {is:'x-para'});
        return foreign.matches(':defined') ? 'ok' : 'foreign';
    "#
        ),
        "ok"
    );
}

#[test]
fn customized_builtins_upgrade_by_creation_identity_and_preserve_their_interface() {
    assert_eq!(
        evaluate(
            "<body><p is=x-paragraph id=parsed></p><p id=ordinary></p></body>",
            r#"
        const parsed = document.getElementById('parsed');
        parsed.removeAttribute('is');
        let during;
        class Paragraph extends HTMLParagraphElement {
            constructor() { super(); during = this.matches(':defined'); }
        }
        customElements.define('x-paragraph', Paragraph, {extends:'p'});
        if (during !== false || !(parsed instanceof Paragraph) || !parsed.matches(':defined')) return 'upgrade';
        const created = document.createElement('p', {is:'x-paragraph'});
        if (!(created instanceof Paragraph) || !created.matches(':defined')) return 'created';
        const direct = new Paragraph();
        if (direct.localName !== 'p' || !(direct instanceof HTMLParagraphElement) || !direct.matches(':defined')) return 'direct';
        const ordinary = document.getElementById('ordinary');ordinary.setAttribute('is','x-paragraph');
        if (ordinary instanceof Paragraph || !ordinary.matches(':defined')) return 'attribute';
        try { parsed.attachInternals(); return 'internals'; } catch (e) { if (e.name !== 'NotSupportedError') return 'error'; }
        for (const name of ['unknown', 'x-unknown']) {
            try { customElements.define('x-bad-'+name, class extends HTMLElement {}, {extends:name}); return 'bad-extends'; }
            catch (e) { if (e.name !== 'NotSupportedError') return 'bad-error'; }
        }
        return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn cloning_and_importing_construct_in_the_destination_registry() {
    assert_eq!(
        evaluate(
            "<body><x-copy></x-copy><p is=x-copy-p></p></body>",
            r#"
        class Copy extends HTMLElement {}
        class CopyP extends HTMLParagraphElement {}
        customElements.define('x-copy', Copy);
        customElements.define('x-copy-p', CopyP, {extends:'p'});
        const original = document.querySelector('x-copy');
        const p = document.querySelector('p');p.removeAttribute('is');
        if (!(original.cloneNode() instanceof Copy)) return 'clone';
        if (!(p.cloneNode() instanceof CopyP)) return 'builtin-clone';
        const inert = document.implementation.createHTMLDocument();
        const imported = inert.importNode(original);
        if (imported.matches(':defined') || imported instanceof Copy) return 'inert-import';
        const active = document.importNode(imported);
        return active instanceof Copy && active.matches(':defined') ? 'ok' : 'active-import';
    "#
        ),
        "ok"
    );
}

#[test]
fn native_definition_state_uses_full_custom_name_syntax_and_no_author_getters() {
    assert_eq!(
        evaluate(
            "<body></body>",
            r#"
        const html = 'http://www.w3.org/1999/xhtml';
        for (const name of ['x-·', 'x-𐀀', 'x-\u200c', 'x-\u037e', 'x-\u{f0000}', 'x-!']) {
            const container = document.createElement('div');
            container.innerHTML = '<'+name+'></'+name+'>';
            const node = container.firstElementChild;
            if (!node || node.localName !== name || node.matches(':defined')) return 'valid:'+name;
            customElements.define(name, class extends HTMLElement {});
            customElements.upgrade(node);
            if (!node.matches(':defined')) return 'registered:'+name;
        }
        for (const name of ['x-A', 'font-face']) {
            const node = document.createElementNS(html, name);
            if (!node.matches(':defined')) return 'invalid:'+name;
        }
        const node = document.createElement('x-private');document.body.append(node);
        customElements.define('x-private', class extends HTMLElement {});
        for (const key of ['localName', 'namespaceURI', '__customElementState']) {
            Object.defineProperty(node, key, {get(){throw Error('author getter');}});
        }
        return node.matches(':defined') ? 'ok' : 'private';
    "#
        ),
        "ok"
    );
}

#[test]
fn custom_constructors_must_call_the_registered_html_interface() {
    assert_eq!(
        evaluate(
            "<body><p is=x-wrong-base></p><x-wrong-auto></x-wrong-auto></body>",
            r#"
        for (const C of [HTMLElement, HTMLParagraphElement, HTMLMediaElement, HTMLVideoElement]) {
            if (C.length !== 0) return 'constructor-arity';
        }
        class WrongBase extends HTMLElement {}
        class WrongAutonomous extends HTMLParagraphElement {}
        customElements.define('x-wrong-base', WrongBase, {extends:'p'});
        customElements.define('x-wrong-auto', WrongAutonomous);
        if (document.querySelector('p').matches(':defined')) return 'builtin-upgrade';
        if (document.querySelector('x-wrong-auto').matches(':defined')) return 'autonomous-upgrade';
        for (const C of [WrongBase, WrongAutonomous]) {
            try { new C(); return 'direct'; } catch (e) { if (e.name !== 'TypeError') return 'wrong-error'; }
        }
        class Section extends HTMLTableSectionElement {}
        customElements.define('x-section', Section, {extends:'tbody'});
        const section = new Section();
        if (section.localName !== 'tbody' || !section.matches(':defined')) return 'shared-interface';
        const reflected = Reflect.construct(HTMLParagraphElement, [], WrongBase);
        return reflected.localName === 'p' && reflected.matches(':defined') ? 'ok' : 'reflect';
    "#
        ),
        "ok"
    );
}

#[test]
fn definition_matching_needs_no_javascript_runtime() {
    let doc = TreeBuilder::parse("<div id=ordinary></div><x-pending id=pending></x-pending><p id=builtin is=x-p></p><font-face id=reserved></font-face>").document();
    let selector = omoikane::css::parse_selector_list(":defined").unwrap();
    for (id, expected) in [
        ("ordinary", true),
        ("pending", false),
        ("builtin", false),
        ("reserved", true),
    ] {
        let node = doc.query_selector(&format!("#{id}")).unwrap();
        assert_eq!(
            omoikane::css::matches_selector(&node, &selector[0]),
            expected,
            "{id}"
        );
    }
    let foreign = omoikane::dom::NodeHandle::xml_element(
        "x-foreign",
        Some("http://www.w3.org/2000/svg".into()),
    );
    assert!(omoikane::css::matches_selector(&foreign, &selector[0]));
}

#[test]
fn upgrade_invalidates_relational_and_filtered_sibling_styles() {
    assert_eq!(
        evaluate(
            "<style>#host{color:red} #host:has(x-late:defined){color:green} #after{color:red} x-late:defined + #after{color:green}</style><body><div id=host><x-late></x-late><p id=after></p></div></body>",
            r#"
        const host = document.getElementById('host'), after = document.getElementById('after');
        if (getComputedStyle(host).color !== 'rgb(255, 0, 0)' || getComputedStyle(after).color !== 'rgb(255, 0, 0)') return 'before';
        if (host.querySelector(':nth-child(1 of :defined)') !== after) return 'before-filter';
        customElements.define('x-late', class extends HTMLElement {});
        if (getComputedStyle(host).color !== 'rgb(0, 128, 0)' || getComputedStyle(after).color !== 'rgb(0, 128, 0)') return 'after';
        return host.querySelector(':nth-child(1 of :defined)').localName === 'x-late' ? 'ok' : 'after-filter';
    "#
        ),
        "ok"
    );
}

#[test]
fn creation_options_follow_the_legacy_string_and_dictionary_conversions() {
    assert_eq!(
        evaluate(
            "<body></body>",
            r#"
        for (const options of [undefined, null, 'ignored', 42, false, 1n]) {
            if (!document.createElement('p', options).matches(':defined')) return 'create';
            if (!document.createElementNS('http://www.w3.org/1999/xhtml', 'p', options).matches(':defined')) return 'createNS';
        }
        let reads = 0;
        const options = {get is(){reads++;return {toString(){return 'x-converted';}}}};
        if (document.createElement('p', options).matches(':defined') || reads !== 1) return 'dictionary';
        try { customElements.define('x-null-extends', class extends HTMLElement {}, {extends:null}); return 'null-extends'; }
        catch (e) { if (e.name !== 'NotSupportedError') return 'extends-error'; }
        return 'ok';
    "#
        ),
        "ok"
    );
}
