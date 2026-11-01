//! Unsupported declarations must not manufacture computed CSS values.
use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;

#[test]
fn word_spacing_zero_computes_to_a_length_through_each_declaration_boundary() {
    let mut runtime = JsRuntime::with_document(
        TreeBuilder::parse("<style>#target {word-spacing: 0}</style><div id=target></div>")
            .document(),
    )
    .unwrap();
    let actual = runtime
        .eval(
            r#"(() => {
        const target = document.getElementById('target');
        const computed = getComputedStyle(target);
        const values = [computed.wordSpacing];
        target.setAttribute('style', 'word-spacing:0');
        values.push(computed.wordSpacing);
        target.style.setProperty('word-spacing', '0');
        values.push(computed.wordSpacing);
        target.style.cssText = 'word-spacing:0';
        values.push(computed.wordSpacing);
        return JSON.stringify(values);
    })()"#,
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(actual, r#"["0px","0px","0px","0px"]"#);
}

#[test]
fn cssom_computation_closes_implicit_math_functions_before_declaration_delimiters() {
    let mut runtime = JsRuntime::new().unwrap();
    let actual = runtime
        .eval(
            r#"(() => {
        const target = document.body.appendChild(document.createElement('div'));
        const values = [];
        for (const [property, input] of [
            ['word-spacing', 'calc(10px - (5% + 10%)'],
            ['scale', '2 calc(300% * sign(1em - 1px)']
        ]) {
            target.style.setProperty(property, input);
            values.push(getComputedStyle(target).getPropertyValue(property));
            target.style.cssText = property + ':' + input;
            values.push(getComputedStyle(target).getPropertyValue(property));
        }
        target.style.content = '"("';
        values.push(target.style.content);
        return JSON.stringify(values);
    })()"#,
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(
        actual,
        r#"["calc(-15% + 10px)","calc(-15% + 10px)","2 3","2 3","\"(\""]"#
    );
}

#[test]
fn svg_style_updates_and_namespaced_mutations_discard_stale_computation_sources() {
    let mut runtime = JsRuntime::new().unwrap();
    let actual = runtime
        .eval(
            r#"(() => {
        return JSON.stringify([false, true].map(svg => {
            const target = svg
                ? document.createElementNS('http://www.w3.org/2000/svg', 'g')
                : document.createElement('div');
            document.body.appendChild(target);
            const computed = getComputedStyle(target);
            target.style.paintOrder = 'fill stroke';
            target.style.paintOrder = 'fill markers';
            const values = [computed.paintOrder];
            target.style.paintOrder = 'stroke fill';
            target.setAttributeNS(null, 'style', 'paint-order: markers stroke');
            values.push(computed.paintOrder);
            target.style.paintOrder = 'fill stroke';
            target.removeAttributeNS(null, 'style');
            values.push(computed.paintOrder);
            return values;
        }));
    })()"#,
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(
        actual,
        r#"[["fill markers","markers stroke","normal"],["fill markers","markers stroke","normal"]]"#
    );
}

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
            if (name === 'font-feature-settings' || name === 'font-variation-settings' || name === 'counter-set' || name === 'anchor-name' ||
                name === 'hyphenate-character' || name === 'scrollbar-color') {
                return CSS.supports(name, value) &&
                    element.style.getPropertyValue(name) === value &&
                    getComputedStyle(element).getPropertyValue(name) === value;
            }
            return !CSS.supports(name, value) && element.style.getPropertyValue(name) === '' && getComputedStyle(element).getPropertyValue(name) === '';
        });
    })()"#).unwrap().as_boolean(), Some(true));
}

#[test]
fn anchor_position_properties_share_strict_cssom_and_supports_grammar() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
        const element = document.body.appendChild(document.createElement('div'));
        const valid = [
            ['anchor-name', '--menu, --fallback'],
            ['anchor-scope', 'all'],
            ['position-anchor', '--menu'],
            ['position-area', 'block-end span-inline-end'],
            ['position-try-fallbacks', '--below flip-inline, top'],
            ['position-try-order', 'most-block-size'],
            ['position-visibility', 'no-overflow anchor-valid'],
            ['justify-self', 'anchor-center'],
            ['align-self', 'anchor-center'],
            ['width', 'anchor-size(--menu width, anchor-size(--fallback block))'],
            ['left', 'anchor-size(--menu inline, 10px)'],
        ];
        const invalid = [
            ['anchor-name', 'menu'],
            ['anchor-scope', '--'],
            ['position-anchor', 'menu'],
            ['position-area', 'top bottom left'],
            ['position-try-fallbacks', 'unknown'],
            ['position-try-order', 'largest'],
            ['position-visibility', 'always no-overflow'],
        ];
        for (const [name, value] of valid) {
            if (!CSS.supports(name, value)) return `unsupported:${name}:${value}`;
            element.style.setProperty(name, value);
            if (!element.style.getPropertyValue(name)) return `not-stored:${name}:${value}`;
        }
        for (const [name, value] of invalid) {
            element.style.removeProperty(name);
            element.style.setProperty(name, value);
            if (CSS.supports(name, value)) return `accepted:${name}:${value}`;
            if (element.style.getPropertyValue(name) !== '') return `stored:${name}:${value}`;
        }
        return 'ok';
    })()"#
            )
            .unwrap()
            .as_string()
            .map(|value| value.to_std_string_escaped()),
        Some("ok".to_string())
    );
}

#[test]
fn position_try_rule_exposes_name_and_live_descriptor_block() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
            const sheet = new CSSStyleSheet();
            sheet.replaceSync('@position-try --below { top: anchor(bottom); left: 10px; }');
            const rule = sheet.cssRules[0];
            if (!(rule instanceof CSSPositionTryRule) ||
                !(rule.style instanceof CSSPositionTryDescriptors) ||
                rule.name !== '--below' || rule.style.left !== '10px') return false;
            rule.style.left = '20px';
            return rule.cssText.includes('@position-try --below') &&
                rule.cssText.includes('left: 20px;');
        })()"#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
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
            rule.getPropertyValue('font-feature-settings') === '"liga" 0' &&
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
