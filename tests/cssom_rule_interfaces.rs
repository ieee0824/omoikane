use omoikane::{html::TreeBuilder, js::JsRuntime};

fn runtime() -> JsRuntime {
    JsRuntime::with_document(TreeBuilder::parse("<main></main>").document()).expect("runtime")
}

fn eval_json(runtime: &mut JsRuntime, source: &str) -> String {
    runtime
        .eval(source)
        .expect("evaluate CSSOM probe")
        .as_string()
        .expect("JSON string")
        .to_std_string_escaped()
}

#[test]
fn css_rules_expose_the_standard_base_hierarchy() {
    let mut runtime = runtime();
    assert_eq!(
        eval_json(
            &mut runtime,
            "(() => { const sheet = new CSSStyleSheet();
              sheet.replaceSync('a{} @media all { b{} } @font-face { font-family:x; src:url(x) }');
              const [style, media, face] = sheet.cssRules;
              return JSON.stringify([
                style instanceof CSSRule, style instanceof CSSStyleRule,
                media instanceof CSSRule, media instanceof CSSGroupingRule,
                media instanceof CSSConditionRule, face instanceof CSSRule,
                CSSRule.STYLE_RULE, CSSRule.MEDIA_RULE, CSSRule.FONT_FACE_RULE
              ]); })()",
        ),
        "[true,true,true,true,true,true,1,4,5]"
    );
}

#[test]
fn keyframes_rules_are_live_and_mutate_the_owning_stylesheet() {
    let mut runtime = runtime();
    assert_eq!(
        eval_json(
            &mut runtime,
            "(() => { const sheet = new CSSStyleSheet();
              sheet.replaceSync('@keyframes fade { from { opacity: 0 } to { opacity: 1 } }');
              const rule = sheet.cssRules[0], first = rule.cssRules[0];
              rule.appendRule('50% { opacity: .5 }');
              const middle = rule.findRule('50%');
              middle.style.opacity = '.6'; rule.name = 'changed'; rule.deleteRule('to');
              return JSON.stringify([
                rule instanceof CSSKeyframesRule, first instanceof CSSKeyframeRule,
                first.parentRule === rule, rule.parentStyleSheet === sheet,
                rule.name, rule.cssRules.length, middle === rule.findRule('50%'),
                middle.keyText, middle.style.opacity, sheet.cssRules[0] === rule,
                sheet.cssRules[0].cssText.includes('opacity: .6')
              ]); })()",
        ),
        "[true,true,true,true,\"changed\",2,true,\"50%\",\".6\",true,true]"
    );
}

#[test]
fn page_namespace_and_modern_rules_use_dedicated_interfaces() {
    let mut runtime = runtime();
    assert_eq!(
        eval_json(
            &mut runtime,
            "(() => { const sheet = new CSSStyleSheet();
              sheet.replaceSync('@namespace svg url(https://example.test/svg);\\n' +
                '@page report:left { margin: 1cm; @top-center { content: \"title\" } }\\n' +
                '@starting-style { a { opacity: 0 } }\\n' +
                '@view-transition { navigation: auto; types: slide }');
              const [namespace, page, starting, transition] = sheet.cssRules;
              const margin = page.cssRules[0];
              return JSON.stringify([
                namespace instanceof CSSNamespaceRule, namespace.prefix, namespace.namespaceURI,
                page instanceof CSSPageRule, page.selectorText, page.style.margin,
                margin instanceof CSSMarginRule, margin.name, margin.parentRule === page,
                starting instanceof CSSStartingStyleRule,
                starting instanceof CSSGroupingRule, starting.cssRules[0] instanceof CSSStyleRule,
                transition instanceof CSSViewTransitionRule,
                transition.navigation, transition.types
              ]); })()",
        ),
        "[true,\"svg\",\"https://example.test/svg\",true,\"report:left\",\"1cm\",true,\"top-center\",true,true,true,true,true,\"auto\",[\"slide\"]]"
    );
}

#[test]
fn dedicated_rule_mutations_update_the_owning_stylesheet() {
    let mut runtime = runtime();
    assert_eq!(
        eval_json(
            &mut runtime,
            "(() => { const sheet = new CSSStyleSheet();
              sheet.replaceSync('@page { margin: 1cm; @top-center { content: \"old\" } }\\n' +
                '@starting-style { a { opacity: 0 } }\\n' +
                '@view-transition { navigation: auto; types: old }');
              const [page, starting, transition] = sheet.cssRules;
              const margin = page.cssRules[0];
              page.style.margin = '2cm'; margin.style.content = '\"new\"';
              starting.insertRule('b { opacity: 1 }', 1);
              transition.navigation = 'none'; transition.types = 'new';
              return JSON.stringify([
                sheet.cssRules[0] === page, page.cssRules[0] === margin,
                page.style.margin, margin.style.content,
                starting.cssRules.length, starting.cssRules[1].selectorText,
                transition.navigation, transition.types,
                page.cssText.includes('margin-top: 2cm'), page.cssText.includes('content: \"new\"'),
                starting.cssText.includes('b { opacity: 1; }'),
                transition.cssText.includes('navigation: auto')
              ]); })()",
        ),
        "[true,true,\"2cm\",\"\\\"new\\\"\",2,\"b\",\"auto\",[\"old\"],true,true,true,true]"
    );
}

#[test]
fn invalid_view_transition_preludes_are_rejected() {
    let mut runtime = runtime();
    assert_eq!(
        eval_json(
            &mut runtime,
            "(() => { const sheet = new CSSStyleSheet();
              let name = ''; try { sheet.insertRule('@view-transition extra {}'); }
              catch (error) { name = error.name; }
              sheet.replaceSync('@view-transition extra {} main {}');
              return JSON.stringify([name, sheet.cssRules.length,
                sheet.cssRules[0] instanceof CSSStyleRule]); })()",
        ),
        "[\"SyntaxError\",1,true]"
    );
}

#[test]
fn unknown_at_rules_are_discarded_without_hiding_following_rules() {
    let mut runtime = runtime();
    assert_eq!(
        eval_json(
            &mut runtime,
            "(() => { const sheet = new CSSStyleSheet();
              sheet.replaceSync('@unknown foo { a: b } @unknown bar; main { color: red }');
              return JSON.stringify([sheet.cssRules.length,
                sheet.cssRules[0] instanceof CSSStyleRule,
                sheet.cssRules[0].selectorText]); })()",
        ),
        "[1,true,\"main\"]"
    );
}
