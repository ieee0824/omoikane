//! Downstream integration check for Boa's Gate 2 bytecode boundary.

use boa_engine::{
    Context, Script, Source,
    vm::{BYTECODE_CONTRACT_VERSION, BytecodeConstant},
};

const BENCHMARK_SHAPES: &str = include_str!("js_benchmark/shapes.js");

#[test]
fn current_benchmark_shapes_compile_through_the_verified_contract() {
    let mut context = Context::default();
    let script = Script::parse(Source::from_bytes(BENCHMARK_SHAPES), None, &mut context)
        .expect("parse current JS benchmark shapes");
    let code = script
        .codeblock(&mut context)
        .expect("compile current JS benchmark shapes");

    let snapshot = code
        .bytecode_contract()
        .verify()
        .expect("verify current JS benchmark bytecode and nested functions");
    assert_eq!(snapshot.version, BYTECODE_CONTRACT_VERSION);
    assert!(!snapshot.instructions.is_empty());
    assert!(snapshot.constants.iter().any(|constant| {
        matches!(constant, BytecodeConstant::Function { contract, .. } if !contract.instructions.is_empty())
    }));

    let first = code.bytecode_contract().dump().expect("first stable dump");
    let second = code.bytecode_contract().dump().expect("second stable dump");
    assert_eq!(first, second);
}
