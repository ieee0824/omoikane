use omoikane::{html::TreeBuilder, js::JsRuntime};

#[test]
fn color4_cssom_and_computed_values_match_firefox_reference() {
    let reference: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/css-color4/firefox-reference.json")).unwrap();
    let document = TreeBuilder::parse("<div id='target'></div>").document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    for record in reference["records"].as_array().unwrap() {
        let input = record["input"].as_str().unwrap();
        let specified = record["specified"].as_str().unwrap();
        // Firefox 157 resolves missing HWB components to zero. CSS Color 4 and
        // the pinned WPT preserve none; keep the observed reference untouched.
        let missing_hwb = input == "hwb(none none none / none)";
        let computed = record["computed"].as_str().unwrap();
        let script = format!(
            "(() => {{ const e = document.getElementById('target'); e.style.color = ''; \
             e.style.color = {}; return JSON.stringify([e.style.color, getComputedStyle(e).color]); }})()",
            serde_json::to_string(input).unwrap()
        );
        let actual = runtime
            .eval(&script)
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        let expected = if missing_hwb {
            serde_json::to_string(&[input, input]).unwrap()
        } else {
            serde_json::to_string(&[specified, computed]).unwrap()
        };
        assert_eq!(actual, expected, "{input}");
    }
}

#[test]
fn color4_is_valid_in_registered_custom_properties_and_stylesheet_cssom() {
    let document = TreeBuilder::parse(
        "<style>#target {color:lab(50% 20% 0);background-color:color(display-p3 1 0 0)}</style>\
         <div id='target'></div>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    assert!(runtime.eval(
        "(() => { CSS.registerProperty({name:'--tint',syntax:'<color>',inherits:false,initialValue:'oklab(50% 0 0)'}); \
         const e=document.getElementById('target'); e.style.setProperty('--tint','lch(50% 0 0)'); \
         const s=document.styleSheets[0].cssRules[0].style; \
         return getComputedStyle(e).getPropertyValue('--tint') === 'lch(50 0 0)' && s.color === 'lab(50 25 0)' && s.backgroundColor === 'color(display-p3 1 0 0)' && \
         CSS.supports('color','hwb(none 0 0 / none)') && CSS.supports('color','oklch(.5 .2 20)'); })()"
    ).unwrap().to_boolean());
}

fn color4_paint_cases() -> Vec<(String, String)> {
    let mut colors = vec![
        ("hwb(0 0% 0%)".to_owned(), "red".to_owned()),
        ("lab(100 0 0)".to_owned(), "white".to_owned()),
        ("lch(100 0 120)".to_owned(), "white".to_owned()),
        ("oklab(1 0 0)".to_owned(), "white".to_owned()),
        ("oklch(1 0 120)".to_owned(), "white".to_owned()),
        ("color(srgb 1 0 0)".to_owned(), "red".to_owned()),
    ];
    let reference: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/css-color4/gamut-reference.json")).unwrap();
    for record in reference["records"].as_array().unwrap() {
        let pixel = record
            .get("expected_canvas_pixel")
            .unwrap_or(&record["pixel"]);
        colors.push((
            record["input"].as_str().unwrap().to_owned(),
            format!("rgb({}, {}, {})", pixel[0], pixel[1], pixel[2]),
        ));
    }
    for color in [
        "hwb(0 0 0 / .5)",
        "lab(100 0 0 / .5)",
        "lch(100 0 120 / .5)",
        "oklab(1 0 0 / .5)",
        "oklch(1 0 120 / .5)",
        "color(srgb 1 0 0 / .5)",
    ] {
        let rgb = if color.starts_with("hwb") || color.starts_with("color") {
            "255, 0, 0"
        } else {
            "255, 255, 255"
        };
        colors.push((color.to_owned(), format!("rgba({rgb}, .5)")));
    }
    colors
}

fn color4_render_markup(color: &str) -> String {
    format!(
        "<style>body{{margin:0;background:#444}}div{{position:absolute;left:10px;top:10px;\
     width:50px;height:50px;background:{color};color:{color};border:3px solid {color};\
     box-shadow:8px 8px 0 {color}}}p{{position:absolute;left:80px;top:0;color:{color};\
     font-size:20px}}section{{position:absolute;left:10px;top:90px;width:100px;height:20px;\
     background:linear-gradient({color},{color})}}</style><div></div><p>Color</p><section></section>\
     <svg width='40' height='40' style='position:absolute;left:100px;top:120px'>\
     <rect x='5' y='5' width='20' height='20' fill='{color}' stroke='{color}' stroke-width='4'/></svg>"
    )
}

#[test]
fn color4_reaches_background_text_border_gradient_shadow_and_svg() {
    use omoikane::{layout::Rect, paint::render_document};
    let viewport = Rect {
        width: 200.0,
        height: 180.0,
        ..Rect::default()
    };
    for (color, equivalent) in color4_paint_cases() {
        let actual = render_document(
            &TreeBuilder::parse(&color4_render_markup(&color)).document(),
            viewport,
        )
        .unwrap();
        let expected = render_document(
            &TreeBuilder::parse(&color4_render_markup(&equivalent)).document(),
            viewport,
        )
        .unwrap();
        // A comparison cannot pass merely because both paths omit a shape.
        for (x, y, path) in [
            (20, 20, "background"),
            (10, 20, "border"),
            (20, 100, "gradient"),
            (70, 70, "shadow"),
            (110, 130, "SVG fill"),
            (104, 130, "SVG stroke"),
        ] {
            assert_ne!(
                expected.pixel(x, y),
                Some(omoikane::paint::Color::rgb(68, 68, 68)),
                "{color}: missing {path}"
            );
        }
        let text_pixels = (10..60)
            .flat_map(|y| (80..180).map(move |x| (x, y)))
            .filter_map(|(x, y)| expected.pixel(x, y))
            .collect::<Vec<_>>();
        let background = omoikane::paint::Color::rgb(68, 68, 68);
        assert!(
            text_pixels.iter().any(|pixel| *pixel != background)
                && text_pixels.iter().any(|pixel| *pixel == background),
            "{color}: text region must contain both glyphs and background"
        );
        let differences = actual
            .pixels()
            .iter()
            .zip(expected.pixels())
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        if !differences.is_empty() {
            let output = std::path::Path::new(".artifacts/issue840");
            std::fs::create_dir_all(output).unwrap();
            std::fs::write(
                output.join("render-failure-actual.png"),
                actual.encode_png(),
            )
            .unwrap();
            std::fs::write(
                output.join("render-failure-expected.png"),
                expected.encode_png(),
            )
            .unwrap();
        }
        assert!(
            differences.is_empty(),
            "{color}: {} differing channels, first {:?}",
            differences.len(),
            differences.first()
        );
    }
}

#[test]
fn color_components_resolve_relative_math_when_font_and_container_change() {
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse(
        "<style>#container{container-type:inline-size;width:1000px}#target{font-size:16px}</style>\
         <div id='container'><div id='target'></div></div>"
    ).document()).unwrap();
    let script = "(() => { const e=document.getElementById('target'); const c=document.getElementById('container'); \
         const value='color(srgb calc(0.5 + (sign(2cqw - 10px) * 0.1)) 0 0 / 0.5)'; \
         if (!CSS.supports('color', value)) return 'unsupported'; e.style.color=value; \
         const first=getComputedStyle(e).color; c.style.width='100px'; \
         return JSON.stringify([first,getComputedStyle(e).color]); })()";
    let actual = runtime
        .eval(script)
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(
        actual,
        "[\"color(srgb 0.6 0 0 / 0.5)\",\"color(srgb 0.4 0 0 / 0.5)\"]"
    );
}

#[test]
fn registered_color_components_follow_container_size_changes() {
    let mut runtime = JsRuntime::with_document(
        TreeBuilder::parse(
            "<style>#container{container-type:inline-size;width:1000px}</style>\
         <div id='container'><div id='target'></div></div>",
        )
        .document(),
    )
    .unwrap();
    let actual = runtime.eval(
        "(() => { CSS.registerProperty({name:'--tint',syntax:'<color>',inherits:false,initialValue:'red'}); \
         const e=document.getElementById('target'), c=document.getElementById('container'); \
         e.style.setProperty('--tint','color(srgb calc(0.5 + (sign(2cqw - 10px) * 0.1)) 0 0)'); \
         const first=getComputedStyle(e).getPropertyValue('--tint'); c.style.width='100px'; \
         return JSON.stringify([first,getComputedStyle(e).getPropertyValue('--tint')]); })()"
    ).unwrap().as_string().unwrap().to_std_string_escaped();
    assert_eq!(actual, "[\"color(srgb 0.6 0 0)\",\"color(srgb 0.4 0 0)\"]");
}
