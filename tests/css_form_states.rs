//! HTML form-state pseudo-classes across selector APIs and style invalidation.

use omoikane::{html::TreeBuilder, js::JsRuntime};

#[test]
fn required_optional_follow_input_applicability_and_boolean_attributes() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
        const applicable = ['text', 'search', 'tel', 'url', 'email', 'password',
            'date', 'month', 'week', 'time', 'datetime-local', 'number',
            'checkbox', 'radio', 'file', 'not-a-type', ''];
        for (const type of applicable) {
            const input = document.createElement('input'); input.type = type;
            if (!input.matches(':optional') || input.matches(':required')) return false;
            input.setAttribute('required', 'false');
            input.disabled = true; input.readOnly = true;
            if (!input.matches(':required') || input.matches(':optional')) return false;
            input.required = false;
            if (!input.matches(':optional') || input.matches(':required')) return false;
        }
        for (const type of ['hidden', 'range', 'color', 'button', 'image', 'reset', 'submit']) {
            const input = document.createElement('input'); input.type = type;
            input.required = true;
            if (input.matches(':required, :optional')) return false;
        }
        for (const tag of ['select', 'textarea']) {
            const control = document.createElement(tag);
            if (!control.matches(':optional')) return false;
            control.required = true; control.disabled = true;
            if (!control.matches(':required') || control.matches(':optional')) return false;
        }
        for (const tag of ['button', 'form', 'fieldset', 'option', 'object', 'div', 'x-widget']) {
            const control = document.createElement(tag); control.setAttribute('required', '');
            if (control.matches(':required, :optional')) return false;
        }
        return CSS.supports('selector(:required)') && CSS.supports('selector(:optional)');
    })()"#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn required_optional_use_html_local_names_and_null_namespace_attributes() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
        for (const namespace of [null, 'http://www.w3.org/2000/svg']) {
            const node = document.createElementNS(namespace, 'input');
            node.setAttribute('required', '');
            if (node.matches(':required, :optional')) return false;
        }
        const html = 'http://www.w3.org/1999/xhtml';
        const doc = document.implementation.createDocument(html, 'html', null);
        const input = doc.createElementNS(html, 'h:input');
        input.setAttributeNS('urn:other', 'required', '');
        if (!input.matches(':optional') || input.matches(':required')) return false;
        input.setAttributeNS(null, 'required', '');
        if (!input.matches(':required') || input.matches(':optional')) return false;
        input.setAttributeNS('urn:other', 'type', 'hidden');
        if (!input.matches(':required')) return false;
        input.setAttributeNS(null, 'type', 'HIDDEN');
        return !input.matches(':required, :optional');
    })()"#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn required_changes_update_sibling_filters_and_relational_styles() {
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse(
        "<style>:required { color: red } :optional { color: green } main:has(:required) { background-color: blue } :nth-child(2 of :required) { background-color: yellow }</style><main id=scope><input id=a><input id=b><textarea id=c></textarea></main>"
    ).document()).unwrap();
    assert_eq!(runtime.eval(r#"(() => {
        const scope = document.getElementById('scope');
        const a = document.getElementById('a'), b = document.getElementById('b');
        const c = document.getElementById('c');
        const color = n => getComputedStyle(n).color;
        const bg = n => getComputedStyle(n).backgroundColor;
        const initialBackground = bg(c);
        if (color(b) !== 'rgb(0, 128, 0)' || bg(scope) !== 'rgba(0, 0, 0, 0)') return false;
        b.required = true; c.required = true;
        if (color(b) !== 'rgb(255, 0, 0)' || bg(scope) !== 'rgb(0, 0, 255)' || bg(c) !== 'rgb(255, 255, 0)') return false;
        b.type = 'hidden';
        if (bg(c) !== initialBackground || b.matches(':required, :optional')) return false;
        c.required = false;
        return color(c) === 'rgb(0, 128, 0)' && bg(scope) === 'rgba(0, 0, 0, 0)' &&
            scope.querySelectorAll(':required').length === 0 && a.matches(':optional');
    })()"#).unwrap().as_boolean(), Some(true));
}

#[test]
fn required_optional_reject_function_arguments() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(runtime.eval(r#"(() => {
        if (!CSS.supports('selector(:REQUIRED)') || !CSS.supports('selector(:OPTIONAL)')) return false;
        for (const selector of [':required()', ':optional(value)']) {
            if (CSS.supports(`selector(${selector})`)) return false;
            const sheet = new CSSStyleSheet();
            sheet.replaceSync(`${selector} { color: red }`);
            if (sheet.cssRules.length !== 0) return false;
            try { document.querySelectorAll(selector); return false; }
            catch (error) { if (error.name !== 'SyntaxError') return false; }
        }
        return true;
    })()"#).unwrap().as_boolean(), Some(true));
}
