//! WebAssembly core module validation and metadata bridge.

use super::*;
use serde::Serialize;
use std::cell::RefCell;
use std::rc::Rc;
use wasmi::{
    Caller, Config, Engine, Extern, ExternRef, ExternType, Func, Global, Instance, Linker, Memory,
    MemoryType, Module, Mutability, Nullable, Ref, RefType, Store, Table, TableType, Val, ValType,
};
use wasmparser::{ExternalKind, Operator, Parser, Payload, TypeRef};

#[path = "wasm_legacy_eh.rs"]
mod legacy_eh;

const MAX_MODULE_BYTES: usize = 64 * 1024 * 1024;
const CALL_FUEL: u64 = 10_000_000;
const LINK_ERROR_PREFIX: &str = "__WASM_LINK__";
const RUNTIME_ERROR_PREFIX: &str = "__WASM_RUNTIME__";
const EXCEPTION_ERROR_PREFIX: &str = "__WASM_EXCEPTION__";

fn module_uses_exceptions(bytes: &[u8]) -> bool {
    Parser::new(0)
        .parse_all(bytes)
        .any(|payload| match payload {
            Ok(Payload::TagSection(_)) => true,
            Ok(Payload::ImportSection(imports)) => imports.into_iter().any(|import| {
                import
                    .map(|import| matches!(import.ty, TypeRef::Tag(_)))
                    .unwrap_or(false)
            }),
            Ok(Payload::CodeSectionEntry(body)) => body
                .get_operators_reader()
                .map(|mut reader| {
                    while !reader.eof() {
                        if matches!(
                            reader.read(),
                            Ok(Operator::Try { .. }
                                | Operator::TryTable { .. }
                                | Operator::Catch { .. }
                                | Operator::CatchAll
                                | Operator::Throw { .. }
                                | Operator::ThrowRef
                                | Operator::Rethrow { .. }
                                | Operator::Delegate { .. })
                        ) {
                            return true;
                        }
                    }
                    false
                })
                .unwrap_or(false),
            _ => false,
        })
}

fn module_uses_legacy_exceptions(bytes: &[u8]) -> bool {
    Parser::new(0)
        .parse_all(bytes)
        .any(|payload| match payload {
            Ok(Payload::CodeSectionEntry(body)) => body
                .get_operators_reader()
                .map(|mut reader| {
                    while !reader.eof() {
                        if matches!(
                            reader.read(),
                            Ok(Operator::Try { .. }
                                | Operator::Catch { .. }
                                | Operator::CatchAll
                                | Operator::Rethrow { .. }
                                | Operator::Delegate { .. })
                        ) {
                            return true;
                        }
                    }
                    false
                })
                .unwrap_or(false),
            _ => false,
        })
}

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
    exception_engine: wasmtime::Engine,
    runtime: Rc<RefCell<RealmRuntime>>,
    exception_runtime: Rc<RefCell<ExceptionRuntime>>,
}

impl Default for State {
    fn default() -> Self {
        let mut config = Config::default();
        config.consume_fuel(true);
        let engine = Engine::new(&config);
        let mut exception_config = wasmtime::Config::new();
        exception_config.wasm_exceptions(true).consume_fuel(true);
        let exception_engine = wasmtime::Engine::new(&exception_config)
            .expect("WebAssembly exception runtime configuration must be valid");
        let store = Store::new(
            &engine,
            StoreData {
                callbacks: Vec::new(),
                pending_error: None,
                instance: None,
            },
        );
        Self {
            engine,
            exception_runtime: Rc::new(RefCell::new(ExceptionRuntime {
                store: wasmtime::Store::new(
                    &exception_engine,
                    ExceptionStoreData {
                        callbacks: Vec::new(),
                        pending_error: None,
                        roots: Vec::new(),
                    },
                ),
            })),
            exception_engine,
            runtime: Rc::new(RefCell::new(RealmRuntime { store })),
        }
    }
}

impl State {
    fn validate(&self, bytes: &[u8]) -> bool {
        bytes.len() <= MAX_MODULE_BYTES && Module::validate(&self.engine, bytes).is_ok()
    }

    fn compile(&self, bytes: &[u8]) -> Result<CompiledModule, wasmi::Error> {
        let metadata =
            ModuleMetadata::parse(bytes).map_err(|error| wasmi::Error::new(error.to_string()))?;
        match Module::new(&self.engine, bytes) {
            Ok(module) => Ok(CompiledModule {
                module: Some(module),
                exception_module: None,
                metadata,
            }),
            Err(_error) if module_uses_exceptions(bytes) => {
                let lowered;
                let runtime_bytes = if module_uses_legacy_exceptions(bytes) {
                    lowered = legacy_eh::lower(bytes)
                        .map_err(|error| wasmi::Error::new(error.to_string()))?;
                    lowered.as_slice()
                } else {
                    bytes
                };
                let module = wasmtime::Module::new(&self.exception_engine, runtime_bytes)
                    .map_err(|fallback| wasmi::Error::new(fallback.to_string()))?;
                Ok(CompiledModule {
                    module: None,
                    exception_module: Some(module),
                    metadata,
                })
            }
            Err(error) => Err(error),
        }
    }
}

#[derive(Trace, Finalize, boa_engine::JsData)]
struct CompiledModule {
    #[allow(dead_code)]
    #[unsafe_ignore_trace]
    module: Option<Module>,
    #[unsafe_ignore_trace]
    exception_module: Option<wasmtime::Module>,
    #[unsafe_ignore_trace]
    metadata: ModuleMetadata,
}

struct StoreData {
    callbacks: Vec<JsObject>,
    pending_error: Option<JsError>,
    instance: Option<Instance>,
}

struct RealmRuntime {
    store: Store<StoreData>,
}

struct ExceptionStoreData {
    callbacks: Vec<JsObject>,
    pending_error: Option<JsError>,
    roots: Vec<JsValue>,
}

struct ExceptionRuntime {
    store: wasmtime::Store<ExceptionStoreData>,
}

#[derive(Trace, Finalize, boa_engine::JsData)]
struct CompiledInstance {
    /// Keeps imported JavaScript callables live while Wasmi stores untraced
    /// clones in its host state.
    callbacks: Vec<JsObject>,
    #[unsafe_ignore_trace]
    reexported_imports: Vec<(String, usize)>,
    #[unsafe_ignore_trace]
    function_export_names: Vec<(String, String)>,
    #[unsafe_ignore_trace]
    instance: Option<Instance>,
    #[unsafe_ignore_trace]
    exception_instance: Option<wasmtime::Instance>,
    #[unsafe_ignore_trace]
    runtime: Rc<RefCell<RealmRuntime>>,
    #[unsafe_ignore_trace]
    exception_runtime: Option<Rc<RefCell<ExceptionRuntime>>>,
    export_handles: boa_gc::GcRefCell<Vec<JsObject>>,
}

#[derive(Trace, Finalize, boa_engine::JsData)]
struct RuntimeMemory {
    #[unsafe_ignore_trace]
    runtime: Rc<RefCell<RealmRuntime>>,
    #[unsafe_ignore_trace]
    memory: Memory,
    buffer: boa_gc::GcRefCell<Option<JsArrayBuffer>>,
}

#[derive(Trace, Finalize, boa_engine::JsData)]
struct RuntimeGlobal {
    #[unsafe_ignore_trace]
    runtime: Rc<RefCell<RealmRuntime>>,
    #[unsafe_ignore_trace]
    global: Global,
}

#[derive(Trace, Finalize, boa_engine::JsData)]
struct RuntimeTable {
    #[unsafe_ignore_trace]
    runtime: Rc<RefCell<RealmRuntime>>,
    #[unsafe_ignore_trace]
    table: Table,
    roots: boa_gc::GcRefCell<Vec<Option<JsValue>>>,
    values: boa_gc::GcRefCell<Vec<JsValue>>,
}

#[derive(Trace, Finalize, boa_engine::JsData)]
struct RuntimeFunc {
    #[unsafe_ignore_trace]
    function: Func,
}

#[derive(Trace, Finalize, boa_engine::JsData)]
struct RuntimeFuncMarker;

impl RuntimeTable {
    fn reference(&self, value: JsValue) -> JsResult<Ref> {
        let mut runtime = self.runtime.borrow_mut();
        let element = self.table.ty(&runtime.store).element();
        if element == RefType::Func {
            if value.is_null() {
                return Ok(Ref::null(RefType::Func));
            }
            if let Some(object) = value.as_object() {
                if let Some(function) = object.downcast_ref::<RuntimeFunc>() {
                    return Ok(Ref::Func(Nullable::Val(function.function)));
                }
            }
            return Err(JsNativeError::typ()
                .with_message("WebAssembly.Table funcref values must be null or Wasm functions")
                .into());
        }
        if value.is_null() {
            return Ok(Ref::null(RefType::Extern));
        }
        let mut roots = self.roots.borrow_mut();
        let index = roots.len();
        roots.push(Some(value));
        let reference = ExternRef::new(&mut runtime.store, index);
        Ok(Ref::Extern(Nullable::Val(reference)))
    }

    fn value(&self, reference: Ref) -> JsResult<JsValue> {
        match reference {
            Ref::Extern(Nullable::Null) | Ref::Func(Nullable::Null) => Ok(JsValue::null()),
            Ref::Extern(Nullable::Val(reference)) => {
                let runtime = self.runtime.borrow();
                let index = reference
                    .data(&runtime.store)
                    .downcast_ref::<usize>()
                    .copied()
                    .ok_or_else(|| JsNativeError::error().with_message("Invalid externref root"))?;
                self.roots
                    .borrow()
                    .get(index)
                    .and_then(Clone::clone)
                    .ok_or_else(|| {
                        JsNativeError::error()
                            .with_message("Missing externref root")
                            .into()
                    })
            }
            Ref::Func(_) => Err(JsNativeError::typ()
                .with_message("WebAssembly function table values are not exposed yet")
                .into()),
        }
    }
}

impl RuntimeMemory {
    fn buffer(&self, context: &mut Context) -> JsResult<JsArrayBuffer> {
        if let Some(buffer) = self.buffer.borrow().as_ref().cloned() {
            return Ok(buffer);
        }
        self.copy_store_to_buffer(context)
    }

    fn copy_buffer_to_store(&self) -> JsResult<()> {
        let Some(buffer) = self.buffer.borrow().as_ref().cloned() else {
            return Ok(());
        };
        let bytes = buffer
            .data()
            .ok_or_else(|| {
                JsNativeError::typ().with_message("WebAssembly.Memory buffer is detached")
            })?
            .to_vec();
        let mut runtime = self.runtime.try_borrow_mut().map_err(|_| {
            JsNativeError::error().with_message("WebAssembly realm is already executing")
        })?;
        let destination = self.memory.data_mut(&mut runtime.store);
        if destination.len() == bytes.len() {
            destination.copy_from_slice(&bytes);
        }
        Ok(())
    }

    fn copy_store_to_buffer(&self, context: &mut Context) -> JsResult<JsArrayBuffer> {
        let bytes = {
            let runtime = self.runtime.borrow();
            self.memory.data(&runtime.store).to_vec()
        };
        if let Some(buffer) = self.buffer.borrow().as_ref().cloned() {
            if buffer.byte_length() == bytes.len() {
                buffer
                    .data_mut()
                    .expect("live WebAssembly buffer must not be detached")
                    .copy_from_slice(&bytes);
                return Ok(buffer);
            }
            let _ = buffer.detach(&JsValue::undefined());
        }
        let block = boa_engine::object::builtins::AlignedVec::from_iter(0, bytes);
        let buffer = JsArrayBuffer::from_byte_block(block, context)?;
        *self.buffer.borrow_mut() = Some(buffer.clone());
        Ok(buffer)
    }
}

#[derive(Serialize)]
struct RuntimeExport {
    name: String,
    kind: &'static str,
    params: usize,
    element: Option<&'static str>,
    #[serde(rename = "functionName", skip_serializing_if = "Option::is_none")]
    function_name: Option<String>,
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
                            index: export.index,
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

    fn reexported_imports(&self) -> Vec<(String, usize)> {
        self.exports
            .iter()
            .filter_map(|export| {
                let import = self
                    .imports
                    .iter()
                    .enumerate()
                    .filter(|(_, import)| import.kind == export.kind)
                    .nth(export.index as usize)?;
                Some((export.name.clone(), import.0))
            })
            .collect()
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
    #[serde(skip)]
    index: u32,
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

fn extern_type_kind(ty: &ExternType) -> &'static str {
    match ty {
        ExternType::Func(_) => "function",
        ExternType::Table(_) => "table",
        ExternType::Memory(_) => "memory",
        ExternType::Global(_) => "global",
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
    let realm = context.realm().clone();
    let source = super::security_reporting::capture_csp_source(context);
    let (document_id, blocking, reporting) = with_host_state(|host| {
        let state = host.borrow();
        let (document_id, policy) = state.dynamic_code_policy_for_realm(&realm)?;
        let (blocking, reporting) = policy.wasm_compilation_violations();
        Ok((document_id, blocking, reporting))
    })?;
    with_host_state(|host| {
        let mut state = host.borrow_mut();
        state.queue_trusted_types_violation(
            document_id,
            "script-src",
            "wasm-eval",
            blocking.clone(),
            "enforce",
            "",
            source.as_ref(),
        );
        state.queue_trusted_types_violation(
            document_id,
            "script-src",
            "wasm-eval",
            reporting,
            "report",
            "",
            source.as_ref(),
        );
        Ok(())
    })?;
    if !blocking.is_empty() {
        return Err(JsNativeError::syntax()
            .with_message("Content Security Policy blocked WebAssembly compilation")
            .into());
    }
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
            "runtimeImports" => {
                serde_json::to_string(&if let Some(compiled) = module.module.as_ref() {
                    compiled
                        .imports()
                        .map(|import| RuntimeImport {
                            results: import.ty().func().map_or(0, |ty| ty.results().len()),
                        })
                        .collect::<Vec<_>>()
                } else {
                    module
                        .exception_module
                        .as_ref()
                        .expect("compiled module must have a runtime")
                        .imports()
                        .map(|import| RuntimeImport {
                            results: import.ty().func().map_or(0, |ty| ty.results().len()),
                        })
                        .collect::<Vec<_>>()
                })
            }
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

fn js_to_exception_wasm(
    value: &JsValue,
    ty: &wasmtime::ValType,
    context: &mut Context,
) -> JsResult<wasmtime::Val> {
    match ty {
        wasmtime::ValType::I32 => Ok(wasmtime::Val::I32(value.to_i32(context)?)),
        wasmtime::ValType::I64 => Ok(wasmtime::Val::I64(value.to_big_int64(context)?)),
        wasmtime::ValType::F32 => Ok(wasmtime::Val::F32(
            (value.to_number(context)? as f32).to_bits(),
        )),
        wasmtime::ValType::F64 => Ok(wasmtime::Val::F64(value.to_number(context)?.to_bits())),
        ty if ty.is_externref() && value.is_null_or_undefined() => {
            Ok(wasmtime::Val::ExternRef(None))
        }
        ty if ty.is_funcref() && value.is_null() => Ok(wasmtime::Val::FuncRef(None)),
        _ => Err(JsNativeError::typ()
            .with_message("Unsupported WebAssembly exception-runtime value type")
            .into()),
    }
}

fn exception_wasm_to_js(value: &wasmtime::Val) -> JsResult<JsValue> {
    match value {
        wasmtime::Val::I32(value) => Ok((*value).into()),
        wasmtime::Val::I64(value) => Ok(boa_engine::JsBigInt::from(*value).into()),
        wasmtime::Val::F32(value) => Ok((f32::from_bits(*value) as f64).into()),
        wasmtime::Val::F64(value) => Ok(f64::from_bits(*value).into()),
        wasmtime::Val::FuncRef(None)
        | wasmtime::Val::ExternRef(None)
        | wasmtime::Val::AnyRef(None)
        | wasmtime::Val::ExnRef(None)
        | wasmtime::Val::ContRef(None) => Ok(JsValue::null()),
        _ => Err(JsNativeError::typ()
            .with_message("Unsupported WebAssembly exception-runtime value type")
            .into()),
    }
}

fn js_to_exception_call_input(
    value: &JsValue,
    ty: &wasmtime::ValType,
    context: &mut Context,
    store: &mut wasmtime::Store<ExceptionStoreData>,
) -> JsResult<wasmtime::Val> {
    if ty.is_externref() && !value.is_null_or_undefined() {
        let index = store.data().roots.len();
        store.data_mut().roots.push(value.clone());
        let reference = wasmtime::ExternRef::new(&mut *store, index)
            .map_err(|error| JsNativeError::error().with_message(error.to_string()))?;
        return Ok(wasmtime::Val::ExternRef(Some(reference)));
    }
    js_to_exception_wasm(value, ty, context)
}

fn instantiate_exception_native(
    compiled: &CompiledModule,
    import_values: &JsObject,
    context: &mut Context,
) -> JsResult<JsValue> {
    let module = compiled
        .exception_module
        .as_ref()
        .expect("exception module dispatch requires an exception module");
    let runtime = with_host_state(|host| Ok(host.borrow().webassembly.exception_runtime.clone()))?;
    let mut callbacks = Vec::new();
    for (index, import) in compiled.metadata.imports.iter().enumerate() {
        let value = import_values
            .get(index, context)?
            .as_object()
            .ok_or_else(|| {
                JsNativeError::typ().with_message(format!(
                    "WebAssembly import {}.{} has an incompatible value",
                    import.module, import.name
                ))
            })?;
        if import.kind == "function" && !value.is_callable() {
            return Err(JsNativeError::typ()
                .with_message(format!(
                    "WebAssembly import {}.{} must be callable",
                    import.module, import.name
                ))
                .into());
        }
        callbacks.push(value);
    }
    let mut runtime_ref = runtime.try_borrow_mut().map_err(|_| {
        JsNativeError::error().with_message("WebAssembly realm is already executing")
    })?;
    let callback_offset = runtime_ref.store.data().callbacks.len();
    runtime_ref
        .store
        .data_mut()
        .callbacks
        .extend(callbacks.iter().cloned());
    runtime_ref
        .store
        .set_fuel(CALL_FUEL)
        .map_err(|error| JsNativeError::error().with_message(error.to_string()))?;
    let mut linker: wasmtime::Linker<ExceptionStoreData> = wasmtime::Linker::new(module.engine());
    for (index, import) in module.imports().enumerate() {
        match import.ty() {
            wasmtime::ExternType::Func(ty) => {
                let callback_index = callback_offset + index;
                let result_types = ty.results().collect::<Vec<_>>();
                linker
                    .func_new(
                        import.module(),
                        import.name(),
                        ty,
                        move |mut caller, inputs, outputs| {
                            let callback = caller.data().callbacks[callback_index].clone();
                            let result = ACTIVE_WASM_CONTEXT.with(|slot| {
                                let context = slot.get();
                                if context.is_null() {
                                    return Err(JsNativeError::error()
                                        .with_message(
                                            "WebAssembly callback has no active JavaScript context",
                                        )
                                        .into());
                                }
                                // SAFETY: ContextGuard limits this pointer to the synchronous call.
                                let context = unsafe { &mut *context };
                                let args = inputs
                                    .iter()
                                    .map(exception_wasm_to_js)
                                    .collect::<JsResult<Vec<_>>>()?;
                                let value = callback.call(&JsValue::undefined(), &args, context)?;
                                if result_types.is_empty() {
                                    return Ok(Vec::new());
                                }
                                if result_types.len() == 1 {
                                    return Ok(vec![js_to_exception_wasm(
                                        &value,
                                        &result_types[0],
                                        context,
                                    )?]);
                                }
                                let values = value.as_object().ok_or_else(|| {
                                    JsNativeError::typ().with_message(
                                        "WebAssembly multi-value result must be iterable",
                                    )
                                })?;
                                result_types
                                    .iter()
                                    .enumerate()
                                    .map(|(index, ty)| {
                                        js_to_exception_wasm(
                                            &values.get(index, context)?,
                                            ty,
                                            context,
                                        )
                                    })
                                    .collect()
                            });
                            match result {
                                Ok(values) => {
                                    outputs.clone_from_slice(&values);
                                    Ok(())
                                }
                                Err(error) => {
                                    caller.data_mut().pending_error = Some(error);
                                    Err(wasmtime::Error::msg("JavaScript WebAssembly import threw"))
                                }
                            }
                        },
                    )
                    .map_err(|error| {
                        JsNativeError::error().with_message(format!("{LINK_ERROR_PREFIX}{error}"))
                    })?;
            }
            wasmtime::ExternType::Tag(ty) => {
                let tag = wasmtime::Tag::new(&mut runtime_ref.store, &ty).map_err(|error| {
                    JsNativeError::error().with_message(format!("{LINK_ERROR_PREFIX}{error}"))
                })?;
                linker
                    .define(&runtime_ref.store, import.module(), import.name(), tag)
                    .map_err(|error| {
                        JsNativeError::error().with_message(format!("{LINK_ERROR_PREFIX}{error}"))
                    })?;
            }
            _ => {
                return Err(JsNativeError::error()
                    .with_message(format!(
                        "{LINK_ERROR_PREFIX}unsupported exception-module import {}.{}",
                        import.module(),
                        import.name()
                    ))
                    .into());
            }
        }
    }
    let _guard = ContextGuard::enter(context);
    let instance = linker
        .instantiate(&mut runtime_ref.store, module)
        .map_err(|error| {
            runtime_ref
                .store
                .data_mut()
                .pending_error
                .take()
                .unwrap_or_else(|| {
                    JsNativeError::error()
                        .with_message(format!("{LINK_ERROR_PREFIX}{error}"))
                        .into()
                })
        })?;
    drop(runtime_ref);
    Ok(JsObject::from_proto_and_data(
        None,
        CompiledInstance {
            callbacks,
            reexported_imports: compiled.metadata.reexported_imports(),
            function_export_names: compiled
                .metadata
                .exports
                .iter()
                .filter(|export| export.kind == "function")
                .map(|export| (export.name.clone(), export.index.to_string()))
                .collect(),
            instance: None,
            exception_instance: Some(instance),
            runtime: with_host_state(|host| Ok(host.borrow().webassembly.runtime.clone()))?,
            exception_runtime: Some(runtime),
            export_handles: boa_gc::GcRefCell::new(Vec::new()),
        },
    )
    .into())
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
    let compiled = module_handle
        .downcast_ref::<CompiledModule>()
        .ok_or_else(|| JsNativeError::typ().with_message("Invalid WebAssembly.Module handle"))?;
    if compiled.exception_module.is_some() {
        return instantiate_exception_native(&compiled, &import_values, context);
    }
    let module = compiled
        .module
        .clone()
        .expect("normal module dispatch requires a Wasmi module");
    let reexported_imports = compiled.metadata.reexported_imports();
    let function_export_names = compiled
        .metadata
        .exports
        .iter()
        .filter(|export| export.kind == "function")
        .map(|export| (export.name.clone(), export.index.to_string()))
        .collect();
    let mut matched_imports = vec![false; compiled.metadata.imports.len()];
    let wasmi_import_order = module
        .imports()
        .map(|import| {
            compiled
                .metadata
                .imports
                .iter()
                .enumerate()
                .find(|(index, descriptor)| {
                    !matched_imports[*index]
                        && descriptor.module == import.module()
                        && descriptor.name == import.name()
                        && descriptor.kind == extern_type_kind(import.ty())
                })
                .map(|(index, _)| {
                    matched_imports[index] = true;
                    index
                })
                .ok_or_else(|| {
                    JsError::from(
                        JsNativeError::error().with_message("WebAssembly import metadata mismatch"),
                    )
                })
        })
        .collect::<JsResult<Vec<_>>>()?;

    let engine = module.engine().clone();
    let runtime = with_host_state(|host| Ok(host.borrow().webassembly.runtime.clone()))?;
    let mut callbacks = Vec::new();
    for (index, import) in compiled.metadata.imports.iter().enumerate() {
        let value = import_values
            .get(index, context)?
            .as_object()
            .ok_or_else(|| {
                JsNativeError::typ().with_message(format!(
                    "WebAssembly import {}.{} has an incompatible value",
                    import.module, import.name
                ))
            })?;
        if import.kind == "function" && !value.is_callable() {
            return Err(JsNativeError::typ()
                .with_message(format!(
                    "WebAssembly import {}.{} must be callable",
                    import.module, import.name
                ))
                .into());
        }
        callbacks.push(value);
    }

    let mut runtime_ref = runtime.try_borrow_mut().map_err(|_| {
        JsNativeError::error().with_message("WebAssembly realm is already executing")
    })?;
    let store = &mut runtime_ref.store;
    let callback_offset = store.data().callbacks.len();
    store.data_mut().callbacks.extend(callbacks.iter().cloned());
    store
        .set_fuel(CALL_FUEL)
        .map_err(|error| JsNativeError::error().with_message(error.to_string()))?;
    let mut linker = Linker::new(&engine);
    let mut externals = Vec::new();
    for (index, import) in module.imports().enumerate() {
        let callback_index = wasmi_import_order[index];
        let value = &callbacks[callback_index];
        let external = match import.ty() {
            ExternType::Func(ty) => {
                imported_function(store, ty.clone(), callback_offset + callback_index).into()
            }
            ExternType::Memory(_) => value
                .downcast_ref::<RuntimeMemory>()
                .map(|memory| Extern::Memory(memory.memory))
                .ok_or_else(|| {
                    JsNativeError::typ().with_message(format!(
                        "WebAssembly import {}.{} must be a Memory from this realm",
                        import.module(),
                        import.name()
                    ))
                })?,
            ExternType::Global(_) => value
                .downcast_ref::<RuntimeGlobal>()
                .map(|global| Extern::Global(global.global))
                .ok_or_else(|| {
                    JsNativeError::typ().with_message(format!(
                        "WebAssembly import {}.{} must be a Global from this realm",
                        import.module(),
                        import.name()
                    ))
                })?,
            ExternType::Table(_) => value
                .downcast_ref::<RuntimeTable>()
                .map(|table| Extern::Table(table.table))
                .ok_or_else(|| {
                    JsNativeError::typ().with_message(format!(
                        "WebAssembly import {}.{} must be a Table from this realm",
                        import.module(),
                        import.name(),
                    ))
                })?,
        };
        linker
            .define(import.module(), import.name(), external)
            .map_err(|error| {
                JsNativeError::error().with_message(format!("{LINK_ERROR_PREFIX}{error}"))
            })?;
        externals.push(external);
    }
    let _guard = ContextGuard::enter(context);
    let instance = linker
        .instantiate_and_start(&mut *store, &module)
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
    drop(runtime_ref);
    Ok(JsObject::from_proto_and_data(
        None,
        CompiledInstance {
            callbacks,
            reexported_imports,
            function_export_names,
            instance: Some(instance),
            exception_instance: None,
            runtime,
            exception_runtime: None,
            export_handles: boa_gc::GcRefCell::new(Vec::new()),
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
    if let (Some(exception_instance), Some(exception_runtime)) = (
        instance.exception_instance,
        instance.exception_runtime.as_ref(),
    ) {
        let mut runtime = exception_runtime.borrow_mut();
        let exports = exception_instance
            .exports(&mut runtime.store)
            .map(|export| (export.name().to_owned(), export.into_extern()))
            .collect::<Vec<_>>()
            .into_iter()
            .map(|(export_name, export)| {
                let ty = export.ty(&runtime.store);
                RuntimeExport {
                    name: export_name.clone(),
                    kind: match ty {
                        wasmtime::ExternType::Func(_) => "function",
                        wasmtime::ExternType::Global(_) => "global",
                        wasmtime::ExternType::Memory(_) => "memory",
                        wasmtime::ExternType::Table(_) => "table",
                        wasmtime::ExternType::Tag(_) => "tag",
                    },
                    params: ty.func().map_or(0, |ty| ty.params().len()),
                    element: None,
                    function_name: ty.func().and_then(|_| {
                        instance
                            .function_export_names
                            .iter()
                            .find_map(|(name, function_name)| {
                                (name == &export_name).then(|| function_name.clone())
                            })
                    }),
                }
            })
            .collect::<Vec<_>>();
        return serde_json::to_string(&exports)
            .map(|json| js_string!(json).into())
            .map_err(|error| {
                JsNativeError::error()
                    .with_message(error.to_string())
                    .into()
            });
    }
    let runtime = instance.runtime.borrow();
    let exports = instance
        .instance
        .expect("normal instance must have a Wasmi instance")
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
                element: ty.table().map(|ty| match ty.element() {
                    RefType::Func => "funcref",
                    RefType::Extern => "externref",
                }),
                function_name: ty.func().and_then(|_| {
                    instance
                        .function_export_names
                        .iter()
                        .find_map(|(name, function_name)| {
                            (name == export.name()).then(|| function_name.clone())
                        })
                }),
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

fn instance_memory_handles(instance: &CompiledInstance) -> Vec<JsObject> {
    instance
        .callbacks
        .iter()
        .cloned()
        .chain(instance.export_handles.borrow().iter().cloned())
        .filter(|object| object.downcast_ref::<RuntimeMemory>().is_some())
        .collect()
}

fn sync_memories_to_store(instance: &CompiledInstance) -> JsResult<()> {
    for object in instance_memory_handles(instance) {
        object
            .downcast_ref::<RuntimeMemory>()
            .expect("memory handles were filtered")
            .copy_buffer_to_store()?;
    }
    Ok(())
}

fn sync_memories_from_store(instance: &CompiledInstance, context: &mut Context) -> JsResult<()> {
    for object in instance_memory_handles(instance) {
        object
            .downcast_ref::<RuntimeMemory>()
            .expect("memory handles were filtered")
            .copy_store_to_buffer(context)?;
    }
    Ok(())
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
    if instance.exception_instance.is_some() {
        return call_exception_native(&instance, &name, &js_args, context);
    }
    let normal_instance = instance
        .instance
        .expect("normal call dispatch requires a Wasmi instance");
    sync_memories_to_store(&instance)?;
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
                let function = normal_instance.get_func(&*caller, &name).ok_or_else(|| {
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
    runtime.store.data_mut().instance = Some(normal_instance);
    let function = normal_instance
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
        let pending_error = runtime.store.data_mut().pending_error.take();
        drop(runtime);
        sync_memories_from_store(&instance, context)?;
        if let Some(error) = pending_error {
            return Err(error);
        }
        return Err(JsNativeError::error()
            .with_message(format!("{RUNTIME_ERROR_PREFIX}{error}"))
            .into());
    }
    drop(runtime);
    sync_memories_from_store(&instance, context)?;
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

fn call_exception_native(
    instance: &CompiledInstance,
    name: &str,
    js_args: &JsObject,
    context: &mut Context,
) -> JsResult<JsValue> {
    let exception_instance = instance
        .exception_instance
        .expect("exception call dispatch requires an exception instance");
    let runtime = instance
        .exception_runtime
        .as_ref()
        .expect("exception instance must retain its runtime");
    let mut runtime = runtime.try_borrow_mut().map_err(|_| {
        JsNativeError::error().with_message("WebAssembly instance is already executing")
    })?;
    runtime
        .store
        .set_fuel(CALL_FUEL)
        .map_err(|error| JsNativeError::error().with_message(error.to_string()))?;
    let function = exception_instance
        .get_func(&mut runtime.store, name)
        .ok_or_else(|| JsNativeError::typ().with_message("Unknown WebAssembly function export"))?;
    let ty = function.ty(&runtime.store);
    let mut inputs = Vec::new();
    for (index, ty) in ty.params().enumerate() {
        let value = js_args.get(index, context)?;
        inputs.push(js_to_exception_call_input(
            &value,
            &ty,
            context,
            &mut runtime.store,
        )?);
    }
    let mut outputs = ty
        .results()
        .map(|ty| {
            wasmtime::Val::default_for_ty(&ty).ok_or_else(|| {
                JsNativeError::typ()
                    .with_message("Unsupported non-nullable WebAssembly exception-runtime result")
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let _guard = ContextGuard::enter(context);
    if let Err(error) = function.call(&mut runtime.store, &inputs, &mut outputs) {
        if let Some(error) = runtime.store.data_mut().pending_error.take() {
            return Err(error);
        }
        if error.is::<wasmtime::ThrownException>() {
            let _ = runtime.store.take_pending_exception();
            return Err(JsNativeError::error()
                .with_message(EXCEPTION_ERROR_PREFIX)
                .into());
        }
        return Err(JsNativeError::error()
            .with_message(format!("{RUNTIME_ERROR_PREFIX}{error}"))
            .into());
    }
    match outputs.as_slice() {
        [] => Ok(JsValue::undefined()),
        [value] => exception_wasm_to_js(value),
        values => Ok(JsArray::from_iter(
            values
                .iter()
                .map(exception_wasm_to_js)
                .collect::<JsResult<Vec<_>>>()?,
            context,
        )
        .into()),
    }
}

fn memory_new_native(_: &JsValue, args: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    let minimum = args.first().and_then(JsValue::as_number).unwrap_or(0.0) as u32;
    let maximum = args
        .get(1)
        .and_then(JsValue::as_number)
        .map(|value| value as u32);
    let runtime = with_host_state(|host| Ok(host.borrow().webassembly.runtime.clone()))?;
    let memory = {
        let mut runtime_ref = runtime.borrow_mut();
        Memory::new(&mut runtime_ref.store, MemoryType::new(minimum, maximum))
            .map_err(|error| JsNativeError::range().with_message(error.to_string()))?
    };
    Ok(JsObject::from_proto_and_data(
        None,
        RuntimeMemory {
            runtime,
            memory,
            buffer: boa_gc::GcRefCell::new(None),
        },
    )
    .into())
}

fn memory_buffer_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let object = args
        .first()
        .and_then(JsValue::as_object)
        .ok_or_else(|| JsNativeError::typ().with_message("Invalid WebAssembly.Memory handle"))?;
    let handle = object
        .downcast_ref::<RuntimeMemory>()
        .ok_or_else(|| JsNativeError::typ().with_message("Invalid WebAssembly.Memory handle"))?;
    let result = handle.buffer(context).map(Into::into);
    drop(handle);
    result
}

fn memory_grow_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let object = args
        .first()
        .and_then(JsValue::as_object)
        .ok_or_else(|| JsNativeError::typ().with_message("Invalid WebAssembly.Memory handle"))?;
    let handle = object
        .downcast_ref::<RuntimeMemory>()
        .ok_or_else(|| JsNativeError::typ().with_message("Invalid WebAssembly.Memory handle"))?;
    let delta = args.get(1).and_then(JsValue::as_number).unwrap_or(0.0) as u64;
    handle.copy_buffer_to_store()?;
    let previous = {
        let mut runtime = handle.runtime.borrow_mut();
        handle
            .memory
            .grow(&mut runtime.store, delta)
            .map_err(|error| JsNativeError::range().with_message(error.to_string()))?
    };
    handle.copy_store_to_buffer(context)?;
    Ok((previous as f64).into())
}

fn val_type(name: &str) -> JsResult<ValType> {
    match name {
        "i32" => Ok(ValType::I32),
        "i64" => Ok(ValType::I64),
        "f32" => Ok(ValType::F32),
        "f64" => Ok(ValType::F64),
        _ => Err(JsNativeError::typ()
            .with_message("Unsupported WebAssembly.Global value type")
            .into()),
    }
}

fn global_new_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let type_name = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .to_string(context)?
        .to_std_string_escaped();
    let ty = val_type(&type_name)?;
    let value = js_to_wasm(args.get(1).unwrap_or(&JsValue::undefined()), ty, context)?;
    let mutable = args.get(2).and_then(JsValue::as_boolean).unwrap_or(false);
    let runtime = with_host_state(|host| Ok(host.borrow().webassembly.runtime.clone()))?;
    let global = {
        let mut runtime_ref = runtime.borrow_mut();
        Global::new(
            &mut runtime_ref.store,
            value,
            if mutable {
                Mutability::Var
            } else {
                Mutability::Const
            },
        )
    };
    Ok(JsObject::from_proto_and_data(None, RuntimeGlobal { runtime, global }).into())
}

fn global_value_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let object = args
        .first()
        .and_then(JsValue::as_object)
        .ok_or_else(|| JsNativeError::typ().with_message("Invalid WebAssembly.Global handle"))?;
    let handle = object
        .downcast_ref::<RuntimeGlobal>()
        .ok_or_else(|| JsNativeError::typ().with_message("Invalid WebAssembly.Global handle"))?;
    if let Some(value) = args.get(1) {
        let mut runtime = handle.runtime.borrow_mut();
        let global_type = handle.global.ty(&runtime.store);
        if global_type.mutability() == Mutability::Const {
            return Err(JsNativeError::typ()
                .with_message("immutable global cannot be changed")
                .into());
        }
        let ty = global_type.content();
        let value = js_to_wasm(value, ty, context)?;
        handle
            .global
            .set(&mut runtime.store, value)
            .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
    }
    let runtime = handle.runtime.borrow();
    wasm_to_js(&handle.global.get(&runtime.store))
}

fn export_handle_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let object = args
        .first()
        .and_then(JsValue::as_object)
        .ok_or_else(|| JsNativeError::typ().with_message("Invalid WebAssembly.Instance handle"))?;
    let instance = object
        .downcast_ref::<CompiledInstance>()
        .ok_or_else(|| JsNativeError::typ().with_message("Invalid WebAssembly.Instance handle"))?;
    let name = args
        .get(2)
        .unwrap_or(&JsValue::undefined())
        .to_string(context)?
        .to_std_string_escaped();
    let kind = args
        .get(3)
        .unwrap_or(&JsValue::undefined())
        .to_string(context)?
        .to_std_string_escaped();
    if kind == "function" && instance.exception_instance.is_some() {
        return Ok(JsObject::from_proto_and_data(None, RuntimeFuncMarker).into());
    }
    let runtime = instance.runtime.borrow();
    let normal_instance = instance
        .instance
        .expect("normal export dispatch requires a Wasmi instance");
    let external = match kind.as_str() {
        "function" => normal_instance
            .get_func(&runtime.store, &name)
            .map(Extern::Func),
        "global" => normal_instance
            .get_global(&runtime.store, &name)
            .map(Extern::Global),
        "memory" => normal_instance
            .get_memory(&runtime.store, &name)
            .map(Extern::Memory),
        "table" => normal_instance
            .get_table(&runtime.store, &name)
            .map(Extern::Table),
        _ => None,
    }
    .ok_or_else(|| JsNativeError::typ().with_message("Unknown WebAssembly export"))?;
    if let Some(handle) = instance
        .reexported_imports
        .iter()
        .find_map(|(export, import)| (export == &name).then(|| instance.callbacks[*import].clone()))
    {
        drop(runtime);
        instance.export_handles.borrow_mut().push(handle.clone());
        return Ok(handle.into());
    }
    let handle = match external {
        Extern::Memory(memory) => JsObject::from_proto_and_data(
            None,
            RuntimeMemory {
                runtime: instance.runtime.clone(),
                memory,
                buffer: boa_gc::GcRefCell::new(None),
            },
        ),
        Extern::Global(global) => JsObject::from_proto_and_data(
            None,
            RuntimeGlobal {
                runtime: instance.runtime.clone(),
                global,
            },
        ),
        Extern::Table(table) => {
            let length = table.size(&runtime.store) as usize;
            JsObject::from_proto_and_data(
                None,
                RuntimeTable {
                    runtime: instance.runtime.clone(),
                    table,
                    roots: boa_gc::GcRefCell::new(Vec::new()),
                    values: boa_gc::GcRefCell::new(vec![JsValue::null(); length]),
                },
            )
        }
        Extern::Func(function) => JsObject::from_proto_and_data(None, RuntimeFunc { function }),
    };
    drop(runtime);
    instance.export_handles.borrow_mut().push(handle.clone());
    Ok(handle.into())
}

fn table_new_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let element = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .to_string(context)?
        .to_std_string_escaped();
    let element = match element.as_str() {
        "externref" => RefType::Extern,
        "funcref" | "anyfunc" => RefType::Func,
        _ => {
            return Err(JsNativeError::typ()
                .with_message("Invalid WebAssembly.Table element type")
                .into());
        }
    };
    let minimum = args.get(1).and_then(JsValue::as_number).unwrap_or(0.0) as u32;
    let maximum = args
        .get(2)
        .and_then(JsValue::as_number)
        .map(|value| value as u32);
    let initial = args.get(3).cloned().unwrap_or_else(JsValue::null);
    let public_initial = args.get(4).cloned().unwrap_or_else(|| initial.clone());
    let runtime = with_host_state(|host| Ok(host.borrow().webassembly.runtime.clone()))?;
    let roots = boa_gc::GcRefCell::new(Vec::new());
    let table = {
        let mut runtime_ref = runtime.borrow_mut();
        let init = if element == RefType::Extern && !initial.is_null() {
            roots.borrow_mut().push(Some(initial.clone()));
            Ref::Extern(Nullable::Val(ExternRef::new(
                &mut runtime_ref.store,
                0usize,
            )))
        } else {
            Ref::null(element)
        };
        Table::new(
            &mut runtime_ref.store,
            TableType::new(element, minimum, maximum),
            init,
        )
        .map_err(|error| JsNativeError::range().with_message(error.to_string()))?
    };
    Ok(JsObject::from_proto_and_data(
        None,
        RuntimeTable {
            runtime,
            table,
            roots,
            values: boa_gc::GcRefCell::new(vec![public_initial; minimum as usize]),
        },
    )
    .into())
}

fn table_op_native(_: &JsValue, args: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    let object = args
        .first()
        .and_then(JsValue::as_object)
        .ok_or_else(|| JsNativeError::typ().with_message("Invalid WebAssembly.Table handle"))?;
    let handle = object
        .downcast_ref::<RuntimeTable>()
        .ok_or_else(|| JsNativeError::typ().with_message("Invalid WebAssembly.Table handle"))?;
    let operation = args
        .get(1)
        .and_then(JsValue::as_string)
        .map(|value| value.to_std_string_escaped());
    let index = args.get(2).and_then(JsValue::as_number).unwrap_or(0.0) as u64;
    match operation.as_deref() {
        Some("length") => Ok((handle.table.size(&handle.runtime.borrow().store) as f64).into()),
        Some("get") => {
            let runtime = handle.runtime.borrow();
            let reference = handle.table.get(&runtime.store, index).ok_or_else(|| {
                JsNativeError::range().with_message("WebAssembly.Table index out of bounds")
            })?;
            if handle.table.ty(&runtime.store).element() == RefType::Extern {
                drop(runtime);
                return handle.value(reference);
            }
            handle
                .values
                .borrow()
                .get(index as usize)
                .cloned()
                .ok_or_else(|| {
                    JsNativeError::range()
                        .with_message("WebAssembly.Table index out of bounds")
                        .into()
                })
        }
        Some("set") => {
            let public_value = args.get(3).cloned().unwrap_or_else(JsValue::null);
            let native_value = args.get(4).cloned().unwrap_or_else(|| public_value.clone());
            let reference = handle.reference(native_value)?;
            handle
                .table
                .set(&mut handle.runtime.borrow_mut().store, index, reference)
                .map_err(|error| JsNativeError::range().with_message(error.to_string()))?;
            handle.values.borrow_mut()[index as usize] = public_value;
            Ok(JsValue::undefined())
        }
        Some("grow") => {
            let public_value = args.get(3).cloned().unwrap_or_else(JsValue::null);
            let native_value = args.get(4).cloned().unwrap_or_else(|| public_value.clone());
            let reference = handle.reference(native_value)?;
            let previous = handle
                .table
                .grow(&mut handle.runtime.borrow_mut().store, index, reference)
                .map_err(|error| JsNativeError::range().with_message(error.to_string()))?;
            handle
                .values
                .borrow_mut()
                .extend(std::iter::repeat_n(public_value, index as usize));
            Ok((previous as f64).into())
        }
        _ => Err(JsNativeError::typ()
            .with_message("Unknown WebAssembly.Table operation")
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
        (
            js_string!("__omoikane_wasm_memory_new"),
            2,
            NativeFunction::from_copy_closure(memory_new_native),
        ),
        (
            js_string!("__omoikane_wasm_memory_buffer"),
            1,
            NativeFunction::from_copy_closure(memory_buffer_native),
        ),
        (
            js_string!("__omoikane_wasm_memory_grow"),
            2,
            NativeFunction::from_copy_closure(memory_grow_native),
        ),
        (
            js_string!("__omoikane_wasm_global_new"),
            3,
            NativeFunction::from_copy_closure(global_new_native),
        ),
        (
            js_string!("__omoikane_wasm_global_value"),
            2,
            NativeFunction::from_copy_closure(global_value_native),
        ),
        (
            js_string!("__omoikane_wasm_export_handle"),
            4,
            NativeFunction::from_copy_closure(export_handle_native),
        ),
        (
            js_string!("__omoikane_wasm_table_new"),
            5,
            NativeFunction::from_copy_closure(table_new_native),
        ),
        (
            js_string!("__omoikane_wasm_table_op"),
            5,
            NativeFunction::from_copy_closure(table_op_native),
        ),
    ] {
        register_private_builtin_callable(context, bindings, name, length, function)?;
    }
    Ok(())
}
