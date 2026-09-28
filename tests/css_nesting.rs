//! CSS Nesting behavior at the DOM and CSSOM boundary.

use omoikane::{html::TreeBuilder, js::JsRuntime};

fn assert_js(runtime: &mut JsRuntime, expression: &str) {
    let value = runtime
        .eval(expression)
        .unwrap_or_else(|error| panic!("{expression}: {error}"));
    assert_eq!(value.as_boolean(), Some(true), "{expression}");
}

#[test]
fn nested_rules_apply_and_later_declarations_keep_cascade_order() {
    let document = TreeBuilder::parse(
        "<style>.parent { color: blue; & .child { width: 12px; } color: green; \
         @media screen { & .child { height: 14px; } } }</style>\
         <div class=parent id=parent><span class=child id=child></span></div>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).expect("runtime");
    assert_js(
        &mut runtime,
        "getComputedStyle(document.getElementById('parent')).color === 'rgb(0, 128, 0)' && \
         getComputedStyle(document.getElementById('child')).width === '12px' && \
         getComputedStyle(document.getElementById('child')).height === '14px'",
    );
}

#[test]
fn style_rule_children_are_live_and_serialize_in_source_order() {
    let mut runtime = JsRuntime::new().expect("runtime");
    assert_js(
        &mut runtime,
        "(() => { const sheet = new CSSStyleSheet(); sheet.insertRule('.a { color: red; }'); \
         const rule = sheet.cssRules[0]; \
         if (!(rule instanceof CSSGroupingRule) || rule.cssRules.length !== 0) return false; \
         rule.insertRule('& .b { color: green; }'); \
         rule.insertRule('& .c { color: blue; }', 1); \
         rule.insertRule('& .d { color: hotpink; }', 1); \
         if (rule.cssRules.length !== 3 || rule.cssRules[1].parentRule !== rule || \
             rule.cssRules[1].selectorText !== '& .d') return false; \
         rule.deleteRule(1); \
         return rule.cssText === '.a {\\n  color: red;\\n  & .b { color: green; }\\n  & .c { color: blue; }\\n}'; \
         })()",
    );
}

#[test]
fn nested_declarations_and_group_rules_are_visible_in_cssom() {
    let mut runtime = JsRuntime::new().expect("runtime");
    assert_js(
        &mut runtime,
        "(() => { const sheet = new CSSStyleSheet(); \
         sheet.replaceSync('.a { & { --x: 1; } --y: 2; @media screen { --z: 3; .b {} } }'); \
         const outer = sheet.cssRules[0]; const late = outer.cssRules[1]; \
         const media = outer.cssRules[2]; \
         return outer.cssRules.length === 3 && late instanceof CSSNestedDeclarations && \
           late.cssText === '--y: 2;' && late.style.getPropertyValue('--y') === '2' && \
           media instanceof CSSMediaRule && media.cssRules.length === 2 && \
           media.cssRules[0] instanceof CSSNestedDeclarations && \
           media.cssRules[1].selectorText === '& .b'; \
         })()",
    );
}

#[test]
fn nested_declarations_can_be_inserted_but_invalid_rules_are_rejected() {
    let mut runtime = JsRuntime::new().expect("runtime");
    assert_js(
        &mut runtime,
        "(() => { const sheet = new CSSStyleSheet(); sheet.replaceSync('.a {}'); \
         const rule = sheet.cssRules[0]; rule.insertRule('width: 100px; height: 200px;'); \
         if (!(rule.cssRules[0] instanceof CSSNestedDeclarations) || \
             rule.cssRules[0].cssText !== 'width: 100px; height: 200px;') return false; \
         try { rule.insertRule('% {}'); return false; } \
         catch (error) { return error.name === 'SyntaxError' && rule.cssRules.length === 1; } \
         })()",
    );
}

#[test]
fn nested_selector_lists_are_normalized_per_branch() {
    let mut runtime = JsRuntime::new().expect("runtime");
    assert_js(
        &mut runtime,
        "(() => { const sheet = new CSSStyleSheet(); \
         sheet.replaceSync('.foo { .bar, .baz { color: red; } > .bar, .foo & { width: 1px; } }'); \
         const children = sheet.cssRules[0].cssRules; \
         return children.length === 2 && \
           children[0].selectorText === '& .bar, & .baz' && \
           children[1].selectorText === '& > .bar, .foo &'; \
         })()",
    );
}

#[test]
fn ampersand_in_attribute_value_does_not_count_as_nesting_selector() {
    let mut runtime = JsRuntime::new().expect("runtime");
    assert_js(
        &mut runtime,
        r#"(() => {
            const sheet = new CSSStyleSheet();
            sheet.replaceSync('.a { [data-code="&"] { color: red; } }');
            return sheet.cssRules[0].cssRules[0].selectorText === '& [data-code="&"]';
        })()"#,
    );
}

#[test]
fn top_level_ampersand_matches_root_with_zero_specificity() {
    let document = TreeBuilder::parse(
        "<style>& { color: red; } :where(&) { color: green; }</style>\
         <div id=p><span class=match></span><span class=match></span></div>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).expect("runtime");
    assert_js(
        &mut runtime,
        "getComputedStyle(document.documentElement).color === 'rgb(0, 128, 0)'",
    );
}

#[test]
fn malformed_nested_rules_do_not_hide_later_valid_rules() {
    let markup = "<style>html { z-index: 0; error3234 {**** ::} \
         #test1 { @media screen { z-index: 1; } } \
         { whatever } :doesntexist {z-index: 9;} \
         #test2 { z-index: 2; } \
         #test3 { z-index: 3; @media screen { error3234; { foo } z-index: 4; } \
         { foo } z-index: 5; } }</style>\
         <div id=test1></div><div id=test2></div><div id=test3></div>";
    let document = TreeBuilder::parse(markup).document();
    let mut runtime = JsRuntime::with_document(document).expect("runtime");
    assert_js(
        &mut runtime,
        "getComputedStyle(document.getElementById('test1')).zIndex === '1' && \
         getComputedStyle(document.getElementById('test2')).zIndex === '2' && \
         getComputedStyle(document.getElementById('test3')).zIndex === '5'",
    );
}

#[test]
fn empty_nested_declarations_do_not_add_blank_serialized_lines() {
    let mut runtime = JsRuntime::new().expect("runtime");
    assert_js(
        &mut runtime,
        "(() => { const sheet = new CSSStyleSheet(); \
         sheet.replaceSync('.a { & { } left: 1px; & { } right: 1px; }'); \
         const rule = sheet.cssRules[0]; \
         if (rule.cssRules.length !== 4) return false; \
         for (const child of rule.cssRules) child.style = ''; \
         return rule.cssText === '.a {\\n  & { }\\n  & { }\\n}'; \
         })()",
    );
}

#[test]
fn cssom_recovers_open_nested_blocks_and_rejects_nested_font_face() {
    let mut runtime = JsRuntime::new().expect("runtime");
    assert_js(
        &mut runtime,
        "(() => { const sheet = new CSSStyleSheet(); \
         sheet.insertRule('div { @media screen { & { color: red; }'); \
         const rule = sheet.cssRules[0]; \
         if (rule.cssRules.length !== 1 || rule.cssRules[0].cssRules.length !== 1) return false; \
         try { rule.cssRules[0].cssRules[0].insertRule('@font-face {}'); return false; } \
         catch (error) { return error.name === 'HierarchyRequestError'; } \
         })()",
    );
}

#[test]
fn boolean_width_media_and_container_queries_match_nested_rules() {
    let document = TreeBuilder::parse(
        "<style>.a { @media (width) { & .x { z-index: 1; } } } \
         .a { @container (width) { & .x { color: red; } } }</style>\
         <main style='container-type:size;width:50px;height:50px'>\
         <div class=a><div class=x id=x></div></div></main>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).expect("runtime");
    assert_js(
        &mut runtime,
        "getComputedStyle(document.getElementById('x')).zIndex === '1' && \
         getComputedStyle(document.getElementById('x')).color === 'rgb(255, 0, 0)'",
    );
}
