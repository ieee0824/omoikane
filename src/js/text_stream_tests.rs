use super::JsRuntime;
use boa_engine::{JsValue, NativeFunction, js_string, object::builtins::JsArrayBuffer};

fn check(source: &str) {
    let mut runtime = JsRuntime::new().unwrap();
    // Detach through the engine API so these tests do not depend on the
    // separately tracked MessagePort transfer implementation (#763).
    runtime
        .context
        .register_global_callable(
            js_string!("detachTestBuffer"),
            1,
            NativeFunction::from_copy_closure(|_, args, _| {
                JsArrayBuffer::from_object(args[0].as_object().unwrap())?
                    .detach(&JsValue::undefined())?;
                Ok(JsValue::undefined())
            }),
        )
        .unwrap();
    runtime
        .eval(
            r#"
            globalThis.result = 'pending';
            function assert(value, message) { if (!value) throw new Error(message); }
            async function collect(stream, chunks) {
              const reader = stream.readable.getReader();
              const reading = (async () => {
                const output = [];
                for (;;) {
                  const next = await reader.read();
                  if (next.done) return output;
                  output.push(next.value);
                }
              })();
              const writer = stream.writable.getWriter();
              for (const chunk of chunks) await writer.write(chunk);
              await writer.close();
              return reading;
            }
            "#,
        )
        .unwrap();
    runtime
        .eval(&format!(
            "(async () => {{ {source} }})().then(() => result = 'ok', error => result = String(error));"
        ))
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("result")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "ok"
    );
}

#[test]
fn text_stream_split_characters_and_flush() {
    check(
        r#"
        const encoded = await collect(new TextEncoderStream(), ['A\uD83D', '', '\uDE00', '\uD800']);
        assert(JSON.stringify(encoded.map(x => [...x])) === '[[65],[240,159,152,128],[239,191,189]]', 'surrogates');
        const decoded = await collect(new TextDecoderStream(), [
          new Uint8Array([0xef]), new Uint8Array([0xbb,0xbf,65,0xf0]),
          new Uint8Array([0x9f]), new Uint8Array([0x98,0x80,0xe2])
        ]);
        assert(JSON.stringify(decoded) === JSON.stringify(['A', '😀', '�']), 'split UTF-8 / flush');
        "#,
    );
}
#[test]
fn text_stream_encoding_labels_options_and_bom() {
    check(
        r#"
      for (const [label, bytes, expected] of [
        [' \tASCII\r\n', [0x80], '€'], ['Shift_JIS', [0x90,0x85], '水'],
        ['utf-16', [0xff,0xfe,0x34,0x6c], '水'], ['UTF-16BE', [0xfe,0xff,0x6c,0x34], '水'],
        ['x-user-defined', [0x80,0xff], '\uF780\uF7FF'],
        ['ISO-2022-JP', [0x1b,0x24,0x42,0x3f,0x65,0x1b,0x28,0x42,65], '水A']
      ]) {
        const output = await collect(new TextDecoderStream(label), bytes.map(x => new Uint8Array([x])));
        assert(output.join('') === expected, 'label ' + label + ': ' + output);
      }
      for (const label of ['', 'replacement', 'iso-2022-cn', 'utf-7', '\u00a0utf-8', 'utf-8\v']) {
        let rejected = false;
        try { new TextDecoderStream(label); } catch(e) { rejected = e instanceof RangeError; }
        assert(rejected, 'invalid label ' + label);
      }
      for (const label of ['utf-8', 'utf-16le', 'utf-16be']) {
        const bom = label === 'utf-8' ? [239,187,191] : label === 'utf-16le' ? [255,254] : [254,255];
        for (const ignoreBOM of [false, true]) {
          const stream = new TextDecoderStream(label, { ignoreBOM, fatal: true });
          assert(stream.encoding === label && stream.ignoreBOM === ignoreBOM && stream.fatal, 'attributes');
          const output = await collect(stream, [...bom,...bom].map(x => new Uint8Array([x])));
          assert(output.join('') === (ignoreBOM ? '\uFEFF\uFEFF' : '\uFEFF'), 'BOM only once');
        }
      }
      const legacy = await collect(new TextDecoderStream('windows-1252'), [new Uint8Array([239,187,191])]);
      assert(legacy.join('') === 'ï»¿', 'do not sniff another encoding');
      new TextDecoderStream(undefined, null);
      for (const options of [true, 1, 'x', Symbol('x')]) {
        let rejected = false;
        try { new TextDecoderStream('utf-8', options); } catch(e) { rejected = e instanceof TypeError; }
        assert(rejected, 'non-dictionary options');
      }
      const order = [];
      new TextDecoderStream({toString() { order.push('label'); return 'utf8'; }}, {
        get fatal() { order.push('fatal'); return 0; },
        get ignoreBOM() { order.push('ignoreBOM'); return 1; }
      });
      assert(order.join() === 'label,fatal,ignoreBOM', 'Web IDL order');
    "#,
    );
}

#[test]
fn text_stream_fatal_and_conversion_errors_reach_both_sides() {
    check(
        r#"
      const rejects = async (promise, expected) => {
        try { await promise; return false; } catch(e) {
          return expected === TypeError ? e instanceof TypeError : e === expected;
        }
      };
      for (const chunks of [[new Uint8Array([65,0xff])], [new Uint8Array([0xf0,0x9f])]]) {
        const s = new TextDecoderStream('utf8', {fatal:true});
        const reader = s.readable.getReader(), writer = s.writable.getWriter();
        const reading = rejects(reader.read(), TypeError);
        const rc = rejects(reader.closed, TypeError), wc = rejects(writer.closed, TypeError);
        const writing = rejects((async () => { for (const c of chunks) await writer.write(c); await writer.close(); })(), TypeError);
        assert(await reading && await writing && await rc && await wc, 'fatal read/write/closed');
      }
      const marker = new Error('coercion');
      for (const [stream, chunk, error] of [
        [new TextEncoderStream(), Symbol(), TypeError],
        [new TextEncoderStream(), {toString() { throw marker; }}, marker],
        [new TextDecoderStream(), [65], TypeError],
        [new TextDecoderStream(), undefined, TypeError],
        [new TextDecoderStream(), new ArrayBuffer(1, {maxByteLength:4}), TypeError],
        [new TextDecoderStream(), new Uint8Array(new ArrayBuffer(1, {maxByteLength:4})), TypeError],
        [new TextDecoderStream(), new SharedArrayBuffer(1, {maxByteLength:4}), TypeError],
        [new TextDecoderStream(), new DataView(new SharedArrayBuffer(1, {maxByteLength:4})), TypeError]
      ]) {
        const reader = stream.readable.getReader(), writer = stream.writable.getWriter();
        const promises = [rejects(reader.read(), error), rejects(reader.closed, error),
          rejects(writer.closed, error), rejects(writer.write(chunk), error)];
        assert((await Promise.all(promises)).every(Boolean), 'conversion failure propagation');
      }
    "#,
    );
}

#[test]
fn text_stream_cancel_abort_and_demand() {
    check(
        r#"
      const rejectedWith = async (promise, reason) => {
        try { await promise; return false; } catch(e) { return e === reason; }
      };
      for (const Constructor of [TextEncoderStream, TextDecoderStream]) {
        for (const reason of [undefined, new Error('stop')]) {
          const s = new Constructor(), writer = s.writable.getWriter();
          const closed = rejectedWith(writer.closed, reason);
          const input = Constructor === TextEncoderStream ? 'A' : new Uint8Array([65]);
          const pending = rejectedWith(writer.write(input), reason);
          await Promise.resolve(); await Promise.resolve();
          await s.readable.cancel(reason);
          assert(await closed && await pending && await rejectedWith(writer.write(input), reason), 'cancel reason / queued write');
          const a = new Constructor(), reader = a.readable.getReader(), aw = a.writable.getWriter();
          const promises = [rejectedWith(reader.read(), reason), rejectedWith(reader.closed, reason), rejectedWith(aw.closed, reason)];
          await aw.abort(reason);
          assert((await Promise.all(promises)).every(Boolean), 'abort reason');
        }
      }
      let converted = false;
      const stream = new TextEncoderStream(), writer = stream.writable.getWriter();
      const write = writer.write({toString() { converted = true; return 'A'; }});
      for (let i = 0; i < 8; ++i) await Promise.resolve();
      assert(!converted, 'conversion must wait for read demand');
      const reader = stream.readable.getReader();
      assert((await reader.read()).value[0] === 65, 'read');
      await write;
      await writer.close();
      assert((await reader.read()).done, 'close');
      const flushed = new TextEncoderStream(), fw = flushed.writable.getWriter(), fr = flushed.readable.getReader();
      const first = fr.read();
      await fw.write('A\uD800');
      assert((await first).value[0] === 65, 'first flush-test chunk');
      let closed = false;
      fr.closed.then(() => closed = true);
      await fw.close();
      assert(!closed, 'readable.closed must wait for queued flush output');
      assert(JSON.stringify([...(await fr.read()).value]) === '[239,191,189]', 'flush queued replacement');
      await fr.closed;
      assert((await fr.read()).done, 'flush final close');
    "#,
    );
}

#[test]
fn text_stream_buffer_sources_and_large_chunks() {
    check(
        r#"
      for (const Constructor of [ArrayBuffer, SharedArrayBuffer]) {
        const buffer = new Constructor(6);
        new Uint8Array(buffer).set([0,65,66,67,68,0]);
        for (const view of [new DataView(buffer,1,4), new Uint16Array(buffer,2,1), buffer]) {
          const out = await collect(new TextDecoderStream(), [view]);
          const expected = view instanceof DataView ? 'ABCD' : view instanceof Uint16Array ? 'BC' : '\x00ABCD\x00';
          assert(out.join('') === expected, 'view byte offset / shared');
        }
      }
      const guarded = new Uint8Array([65]);
      Object.defineProperty(guarded, 'buffer', {get() { throw new Error('public buffer getter'); }});
      Object.defineProperty(guarded, 'byteLength', {get() { throw new Error('public length getter'); }});
      assert((await collect(new TextDecoderStream(), [guarded])).join('') === 'A', 'read internal slots');
      const buffer = new ArrayBuffer(4), view = new Uint8Array(buffer), dataView = new DataView(buffer);
      detachTestBuffer(buffer);
      assert(buffer.byteLength === 0, 'test input is detached');
      assert((await collect(new TextDecoderStream(), [buffer,view,dataView])).length === 0, 'detached input');
      const text = '水😀'.repeat(12000);
      const output = await collect(new TextEncoderStream(), [text]);
      const decoded = await collect(new TextDecoderStream('utf8', {fatal:true}), output);
      assert(decoded.join('') === text, 'large output / native buffer rollover');
    "#,
    );
}

#[test]
fn text_stream_attributes_brand_and_worker() {
    check(
        r#"
      for (const Constructor of [TextEncoderStream, TextDecoderStream]) {
        const stream = new Constructor();
        assert(Object.prototype.toString.call(stream) === '[object ' + Constructor.name + ']', 'toStringTag');
        assert(stream.readable instanceof ReadableStream && stream.writable instanceof WritableStream, 'stream types');
        for (const attr of Object.getOwnPropertyNames(Constructor.prototype).filter(x => x !== 'constructor')) {
          const d = Object.getOwnPropertyDescriptor(Constructor.prototype, attr);
          assert(d.enumerable && d.configurable && d.set === undefined, 'readonly attributes');
          let threw = false;
          try { d.get.call({}); } catch(e) { threw = e instanceof TypeError; }
          assert(threw, 'brand check');
        }
      }
      assert(typeof __omoikane_text_encoder_encode === 'undefined' && typeof __omoikane_text_decoder_create === 'undefined' && typeof __omoikane_text_decoder_decode === 'undefined', 'private hooks');
      const source = `
        (async () => {
          const encoder = new TextEncoderStream();
          const reader = encoder.readable.pipeThrough(new TextDecoderStream()).getReader();
          const writer = encoder.writable.getWriter();
          const reading = reader.read();
          await writer.write('水');
          await writer.close();
          postMessage((await reading).value);
        })().catch(e => postMessage(String(e)));
      `;
      const worker = new Worker('data:text/javascript,' + encodeURIComponent(source));
      const message = await new Promise((resolve, reject) => { worker.onmessage = e => resolve(e.data); worker.onerror = reject; });
      assert(message === '水', 'worker roundtrip: ' + message);
      worker.terminate();
    "#,
    );
}

#[test]
fn text_stream_decoder_state_survives_collection_between_chunks() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            r#"
            globalThis.decodingPhase = 'pending';
            globalThis.decodedText = '';
            const stream = new TextDecoderStream();
            const reader = stream.readable.getReader();
            const writer = stream.writable.getWriter();
            reader.read().then(result => decodedText = result.value);
            writer.write(new Uint8Array([0xf0,0x9f])).then(() => decodingPhase = 'buffered');
            "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("decodingPhase === 'buffered'")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
    boa_gc::force_collect();
    runtime
        .eval(
            "writer.write(new Uint8Array([0x98,0x80])).then(() => writer.close()).then(() => decodingPhase = 'closed');",
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("decodedText === '😀' && decodingPhase === 'closed'")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn stream_consumers_finish_after_discarding_queued_data() {
    check(
        r#"
        const response = new Response('consumed');
        await response.text();
        const bodyReader = response.body.getReader();
        assert((await bodyReader.read()).done, 'already consumed body is closed');
        await bodyReader.closed;

        const [left, right] = WebTransport.createPair();
        await Promise.all([left.ready, right.ready]);
        await left.datagrams.writable.getWriter().write(new Uint8Array([65]));
        await right.datagrams.writable.getWriter().write(new Uint8Array([66]));
        const readers = [left.datagrams.readable.getReader(), right.datagrams.readable.getReader()];
        left.close();
        for (const reader of readers) {
          await reader.closed;
          assert((await reader.read()).done, 'transport closes after discarding queued datagrams');
        }
        "#,
    );
}

#[test]
fn text_stream_internal_operations_do_not_call_page_replacements() {
    check(
        r#"
        const iterator = Array.prototype[Symbol.iterator];
        let encoder, decoder;
        try {
          Array.prototype[Symbol.iterator] = function() { throw new Error('page iterator'); };
          encoder = new TextEncoderStream();
          decoder = new TextDecoderStream();
        } finally { Array.prototype[Symbol.iterator] = iterator; }
        const charCodeAt = String.prototype.charCodeAt, slice = String.prototype.slice;
        const Encoder = TextEncoder, Decoder = TextDecoder, Bytes = Uint8Array;
        try {
          String.prototype.charCodeAt = String.prototype.slice = () => { throw new Error('page string method'); };
          globalThis.TextEncoder = globalThis.TextDecoder = globalThis.Uint8Array = () => { throw new Error('page constructor'); };
          const bytes = await collect(encoder, ['\uD83D', '\uDE00x\uD800']);
          const output = await collect(decoder, bytes);
          assert(output.join('') === '😀x�', 'intrinsic operations');
        } finally {
          String.prototype.charCodeAt = charCodeAt;
          String.prototype.slice = slice;
          globalThis.TextEncoder = Encoder;
          globalThis.TextDecoder = Decoder;
          globalThis.Uint8Array = Bytes;
        }
        "#,
    );
}
