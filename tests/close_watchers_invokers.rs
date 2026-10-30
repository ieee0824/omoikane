//! Close watchers, dialog `closedby`, Invoker Commands and
//! `HTMLInputElement.showPicker()` (Issue #1297).

use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;

const HELPERS: &str = r#"
  globalThis.activate = () =>
    __omoikane_dispatch_keyboard_input('keydown', { key: 'Control', code: 'ControlLeft' });
  globalThis.pressEscape = () =>
    __omoikane_dispatch_keyboard_input('keydown', { key: 'Escape', code: 'Escape' });
  globalThis.press = (element, init = {}) => {
    __omoikane_dispatch_mouse_input(element.__id, 'mousedown', init, false);
    __omoikane_dispatch_mouse_input(element.__id, 'mouseup', init, false);
  };
"#;

fn runtime(body: &str) -> JsRuntime {
    let markup = format!("<!doctype html><html><body>{body}</body></html>");
    let mut runtime = JsRuntime::with_document_and_url(
        TreeBuilder::parse(&markup).document(),
        "https://example.com/",
    )
    .unwrap();
    runtime.eval(HELPERS).unwrap();
    runtime
}

/// Evaluates `script`, which returns `true` or a failure description.
fn check(runtime: &mut JsRuntime, script: &str) {
    let result = runtime
        .eval(&format!(
            "(() => {{ try {{ return ({script})(); }} catch (error) {{ return String(error) + error.stack; }} }})()"
        ))
        .unwrap();
    if result.as_boolean() != Some(true) {
        panic!("{}", result.display());
    }
}

#[test]
fn close_watcher_request_close_close_and_destroy() {
    check(
        &mut runtime(""),
        r#"() => {
          const log = [];
          const watcher = new CloseWatcher();
          watcher.oncancel = event => { log.push('cancel:' + event.cancelable); event.preventDefault(); };
          watcher.addEventListener('close', () => log.push('close'));
          watcher.requestClose();
          if (log.join() !== 'cancel:true') return 'prevented ' + log.join();
          watcher.oncancel = null;
          watcher.requestClose();
          watcher.requestClose();
          if (log.join() !== 'cancel:true,close') return 'closed ' + log.join();

          const direct = new CloseWatcher();
          direct.oncancel = () => log.push('direct-cancel');
          direct.onclose = () => log.push('direct-close');
          direct.close();
          const destroyed = new CloseWatcher();
          destroyed.onclose = () => log.push('destroyed-close');
          destroyed.destroy();
          destroyed.requestClose();
          destroyed.close();
          const controller = new AbortController();
          const aborted = new CloseWatcher({ signal: controller.signal });
          aborted.onclose = () => log.push('aborted-close');
          controller.abort();
          aborted.close();
          if (log.join() !== 'cancel:true,close,direct-close') return 'lifecycle ' + log.join();
          try { CloseWatcher.prototype.requestClose.call({}); return 'brand'; }
          catch (error) { return error instanceof TypeError || String(error); }
        }"#,
    );
}

#[test]
fn escape_closes_one_group_and_cancel_needs_history_action_activation() {
    check(
        &mut runtime(""),
        r#"() => {
          const log = [];
          const watch = name => {
            const watcher = new CloseWatcher();
            watcher.oncancel = event => { log.push(name + '-cancel:' + event.cancelable); event.preventDefault(); };
            watcher.onclose = () => log.push(name + '-close');
            return watcher;
          };
          // Without user activation between them, both watchers share a group,
          // and the cancel event cannot prevent the close.
          watch('a');
          watch('b');
          pressEscape();
          if (log.join() !== 'b-cancel:false,b-close,a-cancel:false,a-close') return 'grouped ' + log.join();
          log.length = 0;

          watch('c');
          activate();
          watch('d');
          activate();
          pressEscape();
          if (log.join() !== 'd-cancel:true') return 'prevented ' + log.join();
          // The prevented request consumed the history-action activation, and
          // each close request only reaches the newest group.
          pressEscape();
          if (log.join() !== 'd-cancel:true,d-cancel:false,d-close') return 'consumed ' + log.join();
          pressEscape();
          return log.join() === 'd-cancel:true,d-cancel:false,d-close,c-cancel:false,c-close' ||
            'separate ' + log.join();
        }"#,
    );
}

#[test]
fn dialog_closed_by_reflects_computed_state_and_controls_escape() {
    check(
        &mut runtime(
            r#"<dialog id="plain"></dialog><dialog id="request" closedby="CloseRequest"></dialog>
               <dialog id="none" closedby="none"></dialog><dialog id="bogus" closedby="bogus"></dialog>"#,
        ),
        r#"() => {
          const [plain, request, none, bogus] = ['plain', 'request', 'none', 'bogus'].map(id => document.getElementById(id));
          if (plain.closedBy !== 'none' || request.closedBy !== 'closerequest' || bogus.closedBy !== 'none') {
            return 'closed state ' + [plain.closedBy, request.closedBy, bogus.closedBy];
          }
          plain.showModal();
          if (plain.closedBy !== 'closerequest' || bogus.closedBy !== 'none') return 'modal ' + plain.closedBy;
          pressEscape();
          if (plain.open) return 'modal dialog stayed open';

          plain.show();
          pressEscape();
          if (!plain.open) return 'non-modal dialog closed by Escape';
          plain.close();

          request.show();
          activate();
          pressEscape();
          if (request.open) return 'closedby=closerequest stayed open';

          none.showModal();
          activate();
          pressEscape();
          if (!none.open) return 'closedby=none closed';
          none.closedBy = 'any';
          return (none.getAttribute('closedby') === 'any' && none.closedBy === 'any') || 'setter';
        }"#,
    );
}

#[test]
fn dialog_request_close_uses_the_close_watcher_and_return_value() {
    let mut runtime = runtime(r#"<dialog id="dialog" closedby="none"></dialog>"#);
    check(
        &mut runtime,
        r#"() => {
          const dialog = document.getElementById('dialog');
          globalThis.log = [];
          globalThis.prevent = true;
          dialog.addEventListener('cancel', event => {
            log.push('cancel:' + event.cancelable);
            if (prevent) event.preventDefault();
          });
          dialog.addEventListener('close', () => log.push('close:' + dialog.returnValue));
          dialog.showModal();
          dialog.requestClose('kept');
          if (!dialog.open || log.join() !== 'cancel:true') return 'prevented ' + log.join();
          prevent = false;
          dialog.requestClose('done');
          // The close event is queued as a task.
          return (!dialog.open && dialog.returnValue === 'done' && log.join() === 'cancel:true,cancel:true') ||
            'closed ' + log.join();
        }"#,
    );
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        r#"() => {
          if (log.join() !== 'cancel:true,cancel:true,close:done') return 'close event ' + log.join();
          // A later Escape-driven close leaves the previous return value.
          const dialog = document.getElementById('dialog');
          dialog.closedBy = 'closerequest';
          dialog.show();
          pressEscape();
          return (!dialog.open && dialog.returnValue === 'done') || 'escape ' + dialog.returnValue;
        }"#,
    );
}

#[test]
fn request_close_ignores_disconnected_dialogs_and_handles_open_attribute_dialogs() {
    let mut runtime = runtime(r#"<dialog id="markup" open closedby="closerequest"></dialog>"#);
    check(
        &mut runtime,
        r#"() => {
          const dialog = document.createElement('dialog');
          document.body.append(dialog);
          dialog.showModal();
          dialog.remove();
          dialog.requestClose();
          if (!dialog.open) return 'disconnected dialog closed';
          const markup = document.getElementById('markup');
          pressEscape();
          return !markup.open || 'open-attribute dialog ignored Escape';
        }"#,
    );
}

#[test]
fn closedby_any_light_dismisses_on_a_press_outside_the_topmost_dialog() {
    check(
        &mut runtime(
            r#"<p id="outside">outside</p><dialog id="dialog" closedby="any"><span id="inside">inside</span></dialog>"#,
        ),
        r#"() => {
          const dialog = document.getElementById('dialog');
          dialog.show();
          press(document.getElementById('inside'));
          if (!dialog.open) return 'inside press closed';
          press(document.getElementById('outside'));
          if (dialog.open) return 'outside press kept it open';
          dialog.closedBy = 'closerequest';
          dialog.show();
          press(document.getElementById('outside'));
          return dialog.open || 'closedby=closerequest light dismissed';
        }"#,
    );
}

#[test]
fn invoker_commands_drive_dialogs_and_popovers() {
    check(
        &mut runtime(
            r#"<dialog id="dialog"></dialog><div id="pop" popover></div>
               <button id="show" commandfor="dialog" command="Show-Modal"></button>
               <button id="close" commandfor="dialog" command="close" value="by-button"></button>
               <button id="request" commandfor="dialog" command="request-close" value="requested"></button>
               <button id="toggle" commandfor="pop" command="toggle-popover"></button>
               <button id="custom" commandfor="pop" command="--refresh"></button>
               <button id="unknown" commandfor="dialog" command="explode"></button>
               <form><button id="submit" type="submit" commandfor="dialog" command="show-modal"></button></form>"#,
        ),
        r#"() => {
          const $ = id => document.getElementById(id);
          const dialog = $('dialog'), pop = $('pop');
          if ($('show').command !== 'show-modal' || $('custom').command !== '--refresh' ||
              $('unknown').command !== '' || $('show').commandForElement !== dialog) return 'reflection';
          if ($('show').type !== 'button') return 'type ' + $('show').type;
          const events = [];
          for (const target of [dialog, pop]) {
            target.addEventListener('command', event => {
              events.push(event.command + '@' + event.target.id + ':' + (event.source === document.getElementById(event.source.id)) +
                ':' + (event instanceof CommandEvent) + ':' + event.cancelable);
            });
          }
          $('show').click();
          if (!dialog.open || !dialog.matches(':modal')) return 'show-modal';
          $('close').click();
          if (dialog.open || dialog.returnValue !== 'by-button') return 'close ' + dialog.returnValue;
          dialog.showModal();
          let cancelled = false;
          dialog.addEventListener('cancel', () => { cancelled = true; }, { once: true });
          $('request').click();
          if (dialog.open || !cancelled || dialog.returnValue !== 'requested') return 'request-close';
          $('toggle').click();
          if (!pop.matches(':popover-open')) return 'toggle open';
          $('toggle').click();
          if (pop.matches(':popover-open')) return 'toggle close';
          $('custom').click();
          if (pop.matches(':popover-open')) return 'custom command acted';
          pop.addEventListener('command', event => event.preventDefault(), { once: true });
          $('toggle').click();
          if (pop.matches(':popover-open')) return 'canceled command acted';
          $('unknown').click();
          const expected = ['show-modal@dialog', 'close@dialog', 'request-close@dialog', 'toggle-popover@pop',
            'toggle-popover@pop', '--refresh@pop', 'toggle-popover@pop'].map(name => name + ':true:true:true');
          if (events.join() !== expected.join()) return 'events ' + events.join();
          // A submit button with a form owner submits instead of running its command.
          const form = $('submit').form;
          form.addEventListener('submit', event => event.preventDefault());
          $('submit').click();
          if (dialog.open) return 'submit button ran its command';
          $('toggle').commandForElement = dialog;
          return ($('toggle').getAttribute('commandfor') === '' && $('toggle').commandForElement === dialog) ||
            'commandForElement setter';
        }"#,
    );
}

#[test]
fn command_event_constructor_validates_source() {
    check(
        &mut runtime(""),
        r#"() => {
          const event = new CommandEvent('command', { command: '--x', source: document.body, cancelable: true });
          if (event.command !== '--x' || event.source !== document.body || !event.cancelable) return 'init';
          document.body.addEventListener('command', e => { globalThis.seen = e.source; }, { once: true });
          document.body.dispatchEvent(event);
          if (globalThis.seen !== document.body) return 'source';
          if (new CommandEvent('command').command !== '') return 'default command';
          try { new CommandEvent('command', { source: {} }); return 'accepted non-element'; }
          catch (error) { return error instanceof TypeError || String(error); }
        }"#,
    );
}

#[test]
fn input_show_picker_checks_mutability_and_consumes_activation() {
    check(
        &mut runtime(
            r#"<input id="date" type="date"><input id="disabled" type="date" disabled>
               <input id="readonly" type="text" readonly><input id="color" type="color" readonly>"#,
        ),
        r#"() => {
          const $ = id => document.getElementById(id);
          const error = call => { try { call(); return 'none'; } catch (e) { return e.name; } };
          if (error(() => $('date').showPicker()) !== 'NotAllowedError') return 'activation required';
          activate();
          if (error(() => $('disabled').showPicker()) !== 'InvalidStateError') return 'disabled';
          if (error(() => $('readonly').showPicker()) !== 'InvalidStateError') return 'readonly';
          if (error(() => $('date').showPicker()) !== 'none') return 'date';
          if (error(() => $('date').showPicker()) !== 'NotAllowedError') return 'activation not consumed';
          activate();
          // readonly does not apply to color inputs.
          return error(() => $('color').showPicker()) === 'none' || 'color';
        }"#,
    );
}

#[test]
fn modal_dialog_is_centered_and_hit_testable_while_the_document_is_inert() {
    let mut runtime = runtime(
        r#"<button id="outside">outside</button><dialog id="dialog"><button id="inside">inside</button></dialog>"#,
    );
    check(
        &mut runtime,
        r#"() => {
          const dialog = document.getElementById('dialog');
          const inside = document.getElementById('inside');
          const outside = document.getElementById('outside');
          const outsideRect = outside.getBoundingClientRect();
          dialog.showModal();
          const style = getComputedStyle(dialog);
          if (style.position !== 'fixed') return 'position ' + style.position;
          const rect = dialog.getBoundingClientRect();
          const centerX = rect.x + rect.width / 2, centerY = rect.y + rect.height / 2;
          if (Math.abs(centerX - innerWidth / 2) > 1 || Math.abs(centerY - innerHeight / 2) > 1) {
            return 'not centered ' + [rect.x, rect.y, rect.width, rect.height];
          }
          const insideRect = inside.getBoundingClientRect();
          const hit = document.elementFromPoint(
            insideRect.x + insideRect.width / 2, insideRect.y + insideRect.height / 2);
          if (hit !== inside) return 'inside hit ' + (hit && (hit.id || hit.tagName));
          // Content outside the modal dialog stays inert.
          const blocked = document.elementFromPoint(outsideRect.x + 2, outsideRect.y + 2);
          return blocked !== outside || 'inert content was hit';
        }"#,
    );
}

#[test]
fn focusing_an_iframe_routes_escape_to_its_document_close_watchers() {
    let mut runtime = runtime(
        r#"<iframe id="frame" srcdoc="<dialog id=d></dialog><script>d.showModal();d.addEventListener('cancel',e=>{parent.log.push('cancel:'+e.cancelable);e.preventDefault()});d.addEventListener('close',()=>parent.log.push('close'))</script>"></iframe>"#,
    );
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        r#"() => {
          globalThis.log = [];
          const frame = document.getElementById('frame');
          const childDocument = frame.contentDocument;
          if (!childDocument.getElementById('d').open) return 'child dialog not open';
          frame.focus();
          if (document.activeElement !== frame || !childDocument.hasFocus()) return 'iframe viewport not focused';
          // Without user activation the cancel event cannot block the close request.
          pressEscape();
          return log.join() === 'cancel:false' || 'escape ' + log.join();
        }"#,
    );
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        r#"() => !document.getElementById('frame').contentDocument.getElementById('d').open &&
          log.join() === 'cancel:false,close' || 'close ' + log.join()"#,
    );
}

#[test]
fn show_modal_keeps_its_popover_ancestors_open() {
    let mut runtime = runtime(
        r#"<div id="outer" popover>outer<dialog id="dialog">dialog</dialog></div><div id="other" popover>other</div>"#,
    );
    check(
        &mut runtime,
        r#"() => {
          const outer = document.getElementById('outer');
          const dialog = document.getElementById('dialog');
          outer.showPopover();
          dialog.showModal();
          if (!outer.matches(':popover-open')) return 'ancestor popover was hidden';
          dialog.close();
          document.getElementById('other').showPopover();
          if (outer.matches(':popover-open')) return 'unrelated popover did not replace outer';
          const other = document.getElementById('other');
          const unrelated = document.createElement('dialog');
          document.body.append(unrelated);
          unrelated.showModal();
          return !other.matches(':popover-open') || 'unrelated popover stayed open';
        }"#,
    );
}

#[test]
fn open_dialogs_use_the_html_rendering_defaults() {
    let mut runtime = runtime(
        r#"<style>body{margin:0}</style><dialog id="dialog"><div style="width:40px;height:20px"></div></dialog>"#,
    );
    check(
        &mut runtime,
        r#"() => {
          const dialog = document.getElementById('dialog');
          dialog.show();
          let style = getComputedStyle(dialog);
          const expected = [['position', 'absolute'], ['padding-top', '16px'],
            ['border-top-style', 'solid'], ['border-top-width', '3px']];
          for (const [name, value] of expected) {
            if (style.getPropertyValue(name) !== value) return name + ' ' + style.getPropertyValue(name);
          }
          let rect = dialog.getBoundingClientRect();
          // 40px content + 2 * (16px padding + 3px border), centered inline.
          if (rect.width !== 78 || rect.x !== (innerWidth - 78) / 2 || rect.y !== 0) {
            return 'modeless ' + [rect.x, rect.y, rect.width, rect.height];
          }
          dialog.close();
          dialog.showModal();
          style = getComputedStyle(dialog);
          if (style.position !== 'fixed' || style.overflowY !== 'auto') return 'modal ' + style.position;
          rect = dialog.getBoundingClientRect();
          return (rect.x === (innerWidth - 78) / 2 && rect.y === (innerHeight - rect.height) / 2) ||
            'modal ' + [rect.x, rect.y, rect.width, rect.height];
        }"#,
    );
}

#[test]
fn user_activation_reaches_same_origin_iframe_close_watchers() {
    let mut runtime = runtime(
        r#"<iframe id="frame" srcdoc="<dialog id=d></dialog><script>d.showModal();d.addEventListener('cancel',e=>{parent.log.push('cancel:'+e.cancelable);e.preventDefault()})</script>"></iframe>"#,
    );
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        r#"() => {
          globalThis.log = [];
          const frame = document.getElementById('frame');
          const dialog = frame.contentDocument.getElementById('d');
          // Activation in the parent also activates the same-origin child.
          activate();
          if (!frame.contentWindow.navigator.userActivation.hasBeenActive) return 'child not activated';
          frame.focus();
          pressEscape();
          if (!dialog.open || log.join() !== 'cancel:true') return 'first escape ' + log.join();
          // The consumed activation no longer lets cancel block the request.
          pressEscape();
          return log.join() === 'cancel:true,cancel:false' || 'second escape ' + log.join();
        }"#,
    );
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        r#"() => !document.getElementById('frame').contentDocument.getElementById('d').open ||
          'dialog stayed open'"#,
    );
}

#[test]
fn preventing_a_close_consumes_history_action_activation_across_frames() {
    let frame = r#"<dialog id=d></dialog><script>d.showModal();d.addEventListener('cancel',e=>{parent.log.push(frameElement.id+':'+e.cancelable);e.preventDefault()})</script>"#;
    let mut runtime = runtime(&format!(
        r#"<button id="top">top</button><iframe id="one" srcdoc="{frame}"></iframe><iframe id="two" srcdoc="{frame}"></iframe>"#
    ));
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        r#"() => {
          globalThis.log = [];
          // The child dialogs took focus; activate from the top document so
          // the notification reaches both same-origin descendants.
          document.getElementById('top').focus();
          activate();
          const ua = id => document.getElementById(id).contentWindow.navigator.userActivation.hasBeenActive;
          if (!ua('one') || !ua('two')) return 'activation ' + ua('one') + ua('two');
          document.getElementById('one').focus();
          pressEscape();
          // The prevented close in one frame consumed the activation in both.
          document.getElementById('two').focus();
          pressEscape();
          return log.join() === 'one:true,two:false' || log.join();
        }"#,
    );
}

#[test]
fn modal_dialog_backdrop_targets_the_dialog_for_light_dismiss() {
    let mut runtime = runtime(
        r#"<div id="outer" popover>outer<dialog id="dialog" closedby="any">dialog</dialog></div>"#,
    );
    check(
        &mut runtime,
        r#"() => {
          const outer = document.getElementById('outer');
          const dialog = document.getElementById('dialog');
          outer.showPopover();
          dialog.showModal();
          // A viewport corner, away from the centered dialog, is beneath its backdrop.
          const x = 5, y = 5;
          if (document.elementFromPoint(x, y) !== dialog) return 'backdrop hit ' + document.elementFromPoint(x, y);
          for (const type of ['mousedown', 'mouseup', 'click']) {
            __omoikane_dispatch_mouse_input(dialog.__id, type, { clientX: x, clientY: y, button: 0 }, type === 'mousedown');
          }
          // Clicking the backdrop light dismisses the dialog but not its popover ancestor.
          return (!dialog.open && outer.matches(':popover-open')) ||
            'after click ' + dialog.open + ' ' + outer.matches(':popover-open');
        }"#,
    );
}
