//! Trusted copy, cut, and paste defaults driven by browser keyboard input.

use omoikane::{html::TreeBuilder, js::JsRuntime};
use std::sync::Mutex;

static CLIPBOARD_TEST_LOCK: Mutex<()> = Mutex::new(());

fn runtime(markup: &str) -> JsRuntime {
    JsRuntime::with_document_and_url(
        TreeBuilder::parse(markup).document(),
        "https://clipboard.example.test/",
    )
    .unwrap()
}

#[test]
fn copy_and_cut_use_the_focused_text_control_selection() {
    let _lock = CLIPBOARD_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut runtime = runtime("<!doctype html><input id='field' value='alpha beta'>");
    runtime
        .eval(
            r#"globalThis.result = 'pending';
      (async () => {
        const field = document.getElementById('field'), seen = [], cutEvents = [];
        field.focus(); field.setSelectionRange(0, 5);
        for (const type of ['copy', 'cut']) field.addEventListener(type, event => {
          seen.push([type, event.isTrusted, event.bubbles, event.cancelable,
            event.composed, event.clipboardData.getData('text/plain')].join(':'));
          globalThis.lastClipboardData = event.clipboardData;
        });
        __omoikane_dispatch_keyboard_input('keydown', {key:'c', ctrlKey:true});
        if (await navigator.clipboard.readText() !== 'alpha' || field.value !== 'alpha beta') {
          result = 'copy default'; return;
        }
        field.setSelectionRange(6, 10);
        field.addEventListener('beforeinput', event => cutEvents.push(event.type + ':' + event.inputType));
        field.addEventListener('input', event => cutEvents.push(event.type + ':' + event.inputType));
        __omoikane_dispatch_keyboard_input('keydown', {key:'x', ctrlKey:true});
        const text = await navigator.clipboard.readText();
        result = JSON.stringify({seen, cutEvents, value:field.value, caret:field.selectionStart, text,
          expired:lastClipboardData.getData('text/plain')});
      })().finally(() => navigator.clipboard.writeText(''));"#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
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
    runtime
        .eval(
            r#"globalThis.result = 'pending';
      (async () => {
        const field = document.getElementById('field');
        field.focus(); field.select();
        field.addEventListener('copy', event => {
          event.clipboardData.setData('text/plain', 'handler text');
          event.clipboardData.setData('text/html', '<b>handler</b>');
          event.preventDefault();
        }, {once:true});
        __omoikane_dispatch_keyboard_input('keydown', {key:'c', metaKey:true});
        const item = (await navigator.clipboard.read())[0];
        const plain = await (await item.getType('text/plain')).text();
        const html = await (await item.getType('text/html')).text();
        field.addEventListener('cut', event => event.preventDefault(), {once:true});
        __omoikane_dispatch_keyboard_input('keydown', {key:'x', ctrlKey:true});
        result = plain + '|' + html + '|' + field.value;
      })().finally(() => navigator.clipboard.writeText(''));"#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
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
    runtime
        .eval(
            r#"globalThis.result = 'pending';
      (async () => {
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
        __omoikane_dispatch_keyboard_input('keydown', {key:'v', ctrlKey:true});
        field.addEventListener('paste', event => event.preventDefault(), {once:true, capture:true});
        __omoikane_dispatch_keyboard_input('keydown', {key:'v', ctrlKey:true});
        result = JSON.stringify({value:field.value, caret:field.selectionStart, seen});
      })().finally(() => navigator.clipboard.writeText(''));"#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
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
    runtime
        .eval(
            r#"globalThis.result = 'pending';
      (async () => {
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
        __omoikane_dispatch_keyboard_input('keydown', {key:'c', ctrlKey:true});
        const copiedItem = (await navigator.clipboard.read())[0];
        const copied = await (await copiedItem.getType('text/plain')).text();
        const copiedHtml = await (await copiedItem.getType('text/html')).text();
        __omoikane_dispatch_keyboard_input('keydown', {key:'Backspace'});
        __omoikane_dispatch_keyboard_input('keydown', {key:'!', text:'!'});
        await navigator.clipboard.write([new ClipboardItem({
          'text/plain': new Blob(['there'], {type:'text/plain'}),
          'text/html': new Blob(['<b>there</b>'], {type:'text/html'}),
        })]);
        __omoikane_dispatch_keyboard_input('keydown', {key:'v', ctrlKey:true});
        result = JSON.stringify({copied, copiedHtml, text:host.textContent, events,
          collapsed:selection.isCollapsed, offset:selection.focusOffset});
      })().finally(() => navigator.clipboard.writeText(''));"#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
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
    let result = runtime
        .eval(
            r#"const host = document.getElementById('host');
      const first = host.firstChild.firstChild, second = host.lastChild.firstChild;
      const selection = getSelection();
      host.focus(); selection.collapse(second, 0);
      __omoikane_dispatch_keyboard_input('keydown', {key:'Backspace'});
      __omoikane_dispatch_keyboard_input('keydown', {key:'ArrowRight'});
      __omoikane_dispatch_keyboard_input('keydown', {key:'ArrowRight', shiftKey:true});
      __omoikane_dispatch_keyboard_input('keydown', {key:'X', text:'X'});
      document.designMode = 'on';
      const design = document.getElementById('design');
      const designEvents = [];
      document.addEventListener('beforeinput', event => designEvents.push(event.type + ':' + event.inputType + ':' + event.data));
      document.addEventListener('input', event => designEvents.push(event.type + ':' + event.inputType + ':' + event.data));
      design.focus(); selection.collapse(design.firstChild, 1);
      __omoikane_dispatch_keyboard_input('keydown', {key:'!', text:'!'});
      __omoikane_dispatch_keyboard_input('keydown', {key:'Enter'});
      JSON.stringify({host:host.textContent, design:design.textContent,
        designEvents, collapsed:selection.isCollapsed, offset:selection.focusOffset})"#,
        )
        .unwrap();
    assert_eq!(
        result.as_string().unwrap().to_std_string_escaped(),
        r#"{"host":"aXd","design":"x!\ny","designEvents":["beforeinput:insertText:!","input:insertText:!","beforeinput:insertParagraph:null","input:insertParagraph:null"],"collapsed":true,"offset":1}"#
    );
}
