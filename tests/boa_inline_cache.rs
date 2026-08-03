//! Regression test for issue 058: Boa inline-cache poisoning.
//!
//! Boa 0.21.1's property inline cache guards a cached *prototype-property* slot
//! only by the receiver object's shape. When a prototype is mutated (a property
//! stored *before* the cached one is deleted) after a call site has been
//! warmed, the receiver's shape is unchanged but the cached slot index into the
//! prototype's storage becomes stale. The warm call site then resolves to a
//! different method.
//!
//! The real-world trigger (tokyo6.tokyo) was a core-js + webpack bundle that
//! deletes/redefines several `String.prototype` methods after a warm
//! `"s".codePointAt(0)` call site, so the stale slot resolved to
//! `String.prototype.concat` and `"s".codePointAt(0)` returned the string
//! `"s0"` instead of the number `115`.
//!
//! This test drives a hand-written, credential-free synthetic reproduction
//! (`fixtures/js/ic_prototype_reindex.js`) that hits the exact same mechanism:
//! a warm prototype-method call site whose prototype storage is reindexed by
//! deletes. Before the fix the warm site returns `"s0"`; after the fix it must
//! still return `115`.

use omoikane::js::JsRuntime;

/// Hand-written minimal reproduction. No third-party code or credentials; see
/// the file header for the mechanism it exercises.
const IC_REINDEX_JS: &str = include_str!("fixtures/js/ic_prototype_reindex.js");

#[test]
fn warm_call_site_survives_prototype_reindex() {
    let mut rt = JsRuntime::new().expect("runtime should build");

    // Run the reproduction: it warms a `obj.target()` prototype-method call
    // site (returning 115), then reindexes the prototype so a stale inline
    // cache would resolve `target` to a sibling returning "s0".
    rt.eval(IC_REINDEX_JS)
        .expect("reproduction fixture should evaluate without throwing");

    // Guard against a false pass: the fixture sets this sentinel only after the
    // prototype mutation completes. If warming or mutation had thrown early the
    // prototype would be un-mutated and re-invoking the warm site would trivially
    // return 115 without ever exercising the stale-slot path.
    let mutated = rt
        .eval("globalThis.__issue058Mutated === true")
        .expect("sentinel read should succeed");
    assert_eq!(
        mutated.as_boolean(),
        Some(true),
        "fixture did not reach the post-mutation sentinel; the reproduction did \
         not run to completion so this test cannot detect the bug"
    );

    // Re-invoke the *same* warm call site after the reindex. Before the fix this
    // returns the string "s0" (the reindexed sibling method); after the fix it
    // must still resolve `target` and return 115.
    let after = rt
        .eval("globalThis.__issue058Victim()")
        .expect("victim re-eval should succeed");

    assert_eq!(
        after.as_number(),
        Some(115.0),
        "inline-cache poisoning: the warm prototype-method call site resolved to \
         the wrong method after the prototype was reindexed (got {after:?})"
    );
}

#[test]
fn shape_transitions_keep_own_prototype_and_accessor_semantics() {
    let mut rt = JsRuntime::new().expect("runtime should build");
    rt.eval(
        r#"
        (() => {
          const prototype = { value: 4 };
          const receiver = Object.create(prototype);
          const read = () => receiver.value;
          const write = value => { receiver.value = value; return receiver.value; };

          const fromPrototype = read();
          const ownValue = write(9);
          delete receiver.value;
          prototype.value = 12;
          const afterPrototypeMutation = read();

          const accessor = { _value: 2 };
          Object.defineProperty(accessor, 'value', {
            configurable: true,
            get() { return this._value; },
            set(value) { this._value = value; },
          });
          const accessorBefore = accessor.value;
          accessor.value = 7;

          globalThis.__issue525Result = {
            values: [fromPrototype, ownValue, afterPrototypeMutation],
            accessor: [accessorBefore, accessor.value],
          };
        })();
        "#,
    )
    .expect("shape transition fixture should evaluate without throwing");

    let result = rt
        .eval("JSON.stringify(globalThis.__issue525Result)")
        .expect("shape transition result should serialize")
        .as_string()
        .expect("shape transition result should be a string")
        .to_std_string_escaped();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&result).expect("valid shape result JSON"),
        serde_json::json!({
            "values": [4, 9, 12],
            "accessor": [2, 7]
        })
    );
}

#[test]
fn shared_shape_branches_do_not_cross_contaminate_property_reads() {
    let mut rt = JsRuntime::new().expect("runtime should build");
    rt.eval(
        r#"
        (() => {
          const left = { value: 1 };
          const right = { value: 2 };
          const read = object => object.value;
          const warm = [read(left), read(right), read(left), read(right)];
          left.extra = 3;
          right.other = 4;
          globalThis.__issue525Shapes = [
            warm,
            read(left),
            read(right),
            left.extra,
            right.other,
          ];
        })();
        "#,
    )
    .expect("shape branch fixture should evaluate without throwing");

    let result = rt
        .eval("JSON.stringify(globalThis.__issue525Shapes)")
        .expect("shape branch result should serialize")
        .as_string()
        .expect("shape branch result should be a string")
        .to_std_string_escaped();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&result).expect("valid shape branch JSON"),
        serde_json::json!([[1, 2, 1, 2], 1, 2, 3, 4])
    );
}
