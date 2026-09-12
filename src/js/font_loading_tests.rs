use super::*;
use crate::html::TreeBuilder;
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

fn serve_font(cors: bool) -> (String, std::thread::JoinHandle<String>) {
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
) -> (String, std::thread::JoinHandle<String>) {
    use std::io::{BufRead, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let worker = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(e) => panic!("font fixture was not requested: {e}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = String::new();
        let mut reader = std::io::BufReader::new(&mut stream);
        loop {
            let mut line = String::new();
            assert_ne!(reader.read_line(&mut line).unwrap(), 0);
            request.push_str(&line);
            if line == "\r\n" {
                break;
            }
        }
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
        let request = worker.join().unwrap();
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
            .unwrap()
            .starts_with("GET /initial/font.ttf HTTP/1.1\r\n")
    );
    assert_eq!(eval_json(&mut runtime, "relativeFace.status"), "loaded");
}

#[test]
fn font_src_blocks_redirect_before_contacting_the_destination() {
    let forbidden = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    forbidden.set_nonblocking(true).unwrap();
    let location = format!("http://{}/blocked.ttf", forbidden.local_addr().unwrap());
    let (origin, worker) =
        serve_font_response("302 Found", format!("Location: {location}\r\n"), Vec::new());
    let document = TreeBuilder::parse("<!doctype html><body></body>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, &format!("{origin}/index.html")).unwrap();
    runtime.install_csp_policy(&["font-src 'self'".into()]);
    runtime.eval("globalThis.redirectResult='pending';new FontFace('Redirect','url(redirect.ttf)').load().then(()=>redirectResult='loaded',e=>redirectResult=e.name);").unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        worker
            .join()
            .unwrap()
            .starts_with("GET /redirect.ttf HTTP/1.1\r\n")
    );
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
