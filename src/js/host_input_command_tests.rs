use super::host_input_types::*;
use super::*;

fn make_runtime() -> JsRuntime {
    JsRuntime::with_document(
        crate::html::TreeBuilder::parse(
            "<body><input id=field><button id=a>A</button><button id=b>B</button></body>",
        )
        .document(),
    )
    .unwrap()
}

#[test]
fn owned_keyboard_preserves_trust_defaults_and_explicit_job_boundary() {
    let mut runtime = make_runtime();
    let document = runtime.document().clone();
    runtime.eval("globalThis.field=document.getElementById('field'); field.focus(); globalThis.trace = []; field.onkeydown = e => { trace.push([e.key,e.isTrusted]); queueMicrotask(() => trace.push('job')); }; __omoikane_dispatch_keyboard_input = () => { throw Error('poison'); };").unwrap();
    let result = runtime
        .dispatch_host_input(
            &document,
            HostInputCommand::Keyboard {
                phase: HostKeyPhase::Down,
                input: HostKeyInput {
                    key: "x".into(),
                    text: "x".into(),
                    ..Default::default()
                },
            },
        )
        .unwrap();
    assert_eq!(result.as_boolean(), Some(true));
    assert_eq!(
        runtime
            .eval("JSON.stringify([field.value,trace])")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[\"x\",[[\"x\",true]]]"
    );
    runtime.run_jobs().unwrap();
    assert_eq!(
        runtime
            .eval("JSON.stringify(trace)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[[\"x\",true],\"job\"]"
    );
}

#[test]
fn owned_mouse_related_target_is_a_node_not_a_json_identity() {
    let mut runtime = make_runtime();
    let document = runtime.document().clone();
    let target = document.query_selector("#a").unwrap();
    let related = document.query_selector("#b").unwrap();
    runtime.eval("globalThis.a=document.getElementById('a'); globalThis.b=document.getElementById('b'); globalThis.observed = null; a.onmouseover = e => { observed = [e.target === a, e.relatedTarget === b, e.relatedTarget.nodeType, e.isTrusted]; };").unwrap();
    runtime
        .dispatch_host_input(
            &document,
            HostInputCommand::Mouse {
                target,
                phase: HostMousePhase::Over,
                input: HostMouseInput {
                    related_target: Some(related),
                    ..Default::default()
                },
                focus_target: false,
            },
        )
        .unwrap();
    assert_eq!(
        runtime
            .eval("JSON.stringify(observed)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[true,true,1,true]"
    );
}

#[test]
fn owned_input_rejects_foreign_nodes_non_documents_and_nonfinite_data_before_events() {
    let mut runtime = make_runtime();
    let document = runtime.document().clone();
    let target = document.query_selector("#a").unwrap();
    let foreign = make_runtime();
    let foreign_document = foreign.document().clone();
    runtime.eval("globalThis.delivered = 0; document.addEventListener('wheel', () => delivered++); document.addEventListener('keydown', () => delivered++);").unwrap();
    for entry in [&target, &foreign_document] {
        assert!(
            runtime
                .dispatch_host_input(
                    entry,
                    HostInputCommand::Keyboard {
                        phase: HostKeyPhase::Down,
                        input: HostKeyInput::default(),
                    }
                )
                .is_err()
        );
    }
    for (node, delta) in [(foreign_document, [0.0; 3]), (target, [f64::NAN, 0.0, 0.0])] {
        assert!(
            runtime
                .dispatch_host_input(
                    &document,
                    HostInputCommand::Wheel {
                        target: node,
                        input: HostMouseInput::default(),
                        delta,
                    }
                )
                .is_err()
        );
    }
    assert_eq!(runtime.eval("delivered").unwrap().as_number(), Some(0.0));
    assert!(
        runtime
            .host_state
            .borrow()
            .input_bridge
            .host_client_document
            .is_none()
    );
}

#[test]
fn owned_touch_preserves_dynamic_contact_fields_and_document_target() {
    let mut runtime = make_runtime();
    let document = runtime.document().clone();
    runtime.eval("globalThis.contact = null; document.addEventListener('touchstart', e => { const p=e.touches[0]; contact=[e.isTrusted,p.identifier,p.target===document,p.touchType,p.altitudeAngle,p.azimuthAngle,p.radiusX,p.force,e.ctrlKey]; });").unwrap();
    runtime
        .dispatch_host_input(
            &document,
            HostInputCommand::Touch {
                target: document.clone(),
                phase: HostTouchPhase::Start,
                input: HostTouchInput {
                    touches: vec![HostTouchPoint {
                        touch_type: HostTouchType::Stylus,
                        altitude_angle: 0.25,
                        azimuth_angle: 0.5,
                        radius: [2.0, 3.0],
                        force: 0.75,
                        ..Default::default()
                    }],
                    modifiers: HostInputModifiers {
                        control: true,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            },
        )
        .unwrap();
    assert_eq!(
        runtime
            .eval("JSON.stringify(contact)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[true,0,true,\"stylus\",0.25,0.5,2,0.75,true]"
    );
}

#[test]
fn owned_composition_wheel_and_capture_have_distinct_effects() {
    let mut runtime = make_runtime();
    let document = runtime.document().clone();
    let field = document.query_selector("#field").unwrap();
    runtime.eval("globalThis.field=document.getElementById('field'); field.focus(); globalThis.wheel=null; globalThis.lost=0; field.addEventListener('wheel',e=>{wheel=[e.deltaX,e.deltaY,e.deltaZ,e.deltaMode,e.isTrusted];e.preventDefault()}); field.addEventListener('lostpointercapture',()=>lost++);").unwrap();
    runtime
        .dispatch_host_input(
            &document,
            HostInputCommand::SetComposition {
                text: "ab".into(),
                selection: [1, 2],
            },
        )
        .unwrap();
    assert_eq!(
        runtime
            .eval("JSON.stringify([field.value,field.selectionStart,field.selectionEnd])")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[\"ab\",1,2]"
    );
    runtime
        .dispatch_host_input(
            &document,
            HostInputCommand::CommitComposition { text: "z".into() },
        )
        .unwrap();
    assert_eq!(
        runtime
            .eval("field.value")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "z"
    );
    let result = runtime
        .dispatch_host_input(
            &document,
            HostInputCommand::Wheel {
                target: field,
                input: HostMouseInput::default(),
                delta: [1.0, 2.0, 3.0],
            },
        )
        .unwrap();
    assert_eq!(result.as_number(), Some(0.0));
    assert_eq!(
        runtime
            .eval("JSON.stringify(wheel)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[1,2,3,0,true]"
    );
    assert!(
        runtime
            .eval("field.setPointerCapture(7); field.hasPointerCapture(7)")
            .unwrap()
            .to_boolean()
    );
    let result = runtime
        .dispatch_host_input(
            &document,
            HostInputCommand::ReleasePointerCapture { pointer_id: 7 },
        )
        .unwrap();
    assert!(result.is_undefined());
    assert!(
        runtime
            .eval("!field.hasPointerCapture(7) && lost===0")
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn owned_drag_preserves_lifecycle_and_cancel_without_target() {
    let mut runtime = make_runtime();
    let document = runtime.document().clone();
    let source = document.query_selector("#a").unwrap();
    let target = document.query_selector("#b").unwrap();
    runtime.eval("globalThis.a=document.getElementById('a'); globalThis.b=document.getElementById('b'); a.draggable=true; globalThis.drag=[]; a.addEventListener('dragstart',e=>{drag.push('start');e.dataTransfer.setData('text/plain','payload')}); a.addEventListener('drag',()=>drag.push('drag')); a.addEventListener('dragend',()=>drag.push('end')); b.addEventListener('dragenter',()=>drag.push('enter')); b.addEventListener('dragover',e=>{drag.push('over');e.preventDefault()}); b.addEventListener('drop',e=>drag.push('drop:'+e.dataTransfer.getData('text/plain')));").unwrap();
    for command in [
        HostInputCommand::PrepareDrag {
            target: source.clone(),
            input: HostMouseInput::default(),
        },
        HostInputCommand::MoveDrag {
            target: target.clone(),
            input: HostMouseInput::default(),
        },
        HostInputCommand::EndDrag {
            target: target.clone(),
            input: HostMouseInput::default(),
        },
    ] {
        assert_eq!(
            runtime
                .dispatch_host_input(&document, command)
                .unwrap()
                .as_boolean(),
            Some(true)
        );
    }
    assert_eq!(
        runtime
            .eval("JSON.stringify(drag)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[\"start\",\"drag\",\"enter\",\"over\",\"over\",\"drop:payload\",\"end\"]"
    );
    runtime
        .dispatch_host_input(
            &document,
            HostInputCommand::PrepareDrag {
                target: source,
                input: HostMouseInput::default(),
            },
        )
        .unwrap();
    runtime
        .dispatch_host_input(
            &document,
            HostInputCommand::MoveDrag {
                target,
                input: HostMouseInput::default(),
            },
        )
        .unwrap();
    assert_eq!(
        runtime
            .dispatch_host_input(
                &document,
                HostInputCommand::CancelDrag {
                    input: HostMouseInput::default()
                }
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    assert_eq!(
        runtime
            .dispatch_host_input(
                &document,
                HostInputCommand::CancelDrag {
                    input: HostMouseInput::default()
                }
            )
            .unwrap()
            .as_boolean(),
        Some(false)
    );
}

#[test]
fn owned_input_rejects_retired_entry_and_target() {
    let mut runtime = make_runtime();
    let document = runtime.document().clone();
    runtime.eval("document.body.innerHTML='<iframe id=f srcdoc=\"<button id=old>old</button>\"></iframe>'; globalThis.frame=document.getElementById('f'); frame.contentDocument; globalThis.events=0; document.addEventListener('keydown',()=>events++);").unwrap();
    let old_document = runtime
        .host_state
        .borrow()
        .iframe_documents
        .values()
        .next()
        .unwrap()
        .document
        .clone();
    let old_target = old_document.query_selector("#old").unwrap();
    runtime
        .eval("frame.srcdoc='<button>new</button>'; frame.contentDocument;")
        .unwrap();
    assert!(
        runtime
            .dispatch_host_input(
                &old_document,
                HostInputCommand::Keyboard {
                    phase: HostKeyPhase::Down,
                    input: HostKeyInput::default()
                }
            )
            .is_err()
    );
    assert!(
        runtime
            .dispatch_host_input(
                &document,
                HostInputCommand::Mouse {
                    target: old_target,
                    phase: HostMousePhase::Click,
                    input: HostMouseInput::default(),
                    focus_target: false
                }
            )
            .is_err()
    );
    assert_eq!(runtime.eval("events").unwrap().as_number(), Some(0.0));
}

#[test]
fn frame_document_lookup_does_not_load_or_initialize_a_realm() {
    let document = crate::html::TreeBuilder::parse("<body><iframe id=f srcdoc='<button>child</button>'></iframe><button id=b>button</button></body>").document();
    let frame = document.query_selector("#f").unwrap();
    let button = document.query_selector("#b").unwrap();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    assert!(runtime.host_state.borrow().iframe_documents.is_empty());
    assert!(runtime.frame_content_document(&frame).is_none());
    assert!(runtime.frame_content_document(&button).is_none());
    assert!(runtime.host_state.borrow().iframe_documents.is_empty());
    runtime
        .eval("globalThis.frame=document.getElementById('f'); frame.contentWindow.closed")
        .unwrap();
    let expected = {
        let state = runtime.host_state.borrow();
        let entry = state.iframe_documents.get(&frame.identity()).unwrap();
        assert!(entry.realm.is_none());
        entry.document.clone()
    };
    assert!(
        runtime
            .frame_content_document(&frame)
            .is_some_and(|node| node == expected)
    );
    assert!(
        runtime.host_state.borrow().iframe_documents[&frame.identity()]
            .realm
            .is_none()
    );
    let foreign =
        JsRuntime::with_document(crate::html::TreeBuilder::parse("<iframe></iframe>").document())
            .unwrap();
    let foreign_frame = foreign.document().query_selector("iframe").unwrap();
    assert!(runtime.frame_content_document(&foreign_frame).is_none());
}

#[test]
fn frame_document_lookup_keeps_old_handle_distinct_after_navigation_and_disconnect() {
    let document = crate::html::TreeBuilder::parse(
        "<body><iframe id=f srcdoc='<button id=old>old</button>'></iframe></body>",
    )
    .document();
    let frame = document.query_selector("#f").unwrap();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime
        .eval("globalThis.frame=document.getElementById('f'); frame.contentDocument")
        .unwrap();
    let old = runtime.frame_content_document(&frame).unwrap();
    runtime
        .eval("frame.srcdoc='<button id=new>new</button>'; frame.contentDocument")
        .unwrap();
    let current = runtime.frame_content_document(&frame).unwrap();
    assert_ne!(old.identity(), current.identity());
    assert!(old.query_selector("#old").is_some());
    assert!(old.query_selector("#new").is_none());
    assert!(current.query_selector("#new").is_some());
    assert!(current.query_selector("#old").is_none());
    assert!(
        runtime
            .dispatch_host_input(
                &old,
                HostInputCommand::Keyboard {
                    phase: HostKeyPhase::Down,
                    input: HostKeyInput::default(),
                }
            )
            .is_err()
    );
    runtime.eval("frame.remove()").unwrap();
    assert!(runtime.frame_content_document(&frame).is_none());
    assert!(
        runtime
            .dispatch_host_input(
                &current,
                HostInputCommand::Keyboard {
                    phase: HostKeyPhase::Down,
                    input: HostKeyInput::default(),
                }
            )
            .is_err()
    );
}

#[test]
fn removing_ancestor_frame_rejects_retained_nested_frame_and_document() {
    let document =
        crate::html::TreeBuilder::parse("<body><iframe id=outer></iframe></body>").document();
    let outer_frame = document.query_selector("#outer").unwrap();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime
        .eval(
            r#"
        globalThis.events=0;
        globalThis.outer=document.getElementById('outer');
        globalThis.inner=outer.contentDocument.createElement('iframe');
        outer.contentDocument.body.appendChild(inner);
        inner.contentWindow.eval("document.body.innerHTML='<input id=field>'; document.onkeydown=()=>top.events++; document.getElementById('field').focus();");
    "#,
        )
        .unwrap();
    let outer_document = runtime.frame_content_document(&outer_frame).unwrap();
    let inner_frame = outer_document.query_selector("iframe").unwrap();
    let inner_document = runtime.frame_content_document(&inner_frame).unwrap();
    runtime
        .dispatch_host_input(
            &inner_document,
            HostInputCommand::Keyboard {
                phase: HostKeyPhase::Down,
                input: HostKeyInput {
                    key: "k".into(),
                    ..Default::default()
                },
            },
        )
        .unwrap();
    assert_eq!(runtime.eval("events").unwrap().as_number(), Some(1.0));
    runtime.eval("outer.remove()").unwrap();
    assert!(runtime.frame_content_document(&outer_frame).is_none());
    assert!(runtime.frame_content_document(&inner_frame).is_none());
    assert!(
        runtime
            .dispatch_host_input(
                &inner_document,
                HostInputCommand::Keyboard {
                    phase: HostKeyPhase::Down,
                    input: HostKeyInput {
                        key: "stale".into(),
                        ..Default::default()
                    },
                }
            )
            .is_err()
    );
    assert_eq!(runtime.eval("events").unwrap().as_number(), Some(1.0));
}

#[test]
fn ordinary_pages_cannot_obtain_host_input_or_synthesize_activation() {
    let mut runtime = make_runtime();
    let document = runtime.document();
    let absent = runtime
        .eval(
            r#"(() => {
        const names = [
            '__omoikane_dispatch_mouse_input', '__omoikane_dispatch_wheel_input',
            '__omoikane_dispatch_touch_input', '__omoikane_dispatch_keyboard_input',
            '__omoikane_dispatch_composition_input', '__omoikane_dispatch_drag_input',
            '__omoikane_prepare_drag_input', '__omoikane_release_pointer_capture',
        ];
        const frame = document.createElement('iframe'); document.body.appendChild(frame);
        return names.every(name => !(name in globalThis) && !(name in frame.contentWindow));
    })()"#,
        )
        .unwrap();
    assert_eq!(absent.as_boolean(), Some(true));
    runtime.eval(r#"
        globalThis.forgedInputTrace = [];
        const field = document.getElementById('field'); field.focus();
        field.onkeydown = event => forgedInputTrace.push([event.isTrusted, navigator.userActivation.hasBeenActive]);
        field.onclick = event => forgedInputTrace.push([event.isTrusted, navigator.userActivation.hasBeenActive]);
        field.dispatchEvent(new KeyboardEvent('keydown', {key:'x', bubbles:true}));
        field.dispatchEvent(new MouseEvent('click', {bubbles:true}));
        field.click();
    "#).unwrap();
    assert_eq!(
        runtime
            .eval("JSON.stringify([forgedInputTrace,navigator.userActivation.hasBeenActive])")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[[[false,false],[false,false],[false,false]],false]"
    );
    runtime
        .dispatch_host_input(
            &document,
            HostInputCommand::Keyboard {
                phase: HostKeyPhase::Down,
                input: HostKeyInput {
                    key: "h".into(),
                    ..Default::default()
                },
            },
        )
        .unwrap();
    assert_eq!(
        runtime
            .eval("JSON.stringify(forgedInputTrace[3])")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[true,true]"
    );
}
