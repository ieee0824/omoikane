use omoikane::css::{Rule, matches_selector, parse_selector_list, parse_stylesheet};
use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;

const LOCATION_PSEUDOS: [&str; 4] = [":link", ":visited", ":any-link", ":target"];

#[test]
fn location_pseudos_parse_in_standalone_lists_and_stylesheets() {
    for pseudo in LOCATION_PSEUDOS {
        assert_eq!(parse_selector_list(&format!("a{pseudo}")).unwrap().len(), 1);
        assert_eq!(
            parse_selector_list(&format!("a{}", pseudo.to_ascii_uppercase()))
                .unwrap()
                .len(),
            1
        );
    }

    let selectors =
        parse_selector_list("a:link, a:visited, area:any-link, #anchor:target").unwrap();
    assert_eq!(selectors.len(), 4);

    let stylesheet =
        parse_stylesheet("a:link, a:visited, area:any-link, #anchor:target { color: red; }")
            .unwrap();
    let [Rule::Style(rule)] = stylesheet.rules.as_slice() else {
        panic!("expected one style rule");
    };
    assert_eq!(rule.selectors.len(), 4);
}

#[test]
fn location_pseudos_reject_arguments_and_unknown_names() {
    for pseudo in LOCATION_PSEUDOS {
        for invalid in [
            format!("a{pseudo}(x)"),
            format!("a{}(x)", pseudo.to_ascii_uppercase()),
            format!("a{pseudo}(x), a:visited"),
        ] {
            assert!(parse_selector_list(&invalid).is_err(), "accepted {invalid}");
            assert!(
                parse_stylesheet(&format!("{invalid} {{ color: red; }}")).is_err(),
                "accepted {invalid} in stylesheet"
            );
        }
    }
    assert!(parse_selector_list("a:unknown-location").is_err());
    assert!(parse_selector_list("a:link, a:unknown-location").is_err());
}

#[test]
fn location_pseudos_are_syntactically_valid_in_matcher_and_dom_queries() {
    let document = TreeBuilder::parse(
        r#"<html><body><a id="anchor" href="/next">next</a><area href="/map"></body></html>"#,
    )
    .document();
    let anchor = document.query_selector("#anchor").unwrap();
    for pseudo in LOCATION_PSEUDOS {
        let selector = parse_selector_list(&format!("a{pseudo}")).unwrap();
        // The dependent issues define matching semantics; parsing and matching
        // must remain usable regardless of the eventual result.
        let _ = matches_selector(&anchor, &selector[0]);
    }

    let mut runtime = JsRuntime::with_document(document).unwrap();
    let result = runtime
        .eval(
            r#"(() => {
                const anchor = document.getElementById('anchor');
                for (const pseudo of [':link', ':visited', ':any-link', ':target']) {
                    try {
                        document.querySelector(`a${pseudo}`);
                        document.querySelectorAll(`a${pseudo}`);
                        anchor.matches(`a${pseudo}`);
                    } catch (_) { return false; }
                }
                for (const selector of ['a:link, a:visited', 'a:unknown-location', 'a:link(x)']) {
                    const invalid = selector !== 'a:link, a:visited';
                    for (const run of [
                        () => document.querySelector(selector),
                        () => document.querySelectorAll(selector),
                        () => anchor.matches(selector),
                    ]) {
                        try {
                            run();
                            if (invalid) return false;
                        } catch (error) {
                            if (!invalid || error.name !== 'SyntaxError') return false;
                        }
                    }
                }
                return true;
            })()"#,
        )
        .unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}
