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
        let url = request.url().to_string();
        if url.ends_with("slow.png") || url.ends_with("slow-error.png") {
            let _ = self.entered.send(());
            let _ = self
                .release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5));
        }
        if url.ends_with("slow-error.png") {
            return Ok(HttpResponse::new(
                404,
                "Not Found",
                vec![("Content-Type".into(), "image/png".into())],
                Vec::new(),
            ));
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

fn wait_until_images_settle(runtime: &mut JsRuntime) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        runtime.run_until_idle().unwrap();
        let pending = runtime
            .host_state
            .borrow()
            .image_resources
            .values()
            .any(|images| images.borrow().has_pending_images());
        if !pending {
            runtime.run_until_idle().unwrap();
            return;
        }
        assert!(Instant::now() < deadline, "image did not settle");
        std::thread::sleep(Duration::from_millis(1));
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
fn asynchronous_image_completion_retains_styles_and_rebuilds_main_geometry() {
    let document = crate::html::TreeBuilder::parse(
        "<style>html,body{margin:0}img{vertical-align:top}#peer{color:red}</style>\
         <body><div id='peer'></div></body>",
    )
    .document();
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
            "globalThis.events=[]; globalThis.image=new Image();\
             image.onload=()=>events.push('load'); image.onerror=()=>events.push('error');\
             image.src='/slow.png'; document.body.prepend(image)",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        runtime
            .eval("image.getBoundingClientRect().width")
            .unwrap()
            .as_number(),
        Some(0.0)
    );
    let red = crate::paint::Color::rgb(255, 0, 0);
    assert_ne!(
        runtime.paint_current_document().unwrap().pixel(0, 0),
        Some(red)
    );
    let generations = runtime.render_generations();
    let peer = runtime.document().query_selector("#peer").unwrap();
    let peer_style = {
        let mut state = runtime.host_state.borrow_mut();
        let document_id = state.document.identity();
        state
            .document_styles
            .get_mut(&document_id)
            .unwrap()
            .resolver
            .as_mut()
            .unwrap()
            .computed_style_shared(&peer)
    };

    release.send(()).unwrap();
    wait_until_images_settle(&mut runtime);
    let invalidated = runtime.render_generations();
    assert_eq!(invalidated.style, generations.style);
    assert_eq!(invalidated.layout, generations.layout);
    assert!(invalidated.paint > generations.paint);
    let peer_style_after = {
        let mut state = runtime.host_state.borrow_mut();
        let document_id = state.document.identity();
        assert!(state.layout_root.is_none());
        let entry = state.document_styles.get_mut(&document_id).unwrap();
        assert!(!entry.dirty);
        entry
            .resolver
            .as_mut()
            .unwrap()
            .computed_style_shared(&peer)
    };
    assert!(Arc::ptr_eq(&peer_style, &peer_style_after));
    assert_eq!(
        runtime
            .eval("[image.getBoundingClientRect().width,image.getBoundingClientRect().height].join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "1,1"
    );
    assert!(runtime.render_generations().layout > generations.layout);
    assert_eq!(
        runtime.paint_current_document().unwrap().pixel(0, 0),
        Some(red)
    );
    assert_eq!(
        runtime
            .eval("JSON.stringify(events)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[\"load\"]"
    );
}

#[test]
fn asynchronous_child_image_completion_drops_only_child_geometry() {
    let document = crate::html::TreeBuilder::parse(
        "<body><iframe id='frame' style='width:20px;height:20px;border:0'></iframe></body>",
    )
    .document();
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
            "globalThis.frame=document.getElementById('frame');\
             frame.srcdoc=\"<style>html,body{margin:0}img{vertical-align:top}#peer{color:red}</style><div id='peer'></div>\";\
             globalThis.child=frame.contentDocument; globalThis.childImage=child.createElement('img');\
             childImage.src='https://image.test/slow.png'; child.body.prepend(childImage)",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        runtime
            .eval("childImage.getBoundingClientRect().width")
            .unwrap()
            .as_number(),
        Some(0.0)
    );
    let main_layout_generation = runtime.render_generations().layout;
    let (child_document, peer, peer_style, child_builds) = {
        let mut state = runtime.host_state.borrow_mut();
        let child_document = state
            .iframe_documents
            .values()
            .next()
            .unwrap()
            .document
            .clone();
        let document_id = child_document.identity();
        let peer = child_document.query_selector("#peer").unwrap();
        let child_builds = state.child_layout_builds;
        let entry = state.document_styles.get_mut(&document_id).unwrap();
        assert!(!entry.dirty);
        assert!(entry.child_layout.is_some());
        let style = entry
            .resolver
            .as_mut()
            .unwrap()
            .computed_style_shared(&peer);
        (child_document, peer, style, child_builds)
    };

    release.send(()).unwrap();
    wait_until_images_settle(&mut runtime);
    let peer_style_after = {
        let mut state = runtime.host_state.borrow_mut();
        let entry = state
            .document_styles
            .get_mut(&child_document.identity())
            .unwrap();
        assert!(!entry.dirty);
        assert!(entry.child_layout.is_none());
        entry
            .resolver
            .as_mut()
            .unwrap()
            .computed_style_shared(&peer)
    };
    assert!(Arc::ptr_eq(&peer_style, &peer_style_after));
    assert_eq!(runtime.render_generations().layout, main_layout_generation);
    assert_eq!(
        runtime
            .eval("childImage.getBoundingClientRect().width")
            .unwrap()
            .as_number(),
        Some(1.0)
    );
    let state = runtime.host_state.borrow();
    assert!(state.child_layout_builds > child_builds);
    assert!(
        state.document_styles[&child_document.identity()]
            .child_layout
            .is_some()
    );
}

#[test]
fn image_layout_invalidation_preserves_a_preexisting_style_rebuild() {
    let document = crate::html::TreeBuilder::parse(
        "<style>body{margin:0}</style><body><div id='target'></div></body>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime
        .eval("document.getElementById('target').getBoundingClientRect().width")
        .unwrap();
    let document = runtime.document().clone();
    let mut state = runtime.host_state.borrow_mut();
    let document_id = document.identity();
    state.document_styles.get_mut(&document_id).unwrap().dirty = true;
    state.mark_document_layout_dirty(&document, "image_resource_completed");
    assert!(state.document_styles[&document_id].dirty);
    assert!(state.layout_root.is_none());
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

#[test]
fn asynchronous_background_completion_repaints_without_rebuilding_main_geometry() {
    let document = crate::html::TreeBuilder::parse(
        "<style>html,body{margin:0}#target{width:20px;height:20px;\
         background-color:blue;background-image:url('/slow.png');\
         background-repeat:no-repeat}</style><body><div id='target'></div></body>",
    )
    .document();
    let mut runtime = JsRuntime::with_document_and_url(document, "https://image.test/").unwrap();
    let (entered, started) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    runtime.set_http_resource_transport(Some(Arc::new(ImageTransport {
        entered,
        release: Mutex::new(gate),
    })));
    runtime.set_nonblocking_image_loading(true);

    let blue = crate::paint::Color::rgb(0, 0, 255);
    let red = crate::paint::Color::rgb(255, 0, 0);
    assert_eq!(
        runtime.paint_current_document().unwrap().pixel(0, 0),
        Some(blue)
    );
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    let generations = runtime.render_generations();
    assert!(runtime.host_state.borrow().layout_root.is_some());

    release.send(()).unwrap();
    wait_until_images_settle(&mut runtime);

    let invalidated = runtime.render_generations();
    assert_eq!(invalidated.style, generations.style);
    assert_eq!(invalidated.layout, generations.layout);
    assert!(invalidated.paint > generations.paint);
    assert!(
        runtime.host_state.borrow().layout_root.is_some(),
        "a paint-only background completion must retain geometry"
    );
    assert_eq!(
        runtime
            .eval(
                "(()=>{const target=document.getElementById('target');return [target.getBoundingClientRect().width,target.getBoundingClientRect().height].join(',')})()",
            )
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "20,20"
    );
    assert_eq!(runtime.render_generations().layout, generations.layout);
    assert_eq!(
        runtime.paint_current_document().unwrap().pixel(0, 0),
        Some(red)
    );
    assert_eq!(runtime.render_generations().layout, generations.layout);
}

#[test]
fn asynchronous_child_background_completion_repaints_without_rebuilding_child_geometry() {
    let document = crate::html::TreeBuilder::parse(
        "<style>html,body{margin:0}iframe{display:block;width:20px;height:20px;border:0}</style>\
         <body><iframe id='frame'></iframe></body>",
    )
    .document();
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
            "globalThis.frame=document.getElementById('frame');\
             frame.srcdoc=\"<style>html,body{margin:0}#target{width:20px;height:20px;background-color:blue;background-image:url('https://image.test/slow.png');background-repeat:no-repeat}</style><div id='target'></div>\";\
             globalThis.child=frame.contentDocument",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();

    let blue = crate::paint::Color::rgb(0, 0, 255);
    let red = crate::paint::Color::rgb(255, 0, 0);
    assert_eq!(
        runtime.paint_current_document().unwrap().pixel(0, 0),
        Some(blue)
    );
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    let generations = runtime.render_generations();
    let (child_id, child_builds) = {
        let state = runtime.host_state.borrow();
        let child = state.iframe_documents.values().next().unwrap();
        let child_id = child.document.identity();
        assert!(state.document_styles[&child_id].child_layout.is_some());
        (child_id, state.child_layout_builds)
    };

    release.send(()).unwrap();
    wait_until_images_settle(&mut runtime);

    let invalidated = runtime.render_generations();
    assert_eq!(invalidated.style, generations.style);
    assert_eq!(invalidated.layout, generations.layout);
    assert!(invalidated.paint > generations.paint);
    assert!(
        runtime.host_state.borrow().document_styles[&child_id]
            .child_layout
            .is_some(),
        "a paint-only child background completion must retain child geometry"
    );
    assert_eq!(
        runtime.paint_current_document().unwrap().pixel(0, 0),
        Some(red)
    );
    assert_eq!(
        runtime.host_state.borrow().child_layout_builds,
        child_builds
    );
    assert_eq!(runtime.render_generations().layout, generations.layout);
}

#[test]
fn nested_blob_svg_background_completion_repaints_without_rebuilding_geometry() {
    let blob = "blob:https://image.test/background-svg";
    let document = crate::html::TreeBuilder::parse(&format!(
        "<style>html,body{{margin:0}}#target{{width:20px;height:20px;\
         background-color:blue;background-image:url('{blob}');\
         background-repeat:no-repeat}}</style><body><div id='target'></div></body>"
    ))
    .document();
    let mut runtime = JsRuntime::with_document_and_url(document, "https://image.test/").unwrap();
    crate::data::register_blob_url(
        blob.into(),
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
          <rect width="10" height="10" fill="blue"/>
          <image href="https://image.test/slow.png" width="10" height="10"/>
        </svg>"#
            .to_vec(),
        "image/svg+xml".into(),
    );
    let (entered, started) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    runtime.set_http_resource_transport(Some(Arc::new(ImageTransport {
        entered,
        release: Mutex::new(gate),
    })));
    runtime.set_nonblocking_image_loading(true);

    assert_eq!(
        runtime.paint_current_document().unwrap().pixel(0, 0),
        Some(crate::paint::Color::rgb(0, 0, 255))
    );
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    let generations = runtime.render_generations();
    assert!(runtime.host_state.borrow().layout_root.is_some());

    release.send(()).unwrap();
    wait_until_images_settle(&mut runtime);

    let invalidated = runtime.render_generations();
    assert_eq!(invalidated.style, generations.style);
    assert_eq!(invalidated.layout, generations.layout);
    assert!(invalidated.paint > generations.paint);
    assert!(
        runtime.host_state.borrow().layout_root.is_some(),
        "a nested image used only by background paint must retain geometry"
    );
    assert_eq!(
        runtime.paint_current_document().unwrap().pixel(0, 0),
        Some(crate::paint::Color::rgb(255, 0, 0))
    );
    assert_eq!(runtime.render_generations().layout, generations.layout);
    crate::data::revoke_blob_url(blob);
}

#[test]
fn asynchronous_failed_background_completion_does_not_rebuild_main_geometry() {
    let document = crate::html::TreeBuilder::parse(
        "<style>html,body{margin:0}#target{width:20px;height:20px;\
         background-color:blue;background-image:url('/slow-error.png');\
         background-repeat:no-repeat}</style><body><div id='target'></div></body>",
    )
    .document();
    let mut runtime = JsRuntime::with_document_and_url(document, "https://image.test/").unwrap();
    let (entered, started) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    runtime.set_http_resource_transport(Some(Arc::new(ImageTransport {
        entered,
        release: Mutex::new(gate),
    })));
    runtime.set_nonblocking_image_loading(true);

    let blue = crate::paint::Color::rgb(0, 0, 255);
    assert_eq!(
        runtime.paint_current_document().unwrap().pixel(0, 0),
        Some(blue)
    );
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    let generations = runtime.render_generations();
    assert!(runtime.host_state.borrow().layout_root.is_some());

    release.send(()).unwrap();
    wait_until_images_settle(&mut runtime);

    let invalidated = runtime.render_generations();
    assert_eq!(invalidated.style, generations.style);
    assert_eq!(invalidated.layout, generations.layout);
    assert!(invalidated.paint > generations.paint);
    assert!(
        runtime.host_state.borrow().layout_root.is_some(),
        "a failed paint-only image must retain geometry"
    );
    assert_eq!(
        runtime.paint_current_document().unwrap().pixel(0, 0),
        Some(blue)
    );
    assert_eq!(runtime.render_generations().layout, generations.layout);
}

#[test]
fn shared_background_and_intrinsic_image_completion_still_rebuilds_geometry() {
    let document = crate::html::TreeBuilder::parse(
        "<style>html,body{margin:0}#target{width:20px;height:20px;\
         background-color:blue;background-image:url('/slow.png');\
         background-repeat:no-repeat}</style>\
         <body><div id='target'></div></body>",
    )
    .document();
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
            "globalThis.events=[];globalThis.image=new Image();\
             image.onload=()=>events.push('load');image.onerror=()=>events.push('error');\
             image.src='/slow.png';document.body.append(image)",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    runtime.paint_current_document().unwrap();
    assert_eq!(
        runtime
            .eval("image.getBoundingClientRect().width")
            .unwrap()
            .as_number(),
        Some(0.0)
    );
    let generations = runtime.render_generations();
    assert!(runtime.host_state.borrow().layout_root.is_some());

    release.send(()).unwrap();
    wait_until_images_settle(&mut runtime);

    let invalidated = runtime.render_generations();
    assert_eq!(invalidated.style, generations.style);
    assert_eq!(invalidated.layout, generations.layout);
    assert!(invalidated.paint > generations.paint);
    assert!(
        runtime.host_state.borrow().layout_root.is_none(),
        "an intrinsic consumer must make a shared URL geometry-dependent"
    );
    assert_eq!(
        runtime
            .eval("[image.getBoundingClientRect().width,image.getBoundingClientRect().height].join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "1,1"
    );
    assert!(runtime.render_generations().layout > generations.layout);
    assert_eq!(
        runtime
            .eval("JSON.stringify(events)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[\"load\"]"
    );
    assert_eq!(
        runtime.paint_current_document().unwrap().pixel(0, 0),
        Some(crate::paint::Color::rgb(255, 0, 0))
    );
}

#[test]
fn inline_svg_nested_image_completion_still_rebuilds_geometry() {
    let document = crate::html::TreeBuilder::parse(
        "<style>html,body{margin:0}svg{display:block}</style><body>\
         <svg id='icon' xmlns='http://www.w3.org/2000/svg' width='10' height='10'>\
         <rect width='10' height='10' fill='blue'/>\
         <image href='https://image.test/slow.png' width='10' height='10'/>\
         </svg></body>",
    )
    .document();
    let mut runtime = JsRuntime::with_document_and_url(document, "https://image.test/").unwrap();
    let (entered, started) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    runtime.set_http_resource_transport(Some(Arc::new(ImageTransport {
        entered,
        release: Mutex::new(gate),
    })));
    runtime.set_nonblocking_image_loading(true);

    assert_eq!(
        runtime.paint_current_document().unwrap().pixel(0, 0),
        Some(crate::paint::Color::rgb(0, 0, 255))
    );
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    let generations = runtime.render_generations();
    assert!(runtime.host_state.borrow().layout_root.is_some());

    release.send(()).unwrap();
    wait_until_images_settle(&mut runtime);

    let invalidated = runtime.render_generations();
    assert_eq!(invalidated.style, generations.style);
    assert_eq!(invalidated.layout, generations.layout);
    assert!(invalidated.paint > generations.paint);
    assert!(
        runtime.host_state.borrow().layout_root.is_none(),
        "an image read while rasterizing inline SVG layout is geometry-dependent"
    );
    assert_eq!(
        runtime.paint_current_document().unwrap().pixel(0, 0),
        Some(crate::paint::Color::rgb(255, 0, 0))
    );
    assert!(runtime.render_generations().layout > generations.layout);
}

#[test]
fn new_owner_without_a_full_layout_treats_completion_as_geometry_dependent() {
    let document = crate::html::TreeBuilder::parse(
        "<style>html,body{margin:0}#target{width:20px;height:20px}</style>\
         <body><div id='target'></div></body>",
    )
    .document();
    let mut runtime = JsRuntime::with_document_and_url(document, "https://image.test/").unwrap();
    let (entered, started) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    runtime.set_http_resource_transport(Some(Arc::new(ImageTransport {
        entered,
        release: Mutex::new(gate),
    })));
    runtime.set_nonblocking_image_loading(true);

    runtime.paint_current_document().unwrap();
    assert!(runtime.host_state.borrow().layout_root.is_some());
    runtime.install_document_response_headers(&[(
        "Cross-Origin-Embedder-Policy".into(),
        "require-corp".into(),
    )]);
    runtime
        .eval(
            "globalThis.events=[];globalThis.detached=new Image();\
             detached.onload=()=>events.push('load');detached.onerror=()=>events.push('error');\
             detached.src='/slow.png'",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    let generations = runtime.render_generations();
    assert!(runtime.host_state.borrow().layout_root.is_some());

    release.send(()).unwrap();
    wait_until_images_settle(&mut runtime);

    let invalidated = runtime.render_generations();
    assert_eq!(invalidated.style, generations.style);
    assert_eq!(invalidated.layout, generations.layout);
    assert!(invalidated.paint > generations.paint);
    assert!(
        runtime.host_state.borrow().layout_root.is_none(),
        "a new image owner remains conservative until its first full layout"
    );
    assert_eq!(
        runtime
            .eval("JSON.stringify(events)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[\"load\"]"
    );
}

#[test]
fn new_csp_policy_without_a_full_layout_treats_completion_as_geometry_dependent() {
    let document = crate::html::TreeBuilder::parse(
        "<style>html,body{margin:0}#target{width:20px;height:20px}</style>\
         <body><div id='target'></div></body>",
    )
    .document();
    let mut runtime = JsRuntime::with_document_and_url(document, "https://image.test/").unwrap();
    let (entered, started) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    runtime.set_http_resource_transport(Some(Arc::new(ImageTransport {
        entered,
        release: Mutex::new(gate),
    })));
    runtime.set_nonblocking_image_loading(true);

    runtime.paint_current_document().unwrap();
    assert!(runtime.host_state.borrow().layout_root.is_some());
    runtime.install_csp_policy(&["img-src *".into()]);
    runtime
        .eval(
            "globalThis.events=[];globalThis.detached=new Image();\
             detached.onload=()=>events.push('load');detached.onerror=()=>events.push('error');\
             detached.src='/slow.png'",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    let generations = runtime.render_generations();
    assert!(runtime.host_state.borrow().layout_root.is_some());

    release.send(()).unwrap();
    wait_until_images_settle(&mut runtime);

    let invalidated = runtime.render_generations();
    assert_eq!(invalidated.style, generations.style);
    assert_eq!(invalidated.layout, generations.layout);
    assert!(invalidated.paint > generations.paint);
    assert!(
        runtime.host_state.borrow().layout_root.is_none(),
        "a new CSP image state remains conservative until its first full layout"
    );
    assert_eq!(
        runtime
            .eval("JSON.stringify(events)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[\"load\"]"
    );
}

#[test]
fn skipped_content_visibility_image_completion_defers_reflow_until_geometry_is_forced() {
    let document = crate::html::TreeBuilder::parse(
        "<style>html,body{margin:0}#spacer{height:2000px}\
         #auto{content-visibility:auto;contain-intrinsic-size:20px}</style>\
         <body><div id='spacer'></div><section id='auto'></section></body>",
    )
    .document();
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
            "globalThis.events=[];globalThis.image=new Image();\
             image.onload=()=>events.push('load');image.onerror=()=>events.push('error');\
             image.src='/slow.png';document.getElementById('auto').append(image)",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    runtime.paint_current_document().unwrap();
    let generations = runtime.render_generations();
    let auto_id = {
        let state = runtime.host_state.borrow();
        let auto = state.document.query_selector("#auto").unwrap();
        assert!(
            state
                .content_visibility_skipped_nodes
                .contains(&auto.identity())
        );
        assert!(state.layout_root.is_some());
        auto.identity()
    };

    release.send(()).unwrap();
    wait_until_images_settle(&mut runtime);

    let invalidated = runtime.render_generations();
    assert_eq!(invalidated.style, generations.style);
    assert_eq!(invalidated.layout, generations.layout);
    assert!(invalidated.paint > generations.paint);
    {
        let state = runtime.host_state.borrow();
        assert!(state.content_visibility_skipped_nodes.contains(&auto_id));
        assert!(
            state.layout_root.is_some(),
            "a resource omitted from skipped contents is not current geometry"
        );
    }
    assert_eq!(
        runtime
            .eval("JSON.stringify(events)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[\"load\"]"
    );
    assert_eq!(
        runtime
            .eval("[image.getBoundingClientRect().width,image.getBoundingClientRect().height].join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "1,1"
    );
    assert!(runtime.render_generations().layout > generations.layout);
}

#[test]
fn background_completion_preserves_a_preexisting_dirty_stylesheet_and_layout() {
    let document = crate::html::TreeBuilder::parse(
        "<style>html,body{margin:0}#target{width:20px;height:20px;\
         background-color:blue;background-image:url('/slow.png');\
         background-repeat:no-repeat}</style><body><div id='target'></div></body>",
    )
    .document();
    let mut runtime = JsRuntime::with_document_and_url(document, "https://image.test/").unwrap();
    let (entered, started) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    runtime.set_http_resource_transport(Some(Arc::new(ImageTransport {
        entered,
        release: Mutex::new(gate),
    })));
    runtime.set_nonblocking_image_loading(true);
    runtime.paint_current_document().unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();

    runtime
        .eval("document.querySelector('style').textContent+='#target{width:21px}'")
        .unwrap();
    // Arrange a stylesheet update still awaiting resolver reconstruction.
    let document = runtime.document().clone();
    runtime
        .host_state
        .borrow_mut()
        .mark_document_style_dirty(&document);
    let dirty_generations = runtime.render_generations();
    let document_id = runtime.document().identity();
    {
        let state = runtime.host_state.borrow();
        assert!(state.document_styles[&document_id].dirty);
        assert!(state.layout_root.is_none());
    }

    release.send(()).unwrap();
    // Poll resource completion directly so unrelated event-loop style work
    // cannot consume the deliberately pending stylesheet update first.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        runtime.poll_image_resource_loads().unwrap();
        let pending = runtime
            .host_state
            .borrow()
            .image_resources
            .values()
            .any(|images| images.borrow().has_pending_images());
        if !pending {
            break;
        }
        assert!(Instant::now() < deadline, "background did not settle");
        std::thread::sleep(Duration::from_millis(1));
    }

    let completed = runtime.render_generations();
    assert_eq!(completed.style, dirty_generations.style);
    assert_eq!(completed.layout, dirty_generations.layout);
    assert!(completed.paint > dirty_generations.paint);
    {
        let state = runtime.host_state.borrow();
        assert!(state.document_styles[&document_id].dirty);
        assert!(state.layout_root.is_none());
    }
    assert_eq!(
        runtime
            .eval("document.getElementById('target').getBoundingClientRect().width")
            .unwrap()
            .as_number(),
        Some(21.0)
    );
    assert!(runtime.render_generations().layout > dirty_generations.layout);
}
