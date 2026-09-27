use omoikane::css::{matches_selector, parse_selector_list};
use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;

const HTML: &str = r#"<!doctype html><html><head>
<link id="resource" href="/style.css">
<style>
  a:link, a:visited { color: rgb(17, 34, 51) }
  area:any-link { color: rgb(34, 51, 68) }
</style></head><body>
<a id="empty" href="">empty</a>
<a id="relative" href="/next">relative</a>
<a id="missing">missing</a>
<map><area id="area" href="/map"><area id="area-missing"></map>
<div id="other" href="/next">other</div>
<svg><a id="svg-link" href="/svg">svg</a></svg>
</body></html>"#;

#[test]
fn html_a_and_area_with_href_match_unvisited_link_selectors() {
    let document = TreeBuilder::parse(HTML).document();
    for (selector, expected) in [
        (":any-link", &["empty", "relative", "area"][..]),
        (":link", &["empty", "relative", "area"][..]),
        (":visited", &[][..]),
    ] {
        let selector = parse_selector_list(selector).unwrap().remove(0);
        for id in [
            "empty",
            "relative",
            "missing",
            "area",
            "area-missing",
            "other",
            "resource",
            "svg-link",
        ] {
            let element = document.query_selector(&format!("#{id}")).unwrap();
            assert_eq!(
                matches_selector(&element, &selector),
                expected.contains(&id),
                "{id} against {selector:?}"
            );
        }
    }
}

#[test]
fn link_selectors_apply_styles_and_match_dom_queries() {
    let document = TreeBuilder::parse(HTML).document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let result = runtime
        .eval(
            r#"(() => {
                const ids = selector => Array.from(document.querySelectorAll(selector), n => n.id).join(',');
                if (ids(':any-link') !== 'empty,relative,area') return false;
                if (ids(':link') !== 'empty,relative,area') return false;
                if (ids(':visited') !== '') return false;
                if (!document.getElementById('empty').matches(':link')) return false;
                if (!document.getElementById('area').matches(':any-link')) return false;
                if (document.getElementById('missing').matches(':link')) return false;
                if (document.getElementById('resource').matches(':any-link')) return false;
                if (document.getElementById('svg-link').matches(':any-link')) return false;
                if (getComputedStyle(document.getElementById('empty')).color !== 'rgb(17, 34, 51)') return false;
                if (getComputedStyle(document.getElementById('relative')).color !== 'rgb(17, 34, 51)') return false;
                if (getComputedStyle(document.getElementById('area')).color !== 'rgb(34, 51, 68)') return false;
                return getComputedStyle(document.getElementById('missing')).color !== 'rgb(17, 34, 51)';
            })()"#,
        )
        .unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn href_mutations_update_link_selectors_and_computed_style() {
    let document = TreeBuilder::parse(HTML).document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let result = runtime
        .eval(
            r#"(() => {
                const link = document.getElementById('missing');
                const area = document.getElementById('area-missing');
                const linkColor = () => getComputedStyle(link).color;
                const areaColor = () => getComputedStyle(area).color;
                const linked = element => element.matches(':any-link') && element.matches(':link');
                const count = () => document.querySelectorAll(':any-link').length;
                const originalLinkColor = linkColor();
                const originalAreaColor = areaColor();
                if (linked(link) || linked(area) || count() !== 3) return false;

                link.setAttribute('href', '');
                if (!linked(link) || count() !== 4 || linkColor() !== 'rgb(17, 34, 51)') return false;
                link.setAttribute('href', '/changed');
                if (!linked(link) || !link.matches('[href="/changed"]') || count() !== 4) return false;
                link.removeAttribute('href');
                if (linked(link) || count() !== 3 || linkColor() !== originalLinkColor) return false;
                link.href = '/property-assigned';
                if (!(link instanceof HTMLAnchorElement) || !linked(link) ||
                    link.getAttribute('href') !== '/property-assigned' || count() !== 4 ||
                    linkColor() !== 'rgb(17, 34, 51)') return false;
                link.removeAttribute('href');
                if (linked(link) || count() !== 3) return false;

                area.setAttribute('href', '/map/new');
                if (!linked(area) || count() !== 4 || areaColor() !== 'rgb(34, 51, 68)') return false;
                area.removeAttribute('href');
                area.href = '/map/property-assigned';
                if (!(area instanceof HTMLAreaElement) || !linked(area) ||
                    area.getAttribute('href') !== '/map/property-assigned' || count() !== 4 ||
                    areaColor() !== 'rgb(34, 51, 68)') return false;
                area.removeAttribute('href');
                return !linked(area) && count() === 3 && areaColor() === originalAreaColor;
            })()"#,
        )
        .unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}
