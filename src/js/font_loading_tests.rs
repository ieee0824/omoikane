use super::*;
use crate::html::TreeBuilder;
use crate::test_support::http_fixture::{
    ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
    read_request_headers,
};
use serde_json::{Value, json};

fn runtime() -> JsRuntime {
    let document = TreeBuilder::parse(
        "<!doctype html><style>span{font:24px/32px RuntimeFont,monospace}</style><span id='sample'>iiiiWW العربية</span>",
    )
    .document();
    JsRuntime::with_document(document).unwrap()
}

fn eval_json(runtime: &mut JsRuntime, script: &str) -> Value {
    let value = runtime.eval(&format!("JSON.stringify({script})")).unwrap();
    serde_json::from_str(&value.as_string().unwrap().to_std_string_escaped()).unwrap()
}

fn font_url() -> String {
    format!(
        "data:font/ttf;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(include_bytes!(
            "../../tests/fixtures/anonymized-arabic-fallback/DejaVuSans.ttf"
        ))
    )
}

#[test]
fn first_paint_defers_network_fonts_and_only_used_faces_are_requested() {
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, Ordering};

    let listener = bind_loopback().unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let requested = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&requested);
    let worker = FixtureWorker::spawn(move || {
        let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let request = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
        observed.store(true, Ordering::Release);
        let bytes =
            include_bytes!("../../tests/fixtures/anonymized-arabic-fallback/DejaVuSans.ttf");
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: font/ttf\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len()).unwrap();
        stream.write_all(bytes).unwrap();
        request
    });
    let document = TreeBuilder::parse(&format!(
        "<!doctype html><style>\
         @font-face{{font-family:UsedFace;src:url({origin}/used.ttf);unicode-range:U+0-7F}}\
         @font-face{{font-family:UsedFace;src:url({origin}/unused-range.ttf);unicode-range:U+600-6FF}}\
         @font-face{{font-family:UnusedFace;src:url({origin}/unused-family.ttf)}}\
         body{{font:20px UsedFace,monospace}}</style><body>AAAA</body>"
    )).document();
    let mut runtime = JsRuntime::with_document_and_url(document, &origin).unwrap();
    runtime.paint_current_document().unwrap();
    assert!(
        !requested.load(Ordering::Acquire),
        "first paint must not wait for font I/O"
    );
    assert_eq!(
        eval_json(&mut runtime, "Array.from(document.fonts,f=>f.status)"),
        json!(["unloaded", "unloaded", "unloaded"])
    );
    runtime.run_until_idle().unwrap();
    let request = worker.join();
    assert!(request.starts_with("GET /used.ttf "), "{request}");
    assert_eq!(
        eval_json(&mut runtime, "Array.from(document.fonts,f=>f.status)"),
        json!(["loaded", "unloaded", "unloaded"])
    );
}

#[test]
fn imported_font_family_preserves_quoting_for_numeric_name_components() {
    let css = "@font-face { font-family: 'M PLUS Rounded 1c'; src: url(missing.ttf); }";
    let source = format!(
        "data:text/css;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(css)
    );
    let document = TreeBuilder::parse(&format!(
        "<!doctype html><style>@import url('{source}');</style><body>serif text</body>"
    ))
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    assert_eq!(
        eval_json(
            &mut runtime,
            "Array.from(document.fonts,f=>[f.family,f.status])"
        ),
        json!([["\"M PLUS Rounded 1c\"", "unloaded"]])
    );
    runtime.run_until_idle().unwrap();
}

#[test]
fn ch_width_recomputes_when_its_web_font_finishes_loading() {
    let document = TreeBuilder::parse(
        "<!doctype html><style>#sample{font-family:RuntimeFont,monospace;font-size:20px;width:10ch}</style><div id=sample></div>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let before = eval_json(
        &mut runtime,
        "document.getElementById('sample').getBoundingClientRect().width",
    )
    .as_f64()
    .unwrap();
    runtime
        .eval(&format!(
            "globalThis.relativeFace=new FontFace('RuntimeFont',{});document.fonts.add(relativeFace);relativeFace.load();",
            serde_json::to_string(&format!("url({})", font_url())).unwrap()
        ))
        .unwrap();
    runtime.run_until_idle().unwrap();
    let after = eval_json(
        &mut runtime,
        "document.getElementById('sample').getBoundingClientRect().width",
    )
    .as_f64()
    .unwrap();
    let font = crate::font::Font::load_from_bytes(
        include_bytes!("../../tests/fixtures/anonymized-arabic-fallback/DejaVuSans.ttf").to_vec(),
    )
    .unwrap();
    let expected = font.css_relative_metrics(20.0, false, false).ch * 10.0;
    assert!(
        (after - expected as f64).abs() < 0.02,
        "before={before}, after={after}, expected={expected}"
    );
    assert!(
        (after - before).abs() > 0.1,
        "loading the font must invalidate the used ch width"
    );
}

#[test]
fn url_face_is_task_loaded_and_membership_controls_layout() {
    let mut runtime = runtime();
    runtime
        .eval(&format!(
            "globalThis.face = new FontFace('RuntimeFont', {});globalThis.events=[];globalThis.before=document.getElementById('sample').getBoundingClientRect().width;document.fonts.add(face);for(const type of ['loading','loadingdone','loadingerror'])document.fonts.addEventListener(type,e=>events.push([e.type,e.fontfaces ? e.fontfaces.length : null]));globalThis.loaded=face.load();globalThis.same=loaded===face.loaded;globalThis.ready=document.fonts.ready;globalThis.resolved=false;ready.then(s=>{{resolved=s===document.fonts}});",
            serde_json::to_string(&format!("url({})", font_url())).unwrap()
        ))
        .unwrap();
    assert_eq!(
        eval_json(
            &mut runtime,
            "[face.status,document.fonts.status,same,resolved,document.fonts.has(face)]"
        ),
        json!(["loading", "loading", true, false, true])
    );
    runtime.run_until_idle().unwrap();
    assert_eq!(
        eval_json(
            &mut runtime,
            "[face.status,document.fonts.status,resolved,events,document.fonts.check('24px RuntimeFont')]"
        ),
        json!([
            "loaded",
            "loaded",
            true,
            [["loading", null], ["loadingdone", 1]],
            true
        ])
    );
    let widths = eval_json(
        &mut runtime,
        "(()=>{const after=document.getElementById('sample').getBoundingClientRect().width;document.fonts.delete(face);const removed=document.getElementById('sample').getBoundingClientRect().width;return [before,after,removed,document.fonts.size]})()",
    );
    assert_ne!(
        widths[0], widths[1],
        "loaded font must change glyph advances"
    );
    assert_eq!(widths[0], widths[2], "removal must restore the fallback");
    assert_eq!(widths[3], 0);
}

#[test]
fn syntax_and_binary_failures_reject_without_throwing_the_constructor() {
    let mut runtime = runtime();
    runtime.eval("globalThis.errors=[];globalThis.badSource=new FontFace('Bad','naked.ttf');globalThis.badBytes=new FontFace('Bad',new Uint8Array([0,1,2,3]));badSource.loaded.catch(e=>errors.push(['source',e.name]));badBytes.loaded.catch(e=>errors.push(['bytes',e.name]));").unwrap();
    assert_eq!(
        eval_json(
            &mut runtime,
            "[badSource.status,badSource.family,badSource.style]"
        ),
        json!(["error", "", ""])
    );
    runtime.run_until_idle().unwrap();
    assert_eq!(
        eval_json(&mut runtime, "[badSource.status,badBytes.status,errors]"),
        json!([
            "error",
            "error",
            [["source", "SyntaxError"], ["bytes", "SyntaxError"]]
        ])
    );
}

#[test]
fn unloaded_faces_are_matched_by_font_and_unicode_range() {
    let mut runtime = runtime();
    runtime.eval("globalThis.latin=new FontFace('Sample','url(missing.ttf)',{unicodeRange:'U+0-7F'});globalThis.arabic=new FontFace('Sample','url(missing.ttf)',{unicodeRange:'U+600-6FF'});document.fonts.add(latin).add(arabic);globalThis.invalid='';try{document.fonts.check('invalid')}catch(e){invalid=e.name};").unwrap();
    assert_eq!(
        eval_json(
            &mut runtime,
            "[document.fonts.check('16px Sample','abc'),document.fonts.check('16px Sample','日本語'),document.fonts.check('16px Uninstalled'),invalid,Array.from(document.fonts).length]"
        ),
        json!([false, true, true, "SyntaxError", 2])
    );
}

#[test]
fn font_set_matching_applies_stretch_before_style_and_weight() {
    let mut runtime = runtime();
    runtime
        .eval(&format!(
            "globalThis.condensed=new FontFace('StretchSet',{},{{stretch:'75%',style:'normal',weight:'400'}});globalThis.normal=new FontFace('StretchSet','url(missing.ttf)',{{stretch:'100%',style:'italic',weight:'700'}});document.fonts.add(condensed).add(normal);globalThis.stretchResult=null;document.fonts.load('italic 700 condensed 16px StretchSet','A').then(value=>stretchResult=[value.length,value[0]===condensed],e=>stretchResult=e.name);",
            serde_json::to_string(&format!("url({})", font_url())).unwrap()
        ))
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        eval_json(
            &mut runtime,
            "[condensed.status,normal.status,stretchResult]"
        ),
        json!(["loaded", "unloaded", [1, true]])
    );
}

#[test]
fn font_set_matching_applies_oblique_angle_ranges() {
    let mut runtime = runtime();
    runtime
        .eval(&format!(
            "globalThis.negative=new FontFace('AngleSet',{},{{style:'oblique -20deg -5deg'}});globalThis.positive=new FontFace('AngleSet','url(missing.ttf)',{{style:'oblique 5deg 20deg'}});document.fonts.add(negative).add(positive);globalThis.angleResult=null;document.fonts.load('oblique -10deg 16px AngleSet','A').then(value=>angleResult=[value.length,value[0]===negative],e=>angleResult=e.name);",
            serde_json::to_string(&format!("url({})", font_url())).unwrap()
        ))
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        eval_json(
            &mut runtime,
            "[negative.status,positive.status,angleResult]"
        ),
        json!(["loaded", "unloaded", [1, true]])
    );
}

#[test]
fn css_connected_face_round_trips_range_descriptors() {
    let mut runtime = runtime();
    runtime
        .eval(&format!(
            "globalThis.rangeSheet=document.createElement('style');rangeSheet.textContent={};document.head.appendChild(rangeSheet);globalThis.rangeRule=rangeSheet.sheet.cssRules[0];globalThis.rangeFace=Array.from(document.fonts)[0];",
            serde_json::to_string(&format!(
                "@font-face{{font-family:RangeFace;src:url({});font-weight:300 700;font-style:oblique -10deg 20deg;font-stretch:75% 125%;unicode-range:U+0-7F, U+600-6FF}}",
                font_url()
            ))
            .unwrap()
        ))
        .unwrap();
    assert_eq!(
        eval_json(
            &mut runtime,
            "[rangeFace.weight,rangeFace.style,rangeFace.stretch,rangeFace.unicodeRange]"
        ),
        json!([
            "300 700",
            "oblique -10deg 20deg",
            "75% 125%",
            "U+0-7F, U+600-6FF"
        ])
    );
    runtime
        .eval("rangeFace.stretch='expanded';rangeFace.unicodeRange='U+41-42';")
        .unwrap();
    assert_eq!(
        eval_json(
            &mut runtime,
            "[rangeRule.style.getPropertyValue('font-stretch'),rangeRule.style.getPropertyValue('unicode-range'),Array.from(document.fonts)[0]===rangeFace]"
        ),
        json!(["expanded", "U+41-42", true])
    );
}

#[test]
fn css_unicode_range_faces_contribute_separate_text_runs() {
    let encode = |bytes: &[u8]| {
        format!(
            "data:font/ttf;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        )
    };
    let regular = encode(include_bytes!(
        "../../tests/fixtures/anonymized-font-selection/OmoikaneFixture-Regular.ttf"
    ));
    let bold = encode(include_bytes!(
        "../../tests/fixtures/anonymized-font-selection/OmoikaneFixture-Bold.ttf"
    ));
    let document = TreeBuilder::parse(&format!(
        "<!doctype html><style>\
         @font-face{{font-family:Composite;src:url({regular});unicode-range:U+41}}\
         @font-face{{font-family:Composite;src:url({bold});unicode-range:U+42}}\
         @font-face{{font-family:OnlyRegular;src:url({regular})}}\
         @font-face{{font-family:OnlyBold;src:url({bold})}}\
         span{{font-size:20px}}#both{{font-family:Composite}}\
         #a{{font-family:OnlyRegular}}#b{{font-family:OnlyBold}}\
         </style><span id=both>AB</span><span id=a>A</span><span id=b>B</span>"
    ))
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let widths = eval_json(
        &mut runtime,
        "['both','a','b'].map(id=>document.getElementById(id).getBoundingClientRect().width)",
    );
    let both = widths[0].as_f64().unwrap();
    let pieces = widths[1].as_f64().unwrap() + widths[2].as_f64().unwrap();
    assert!((both - pieces).abs() < 0.01, "{widths:?}");
}

#[test]
fn binary_source_is_copied_and_used_by_both_layout_and_paint() {
    let mut runtime = runtime();
    let bytes = include_bytes!("../../tests/fixtures/anonymized-arabic-fallback/DejaVuSans.ttf");
    let array = JsUint8Array::from_iter(bytes.iter().copied(), &mut runtime.context).unwrap();
    runtime
        .context
        .register_global_property(
            js_string!("fontBytes"),
            array,
            boa_engine::property::Attribute::all(),
        )
        .unwrap();
    runtime.eval("globalThis.binaryFace=new FontFace('RuntimeFont',new DataView(fontBytes.buffer));fontBytes.fill(0);document.fonts.add(binaryFace);globalThis.binaryLoaded=false;binaryFace.loaded.then(f=>binaryLoaded=f===binaryFace);").unwrap();
    assert_eq!(
        eval_json(&mut runtime, "[binaryFace.status,binaryLoaded]"),
        json!(["unloaded", false])
    );
    runtime.run_until_idle().unwrap();
    assert_eq!(
        eval_json(&mut runtime, "[binaryFace.status,binaryLoaded]"),
        json!(["loaded", true])
    );
    runtime.set_viewport(320.0, 80.0);
    let (canvas, records) =
        crate::font::with_font_selection_diagnostics(|| runtime.paint_current_document().unwrap());
    for phase in ["layout", "paint"] {
        assert!(
            records.iter().any(|r| r.phase == phase
                && r.source == "web"
                && r.requested_families
                    .iter()
                    .any(|family| family == "runtimefont")),
            "{phase}: {records:?}"
        );
    }
    assert!(
        canvas
            .pixels()
            .chunks_exact(4)
            .any(|pixel| pixel[..3] != [255, 255, 255])
    );
}

#[test]
fn font_policy_is_independent_of_connect_policy_and_errors_are_task_delivered() {
    for allowed in [false, true] {
        let policy = if allowed {
            "font-src data:; connect-src 'none'"
        } else {
            "font-src 'none'; connect-src data:"
        };
        let document = TreeBuilder::parse(&format!("<!doctype html><head><meta http-equiv='Content-Security-Policy' content=\"{policy}\"></head><body></body>")).document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/").unwrap();
        runtime.install_csp_policy(&[]);
        runtime.eval(&format!("globalThis.policyFace=new FontFace('PolicyFont',{});globalThis.outcome='pending';globalThis.fontEvents=[];document.fonts.add(policyFace);document.fonts.onloadingdone=e=>fontEvents.push([e.type,e.fontfaces.length]);document.fonts.onloadingerror=e=>fontEvents.push([e.type,e.fontfaces.length]);policyFace.load().then(()=>outcome='loaded',e=>outcome=e.name);",serde_json::to_string(&format!("url({})",font_url())).unwrap())).unwrap();
        assert_eq!(eval_json(&mut runtime, "outcome"), "pending");
        runtime.run_until_idle().unwrap();
        let observed = eval_json(
            &mut runtime,
            "[outcome,fontEvents,document.cspViolations.map(v=>v.effectiveDirective)]",
        );
        if allowed {
            assert_eq!(observed, json!(["loaded", [["loadingdone", 1]], []]));
        } else {
            assert_eq!(
                observed,
                json!([
                    "NetworkError",
                    [["loadingdone", 0], ["loadingerror", 1]],
                    ["font-src"]
                ])
            );
        }
    }
}

#[test]
fn delayed_font_violation_keeps_load_call_source_position() {
    let document = TreeBuilder::parse("<!doctype html><body></body>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "https://example.test/document").unwrap();
    runtime.install_csp_policy(&["font-src 'none'; report-to fonts".into()]);
    runtime.eval("globalThis.fontSourceEvents=[];globalThis.fontSourceReports=[];\
        document.addEventListener('securitypolicyviolation',event=>fontSourceEvents.push(event));\
        new ReportingObserver(batch=>fontSourceReports.push(...batch),{types:['csp-violation']}).observe();\
        globalThis.delayedFace=new FontFace('Delayed','url(https://example.test/font.ttf)');").unwrap();
    let (evaluated, ..) = runtime.eval_safe_timed_source(
        "const pending = delayedFace.load(); pending.catch(()=>{});",
        Some("https://private.test/effective.js"),
        Some("https://example.test/requested.js#fragment"),
        (23, 4),
        false,
    );
    evaluated.unwrap();
    let (unrelated, ..) = runtime.eval_safe_timed_source(
        "globalThis.unrelatedFontSourceCall=true;",
        Some("https://other.test/effective.js"),
        Some("https://other.test/requested.js"),
        (91, 1),
        false,
    );
    unrelated.unwrap();
    runtime.run_until_idle().unwrap();
    let observed = eval_json(
        &mut runtime,
        "[fontSourceEvents.map(e=>[e.sourceFile,e.lineNumber,e.columnNumber]),fontSourceReports.map(r=>[r.body.sourceFile,r.body.lineNumber,r.body.columnNumber])]",
    );
    assert_eq!(observed[0].as_array().map(Vec::len), Some(1), "{observed}");
    assert_eq!(observed[1], observed[0]);
    assert_eq!(observed[0][0][0], "https://example.test/requested.js");
    assert_eq!(observed[0][0][1], 23);
    assert!(observed[0][0][2].as_u64().is_some_and(|column| column > 0));
}

fn observe_font_csp(runtime: &mut JsRuntime) {
    runtime
        .eval(
            "globalThis.fontCspEvents=[];globalThis.fontCspReports=[];
             document.addEventListener('securitypolicyviolation',event=>fontCspEvents.push([
               event.effectiveDirective,event.disposition,event.blockedURI,event.originalPolicy]));
             new ReportingObserver(batch=>fontCspReports.push(...batch.map(report=>[
               report.body.effectiveDirective,report.body.disposition,
               report.body.blockedURL,report.body.originalPolicy]))).observe();",
        )
        .unwrap();
}

#[test]
fn report_only_font_policy_reports_data_request_and_response_without_blocking() {
    for (scheme, monitoring) in [
        ("data", "font-src 'none'; report-to fonts"),
        ("DATA", "font-src *; report-to fonts"),
    ] {
        let document = TreeBuilder::parse("<!doctype html><body></body>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "https://example.test/").unwrap();
        runtime.install_response_security_headers(&[
            (
                "Content-Security-Policy".into(),
                "font-src data:; connect-src 'none'".into(),
            ),
            (
                "Content-Security-Policy-Report-Only".into(),
                monitoring.into(),
            ),
        ]);
        observe_font_csp(&mut runtime);
        let source = format!("url({scheme}:{})", &font_url()[5..]);
        runtime
            .eval(&format!(
                "globalThis.observedFace=new FontFace('ObservedFont',{});
                 globalThis.fontOutcome='pending';document.fonts.add(observedFace);
                 observedFace.load().then(()=>fontOutcome='loaded',error=>fontOutcome=error.name);",
                serde_json::to_string(&source).unwrap()
            ))
            .unwrap();
        assert_eq!(
            eval_json(&mut runtime, "[fontOutcome,fontCspEvents,fontCspReports]"),
            json!(["pending", [], []])
        );
        runtime.run_until_idle().unwrap();
        let expected = json!(["font-src", "report", "data", monitoring]);
        assert_eq!(
            eval_json(
                &mut runtime,
                "[fontOutcome,observedFace.status,fontCspEvents,fontCspReports,document.cspViolations]"
            ),
            json!([
                "loaded",
                "loaded",
                [expected.clone(), expected.clone()],
                [expected.clone(), expected],
                []
            ]),
            "{scheme} with {monitoring}"
        );
        assert!(runtime.take_task_errors().is_empty());
    }
}

#[test]
fn stylesheet_faces_are_stable_and_cannot_be_removed_by_clear() {
    let mut runtime = runtime();
    runtime.eval(&format!("globalThis.sheet=document.createElement('style');sheet.textContent={};document.head.appendChild(sheet);globalThis.cssSet=document.fonts;globalThis.cssFace=Array.from(cssSet)[0];globalThis.cssDone=false;cssSet.ready.then(()=>cssDone=true);",serde_json::to_string(&format!("@font-face{{font-family:SheetFont;src:url({})}}",font_url())).unwrap())).unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        eval_json(
            &mut runtime,
            "[cssSet.size,cssFace.family,cssFace.status,cssDone,cssSet.delete(cssFace)]"
        ),
        json!([1, "SheetFont", "loaded", true, false])
    );
    runtime
        .eval("cssSet.clear();document.body.setAttribute('class','unrelated');")
        .unwrap();
    assert_eq!(
        eval_json(
            &mut runtime,
            "[cssSet.size,Array.from(cssSet)[0]===cssFace]"
        ),
        json!([1, true])
    );
    runtime.eval("sheet.remove()").unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        eval_json(&mut runtime, "[cssSet.size,cssFace.status]"),
        json!([0, "loaded"])
    );
}

#[test]
fn layered_css_faces_remain_enumerable_while_layout_uses_the_winner() {
    let encode = |bytes: &[u8]| {
        format!(
            "data:font/ttf;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        )
    };
    let lower = encode(include_bytes!(
        "../../tests/fixtures/anonymized-font-selection/OmoikaneFixture-Regular.ttf"
    ));
    let winner = encode(include_bytes!(
        "../../tests/fixtures/anonymized-font-selection/OmoikaneFixture-Italic.ttf"
    ));
    let face = |url: &str| format!("@font-face{{font-family:LayerSet;src:url({url})}}");
    let lower_face = face(&lower);
    let winner_face = face(&winner);
    let measure = |rules: &str| {
        let document = TreeBuilder::parse(&format!(
            "<!doctype html><style>{rules}#sample{{font:20px LayerSet}}</style>\
             <span id=sample>ABBA</span>"
        ))
        .document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        let result = eval_json(
            &mut runtime,
            "[document.getElementById('sample').getBoundingClientRect().width,\
              document.fonts.size,Array.from(document.fonts).length]",
        );
        (result[0].clone(), result[1].clone(), result[2].clone())
    };

    let layered = measure(&format!(
        "@layer base,override;\
         @layer override{{{winner_face}}}\
         @layer base{{{lower_face}}}"
    ));
    let winner_only = measure(&winner_face);
    let lower_only = measure(&lower_face);

    assert_eq!((layered.1, layered.2), (json!(2), json!(2)));
    assert_eq!(
        layered.0, winner_only.0,
        "the higher-precedence face must be the only registry entry used by layout"
    );
    assert_ne!(
        layered.0, lower_only.0,
        "the fixture faces must have distinct advances so the winner assertion is meaningful"
    );
}

#[test]
fn shadow_font_faces_are_scoped_and_nested_shadow_lookup_uses_the_parent_scope() {
    let encode = |bytes: &[u8]| {
        format!(
            "data:font/ttf;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        )
    };
    let document_face = encode(include_bytes!(
        "../../tests/fixtures/anonymized-font-selection/OmoikaneFixture-Regular.ttf"
    ));
    let shadow_face = encode(include_bytes!(
        "../../tests/fixtures/anonymized-font-selection/OmoikaneFixture-Italic.ttf"
    ));
    let document = TreeBuilder::parse(&format!(
        "<!doctype html><style>\
         @font-face{{font-family:ScopeFace;src:url({document_face})}}\
         #document-target{{font:20px ScopeFace}}\
         </style><span id=document-target>ABBA</span>"
    ))
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let result = eval_json(
        &mut runtime,
        &format!(
            "(()=>{{\
               const outerHost=document.createElement('x-outer');\
               document.body.appendChild(outerHost);\
               const outer=outerHost.attachShadow({{mode:'open'}});\
               outer.innerHTML=`<style>\
                 @font-face{{font-family:ScopeFace;src:url({shadow_face})}}\
                 #outer-target,x-inner{{font:20px ScopeFace}}\
                 </style><span id=outer-target>ABBA</span><x-inner></x-inner>`;\
               const innerHost=outer.querySelector('x-inner');\
               const inner=innerHost.attachShadow({{mode:'open'}});\
               inner.innerHTML='<style>#inner-target{{font:20px ScopeFace}}</style>' +\
                 '<span id=inner-target>ABBA</span>';\
               const width=id=>id.getBoundingClientRect().width;\
               return [width(document.getElementById('document-target')),\
                       width(outer.querySelector('#outer-target')),\
                       width(inner.querySelector('#inner-target')),\
                       document.fonts.size];\
             }})()"
        ),
    );

    assert_eq!(
        result[1], result[2],
        "nested lookup must use the outer face"
    );
    assert_ne!(
        result[0], result[1],
        "document and shadow scopes must keep distinct same-name faces"
    );
    assert_eq!(result[3], 2);
}

#[test]
fn font_set_load_starts_in_a_task_and_only_loads_the_matching_weight() {
    let mut runtime = runtime();
    runtime.eval(&format!("globalThis.regular=new FontFace('Variants',{},{{weight:'400'}});globalThis.bold=new FontFace('Variants','url(missing.ttf)',{{weight:'700'}});document.fonts.add(regular).add(bold);globalThis.variantResult=null;document.fonts.load('16px Variants','x').then(value=>variantResult=[value.length,value[0]===regular],e=>variantResult=e.name);",serde_json::to_string(&format!("url({})",font_url())).unwrap())).unwrap();
    assert_eq!(
        eval_json(&mut runtime, "[regular.status,bold.status,variantResult]"),
        json!(["unloaded", "unloaded", null])
    );
    runtime.run_until_idle().unwrap();
    assert_eq!(
        eval_json(
            &mut runtime,
            "[regular.status,bold.status,variantResult,document.fonts.check('16px Variants'),document.fonts.check('bold 16px Variants')]"
        ),
        json!(["loaded", "unloaded", [1, true], true, false])
    );
}

#[test]
fn a_registered_font_used_by_the_document_loads_without_an_explicit_load_call() {
    let mut runtime = runtime();
    runtime.eval(&format!("globalThis.autoFace=new FontFace('RuntimeFont',{});document.fonts.add(autoFace);document.getElementById('sample').getBoundingClientRect();",serde_json::to_string(&format!("url({})",font_url())).unwrap())).unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(eval_json(&mut runtime, "autoFace.status"), "loaded");
}

#[test]
fn internal_font_notifications_preserve_author_mutation_records() {
    let mut runtime = runtime();
    runtime.run_until_idle().unwrap();
    runtime
        .eval(
            r#"
            globalThis.takeRecordsCalls = 0;
            globalThis.authorRecords = [];
            const originalTakeRecords = MutationObserver.prototype.takeRecords;
            MutationObserver.prototype.takeRecords = function() {
                takeRecordsCalls++;
                return originalTakeRecords.call(this);
            };
            const authorObserver = new MutationObserver(records => authorRecords.push(...records));
            authorObserver.observe(document.body, {
                subtree: true, childList: true, characterData: true,
                characterDataOldValue: true, attributes: true, attributeOldValue: true
            });
            for (let i = 0; i < 16; i++) {
                const parent = document.createElement('a');
                const text = document.createTextNode('before');
                parent.appendChild(text);
                document.body.appendChild(parent);
                parent.setAttribute('class', 'probe');
                text.data = 'after';
                document.body.removeChild(parent);
            }
            "#,
        )
        .unwrap();
    assert_eq!(eval_json(&mut runtime, "authorRecords.length"), 0);
    runtime.run_until_idle().unwrap();
    assert_eq!(
        eval_json(
            &mut runtime,
            "[takeRecordsCalls,authorRecords.length,\
             authorRecords.filter(record=>record.type==='childList').length,\
             authorRecords.filter(record=>record.type==='attributes'&&record.oldValue===null).length,\
             authorRecords.filter(record=>record.type==='characterData'&&record.oldValue==='before').length]"
        ),
        json!([1, 64, 32, 16, 16])
    );
}

#[test]
fn text_mutation_requests_fonts_without_using_public_observer_methods() {
    let mut runtime = runtime();
    runtime
        .eval(&format!(
            "document.getElementById('sample').textContent='';\
             globalThis.mutationFace=new FontFace('RuntimeFont',{});\
             document.fonts.add(mutationFace);",
            serde_json::to_string(&format!("url({})", font_url())).unwrap()
        ))
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(eval_json(&mut runtime, "mutationFace.status"), "unloaded");
    runtime
        .eval(
            "MutationObserver.prototype.takeRecords=function(){throw new Error('public observer method')};\
             document.getElementById('sample').appendChild(document.createTextNode('A'));",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(eval_json(&mut runtime, "mutationFace.status"), "loaded");
}

#[test]
fn unrelated_child_list_keeps_css_faces_but_still_syncs_style_child_lists() {
    let mut runtime = runtime();
    runtime
        .eval(&format!(
            "globalThis.sheet=document.createElement('style');\
             sheet.textContent='@font-face{{font-family:MutationFont;src:url({})}}';\
             document.head.appendChild(sheet);\
             globalThis.face=Array.from(document.fonts)[0];",
            font_url()
        ))
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(eval_json(&mut runtime, "face.status"), "loaded");
    runtime
        .eval("document.getElementById('sample').appendChild(document.createElement('div'));")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        eval_json(
            &mut runtime,
            "[document.fonts.size,Array.from(document.fonts)[0]===face,face.status]"
        ),
        json!([1, true, "loaded"])
    );
    runtime
        .eval(&format!(
            "globalThis.addedSheet=document.createElement('style');\
             addedSheet.textContent='@font-face{{font-family:AddedFont;src:url({})}}';\
             document.head.appendChild(addedSheet);",
            font_url()
        ))
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        eval_json(
            &mut runtime,
            "[document.fonts.size,Array.from(document.fonts).map(face=>face.family)]"
        ),
        json!([2, ["MutationFont", "AddedFont"]])
    );
}

#[test]
fn automatic_font_loading_uses_the_computed_stretch() {
    let document = TreeBuilder::parse(
        "<!doctype html><style>#sample{font-family:AutoStretch;font-stretch:75%;font-size:20px}</style><span id=sample>A</span>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime
        .eval(&format!(
            "globalThis.condensedAuto=new FontFace('AutoStretch',{},{{stretch:'75%'}});globalThis.normalAuto=new FontFace('AutoStretch','url(missing.ttf)',{{stretch:'100%'}});document.fonts.add(condensedAuto).add(normalAuto);document.getElementById('sample').getBoundingClientRect();",
            serde_json::to_string(&format!("url({})", font_url())).unwrap()
        ))
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        eval_json(&mut runtime, "[condensedAuto.status,normalAuto.status]"),
        json!(["loaded", "unloaded"])
    );
}

#[test]
fn same_origin_child_can_register_a_parent_face_without_leaking_membership() {
    let document = TreeBuilder::parse(
        "<!doctype html><body><iframe id='frame' srcdoc='<p>Child</p>'></iframe></body>",
    )
    .document();
    let mut runtime = JsRuntime::with_document_and_url(document, "https://example.test/").unwrap();
    runtime.eval(&format!("globalThis.sharedFace=new FontFace('Shared',{});globalThis.child=document.getElementById('frame').contentWindow;child.document.fonts.add(sharedFace);globalThis.sharedResult=false;sharedFace.load().then(()=>sharedResult=true);",serde_json::to_string(&format!("url({})",font_url())).unwrap())).unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        eval_json(
            &mut runtime,
            "[sharedResult,child.document.fonts.has(sharedFace),document.fonts.has(sharedFace),child.document.fonts===document.getElementById('frame').contentDocument.fonts,child.document.fonts.size,document.fonts.size]"
        ),
        json!([true, true, false, true, 1, 0])
    );
    assert!(runtime.take_task_errors().is_empty());
}

fn serve_font(cors: bool) -> (String, FixtureWorker<String>) {
    serve_font_response(
        "200 OK",
        if cors {
            "Access-Control-Allow-Origin: *\r\n".into()
        } else {
            String::new()
        },
        include_bytes!("../../tests/fixtures/anonymized-arabic-fallback/DejaVuSans.ttf").to_vec(),
    )
}

fn serve_font_response(
    status: &'static str,
    headers: String,
    bytes: Vec<u8>,
) -> (String, FixtureWorker<String>) {
    use std::io::Write;
    let listener = bind_loopback().unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let worker = FixtureWorker::spawn(move || {
        let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let request = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
        write!(stream,"HTTP/1.1 {status}\r\nContent-Type: font/ttf\r\nContent-Length: {}\r\n{}Connection: close\r\n\r\n",bytes.len(),headers).unwrap();
        stream.write_all(&bytes).unwrap();
        request
    });
    (origin, worker)
}

#[test]
fn font_requests_enforce_cors_and_send_the_creation_documents_origin() {
    for allowed in [false, true] {
        let (origin, worker) = serve_font(allowed);
        let document = TreeBuilder::parse("<!doctype html><body></body>").document();
        let mut runtime =
            JsRuntime::with_document_and_url(document, "http://127.0.0.1:1/").unwrap();
        runtime.eval(&format!("globalThis.corsResult='pending';new FontFace('Cors',{}).load().then(()=>corsResult='loaded',e=>corsResult=e.name);",serde_json::to_string(&format!("url({origin}/font.ttf)")).unwrap())).unwrap();
        runtime.run_until_idle().unwrap();
        let request = worker.join();
        assert!(
            request
                .to_ascii_lowercase()
                .contains("origin: http://127.0.0.1:1\r\n"),
            "{request}"
        );
        assert_eq!(
            eval_json(&mut runtime, "corsResult"),
            if allowed { "loaded" } else { "NetworkError" }
        );
    }
}

#[test]
fn sandboxed_child_font_request_uses_opaque_origin() {
    let (origin, worker) = serve_font(true);
    let mut runtime = JsRuntime::with_document_and_url(
        TreeBuilder::parse("<!doctype html><body></body>").document(),
        "http://127.0.0.1:1/",
    )
    .unwrap();
    let source =
        format!("<script>new FontFace('Opaque', 'url({origin}/font.ttf)').load()</script>");
    runtime
        .eval(&format!(
            "const frame = document.createElement('iframe'); frame.setAttribute('sandbox', 'allow-scripts'); frame.srcdoc = {}; document.body.appendChild(frame);",
            serde_json::to_string(&source).unwrap()
        ))
        .unwrap();
    runtime.run_until_idle().unwrap();
    let request = worker.join();
    assert!(
        request.to_ascii_lowercase().contains("origin: null\r\n"),
        "{request}"
    );
    assert!(runtime.take_task_errors().is_empty());
}

#[test]
fn font_url_keeps_the_base_used_at_construction() {
    let (origin, worker) = serve_font(false);
    let document = TreeBuilder::parse(
        "<!doctype html><head><base id='base' href='/initial/'></head><body></body>",
    )
    .document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, &format!("{origin}/index.html")).unwrap();
    runtime.eval("globalThis.relativeFace=new FontFace('Relative','url(font.ttf)');document.getElementById('base').setAttribute('href','/changed/');relativeFace.load();").unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        worker
            .join()
            .starts_with("GET /initial/font.ttf HTTP/1.1\r\n")
    );
    assert_eq!(eval_json(&mut runtime, "relativeFace.status"), "loaded");
}

#[test]
fn font_src_blocks_redirect_before_contacting_the_destination() {
    let forbidden = bind_loopback().unwrap();
    let location = format!("http://{}/blocked.ttf", forbidden.local_addr().unwrap());
    let (origin, worker) =
        serve_font_response("302 Found", format!("Location: {location}\r\n"), Vec::new());
    let document = TreeBuilder::parse("<!doctype html><body></body>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, &format!("{origin}/index.html")).unwrap();
    runtime.install_csp_policy(&["font-src 'self'".into()]);
    runtime.eval("globalThis.redirectResult='pending';new FontFace('Redirect','url(redirect.ttf)').load().then(()=>redirectResult='loaded',e=>redirectResult=e.name);").unwrap();
    runtime.run_until_idle().unwrap();
    assert!(worker.join().starts_with("GET /redirect.ttf HTTP/1.1\r\n"));
    assert_eq!(
        forbidden.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert_eq!(
        eval_json(
            &mut runtime,
            "[redirectResult,document.cspViolations.map(v=>v.effectiveDirective)]"
        ),
        json!(["NetworkError", ["font-src"]])
    );
}

#[test]
fn report_only_font_redirects_reach_transport_and_report_the_initial_url() {
    let (final_origin, final_worker) = serve_font(true);
    let (middle_origin, middle_worker) = serve_font_response(
        "302 Found",
        format!("Location: {final_origin}/final.ttf\r\nAccess-Control-Allow-Origin: *\r\n"),
        Vec::new(),
    );
    let (origin, worker) = serve_font_response(
        "302 Found",
        format!("Location: {middle_origin}/middle.ttf\r\n"),
        Vec::new(),
    );
    let document = TreeBuilder::parse("<!doctype html><body></body>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, &format!("{origin}/index.html")).unwrap();
    runtime.install_response_security_headers(&[
        (
            "Content-Security-Policy".into(),
            "font-src *; connect-src 'none'".into(),
        ),
        (
            "Content-Security-Policy-Report-Only".into(),
            "font-src 'self'; report-to fonts".into(),
        ),
    ]);
    observe_font_csp(&mut runtime);
    runtime.eval("globalThis.fontOutcome='pending';new FontFace('Redirect','url(start.ttf)').load().then(()=>fontOutcome='loaded',error=>fontOutcome=error.name);").unwrap();
    runtime.run_until_idle().unwrap();
    assert!(worker.join().starts_with("GET /start.ttf HTTP/1.1\r\n"));
    assert!(
        middle_worker
            .join()
            .starts_with("GET /middle.ttf HTTP/1.1\r\n")
    );
    assert!(
        final_worker
            .join()
            .starts_with("GET /final.ttf HTTP/1.1\r\n")
    );
    let expected = json!([
        "font-src",
        "report",
        format!("{origin}/start.ttf"),
        "font-src 'self'; report-to fonts"
    ]);
    assert_eq!(
        eval_json(
            &mut runtime,
            "[fontOutcome,fontCspEvents,fontCspReports,document.cspViolations]"
        ),
        json!([
            "loaded",
            [expected.clone(), expected.clone(), expected.clone()],
            [expected.clone(), expected.clone(), expected],
            []
        ])
    );
    assert!(runtime.take_task_errors().is_empty());
}

#[test]
fn font_redirects_ignore_path_after_the_first_request_for_both_policy_dispositions() {
    use std::io::Write;
    let listener = bind_loopback().unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let worker = FixtureWorker::spawn(move || {
        let font = include_bytes!(
            "../../tests/fixtures/anonymized-font-selection/OmoikaneFixture-Regular.ttf"
        );
        let mut requests = Vec::new();
        for (status, headers, body) in [
            ("302 Found", "Location: /moved.ttf\r\n", &[][..]),
            ("302 Found", "Location: /final.ttf\r\n", &[][..]),
            ("200 OK", "", font.as_slice()),
        ] {
            let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            requests.push(read_request_headers(&mut stream, READ_TIMEOUT).unwrap());
            write!(stream,"HTTP/1.1 {status}\r\nContent-Type: font/ttf\r\nContent-Length: {}\r\n{headers}Connection: close\r\n\r\n",body.len()).unwrap();
            stream.write_all(body).unwrap();
        }
        requests
    });
    let document = TreeBuilder::parse("<!doctype html><body></body>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, &format!("{origin}/index.html")).unwrap();
    let policy = format!("font-src {origin}/allowed/; report-to fonts");
    runtime.install_response_security_headers(&[
        ("Content-Security-Policy".into(), policy.clone()),
        ("Content-Security-Policy-Report-Only".into(), policy),
    ]);
    observe_font_csp(&mut runtime);
    runtime.eval("globalThis.fontOutcome='pending';new FontFace('Path','url(allowed/start.ttf)').load().then(()=>fontOutcome='loaded',error=>fontOutcome=error.name);").unwrap();
    runtime.run_until_idle().unwrap();
    let requests = worker.join();
    for (request, path) in requests
        .iter()
        .zip(["/allowed/start.ttf", "/moved.ttf", "/final.ttf"])
    {
        assert!(
            request.starts_with(&format!("GET {path} HTTP/1.1\r\n")),
            "{request}"
        );
    }
    assert_eq!(
        eval_json(
            &mut runtime,
            "[fontOutcome,fontCspEvents,fontCspReports,document.cspViolations]"
        ),
        json!(["loaded", [], [], []])
    );
    assert!(runtime.take_task_errors().is_empty());
}

#[test]
fn cssom_font_face_preserves_data_urls_and_live_descriptors() {
    let mut runtime = runtime();
    let small_font = format!(
        "data:font/ttf;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(include_bytes!(
            "../../tests/fixtures/anonymized-font-selection/OmoikaneFixture-Regular.ttf"
        ))
    );
    runtime.eval(&format!(
        "globalThis.fontSource={};globalThis.fontSheet=document.createElement('style');fontSheet.textContent='@font-face{{font-family:LiveFace;src:'+fontSource+';font-weight:400}}';document.head.appendChild(fontSheet);globalThis.fontRule=fontSheet.sheet.cssRules[0];",
        serde_json::to_string(&format!("url({small_font})")).unwrap()
    )).unwrap();
    assert_eq!(
        eval_json(
            &mut runtime,
            "fontRule.style.getPropertyValue('src').includes(fontSource.slice(4,-1))"
        ),
        true,
        "CSSOM must preserve the semicolon and bytes inside a data URL"
    );
    runtime.eval("globalThis.liveFace=Array.from(document.fonts)[0];fontRule.style.setProperty('font-weight','700');").unwrap();
    assert_eq!(
        eval_json(
            &mut runtime,
            "[Array.from(document.fonts)[0]===liveFace,liveFace.weight,fontSheet.sheet.cssRules[0].style.getPropertyValue('font-weight')]"
        ),
        json!([true, "700", "700"])
    );
    runtime.eval("liveFace.family='ChangedFace'").unwrap();
    assert_eq!(
        eval_json(
            &mut runtime,
            "[Array.from(document.fonts)[0]===liveFace,fontSheet.sheet.cssRules[0].style.getPropertyValue('font-family')]"
        ),
        json!([true, "ChangedFace"])
    );
}

#[test]
fn cssom_can_enumerate_a_large_font_rule_without_a_js_character_loop() {
    let mut runtime = runtime();
    runtime.eval("globalThis.largeSheet=document.createElement('style');largeSheet.textContent='@font-face{font-family:Large;src:url(data:font/ttf;base64,'+'A'.repeat(1000100)+')}';document.head.appendChild(largeSheet);").unwrap();
    assert_eq!(
        eval_json(&mut runtime, "largeSheet.sheet.cssRules.length"),
        1
    );
}

#[test]
fn cssom_font_src_change_and_rule_removal_disconnect_faces() {
    let mut runtime = runtime();
    let source = |path| {
        format!(
            "url(data:font/ttf;base64,{})",
            base64::engine::general_purpose::STANDARD.encode(std::fs::read(path).unwrap())
        )
    };
    let regular = source("tests/fixtures/anonymized-font-selection/OmoikaneFixture-Regular.ttf");
    let bold = source("tests/fixtures/anonymized-font-selection/OmoikaneFixture-Bold.ttf");
    runtime
        .eval(&format!(
            "globalThis.sources=[{},{}];globalThis.switchSheet=document.createElement('style');switchSheet.textContent='@font-face{{font-family:SwitchFace;src:'+sources[0]+'}}';document.head.appendChild(switchSheet);globalThis.switchRule=switchSheet.sheet.cssRules[0];globalThis.firstFace=Array.from(document.fonts)[0];switchRule.style.setProperty('src',sources[1]);globalThis.secondFace=Array.from(document.fonts)[0];",
            serde_json::to_string(&regular).unwrap(),
            serde_json::to_string(&bold).unwrap(),
        ))
        .unwrap();
    assert_eq!(
        eval_json(
            &mut runtime,
            "[switchSheet.sheet.cssRules[0]===switchRule,secondFace!==firstFace,document.fonts.has(firstFace),document.fonts.has(secondFace)]"
        ),
        json!([true, true, false, true])
    );
    runtime.eval("switchSheet.sheet.deleteRule(0)").unwrap();
    assert_eq!(
        eval_json(
            &mut runtime,
            "[document.fonts.size,document.fonts.has(secondFace),document.fonts.add(secondFace)===document.fonts,document.fonts.has(secondFace)]"
        ),
        json!([0, false, true, true]),
        "a removed rule disconnects its face, which may then be added explicitly"
    );
}

#[test]
fn removed_style_updates_font_set_synchronously() {
    let mut runtime = runtime();
    let font = format!(
        "data:font/ttf;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(include_bytes!(
            "../../tests/fixtures/anonymized-font-selection/OmoikaneFixture-Regular.ttf"
        ))
    );
    runtime
        .eval(&format!(
            "globalThis.removedStyle=document.createElement('style');removedStyle.textContent='@font-face{{font-family:RemovedFace;src:url({font})}}';document.head.appendChild(removedStyle);globalThis.removedSet=document.fonts;globalThis.removedFace=Array.from(removedSet)[0];removedStyle.remove();"
        ))
        .unwrap();
    assert_eq!(
        eval_json(
            &mut runtime,
            "[removedSet.has(removedFace),removedSet.size,removedFace.status]"
        ),
        json!([false, 0, "loading"])
    );
}

#[test]
fn invalid_font_promise_is_exposed_lazily_and_keeps_its_rejection() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"
        globalThis.rejections = [];
        addEventListener('unhandledrejection', event => {
            rejections.push(event.reason.name);
            event.preventDefault();
        });
        globalThis.invalidFace = new FontFace('Bad', 'src');
        globalThis.invalidBinary = new FontFace('Bad', new Uint8Array([0,1,2,3]));
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        eval_json(
            &mut runtime,
            "[invalidFace.status,invalidBinary.status,rejections]"
        ),
        json!(["error", "error", []])
    );
    runtime
        .eval(
            r#"
        globalThis.exposed = invalidFace.loaded;
        globalThis.identity = exposed === invalidFace.loaded && exposed === invalidFace.load();
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        eval_json(&mut runtime, "[identity,rejections]"),
        json!([true, ["SyntaxError"]])
    );
    runtime
        .eval("globalThis.observed = ''; exposed.catch(error => observed = error.name);")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(eval_json(&mut runtime, "observed"), json!("SyntaxError"));
}

#[test]
fn class_change_loads_a_newly_used_css_font_without_explicit_font_api_calls() {
    let (origin, worker) = serve_font(true);
    let document = TreeBuilder::parse(&format!(
        "<!doctype html><style>@font-face{{font-family:LaterFont;src:url({origin}/later.ttf)}}\
         body.with-font{{font-family:LaterFont}}</style><body>AAAA</body>"
    ))
    .document();
    let mut runtime = JsRuntime::with_document_and_url(document, &origin).unwrap();
    runtime.set_nonblocking_font_loading(true);
    runtime.paint_current_document().unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        eval_json(&mut runtime, "Array.from(document.fonts,f=>f.status)"),
        json!(["unloaded"])
    );
    runtime.eval("document.body.className='with-font'").unwrap();
    runtime.run_until_idle().unwrap();
    let request = worker.join();
    assert!(request.starts_with("GET /later.ttf "), "{request}");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        runtime.run_until_idle().unwrap();
        if !runtime.host_state.borrow().font_loading.has_pending() {
            break;
        }
        assert!(Instant::now() < deadline, "newly used font did not settle");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        eval_json(&mut runtime, "Array.from(document.fonts,f=>f.status)"),
        json!(["loaded"])
    );
}
