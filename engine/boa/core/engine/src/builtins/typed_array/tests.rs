use crate::{TestAction, run_test_actions};

#[test]
fn resizable_buffer_filter_propagates_bigint_conversion_errors() {
    run_test_actions([TestAction::assert(
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
                                if (detach) buffer.transfer(); else buffer.resize(8);
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
    )]);
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
