//! The pinned Boa parser must preserve small-stack parsing through the public runtime.

use omoikane::js::JsRuntime;

fn check_runtime(stack_bytes: usize) {
    std::thread::Builder::new()
        .name(format!("js-parser-{stack_bytes}"))
        .stack_size(stack_bytes)
        .spawn(|| {
            let mut runtime = JsRuntime::new().expect("runtime initialization");
            let source = format!(
                "function f() {{ return {} function (a) {{
                    var v = a;
                    if (Number(v) !== 42) throw new Error('unexpected value');
                    return function () {{ return v; }};
                }} {} f(){}(42)();",
                "function () { return ".repeat(14),
                "};".repeat(15),
                "()".repeat(14),
            );
            assert_eq!(runtime.eval(&source).unwrap().as_number(), Some(42.0));

            let too_deep = format!("{}0{}", "[".repeat(8192), "]".repeat(8192));
            let error = runtime
                .eval(&too_deep)
                .expect_err("excessive parser recursion");
            let native = error.as_native().expect("native parser error");
            assert!(native.is_syntax());
            assert!(native.message().contains("parser recursion limit exceeded"));
            assert_eq!(runtime.eval("6 * 7").unwrap().as_number(), Some(42.0));
        })
        .expect("spawn explicit-stack runtime thread")
        .join()
        .expect("runtime must return without aborting or panicking");
}

#[test]
fn javascript_parser_on_2_mib_stack() {
    check_runtime(2 * 1024 * 1024);
}

#[test]
fn javascript_parser_on_8_mib_stack() {
    check_runtime(8 * 1024 * 1024);
}
