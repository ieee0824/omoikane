use omoikane::{
    html::TreeBuilder,
    js::{JsRuntime, StorageManager},
};

fn runtime_with_storage(url: &str, storage: StorageManager, session_id: u64) -> JsRuntime {
    JsRuntime::with_document_url_and_storage(
        TreeBuilder::parse("<body></body>").document(),
        url,
        storage,
        session_id,
    )
    .expect("create JavaScript runtime")
}

fn string(runtime: &mut JsRuntime, source: &str) -> String {
    runtime
        .eval(source)
        .unwrap_or_else(|error| panic!("{source}: {error}"))
        .as_string()
        .expect("expression must return a string")
        .to_std_string_escaped()
}

#[test]
fn exposes_secure_surface_and_holds_until_callback_settles() {
    let storage = StorageManager::new();
    let session = storage.create_session();
    let mut runtime = runtime_with_storage("https://locks.test/page", storage, session);
    runtime
        .eval(
            r#"
            globalThis.lockLog = [];
            globalThis.releaseFirst = null;
            const blocker = new Promise(resolve => { releaseFirst = resolve; });
            navigator.locks.request("resource", lock => {
              lockLog.push(lock.name + ":" + lock.mode + ":first");
              return blocker;
            }).then(() => lockLog.push("first-released"));
            navigator.locks.request("resource", lock => {
              lockLog.push(lock.name + ":" + lock.mode + ":second");
              return 42;
            }).then(value => lockLog.push("second-result:" + value));
            "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        string(&mut runtime, "lockLog.join(',')"),
        "resource:exclusive:first"
    );

    runtime
        .eval("navigator.locks.query().then(state => globalThis.lockState = JSON.stringify(state))")
        .unwrap();
    runtime.run_until_idle().unwrap();
    let state = string(&mut runtime, "lockState");
    assert!(state.contains(r#""held":[{"#), "{state}");
    assert!(state.contains(r#""pending":[{"#), "{state}");
    assert!(state.contains(r#""name":"resource""#), "{state}");

    runtime.eval("releaseFirst('done')").unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        string(&mut runtime, "lockLog.join(',')"),
        "resource:exclusive:first,first-released,resource:exclusive:second,second-result:42"
    );
}

#[test]
fn supports_shared_if_available_abort_and_steal() {
    let storage = StorageManager::new();
    let session = storage.create_session();
    let mut runtime = runtime_with_storage("https://locks-options.test/page", storage, session);
    runtime
        .eval(
            r#"
            globalThis.optionLog = [];
            globalThis.releaseShared = null;
            const sharedBlocker = new Promise(resolve => { releaseShared = resolve; });
            navigator.locks.request("shared", { mode: "shared" }, () => sharedBlocker).catch(() => {});
            navigator.locks.request("shared", { mode: "shared", ifAvailable: true }, lock => {
              optionLog.push(lock ? "shared-granted" : "shared-missing");
            });
            navigator.locks.request("shared", { ifAvailable: true }, lock => {
              optionLog.push(lock === null ? "exclusive-missing" : "exclusive-granted");
            });

            const controller = new AbortController();
            navigator.locks.request("abort", { signal: controller.signal }, () => {
              optionLog.push("abort-callback");
            }).catch(error => optionLog.push("abort:" + error.name));
            controller.abort();

            const never = new Promise(() => {});
            navigator.locks.request("stolen", () => never)
              .catch(error => optionLog.push("stolen:" + error.name));
            navigator.locks.request("stolen", { steal: true }, () => {
              optionLog.push("stealer");
            });
            "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        string(&mut runtime, "optionLog.join(',')"),
        "shared-granted,exclusive-missing,abort:AbortError,stealer,stolen:AbortError"
    );
    runtime.eval("releaseShared()").unwrap();
    runtime.run_until_idle().unwrap();
}

#[test]
fn same_origin_runtimes_contend_and_other_origins_are_isolated() {
    let storage = StorageManager::new();
    let session = storage.create_session();
    let mut first = runtime_with_storage("https://same.test/first", storage.clone(), session);
    let mut second = runtime_with_storage("https://same.test/second", storage.clone(), session);
    let mut other = runtime_with_storage("https://other.test/page", storage, session);

    first
        .eval(
            "globalThis.release = null; navigator.locks.request('shared-name', () => new Promise(resolve => release = resolve));",
        )
        .unwrap();
    first.run_until_idle().unwrap();
    second
        .eval(
            "globalThis.secondGranted = false; navigator.locks.request('shared-name', () => { secondGranted = true; });",
        )
        .unwrap();
    second.run_until_idle().unwrap();
    assert_eq!(string(&mut second, "String(secondGranted)"), "false");

    other
        .eval(
            "globalThis.otherGranted = false; navigator.locks.request('shared-name', () => { otherGranted = true; });",
        )
        .unwrap();
    other.run_until_idle().unwrap();
    assert_eq!(string(&mut other, "String(otherGranted)"), "true");

    first.eval("release()").unwrap();
    first.run_until_idle().unwrap();
    second.run_until_idle().unwrap();
    assert_eq!(string(&mut second, "String(secondGranted)"), "true");
}

#[test]
fn insecure_context_does_not_expose_web_locks() {
    let storage = StorageManager::new();
    let session = storage.create_session();
    let mut runtime = runtime_with_storage("http://insecure.example/page", storage, session);
    assert_eq!(
        string(
            &mut runtime,
            "typeof Lock + ':' + typeof LockManager + ':' + typeof navigator.locks",
        ),
        "undefined:undefined:undefined"
    );
}

#[test]
fn null_options_are_converted_to_an_empty_dictionary() {
    let storage = StorageManager::new();
    let session = storage.create_session();
    let mut runtime = runtime_with_storage("https://null-options.test/page", storage, session);
    runtime
        .eval(
            r#"
            globalThis.nullOptionsResult = "pending";
            navigator.locks.request("resource", null, lock => lock.mode)
              .then(value => { nullOptionsResult = value; });
            "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(string(&mut runtime, "nullOptionsResult"), "exclusive");
}

#[test]
fn iframe_removal_releases_its_lock_and_preserves_distinct_client_ids() {
    let storage = StorageManager::new();
    let session = storage.create_session();
    let mut runtime = JsRuntime::with_document_url_and_storage(
        TreeBuilder::parse("<body><iframe id=child></iframe></body>").document(),
        "https://frame-locks.test/page",
        storage,
        session,
    )
    .unwrap();
    runtime
        .eval(
            r#"
            globalThis.frame = document.getElementById("child");
            globalThis.child = frame.contentWindow;
            child.eval("globalThis.never = new Promise(() => {}); navigator.locks.request('frame-resource', () => never);");
            globalThis.parentGranted = false;
            navigator.locks.request("frame-resource", () => { parentGranted = true; });
            navigator.locks.query().then(state => {
              const locks = state.held.concat(state.pending).filter(lock => lock.name === "frame-resource");
              globalThis.distinctFrameClients = locks.length === 2 && locks[0].clientId !== locks[1].clientId;
            });
            "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(string(&mut runtime, "String(parentGranted)"), "false");
    assert_eq!(string(&mut runtime, "String(distinctFrameClients)"), "true");

    runtime.eval("frame.remove()").unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(string(&mut runtime, "String(parentGranted)"), "true");
}

#[test]
fn terminating_worker_releases_same_origin_lock() {
    let storage = StorageManager::new();
    let session = storage.create_session();
    let mut runtime = runtime_with_storage("https://worker-locks.test/page", storage, session);
    runtime
        .eval(
            r#"
            globalThis.workerEvents = [];
            const source = encodeURIComponent(
              "navigator.locks.request('worker-resource', lock => {" +
              "postMessage('held'); return new Promise(() => {}); });"
            );
            globalThis.lockWorker = new Worker("data:text/javascript," + source);
            lockWorker.onmessage = event => workerEvents.push(event.data);
            "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(string(&mut runtime, "workerEvents.join(',')"), "held");

    runtime
        .eval(
            "globalThis.ownerGranted = false; navigator.locks.request('worker-resource', () => { ownerGranted = true; });",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(string(&mut runtime, "String(ownerGranted)"), "false");

    runtime.eval("lockWorker.terminate()").unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(string(&mut runtime, "String(ownerGranted)"), "true");
}
