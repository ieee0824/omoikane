use super::{CssToken, extract_font_face_rules, parse_stylesheet, tokenize};

#[test]
fn unquoted_urls_preserve_digit_runs_and_punctuation() {
    for url in [
        "data:font/ttf;base64,0012+00001/0987654321+01234567890==",
        "https://example.test/0012.png?version=0003#0004",
        "../0001.002.ttf",
    ] {
        assert_eq!(
            tokenize(&format!("url({url})")).unwrap(),
            [CssToken::Url(url.into())]
        );
        for input in [
            format!("url({url})"),
            format!("URL(  {url}  )"),
            format!("url(\"{url}\")"),
        ] {
            let sheet =
                parse_stylesheet(&format!("@font-face{{font-family:Audit;src:{input}}}")).unwrap();
            let faces = extract_font_face_rules(&sheet);
            assert_eq!(faces.len(), 1);
            assert_eq!(faces[0].src_url, url);
        }
    }
}

#[test]
fn unquoted_urls_keep_escaped_delimiters_in_their_argument() {
    assert_eq!(
        tokenize(r"url(a\)001.ttf)").unwrap(),
        [CssToken::Url(r"a\)001.ttf".into())]
    );
    assert_eq!(
        tokenize(r"url(a\20 b.ttf)").unwrap(),
        [CssToken::Url(r"a\20 b.ttf".into())]
    );
}
