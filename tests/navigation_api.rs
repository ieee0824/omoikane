use omoikane::{html::TreeBuilder, js::JsRuntime};

fn runtime() -> JsRuntime {
    let mut runtime =
        JsRuntime::with_document(TreeBuilder::parse("<main id='app'></main>").document())
            .expect("runtime");
    runtime.set_base_url("https://example.test/start".parse().expect("base URL"));
    runtime
}

fn json(runtime: &mut JsRuntime, source: &str) -> String {
    runtime
        .eval(source)
        .expect("evaluate navigation probe")
        .as_string()
        .expect("JSON string")
        .to_std_string_escaped()
}

#[test]
fn navigation_entries_share_the_existing_history_controller() {
    let mut runtime = runtime();
    assert_eq!(
        json(
            &mut runtime,
            r#"(() => {
              const initial = navigation.currentEntry;
              history.pushState({page: 2}, '', '/two');
              const current = navigation.currentEntry;
              return JSON.stringify([
                navigation instanceof Navigation,
                initial instanceof NavigationHistoryEntry,
                navigation.entries().length,
                initial.index, current.index, current.url,
                current.getState(), current.sameDocument,
                initial.key !== current.key, initial.id !== current.id,
                navigation.canGoBack, navigation.canGoForward
              ]);
            })()"#,
        ),
        "[true,true,2,0,1,\"http://localhost/two\",{\"page\":2},true,true,true,true,false]"
    );
}

#[test]
fn intercepted_navigation_commits_before_finishing_in_event_order() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"globalThis.events = [];
               navigation.addEventListener('navigate', event => {
                 events.push('navigate:' + event.navigationType + ':' + event.destination.url);
                 event.intercept({ handler: async () => { events.push('handler'); } });
               });
               navigation.addEventListener('currententrychange', () => events.push('commit'));
               navigation.addEventListener('navigatesuccess', () => events.push('success'));
               globalThis.result = navigation.navigate('/next', { state: { ok: true } });
               result.committed.then(entry => events.push('committed:' + entry.url));
               result.finished.then(entry => events.push('finished:' + entry.url));"#,
        )
        .expect("start intercepted navigation");
    runtime.run_jobs().expect("navigation jobs");
    assert_eq!(
        json(
            &mut runtime,
            "JSON.stringify([events, navigation.currentEntry.getState(), location.pathname])",
        ),
        "[[\"navigate:push:http://localhost/next\",\"commit\",\"handler\",\"committed:http://localhost/next\",\"success\",\"finished:http://localhost/next\"],{\"ok\":true},\"/next\"]"
    );
}

#[test]
fn traverse_to_back_forward_and_update_current_entry_are_live() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"history.pushState({page: 2}, '', '/two');
               history.pushState({page: 3}, '', '/three');
               globalThis.firstKey = navigation.entries()[0].key;
               globalThis.traversal = navigation.traverseTo(firstKey);"#,
        )
        .expect("traverse");
    runtime.run_jobs().expect("traversal jobs");
    assert_eq!(
        json(
            &mut runtime,
            r#"(() => {
              navigation.updateCurrentEntry({ state: {updated: true} });
              return JSON.stringify([
                navigation.currentEntry.index, location.pathname,
                navigation.currentEntry.getState(), navigation.canGoBack,
                navigation.canGoForward, traversal.finished instanceof Promise
              ]);
            })()"#,
        ),
        "[0,\"/\",{\"updated\":true},false,true,true]"
    );
}

#[test]
fn entry_identity_handlers_and_constructor_boundaries_match_navigation_idl() {
    let mut runtime = runtime();
    assert_eq!(
        json(
            &mut runtime,
            r#"(() => {
              const uuid = /^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/;
              const initial = navigation.currentEntry;
              let change = null;
              navigation.oncurrententrychange = event => change = [event.navigationType, event.from === initial, event.isTrusted];
              history.pushState(1, '', '#one');
              const current = navigation.currentEntry;
              return JSON.stringify([
                initial === navigation.entries()[0], current === navigation.entries()[1],
                uuid.test(initial.key), uuid.test(initial.id), change,
                (() => { try { new NavigationCurrentEntryChangeEvent('x'); return false; } catch (e) { return e instanceof TypeError; } })(),
                (() => { try { new NavigateEvent('x', { destination: {} }); return false; } catch (e) { return e instanceof TypeError; } })(),
                navigation.oncurrententrychange instanceof Function
              ]);
            })()"#,
        ),
        "[true,true,true,true,[\"push\",true,true],true,true,true]"
    );
}

#[test]
fn history_api_dispatches_navigate_before_current_entry_change() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"globalThis.events = [];
               navigation.onnavigate = event => {
                 events.push(['navigate', event.navigationType, event.hashChange, navigation.currentEntry.url]);
                 event.intercept({ handler: () => events.push(['handler', location.hash]) });
               };
               navigation.oncurrententrychange = event => events.push(['change', event.navigationType, event.from.index]);
               navigation.onnavigatesuccess = () => events.push(['success']);
               history.pushState('update', '', '#one');"#,
        )
        .expect("push history entry");
    runtime.run_jobs().expect("history navigation jobs");
    assert_eq!(
        json(&mut runtime, "JSON.stringify(events)"),
        "[[\"navigate\",\"push\",true,\"http://localhost/\"],[\"change\",\"push\",0],[\"handler\",\"#one\"],[\"success\"]]"
    );
}
