use omoikane::{html::TreeBuilder, js::JsRuntime};

fn runtime() -> JsRuntime {
    JsRuntime::with_document(TreeBuilder::parse("<main></main>").document()).expect("runtime")
}

fn json(runtime: &mut JsRuntime, source: &str) -> String {
    runtime
        .eval(source)
        .expect("evaluate URLPattern probe")
        .as_string()
        .expect("JSON string")
        .to_std_string_escaped()
}

#[test]
fn string_patterns_expose_components_and_named_groups() {
    let mut runtime = runtime();
    assert_eq!(
        json(
            &mut runtime,
            r#"(() => {
              const pattern = new URLPattern('https://:sub.example.com/books/:id');
              const match = pattern.exec('https://www.example.com/books/42');
              return JSON.stringify([
                pattern.protocol, pattern.hostname, pattern.pathname,
                pattern.hasRegExpGroups,
                pattern.test('https://www.example.com/books/42'),
                pattern.test('https://www.example.com/music/42'),
                match.hostname.input, match.hostname.groups.sub,
                match.pathname.groups.id, match.inputs
              ]);
            })()"#,
        ),
        "[\"https\",\":sub.example.com\",\"/books/:id\",false,true,false,\"www.example.com\",\"www\",\"42\",[\"https://www.example.com/books/42\"]]"
    );
}

#[test]
fn dictionary_patterns_resolve_base_urls_and_support_ignore_case() {
    let mut runtime = runtime();
    assert_eq!(
        json(
            &mut runtime,
            r#"(() => {
              const pattern = new URLPattern({pathname:'/Docs/:page', baseURL:'https://example.test/root/'}, {ignoreCase:true});
              const match = pattern.exec('/docs/Intro', 'https://example.test');
              return JSON.stringify([
                pattern.test('https://example.test/DOCS/start'),
                match.pathname.groups.page,
                match.inputs.length,
                Object.prototype.toString.call(pattern),
                URLPattern.length,
                Object.getOwnPropertyDescriptor(globalThis, 'URLPattern').enumerable
              ]);
            })()"#,
        ),
        "[true,\"Intro\",2,\"[object URLPattern]\",1,false]"
    );
}

#[test]
fn conversion_and_brand_checks_happen_at_the_public_boundary() {
    let mut runtime = runtime();
    assert_eq!(
        json(
            &mut runtime,
            r#"(() => {
              const order = [];
              const input = {
                get protocol() { order.push('protocol'); return 'https'; },
                get hostname() { order.push('hostname'); return 'example.test'; },
                get pathname() { order.push('pathname'); return '/:name'; },
              };
              const pattern = new URLPattern(input);
              const before = order.slice();
              let brand = false;
              try { URLPattern.prototype.test.call({}, 'https://example.test/a'); } catch (error) { brand = error instanceof TypeError; }
              return JSON.stringify([before, pattern.test('https://example.test/a'), brand, pattern.exec()]);
            })()"#,
        ),
        "[[\"protocol\",\"hostname\",\"pathname\"],true,true,null]"
    );
}
