//! Gate 3-2 integration contract for verified baseline lowering and dispatch.

use boa_engine::{Context, Source};

fn evaluate(source: &str) -> String {
    Context::default()
        .eval(Source::from_bytes(source))
        .expect("evaluate workload")
        .display()
        .to_string()
}

#[test]
fn default_and_feature_builds_preserve_interpreter_semantics() {
    assert_eq!(
        evaluate("var s=1; for(var i=0;i<20;i++) s=(s+i*3)%17; s"),
        "10"
    );
    assert_eq!(
        evaluate("var o={x:1}; for(var i=0;i<8;i++) o.x=o.x+i; o.x"),
        "29"
    );
}

#[cfg(feature = "baseline-jit")]
mod enabled {
    use boa_engine::jit::{BaselineBlockKind, BaselineIr, BytecodeCodeMap};
    use boa_engine::{Context, Script, Source};

    #[cfg(all(target_arch = "x86_64", any(target_os = "linux", target_os = "macos")))]
    use boa_engine::jit::{
        BaselineController, BaselineEntry, CompileDecision, JitCacheKey, JitCodeCache,
    };
    #[cfg(all(target_arch = "x86_64", any(target_os = "linux", target_os = "macos")))]
    use boa_engine::vm::BYTECODE_CONTRACT_VERSION;

    fn lower(source: &str) -> BaselineIr {
        let mut context = Context::default();
        let script =
            Script::parse(Source::from_bytes(source), None, &mut context).expect("parse workload");
        let snapshot = script
            .codeblock(&mut context)
            .expect("compile workload")
            .bytecode_contract()
            .verify()
            .expect("verify bytecode contract");
        BaselineIr::lower(&snapshot).expect("lower verified bytecode")
    }

    #[test]
    fn mixed_property_and_arithmetic_workload_has_explicit_fallback_boundaries() {
        let ir = lower("var o={x:1}; for(var i=0;i<8;i++) o.x=o.x+i; o.x");
        assert!(
            ir.blocks
                .iter()
                .any(|block| matches!(block.kind, BaselineBlockKind::Compilable))
        );
        assert!(
            ir.blocks
                .iter()
                .any(|block| matches!(block.kind, BaselineBlockKind::InterpreterFallback { .. }))
        );
        assert!(ir.dump().contains("GetPropertyByName"));
        assert!(ir.dump().contains("Add"));
        for block in &ir.blocks {
            for successor in &block.successors {
                assert!(ir.blocks.iter().any(|candidate| candidate.id == *successor));
            }
        }
    }

    #[test]
    fn exception_handlers_are_explicit_cfg_successors() {
        let ir = lower("try { throw 1 } catch (error) { error + 1 }");
        assert!(
            ir.blocks
                .iter()
                .any(|block| !block.exception_successors.is_empty())
        );
        for block in &ir.blocks {
            for successor in &block.exception_successors {
                assert!(ir.blocks.iter().any(|candidate| candidate.id == *successor));
            }
        }
    }

    #[test]
    fn code_map_is_stable_and_rejects_duplicate_source_offsets() {
        let mut map = BytecodeCodeMap::default();
        map.push(0, 0).expect("first mapping");
        map.push(5, 11).expect("second mapping");
        assert!(map.push(5, 12).is_err());
        assert_eq!(
            map.dump(),
            "bytecode-to-machine-offset\n  bc=000000 machine=+0x000000\n  bc=000005 machine=+0x00000b\n"
        );
    }

    #[cfg(all(target_arch = "x86_64", any(target_os = "linux", target_os = "macos")))]
    #[test]
    fn hotness_entry_invalidation_and_recompile_use_generation_checked_code() {
        let key = JitCacheKey {
            code_id: 530,
            version: 1,
        };
        let mut cache = JitCodeCache::new();
        let mut controller = BaselineController::new(2);
        assert_eq!(controller.enter(), CompileDecision::Interpret);
        assert_eq!(controller.enter(), CompileDecision::CompileNow);

        let first = cache.compile_fixed_return(key, 1).expect("first entry");
        controller
            .install(BaselineEntry {
                handle: first,
                contract_version: BYTECODE_CONTRACT_VERSION,
                code_map: BytecodeCodeMap::default(),
            })
            .expect("install first entry");
        assert_eq!(controller.enter(), CompileDecision::EnterCompiled(first));

        assert_eq!(controller.invalidate(), Some(first));
        assert!(cache.invalidate(key));
        assert_eq!(controller.enter(), CompileDecision::CompileNow);
        let second = cache
            .compile_fixed_return(key, 2)
            .expect("replacement entry");
        controller
            .install(BaselineEntry {
                handle: second,
                contract_version: BYTECODE_CONTRACT_VERSION,
                code_map: BytecodeCodeMap::default(),
            })
            .expect("install replacement entry");
        assert_eq!(cache.call_fixed_return(second).unwrap(), 2);
        assert_eq!(controller.diagnostics().compile_requests, 2);
        assert_eq!(controller.diagnostics().invalidations, 1);
    }
}
