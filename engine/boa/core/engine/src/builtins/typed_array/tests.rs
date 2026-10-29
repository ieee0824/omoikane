use crate::{TestAction, run_test_actions};

fn register_fill_detach_helper(context: &mut crate::Context) {
    context
        .register_global_builtin_callable(
            crate::js_string!("detachFillTestBuffer"),
            1,
            crate::native_function::NativeFunction::from_fn_ptr(|_, args, _| {
                let object = args[0].as_object().expect("test ArrayBuffer");
                let buffer = crate::object::builtins::JsArrayBuffer::from_object(object.clone())?;
                buffer.detach(&crate::JsValue::undefined())?;
                Ok(crate::JsValue::undefined())
            }),
        )
        .expect("install test-only detach helper");
}

#[test]
fn fill_single_byte_empty_ranges_still_convert_values_and_validate_the_initial_view() {
    run_test_actions([
        TestAction::inspect_context(register_fill_detach_helper),
        TestAction::assert(
            r"(() => {
            for (const value of [Symbol('fill'), 1n]) {
                let threw = false;
                try { new Uint8Array(0).fill(value); }
                catch (error) { threw = error instanceof TypeError; }
                if (!threw) return false;
            }
            let bigintThrew = false;
            try { new BigInt64Array(0).fill(1); }
            catch (error) { bigintThrew = error instanceof TypeError; }
            if (!bigintThrew) return false;
            const buffer = new ArrayBuffer(4);
            const detached = new Uint8Array(buffer);
            detachFillTestBuffer(buffer);
            let calls = 0;
            let threw = false;
            try { detached.fill({valueOf() { calls++; return 1; }}); }
            catch (error) { threw = error instanceof TypeError; }
            if (!threw || calls !== 0) return false;
            const resizable = new ArrayBuffer(8, {maxByteLength: 8});
            const fixed = new Uint8Array(resizable, 4, 4);
            resizable.resize(3);
            threw = false;
            try { fixed.fill({valueOf() { calls++; return 1; }}); }
            catch (error) { threw = error instanceof TypeError; }
            return threw && calls === 0;
        })()",
        ),
    ]);
}

#[test]
fn fill_single_byte_length_tracking_offset_boundary_revalidates_after_coercion() {
    run_test_actions([TestAction::assert(
        r"(() => {
        for (const length of [4, 3]) {
            const buffer = new ArrayBuffer(8, {maxByteLength: 8});
            const tracking = new Uint8Array(buffer, 4);
            const log = [];
            let threw = false;
            try {
                tracking.fill(
                    {valueOf() { log.push('value'); buffer.resize(length); return 7; }},
                    {valueOf() { log.push('start'); return 0; }},
                    {valueOf() { log.push('end'); return 4; }},
                );
            } catch (error) { threw = error instanceof TypeError; }
            if (threw !== (length === 3) || log.join(',') !== 'value,start,end') return false;
            if (new Uint8Array(buffer).some(value => value !== 0)) return false;
        }
        return true;
    })()",
    )]);
}

#[test]
fn fill_single_byte_kinds_preserves_numeric_conversion_and_view_bounds() {
    run_test_actions([TestAction::assert(
        r"(() => {
        const cases = [
            [Int8Array, [
                [-129, 127], [-1, -1], [128, -128], [255, -1],
                [NaN, 0], [Infinity, 0],
            ]],
            [Uint8Array, [
                [-1, 255], [256, 0], [257, 1], [NaN, 0], [Infinity, 0],
            ]],
            [Uint8ClampedArray, [
                [-1, 0], [0.5, 0], [1.5, 2], [2.5, 2], [254.5, 254],
                [255.5, 255], [NaN, 0], [Infinity, 255],
            ]],
        ];
        for (const [TA, conversions] of cases) {
            for (const [input, expected] of conversions) {
                const array = new TA(6);
                array.fill(19);
                if (array.fill(input, 2, -1) !== array) return false;
                if (array[0] !== 19 || array[1] !== 19 || array[2] !== expected ||
                    array[3] !== expected || array[4] !== expected || array[5] !== 19) {
                    return false;
                }
            }
        }

        const buffer = new ArrayBuffer(10);
        const bytes = new Uint8Array(buffer);
        bytes.fill(0xaa);
        const view = new Uint8ClampedArray(buffer, 2, 6);
        view.fill(7, 1, 5);
        if (bytes.join(',') !== '170,170,170,7,7,7,7,170,170,170') return false;

        new Int8Array(buffer, 3, 4).fill(-1);
        return bytes.join(',') === '170,170,170,255,255,255,255,170,170,170';
    })()",
    )]);
}

#[test]
fn fill_single_byte_kinds_preserves_coercion_order_and_empty_ranges() {
    run_test_actions([TestAction::assert(
        r"(() => {
        const log = [];
        const value = {valueOf() { log.push('value'); return 9; }};
        const start = {valueOf() { log.push('start'); return 3; }};
        const end = {valueOf() { log.push('end'); return 1; }};
        const array = new Uint8Array([1, 2, 3, 4]);
        if (array.fill(value, start, end) !== array) return false;
        if (log.join(',') !== 'value,start,end' || array.join(',') !== '1,2,3,4') return false;

        log.length = 0;
        try {
            array.fill({valueOf() { log.push('value'); throw new RangeError('value'); }},
                {valueOf() { log.push('start'); return 0; }},
                {valueOf() { log.push('end'); return 1; }});
            return false;
        } catch (error) {
            if (!(error instanceof RangeError) || log.join(',') !== 'value') return false;
        }

        log.length = 0;
        try {
            array.fill({valueOf() { log.push('value'); return 4; }},
                {valueOf() { log.push('start'); throw new RangeError('start'); }},
                {valueOf() { log.push('end'); return 1; }});
            return false;
        } catch (error) {
            if (!(error instanceof RangeError) || log.join(',') !== 'value,start') return false;
        }
        return true;
    })()",
    )]);
}

#[test]
fn fill_single_byte_kinds_revalidates_resizable_and_detached_buffers() {
    run_test_actions([
        TestAction::inspect_context(register_fill_detach_helper),
        TestAction::assert(
            r"(() => {
        {
            const buffer = new ArrayBuffer(4, {maxByteLength: 8});
            const array = new Uint8Array(buffer);
            array.fill({valueOf() { buffer.resize(8); return 9; }});
            if (new Uint8Array(buffer).join(',') !== '9,9,9,9,0,0,0,0') return false;
        }
        {
            const buffer = new ArrayBuffer(8, {maxByteLength: 8});
            const array = new Uint8Array(buffer);
            array.set([1, 2, 3, 4, 5, 6, 7, 8]);
            array.fill(9, {valueOf() { buffer.resize(4); return 1; }}, 8);
            if (array.length !== 4 || array.join(',') !== '1,9,9,9') return false;
        }
        {
            const buffer = new ArrayBuffer(8, {maxByteLength: 8});
            const array = new Uint8Array(buffer);
            array.fill(5);
            let valueCalls = 0;
            if (array.fill(
                {valueOf() { valueCalls++; return 7; }},
                {valueOf() { buffer.resize(4); return 6; }},
                8,
            ) !== array) return false;
            if (valueCalls !== 1 || array.length !== 4 || array.join(',') !== '5,5,5,5') {
                return false;
            }
        }
        {
            const log = [];
            const buffer = new ArrayBuffer(8, {maxByteLength: 8});
            const fixed = new Uint8Array(buffer, 4, 4);
            let threw = false;
            try {
                fixed.fill(
                    {valueOf() { log.push('value'); buffer.resize(4); return 1; }},
                    {valueOf() { log.push('start'); return 0; }},
                    {valueOf() { log.push('end'); return 4; }},
                );
            } catch (error) { threw = error instanceof TypeError; }
            if (!threw || log.join(',') !== 'value,start,end') return false;
        }
        {
            const log = [];
            const buffer = new ArrayBuffer(8);
            const array = new Uint8ClampedArray(buffer);
            let threw = false;
            try {
                array.fill(
                    {valueOf() { log.push('value'); detachFillTestBuffer(buffer); return 1; }},
                    {valueOf() { log.push('start'); return 3; }},
                    {valueOf() { log.push('end'); return 1; }},
                );
            } catch (error) { threw = error instanceof TypeError; }
            if (!threw || log.join(',') !== 'value,start,end') return false;
        }
        return true;
    })()",
        ),
    ]);
}

#[test]
fn fill_other_kinds_and_shared_buffers_keep_existing_semantics() {
    run_test_actions([TestAction::assert(
        r"(() => {
        const numeric = [
            [Int16Array, -0x1234], [Uint16Array, 0xfedc],
            [Int32Array, -0x1234567], [Uint32Array, 0xfedcba98],
            [Float32Array, 1.25], [Float64Array, -123.5],
        ];
        for (const [TA, value] of numeric) {
            const array = new TA(4);
            array.fill(value, 1, 3);
            if (array[0] !== 0 || !Object.is(array[1], value) ||
                !Object.is(array[2], value) || array[3] !== 0) return false;
        }
        for (const [TA, value] of [[BigInt64Array, -5n], [BigUint64Array, 5n]]) {
            const array = new TA(4);
            array.fill(value, 1, 3);
            if (array[0] !== 0n || array[1] !== value || array[2] !== value ||
                array[3] !== 0n) return false;
        }

        if (typeof SharedArrayBuffer === 'function') {
            const buffer = new SharedArrayBuffer(8);
            const bytes = new Uint8Array(buffer);
            bytes.fill(0xaa);
            new Int8Array(buffer, 2, 4).fill(-1, 1, 3);
            if (bytes.join(',') !== '170,170,170,255,255,170,170,170') return false;
        }
        return true;
    })()",
    )]);
}

#[test]
#[ignore = "manual baseline/candidate timing fixture for canvas-sized byte fills"]
fn fill_single_byte_large_canvas_fixture() {
    let started = std::time::Instant::now();
    run_test_actions([TestAction::assert(
        r"(() => {
        const pixels = new Uint8ClampedArray(1920 * 1080 * 4);
        pixels.fill(255);
        for (let pass = 0; pass < 4; pass++) pixels.fill(0);
        return pixels[0] === 0 && pixels[pixels.length >>> 1] === 0 &&
            pixels[pixels.length - 1] === 0;
    })()",
    )]);
    eprintln!(
        "fill_single_byte_large_canvas_fixture elapsed_ms={:.3}",
        started.elapsed().as_secs_f64() * 1000.0
    );
}

#[test]
fn resizable_buffer_filter_propagates_bigint_conversion_errors() {
    // `ArrayBuffer.prototype.transfer` requires the `experimental` feature, so
    // detach through the test helper to cover default features too.
    run_test_actions([
        TestAction::inspect_context(register_fill_detach_helper),
        TestAction::assert(
            r"(() => {
        for (const TA of [BigInt64Array, BigUint64Array]) {
            for (const fixed of [false, true]) {
                for (const detach of [false, true]) {
                    const buffer = new ArrayBuffer(24, {maxByteLength:24});
                    const array = fixed ? new TA(buffer, 0, 3) : new TA(buffer);
                    array.set([1n, 2n, 3n]);
                    let calls = 0;
                    let threw = false;
                    try {
                        array.filter((value, index, receiver) => {
                            if (index !== calls || receiver !== array) throw Error('callback');
                            if (calls === 0) {
                                if (value !== 1n) throw Error('initial value');
                                if (detach) detachFillTestBuffer(buffer); else buffer.resize(8);
                            } else if (value !== undefined) throw Error('missing value');
                            calls++;
                            return true;
                        });
                    } catch (error) { threw = error instanceof TypeError; }
                    if (!threw || calls !== 3) return false;
                }
            }
        }
        return true;
    })()",
        ),
    ]);
}

#[test]
fn resizable_buffer_with_propagates_bigint_conversion_errors() {
    run_test_actions([TestAction::assert(
        r"(() => {
        for (const TA of [BigInt64Array, BigUint64Array]) {
            for (const shrinkIndex of [false, true]) {
                const buffer = new ArrayBuffer(24, {maxByteLength:24});
                const array = new TA(buffer);
                array.set([1n, 2n, 3n]);
                let calls = 0;
                const argument = {valueOf() {
                    calls++;
                    buffer.resize(8);
                    return shrinkIndex ? 0 : 9n;
                }};
                let threw = false;
                try { array.with(shrinkIndex ? argument : 0, shrinkIndex ? 9n : argument); }
                catch (error) { threw = error instanceof TypeError; }
                if (!threw || calls !== 1 || array.length !== 1 || array[0] !== 1n) return false;
            }
        }
        return true;
    })()",
    )]);
}

#[test]
fn resizable_buffer_slice_zero_copy_preserves_allocated_result() {
    run_test_actions([TestAction::assert(
        r"(() => {
        const constructors = [Int8Array, Uint8Array, Uint8ClampedArray, Int16Array,
            Uint16Array, Int32Array, Uint32Array, Float32Array, Float64Array];
        if (typeof Float16Array === 'function') constructors.push(Float16Array);
        for (const Source of constructors) {
            for (const Target of constructors) {
                for (let index = 0; index <= 4; index++) {
                    const bytes = 4 * Source.BYTES_PER_ELEMENT;
                    const buffer = new ArrayBuffer(bytes, {maxByteLength:bytes});
                    const array = new Source(buffer);
                    array.fill(1);
                    array.constructor = Target;
                    const result = array.slice({valueOf() { buffer.resize(0); return index; }});
                    if (!(result instanceof Target) || result.length !== 4 - index ||
                        !result.every(value => value === 0)) return false;
                }
            }
        }
        return true;
    })()",
    )]);
}
