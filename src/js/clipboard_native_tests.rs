//! Finite lexical input capabilities used only by clipboard unit fixtures.
use super::*;
use std::cell::Cell;
use std::sync::Mutex;

pub(super) static CLIPBOARD_TEST_LOCK: Mutex<()> = Mutex::new(());

#[derive(Default)]
struct Progress {
    cursor: Cell<usize>,
    busy: Cell<bool>,
    revoked: Cell<bool>,
}

#[derive(Trace, Finalize)]
struct Captures {
    callback: JsValue,
    #[unsafe_ignore_trace]
    host: Weak<RefCell<HostState>>,
    #[unsafe_ignore_trace]
    document: usize,
    #[unsafe_ignore_trace]
    keys: Vec<host_input_types::HostKeyInput>,
    #[unsafe_ignore_trace]
    progress: Rc<Progress>,
}

struct InputPlan {
    function: Rooted<JsObject>,
    progress: Rc<Progress>,
    expected: usize,
}

impl Drop for InputPlan {
    fn drop(&mut self) {
        self.progress.revoked.set(true);
    }
}

struct BusyGuard(Rc<Progress>);
impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.busy.set(false);
    }
}

fn plan_error(message: &'static str) -> JsError {
    JsNativeError::typ().with_message(message).into()
}

fn next_input(
    _: &JsValue,
    args: &[JsValue],
    captures: &Captures,
    context: &mut Context,
) -> JsResult<JsValue> {
    if !args.is_empty() {
        return Err(plan_error("input plan accepts no page-supplied arguments"));
    }
    if captures.progress.revoked.get() || captures.progress.busy.get() {
        return Err(plan_error("input plan is revoked or reentrant"));
    }
    let index = captures.progress.cursor.get();
    let key = captures
        .keys
        .get(index)
        .ok_or_else(|| plan_error("input plan exhausted"))?
        .clone();
    // A failed attempt is consumed too; it cannot be retried under changed state.
    captures.progress.cursor.set(index + 1);
    captures.progress.busy.set(true);
    let _busy = BusyGuard(Rc::clone(&captures.progress));
    let host = captures
        .host
        .upgrade()
        .ok_or_else(|| plan_error("input owner is gone"))?;
    with_host_state(|active| {
        if !Rc::ptr_eq(active, &host) {
            return Err(plan_error("input plan belongs to another runtime"));
        }
        Ok(())
    })?;
    let client = author_script_realm(context).and_then(|realm| {
        realm
            .host_defined()
            .get::<ModuleDocumentId>()
            .map(|id| id.0)
    });
    if client != Some(captures.document) {
        return Err(plan_error("input plan belongs to another author Document"));
    }
    let prepared = {
        let state = host.borrow();
        if !state.document_is_active(captures.document) {
            return Err(plan_error("input Document retired"));
        }
        host_input_command::prepare(
            &state,
            &host_input_types::HostInputCommand::Keyboard {
                phase: host_input_types::HostKeyPhase::Down,
                input: key,
            },
        )?
    };
    // No HostState/plan RefCell borrow survives invocation of author listeners.
    let callback = Rooted::new(captures.callback.clone());
    let values = prepared
        .arguments
        .iter()
        .map(|value| JsValue::from_json(value, context))
        .collect::<JsResult<Vec<_>>>()?;
    let values = JsArray::from_iter(values, context);
    callback.as_callable().unwrap().call(
        &JsValue::undefined(),
        &[
            js_string!("keyboard").into(),
            values.into(),
            JsValue::null(),
            JsValue::null(),
        ],
        context,
    )
}

impl InputPlan {
    fn new(runtime: &mut JsRuntime, keys: Vec<host_input_types::HostKeyInput>) -> Self {
        let document = runtime.document().identity();
        Self::new_for_document(runtime, document, keys)
    }

    fn new_for_document(
        runtime: &mut JsRuntime,
        document: usize,
        keys: Vec<host_input_types::HostKeyInput>,
    ) -> Self {
        let callback = runtime
            .host_state
            .borrow()
            .input_bridge
            .retained_dispatcher_for_test(document)
            .unwrap();
        // Root while allocating the native function that will trace its capture.
        let callback = Rooted::new(callback);
        let progress = Rc::new(Progress::default());
        let expected = keys.len();
        let captures = Captures {
            callback: (*callback).clone(),
            host: Rc::downgrade(&runtime.host_state),
            document,
            keys,
            progress: Rc::clone(&progress),
        };
        let function = FunctionObjectBuilder::new(
            runtime.context.realm(),
            NativeFunction::from_copy_closure_with_captures(next_input, captures),
        )
        .name(js_string!("nextInput"))
        .length(0)
        .build();
        Self {
            function: Rooted::new(function.into()),
            progress,
            expected,
        }
    }
}

/// Keeps a fixed test-only input plan alive across the caller's existing jobs.
pub(super) fn with_input_plan<T>(
    runtime: &mut JsRuntime,
    keys: Vec<host_input_types::HostKeyInput>,
    run: impl FnOnce(&mut JsRuntime, JsValue) -> T,
) -> (T, bool) {
    let plan = InputPlan::new(runtime, keys);
    let result = run(runtime, (*plan.function).clone().into());
    let consumed = plan.progress.cursor.get() == plan.expected;
    (result, consumed)
}

/// Executes the fixed plan synchronously without introducing a job checkpoint.
pub(super) fn run_synchronous_input_fixture(
    runtime: &mut JsRuntime,
    script: &str,
    keys: Vec<host_input_types::HostKeyInput>,
) {
    let (_, consumed) = with_input_plan(runtime, keys, |runtime, next| {
        runtime.call_function_with_value(script, next).unwrap();
    });
    assert!(consumed, "all fixed synchronous inputs executed");
}

fn run_fixture(runtime: &mut JsRuntime, script: &str, keys: Vec<host_input_types::HostKeyInput>) {
    let plan = InputPlan::new(runtime, keys);
    let value = runtime
        .call_function_with_value(script, (*plan.function).clone().into())
        .unwrap();
    let value = Rooted::new(value);
    runtime.run_until_idle().unwrap();
    let promise =
        JsPromise::from_object(value.as_object().expect("fixture Promise").clone()).unwrap();
    assert!(
        matches!(promise.state(), PromiseState::Fulfilled(_)),
        "fixture and finally must fulfill: {:?}",
        promise.state()
    );
    assert_eq!(
        plan.progress.cursor.get(),
        plan.expected,
        "all planned inputs executed"
    );
    assert_eq!(
        runtime.host_state.borrow().clipboard.read_text(),
        "",
        "finally cleared clipboard"
    );
    // Drop revokes any callback retained by test JavaScript after completion.
}

use super::host_input_types::{HostInputCommand, HostInputModifiers, HostKeyInput, HostKeyPhase};
use crate::{html::TreeBuilder, js::JsRuntime};

fn runtime(markup: &str) -> JsRuntime {
    JsRuntime::with_document_and_url(
        TreeBuilder::parse(markup).document(),
        "https://clipboard.example.test/",
    )
    .unwrap()
}

fn press_key(runtime: &mut JsRuntime, key: &str, text: &str, shift: bool) {
    let document = runtime.document();
    runtime
        .dispatch_host_input(
            &document,
            HostInputCommand::Keyboard {
                phase: HostKeyPhase::Down,
                input: HostKeyInput {
                    key: key.into(),
                    text: text.into(),
                    modifiers: HostInputModifiers {
                        shift,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            },
        )
        .unwrap();
}

#[test]
fn copy_and_cut_use_the_focused_text_control_selection() {
    let _lock = CLIPBOARD_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut runtime = runtime("<!doctype html><input id='field' value='alpha beta'>");
    run_fixture(
        &mut runtime,
        r#"(nextInput) => {
globalThis.result = 'pending';
      return (async () => {
        const field = document.getElementById('field'), seen = [], cutEvents = [];
        field.focus(); field.setSelectionRange(0, 5);
        for (const type of ['copy', 'cut']) field.addEventListener(type, event => {
          seen.push([type, event.isTrusted, event.bubbles, event.cancelable,
            event.composed, event.clipboardData.getData('text/plain')].join(':'));
          globalThis.lastClipboardData = event.clipboardData;
        });
        nextInput();
        if (await navigator.clipboard.readText() !== 'alpha' || field.value !== 'alpha beta') {
          result = 'copy default'; return;
        }
        field.setSelectionRange(6, 10);
        field.addEventListener('beforeinput', event => cutEvents.push(event.type + ':' + event.inputType));
        field.addEventListener('input', event => cutEvents.push(event.type + ':' + event.inputType));
        nextInput();
        const text = await navigator.clipboard.readText();
        result = JSON.stringify({seen, cutEvents, value:field.value, caret:field.selectionStart, text,
          expired:lastClipboardData.getData('text/plain')});
      })().finally(() => navigator.clipboard.writeText(''));
    }"#,
        vec![
            HostKeyInput {
                key: "c".into(),
                text: "".into(),
                modifiers: HostInputModifiers {
                    control: true,
                    meta: false,
                    ..Default::default()
                },
                ..Default::default()
            },
            HostKeyInput {
                key: "x".into(),
                text: "".into(),
                modifiers: HostInputModifiers {
                    control: true,
                    meta: false,
                    ..Default::default()
                },
                ..Default::default()
            },
        ],
    );
    assert_eq!(
        runtime
            .eval("result")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        r#"{"seen":["copy:true:true:true:true:","cut:true:true:true:true:"],"cutEvents":["beforeinput:deleteByCut","input:deleteByCut"],"value":"alpha ","caret":6,"text":"beta","expired":""}"#
    );
}

#[test]
fn canceled_copy_commits_handler_data_and_canceled_cut_preserves_content() {
    let _lock = CLIPBOARD_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut runtime = runtime("<!doctype html><textarea id='field'>original</textarea>");
    run_fixture(
        &mut runtime,
        r#"(nextInput) => {
globalThis.result = 'pending';
      return (async () => {
        const field = document.getElementById('field');
        field.focus(); field.select();
        field.addEventListener('copy', event => {
          event.clipboardData.setData('text/plain', 'handler text');
          event.clipboardData.setData('text/html', '<b>handler</b>');
          event.preventDefault();
        }, {once:true});
        nextInput();
        const item = (await navigator.clipboard.read())[0];
        const plain = await (await item.getType('text/plain')).text();
        const html = await (await item.getType('text/html')).text();
        field.addEventListener('cut', event => event.preventDefault(), {once:true});
        nextInput();
        result = plain + '|' + html + '|' + field.value;
      })().finally(() => navigator.clipboard.writeText(''));
    }"#,
        vec![
            HostKeyInput {
                key: "c".into(),
                text: "".into(),
                modifiers: HostInputModifiers {
                    control: false,
                    meta: true,
                    ..Default::default()
                },
                ..Default::default()
            },
            HostKeyInput {
                key: "x".into(),
                text: "".into(),
                modifiers: HostInputModifiers {
                    control: true,
                    meta: false,
                    ..Default::default()
                },
                ..Default::default()
            },
        ],
    );
    assert_eq!(
        runtime
            .eval("result")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "handler text|<b>handler</b>|original"
    );
}

#[test]
fn paste_is_readonly_cancelable_and_uses_the_text_editing_event_path() {
    let _lock = CLIPBOARD_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut runtime = runtime("<!doctype html><input id='field' value='ab'>");
    run_fixture(
        &mut runtime,
        r#"(nextInput) => {
globalThis.result = 'pending';
      return (async () => {
        await navigator.clipboard.writeText('PASTE');
        const field = document.getElementById('field'), seen = [];
        field.focus(); field.setSelectionRange(1, 1);
        field.addEventListener('paste', event => {
          seen.push('paste:' + event.isTrusted + ':' + event.clipboardData.getData('text/plain'));
          event.clipboardData.setData('text/plain', 'mutated');
          seen.push('readonly:' + event.clipboardData.getData('text/plain'));
        });
        field.addEventListener('beforeinput', event => seen.push(event.type + ':' + event.inputType + ':' + event.data));
        field.addEventListener('input', event => seen.push(event.type + ':' + event.inputType + ':' + event.data));
        nextInput();
        field.addEventListener('paste', event => event.preventDefault(), {once:true, capture:true});
        nextInput();
        result = JSON.stringify({value:field.value, caret:field.selectionStart, seen});
      })().finally(() => navigator.clipboard.writeText(''));
    }"#,
        vec![
            HostKeyInput {
                key: "v".into(),
                text: "".into(),
                modifiers: HostInputModifiers {
                    control: true,
                    meta: false,
                    ..Default::default()
                },
                ..Default::default()
            },
            HostKeyInput {
                key: "v".into(),
                text: "".into(),
                modifiers: HostInputModifiers {
                    control: true,
                    meta: false,
                    ..Default::default()
                },
                ..Default::default()
            },
        ],
    );
    assert_eq!(
        runtime
            .eval("result")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        r#"{"value":"aPASTEb","caret":6,"seen":["paste:true:PASTE","readonly:PASTE","beforeinput:insertFromPaste:PASTE","input:insertFromPaste:PASTE","paste:true:PASTE","readonly:PASTE"]}"#
    );
}

#[test]
fn contenteditable_keyboard_and_clipboard_share_document_selection() {
    let _lock = CLIPBOARD_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut runtime =
        runtime("<!doctype html><div id='host' contenteditable>hello <b>world</b></div>");
    run_fixture(
        &mut runtime,
        r#"(nextInput) => {
globalThis.result = 'pending';
      return (async () => {
        const host = document.getElementById('host');
        const selection = getSelection(), events = [];
        host.focus();
        const selected = document.createRange(); selected.selectNode(host.lastChild);
        selection.removeAllRanges(); selection.addRange(selected);
        const record = event => events.push(event.type + ':' + event.inputType + ':' + event.data + ':' +
          event.getTargetRanges().length + ':' + (event.dataTransfer ?
            event.dataTransfer.types.join(',') + '=' + event.dataTransfer.getData('text/html') : '-'));
        host.addEventListener('beforeinput', record);
        host.addEventListener('input', record);
        nextInput();
        const copiedItem = (await navigator.clipboard.read())[0];
        const copied = await (await copiedItem.getType('text/plain')).text();
        const copiedHtml = await (await copiedItem.getType('text/html')).text();
        nextInput();
        nextInput();
        await navigator.clipboard.write([new ClipboardItem({
          'text/plain': new Blob(['there'], {type:'text/plain'}),
          'text/html': new Blob(['<b>there</b>'], {type:'text/html'}),
        })]);
        nextInput();
        result = JSON.stringify({copied, copiedHtml, text:host.textContent, events,
          collapsed:selection.isCollapsed, offset:selection.focusOffset});
      })().finally(() => navigator.clipboard.writeText(''));
    }"#,
        vec![
            HostKeyInput {
                key: "c".into(),
                text: "".into(),
                modifiers: HostInputModifiers {
                    control: true,
                    meta: false,
                    ..Default::default()
                },
                ..Default::default()
            },
            HostKeyInput {
                key: "Backspace".into(),
                text: "".into(),
                modifiers: HostInputModifiers {
                    control: false,
                    meta: false,
                    ..Default::default()
                },
                ..Default::default()
            },
            HostKeyInput {
                key: "!".into(),
                text: "!".into(),
                modifiers: HostInputModifiers {
                    control: false,
                    meta: false,
                    ..Default::default()
                },
                ..Default::default()
            },
            HostKeyInput {
                key: "v".into(),
                text: "".into(),
                modifiers: HostInputModifiers {
                    control: true,
                    meta: false,
                    ..Default::default()
                },
                ..Default::default()
            },
        ],
    );
    assert_eq!(
        runtime
            .eval("result")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        r#"{"copied":"world","copiedHtml":"<b>world</b>","text":"hello !there","events":["beforeinput:deleteContentBackward:null:1:-","input:deleteContentBackward:null:0:-","beforeinput:insertText:!:1:-","input:insertText:!:0:-","beforeinput:insertFromPaste:null:1:text/plain,text/html=<b>there</b>","input:insertFromPaste:null:0:text/plain,text/html=<b>there</b>"],"collapsed":true,"offset":5}"#
    );
}

#[test]
fn editing_host_caret_crosses_text_nodes_and_design_mode_accepts_input() {
    let _lock = CLIPBOARD_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut runtime = runtime(
        "<!doctype html><div id='host' contenteditable><span>ab</span><span>cd</span></div><p id='design'>xy</p>",
    );
    runtime
        .eval(
            r#"const host = document.getElementById('host');
      const first = host.firstChild.firstChild, second = host.lastChild.firstChild;
      const selection = getSelection();
      host.focus(); selection.collapse(second, 0);"#,
        )
        .unwrap();
    press_key(&mut runtime, "Backspace", "", false);
    press_key(&mut runtime, "ArrowRight", "", false);
    press_key(&mut runtime, "ArrowRight", "", true);
    press_key(&mut runtime, "X", "X", false);
    runtime.eval(r#"document.designMode = 'on';
      const design = document.getElementById('design');
      const designEvents = [];
      document.addEventListener('beforeinput', event => designEvents.push(event.type + ':' + event.inputType + ':' + event.data));
      document.addEventListener('input', event => designEvents.push(event.type + ':' + event.inputType + ':' + event.data));
      design.focus(); selection.collapse(design.firstChild, 1);"#).unwrap();
    press_key(&mut runtime, "!", "!", false);
    press_key(&mut runtime, "Enter", "", false);
    let result = runtime
        .eval(
            r#"JSON.stringify({host:host.textContent, design:design.textContent,
        designEvents, collapsed:selection.isCollapsed, offset:selection.focusOffset})"#,
        )
        .unwrap();
    assert_eq!(
        result.as_string().unwrap().to_std_string_escaped(),
        r#"{"host":"aXd","design":"x!\ny","designEvents":["beforeinput:insertText:!","input:insertText:!","beforeinput:insertParagraph:null","input:insertParagraph:null"],"collapsed":true,"offset":1}"#
    );
}

fn test_key() -> HostKeyInput {
    HostKeyInput {
        key: "a".into(),
        ..Default::default()
    }
}

#[test]
fn lexical_plan_is_finite_and_drop_revokes_retained_callback_after_gc() {
    let mut runtime = runtime("<body></body>");
    let plan = InputPlan::new(&mut runtime, vec![test_key()]);
    assert_eq!(
        runtime
            .eval("typeof nextInput")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "undefined"
    );
    let result = runtime.call_function_with_value(r#"next => {
        globalThis.retainedInput = next;
        globalThis.inputCount = 0;
        document.body.onkeydown = () => inputCount++;
        let argsRejected = false, exhausted = false;
        try { next('author chosen input'); } catch (error) { argsRejected = error instanceof TypeError; }
        next();
        try { next(); } catch (error) { exhausted = error instanceof TypeError; }
        return argsRejected && exhausted && inputCount === 1;
    }"#, (*plan.function).clone().into()).unwrap();
    assert_eq!(result.as_boolean(), Some(true));
    assert_eq!(plan.progress.cursor.get(), 1);
    drop(plan);
    runtime.context.clear_kept_objects();
    boa_gc::force_collect();
    assert_eq!(runtime.eval("try { retainedInput(); false; } catch (error) { error instanceof TypeError && String(error).includes('revoked') && inputCount === 1; }").unwrap().as_boolean(), Some(true));
}

#[test]
fn lexical_plan_rejects_listener_reentry_without_consuming_next_command() {
    let mut runtime = runtime("<body></body>");
    let plan = InputPlan::new(&mut runtime, vec![test_key(), test_key()]);
    let result = runtime
        .call_function_with_value(
            r#"next => {
        let deliveries = 0, rejections = 0;
        document.body.onkeydown = () => {
            deliveries++;
            try { next(); } catch (error) { if (error instanceof TypeError) rejections++; }
        };
        next(); next();
        return deliveries === 2 && rejections === 2;
    }"#,
            (*plan.function).clone().into(),
        )
        .unwrap();
    assert_eq!(result.as_boolean(), Some(true));
    assert_eq!(plan.progress.cursor.get(), 2);
    assert!(!plan.progress.busy.get());
}

#[test]
fn lexical_plan_rejects_retired_entry_after_await_and_collection() {
    let mut runtime = runtime("<body><iframe id=f></iframe></body>");
    runtime.eval("globalThis.deliveries=0; globalThis.frame=document.getElementById('f'); frame.contentDocument.body.onkeydown=()=>deliveries++; frame.focus();").unwrap();
    let frame = runtime.document().query_selector("#f").unwrap();
    let child = runtime.frame_content_document(&frame).unwrap();
    let realm = runtime
        .realm_for_document(child.identity())
        .unwrap()
        .unwrap();
    let previous = runtime.context.enter_realm(realm);
    let plan =
        InputPlan::new_for_document(&mut runtime, child.identity(), vec![test_key(), test_key()]);
    let promise = runtime
        .call_function_with_value(
            "next => (async () => { next(); await Promise.resolve(); next(); })()",
            (*plan.function).clone().into(),
        )
        .unwrap();
    let promise = Rooted::new(promise);
    runtime.context.enter_realm(previous);
    assert_eq!(
        runtime
            .eval("document.activeElement === frame && frame.contentDocument.hasFocus()")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    assert_eq!(
        runtime.eval("deliveries").unwrap().as_number(),
        Some(1.0),
        "active child receives the pre-await input"
    );
    runtime
        .eval("frame.srcdoc='<p>replacement</p>'; void frame.contentDocument;")
        .unwrap();
    assert!(
        !runtime
            .host_state
            .borrow()
            .document_is_active(child.identity())
    );
    runtime.context.clear_kept_objects();
    boa_gc::force_collect();
    runtime.run_until_idle().unwrap();
    let promise = JsPromise::from_object(promise.as_object().unwrap().clone()).unwrap();
    match promise.state() {
        PromiseState::Rejected(error) => assert!(
            error
                .display()
                .to_string()
                .contains("input Document retired"),
            "{error:?}"
        ),
        other => panic!("retired input must reject: {other:?}"),
    }
    assert_eq!(plan.progress.cursor.get(), 2);
    assert_eq!(runtime.eval("deliveries").unwrap().as_number(), Some(1.0));
}

#[test]
fn lexical_plan_pending_promise_is_not_success_and_unwind_revokes() {
    let _lock = CLIPBOARD_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut runtime = runtime("<body></body>");
    runtime
        .host_state
        .borrow()
        .clipboard
        .write_text(String::new())
        .unwrap();
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_fixture(
            &mut runtime,
            "next => { globalThis.retainedInput=next; next(); return new Promise(()=>{}); }",
            vec![test_key()],
        );
    }));
    assert!(
        failure.is_err(),
        "pending Promise cannot satisfy fixture completion"
    );
    assert_eq!(
        runtime
            .eval("try { retainedInput(); false; } catch (error) { error instanceof TypeError && String(error).includes('revoked'); }")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn lexical_plan_rejected_promise_runs_finally_and_unwind_revokes() {
    let _lock = CLIPBOARD_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut runtime = runtime("<body></body>");
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_fixture(
            &mut runtime,
            "next => { globalThis.retainedInput=next; globalThis.cleanup=false; return Promise.reject(new Error('fixture failed')).finally(()=>{cleanup=true;}); }",
            vec![test_key()],
        );
    }));
    assert!(
        failure.is_err(),
        "rejected fixture cannot satisfy completion"
    );
    assert_eq!(runtime.eval("cleanup").unwrap().as_boolean(), Some(true));
    assert_eq!(runtime.eval("try { retainedInput(); false; } catch (error) { error instanceof TypeError && String(error).includes('revoked'); }").unwrap().as_boolean(), Some(true));
}

#[test]
fn lexical_plan_wrong_runtime_attempt_is_consumed_and_not_replayed() {
    let mut owner = runtime("<body></body>");
    let mut other = runtime("<body></body>");
    let plan = InputPlan::new(&mut owner, vec![test_key()]);
    let result = other.call_function_with_value("next => { try { next(); return 'unexpected'; } catch (error) { return String(error); } }", (*plan.function).clone().into()).unwrap();
    assert!(
        result
            .as_string()
            .unwrap()
            .to_std_string_escaped()
            .contains("another runtime")
    );
    assert_eq!(plan.progress.cursor.get(), 1);
    assert!(!plan.progress.busy.get());
    let result = owner.call_function_with_value("next => { try { next(); return 'unexpected'; } catch (error) { return String(error); } }", (*plan.function).clone().into()).unwrap();
    assert!(
        result
            .as_string()
            .unwrap()
            .to_std_string_escaped()
            .contains("exhausted")
    );
}

#[test]
fn lexical_plan_rejects_another_author_document_in_same_runtime() {
    let mut runtime = runtime("<body><iframe id=f></iframe></body>");
    runtime.eval("globalThis.deliveries=0; document.body.onkeydown=()=>deliveries++; document.getElementById('f').contentDocument;").unwrap();
    let plan = InputPlan::new(&mut runtime, vec![test_key()]);
    let frame = runtime.document().query_selector("#f").unwrap();
    let child = runtime.frame_content_document(&frame).unwrap();
    let realm = runtime
        .realm_for_document(child.identity())
        .unwrap()
        .unwrap();
    let previous = runtime.context.enter_realm(realm);
    let result = runtime.call_function_with_value("next => { try { next(); return 'unexpected'; } catch (error) { return String(error); } }", (*plan.function).clone().into()).unwrap();
    runtime.context.enter_realm(previous);
    assert!(
        result
            .as_string()
            .unwrap()
            .to_std_string_escaped()
            .contains("another author Document")
    );
    assert_eq!(plan.progress.cursor.get(), 1);
    assert_eq!(runtime.eval("deliveries").unwrap().as_number(), Some(0.0));
}
