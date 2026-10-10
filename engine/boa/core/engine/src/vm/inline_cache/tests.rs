use boa_gc::{Rooted, force_collect};
use boa_parser::Source;

use crate::{
    Context, JsObject, JsResult, JsString, JsValue,
    builtins::{OrdinaryObject, function::OrdinaryFunction},
    js_string,
    object::{
        ObjectInitializer,
        internal_methods::InternalMethodPropertyContext,
        shape::{RootedWeakShape, slot::SlotAttributes},
    },
    property::{Attribute, PropertyDescriptor, PropertyKey},
    vm::CodeBlock,
};

#[test]
fn get_own_property_internal_method() {
    let context = &mut Context::default();

    let o = context
        .intrinsics()
        .templates()
        .ordinary_object()
        .create(OrdinaryObject, Vec::default());
    let _o_root = o.clone().root();

    let property: PropertyKey = js_string!("prop").into();
    let value = 100;

    o.set(property.clone(), value, true, context)
        .expect("should not fail");

    let context = &mut InternalMethodPropertyContext::new(context);

    assert_eq!(context.slot().index, 0);
    assert_eq!(context.slot().attributes, SlotAttributes::empty());

    o.__get_own_property__(&property, context)
        .expect("should not fail");

    assert!(
        !context.slot().in_prototype(),
        "Since it's an owned property, the prototype bit should not be set"
    );

    assert!(
        context.slot().is_cachable(),
        "Since it's an owned property, this should be cachable"
    );

    let shape = o.borrow().shape().clone();

    let slot = shape.lookup(&property);

    assert!(slot.is_some(), "the property should be found in the object");

    let slot = slot.expect("the property should be found in the object");

    assert_eq!(context.slot().index, slot.index);
}

#[test]
fn property_inline_cache_retains_multiple_live_shapes() -> JsResult<()> {
    let context = &mut Context::default();
    let function = context.eval(Source::from_bytes("(function (o) { return o.value; })"))?;
    let (function, code) = get_codeblock(&function).unwrap();
    let _function_root = function.clone().root();
    assert_eq!(code.ic.len(), 1);
    code.set_inline_cache_telemetry_enabled(true);

    let objects = [
        ObjectInitializer::new(context)
            .property(js_string!("value"), 1, Attribute::all())
            .property(js_string!("first"), 2, Attribute::all())
            .build(),
        ObjectInitializer::new(context)
            .property(js_string!("first"), 2, Attribute::all())
            .property(js_string!("value"), 3, Attribute::all())
            .build(),
        ObjectInitializer::new(context)
            .property(js_string!("second"), 4, Attribute::all())
            .property(js_string!("value"), 5, Attribute::all())
            .build(),
    ];
    let _object_roots: Vec<_> = objects.iter().map(|object| object.clone().root()).collect();

    for (object, expected) in objects.iter().zip([1, 3, 5]) {
        let value = function.call(&JsValue::undefined(), &[object.clone().into()], context)?;
        assert_eq!(value.as_number(), Some(f64::from(expected)));
    }

    // The cache keeps its ephemeron allocations rooted even when the code
    // block was already promoted before the cache was warmed.
    force_collect();

    assert_ne!(code.ic[0].shape.borrow().to_addr_usize(), 0);
    assert_eq!(code.ic[0].secondary_shape_count(), 2);

    // Revisit the first shape after warming the secondary entries. This proves
    // the primary entry remains usable instead of being cleared on a miss.
    let value = function.call(&JsValue::undefined(), &[objects[0].clone().into()], context)?;
    assert_eq!(value.as_number(), Some(1.0));

    let metadata = code.inline_cache_metadata();
    assert_eq!(metadata.len(), 1);
    assert_eq!(metadata[0].index, 0);
    assert_eq!(metadata[0].state, super::InlineCacheState::Polymorphic);
    assert_eq!(metadata[0].live_entries, 3);
    assert_eq!(metadata[0].capacity, 8);
    assert_eq!(metadata[0].hits, 1);
    assert_eq!(metadata[0].misses, 3);
    assert_eq!(metadata[0].installs, 3);
    assert_eq!(metadata[0].replacements, 0);

    code.reset_inline_cache_telemetry();
    let reset = code.inline_cache_metadata();
    assert_eq!(
        (reset[0].hits, reset[0].misses, reset[0].installs),
        (0, 0, 0)
    );
    assert_eq!(reset[0].state, super::InlineCacheState::Polymorphic);

    Ok(())
}

#[test]
fn metadata_excludes_entry_with_dead_prototype_guard() {
    let context = &mut Context::default();
    let receiver = ObjectInitializer::new(context).build();
    let receiver_shape = receiver.borrow().shape_edge().clone();
    let cache = super::InlineCache::new(js_string!("value"));

    *cache.shape.borrow_mut() = RootedWeakShape::from(&receiver_shape);
    cache.slot.set(crate::object::shape::slot::Slot {
        attributes: SlotAttributes::PROTOTYPE,
        ..crate::object::shape::slot::Slot::new()
    });

    let metadata = cache.metadata(0);
    assert_eq!(metadata.live_entries, 0);
    assert_eq!(metadata.state, super::InlineCacheState::Empty);
}

#[test]
fn inline_cache_reports_megamorphic_replacement_and_hits() -> JsResult<()> {
    let context = &mut Context::default();
    let function = context.eval(Source::from_bytes(
        "(function (object) { return object.value; })",
    ))?;
    let (function, code) = get_codeblock(&function).unwrap();
    let _function_root = function.clone().root();
    code.set_inline_cache_telemetry_enabled(true);
    let mut objects = Vec::new();
    for index in 0..9 {
        objects.push(
            ObjectInitializer::new(context)
                .property(
                    JsString::from(format!("shape{index}")),
                    index,
                    Attribute::all(),
                )
                .property(js_string!("value"), index + 10, Attribute::all())
                .build(),
        );
    }
    let _object_roots: Vec<_> = objects.iter().map(|object| object.clone().root()).collect();
    for (index, object) in objects.iter().enumerate() {
        let value = function.call(&JsValue::undefined(), &[object.clone().into()], context)?;
        assert_eq!(value.as_number(), Some(index as f64 + 10.0));
    }
    let value = function.call(&JsValue::undefined(), &[objects[8].clone().into()], context)?;
    assert_eq!(value.as_number(), Some(18.0));

    let metadata = code.inline_cache_metadata();
    assert_eq!(metadata.len(), 1);
    assert_eq!(metadata[0].index, 0);
    assert_eq!(metadata[0].state, super::InlineCacheState::Megamorphic);
    assert!(metadata[0].telemetry_enabled);
    assert_eq!(metadata[0].live_entries, 8);
    assert_eq!(metadata[0].capacity, 8);
    assert_eq!(metadata[0].hits, 1);
    assert_eq!(metadata[0].misses, 9);
    assert_eq!(metadata[0].installs, 9);
    assert_eq!(metadata[0].replacements, 1);

    Ok(())
}

#[test]
fn get_internal_method() {
    let context = &mut Context::default();

    let o = context
        .intrinsics()
        .templates()
        .ordinary_object()
        .create(OrdinaryObject, Vec::default());
    let _o_root = o.clone().root();

    let property: PropertyKey = js_string!("prop").into();
    let value = 100;

    o.set(property.clone(), value, true, context)
        .expect("should not fail");

    let context = &mut InternalMethodPropertyContext::new(context);

    assert_eq!(context.slot().index, 0);
    assert_eq!(context.slot().attributes, SlotAttributes::empty());

    o.__get__(&property, o.clone().into(), context)
        .expect("should not fail");

    assert!(
        !context.slot().in_prototype(),
        "Since it's an owned property, the prototype bit should not be set"
    );

    assert!(
        context.slot().is_cachable(),
        "Since it's an owned property, this should be cachable"
    );

    let shape = o.borrow().shape().clone();

    let slot = shape.lookup(&property);

    assert!(slot.is_some(), "the property should be found in the object");

    let slot = slot.expect("the property should be found in the object");

    assert_eq!(context.slot().index, slot.index);
}

#[test]
fn get_internal_method_in_prototype() {
    let context = &mut Context::default();

    let o = context
        .intrinsics()
        .templates()
        .ordinary_object()
        .create(OrdinaryObject, Vec::default());
    let _o_root = o.clone().root();

    let property: PropertyKey = js_string!("prop").into();
    let value = 100;

    let prototype = context.intrinsics().constructors().object().prototype();

    prototype
        .set(property.clone(), value, true, context)
        .expect("should not fail");

    let context = &mut InternalMethodPropertyContext::new(context);

    assert_eq!(context.slot().index, 0);
    assert_eq!(context.slot().attributes, SlotAttributes::empty());

    o.__get__(&property, o.clone().into(), context)
        .expect("should not fail");

    assert!(
        context.slot().in_prototype(),
        "Since it's an prototype property, the prototype bit should not be set"
    );

    assert!(
        context.slot().is_cachable(),
        "Since it's an prototype property, this should be cachable"
    );

    let shape = prototype.borrow().shape().clone();

    let slot = shape.lookup(&property);

    assert!(slot.is_some(), "the property should be found in the object");

    let slot = slot.expect("the property should be found in the object");

    assert_eq!(context.slot().index, slot.index);
}

#[test]
fn define_own_property_internal_method_non_existant_property() {
    let context = &mut Context::default();

    let o = context
        .intrinsics()
        .templates()
        .ordinary_object()
        .create(OrdinaryObject, Vec::default());
    let _o_root = o.clone().root();

    let property: PropertyKey = js_string!("prop").into();
    let value = 100;

    o.set(property.clone(), value, true, context)
        .expect("should not fail");

    let context = &mut InternalMethodPropertyContext::new(context);

    assert_eq!(context.slot().index, 0);
    assert_eq!(context.slot().attributes, SlotAttributes::empty());

    o.__define_own_property__(
        &property,
        PropertyDescriptor::builder()
            .value(value)
            .writable(true)
            .configurable(true)
            .enumerable(true)
            .build(),
        context,
    )
    .expect("should not fail");

    assert!(
        !context.slot().in_prototype(),
        "Since it's an owned property, the prototype bit should not be set"
    );

    assert!(
        context.slot().is_cachable(),
        "Since it's an owned property, this should be cachable"
    );

    let shape = o.borrow().shape().clone();

    let slot = shape.lookup(&property);

    assert!(slot.is_some(), "the property should be found in the object");

    let slot = slot.expect("the property should be found in the object");

    assert_eq!(context.slot().index, slot.index);
}

#[test]
fn define_own_property_internal_method_existing_property_property() {
    let context = &mut Context::default();

    let o = context
        .intrinsics()
        .templates()
        .ordinary_object()
        .create(OrdinaryObject, Vec::default());
    let _o_root = o.clone().root();

    let property: PropertyKey = js_string!("prop").into();
    let value = 100;

    o.set(property.clone(), value, true, context)
        .expect("should not fail");

    o.__define_own_property__(
        &property,
        PropertyDescriptor::builder()
            .value(value)
            .writable(true)
            .configurable(true)
            .enumerable(true)
            .build(),
        &mut context.into(),
    )
    .expect("should not fail");

    let context = &mut InternalMethodPropertyContext::new(context);

    assert_eq!(context.slot().index, 0);
    assert_eq!(context.slot().attributes, SlotAttributes::empty());

    o.__define_own_property__(
        &property,
        PropertyDescriptor::builder()
            .value(value + 100)
            .writable(true)
            .configurable(true)
            .enumerable(true)
            .build(),
        context,
    )
    .expect("should not fail");

    assert!(
        !context.slot().in_prototype(),
        "Since it's an owned property, the prototype bit should not be set"
    );

    assert!(
        context.slot().is_cachable(),
        "Since it's an owned property, this should be cachable"
    );

    let shape = o.borrow().shape().clone();

    let slot = shape.lookup(&property);

    assert!(slot.is_some(), "the property should be found in the object");

    let slot = slot.expect("the property should be found in the object");

    assert_eq!(context.slot().index, slot.index);
}

#[test]
fn set_internal_method() {
    let context = &mut Context::default();

    let o = context
        .intrinsics()
        .templates()
        .ordinary_object()
        .create(OrdinaryObject, Vec::default());
    let _o_root = o.clone().root();

    let property: PropertyKey = js_string!("prop").into();
    let value = 100;

    o.set(property.clone(), value, true, context)
        .expect("should not fail");

    let context = &mut InternalMethodPropertyContext::new(context);

    assert_eq!(context.slot().index, 0);
    assert_eq!(context.slot().attributes, SlotAttributes::empty());

    o.__set__(property.clone(), value.into(), o.clone().into(), context)
        .expect("should not fail");

    assert!(
        !context.slot().in_prototype(),
        "Since it's an owned property, the prototype bit should not be set"
    );

    assert!(
        context.slot().is_cachable(),
        "Since it's an owned property, this should be cachable"
    );

    let shape = o.borrow().shape().clone();

    let slot = shape.lookup(&property);

    assert!(slot.is_some(), "the property should be found in the object");

    let slot = slot.expect("the property should be found in the object");

    assert_eq!(context.slot().index, slot.index);
}

fn get_codeblock(value: &JsValue) -> Option<(JsObject, Rooted<CodeBlock>)> {
    let object = value.as_object()?.clone();
    let code = object
        .downcast_ref::<OrdinaryFunction>()?
        .code
        .clone()
        .root();

    Some((object, code))
}

#[test]
fn set_property_by_name_set_inline_cache_on_property_load() -> JsResult<()> {
    let context = &mut Context::default();
    let function = context.eval(Source::from_bytes("(function (o) { return o.test; })"))?;
    let (function, code) = get_codeblock(&function).unwrap();
    let _function_root = function.clone().root();

    assert_eq!(code.ic.len(), 1);
    assert!(matches!(*code.ic[0].shape.borrow(), RootedWeakShape::None));

    let o = ObjectInitializer::new(context)
        .property(js_string!("test"), 0, Attribute::all())
        .build();
    let _o_root = o.clone().root();
    let o_shape = o.borrow().shape().clone();

    function.call(&JsValue::undefined(), &[o.clone().into()], context)?;

    assert_eq!(
        code.ic[0].shape.borrow().to_addr_usize(),
        o_shape.to_addr_usize()
    );

    Ok(())
}

/// Regression test: a warmed prototype-property inline cache must not return a
/// stale slot after the prototype's storage is reindexed.
///
/// A cachable prototype-property slot indexes into the *prototype's* storage,
/// but the inline cache only tracks the *receiver's* shape. Deleting an earlier
/// property from the prototype compacts its storage and shifts the target
/// property's slot index down, while the receiver's shape is unchanged. Before
/// the prototype-shape guard was added, the warm call site kept hitting with
/// the stale slot index and resolved to a different property (this is the
/// core-js / `String.prototype` poisoning from Omoikane issue 058, reduced to a
/// deterministic minimal case).
#[test]
fn prototype_property_inline_cache_survives_prototype_reindex() -> JsResult<()> {
    let context = &mut Context::default();

    let result = context.eval(Source::from_bytes(
        r"
        var proto = {};
        proto.a = 10;
        proto.b = 20;
        proto.target = 42;
        proto.c = 99;
        proto.d = 88;
        var o = Object.create(proto);
        function read(x) { return x.target; }
        // Warm the inline cache for the `target` prototype-property load; the
        // slot caches index 2 into the prototype's storage.
        read(o);
        read(o);
        read(o);
        if (read(o) !== 42) { throw new Error('warmup should read 42'); }
        // Reindex the prototype: deleting `a` then `b` compacts storage so that
        // `target` moves to index 0 and index 2 now holds `d` (== 88). The
        // receiver `o`'s shape is unaffected.
        delete proto.a;
        delete proto.b;
        read(o)
        ",
    ))?;

    assert_eq!(
        result.as_number(),
        Some(42.0),
        "prototype inline cache returned a stale slot after the prototype was reindexed"
    );

    Ok(())
}

#[test]
fn get_property_by_name_set_inline_cache_on_property_load() -> JsResult<()> {
    let context = &mut Context::default();
    let function = context.eval(Source::from_bytes("(function (o) { o.test = 30; })"))?;
    let (function, code) = get_codeblock(&function).unwrap();
    let _function_root = function.clone().root();

    assert_eq!(code.ic.len(), 1);
    assert!(matches!(*code.ic[0].shape.borrow(), RootedWeakShape::None));

    let o = ObjectInitializer::new(context)
        .property(js_string!("test"), 0, Attribute::all())
        .build();
    let _o_root = o.clone().root();
    let o_shape = o.borrow().shape().clone();

    function.call(&JsValue::undefined(), &[o.clone().into()], context)?;

    assert_eq!(
        code.ic[0].shape.borrow().to_addr_usize(),
        o_shape.to_addr_usize()
    );

    Ok(())
}

#[derive(Debug, boa_gc::Trace, crate::JsData)]
struct CacheGetterResult {
    #[unsafe_ignore_trace]
    finalized: std::rc::Rc<std::cell::Cell<bool>>,
}

impl boa_gc::Finalize for CacheGetterResult {
    fn finalize(&self) {
        self.finalized.set(true);
    }
}

#[derive(boa_gc::Trace)]
struct CacheFillGarbage {
    #[unsafe_ignore_trace]
    _padding: [u8; 8192],
    #[unsafe_ignore_trace]
    finalized: std::rc::Rc<std::cell::Cell<usize>>,
}

impl boa_gc::Finalize for CacheFillGarbage {
    fn finalize(&self) {
        self.finalized.set(self.finalized.get() + 1);
    }
}

#[derive(boa_gc::Finalize, boa_gc::Trace)]
struct CacheGetterCaptures {
    #[unsafe_ignore_trace]
    result_finalized: std::rc::Rc<std::cell::Cell<bool>>,
    #[unsafe_ignore_trace]
    garbage_finalized: std::rc::Rc<std::cell::Cell<usize>>,
}

fn assert_getter_result_survives_cache_fill(source: &str) -> JsResult<()> {
    use std::{cell::Cell, rc::Rc};

    let context = &mut Context::default();
    let result_finalized = Rc::new(Cell::new(false));
    let garbage_finalized = Rc::new(Cell::new(0));
    let getter = crate::object::FunctionObjectBuilder::new(
        context.realm(),
        crate::NativeFunction::from_copy_closure_with_captures(
            |_, _, captures, _| {
                let result = JsObject::from_proto_and_data(
                    None,
                    CacheGetterResult {
                        finalized: Rc::clone(&captures.result_finalized),
                    },
                );
                // Exceed the nursery threshold while the getter runs, then let
                // the inline cache's next allocation perform the collection.
                let _no_gc = boa_gc::NoGcScope::new();
                for _ in 0..512 {
                    let _garbage = boa_gc::GcEdge::new(CacheFillGarbage {
                        _padding: [0; 8192],
                        finalized: Rc::clone(&captures.garbage_finalized),
                    });
                }
                Ok(result.into())
            },
            CacheGetterCaptures {
                result_finalized: Rc::clone(&result_finalized),
                garbage_finalized: Rc::clone(&garbage_finalized),
            },
        ),
    )
    .build();
    context.global_object().define_property_or_throw(
        js_string!("fresh"),
        PropertyDescriptor::builder().get(getter).configurable(true),
        context,
    )?;
    let result = context.eval(Source::from_bytes(source))?;
    assert!(garbage_finalized.get() > 0, "cache fill must collect");
    // Check finalization before touching the result so the regression fails
    // without dereferencing the object that the buggy cache fill has freed.
    assert!(
        !result_finalized.get(),
        "getter result was collected during cache fill"
    );
    assert!(result.as_object().unwrap().is::<CacheGetterResult>());
    Ok(())
}

#[test]
fn named_property_getter_result_survives_cache_fill() -> JsResult<()> {
    assert_getter_result_survives_cache_fill("globalThis.fresh")
}

#[test]
fn global_getter_result_survives_cache_fill() -> JsResult<()> {
    assert_getter_result_survives_cache_fill("fresh")
}

#[test]
fn map_iterator_finalization_can_borrow_cache_fill_receiver() -> JsResult<()> {
    use std::{cell::Cell, rc::Rc};

    let context = &mut Context::default();
    let map = context
        .eval(Source::from_bytes("globalThis.map = new Map([[1, 2]])"))?
        .as_object()
        .unwrap();
    let _map_root = map.clone().root();
    let garbage_finalized = Rc::new(Cell::new(0));
    let getter = crate::object::FunctionObjectBuilder::new(
        context.realm(),
        crate::NativeFunction::from_copy_closure_with_captures(
            |_, _, captures, context| {
                let _no_gc = boa_gc::NoGcScope::new();
                // Abandon a real iterator over the object whose property cache
                // is about to be filled. Its finalizer must update the map.
                context.eval(Source::from_bytes("map.entries()"))?;
                for _ in 0..512 {
                    let _garbage = boa_gc::GcEdge::new(CacheFillGarbage {
                        _padding: [0; 8192],
                        finalized: Rc::clone(&captures.garbage_finalized),
                    });
                }
                Ok(1.into())
            },
            CacheGetterCaptures {
                result_finalized: Rc::new(Cell::new(false)),
                garbage_finalized: Rc::clone(&garbage_finalized),
            },
        ),
    )
    .build();
    map.define_property_or_throw(
        js_string!("fresh"),
        PropertyDescriptor::builder().get(getter).configurable(true),
        context,
    )?;
    assert_eq!(context.eval(Source::from_bytes("map.fresh"))?, 1.into());
    assert!(garbage_finalized.get() > 0, "cache fill must collect");
    assert_eq!(context.eval(Source::from_bytes("map.size"))?, 1.into());
    Ok(())
}
