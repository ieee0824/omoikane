//! Surface contracts and an opt-in shape oracle for bootstrap separation.
//!
//! The initial oracle uses the current full bootstrap plus each installer. It
//! does not replace factory, fetch-policy, lifetime, or credential regressions.
use super::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const SNAPSHOT: &str = include_str!("bootstrap_surface_snapshot.js");
const COMMON_APIS: &[&str] = &[
    "Event",
    "EventTarget",
    "MessageEvent",
    "AbortController",
    "AbortSignal",
    "URL",
    "URLSearchParams",
    "Blob",
    "Headers",
    "Request",
    "Response",
    "TextEncoder",
    "TextDecoder",
    "ReadableStream",
    "Promise",
    "fetch",
    "structuredClone",
    "queueMicrotask",
    "setTimeout",
    "clearTimeout",
];
const DOM_APIS: &[&str] = &[
    "Highlight",
    "HighlightRegistry",
    "XPathEvaluator",
    "XPathExpression",
    "XPathResult",
    "__omoikane_find_in_page",
    "CDATASection",
    "Comment",
    "DOMParser",
    "Document",
    "DocumentFragment",
    "DocumentType",
    "Element",
    "HTMLAnchorElement",
    "HTMLAreaElement",
    "HTMLDataListElement",
    "HTMLDetailsElement",
    "HTMLDialogElement",
    "HTMLElement",
    "HTMLEmbedElement",
    "HTMLFieldSetElement",
    "HTMLHeadElement",
    "HTMLHtmlElement",
    "HTMLLabelElement",
    "HTMLLinkElement",
    "HTMLMapElement",
    "HTMLMetaElement",
    "HTMLMeterElement",
    "HTMLObjectElement",
    "HTMLOptGroupElement",
    "HTMLOutputElement",
    "HTMLParagraphElement",
    "HTMLPictureElement",
    "HTMLProgressElement",
    "HTMLSelectedContentElement",
    "HTMLSlotElement",
    "HTMLSourceElement",
    "HTMLStyleElement",
    "HTMLTableCellElement",
    "HTMLTableElement",
    "HTMLTableRowElement",
    "HTMLTableSectionElement",
    "HTMLTrackElement",
    "HTMLUnknownElement",
    "Image",
    "MathMLAnchorElement",
    "MathMLElement",
    "Node",
    "NodeIterator",
    "Option",
    "ProcessingInstruction",
    "SVGCircleElement",
    "SVGEllipseElement",
    "SVGGeometryElement",
    "SVGGraphicsElement",
    "SVGImageElement",
    "SVGLineElement",
    "SVGPathElement",
    "SVGPolygonElement",
    "SVGPolylineElement",
    "SVGRectElement",
    "SVGTSpanElement",
    "SVGTextContentElement",
    "SVGTextElement",
    "SVGTextPathElement",
    "SVGTextPositioningElement",
    "Selection",
    "ShadowRoot",
    "Text",
    "TreeWalker",
    "XMLSerializer",
    "document",
    "getSelection",
    "window",
];

fn runtime_for_surface(installer: Option<&str>) -> JsRuntime {
    let document = blank_html_document();
    let url = "https://bootstrap.example.test/page";
    let mut runtime = if let Some(installer) = installer {
        let kind = match installer {
            "__omoikane_install_worker_global" => BootstrapKind::DedicatedWorker,
            "__omoikane_install_shared_worker_global" => BootstrapKind::SharedWorker,
            "__omoikane_install_worklet_global" => BootstrapKind::Worklet,
            _ => panic!("unknown child installer {installer}"),
        };
        let storage = StorageManager::new();
        let session = storage.create_session();
        JsRuntime::with_child_bootstrap(document, url, storage, session, kind).unwrap()
    } else {
        JsRuntime::with_document_and_url(document, url).unwrap()
    };
    if let Some(installer) = installer {
        runtime
            .eval(&format!(
                "{installer}('https://bootstrap.example.test/child.js', '1')"
            ))
            .unwrap();
    }
    runtime
}

fn assert_callable(runtime: &mut JsRuntime, names: &[&str]) {
    for name in names {
        let result = runtime
            .eval(&format!("typeof globalThis[{name:?}] === 'function'"))
            .unwrap();
        assert_eq!(result.as_boolean(), Some(true), "missing callable {name}");
    }
}

fn assert_no_dom(runtime: &mut JsRuntime) {
    // Check the entire list before failing so the pre-fix evidence names every
    // leaked DOM interface instead of stopping at the first missing deletion.
    let script = format!(
        "{}.filter(name => name in globalThis || typeof globalThis[name] !== 'undefined').join(',')",
        serde_json::to_string(DOM_APIS).unwrap(),
    );
    let leaked = runtime
        .eval(&script)
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(leaked, "", "DOM APIs leaked into a child global");
}

pub(super) fn assert_child_non_dom_apis(runtime: &mut JsRuntime) {
    let result = runtime
        .eval(include_str!("bootstrap_worker_contract.js"))
        .unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

pub(super) fn assert_private_capabilities_absent(runtime: &mut JsRuntime) {
    let names: Vec<String> = runtime
        .host_state
        .borrow()
        .parsed_bootstrap_modules
        .keys()
        .flat_map(|key| key.names.iter().cloned())
        .collect();
    let script = format!(
        "{}.filter(name => Object.hasOwn(globalThis, name)).join(',')",
        serde_json::to_string(&names).unwrap(),
    );
    let leaked = runtime
        .eval(&script)
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(leaked, "", "private native capability exposed");
}

fn assert_baseline_shape(kind: &str, shape: &Value, sources: &[Value]) {
    let kind = kind.strip_prefix("factory-").unwrap_or(kind);
    let fixture = match kind {
        "window" => include_str!("bootstrap_surface_baselines/window.json"),
        "dedicated-worker" => include_str!("bootstrap_surface_baselines/dedicated-worker.json"),
        "shared-worker" => include_str!("bootstrap_surface_baselines/shared-worker.json"),
        "worklet" => include_str!("bootstrap_surface_baselines/worklet.json"),
        _ => panic!("unknown bootstrap surface {kind}"),
    };
    let baseline: Value = serde_json::from_str(fixture).unwrap();
    assert_eq!(shape, &baseline["shape"], "{kind} bootstrap shape changed");
    if kind == "window" {
        let hashes: Vec<Value> = sources
            .iter()
            .map(|source| source["generated_source_sha256"].clone())
            .collect();
        assert_eq!(
            json!(hashes),
            baseline["generated_source_sha256"],
            "Window generated source changed"
        );
    }
}

pub(super) fn record_surface(runtime: &mut JsRuntime, kind: &str) {
    let snapshot = runtime
        .eval(SNAPSHOT)
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    let shape: Value = serde_json::from_str(&snapshot).unwrap();
    assert!(
        shape["globals"]
            .as_array()
            .is_some_and(|names| !names.is_empty())
    );
    let sources: Vec<Value> = runtime.host_state.borrow().parsed_bootstrap_modules.keys()
        .map(|key| {
            json!({"bindings": &key.names, "generated_source_sha256": format!("{:x}", Sha256::digest(key.build_module_source().as_bytes()))})
        }).collect();
    // No default filesystem writes. Explicit diagnostic output is create-new:
    // an existing result cannot silently be replaced by another source/run.
    if let Some(directory) = std::env::var_os("OMOIKANE_BOOTSTRAP_SNAPSHOT_DIR") {
        use std::io::Write as _;
        let path = std::path::PathBuf::from(directory).join(format!("{kind}.json"));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .unwrap();
        writeln!(file, "{}", serde_json::to_string_pretty(&json!({
            "kind": kind, "shape": shape, "bootstrap_sources": sources,
            "scope": "full bootstrap plus installer baseline; not a factory or conformance proof",
        })).unwrap())
        .unwrap();
    }
    assert_baseline_shape(kind, &shape, &sources);
}

#[test]
fn window_bootstrap_preserves_document_and_common_apis() {
    let mut runtime = runtime_for_surface(None);
    record_surface(&mut runtime, "window");
    assert_callable(&mut runtime, COMMON_APIS);
    assert_callable(
        &mut runtime,
        &["Node", "Element", "Document", "DOMParser", "XPathEvaluator"],
    );
    assert_private_capabilities_absent(&mut runtime);
    assert_eq!(
        runtime
            .eval("window === self && window === globalThis && document.defaultView === window")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn dedicated_worker_bootstrap_has_worker_apis_without_dom() {
    let mut runtime = runtime_for_surface(Some("__omoikane_install_worker_global"));
    record_surface(&mut runtime, "dedicated-worker");
    assert_callable(&mut runtime, COMMON_APIS);
    assert_callable(
        &mut runtime,
        &[
            "Worker",
            "WorkerGlobalScope",
            "DedicatedWorkerGlobalScope",
            "postMessage",
            "importScripts",
            "FileReaderSync",
        ],
    );
    assert_private_capabilities_absent(&mut runtime);
    assert_eq!(
        runtime
            .eval("typeof SharedWorkerGlobalScope")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "undefined"
    );
    assert_no_dom(&mut runtime);
}

#[test]
fn shared_worker_bootstrap_has_connect_apis_without_dom() {
    let mut runtime = runtime_for_surface(Some("__omoikane_install_shared_worker_global"));
    record_surface(&mut runtime, "shared-worker");
    assert_callable(&mut runtime, COMMON_APIS);
    assert_callable(
        &mut runtime,
        &[
            "Worker",
            "WorkerGlobalScope",
            "SharedWorkerGlobalScope",
            "importScripts",
            "MessagePort",
        ],
    );
    assert_private_capabilities_absent(&mut runtime);
    assert_eq!(
        runtime
            .eval("typeof DedicatedWorkerGlobalScope + ':' + ('onconnect' in globalThis)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "undefined:true"
    );
    assert_no_dom(&mut runtime);
}

#[test]
fn worklet_bootstrap_has_registration_apis_without_dom() {
    let mut runtime = runtime_for_surface(Some("__omoikane_install_worklet_global"));
    record_surface(&mut runtime, "worklet");
    assert_callable(&mut runtime, COMMON_APIS);
    assert_callable(
        &mut runtime,
        &[
            "WorkletGlobalScope",
            "registerPaint",
            "registerProcessor",
            "registerWorklet",
            "AudioWorkletProcessor",
        ],
    );
    assert_private_capabilities_absent(&mut runtime);
    assert_eq!(runtime.eval("typeof Worker + ':' + typeof SharedWorker + ':' + typeof DedicatedWorkerGlobalScope + ':' + typeof postMessage").unwrap().as_string().unwrap().to_std_string_escaped(), "undefined:undefined:undefined:undefined");
    assert_no_dom(&mut runtime);
}

#[test]
fn iframe_bootstrap_preserves_independent_document_apis() {
    let mut runtime = runtime_for_surface(None);
    runtime.eval("globalThis.bootstrapFrame = document.body.appendChild(document.createElement('iframe'))").unwrap();
    assert_eq!(runtime.eval("(() => { const child = bootstrapFrame.contentWindow; const element = child.document.createElement('span'); return child !== window && child.Node !== Node && element instanceof child.HTMLElement && !(element instanceof HTMLElement) && child.document.defaultView === child; })()").unwrap().as_boolean(), Some(true));
    assert_private_capabilities_absent(&mut runtime);
}

#[derive(Default)]
struct SurfaceScriptTransport;

impl crate::http::HttpTransport for SurfaceScriptTransport {
    fn send(
        &self,
        request: &HttpRequest,
        _: Option<Duration>,
    ) -> Result<crate::http::HttpResponse, crate::http::HttpParseError> {
        assert!(
            request.url().path().starts_with("/surface-") && request.url().path().ends_with(".js")
        );
        Ok(crate::http::HttpResponse::new(
            200,
            "OK",
            vec![("Content-Type".into(), "text/javascript".into())],
            Vec::new(),
        ))
    }
}

pub(super) fn factory_owner() -> JsRuntime {
    let mut owner = runtime_for_surface(None);
    owner.set_http_resource_transport(Some(Arc::new(SurfaceScriptTransport)));
    owner
}

pub(super) fn assert_cached_kind(runtime: &JsRuntime, expected: BootstrapKind) {
    let state = runtime.host_state.borrow();
    assert_eq!(state.parsed_bootstrap_modules.len(), 1);
    assert!(
        state
            .parsed_bootstrap_modules
            .keys()
            .all(|key| key.kind == expected)
    );
}

#[test]
fn dedicated_worker_factory_preserves_the_bootstrap_surface() {
    let mut owner = factory_owner();
    owner
        .eval("globalThis.surfaceWorker = new Worker('/surface-dedicated.js')")
        .unwrap();
    owner.run_until_idle().unwrap();
    let entry = owner
        .host_state
        .borrow()
        .workers
        .values()
        .next()
        .cloned()
        .unwrap();
    let mut entry = entry.borrow_mut();
    assert!(!entry.terminated && entry.startup_error.is_none());
    assert_cached_kind(&entry.runtime, BootstrapKind::DedicatedWorker);
    record_surface(&mut entry.runtime, "factory-dedicated-worker");
    assert_child_non_dom_apis(&mut entry.runtime);
    assert_private_capabilities_absent(&mut entry.runtime);
}

#[test]
fn data_worker_storage_url_uses_the_committed_worker_url() {
    let mut owner = factory_owner();
    owner
        .eval("globalThis.surfaceWorker = new Worker('data:text/javascript,')")
        .unwrap();
    owner.run_until_idle().unwrap();
    let entry = owner
        .host_state
        .borrow()
        .workers
        .values()
        .next()
        .cloned()
        .unwrap();
    let mut entry = entry.borrow_mut();
    assert!(!entry.terminated && entry.startup_error.is_none());
    let matches = entry
        .runtime
        .eval(
            "localStorage.__document.URL === 'data:text/javascript,' && \
         sessionStorage.__document.URL === 'data:text/javascript,'",
        )
        .unwrap();
    assert_eq!(matches.as_boolean(), Some(true));
    // An author-supplied Location value must not change the native URL handle.
    let matches = entry
        .runtime
        .eval(
            "try { globalThis.location = {href:'https://forged.invalid/'}; } catch (_) {} \
         localStorage.__document.URL === 'data:text/javascript,'",
        )
        .unwrap();
    assert_eq!(matches.as_boolean(), Some(true));
}
