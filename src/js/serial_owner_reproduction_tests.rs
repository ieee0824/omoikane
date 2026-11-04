use super::*;

#[test]
fn web_serial_retiring_unused_same_origin_document_preserves_other_session() {
    let provider = Arc::new(DeterministicSerialProvider::default());
    provider.add_port(SerialPortInfo {
        id: "retired-port".into(),
        usb_vendor_id: Some(9),
        usb_product_id: None,
        bluetooth_service_class_id: None,
        connected: true,
    });
    let origin = "https://retire.serial.test";
    provider.request_port(origin, &[], &[]).unwrap();
    let mut runtime =
        JsRuntime::with_document_and_url(default_document(), "https://retire.serial.test/page")
            .unwrap();
    runtime.set_serial_provider(provider.clone());
    let options = SerialOptions {
        baud_rate: 9600,
        data_bits: 8,
        stop_bits: 1,
        parity: "none".into(),
        buffer_size: 255,
        flow_control: "none".into(),
    };
    runtime
        .host_state
        .borrow()
        .web_serial
        .open(
            runtime.document().identity(),
            origin,
            "retired-port",
            &options,
        )
        .unwrap();
    let token = runtime
        .host_state
        .borrow()
        .web_serial
        .session_token_for_test(runtime.document().identity(), "retired-port");
    // A owns the open provider session; B never opens any port.
    let mut other =
        JsRuntime::with_document_and_url(default_document(), "https://retire.serial.test/unused")
            .unwrap();
    other.set_serial_provider(provider.clone());
    let document_id = other.document().identity();
    other
        .host_state
        .borrow()
        .retire_web_serial_document(document_id);
    assert!(
        provider
            .write(origin, "retired-port", token, &[1, 2])
            .is_ok(),
        "retiring a document without a serial session must preserve the other owner"
    );
    assert_eq!(provider.writes("retired-port"), vec![vec![1, 2]]);
    runtime
        .host_state
        .borrow()
        .retire_web_serial_document(runtime.document().identity());
    assert!(
        provider.open(origin, "retired-port", &options).is_ok(),
        "retiring the owner must release the session"
    );
}

#[test]
fn web_serial_js_open_survives_unused_same_origin_iframe_navigation() {
    let provider = Arc::new(DeterministicSerialProvider::default());
    provider.add_port(SerialPortInfo {
        id: "js-owned-port".into(),
        usb_vendor_id: None,
        usb_product_id: None,
        bluetooth_service_class_id: None,
        connected: true,
    });
    provider
        .request_port("https://retire.serial.test", &[], &[])
        .unwrap();
    let mut runtime =
        JsRuntime::with_document_and_url(default_document(), "https://retire.serial.test/page")
            .unwrap();
    runtime.set_serial_provider(provider.clone());
    runtime
        .eval(
            r#"
        var serialPort;
        var serialStatus = 'pending';
        (async () => {
            const ports = await navigator.serial.getPorts();
            serialPort = ports[0];
            await serialPort.open({baudRate: 9600});
            serialStatus = 'opened';
        })().catch(error => serialStatus = error.name + ':' + error.message);
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("serialStatus")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "opened"
    );
    runtime
        .eval(
            r#"
        var unusedFrame = document.createElement('iframe');
        unusedFrame.srcdoc = '<p>unused</p>';
        document.body.appendChild(unusedFrame);
        void unusedFrame.contentDocument;
    "#,
        )
        .unwrap();
    let previous = runtime
        .host_state
        .borrow()
        .iframe_documents
        .values()
        .next()
        .expect("iframe document was created")
        .document
        .identity();
    runtime
        .eval("unusedFrame.srcdoc = '<p>replacement</p>'; void unusedFrame.contentDocument;")
        .unwrap();
    assert!(
        !runtime
            .host_state
            .borrow()
            .iframe_documents
            .values()
            .any(|entry| entry.document.identity() == previous),
        "old iframe document must retire"
    );
    runtime
        .eval(
            r#"
        serialStatus = 'pending';
        (async () => {
            const writer = serialPort.writable.getWriter();
            try { await writer.write(Uint8Array.of(4, 5)); serialStatus = 'written'; }
            finally { writer.releaseLock(); }
        })().catch(error => serialStatus = error.name + ':' + error.message);
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("serialStatus")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "written",
        "retiring an unused iframe must preserve the parent JS-opened serial session"
    );
    assert_eq!(provider.writes("js-owned-port"), vec![vec![4, 5]]);
}

#[test]
fn web_serial_stale_session_cannot_close_or_write_reopened_port() {
    let provider = DeterministicSerialProvider::default();
    provider.add_port(SerialPortInfo {
        id: "reconnected".into(),
        usb_vendor_id: None,
        usb_product_id: None,
        bluetooth_service_class_id: None,
        connected: true,
    });
    let origin = "https://serial.test";
    provider.request_port(origin, &[], &[]).unwrap();
    let options = SerialOptions {
        baud_rate: 9600,
        data_bits: 8,
        stop_bits: 1,
        parity: "none".into(),
        buffer_size: 255,
        flow_control: "none".into(),
    };
    let old = provider.open(origin, "reconnected", &options).unwrap();
    provider.set_connected("reconnected", false);
    provider.set_connected("reconnected", true);
    let new = provider.open(origin, "reconnected", &options).unwrap();
    assert_ne!(old, new);
    provider.retire(origin, "reconnected", old);
    provider.fail_next(SerialError {
        name: "NetworkError".into(),
        message: "current connection close failed".into(),
    });
    provider.close(origin, "reconnected", old).unwrap();
    assert_eq!(
        provider.read(origin, "reconnected", old).unwrap_err().name,
        "InvalidStateError"
    );
    assert_eq!(
        provider
            .get_signals(origin, "reconnected", old)
            .unwrap_err()
            .name,
        "InvalidStateError"
    );
    assert_eq!(
        provider
            .set_signals(origin, "reconnected", old, &SerialOutputSignals::default())
            .unwrap_err()
            .name,
        "InvalidStateError"
    );
    assert_eq!(
        provider
            .write(origin, "reconnected", old, &[0])
            .unwrap_err()
            .name,
        "InvalidStateError"
    );
    assert_eq!(
        provider.close(origin, "reconnected", new).unwrap_err().name,
        "NetworkError"
    );
    assert!(provider.get_signals(origin, "reconnected", new).is_ok());
    provider.write(origin, "reconnected", new, &[1]).unwrap();
    assert_eq!(provider.writes("reconnected"), vec![vec![1]]);
    provider.close(origin, "reconnected", new).unwrap();
    assert_eq!(
        provider.read(origin, "reconnected", new).unwrap_err().name,
        "InvalidStateError"
    );
}

fn opened_owned_runtime() -> (JsRuntime, Arc<DeterministicSerialProvider>) {
    let provider = Arc::new(DeterministicSerialProvider::default());
    provider.add_port(SerialPortInfo {
        id: "owned".into(),
        usb_vendor_id: None,
        usb_product_id: None,
        bluetooth_service_class_id: None,
        connected: true,
    });
    provider
        .request_port("https://owner.serial.test", &[], &[])
        .unwrap();
    let mut runtime =
        JsRuntime::with_document_and_url(default_document(), "https://owner.serial.test/page")
            .unwrap();
    runtime.set_serial_provider(provider.clone());
    runtime.eval("var serialPort; var serialStatus='pending'; (async()=>{serialPort=(await navigator.serial.getPorts())[0];await serialPort.open({baudRate:9600});serialStatus='opened';})().catch(e=>serialStatus=e.name);").unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("serialStatus")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "opened"
    );
    (runtime, provider)
}

#[test]
fn web_serial_close_failure_keeps_owner_for_retry() {
    let (mut runtime, provider) = opened_owned_runtime();
    provider.fail_next(SerialError {
        name: "NetworkError".into(),
        message: "close failed".into(),
    });
    runtime.eval("serialStatus='pending';serialPort.close().then(()=>serialStatus='closed',e=>serialStatus=e.name);").unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("serialStatus")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "NetworkError"
    );
    runtime.eval("serialStatus='pending';serialPort.close().then(()=>serialStatus='closed',e=>serialStatus=e.name);").unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("serialStatus")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "closed"
    );
}

#[test]
fn web_serial_retire_invalidates_even_with_pending_close_error() {
    let (runtime, provider) = opened_owned_runtime();
    let document = runtime.document().identity();
    let token = runtime
        .host_state
        .borrow()
        .web_serial
        .session_token_for_test(document, "owned");
    provider.fail_next(SerialError {
        name: "NetworkError".into(),
        message: "close failed".into(),
    });
    runtime
        .host_state
        .borrow()
        .retire_web_serial_document(document);
    // Retirement does not consume a fallible-operation error. Consume it first
    // so the token check below cannot pass because of the injected error.
    assert_eq!(
        provider
            .request_port("https://owner.serial.test", &[], &[])
            .unwrap_err()
            .name,
        "NetworkError"
    );
    assert_eq!(
        provider
            .get_signals("https://owner.serial.test", "owned", token)
            .unwrap_err()
            .name,
        "InvalidStateError"
    );
}

#[test]
fn web_serial_provider_replacement_does_not_retarget_retirement() {
    let (mut runtime, original) = opened_owned_runtime();
    let document = runtime.document().identity();
    let old = runtime
        .host_state
        .borrow()
        .web_serial
        .session_token_for_test(document, "owned");
    let (replacement_runtime, replacement) = opened_owned_runtime();
    let replacement_token = replacement_runtime
        .host_state
        .borrow()
        .web_serial
        .session_token_for_test(replacement_runtime.document().identity(), "owned");
    assert_eq!(
        old, replacement_token,
        "independent providers may issue the same scalar token"
    );
    // The replacement's token is deliberately the same scalar from another
    // provider. Retiring the original must use the issuing Arc as well.
    runtime.set_serial_provider(replacement.clone());
    runtime
        .host_state
        .borrow()
        .retire_web_serial_document(document);
    assert_eq!(
        original
            .get_signals("https://owner.serial.test", "owned", old)
            .unwrap_err()
            .name,
        "InvalidStateError"
    );
    replacement
        .write(
            "https://owner.serial.test",
            "owned",
            replacement_token,
            &[8],
        )
        .unwrap();
    assert_eq!(replacement.writes("owned"), vec![vec![8]]);
}

#[test]
fn web_serial_runtime_drop_retires_owned_session() {
    let (runtime, provider) = opened_owned_runtime();
    let token = runtime
        .host_state
        .borrow()
        .web_serial
        .session_token_for_test(runtime.document().identity(), "owned");
    drop(runtime);
    assert_eq!(
        provider
            .get_signals("https://owner.serial.test", "owned", token)
            .unwrap_err()
            .name,
        "InvalidStateError",
        "dropping the runtime must retire its remaining sessions"
    );
}

#[test]
fn web_serial_old_document_retire_preserves_reconnected_other_owner() {
    let (first, provider) = opened_owned_runtime();
    provider.set_connected("owned", false);
    provider.set_connected("owned", true);
    let mut second =
        JsRuntime::with_document_and_url(default_document(), "https://owner.serial.test/second")
            .unwrap();
    second.set_serial_provider(provider.clone());
    second.eval("var reconnectedStatus='pending';(async()=>{const port=(await navigator.serial.getPorts())[0];await port.open({baudRate:9600});reconnectedStatus='opened';})().catch(e=>reconnectedStatus=e.name);").unwrap();
    second.run_until_idle().unwrap();
    assert_eq!(
        second
            .eval("reconnectedStatus")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "opened"
    );
    let token = second
        .host_state
        .borrow()
        .web_serial
        .session_token_for_test(second.document().identity(), "owned");
    first
        .host_state
        .borrow()
        .retire_web_serial_document(first.document().identity());
    provider
        .write("https://owner.serial.test", "owned", token, &[6])
        .unwrap();
    assert_eq!(provider.writes("owned"), vec![vec![6]]);
}

fn serial_status(runtime: &mut JsRuntime) -> String {
    runtime.run_until_idle().unwrap();
    runtime
        .eval("serialStatus")
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped()
}

#[test]
fn web_serial_close_after_disconnect_allows_the_same_object_to_reopen() {
    let (mut runtime, provider) = opened_owned_runtime();
    provider.set_connected("owned", false);
    runtime.eval("serialStatus='pending';serialPort.close().then(()=>serialStatus='closed',e=>serialStatus=e.name);").unwrap();
    assert_eq!(serial_status(&mut runtime), "closed");
    provider.set_connected("owned", true);
    runtime.eval("serialStatus='pending';serialPort.open({baudRate:9600}).then(()=>serialStatus='reopened',e=>serialStatus=e.name);").unwrap();
    assert_eq!(serial_status(&mut runtime), "reopened");
}

#[test]
fn web_serial_close_finishes_old_streams_before_reopening() {
    let (mut runtime, provider) = opened_owned_runtime();
    runtime
        .eval(
            r#"
        serialStatus='pending';
        (async()=>{
            const readable=serialPort.readable, writable=serialPort.writable;
            const lockedWriter=writable.getWriter();
            const lockedRejected=await serialPort.close().then(()=>false,e=>e.name==='InvalidStateError');
            try {await lockedWriter.write(Uint8Array.of(1));}finally{lockedWriter.releaseLock();}
            const closed=serialPort.close();
            const secondRejected=serialPort.close().then(()=>false,e=>e.name==='InvalidStateError');
            const signalsRejected=serialPort.getSignals().then(()=>false,e=>e.name==='InvalidStateError');
            const setRejected=serialPort.setSignals({dataTerminalReady:true}).then(()=>false,e=>e.name==='InvalidStateError');
            const lateReader=readable.getReader();
            await closed;
            let earlyDone;
            try {earlyDone=(await lateReader.read()).done;}finally{lateReader.releaseLock();}
            const closingResults=await Promise.all([secondRejected,signalsRejected,setRejected]);
            await serialPort.open({baudRate:9600});
            const reader=readable.getReader();
            let done;
            try { done=(await reader.read()).done; } finally { reader.releaseLock(); }
            const oldWriter=writable.getWriter();
            let rejected=false;
            try { await oldWriter.write(Uint8Array.of(99)); }
            catch (_) { rejected=true; } finally { oldWriter.releaseLock(); }
            const writer=serialPort.writable.getWriter();
            try { await writer.write(Uint8Array.of(7)); } finally { writer.releaseLock(); }
            serialStatus=[lockedRejected,...closingResults,earlyDone,done,rejected].join(':');
        })().catch(e=>serialStatus='outer:'+e.name);
    "#,
        )
        .unwrap();
    assert_eq!(
        serial_status(&mut runtime),
        "true:true:true:true:true:true:true"
    );
    assert_eq!(provider.writes("owned"), vec![vec![1], vec![7]]);
}

#[test]
fn web_serial_signal_getter_cannot_control_a_closing_session() {
    let (mut runtime, provider) = opened_owned_runtime();
    provider.fail_next(SerialError {
        name: "NetworkError".into(),
        message: "close must retain its pending backend error".into(),
    });
    runtime
        .eval(
            r#"
        serialStatus='pending';
        (async()=>{
            const counts=[0,0,0];
            let closed;
            const result=await serialPort.setSignals({
                get dataTerminalReady(){counts[0]++;closed=serialPort.close();return true;},
                get requestToSend(){counts[1]++;return true;},
                get break(){counts[2]++;return false;},
            }).then(()=>'accepted',e=>e.name);
            const closeResult=await closed.then(()=>'closed',e=>e.name);
            serialStatus=[result,closeResult,...counts].join(':');
        })().catch(e=>serialStatus='outer:'+e.name);
    "#,
        )
        .unwrap();
    assert_eq!(
        serial_status(&mut runtime),
        "InvalidStateError:NetworkError:1:1:1"
    );
    runtime
        .eval(
            r#"
        serialStatus='pending';
        (async()=>{
            await serialPort.close();
            await serialPort.open({baudRate:9600});
            const counts=[0,0,0];
            await serialPort.setSignals({
                get dataTerminalReady(){counts[0]++;return true;},
                get requestToSend(){counts[1]++;return true;},
                get break(){counts[2]++;return false;},
            });
            const writer=serialPort.writable.getWriter();
            try {await writer.write(Uint8Array.of(8));}finally{writer.releaseLock();}
            serialStatus=counts.join(':');
        })().catch(e=>serialStatus='outer:'+e.name);
    "#,
        )
        .unwrap();
    assert_eq!(serial_status(&mut runtime), "1:1:1");
    assert_eq!(provider.writes("owned"), vec![vec![8]]);
}

#[test]
fn web_serial_old_object_callbacks_cannot_use_a_replacement_connection() {
    let (mut runtime, provider) = opened_owned_runtime();
    provider.set_connected("owned", false);
    runtime.eval("var oldPort=serialPort;serialStatus='pending';oldPort.forget().then(()=>serialStatus='forgotten',e=>serialStatus=e.name);").unwrap();
    assert_eq!(serial_status(&mut runtime), "forgotten");
    provider.set_connected("owned", true);
    provider
        .request_port("https://owner.serial.test", &[], &[])
        .unwrap();
    provider.enqueue_read("owned", vec![11]);
    runtime.eval(r#"
        var freshPort;serialStatus='pending';
        (async()=>{
            freshPort=(await navigator.serial.getPorts())[0];
            await freshPort.open({baudRate:9600});
            const outcomes=[];
            const operations=[
                ()=>oldPort.getSignals(),
                ()=>oldPort.setSignals({dataTerminalReady:true}),
                async()=>{const reader=oldPort.readable.getReader();try {await reader.read();}finally{reader.releaseLock();}},
                async()=>{const writer=oldPort.writable.getWriter();try {await writer.write(Uint8Array.of(99));}finally{writer.releaseLock();}},
            ];
            for(const operation of operations) {
                try {await operation();outcomes.push('accepted');}
                catch(error) {outcomes.push(error.name);}
            }
            await oldPort.close();
            const writer=freshPort.writable.getWriter();
            try {await writer.write(Uint8Array.of(6));}finally{writer.releaseLock();}
            serialStatus=outcomes.join(',')+':'+(oldPort!==freshPort);
        })().catch(e=>serialStatus='outer:'+e.name);
    "#).unwrap();
    assert_eq!(
        serial_status(&mut runtime),
        "InvalidStateError,InvalidStateError,InvalidStateError,InvalidStateError:true"
    );
    assert_eq!(provider.writes("owned"), vec![vec![6]]);
    runtime
        .eval(
            r#"
        serialStatus='pending';
        (async()=>{const reader=freshPort.readable.getReader();
            try {serialStatus=Array.from((await reader.read()).value).join(',');}
            finally {reader.releaseLock();}
        })().catch(e=>serialStatus=e.name);
    "#,
        )
        .unwrap();
    assert_eq!(serial_status(&mut runtime), "11");
}

#[test]
fn web_serial_signal_updates_preserve_omitted_outputs() {
    let (mut runtime, provider) = opened_owned_runtime();
    for (script, expected) in [
        ("{dataTerminalReady:true,break:true}", (true, false, true)),
        ("{requestToSend:true}", (true, true, true)),
        ("{requestToSend:false}", (true, false, true)),
        ("{dataTerminalReady:null}", (false, false, true)),
        (
            "{break:false,requestToSend:undefined}",
            (false, false, false),
        ),
    ] {
        runtime.eval(&format!(
            "serialStatus='pending';serialPort.setSignals({script}).then(()=>serialStatus='ok',e=>serialStatus=e.name);"
        )).unwrap();
        assert_eq!(serial_status(&mut runtime), "ok");
        assert_eq!(provider.output_signals_for_test("owned"), expected);
    }
}

#[test]
fn web_serial_empty_signal_updates_never_reach_provider() {
    let (mut runtime, provider) = opened_owned_runtime();
    provider.fail_next(SerialError {
        name: "NetworkError".into(),
        message: "pending".into(),
    });
    for input in [
        "{}",
        "{break:undefined,dataTerminalReady:undefined,requestToSend:undefined}",
        "null",
    ] {
        runtime.eval(&format!(
            "serialStatus='pending';serialPort.setSignals({input}).then(()=>serialStatus='accepted',e=>serialStatus=e.name);"
        )).unwrap();
        assert_eq!(serial_status(&mut runtime), "TypeError");
        assert_eq!(
            provider.output_signals_for_test("owned"),
            (false, false, false)
        );
    }
    runtime.eval("serialStatus='pending';serialPort.setSignals({requestToSend:true}).then(()=>serialStatus='accepted',e=>serialStatus=e.name);").unwrap();
    assert_eq!(serial_status(&mut runtime), "NetworkError");
}

#[test]
fn web_serial_signal_wire_preserves_presence_and_rejects_null() {
    let absent: SerialOutputSignals = serde_json::from_str("{}").unwrap();
    assert_eq!(absent, SerialOutputSignals::default());
    assert_eq!(
        serde_json::to_value(&absent).unwrap(),
        serde_json::json!({})
    );
    for value in [false, true] {
        let signals: SerialOutputSignals =
            serde_json::from_value(serde_json::json!({"requestToSend": value})).unwrap();
        assert_eq!(signals.request_to_send, Some(value));
        assert_eq!(signals.data_terminal_ready, None);
        assert_eq!(
            serde_json::to_value(&signals).unwrap(),
            serde_json::json!({"requestToSend": value})
        );
    }
    for invalid in [
        r#"{"break":null}"#,
        r#"{"dataTerminalReady":null}"#,
        r#"{"requestToSend":null}"#,
        r#"{"other":true}"#,
    ] {
        assert!(
            serde_json::from_str::<SerialOutputSignals>(invalid).is_err(),
            "{invalid}"
        );
    }
}

#[test]
fn web_serial_signal_dictionary_reads_inherited_members_once_in_order() {
    let (mut runtime, provider) = opened_owned_runtime();
    runtime
        .eval(
            r#"
        serialStatus='pending';
        const order=[];
        const inherited={
            get break(){order.push('break');return true;},
            get dataTerminalReady(){order.push('dtr');return true;},
            get requestToSend(){order.push('rts');return undefined;},
        };
        serialPort.setSignals(Object.create(inherited)).then(
            ()=>serialStatus=order.join(':'),e=>serialStatus=e.name);
    "#,
        )
        .unwrap();
    assert_eq!(serial_status(&mut runtime), "break:dtr:rts");
    assert_eq!(
        provider.output_signals_for_test("owned"),
        (true, false, true)
    );
}

#[test]
fn web_serial_signal_getter_cannot_modify_reopened_connection() {
    let (mut runtime, provider) = opened_owned_runtime();
    runtime
        .eval(
            r#"
        serialStatus='pending';
        (async()=>{
            let reopened;
            const result=await serialPort.setSignals({
                get break(){
                    reopened=serialPort.close().then(()=>serialPort.open({baudRate:9600}));
                    return true;
                },
                requestToSend:true,
            }).then(()=>'accepted',e=>e.name);
            await reopened;
            serialStatus=result;
        })().catch(e=>serialStatus='outer:'+e.name);
    "#,
        )
        .unwrap();
    assert_eq!(serial_status(&mut runtime), "InvalidStateError");
    assert_eq!(
        provider.output_signals_for_test("owned"),
        (false, false, false)
    );
    runtime.eval("serialStatus='pending';serialPort.setSignals({dataTerminalReady:true}).then(()=>serialStatus='ok',e=>serialStatus=e.name);").unwrap();
    assert_eq!(serial_status(&mut runtime), "ok");
    assert_eq!(
        provider.output_signals_for_test("owned"),
        (true, false, false)
    );
}

#[test]
fn web_serial_signal_normalization_does_not_invoke_prototype_setters() {
    let (mut runtime, provider) = opened_owned_runtime();
    runtime.eval(r#"
        serialStatus='pending';
        (async()=>{
            let called=0;
            const names=['break','dataTerminalReady','requestToSend'];
            const descriptors=names.map(name=>Object.getOwnPropertyDescriptor(Object.prototype,name));
            try {
                for(const name of names) Object.defineProperty(Object.prototype,name,{
                    configurable:true,set(){called++;throw new Error('prototype setter');}
                });
                await serialPort.setSignals({requestToSend:true});
                serialStatus=String(called);
            } finally {
                names.forEach((name,index)=>{
                    if(descriptors[index]) Object.defineProperty(Object.prototype,name,descriptors[index]);
                    else delete Object.prototype[name];
                });
            }
        })().catch(e=>serialStatus='outer:'+e.name);
    "#).unwrap();
    assert_eq!(serial_status(&mut runtime), "0");
    assert_eq!(
        provider.output_signals_for_test("owned"),
        (false, true, false)
    );
}
