//! Unsupported declarations must not manufacture computed CSS values.
use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;

#[test]
fn property_rule_cssom_preserves_escaped_literal_identifiers() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(runtime.eval(r#"(() => {
        const sheet = new CSSStyleSheet();
        sheet.replaceSync(String.raw`@property --literal {
            syntax: "I\\ dent|none"; inherits: false; initial-value: I\ dent;
        }
        @property --pipe {
            syntax: "a\\|b|none"; inherits: false; initial-value: a\|b;
        }`);
        const literal = sheet.cssRules[0];
        const pipe = sheet.cssRules[1];
        if (!literal || !pipe || literal.syntax !== String.raw`I\ dent|none` ||
            literal.initialValue !== String.raw`I\ dent` ||
            literal.cssText !== String.raw`@property --literal { syntax: "I\\ dent|none"; inherits: false; initial-value: I\ dent; }` ||
            pipe.initialValue !== String.raw`a\|b`) return false;
        CSS.registerProperty({name: '--escaped-api', syntax: String.raw`a\+`,
            inherits: false, initialValue: String.raw`a\+`});
        try {
            CSS.registerProperty({name: '--reserved-api', syntax: String.raw`IN\49TIAL`,
                inherits: false, initialValue: 'initial'});
            return false;
        } catch (error) { return error.name === 'SyntaxError'; }
    })()"#).unwrap().as_boolean(), Some(true));
}

#[test]
fn retained_computed_overscroll_shorthand_coalesces_and_tracks_axis_changes() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
        const element = document.body.appendChild(document.createElement('div'));
        const style = getComputedStyle(element);
        if (!('overscroll-behavior' in style) || style.overscrollBehavior !== 'auto') return false;
        element.style.overscrollBehaviorX = 'contain';
        if (style.overscrollBehavior !== 'contain auto') return false;
        element.style.overscrollBehaviorY = 'contain';
        if (style.getPropertyValue('overscroll-behavior') !== 'contain') return false;
        element.style.cssText = 'overscroll-behavior: none auto';
        return style.overscrollBehavior === 'none auto' &&
            style.overscrollBehaviorX === 'none' && style.overscrollBehaviorY === 'auto';
    })()"#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn toggle_source_retargets_for_each_listener_and_after_shadow_dispatch() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
        const host = document.body.appendChild(document.createElement('div'));
        const root = host.attachShadow({mode: 'closed'});
        const source = document.createElement('button');
        const target = document.createElement('div');
        root.append(source, target);
        const event = new ToggleEvent('toggle', {source, bubbles: true, composed: true});
        const observed = [];
        root.addEventListener('toggle', e => observed.push(e.source === source));
        host.addEventListener('toggle', e => observed.push(e.source === host));
        if (event.source !== host) return false;
        target.dispatchEvent(event);
        return observed.length === 2 && observed.every(Boolean) &&
            event.source === host && new ToggleEvent('toggle').source === null;
    })()"#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn property_support_agrees_with_css_supports_and_cascade() {
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse(
        "<style>#probe { rotate:10deg; font-feature-settings: \"liga\" 0; scrollbar-color:red blue; }</style><div id=probe></div>"
    ).document()).unwrap();
    assert_eq!(runtime.eval(r#"(() => {
        const element = document.getElementById('probe');
        const declarations = [
            ['rotate', '10deg'], ['font-feature-settings', '"liga" 0'],
            ['font-variation-settings', '"wght" 500'], ['counter-set', 'a 1'],
            ['scrollbar-color', 'red blue'], ['hyphenate-character', '"-"'],
            ['anchor-name', '--a'], ['unrecognized-property', 'inherit'],
        ];
        return declarations.every(([name, value]) => {
            element.style.setProperty(name, value);
            if (name === 'rotate' && CSS.supports(name, value)) {
                return element.style.getPropertyValue(name) === '10deg' &&
                    getComputedStyle(element).getPropertyValue(name) === '10deg';
            }
            if (name === 'font-variation-settings' || name === 'counter-set') {
                return CSS.supports(name, value) &&
                    element.style.getPropertyValue(name) === value &&
                    getComputedStyle(element).getPropertyValue(name) === value;
            }
            return !CSS.supports(name, value) && element.style.getPropertyValue(name) === '' && getComputedStyle(element).getPropertyValue(name) === '';
        });
    })()"#).unwrap().as_boolean(), Some(true));
}

#[test]
fn invalid_cssom_writes_preserve_valid_declarations_and_custom_values() {
    let mut runtime =
        JsRuntime::with_document(TreeBuilder::parse("<div id=probe></div>").document()).unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
        const style = document.getElementById('probe').style;
        style.cssText = 'width:20px; unrecognized-property:inherit; --CaseSensitive:value';
        style.setProperty('width', 'nonsense');
        return style.width === '20px' && style.getPropertyValue('unrecognized-property') === '' &&
            style.getPropertyValue('--CaseSensitive') === 'value' && style.length === 2;
    })()"#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn cssom_rule_declarations_reject_unknown_properties_but_preserve_descriptors() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(runtime.eval(r#"(() => {
        const sheet = new CSSStyleSheet();
        sheet.replaceSync('div { color:red; unrecognized-property:inherit; } @font-face { font-family:Probe; src:url(probe.ttf); }');
        const rule = sheet.cssRules[0].style;
        rule.setProperty('font-feature-settings', '"liga" 0');
        rule.setProperty('color', 'nonsense');
        const face = sheet.cssRules[1].style;
        return rule.getPropertyValue('unrecognized-property') === '' &&
            rule.getPropertyValue('font-feature-settings') === '' &&
            rule.getPropertyValue('color') === 'red' &&
            face.getPropertyValue('src') === 'url(probe.ttf)';
    })()"#).unwrap().as_boolean(), Some(true));
}

#[test]
fn cssom_pending_border_shorthand_is_valid_and_replaces_earlier_width() {
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse(
        "<div id=probe style='--edge:3px dotted red; border-left:5px solid black; border-width:1px'></div>"
    ).document()).unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
        const element = document.getElementById('probe');
        if (!CSS.supports('border-left', 'var(--edge)')) return false;
        element.style.borderLeft = 'var(--edge)';
        const style = getComputedStyle(element);
        return style.borderLeftWidth === '3px' && style.borderLeftStyle === 'dotted' &&
            style.borderLeftColor === 'rgb(255, 0, 0)' && style.borderRightWidth === '1px';
    })()"#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn cached_cssom_reads_observe_declaration_block_and_property_changes() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
        const sheet = new CSSStyleSheet();
        sheet.replaceSync('div { color:red; width:10px; }');
        const style = sheet.cssRules[0].style;
        if (style.getPropertyValue('width') !== '10px') return false;
        style.cssText = 'width:20px; unknown-property:inherit';
        if (style.length !== 1 || style.color !== '' || style.width !== '20px') return false;
        style.setProperty('width', 'nonsense');
        if (style.width !== '20px') return false;
        style.setProperty('width', '30px');
        if (style.getPropertyValue('width') !== '30px') return false;
        style.removeProperty('width');
        return style.length === 0 && style.getPropertyValue('width') === '';
    })()"#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn malformed_shorthands_cannot_fall_back_to_generic_computed_values() {
    let mut runtime =
        JsRuntime::with_document(TreeBuilder::parse("<div id=probe></div>").document()).unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
        const element = document.getElementById('probe');
        return [
            ['border-radius', '1px 2px 3px 4px 5px'],
            ['animation', '1s 2s 3s'],
            ['background-position', '1px 2px 3px 4px 5px'],
        ].every(([name, value]) => {
            const original = getComputedStyle(element).getPropertyValue(name);
            element.style.setProperty(name, value);
            return !CSS.supports(name, value) && element.style.getPropertyValue(name) === '' &&
                getComputedStyle(element).getPropertyValue(name) === original;
        });
    })()"#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}
