//! Filtered sibling indexing in selector APIs and the cascade.

use omoikane::{html::TreeBuilder, js::JsRuntime};

#[test]
fn nth_filter_counts_matching_siblings_in_both_directions() {
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse(
        "<main id=scope><i id=a class=pick></i>text<i id=b></i><!--gap--><b id=c class=pick></b><i id=d class=pick></i></main>"
    ).document()).unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
        const scope = document.getElementById('scope');
        const ids = s => Array.from(scope.querySelectorAll(s), n => n.id).join(',');
        return ids(':nth-child(2 of .pick)') === 'c' &&
            ids(':nth-last-child(2 of .pick)') === 'c' &&
            ids('i:nth-child(2 of .pick)') === '' &&
            ids(':nth-child(odd of .pick, #a)') === 'a,d' &&
            ids(':nth-child(-n+2 of :scope > .pick)') === 'a,c' &&
            ids(':nth-child(1 of [id="of"], #b)') === 'b';
    })()"#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn nth_filter_support_queries_and_invalid_lists() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(runtime.eval(r#"(() => {
        const valid = [':nth-child(2n+1 of .pick)', ':nth-last-child(1 OF #x, .y)',
            ':nth-child(1 of :is(.x, .y))'];
        const invalid = [':nth-child(1 of)', ':nth-child(of .x)',
            ':nth-child(1 of .x,)', ':nth-child(1 of .x, :unknown)',
            ':nth-child(1 of ::before)', ':nth-of-type(1 of .x)',
            ':nth-last-of-type(1 of .x)', ':nth-child(1 of > .x)'];
        return valid.filter(s => !CSS.supports(`selector(${s})`) || document.querySelectorAll(s).length < 0)
            .concat(invalid.filter(s => {
                if (CSS.supports(`selector(${s})`)) return true;
                try { document.querySelectorAll(s); return true; }
                catch (e) { return e.name !== 'SyntaxError'; }
            })).join(';');
    })()"#).unwrap().as_string().unwrap().to_std_string_escaped(), "");
}

#[test]
fn nth_filter_cascade_and_mutations_recompute_sibling_positions() {
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse(
        "<style>:nth-child(2 of .pick, #absent) { color: rgb(1, 2, 3) } .pick.pick { color: rgb(4, 5, 6) } @supports selector(:nth-child(2n+1 of .pick)) { #scope { background-color: rgb(7, 8, 9) } }</style><main id=scope><i id=a class=pick></i><i id=b class=pick></i><i id=c class=pick></i></main>"
    ).document()).unwrap();
    assert_eq!(runtime.eval(r#"(() => {
        const a = document.getElementById('a'), b = document.getElementById('b');
        const c = document.getElementById('c'), scope = document.getElementById('scope');
        const selected = () => Array.from(scope.querySelectorAll(':nth-child(2 of .pick)'), n => n.id).join(',');
        const color = n => getComputedStyle(n).color;
        if (selected() !== 'b' || color(b) !== 'rgb(1, 2, 3)' ||
            getComputedStyle(scope).backgroundColor !== 'rgb(7, 8, 9)') return false;
        a.className = '';
        if (selected() !== 'c' || color(c) !== 'rgb(1, 2, 3)' || color(b) !== 'rgb(4, 5, 6)') return false;
        scope.insertBefore(c, b);
        if (selected() !== 'b' || color(b) !== 'rgb(1, 2, 3)') return false;
        c.remove();
        return selected() === '' && color(b) === 'rgb(4, 5, 6)';
    })()"#).unwrap().as_boolean(), Some(true));
}

#[test]
fn nth_filter_nested_selectors_and_inclusive_siblings() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(runtime.eval(r#"(() => {
        const node = document.createElement('i');
        node.className = 'pick';
        if (!node.matches(':first-child:last-child:only-child:nth-of-type(1):nth-last-of-type(1)') ||
            !node.matches(':nth-child(1 of .pick):nth-last-child(1 of .pick)') ||
            node.matches(':nth-child(1 of .absent)')) return false;
        const fragment = document.createDocumentFragment();
        fragment.appendChild(document.createTextNode('ignored'));
        fragment.appendChild(node);
        const next = document.createElement('i');
        next.className = 'pick';
        fragment.appendChild(next);
        if (!next.matches(':nth-child(2 of .pick)') ||
            !node.matches(':nth-last-child(2 of .pick)') ||
            !document.documentElement.matches(':nth-child(1 of html)')) return false;
        const container = document.createElement('div');
        container.innerHTML = '<i id="a"><b></b></i><i id="b"></i><i id="c"><b></b></i>';
        document.body.appendChild(container);
        return container.querySelector(':nth-child(2 of :has(b))').id === 'c' &&
            container.querySelector(':nth-child(2 of :nth-child(2n+1))').id === 'c' &&
            CSS.supports('selector(:nth-child(2 of :nth-child(2n+1)))') &&
            container.querySelector(':not(:nth-child(2n+1))').id === 'b';
    })()"#).unwrap().as_boolean(), Some(true));
}

#[test]
fn nth_filter_invalid_forgiving_branch_does_not_discard_the_rule() {
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse(
        "<style>.fallback { color: green } :is(.pick, #unused:nth-child(1 of)) { color: red }</style><i id=probe class='pick fallback'></i>"
    ).document()).unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
        const probe = document.getElementById('probe');
        const list = probe.classList;
        list.value = 'pick fallback';
        return list.value === probe.className &&
            probe.matches(':is(.pick, #unused:nth-child(1 of))') &&
            getComputedStyle(probe).color === 'rgb(255, 0, 0)';
    })()"#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn nth_filter_attribute_and_focus_changes_invalidate_other_siblings() {
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse(
        "<style>:nth-child(2 of [data-pick]) { color: red } :nth-child(2 of .pick, :focus) { background-color: blue }</style><main><input id=a><i id=b class=pick data-pick></i><i id=c class=pick data-pick></i></main>"
    ).document()).unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
        const a = document.getElementById('a'), b = document.getElementById('b');
        const c = document.getElementById('c');
        const red = n => getComputedStyle(n).color === 'rgb(255, 0, 0)';
        const blue = n => getComputedStyle(n).backgroundColor === 'rgb(0, 0, 255)';
        if (red(b) || !red(c) || blue(b) || !blue(c)) return false;
        a.setAttribute('data-pick', '');
        if (!red(b) || red(c)) return false;
        a.focus();
        return blue(b) && !blue(c);
    })()"#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn nth_filter_scope_references_include_each_scope_root() {
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse(
        "<style>@scope (.card) { :nth-child(1 of :scope) { background-color: rgb(1, 2, 3) } }</style><section class=card id=first><i id=child></i></section><section class=card id=second></section>"
    ).document()).unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
        const background = id => getComputedStyle(document.getElementById(id)).backgroundColor;
        return background('first') === 'rgb(1, 2, 3)' &&
            background('second') === 'rgb(1, 2, 3)' &&
            background('child') === 'rgba(0, 0, 0, 0)';
    })()"#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn nth_filter_scope_boundary_preserves_signed_formulas() {
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse(
        "<style>@scope (:nth-child(2n+1 of .card)) { :scope { background-color: red } }</style><section class=card id=first></section><section class=card id=second></section><section class=card id=third></section>"
    ).document()).unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
        const background = id => getComputedStyle(document.getElementById(id)).backgroundColor;
        return background('first') === 'rgb(255, 0, 0)' &&
            background('third') === 'rgb(255, 0, 0)' &&
            background('second') === 'rgba(0, 0, 0, 0)';
    })()"#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn nth_filter_forgiving_unknown_branches_do_not_raise_specificity() {
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse(
        "<style>:nth-child(1 of :is(.pick, #unused:unknown)) { color: red } .pick.pick.pick { color: green }</style><i id=probe class=pick></i>"
    ).document()).unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
        const probe = document.getElementById('probe');
        return probe.matches(':nth-child(1 of :is(.pick, #unused:unknown))') &&
            probe.matches(':nth-child(1 of :is(.pick, ::before))') &&
            !CSS.supports('selector(:nth-child(1 of :is(.pick, #unused:unknown)))') &&
            getComputedStyle(probe).color === 'rgb(0, 128, 0)';
    })()"#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}
