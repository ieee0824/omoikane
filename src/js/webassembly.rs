//! WebAssembly core module validation and metadata bridge.

use super::*;
use serde::Serialize;
use std::cell::RefCell;
use wasmi::{
    Caller, Config, Engine, ExternType, Func, Instance, Linker, Module, Store, Val, ValType,
};
use wasmparser::{ExternalKind, Parser, Payload, TypeRef};

const MAX_MODULE_BYTES: usize = 64 * 1024 * 1024;
const CALL_FUEL: u64 = 10_000_000;
const LINK_ERROR_PREFIX: &str = "__WASM_LINK__";
const RUNTIME_ERROR_PREFIX: &str = "__WASM_RUNTIME__";

thread_local! {
    static ACTIVE_WASM_CONTEXT: std::cell::Cell<*mut Context> = const {
        std::cell::Cell::new(std::ptr::null_mut())
    };
    static ACTIVE_WASM_CALLER: std::cell::Cell<*mut ()> = const {
        std::cell::Cell::new(std::ptr::null_mut())
    };
}

/// Runtime-owned WebAssembly engine shared by compiled module objects. Wasmi
/// types and lifetimes do not leak through the JavaScript API.
pub(super) struct State {
    engine: Engine,
}

impl Default for State {
    fn default() -> Self {
        let mut config = Config::default();
        config.consume_fuel(true);
        Self {
            engine: Engine::new(&config),
        }
    }
}

impl State {
    fn validate(&self, bytes: &[u8]) -> bool {
        bytes.len() <= MAX_MODULE_BYTES && Module::validate(&self.engine, bytes).is_ok()
    }

    fn compile(&self, bytes: &[u8]) -> Result<CompiledModule, wasmi::Error> {
        let module = Module::new(&self.engine, bytes)?;
        let metadata =
            ModuleMetadata::parse(bytes).map_err(|error| wasmi::Error::new(error.to_string()))?;
        Ok(CompiledModule { module, metadata })
    }
}

#[derive(Trace, Finalize, boa_engine::JsData)]
struct CompiledModule {
    #[allow(dead_code)]
    #[unsafe_ignore_trace]
    module: Module,
    #[unsafe_ignore_trace]
    metadata: ModuleMetadata,
}

struct StoreData {
    callbacks: Vec<JsObject>,
    pending_error: Option<JsError>,
    instance: Option<Instance>,
}

struct RuntimeInstance {
    store: Store<StoreData>,
    instance: Instance,
}

#[derive(Trace, Finalize, boa_engine::JsData)]
struct CompiledInstance {
    /// Keeps imported JavaScript callables live while Wasmi stores untraced
    /// clones in its host state.
    callbacks: Vec<JsObject>,
    #[unsafe_ignore_trace]
    runtime: RefCell<RuntimeInstance>,
}

#[derive(Serialize)]
struct RuntimeExport {
    name: String,
    kind: &'static str,
    params: usize,
}

#[derive(Serialize)]
struct RuntimeImport {
    results: usize,
}

struct ContextGuard {
    previous: *mut Context,
}

struct CallerGuard {
    previous: *mut (),
}

impl CallerGuard {
    fn enter(caller: &mut Caller<'_, StoreData>) -> Self {
        let pointer = std::ptr::from_mut(caller).cast::<()>();
        let previous = ACTIVE_WASM_CALLER.with(|slot| slot.replace(pointer));
        Self { previous }
    }
}

impl Drop for CallerGuard {
    fn drop(&mut self) {
        ACTIVE_WASM_CALLER.with(|slot| slot.set(self.previous));
    }
}

impl ContextGuard {
    fn enter(context: &mut Context) -> Self {
        let previous = ACTIVE_WASM_CONTEXT.with(|slot| slot.replace(context));
        Self { previous }
    }
}

impl Drop for ContextGuard {
    fn drop(&mut self) {
        ACTIVE_WASM_CONTEXT.with(|slot| slot.set(self.previous));
    }
}

#[derive(Default)]
struct ModuleMetadata {
    imports: Vec<ImportDescriptor>,
    exports: Vec<ExportDescriptor>,
    custom_sections: Vec<CustomSection>,
}

impl ModuleMetadata {
    fn parse(bytes: &[u8]) -> Result<Self, wasmparser::BinaryReaderError> {
        let mut metadata = Self::default();
        for payload in Parser::new(0).parse_all(bytes) {
            match payload? {
                Payload::ImportSection(section) => {
                    for import in section {
                        let import = import?;
                        metadata.imports.push(ImportDescriptor {
                            module: import.module.to_owned(),
                            name: import.name.to_owned(),
                            kind: import_kind(import.ty),
                        });
                    }
                }
                Payload::ExportSection(section) => {
                    for export in section {
                        let export = export?;
                        metadata.exports.push(ExportDescriptor {
                            name: export.name.to_owned(),
                            kind: external_kind(export.kind),
                        });
                    }
                }
                Payload::CustomSection(section) => metadata.custom_sections.push(CustomSection {
                    name: section.name().to_owned(),
                    data: section.data().to_vec(),
                }),
                _ => {}
            }
        }
        Ok(metadata)
    }
}

#[derive(Serialize)]
struct ImportDescriptor {
    module: String,
    name: String,
    kind: &'static str,
}

#[derive(Serialize)]
struct ExportDescriptor {
    name: String,
    kind: &'static str,
}

struct CustomSection {
    name: String,
    data: Vec<u8>,
}

fn import_kind(ty: TypeRef) -> &'static str {
    match ty {
        TypeRef::Func(_) => "function",
        TypeRef::Table(_) => "table",
        TypeRef::Memory(_) => "memory",
        TypeRef::Global(_) => "global",
        TypeRef::Tag(_) => "tag",
    }
}

fn external_kind(kind: ExternalKind) -> &'static str {
    match kind {
        ExternalKind::Func => "function",
        ExternalKind::Table => "table",
        ExternalKind::Memory => "memory",
        ExternalKind::Global => "global",
        ExternalKind::Tag => "tag",
    }
}

fn source_bytes(args: &[JsValue], context: &mut Context) -> JsResult<Vec<u8>> {
    let bytes = text_stream::buffer_source_bytes(
        args.first().unwrap_or(&JsValue::undefined()),
        context,
        "WebAssembly source",
        false,
    )?;
    if bytes.len() > MAX_MODULE_BYTES {
        return Err(JsNativeError::range()
            .with_message("WebAssembly module exceeds the 64 MiB limit")
            .into());
    }
    Ok(bytes)
}

fn validate_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let bytes = match source_bytes(args, context) {
        Ok(bytes) => bytes,
        Err(error) if error.as_native().is_some_and(|native| native.is_range()) => {
            return Ok(false.into());
        }
        Err(error) => return Err(error),
    };
    with_host_state(|host| Ok(host.borrow().webassembly.validate(&bytes).into()))
}

fn compile_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let bytes = source_bytes(args, context)?;
    with_host_state(|host| {
        let module = host
            .borrow()
            .webassembly
            .compile(&bytes)
            .map_err(|error| JsNativeError::syntax().with_message(error.to_string()))?;
        Ok(JsObject::from_proto_and_data(None, module).into())
    })
}

fn metadata_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let module_handle = args
        .first()
        .and_then(JsValue::as_object)
        .ok_or_else(|| JsNativeError::typ().with_message("Invalid WebAssembly.Module handle"))?;
    let operation = args
        .get(1)
        .unwrap_or(&JsValue::undefined())
        .to_string(context)?
        .to_std_string_escaped();
    let name = args
        .get(2)
        .map(|value| value.to_string(context))
        .transpose()?
        .map(|value| value.to_std_string_escaped());
    let module = module_handle
        .downcast_ref::<CompiledModule>()
        .ok_or_else(|| JsNativeError::typ().with_message("Invalid WebAssembly.Module handle"))?;
    let result = (|| {
        let json = match operation.as_str() {
            "imports" => serde_json::to_string(&module.metadata.imports),
            "exports" => serde_json::to_string(&module.metadata.exports),
            "customSections" => serde_json::to_string(
                &module
                    .metadata
                    .custom_sections
                    .iter()
                    .filter(|section| name.as_deref() == Some(section.name.as_str()))
                    .map(|section| section.data.as_slice())
                    .collect::<Vec<_>>(),
            ),
            "runtimeImports" => serde_json::to_string(
                &module
                    .module
                    .imports()
                    .map(|import| RuntimeImport {
                        results: import.ty().func().map_or(0, |ty| ty.results().len()),
                    })
                    .collect::<Vec<_>>(),
            ),
            _ => {
                return Err(JsNativeError::typ()
                    .with_message("Unknown WebAssembly.Module metadata operation")
                    .into());
            }
        }
        .map_err(|error| JsNativeError::error().with_message(error.to_string()))?;
        Ok(js_string!(json).into())
    })();
    drop(module);
    result
}

fn js_to_wasm(value: &JsValue, ty: ValType, context: &mut Context) -> JsResult<Val> {
    match ty {
        ValType::I32 => Ok(Val::I32(value.to_i32(context)?)),
        ValType::I64 => Ok(Val::I64(value.to_big_int64(context)?)),
        ValType::F32 => Ok(Val::F32((value.to_number(context)? as f32).into())),
        ValType::F64 => Ok(Val::F64(value.to_number(context)?.into())),
        _ => Err(JsNativeError::typ()
            .with_message("WebAssembly reference and vector values are not supported yet")
            .into()),
    }
}

fn wasm_to_js(value: &Val) -> JsResult<JsValue> {
    match value {
        Val::I32(value) => Ok((*value).into()),
        Val::I64(value) => Ok(boa_engine::JsBigInt::from(*value).into()),
        Val::F32(value) => Ok((f32::from(*value) as f64).into()),
        Val::F64(value) => Ok(f64::from(*value).into()),
        _ => Err(JsNativeError::typ()
            .with_message("WebAssembly reference and vector values are not supported yet")
            .into()),
    }
}

fn callback_result(
    value: JsValue,
    result_types: &[ValType],
    context: &mut Context,
) -> JsResult<Vec<Val>> {
    if result_types.is_empty() {
        return Ok(Vec::new());
    }
    if result_types.len() == 1 {
        return Ok(vec![js_to_wasm(&value, result_types[0], context)?]);
    }
    let values = value.as_object().ok_or_else(|| {
        JsNativeError::typ().with_message("WebAssembly multi-value result must be iterable")
    })?;
    result_types
        .iter()
        .enumerate()
        .map(|(index, ty)| js_to_wasm(&values.get(index, context)?, *ty, context))
        .collect()
}

fn imported_function(
    store: &mut Store<StoreData>,
    ty: wasmi::FuncType,
    callback_index: usize,
) -> Func {
    let result_types = ty.results().to_vec();
    Func::new(
        store,
        ty,
        move |mut caller: Caller<'_, StoreData>, inputs, outputs| {
            let callback = caller.data().callbacks[callback_index].clone();
            let _caller_guard = CallerGuard::enter(&mut caller);
            let result = ACTIVE_WASM_CONTEXT.with(|slot| {
                let context = slot.get();
                if context.is_null() {
                    return Err(JsNativeError::error()
                        .with_message("WebAssembly callback has no active JavaScript context")
                        .into());
                }
                // SAFETY: ContextGuard installs this pointer only for the duration of
                // a synchronous Wasm call on this thread and restores it on drop.
                let context = unsafe { &mut *context };
                let args = inputs
                    .iter()
                    .map(wasm_to_js)
                    .collect::<JsResult<Vec<_>>>()?;
                let value = callback.call(&JsValue::undefined(), &args, context)?;
                callback_result(value, &result_types, context)
            });
            match result {
                Ok(values) => {
                    outputs.clone_from_slice(&values);
                    Ok(())
                }
                Err(error) => {
                    caller.data_mut().pending_error = Some(error);
                    Err(wasmi::Error::new("JavaScript WebAssembly import threw"))
                }
            }
        },
    )
}

fn instantiate_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let module_handle = args
        .first()
        .and_then(JsValue::as_object)
        .ok_or_else(|| JsNativeError::typ().with_message("Invalid WebAssembly.Module handle"))?;
    let import_values = args
        .get(1)
        .and_then(JsValue::as_object)
        .ok_or_else(|| JsNativeError::typ().with_message("Invalid WebAssembly import list"))?;
    let module = module_handle
        .downcast_ref::<CompiledModule>()
        .ok_or_else(|| JsNativeError::typ().with_message("Invalid WebAssembly.Module handle"))?
        .module
        .clone();

    let engine = module.engine().clone();
    let mut callbacks = Vec::new();
    for (index, import) in module.imports().enumerate() {
        let ExternType::Func(_) = import.ty() else {
            return Err(JsNativeError::typ()
                .with_message("WebAssembly non-function imports are not supported yet")
                .into());
        };
        let callback = import_values
            .get(index, context)?
            .as_object()
            .filter(|object| object.is_callable())
            .ok_or_else(|| {
                JsNativeError::typ().with_message(format!(
                    "WebAssembly import {}.{} must be callable",
                    import.module(),
                    import.name()
                ))
            })?;
        callbacks.push(callback);
    }

    let mut store = Store::new(
        &engine,
        StoreData {
            callbacks: callbacks.clone(),
            pending_error: None,
            instance: None,
        },
    );
    store
        .set_fuel(CALL_FUEL)
        .map_err(|error| JsNativeError::error().with_message(error.to_string()))?;
    let mut linker = Linker::new(&engine);
    for (index, import) in module.imports().enumerate() {
        let ExternType::Func(ty) = import.ty() else {
            unreachable!("non-function imports were rejected above")
        };
        let function = imported_function(&mut store, ty.clone(), index);
        linker
            .define(import.module(), import.name(), function)
            .map_err(|error| {
                JsNativeError::error().with_message(format!("{LINK_ERROR_PREFIX}{error}"))
            })?;
    }
    let _guard = ContextGuard::enter(context);
    let instance = linker
        .instantiate_and_start(&mut store, &module)
        .map_err(|error| {
            store.data_mut().pending_error.take().unwrap_or_else(|| {
                JsNativeError::error()
                    .with_message(format!(
                        "{}{}",
                        if error.as_trap_code().is_some() {
                            RUNTIME_ERROR_PREFIX
                        } else {
                            LINK_ERROR_PREFIX
                        },
                        error
                    ))
                    .into()
            })
        })?;
    store.data_mut().instance = Some(instance);
    Ok(JsObject::from_proto_and_data(
        None,
        CompiledInstance {
            callbacks,
            runtime: RefCell::new(RuntimeInstance { store, instance }),
        },
    )
    .into())
}

fn exports_native(_: &JsValue, args: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    let instance_handle = args
        .first()
        .and_then(JsValue::as_object)
        .ok_or_else(|| JsNativeError::typ().with_message("Invalid WebAssembly.Instance handle"))?;
    let instance = instance_handle
        .downcast_ref::<CompiledInstance>()
        .ok_or_else(|| JsNativeError::typ().with_message("Invalid WebAssembly.Instance handle"))?;
    let runtime = instance.runtime.borrow();
    let exports = runtime
        .instance
        .exports(&runtime.store)
        .map(|export| {
            let ty = export.ty(&runtime.store);
            RuntimeExport {
                name: export.name().to_owned(),
                kind: match ty {
                    ExternType::Func(_) => "function",
                    ExternType::Global(_) => "global",
                    ExternType::Memory(_) => "memory",
                    ExternType::Table(_) => "table",
                },
                params: ty.func().map_or(0, |ty| ty.params().len()),
            }
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&exports)
        .map(|json| js_string!(json).into())
        .map_err(|error| {
            JsNativeError::error()
                .with_message(error.to_string())
                .into()
        })
}

fn call_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let instance_handle = args
        .first()
        .and_then(JsValue::as_object)
        .ok_or_else(|| JsNativeError::typ().with_message("Invalid WebAssembly.Instance handle"))?;
    let name = args
        .get(1)
        .unwrap_or(&JsValue::undefined())
        .to_string(context)?
        .to_std_string_escaped();
    let js_args = args
        .get(2)
        .and_then(JsValue::as_object)
        .ok_or_else(|| JsNativeError::typ().with_message("Invalid WebAssembly argument list"))?;
    let instance = instance_handle
        .downcast_ref::<CompiledInstance>()
        .ok_or_else(|| JsNativeError::typ().with_message("Invalid WebAssembly.Instance handle"))?;
    let mut runtime = match instance.runtime.try_borrow_mut() {
        Ok(runtime) => runtime,
        Err(_) => {
            return ACTIVE_WASM_CALLER.with(|slot| {
                let pointer = slot.get();
                if pointer.is_null() {
                    return Err(JsNativeError::error()
                        .with_message("WebAssembly instance is already executing")
                        .into());
                }
                // SAFETY: CallerGuard exposes this pointer only while the host
                // callback's Caller is alive on this thread. Nested JS executes
                // synchronously and cannot retain this private native hook.
                let caller = unsafe { &mut *pointer.cast::<Caller<'_, StoreData>>() };
                let active_instance = caller.data().instance.ok_or_else(|| {
                    JsNativeError::error().with_message("WebAssembly instance is not initialized")
                })?;
                let function = active_instance.get_func(&*caller, &name).ok_or_else(|| {
                    JsNativeError::typ().with_message("Unknown WebAssembly function export")
                })?;
                let ty = function.ty(&*caller);
                let inputs = ty
                    .params()
                    .iter()
                    .enumerate()
                    .map(|(index, ty)| js_to_wasm(&js_args.get(index, context)?, *ty, context))
                    .collect::<JsResult<Vec<_>>>()?;
                let mut outputs = ty
                    .results()
                    .iter()
                    .copied()
                    .map(Val::default_for_ty)
                    .collect::<Vec<_>>();
                function
                    .call(&mut *caller, &inputs, &mut outputs)
                    .map_err(|error| {
                        JsNativeError::error()
                            .with_message(format!("{RUNTIME_ERROR_PREFIX}{error}"))
                    })?;
                match outputs.as_slice() {
                    [] => Ok(JsValue::undefined()),
                    [value] => wasm_to_js(value),
                    values => Ok(JsArray::from_iter(
                        values
                            .iter()
                            .map(wasm_to_js)
                            .collect::<JsResult<Vec<_>>>()?,
                        context,
                    )
                    .into()),
                }
            });
        }
    };
    runtime
        .store
        .set_fuel(CALL_FUEL)
        .map_err(|error| JsNativeError::error().with_message(error.to_string()))?;
    let function = runtime
        .instance
        .get_func(&runtime.store, &name)
        .ok_or_else(|| JsNativeError::typ().with_message("Unknown WebAssembly function export"))?;
    let ty = function.ty(&runtime.store);
    let inputs = ty
        .params()
        .iter()
        .enumerate()
        .map(|(index, ty)| js_to_wasm(&js_args.get(index, context)?, *ty, context))
        .collect::<JsResult<Vec<_>>>()?;
    let mut outputs = ty
        .results()
        .iter()
        .copied()
        .map(Val::default_for_ty)
        .collect::<Vec<_>>();
    let _guard = ContextGuard::enter(context);
    if let Err(error) = function.call(&mut runtime.store, &inputs, &mut outputs) {
        if let Some(error) = runtime.store.data_mut().pending_error.take() {
            return Err(error);
        }
        return Err(JsNativeError::error()
            .with_message(format!("{RUNTIME_ERROR_PREFIX}{error}"))
            .into());
    }
    match outputs.as_slice() {
        [] => Ok(JsValue::undefined()),
        [value] => wasm_to_js(value),
        values => Ok(JsArray::from_iter(
            values
                .iter()
                .map(wasm_to_js)
                .collect::<JsResult<Vec<_>>>()?,
            context,
        )
        .into()),
    }
}

/// Registers the private hooks consumed by the realm-local WebAssembly wrapper.
pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    for (name, length, function) in [
        (
            js_string!("__omoikane_wasm_validate"),
            1,
            NativeFunction::from_copy_closure(validate_native),
        ),
        (
            js_string!("__omoikane_wasm_compile"),
            1,
            NativeFunction::from_copy_closure(compile_native),
        ),
        (
            js_string!("__omoikane_wasm_module_metadata"),
            3,
            NativeFunction::from_copy_closure(metadata_native),
        ),
        (
            js_string!("__omoikane_wasm_instantiate"),
            2,
            NativeFunction::from_copy_closure(instantiate_native),
        ),
        (
            js_string!("__omoikane_wasm_instance_exports"),
            1,
            NativeFunction::from_copy_closure(exports_native),
        ),
        (
            js_string!("__omoikane_wasm_call"),
            3,
            NativeFunction::from_copy_closure(call_native),
        ),
    ] {
        register_private_builtin_callable(context, bindings, name, length, function)?;
    }
    Ok(())
}
