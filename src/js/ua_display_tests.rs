use super::JsRuntime;
use crate::html::TreeBuilder;
use crate::paint::Color;

#[test]
fn ua_hidden_elements_follow_author_display_in_cssom_and_paint() {
    for tag in ["noframes", "noembed", "datalist", "rp"] {
        let html = format!(
            "<html><head><style>body {{margin:0;min-height:100vh;background:white}} \
             #probe {{width:40px;height:40px;background:red}}</style></head>\
             <body><{tag} id='probe'>hidden content</{tag}></body></html>"
        );
        let document = TreeBuilder::parse(&html).document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        runtime.set_viewport(80.0, 80.0);
        assert_eq!(
            runtime
                .eval("getComputedStyle(document.getElementById('probe')).display")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "none",
            "{tag}"
        );
        let hidden = runtime.paint_current_document().unwrap();
        assert!(
            hidden
                .pixels()
                .chunks_exact(4)
                .all(|pixel| pixel == [255, 255, 255, 255]),
            "{tag} must paint neither its box nor its contents"
        );
        runtime
            .eval("document.getElementById('probe').style.display = 'block'")
            .unwrap();
        assert_eq!(
            runtime
                .eval("getComputedStyle(document.getElementById('probe')).display")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "block",
            "{tag}"
        );
        assert_eq!(
            runtime.paint_current_document().unwrap().pixel(20, 20),
            Some(Color::rgb(255, 0, 0)),
            "{tag}"
        );
    }
}

#[test]
fn all_standard_ua_hidden_elements_allow_author_display_override() {
    for tag in [
        "area", "base", "basefont", "datalist", "head", "link", "meta", "noembed", "noframes",
        "param", "rp", "script", "style", "template", "title",
    ] {
        let document = TreeBuilder::parse("<html><head><style>body {margin:0;min-height:100vh;background:white} #probe {width:40px;height:40px;background:red}</style></head><body></body></html>").document();
        let probe = crate::dom::NodeHandle::element(tag);
        probe.set_attribute("id", "probe");
        document.query_selector("body").unwrap().append_child(probe);
        let mut runtime = JsRuntime::with_document(document).unwrap();
        runtime.set_viewport(80.0, 80.0);
        let display = runtime
            .eval("getComputedStyle(document.getElementById('probe')).display")
            .unwrap();
        assert_eq!(
            display.as_string().unwrap().to_std_string_escaped(),
            "none",
            "{tag}"
        );
        let hidden = runtime.paint_current_document().unwrap();
        assert!(
            hidden
                .pixels()
                .chunks_exact(4)
                .all(|pixel| pixel == [255, 255, 255, 255]),
            "{tag} must paint neither its box nor its contents"
        );
        runtime
            .eval("document.getElementById('probe').style.display = 'block'")
            .unwrap();
        assert_eq!(
            runtime.paint_current_document().unwrap().pixel(20, 20),
            Some(Color::rgb(255, 0, 0)),
            "{tag}"
        );
    }
}
