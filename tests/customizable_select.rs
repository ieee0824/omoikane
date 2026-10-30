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
          const listBox = document.createElement('select');
          listBox.multiple = true;
          if (getComputedStyle(listBox, '::picker(select)').length !== 0) return 'list box has no picker';
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
          optgroup.disabled = false;
          select.disabled = true;
          if (!wrapped.matches(':disabled') || !optgroup.matches(':disabled')) return 'select disables options';
          if (!select.options[0].selected) return 'a disabled select keeps its selection';
          if (getComputedStyle(select.options[0]).display !== 'block') return 'option display';
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

#[test]
fn base_appearance_select_uses_html_ua_rules() {
    let mut runtime = runtime(
        "<style>select, ::picker(select) { appearance: base-select }</style>\
         <select id=s><button>b</button><option>one</option>\
         <optgroup><legend>l</legend><option>two</option></optgroup></select>\
         <select id=native><button>n</button><option>x</option></select>",
    );
    check(
        &mut runtime,
        r#"() => {
          const select = document.getElementById('s');
          const style = getComputedStyle(select);
          const expect = (actual, expected, label) =>
            actual === expected ? null : `${label}: ${actual} !== ${expected}`;
          const option = select.querySelector('option');
          const failures = [
            expect(style.appearance, 'base-select', 'appearance'),
            expect(style.display, 'inline-flex', 'select display'),
            expect(style.backgroundColor, 'rgba(0, 0, 0, 0)', 'select background'),
            expect(style.borderTopLeftRadius, '8px', 'select radius'),
            expect(style.paddingLeft, '8px', 'select padding'),
            expect(getComputedStyle(select, '::picker(select)').boxSizing, 'border-box', 'picker'),
            expect(getComputedStyle(select, '::picker-icon').display, 'block', 'picker-icon'),
            expect(getComputedStyle(option).display, 'flex', 'option display'),
            getComputedStyle(option, '::checkmark').visibility === 'hidden' ? 'checked mark hidden' : null,
            expect(getComputedStyle(option, '::checkmark').content, '"\u2713" / ""', 'checkmark content'),
            expect(getComputedStyle(select.options[1], '::checkmark').visibility, 'hidden', 'unchecked mark'),
            expect(getComputedStyle(select.querySelector('optgroup')).fontWeight, '700', 'optgroup'),
            expect(getComputedStyle(select.options[1]).fontWeight, '400', 'optgroup option'),
            expect(getComputedStyle(select.querySelector('legend')).display, 'block', 'legend'),
            expect(getComputedStyle(select.querySelector('button')).display, 'contents', 'button'),
            expect(CSS.supports('selector(::picker(select))'), true, 'picker selector'),
            expect(CSS.supports('selector(::picker(input))'), false, 'picker argument'),
            expect(CSS.supports('selector(::picker(select):hover)'), true, 'user action after picker'),
            expect(CSS.supports('selector(::picker(select):popover-open)'), false, 'state after picker'),
            expect(getComputedStyle(document.getElementById('native'), '::picker-icon').display, 'block', 'icon of a drop-down'),
          ].filter(Boolean);
          select.disabled = true;
          if (getComputedStyle(select).cursor !== 'not-allowed') failures.push('disabled cursor');
          const native = document.getElementById('native');
          native.style.appearance = 'auto';
          if (getComputedStyle(native).display !== 'inline-block') failures.push('native display');
          if (getComputedStyle(native.querySelector('button')).display === 'contents') failures.push('native button');
          return failures.length ? failures.join('; ') : true;
        }"#,
    );
}

#[test]
fn base_picker_opens_renders_options_and_handles_keys() {
    let mut runtime = runtime(
        "<style>select, ::picker(select) { appearance: base-select }</style>\
         <select id=s><button id=b><span>pick</span></button>\
         <option>one</option><div><option>two</option></div><option disabled>three</option></select>\
         <p id=outside>outside</p>",
    );
    runtime.set_viewport(400.0, 300.0);
    check(
        &mut runtime,
        r#"() => {
          const select = document.getElementById('s');
          const button = document.getElementById('b');
          const [one, two] = select.options;
          const events = [];
          select.addEventListener('change', () => events.push('change'));
          const click = node => __omoikane_dispatch_mouse_input(node.__id, 'click', {button: 0}, false);
          const key = k => __omoikane_dispatch_keyboard_input('keydown', {key: k});
          button.focus();
          if (document.activeElement === button) return 'select button is not focusable';
          if (one.getBoundingClientRect().height !== 0) return 'closed picker hides options';
          click(button.firstChild);
          if (!select.matches(':open')) return 'clicking the button opens the picker';
          if (document.activeElement !== one) return 'selected option focused: ' + document.activeElement;
          const selectRect = select.getBoundingClientRect();
          const rect = two.getBoundingClientRect();
          if (!(rect.height > 0 && rect.top >= selectRect.bottom)) return 'picker below select';
          if (document.elementFromPoint(rect.left + 2, rect.top + 2) !== two) return 'option hit testing';
          key('ArrowDown');
          if (document.activeElement !== two || select.value !== 'one') return 'arrow moves focus';
          key('ArrowDown');
          if (document.activeElement !== two) return 'disabled options are skipped';
          key('Enter');
          if (select.matches(':open') || select.value !== 'two') return 'enter picks';
          if (document.activeElement !== select) return 'focus returns to the select';
          if (events.join() !== 'change') return 'events ' + events;
          key(' ');
          if (!select.matches(':open') || document.activeElement !== two) return 'space opens';
          key('Escape');
          if (select.matches(':open') || document.activeElement !== select) return 'escape closes';
          click(select);
          click(one);
          if (select.matches(':open') || select.value !== 'one') return 'clicking an option picks it';
          click(select);
          click(select);
          if (select.matches(':open')) return 'clicking the select again closes it';
          click(select);
          click(document.getElementById('outside'));
          if (select.matches(':open')) return 'light dismiss';
          return true;
        }"#,
    );
}
