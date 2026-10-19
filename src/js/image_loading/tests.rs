use super::*;
use crate::http::{HttpParseError, HttpRequest, HttpResponse, HttpTransport};
use std::sync::mpsc;
use std::time::Duration;

struct ImageTransport {
    entered: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl HttpTransport for ImageTransport {
    fn send(
        &self,
        request: &HttpRequest,
        _: Option<Duration>,
    ) -> Result<HttpResponse, HttpParseError> {
        if request.url().to_string().ends_with("slow.png") {
            let _ = self.entered.send(());
            let _ = self
                .release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5));
        }
        let mut canvas = crate::paint::Canvas::new(1, 1);
        canvas.set_pixel(0, 0, crate::paint::Color::rgb(255, 0, 0));
        Ok(HttpResponse::new(
            200,
            "OK",
            vec![("Content-Type".into(), "image/png".into())],
            canvas.encode_png(),
        ))
    }
}

#[test]
fn blob_svg_load_event_waits_for_its_http_image_dependency() {
    let document = crate::html::TreeBuilder::parse("<body></body>").document();
    let mut runtime = JsRuntime::with_document_and_url(document, "https://image.test/").unwrap();
    let (entered, started) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    runtime.set_http_resource_transport(Some(Arc::new(ImageTransport {
        entered,
        release: Mutex::new(gate),
    })));
    runtime.set_nonblocking_image_loading(true);
    let blob = "blob:https://image.test/load-event-svg";
    crate::data::register_blob_url(
        blob.into(),
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1">
          <rect width="1" height="1" fill="blue"/>
          <image href="https://image.test/slow.png" width="1" height="1"/>
        </svg>"#
            .to_vec(),
        "image/svg+xml".into(),
    );
    runtime
        .eval(
            "globalThis.events=[]; globalThis.image=new Image();
         image.onload=()=>events.push('load'); image.onerror=()=>events.push('error');
         image.src='blob:https://image.test/load-event-svg'; document.body.append(image)",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    for _ in 0..3 {
        runtime.run_until_idle().unwrap();
        assert_eq!(
            runtime.eval("events.length").unwrap().as_number(),
            Some(0.0)
        );
        assert_eq!(runtime.host_state.borrow().pending_image_events.len(), 1);
    }
    assert_eq!(
        runtime.next_rendering(),
        crate::cdp::NextRendering::EveryFrame
    );
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        runtime.run_until_idle().unwrap();
        if runtime.eval("events.length").unwrap().as_number() == Some(1.0) {
            break;
        }
        assert!(Instant::now() < deadline, "blob SVG load did not settle");
        std::thread::sleep(Duration::from_millis(1));
    }
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("JSON.stringify(events)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[\"load\"]"
    );
    assert!(runtime.host_state.borrow().pending_image_events.is_empty());
    crate::data::revoke_blob_url(blob);
}

#[test]
fn asynchronous_image_events_wait_for_completion_and_discard_replaced_sources() {
    let document = crate::html::TreeBuilder::parse("<body></body>").document();
    let mut runtime = JsRuntime::with_document_and_url(document, "https://image.test/").unwrap();
    let (entered, started) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    runtime.set_http_resource_transport(Some(Arc::new(ImageTransport {
        entered,
        release: Mutex::new(gate),
    })));
    runtime.set_nonblocking_image_loading(true);
    runtime
        .eval(
            "globalThis.events=[]; globalThis.image=new Image();
        image.onload=()=>events.push(image.src); image.onerror=()=>events.push('error');
        image.src='/slow.png'; document.body.append(image)",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        runtime.eval("events.length").unwrap().as_number(),
        Some(0.0)
    );
    assert_eq!(
        runtime.next_rendering(),
        crate::cdp::NextRendering::EveryFrame
    );
    runtime.eval("image.src='/fast.png'").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        runtime.run_until_idle().unwrap();
        if runtime.eval("events.length").unwrap().as_number() == Some(1.0) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "replacement image did not finish"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    release.send(()).unwrap();
    loop {
        runtime.run_until_idle().unwrap();
        if !runtime
            .host_state
            .borrow()
            .image_resources
            .values()
            .any(|images| images.borrow().has_pending_images())
        {
            break;
        }
        assert!(Instant::now() < deadline, "old image did not settle");
        std::thread::sleep(Duration::from_millis(1));
    }
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("JSON.stringify(events)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[\"/fast.png\"]"
    );
}

#[test]
fn retiring_child_document_removes_pending_image_notifications() {
    let document = crate::html::TreeBuilder::parse("<body></body>").document();
    let mut runtime = JsRuntime::with_document_and_url(document, "https://image.test/").unwrap();
    let (entered, started) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    runtime.set_http_resource_transport(Some(Arc::new(ImageTransport {
        entered,
        release: Mutex::new(gate),
    })));
    runtime.set_nonblocking_image_loading(true);
    runtime
        .eval(
            "globalThis.events=[]; globalThis.frame=document.createElement('iframe');
        document.body.append(frame); frame.contentWindow.eval(`
          globalThis.image=new Image();
          image.onload=()=>parent.events.push('load');
          image.onerror=()=>parent.events.push('error');
          image.src='https://image.test/slow.png'; document.body.append(image);
        `)",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(runtime.host_state.borrow().pending_image_events.len(), 1);
    runtime.eval("frame.remove()").unwrap();
    assert!(runtime.host_state.borrow().pending_image_events.is_empty());
    assert!(
        !runtime
            .host_state
            .borrow()
            .image_resources
            .values()
            .any(|images| images.borrow().has_pending_images())
    );
    release.send(()).unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime.eval("events.length").unwrap().as_number(),
        Some(0.0)
    );
}

#[test]
fn runtime_drop_cancels_images_even_when_host_state_is_retained() {
    let document = crate::html::TreeBuilder::parse("<body></body>").document();
    let mut runtime = JsRuntime::with_document_and_url(document, "https://image.test/").unwrap();
    let (entered, started) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    runtime.set_http_resource_transport(Some(Arc::new(ImageTransport {
        entered,
        release: Mutex::new(gate),
    })));
    runtime.set_nonblocking_image_loading(true);
    runtime
        .eval("globalThis.image=new Image(); image.src='/slow.png'; document.body.append(image)")
        .unwrap();
    runtime.run_until_idle().unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    let retained_state = Rc::clone(&runtime.host_state);
    assert!(!retained_state.borrow().pending_image_events.is_empty());
    drop(runtime);
    assert!(retained_state.borrow().pending_image_events.is_empty());
    assert!(retained_state.borrow().image_resources.is_empty());
    release.send(()).unwrap();
}
