use omoikane::{html::TreeBuilder, js::JsRuntime};

#[test]
fn sibling_queries_and_style_follow_insert_remove_and_reorder() {
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse(
        "<style>i + i { color: rgb(1, 2, 3) }</style><main id='scope'><i id='a'></i>text<!--comment--><i id='b'></i><i id='c'></i></main>"
    ).document()).unwrap();
    assert_eq!(runtime.eval(r#"(() => {
        const scope = document.getElementById('scope');
        const a = document.getElementById('a'), b = document.getElementById('b'), c = document.getElementById('c');
        const ids = selector => Array.from(scope.querySelectorAll(selector), n => n.id).join(',');
        if (ids('i + i') !== 'b,c' || ids('i:has(+ i)') !== 'a,b' || ids('i:nth-child(2n)') !== 'b') return false;
        if (getComputedStyle(b).color !== 'rgb(1, 2, 3)') return false;
        scope.insertBefore(b, a);
        if (ids('i + i') !== 'a,c' || ids('i:has(+ i)') !== 'b,a' || ids('i:nth-child(2n)') !== 'a') return false;
        if (getComputedStyle(a).color !== 'rgb(1, 2, 3)' || getComputedStyle(b).color === 'rgb(1, 2, 3)') return false;
        a.remove();
        if (ids('i + i') !== 'c' || ids('i ~ i') !== 'c') return false;
        scope.appendChild(b);
        return ids('i + i') === 'b' && ids('i:has(+ i)') === 'c' &&
            getComputedStyle(b).color === 'rgb(1, 2, 3)' && getComputedStyle(c).color !== 'rgb(1, 2, 3)';
    })()"#).unwrap().as_boolean(), Some(true));
}

#[test]
fn query_caches_are_scoped_to_each_light_shadow_and_template_search() {
    let mut runtime = JsRuntime::new().unwrap();
    assert_eq!(runtime.eval(r#"(() => {
        const host = document.createElement('div');
        document.body.appendChild(host);
        host.innerHTML = '<i id="light-a"></i><i id="light-b"></i><template><i id="inert-a"></i><i id="inert-b"></i></template>';
        const root = host.attachShadow({mode: 'open'});
        root.innerHTML = '<i id="shadow-a"></i><i id="shadow-b"></i>';
        const template = host.querySelector('template');
        const one = (scope, id) => {
            const result = scope.querySelectorAll('i + i');
            return result.length === 1 && result[0].id === id;
        };
        return one(host, 'light-b') && one(document, 'light-b') &&
            one(root, 'shadow-b') && one(template.content, 'inert-b');
    })()"#).unwrap().as_boolean(), Some(true));
}
