//! Gate 3 arithmetic/property native execution and fallback integration contract.
#![cfg(feature = "baseline-jit")]

use boa_engine::{Context, Source};

fn evaluate(source: &str) -> String {
    Context::default()
        .eval(Source::from_bytes(source))
        .expect("evaluate arithmetic workload")
        .display()
        .to_string()
}

fn expected_arithmetic(iterations: i64) -> i64 {
    (0..iterations).fold(1, |sum, i| (sum + i * 3) % 1_000_003)
}

#[test]
fn issue_305_arithmetic_shape_matches_the_reference_result() {
    let iterations = 20_000;
    let result = evaluate(&format!(
        "(function(n){{var s=1;for(var i=0;i<n;i++)s=(s+i*3)%1000003;return s}})({iterations})"
    ));
    assert_eq!(result, expected_arithmetic(iterations).to_string());
}

#[test]
#[cfg(all(target_arch = "x86_64", any(target_os = "linux", target_os = "macos")))]
fn issue_305_function_reports_a_compiled_entry() {
    let mut context = Context::default();
    let result = context
        .eval(Source::from_bytes(
            "(function(n){var s=1;for(var i=0;i<n;i++)s=(s+i*3)%1000003;return s})(2000)",
        ))
        .expect("execute hot arithmetic loop");
    assert_eq!(
        result.display().to_string(),
        expected_arithmetic(2000).to_string()
    );
    let diagnostics = context.arithmetic_jit_diagnostics();
    assert_eq!(diagnostics.compile_requests, 1);
    assert_eq!(diagnostics.successful_compilations, 1);
    assert!(diagnostics.compiled_entries >= 1);
}

#[test]
#[cfg(all(target_arch = "x86_64", any(target_os = "linux", target_os = "macos")))]
fn issue_305_prop_mono_shape_uses_guarded_native_slots() {
    let iterations = 2_000_i64;
    let mut context = Context::default();
    let result = context
        .eval(Source::from_bytes(&format!(
            "(function(n){{var o={{a:1,b:2,c:3}},s=0;for(var i=0;i<n;i++){{o.b=o.a+i;s+=o.b+o.c}}return s}})({iterations})",
        )))
        .expect("execute hot monomorphic property loop");
    let expected = iterations * (iterations - 1) / 2 + 4 * iterations;
    assert_eq!(result.display().to_string(), expected.to_string());
    let diagnostics = context.arithmetic_jit_diagnostics();
    assert!(diagnostics.successful_compilations >= 1);
    assert!(diagnostics.property_guard_hits >= 1);
    assert_eq!(diagnostics.property_guard_misses, 0);
    assert_eq!(diagnostics.property_bailouts, 0);
}

#[test]
fn property_shape_and_descriptor_changes_match_interpreter_semantics() {
    assert_eq!(
        evaluate(
            "function f(o,n){var s=0;for(var i=0;i<n;i++)s+=o.x;return s}\
             var a={x:2};f(a,200);delete a.x;Object.defineProperty(a,'x',{value:7});\
             var own=f(a,1000), accessor=f({get x(){return 11}},1000);own+accessor"
        ),
        "18000"
    );
    assert_eq!(
        evaluate(
            "function f(o,n){var s=0;for(var i=0;i<n;i++)s+=o.x;return s}\
             var p={x:3},o=Object.create(p),first=f(o,200);p.x=5;first+f(o,200)"
        ),
        "1600"
    );
}

#[test]
#[cfg(all(target_arch = "x86_64", any(target_os = "linux", target_os = "macos")))]
fn property_mutation_enters_native_code_then_bails_out_safely() {
    let mut context = Context::default();
    let result = context
        .eval(Source::from_bytes(
            "function own(o,n){var s=0;for(var i=0;i<n;i++)s+=o.x;return s}\
             function inherited(o,n){var s=0;for(var i=0;i<n;i++)s+=o.x;return s}\
             var a={x:2},warmOwn=own(a,200);delete a.x;\
             Object.defineProperty(a,'x',{value:7});var changedOwn=own(a,1000);\
             var p={x:3},o=Object.create(p),warmInherited=inherited(o,200);\
             p.x=5;[warmOwn,changedOwn,warmInherited,inherited(o,200)].join(',')",
        ))
        .expect("execute property mutations after hot native entries");
    assert_eq!(result.display().to_string(), "\"400,7000,600,1000\"");

    let diagnostics = context.arithmetic_jit_diagnostics();
    assert!(diagnostics.successful_compilations >= 1);
    assert!(diagnostics.property_guard_hits >= 1);
    assert!(diagnostics.property_guard_misses >= 1);
}

#[test]
fn overflow_nan_negative_zero_branch_and_type_mismatch_preserve_number_semantics() {
    assert_eq!(
        evaluate(
            "function f(n,s){for(var i=0;i<n;i++)s=s+i*3;return s}\
             f(200,1); f(100,2147483640)"
        ),
        "2147498490"
    );
    assert_eq!(
        evaluate(
            "function f(n){var s=1;for(var i=0;i<n;i++)s=(s+i*3)%17;return s}\
             f(200); f(NaN)*100+f('40')"
        ),
        (100 + (0..40).fold(1, |sum, i| (sum + i * 3) % 17)).to_string()
    );
    assert_eq!(
        evaluate(
            "function f(n,s){for(var i=0;i<n;i++)s=s%3;return s}\
             f(200,1); Object.is(f(100,-0),-0)"
        ),
        "true"
    );
    assert_eq!(
        evaluate(
            "function f(n){var z=1,b=false,zero=0,neg=-1;\
             for(var i=0;i<n;i++){z=zero*neg;b=i<3}\
             return Object.is(z,-0) && typeof b === 'boolean'} f(200)"
        ),
        "true"
    );
    assert_eq!(
        evaluate(
            "function f(n){var s=0;for(var i=0;i<n;i++){if(i<50)s=s+2;else s=s-1}return s}\
             f(200)"
        ),
        "-50"
    );
}
