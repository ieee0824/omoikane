//! Downstream integration checks for Boa's Gate 2 inline-cache boundary.

use boa_engine::{vm::InlineCacheState, Context, Script, Source};

#[test]
fn property_call_site_exposes_stable_state_and_opt_in_counters() {
    let mut context = Context::default();
    let script = Script::parse(
        Source::from_bytes(
            r#"
            const first = { value: 1, first: true };
            const second = { second: true, value: 2 };
            const third = { third: true, extra: true, value: 3 };
            let total = 0;
            for (let index = 0; index < 30; index++) {
                const object = index % 3 === 0
                    ? first
                    : (index % 3 === 1 ? second : third);
                total += object.value;
            }
            total;
            "#,
        ),
        None,
        &mut context,
    )
    .expect("parse inline-cache probe");
    let code = script
        .codeblock(&mut context)
        .expect("compile inline-cache probe");

    let value_slot = code
        .inline_cache_metadata()
        .into_iter()
        .find(|slot| slot.name.to_std_string_escaped() == "value")
        .expect("property bytecode should expose a stable value IC slot");
    assert!(!value_slot.telemetry_enabled);
    assert_eq!(value_slot.state, InlineCacheState::Empty);
    let stable_index = value_slot.index;

    code.set_inline_cache_telemetry_enabled(true);
    let result = script
        .evaluate(&mut context)
        .expect("execute inline-cache probe");
    assert_eq!(result.as_number(), Some(60.0));

    let warmed = code
        .inline_cache_metadata()
        .into_iter()
        .find(|slot| slot.index == stable_index)
        .expect("the same IC index should remain observable after execution");
    assert_eq!(warmed.name.to_std_string_escaped(), "value");
    assert_eq!(warmed.state, InlineCacheState::Polymorphic);
    assert_eq!(warmed.live_entries, 3);
    assert_eq!(warmed.misses, 3);
    assert_eq!(warmed.installs, 3);
    assert_eq!(warmed.hits, 27);
    assert_eq!(warmed.replacements, 0);

    code.reset_inline_cache_telemetry();
    let reset = code
        .inline_cache_metadata()
        .into_iter()
        .find(|slot| slot.index == stable_index)
        .expect("reset should preserve the warmed IC slot");
    assert_eq!(reset.state, InlineCacheState::Polymorphic);
    assert_eq!((reset.hits, reset.misses, reset.installs), (0, 0, 0));
}
