// RoundTiesToEven expectations from binary16 spacing, independent of Math.f16round.
// https://tc39.es/ecma262/#sec-math.f16round
(() => {
    function same(actual, expected, label) {
        if (!Object.is(actual, expected)) {
            throw new Error(label + ': expected ' + expected + ', got ' + actual);
        }
    }
    const cases = [
        [0, 0], [-0, -0], [Infinity, Infinity], [-Infinity, -Infinity], [NaN, NaN],
        [2.980232238769531e-8, 0],
        [2.9802322387695312e-8, 0], // 2^-25: tie at zero/minimum subnormal
        [2.980232238769532e-8, 5.960464477539063e-8],
        [1.4901161193847654e-7, 1.1920928955078125e-7],
        [1.4901161193847656e-7, 1.1920928955078125e-7],
        [1.490116119384766e-7, 1.7881393432617188e-7],
        [1.0004882812499998, 1],
        [1.00048828125, 1],
        [1.0004882812500002, 1.0009765625],
        [1.00146484375, 1.001953125], // tie with odd lower mantissa
        [65519.99999999999, 65504], [65520, Infinity],
    ];
    for (const [positive, rounded] of cases) {
        for (const sign of [1, -1]) {
            const input = positive * sign;
            const expected = rounded * sign;
            same(Math.f16round(input), expected, 'Math.f16round');
            const view = new DataView(new ArrayBuffer(2));
            for (const littleEndian of [false, true]) {
                same(view.setFloat16(0, input, littleEndian), undefined, 'DataView return');
                same(view.getFloat16(0, littleEndian), expected, 'DataView roundtrip');
            }
            same(new Float16Array([input])[0], expected, 'array constructor');
            same(new Float16Array(new Float64Array([input]))[0], expected, 'typed constructor');
            const array = new Float16Array(1);
            array[0] = input;
            same(array[0], expected, 'indexed assignment');
            Object.defineProperty(array, '0', {value: input});
            same(array[0], expected, 'DefineOwnProperty');
            array.fill(input);
            same(array[0], expected, 'fill');
            same(array.map(() => input)[0], expected, 'map');
            array.set([input]);
            same(array[0], expected, 'set array');
            array.set(new Float64Array([input]));
            same(array[0], expected, 'set Float64Array');
        }
    }
    // Value coercion occurs once; DataView converts offset before value.
    const events = [];
    const value = {[Symbol.toPrimitive](hint) {
        events.push('value:' + hint);
        return 1.0004882812500002;
    }};
    const offset = {valueOf() { events.push('offset'); return 0; }};
    const view = new DataView(new ArrayBuffer(2));
    same(view.setFloat16(offset, value, true), undefined, 'coerced DataView return');
    same(events.join(','), 'offset,value:number', 'DataView coercion order');
    same(view.getUint16(0, true), 0x3c01, 'DataView exact representation');
    events.length = 0;
    same(Math.f16round(value), 1.0009765625, 'coerced Math.f16round');
    same(events.join(','), 'value:number', 'Math coercion count');
    events.length = 0;
    same(new Float16Array([value])[0], 1.0009765625, 'coerced array');
    same(events.join(','), 'value:number', 'array coercion count');

    // Abrupt completion is preserved; incompatible numeric types are not accepted.
    const marker = {};
    for (const convert of [
        x => Math.f16round(x),
        x => view.setFloat16(0, x),
        x => new Float16Array([x]),
    ]) {
        let caught;
        try { convert({valueOf() { throw marker; }}); } catch (error) { caught = error; }
        same(caught, marker, 'coercion exception');
        for (const invalid of [1n, Symbol('invalid')]) {
            caught = undefined;
            try { convert(invalid); } catch (error) { caught = error; }
            same(caught instanceof TypeError, true, 'ToNumber TypeError');
        }
    }
    return true;
})()
