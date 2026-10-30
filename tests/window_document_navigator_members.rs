//! Window, Document, Navigator, Location and Performance members describing
//! the browsing context (Issue #1291).

use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;

fn runtime_at(markup: &str, url: &str) -> JsRuntime {
    JsRuntime::with_document_and_url(TreeBuilder::parse(markup).document(), url).unwrap()
}

fn runtime() -> JsRuntime {
    runtime_at(
        "<!doctype html><html><head><script>1</script></head><body></body></html>",
        "https://sub.example.com/page",
    )
}

/// Evaluates `script`, which returns `true` or a failure description.
fn check(runtime: &mut JsRuntime, script: &str) {
    let result = runtime
        .eval(&format!(
            "(() => {{ try {{ return ({script})(); }} catch (error) {{ return String(error); }} }})()"
        ))
        .unwrap();
    if result.as_boolean() != Some(true) {
        panic!("{}", result.display());
    }
}

#[test]
fn window_exposes_barprops_external_origin_and_ignored_geometry_operations() {
    check(
        &mut runtime(),
        r#"() => {
          for (const name of ['locationbar', 'menubar', 'personalbar', 'scrollbars', 'statusbar', 'toolbar']) {
            const bar = window[name];
            if (!(bar instanceof BarProp) || bar.visible !== true || bar !== window[name]) return name;
          }
          try { new BarProp(); return 'BarProp constructible'; } catch (error) {
            if (!(error instanceof TypeError)) return 'BarProp error';
          }
          if (!(external instanceof Object) || external.AddSearchProvider() !== undefined ||
              external.IsSearchProviderInstalled() !== undefined) return 'external';
          if (origin !== 'https://sub.example.com' || origin !== location.origin) return 'origin ' + origin;
          window.origin = 'replaced';
          if (origin !== 'replaced') return 'origin is not replaceable';
          if (window.closed !== false || window.close() !== undefined || window.closed !== false) return 'close';
          if (window.stop() !== undefined) return 'stop';
          if (window.opener !== null || window.length !== 0) return 'opener/length';
          for (const name of ['close', 'stop', 'print', 'moveTo', 'captureEvents', 'releaseEvents', 'closed', 'external']) {
            // Window is a [Global] interface, so its members are own properties.
            if (!Object.prototype.hasOwnProperty.call(window, name)) return 'not own ' + name;
          }
          if (captureEvents() !== undefined || releaseEvents() !== undefined) return 'legacy events';
          if (originAgentCluster !== true || clientInformation !== navigator) return 'agent cluster/clientInformation';
          if (status !== '') return 'status';
          status = 12;
          if (status !== '12') return 'status setter';
          if (String(locationbar) !== '[object BarProp]' || String(external) !== '[object External]') return 'toString';
          if (BarProp.length !== 0 || Object.keys(window).includes('BarProp')) return 'interface object';
          try { Object.getOwnPropertyDescriptor(BarProp.prototype, 'visible').get.call({}); return 'brand'; }
          catch (error) { if (!(error instanceof TypeError)) return 'brand error'; }
          const converted = [];
          const value = name => ({ valueOf() { converted.push(name); return 1.9; } });
          for (const name of ['moveTo', 'moveBy', 'resizeTo', 'resizeBy']) {
            if (window[name](value(name + 'x'), value(name + 'y')) !== undefined) return name;
            try { window[name](1); return name + ' accepted one argument'; } catch (error) {
              if (!(error instanceof TypeError)) return name + ' error';
            }
          }
          return converted.join() === 'moveTox,moveToy,moveByx,moveByy,resizeTox,resizeToy,resizeByx,resizeByy' ||
            converted.join();
        }"#,
    );
}

#[test]
fn print_dispatches_beforeprint_then_afterprint() {
    check(
        &mut runtime(),
        r#"() => {
          const log = [];
          onbeforeprint = () => log.push('handler-before');
          addEventListener('beforeprint', event => log.push(event.type + ':' + event.cancelable));
          addEventListener('afterprint', event => log.push(event.type));
          if (print() !== undefined) return 'print result';
          return log.join() === 'handler-before,beforeprint:false,afterprint' || log.join();
        }"#,
    );
}

#[test]
fn document_domain_validates_suffixes_but_keeps_origin_keyed_access() {
    check(
        &mut runtime(),
        r#"() => {
          if (document.domain !== 'sub.example.com') return 'getter ' + document.domain;
          document.domain = 'sub.example.com';
          document.domain = 'example.com';
          for (const value of ['com', 'ample.com', 'other.com', '', 'a.sub.example.com']) {
            try { document.domain = value; return 'accepted ' + value; } catch (error) {
              if (error.name !== 'SecurityError') return value + ' ' + error.name;
            }
          }
          // Agent clusters are origin-keyed, so the effective domain is unchanged.
          if (document.domain !== 'sub.example.com') return 'changed ' + document.domain;
          const detached = document.implementation.createHTMLDocument('');
          if (detached.domain !== 'sub.example.com') return 'detached getter ' + detached.domain;
          try { detached.domain = 'example.com'; return 'detached setter accepted'; } catch (error) {
            return error.name === 'SecurityError' || error.name;
          }
        }"#,
    );
}

#[test]
fn opaque_origin_has_empty_domain_and_null_origin() {
    let mut runtime = runtime_at(
        r#"<html><body><iframe id="child" sandbox="allow-scripts" srcdoc="<script>
            parent.postMessage([document.domain, origin].join('|'), '*');
            try { document.domain = 'example.com'; parent.postMessage('set', '*'); }
            catch (error) { parent.postMessage(error.name, '*'); }
        </script>"></iframe></body></html>"#,
        "https://example.com/",
    );
    runtime
        .eval("globalThis.messages = []; addEventListener('message', event => messages.push(event.data));")
        .unwrap();
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        "() => messages.join() === '|null,SecurityError' || messages.join()",
    );
}

#[test]
fn last_modified_uses_response_header_or_current_time_in_html_format() {
    let mut runtime = runtime();
    check(
        &mut runtime,
        r#"() => {
          const value = document.lastModified;
          const match = /^(\d\d)\/(\d\d)\/(\d{4}) (\d\d):(\d\d):(\d\d)$/.exec(value);
          if (!match) return 'format ' + value;
          const parsed = new Date(+match[3], match[1] - 1, +match[2], +match[4], +match[5], +match[6]);
          return Math.abs(Date.now() - parsed.getTime()) < 5000 || 'not current ' + value;
        }"#,
    );
    runtime.install_document_response_headers(&[(
        "Last-Modified".into(),
        "Wed, 21 Oct 2015 07:28:00 GMT".into(),
    )]);
    check(
        &mut runtime,
        r#"() => {
          const date = new Date(Date.UTC(2015, 9, 21, 7, 28, 0));
          const pad = (value, width = 2) => String(value).padStart(width, '0');
          const expected = `${pad(date.getMonth() + 1)}/${pad(date.getDate())}/${date.getFullYear()} ` +
            `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
          return document.lastModified === expected || document.lastModified + ' != ' + expected;
        }"#,
    );
}

#[test]
fn document_scripts_is_a_live_html_collection() {
    check(
        &mut runtime(),
        r#"() => {
          const scripts = document.scripts;
          if (!(scripts instanceof HTMLCollection) || scripts !== document.scripts) return 'identity';
          if (scripts.length !== 1) return 'initial ' + scripts.length;
          const script = document.createElement('script');
          script.id = 'added';
          document.body.append(script);
          document.body.append(document.createElementNS('http://www.w3.org/2000/svg', 'script'));
          return (scripts.length === 2 && scripts.added === script && scripts[1] === script) ||
            'live ' + scripts.length;
        }"#,
    );
}

#[test]
fn navigator_reports_device_automation_and_viewer_state() {
    let mut runtime = runtime();
    check(
        &mut runtime,
        r#"() => {
          const concurrency = navigator.hardwareConcurrency;
          if (!Number.isInteger(concurrency) || concurrency < 1) return 'concurrency ' + concurrency;
          if (!('hardwareConcurrency' in Navigator.prototype)) return 'not on prototype';
          if (navigator.maxTouchPoints !== 0) return 'touch';
          if (navigator.pdfViewerEnabled !== false || navigator.plugins.length !== 0) return 'pdf';
          return navigator.webdriver === false || 'webdriver ' + navigator.webdriver;
        }"#,
    );
    runtime.enable_test_automation().unwrap();
    check(&mut runtime, "() => navigator.webdriver === true");
}

#[test]
fn user_activation_tracks_the_shared_transient_and_sticky_activation() {
    let mut runtime = runtime();
    check(
        &mut runtime,
        r#"() => {
          const activation = navigator.userActivation;
          if (!(activation instanceof UserActivation) || activation !== navigator.userActivation) return 'identity';
          try { new UserActivation(); return 'constructible'; } catch (_) {}
          if (activation.hasBeenActive || activation.isActive) return 'initially active';
          globalThis.during = [];
          document.addEventListener('keydown', () => {
            during.push(activation.isActive, activation.hasBeenActive);
            // A user-activation-gated API consumes the transient activation.
            document.body.requestFullscreen().catch(() => {});
            during.push(activation.isActive, activation.hasBeenActive);
          }, { once: true });
          __omoikane_dispatch_keyboard_input('keydown', { key: 'Enter', code: 'Enter' });
          return during.join() === 'true,true,false,true' || during.join();
        }"#,
    );
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        "() => (!navigator.userActivation.isActive && navigator.userActivation.hasBeenActive) || 'after'",
    );
}

#[test]
fn location_ancestor_origins_lists_containers_nearest_first() {
    let mut runtime = runtime_at(
        r#"<html><body>
          <iframe id="plain" srcdoc="<script>parent.postMessage(['plain', location.ancestorOrigins.length, location.ancestorOrigins[0], location.ancestorOrigins.contains('https://example.com'), location.ancestorOrigins instanceof DOMStringList, location.ancestorOrigins === location.ancestorOrigins].join(), '*')</script>"></iframe>
          <iframe id="hidden" referrerpolicy="no-referrer" srcdoc="<script>parent.postMessage(['hidden', location.ancestorOrigins.item(0)].join(), '*')</script>"></iframe>
        </body></html>"#,
        "https://example.com/",
    );
    runtime
        .eval("globalThis.messages = []; addEventListener('message', event => messages.push(event.data));")
        .unwrap();
    runtime.run_until_idle().unwrap();
    check(
        &mut runtime,
        r#"() => {
          if (location.ancestorOrigins.length !== 0 || location.ancestorOrigins.item(0) !== null) return 'top';
          const sorted = messages.slice().sort().join('|');
          return sorted === 'hidden,null|plain,1,https://example.com,true,true,true' || sorted;
        }"#,
    );
}

#[test]
fn performance_to_json_serializes_time_origin() {
    check(
        &mut runtime(),
        r#"() => {
          const json = JSON.parse(JSON.stringify(performance));
          return (typeof performance.toJSON === 'function' && json.timeOrigin === performance.timeOrigin) ||
            JSON.stringify(json);
        }"#,
    );
}
