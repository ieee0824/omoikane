//! Gate 3-1 integration contract for Boa's opt-in native code substrate.
//!
//! This test does not route JavaScript through JIT code. It verifies only the
//! W^X publication, entry ABI, cache lifetime, and runtime separation needed by
//! later lowering work.

#![cfg(all(
    feature = "baseline-jit",
    target_arch = "x86_64",
    any(target_os = "linux", target_os = "macos")
))]

use boa_engine::jit::{CodePermission, JIT_ABI, JitCacheKey, JitCodeCache, JitError};

fn key(code_id: u64, version: u32) -> JitCacheKey {
    JitCacheKey { code_id, version }
}

#[test]
fn fixed_stub_uses_the_frozen_abi_and_rx_mapping() {
    assert_eq!(JIT_ABI, "System V AMD64: extern C fn() -> u64");
    let mut cache = JitCodeCache::new();
    let handle = cache
        .compile_fixed_return(key(1, 1), 0x0123_4567_89AB_CDEF)
        .expect("compile fixed-return stub");

    assert_eq!(
        cache.call_fixed_return(handle).expect("enter JIT stub"),
        0x0123_4567_89AB_CDEF
    );
    let (permission, mapped_len) = cache.diagnostics(handle).expect("live diagnostics");
    assert_eq!(permission, CodePermission::ReadExecute);
    assert!(mapped_len >= 11);
}

#[test]
fn replacement_and_invalidation_cannot_reuse_stale_code() {
    let mut cache = JitCodeCache::new();
    let old = cache
        .compile_fixed_return(key(2, 1), 11)
        .expect("compile old generation");
    let replacement = cache
        .compile_fixed_return(key(2, 1), 22)
        .expect("compile replacement generation");

    assert!(matches!(
        cache.call_fixed_return(old),
        Err(JitError::StaleCodeHandle)
    ));
    assert_eq!(cache.call_fixed_return(replacement).unwrap(), 22);
    assert!(cache.invalidate(key(2, 1)));
    assert!(matches!(
        cache.call_fixed_return(replacement),
        Err(JitError::StaleCodeHandle)
    ));
}

#[test]
fn executable_code_is_runtime_local() {
    let mut first_runtime = JitCodeCache::new();
    let mut second_runtime = JitCodeCache::new();
    let first = first_runtime
        .compile_fixed_return(key(3, 1), 31)
        .expect("compile first runtime");
    let second = second_runtime
        .compile_fixed_return(key(3, 1), 32)
        .expect("compile second runtime");

    assert_eq!(first_runtime.call_fixed_return(first).unwrap(), 31);
    assert_eq!(second_runtime.call_fixed_return(second).unwrap(), 32);
    assert!(matches!(
        first_runtime.call_fixed_return(second),
        Err(JitError::StaleCodeHandle)
    ));
    assert!(matches!(
        second_runtime.call_fixed_return(first),
        Err(JitError::StaleCodeHandle)
    ));
}
