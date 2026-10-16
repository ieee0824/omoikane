//! Frame policy introspection describes the container without fetching a child.
use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;
use std::sync::Mutex;

// Both stress tests create 80 realms; keep their resource use independent.
static REALM_STRESS_MUTEX: Mutex<()> = Mutex::new(());

fn runtime() -> JsRuntime {
    let document = TreeBuilder::parse("<!doctype html><body></body>").document();
    JsRuntime::with_document_and_url(document, "https://owner.test/page").unwrap()
}

#[test]
fn frame_policy_is_stable_and_tracks_attribute_changes_before_navigation() {
    let mut runtime = runtime();
    let result = runtime
        .eval(
            r#"
            const frame = document.createElement('iframe');
            const policy = frame.permissionsPolicy;
            if (!policy || !policy.allowsFeature('fullscreen')) {
              throw new Error('detached iframe policy is unavailable');
            }
            document.body.appendChild(frame);
            const checks = [policy === frame.permissionsPolicy,
                            policy === frame.featurePolicy,
                            policy.allowsFeature('fullscreen')];
            frame.src = 'https://other.test/frame';
            checks.push(!policy.allowsFeature('fullscreen'));
            frame.allow = 'fullscreen *';
            checks.push(policy.allowsFeature('fullscreen'));
            frame.allow = "fullscreen 'none'";
            checks.push(!policy.allowedFeatures().includes('fullscreen'));
            frame.allow = "fullscreen 'self'";
            frame.srcdoc = '<p>local</p>';
            checks.push(policy.allowsFeature('fullscreen'));
            checks.every(Boolean);
            "#,
        )
        .unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn many_sibling_iframe_realms_do_not_exhaust_the_vm_value_stack() {
    let _serial = REALM_STRESS_MUTEX
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut runtime = runtime();
    runtime
        .eval(
            r#"
            globalThis.loaded = 0;
            for (let i = 0; i < 80; ++i) {
              const frame = document.createElement('iframe');
              frame.srcdoc = '<script>parent.loaded++</script>';
              document.body.appendChild(frame);
            }
            "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    let errors = runtime.take_task_errors();
    assert!(errors.is_empty(), "iframe task errors: {errors:?}");
    assert_eq!(runtime.eval("loaded").unwrap().as_number(), Some(80.0));
}

#[test]
fn caught_script_errors_do_not_poison_later_iframe_realm_initialization() {
    let _serial = REALM_STRESS_MUTEX
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut runtime = runtime();
    runtime
        .eval(
            r#"
            globalThis.loaded = 0;
            globalThis.caught = 0;
            function record(callback) {
              try { callback(); } catch (error) { caught++; String(error.stack); }
            }
            for (let i = 0; i < 80; ++i) {
              record(() => {
                const frame = document.createElement('iframe');
                frame.srcdoc = '<script>parent.loaded++</script>';
                document.body.appendChild(frame);
                const absent = undefined;
                absent.allowedFeatures();
              });
            }
            "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    let errors = runtime.take_task_errors();
    assert!(errors.is_empty(), "iframe task errors: {errors:?}");
    assert_eq!(runtime.eval("caught").unwrap().as_number(), Some(80.0));
    assert_eq!(runtime.eval("loaded").unwrap().as_number(), Some(80.0));
}
