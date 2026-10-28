//! Basic legacy editing commands share the browser's native editing core.

use omoikane::{html::TreeBuilder, js::JsRuntime};

fn runtime(markup: &str) -> JsRuntime {
    JsRuntime::with_document_and_url(
        TreeBuilder::parse(markup).document(),
        "https://editing.example.test/",
    )
    .unwrap()
}

#[test]
fn command_queries_are_case_insensitive_and_validate_the_document_receiver() {
    let mut runtime = runtime("<!doctype html><div contenteditable id='host'>text</div>");
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
          const names = ['execCommand','queryCommandSupported','queryCommandEnabled',
            'queryCommandState','queryCommandValue'];
          if (!names.every(name => typeof document[name] === 'function')) return false;
          if (!document.queryCommandSupported('InSeRtTeXt') ||
              !document.queryCommandSupported('delete') ||
              !document.queryCommandSupported('bold') ||
              document.queryCommandSupported('not-a-command')) return false;
          const getter = Document.prototype.queryCommandSupported;
          try { getter.call({}, 'bold'); return false; }
          catch (error) { if (!(error instanceof TypeError)) return false; }
          return document.queryCommandValue('not-a-command') === '' &&
            document.queryCommandState('not-a-command') === false;
        })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn insert_text_and_delete_commands_edit_focused_text_controls() {
    let mut runtime = runtime("<!doctype html><input id='field' value='abcd'>");
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
          const field = document.getElementById('field'), events = [];
          field.focus(); field.setSelectionRange(1, 3);
          field.addEventListener('beforeinput', event => events.push('unexpected-beforeinput'));
          field.addEventListener('input', event => events.push(event.type + ':' + event.inputType + ':' + event.data + ':' + event.isTrusted));
          if (!document.queryCommandEnabled('insertText') || document.queryCommandEnabled('bold')) return false;
          if (!document.execCommand('insertText', false, 'XY') || field.value !== 'aXYd') return false;
          if (!document.execCommand('delete') || field.value !== 'aXd') return false;
          return events.join('|') ===
            'input:insertText:XY:true|input:deleteContentBackward:null:true';
        })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn contenteditable_insert_delete_and_bold_use_the_active_range() {
    let mut runtime = runtime("<!doctype html><div contenteditable id='host'>hello world</div>");
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
          const host = document.getElementById('host'), text = host.firstChild;
          const selection = getSelection(), events = [];
          host.focus(); selection.setBaseAndExtent(text, 6, text, 11);
          host.addEventListener('beforeinput', event => events.push('unexpected-beforeinput'));
          host.addEventListener('input', event => events.push(event.inputType + ':' + event.getTargetRanges().length + ':' + event.isTrusted));
          if (!document.queryCommandEnabled('bold') || document.queryCommandState('bold')) return false;
          if (!document.execCommand('bold') || host.innerHTML !== 'hello <b>world</b>' ||
              !document.queryCommandState('bold') || document.queryCommandValue('bold') !== 'true') return false;
          if (!document.execCommand('insertText', false, '!') || host.textContent !== 'hello !') return false;
          if (!document.execCommand('delete') || host.textContent !== 'hello ') return false;
          return events.join('|') === 'formatBold:0:true|insertText:0:true|' +
            'deleteContentBackward:0:true';
        })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn recursive_exec_command_is_rejected_without_a_second_edit() {
    let mut runtime = runtime("<!doctype html><div contenteditable id='host'>a</div>");
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
          const host = document.getElementById('host'), selection = getSelection();
          host.focus(); selection.collapse(host.firstChild, 1);
          let recursive;
          host.addEventListener('input', () => {
            recursive = document.execCommand('insertText', false, 'nested');
          }, {once:true});
          const outer = document.execCommand('insertText', false, 'x');
          return outer && recursive === false && host.textContent === 'ax';
        })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn cut_command_dispatches_trusted_clipboard_and_input_without_beforeinput() {
    let mut runtime = runtime("<!doctype html><textarea id='field'>cut me</textarea>");
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
          const field = document.getElementById('field'), events = [];
          field.focus(); field.select();
          field.addEventListener('cut', event => events.push('cut:' + event.isTrusted + ':' + event.composed));
          field.addEventListener('beforeinput', () => events.push('unexpected-beforeinput'));
          field.addEventListener('input', event => events.push('input:' + event.inputType + ':' + event.isTrusted));
          return document.execCommand('cut') && field.value === '' &&
            events.join('|') === 'cut:true:true|input:deleteByCut:true';
        })()"#,
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}
