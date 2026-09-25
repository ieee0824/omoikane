use omoikane::cdp::CdpSession;
use omoikane::frame::render_browser_frame;
use serde_json::{Value, json};

fn session_for(html: &str) -> CdpSession {
    let mut session = CdpSession::new().expect("session");
    let url = format!(
        "data:text/html,{}",
        html.bytes()
            .map(|byte| format!("%{byte:02X}"))
            .collect::<String>()
    );
    session
        .dispatch("Page.navigate", json!({ "url": url }))
        .expect("navigate");
    session
}

fn find(session: &mut CdpSession, action: &str, query: &str) -> Value {
    session
        .dispatch(
            "Omoikane.findInPage",
            json!({ "action": action, "query": query }),
        )
        .expect("find-in-page")
}

fn eval(session: &mut CdpSession, expression: &str) -> Value {
    session
        .dispatch(
            "Runtime.evaluate",
            json!({ "expression": expression, "returnByValue": true }),
        )
        .expect("evaluate")["result"]["value"]
        .clone()
}

#[test]
fn find_matches_navigate_wrap_and_end() {
    let mut session = session_for("<p>alpha alpha</p><p>alpha</p>");
    assert_eq!(find(&mut session, "start", "alpha")["matchCount"], 3);
    assert_eq!(find(&mut session, "status", "")["activeMatchOrdinal"], 1);
    assert_eq!(find(&mut session, "next", "")["activeMatchOrdinal"], 2);
    assert_eq!(find(&mut session, "next", "")["activeMatchOrdinal"], 3);
    assert_eq!(find(&mut session, "next", "")["activeMatchOrdinal"], 1);
    assert_eq!(find(&mut session, "previous", "")["activeMatchOrdinal"], 3);
    assert_eq!(find(&mut session, "start", "missing")["matchCount"], 0);
    assert_eq!(find(&mut session, "next", "")["activeMatchOrdinal"], 0);
    assert_eq!(find(&mut session, "previous", "")["activeMatchOrdinal"], 0);
    assert_eq!(find(&mut session, "stop", "")["matchCount"], 0);
    assert_eq!(eval(&mut session, "String(getSelection())"), "");
}

#[test]
fn hidden_text_is_excluded_but_offscreen_auto_is_found_and_revealed() {
    let mut session = session_for(
        r#"<style>
            #none { display: none }
            #hidden { content-visibility: hidden }
            #auto { content-visibility: auto; contain-intrinsic-size: 80px }
          </style>
          <p id="none">notfound</p><p id="hidden">notfound</p>
          <div style="height:1400px"></div><p id="auto">targetword</p>"#,
    );
    assert_eq!(find(&mut session, "start", "notfound")["matchCount"], 0);
    assert_eq!(
        eval(&mut session, "document.getElementById('auto').innerText"),
        ""
    );
    assert_eq!(find(&mut session, "start", "targetword")["matchCount"], 1);
    assert_eq!(eval(&mut session, "String(getSelection())"), "targetword");
    assert_eq!(
        eval(&mut session, "document.getElementById('auto').innerText"),
        "targetword"
    );
    assert!(eval(&mut session, "window.scrollY").as_f64().unwrap_or(0.0) > 0.0);
    let top = eval(
        &mut session,
        "document.getElementById('auto').getBoundingClientRect().top",
    )
    .as_f64()
    .unwrap();
    assert!(
        (0.0..720.0).contains(&top),
        "active match is outside the viewport: {top}"
    );
    find(&mut session, "stop", "");
    assert_eq!(eval(&mut session, "String(getSelection())"), "");
    eval(&mut session, "window.scrollTo(0, 0)");
    assert_eq!(
        eval(&mut session, "document.getElementById('auto').innerText"),
        ""
    );
}

#[test]
fn status_refreshes_after_dom_and_style_changes() {
    let mut session =
        session_for("<style>#second{display:none}</style><p>needle</p><p id='second'>needle</p>");
    assert_eq!(find(&mut session, "start", "needle")["matchCount"], 1);
    eval(
        &mut session,
        "document.getElementById('second').style.display = 'block'",
    );
    assert_eq!(find(&mut session, "status", "")["matchCount"], 2);
    eval(&mut session, "document.querySelector('p').remove()");
    assert_eq!(find(&mut session, "status", "")["matchCount"], 1);
}

#[test]
fn searches_shadow_and_iframe_text_once() {
    let mut session = session_for(
        r#"<div id="host"></div><iframe srcdoc="<p>branchword</p>"></iframe>
           <script>document.getElementById('host').attachShadow({mode:'open'}).innerHTML='<p>branchword</p>';</script>"#,
    );
    assert_eq!(find(&mut session, "start", "branchword")["matchCount"], 2);
    assert_eq!(find(&mut session, "next", "")["activeMatchOrdinal"], 2);
}

#[test]
fn literal_unicode_query_and_empty_nodes_keep_document_order() {
    let mut session = session_for("<p id='a'>a.b a.b</p><p></p><p id='b'>A.B</p><p>&#x1F600;</p>");
    eval(
        &mut session,
        "document.getElementById('a').append(document.createTextNode(''))",
    );
    assert_eq!(find(&mut session, "start", "a.b")["matchCount"], 3);
    assert_eq!(
        eval(&mut session, "getSelection().anchorNode.parentElement.id"),
        "a"
    );
    assert_eq!(find(&mut session, "next", "")["activeMatchOrdinal"], 2);
    assert_eq!(find(&mut session, "next", "")["activeMatchOrdinal"], 3);
    assert_eq!(
        eval(&mut session, "getSelection().anchorNode.parentElement.id"),
        "b"
    );
    assert_eq!(find(&mut session, "start", "😀")["matchCount"], 1);
    assert_eq!(eval(&mut session, "String(getSelection())"), "😀");
}

#[test]
fn navigation_discards_old_page_matches_and_selection() {
    let mut session = session_for("<p>oldword</p>");
    assert_eq!(find(&mut session, "start", "oldword")["matchCount"], 1);
    let url = "data:text/html,%3Cp%3Enewword%3C%2Fp%3E";
    session
        .dispatch("Page.navigate", json!({ "url": url }))
        .unwrap();
    assert_eq!(find(&mut session, "status", "")["matchCount"], 0);
    assert_eq!(find(&mut session, "start", "newword")["matchCount"], 1);
    assert_eq!(eval(&mut session, "String(getSelection())"), "newword");
}

#[test]
fn restores_existing_selection_when_search_ends() {
    let mut session = session_for("<p id='a'>original</p><p>needle</p>");
    eval(
        &mut session,
        "const range=document.createRange();range.selectNodeContents(document.getElementById('a'));getSelection().addRange(range)",
    );
    assert_eq!(eval(&mut session, "String(getSelection())"), "original");
    assert_eq!(find(&mut session, "start", "needle")["matchCount"], 1);
    find(&mut session, "stop", "");
    assert_eq!(eval(&mut session, "String(getSelection())"), "original");
}

#[test]
fn restores_preexisting_selections_in_top_and_iframe_documents() {
    let mut session = session_for(
        "<p id='top-original'>top-original</p><p>needle</p><iframe srcdoc='<p id=\"frame-original\">frame-original</p><p>needle</p>'></iframe>",
    );
    eval(
        &mut session,
        "const topRange=document.createRange();topRange.selectNodeContents(document.getElementById('top-original'));getSelection().addRange(topRange);const child=document.querySelector('iframe').contentDocument;const frameRange=child.createRange();frameRange.selectNodeContents(child.getElementById('frame-original'));child.getSelection().addRange(frameRange)",
    );
    assert_eq!(find(&mut session, "start", "needle")["matchCount"], 2);
    assert_eq!(find(&mut session, "next", "")["activeMatchOrdinal"], 2);
    find(&mut session, "stop", "");
    assert_eq!(eval(&mut session, "String(getSelection())"), "top-original");
    assert_eq!(
        eval(
            &mut session,
            "String(document.querySelector('iframe').contentDocument.getSelection())"
        ),
        "frame-original"
    );
}

#[test]
fn firefox_reference_fixture_has_the_same_searchability_matrix() {
    let mut session = session_for(include_str!("fixtures/find-in-page/basic.html"));
    for (query, expected) in [
        ("ordinarytoken", 2),
        ("autotoken", 1),
        ("hiddentoken", 0),
        ("nonetoken", 0),
        ("shadowtoken", 1),
        ("frametoken", 1),
    ] {
        assert_eq!(
            find(&mut session, "start", query)["matchCount"],
            expected,
            "{query}"
        );
    }
    assert_eq!(
        eval(
            &mut session,
            "String(document.querySelector('iframe').contentDocument.getSelection())"
        ),
        "frametoken"
    );
}

#[test]
fn unchanged_status_reuses_matches_but_dom_mutation_invalidates_them() {
    let mut session = session_for("<p>needle</p>");
    eval(
        &mut session,
        "globalThis.styleCalls=0;const originalStyle=getComputedStyle;globalThis.getComputedStyle=(node)=>{styleCalls++;return originalStyle(node)}",
    );
    assert_eq!(find(&mut session, "start", "needle")["matchCount"], 1);
    let before = eval(&mut session, "styleCalls").as_u64().unwrap();
    assert_eq!(find(&mut session, "status", "")["matchCount"], 1);
    assert_eq!(eval(&mut session, "styleCalls"), before);
    eval(
        &mut session,
        "document.querySelector('p').append(' needle')",
    );
    assert_eq!(find(&mut session, "status", "")["matchCount"], 2);
    assert!(eval(&mut session, "styleCalls").as_u64().unwrap() > before);
}

#[test]
fn shadow_and_iframe_mutations_refresh_cached_results() {
    let mut session = session_for(
        "<div id='host'></div><iframe srcdoc='<p>needle</p>'></iframe><script>document.getElementById('host').attachShadow({mode:'open'}).innerHTML='<p>needle</p>'</script>",
    );
    assert_eq!(find(&mut session, "start", "needle")["matchCount"], 2);
    eval(
        &mut session,
        "document.getElementById('host').shadowRoot.querySelector('p').append(' needle')",
    );
    assert_eq!(find(&mut session, "status", "")["matchCount"], 3);
    eval(
        &mut session,
        "document.querySelector('iframe').contentDocument.querySelector('p').append(' needle')",
    );
    assert_eq!(find(&mut session, "status", "")["matchCount"], 4);
}

#[test]
fn visibility_override_allows_visible_descendant_only() {
    let mut session = session_for(
        "<div style='visibility:hidden'>hiddenword<span>inheritedhidden</span><span style='visibility:visible'>visibleword</span></div>",
    );
    assert_eq!(find(&mut session, "start", "hiddenword")["matchCount"], 0);
    assert_eq!(
        find(&mut session, "start", "inheritedhidden")["matchCount"],
        0
    );
    assert_eq!(find(&mut session, "start", "visibleword")["matchCount"], 1);
}

#[test]
fn finding_text_in_offscreen_iframe_scrolls_the_outer_page() {
    let mut session =
        session_for("<div style='height:1500px'></div><iframe srcdoc='<p>frameword</p>'></iframe>");
    assert_eq!(find(&mut session, "start", "frameword")["matchCount"], 1);
    assert_eq!(
        eval(
            &mut session,
            "String(document.querySelector('iframe').contentDocument.getSelection())"
        ),
        "frameword"
    );
    assert!(eval(&mut session, "window.scrollY").as_f64().unwrap_or(0.0) > 0.0);
}

#[test]
fn matches_across_inline_text_nodes_but_not_block_boundaries() {
    let mut inline = session_for(include_str!("fixtures/find-in-page/inline.html"));
    assert_eq!(find(&mut inline, "start", "helloworld")["matchCount"], 1);
    assert_eq!(eval(&mut inline, "String(getSelection())"), "helloworld");
    let mut block = session_for(include_str!("fixtures/find-in-page/block.html"));
    assert_eq!(find(&mut block, "start", "helloworld")["matchCount"], 0);
}

#[test]
fn offscreen_auto_match_is_painted_after_find() {
    let mut session = session_for(
        "<style>body{margin:0}#auto{content-visibility:auto;contain-intrinsic-size:80px;color:#c00000;font:32px sans-serif}</style><div style='height:1400px'></div><p id='auto'>redword</p>",
    );
    let red_pixels = |pixels: &[u8]| {
        pixels
            .chunks_exact(4)
            .filter(|pixel| pixel[0] > 120 && pixel[1] < 80 && pixel[2] < 80)
            .count()
    };
    let before = render_browser_frame(&mut session, 320, 240, 0).unwrap();
    assert_eq!(red_pixels(before.pixels()), 0);
    assert_eq!(find(&mut session, "start", "redword")["matchCount"], 1);
    let after = render_browser_frame(&mut session, 320, 240, 16).unwrap();
    assert!(
        red_pixels(after.pixels()) > 5,
        "active auto match is not painted"
    );
}

#[test]
fn slotted_matches_follow_assignment_once_and_hidden_host_excludes_them() {
    let mut session = session_for(include_str!("fixtures/find-in-page/slot.html"));
    assert_eq!(find(&mut session, "start", "slotword")["matchCount"], 2);
    assert_eq!(
        eval(&mut session, "getSelection().anchorNode.parentElement.id"),
        "first"
    );
    find(&mut session, "next", "");
    assert_eq!(
        eval(&mut session, "getSelection().anchorNode.parentElement.id"),
        "second"
    );
    eval(
        &mut session,
        "document.getElementById('host').style.display='none'",
    );
    assert_eq!(find(&mut session, "status", "")["matchCount"], 0);
}
