//! Gate 4-2 integration contract for generated runtime calls and allocation.

use boa_engine::{Context, JsValue, Source};

fn evaluate(source: &str) -> JsValue {
    Context::default()
        .eval(Source::from_bytes(source))
        .expect("evaluate interpreter workload")
}

#[test]
fn interpreter_allocation_and_exception_semantics_remain_unchanged() {
    assert_eq!(
        evaluate(
            "var o={x:1}; var a=[o,2]; var f=()=>a[0].x+a[1]; \
             try { if (f()===3) throw 4 } catch (e) { e+f() }",
        ),
        JsValue::from(7),
    );
}

#[cfg(feature = "baseline-jit")]
mod enabled {
    use boa_engine::{
        jit::{JitAllocationKind, JitRuntimeCall, RuntimeCallError},
        JsObject,
    };

    use super::*;

    #[test]
    #[cfg(all(target_arch = "x86_64", any(target_os = "linux", target_os = "macos")))]
    fn generated_calls_allocate_return_and_preserve_live_values_across_gc() {
        let mut context = Context::default();
        let runtime = JitRuntimeCall::new(2, 1).expect("compile runtime-call boundary");
        let live = JsObject::with_null_proto();

        let object = runtime
            .allocate(JitAllocationKind::Object, &[], &mut context)
            .expect("enter object helper")
            .expect("object allocation");
        assert!(object.is_object());

        // The one fast-path credit is exhausted, so this nested allocation
        // enters the collector slow path while `live` is in a frame spill slot.
        let array = runtime
            .nested_allocate_for_test(
                JitAllocationKind::Array,
                &[live.clone().into()],
                &mut context,
            )
            .expect("enter nested array helper")
            .expect("array allocation");
        let array = array.as_object().expect("array result");
        assert!(JsObject::equals(
            &array
                .get(0, &mut context)
                .expect("array element")
                .as_object()
                .unwrap(),
            &live,
        ));

        let closure = runtime
            .allocate(JitAllocationKind::Closure, &[], &mut context)
            .expect("enter closure helper")
            .expect("closure allocation");
        assert!(closure.as_object().unwrap().is_callable());

        let diagnostics = runtime.diagnostics();
        assert_eq!(diagnostics.generated_calls, 4);
        assert_eq!(diagnostics.nested_calls, 1);
        assert!(diagnostics.fast_allocations >= 1);
        assert!(diagnostics.slow_allocations >= 1);
    }

    #[test]
    #[cfg(all(target_arch = "x86_64", any(target_os = "linux", target_os = "macos")))]
    fn exception_and_allocation_failure_return_without_corrupting_interpreter() {
        let mut context = Context::default();
        let mut runtime = JitRuntimeCall::new(0, 1).unwrap();

        assert!(runtime.throw_for_test(&mut context).unwrap().is_err());
        runtime.set_allocation_budget(Some(0));
        assert!(matches!(
            runtime.allocate(JitAllocationKind::Object, &[], &mut context),
            Err(RuntimeCallError::AllocationFailure)
        ));
        assert_eq!(
            context.eval(Source::from_bytes("21 * 2")).unwrap(),
            JsValue::from(42)
        );
    }

    #[test]
    #[cfg(not(all(target_arch = "x86_64", any(target_os = "linux", target_os = "macos"))))]
    fn unsupported_target_reports_an_error_instead_of_running_zero_tests() {
        assert!(matches!(
            JitRuntimeCall::new(0, 1),
            Err(RuntimeCallError::Jit(_))
        ));
    }
}
