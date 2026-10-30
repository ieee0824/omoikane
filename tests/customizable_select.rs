//! Customizable select: option lists, selectedness and `selectedcontent`
//! (Issue #1573).

use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;

fn runtime(body: &str) -> JsRuntime {
    let markup = format!("<!doctype html><html><body>{body}</body></html>");
    JsRuntime::with_document_and_url(
        TreeBuilder::parse(&markup).document(),
        "https://example.com/",
    )
    .unwrap()
}

/// Evaluates `script`, which returns `true` or a failure description.
fn check(runtime: &mut JsRuntime, script: &str) {
    let result = runtime.eval(&format!("({script})()")).unwrap();
    assert!(result.as_boolean() == Some(true), "{}", result.display());
}

#[test]
fn wrapped_options_belong_to_the_select() {
    let mut runtime = runtime(
        "<select id=s><div><option>one</option></div><optgroup><div><option disabled>two</option></div></optgroup>\
         <hr><option>three</option><datalist><option>hidden</option></datalist></select>",
    );
    check(
        &mut runtime,
        r#"() => {
          const select = document.getElementById('s');
          const texts = Array.from(select.options, option => option.text).join();
          if (texts !== 'one,two,three') return 'options ' + texts;
          if (select.value !== 'one') return 'value ' + select.value;
          const hidden = select.querySelector('datalist option');
          if (hidden.form !== null || hidden.index !== 0) return 'datalist option';
          if (select.options[1].index !== 1) return 'index';
          const optgroup = select.querySelector('optgroup');
          const wrapped = optgroup.querySelector('option');
          wrapped.removeAttribute('disabled');
          if (wrapped.matches(':disabled')) return 'enabled option';
          optgroup.disabled = true;
          if (!wrapped.matches(':disabled')) return 'optgroup disables wrapped option';
          return true;
        }"#,
    );
}

#[test]
fn selectedness_setting_runs_on_insertion_removal_and_setters() {
    let mut runtime = runtime("<select id=s></select>");
    check(
        &mut runtime,
        r#"() => {
          const select = document.getElementById('s');
          const a = new Option('a'), b = new Option('b');
          select.append(a, b);
          if (!a.selected || b.selected) return 'first option selected on insertion';
          b.selected = true;
          if (a.selected || select.selectedIndex !== 1) return 'selecting deselects others';
          b.selected = false;
          if (!a.selected) return 'deselecting resets to the first enabled option';
          select.selectedIndex = -1;
          if (select.selectedOptions.length !== 0 || select.value !== '') return 'selectedIndex -1';
          a.remove();
          if (!b.selected) return 'removal resets the remaining options';
          b.setAttribute('selected', '');
          select.multiple = true;
          if (select.selectedOptions.length !== 1) return 'multiple keeps selection';
          return true;
        }"#,
    );
}

#[test]
fn selectedcontent_mirrors_the_selected_option() {
    let mut runtime = runtime(
        "<select id=s><button><selectedcontent></selectedcontent></button>\
         <option><b>one</b></option><option>two</option></select>",
    );
    check(
        &mut runtime,
        r#"() => {
          const select = document.getElementById('s');
          const content = select.querySelector('selectedcontent');
          if (!(content instanceof HTMLSelectedContentElement)) return 'interface';
          if (content.innerHTML !== '<b>one</b>') return 'parsed ' + content.innerHTML;
          select.options[1].selected = true;
          if (content.innerHTML !== 'two') return 'option.selected ' + content.innerHTML;
          select.value = 'one';
          if (content.innerHTML !== '<b>one</b>') return 'value ' + content.innerHTML;
          select.options[0].firstChild.textContent = 'edited';
          if (content.innerHTML !== '<b>one</b>') return 'option edits are not mirrored';
          return true;
        }"#,
    );
    check(
        &mut runtime,
        r#"() => {
          const select = document.getElementById('s');
          globalThis.content = select.querySelector('selectedcontent');
          select.options[0].remove();
          globalThis.beforeMicrotask = content.innerHTML;
          return true;
        }"#,
    );
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        r#"() => beforeMicrotask === '<b>one</b>' && content.innerHTML === 'two' ||
          'removal ' + beforeMicrotask + ' -> ' + content.innerHTML"#,
    );
}

#[test]
fn select_parsing_allows_arbitrary_content() {
    let mut runtime = runtime(
        "<select id=a><button>pick</button><div><option>x</option></div><select id=b></select>\
         <p id=after></p><select id=c><option>y</option><input id=i></select>",
    );
    check(
        &mut runtime,
        r#"() => {
          const a = document.getElementById('a');
          if (!a.querySelector('button') || a.options.length !== 1) return 'content kept';
          if (document.getElementById('b')) return 'nested select is ignored';
          if (document.getElementById('after').parentNode !== document.body) return 'select closed';
          if (document.getElementById('i').parentNode !== document.body) return 'input closes select';
          const fragment = document.createElement('select');
          fragment.innerHTML = '<button><selectedcontent></selectedcontent></button><option>z</option>';
          return fragment.querySelector('selectedcontent') !== null || 'fragment keeps button';
        }"#,
    );
}
