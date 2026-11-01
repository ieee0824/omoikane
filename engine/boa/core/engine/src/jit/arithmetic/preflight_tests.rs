use crate::{Context, Script, vm::Constant};
use boa_parser::Source;
use std::cell::Cell;

#[test]
fn unsupported_loop_does_not_copy_unrelated_function_contracts() {
    use crate::vm::bytecode_contract::{ROOT_VERIFICATION_VISITS, SNAPSHOT_CONSTANT_VISITS};

    for functions in [32, 64, 128] {
        let definitions = (0..functions)
            .map(|i| format!("function unused{i}(){{return {i};}}"))
            .collect::<String>();
        let source = format!(
            "function probe(n){{{definitions}var s=0;for(var i=0;i<n;i++)s+=Math.abs(i);return s}}probe(16)"
        );
        let mut context = Context::default();
        context.set_baseline_jit_enabled(false);
        let script = Script::parse(Source::from_bytes(&source), None, &mut context).unwrap();
        let outer = script.codeblock(&mut context).unwrap();
        let index = outer
            .constants
            .iter()
            .position(|constant| matches!(constant, Constant::Function(_)))
            .unwrap();
        let function = outer.constant_function(index);
        SNAPSHOT_CONSTANT_VISITS.with(|count| count.set(0));
        let full = function.bytecode_contract().verify().unwrap();
        assert!(SNAPSHOT_CONSTANT_VISITS.with(Cell::get) >= functions);
        assert!(
            full.instructions
                .iter()
                .any(|i| i.name == "IncrementLoopIteration")
        );
        context.set_baseline_jit_enabled(true);
        // Warm runtime helpers below the arithmetic tier's 32-iteration
        // threshold so its first request is measured separately.
        assert_eq!(
            script.evaluate(&mut context).unwrap().as_number(),
            Some(120.0)
        );
        assert_eq!(context.arithmetic_jit_diagnostics().compile_requests, 0);
        SNAPSHOT_CONSTANT_VISITS.with(|count| count.set(0));
        ROOT_VERIFICATION_VISITS.with(|count| count.set(0));
        assert_eq!(
            context
                .eval(Source::from_bytes("probe(100)"))
                .unwrap()
                .as_number(),
            Some(4950.0)
        );
        assert_eq!(context.arithmetic_jit_diagnostics().compile_rejections, 1);
        assert_eq!(SNAPSHOT_CONSTANT_VISITS.with(Cell::get), 0);
        assert_eq!(ROOT_VERIFICATION_VISITS.with(Cell::get), 0);
    }
}

#[test]
fn supported_loop_with_nested_function_still_enters_native_code() {
    let source = "function f(n){function nested(){return 17;}var s=0;for(var i=0;i<n;i++)s+=i;return s+nested()}f(200)";
    for enabled in [false, true] {
        let mut context = Context::default();
        context.set_baseline_jit_enabled(enabled);
        let result = Script::parse(Source::from_bytes(source), None, &mut context)
            .unwrap()
            .evaluate(&mut context)
            .unwrap();
        assert_eq!(result.as_number(), Some(19917.0));
        assert_eq!(
            context.arithmetic_jit_diagnostics().compiled_entries > 0,
            enabled
        );
    }
}
