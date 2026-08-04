//! Gate 4-3 integration contract for generational JIT frame root scanning.

#[cfg(feature = "baseline-jit")]
mod enabled {
    #[cfg(not(all(target_arch = "x86_64", any(target_os = "linux", target_os = "macos"))))]
    use boa_engine::jit::RuntimeCallError;
    use boa_engine::{
        jit::JitRuntimeCall, js_string, property::Attribute, Context, JsObject, JsValue, Source,
    };

    #[test]
    #[cfg(not(all(target_arch = "x86_64", any(target_os = "linux", target_os = "macos"))))]
    fn unsupported_target_reports_an_error_instead_of_running_zero_tests() {
        assert!(matches!(
            JitRuntimeCall::new(1, 1),
            Err(RuntimeCallError::Jit(_))
        ));
    }

    #[test]
    #[cfg(all(target_arch = "x86_64", any(target_os = "linux", target_os = "macos")))]
    fn jit_only_root_survives_minor_and_major_then_weak_ref_clears() {
        let mut context = Context::default();
        let runtime = JitRuntimeCall::new(1, 1).expect("compile runtime-call boundary");
        let target = context
            .eval(Source::from_bytes(
                "globalThis.weak = new WeakRef({marker: 42}); weak.deref()",
            ))
            .expect("create weakly observed target")
            .as_object()
            .expect("target object");
        // WeakRef construction/deref adds the target to the current job's kept
        // objects. Clear that host root so only the generated frame spill keeps
        // it alive during the collections below.
        context.clear_kept_objects();

        for _ in 0..2 {
            let returned = runtime
                .collect_minor_for_test(&[target.clone().into()], &mut context)
                .expect("enter minor-collection helper")
                .expect("minor-collection result");
            assert!(JsObject::equals(&returned.as_object().unwrap(), &target));
        }
        let returned = runtime
            .collect_major_for_test(&[target.clone().into()], &mut context)
            .expect("enter major-collection helper")
            .expect("major-collection result");
        assert!(JsObject::equals(&returned.as_object().unwrap(), &target));
        assert_eq!(
            context
                .eval(Source::from_bytes("weak.deref().marker"))
                .expect("read live weak target"),
            JsValue::from(42),
        );

        drop(returned);
        drop(target);
        context.clear_kept_objects();
        runtime
            .collect_major_for_test(&[], &mut context)
            .expect("collect unreachable target")
            .expect("empty collection result");
        assert_eq!(
            context
                .eval(Source::from_bytes("weak.deref() === undefined"))
                .expect("observe cleared weak target"),
            JsValue::from(true),
        );
    }

    #[test]
    #[cfg(all(target_arch = "x86_64", any(target_os = "linux", target_os = "macos")))]
    fn old_to_young_store_and_nested_frames_survive_collection() {
        let mut context = Context::default();
        let runtime = JitRuntimeCall::new(1, 0).expect("compile runtime-call boundary");
        let parent = JsObject::with_null_proto();
        for _ in 0..2 {
            runtime
                .collect_minor_for_test(&[parent.clone().into()], &mut context)
                .expect("enter parent promotion helper")
                .expect("parent promotion result");
        }

        let child = JsObject::with_null_proto();
        parent
            .set(js_string!("child"), child.clone(), true, &mut context)
            .expect("old-to-young property store");
        drop(child);
        let nested = runtime
            .nested_allocate_for_test(
                boa_engine::jit::JitAllocationKind::Array,
                &[parent.clone().into()],
                &mut context,
            )
            .expect("nested generated allocation")
            .expect("nested allocation result");
        let retained_parent = nested
            .as_object()
            .unwrap()
            .get(0, &mut context)
            .expect("retained parent")
            .as_object()
            .expect("parent object");
        assert!(JsObject::equals(&retained_parent, &parent));
        assert!(retained_parent
            .get(js_string!("child"), &mut context)
            .expect("remembered young child")
            .is_object());

        context
            .register_global_property(js_string!("afterGc"), 42, Attribute::all())
            .unwrap();
        assert_eq!(
            context.eval(Source::from_bytes("afterGc")).unwrap(),
            JsValue::from(42),
        );
    }
}
