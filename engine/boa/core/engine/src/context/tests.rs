use super::Context;
use crate::Source;

#[test]
fn repeated_weak_dereferences_keep_each_identity_once_until_job_end() {
    let mut context = Context::default();
    context
        .eval(Source::from_bytes(
            r#"
            var first = new WeakRef({ value: 7 });
            var second = new WeakRef({ value: 7 });
            for (var i = 0; i < 4096; i++) {
                first.deref();
                second.deref();
            }
            "#,
        ))
        .unwrap();
    assert_eq!(
        context.roots.kept_alive.len(),
        2,
        "repeated dereferences must not multiply the job's strong GC roots"
    );
    boa_gc::force_collect();
    assert_eq!(
        context
            .eval(Source::from_bytes(
                "first.deref().value === 7 && second.deref().value === 7 && first.deref() !== second.deref()",
            ))
            .unwrap()
            .as_boolean(),
        Some(true),
        "distinct targets stay alive until the host clears the job's kept objects"
    );
    context.clear_kept_objects();
    boa_gc::force_collect();
    assert_eq!(
        context
            .eval(Source::from_bytes(
                "first.deref() === undefined && second.deref() === undefined",
            ))
            .unwrap()
            .as_boolean(),
        Some(true),
        "the identity index must not extend a target's lifetime beyond the job"
    );
}
