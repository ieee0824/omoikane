// Behavioral contracts for non-DOM child APIs retained by Issue #1689.
(() => {
    const require = (condition, name) => { if (!condition) throw 'worker API probe failed: ' + name; };
    const eventTarget = new EventTarget();
    let calls = 0;
    eventTarget.addEventListener('probe', () => calls++, {once:true});
    eventTarget.dispatchEvent(new Event('probe'));
    eventTarget.dispatchEvent(new Event('probe'));
    require(calls === 1, 'EventTarget once');
    for (const method of ['focus', 'blur']) {
        require(globalThis[method]() === undefined, method + ' result');
        require(globalThis[method]() === undefined, method + ' repeated result');
        let invalidReceiver = false;
        try { globalThis[method].call({}); }
        catch (error) { invalidReceiver = error.name === 'TypeError'; }
        require(invalidReceiver, method + ' invalid receiver');
    }
    const brandContext = new OffscreenCanvas(1, 1).getContext('2d');
    for (const name of ['HTMLImageElement', 'SVGImageElement', 'HTMLVideoElement']) {
        const previous = Object.getOwnPropertyDescriptor(globalThis, name);
        let reads = 0, calls = 0;
        const forged = {[Symbol.hasInstance]: () => { calls++; return true; }};
        Object.defineProperty(globalThis, name, {
            configurable: true, get() { reads++; return forged; },
        });
        try {
            for (const operation of [
                () => brandContext.drawImage({}, 0, 0),
                () => brandContext.createPattern({}, 'repeat'),
            ]) {
                let rejected = false;
                try { operation(); } catch (error) { rejected = error.name === 'TypeError'; }
                require(rejected && reads === 0 && calls === 0, name + ' captured brand');
            }
        } finally {
            if (previous) Object.defineProperty(globalThis, name, previous);
            else delete globalThis[name];
        }
    }
    const weakMapGet = WeakMap.prototype.get;
    let adoptedMapReads = 0;
    WeakMap.prototype.get = function() {
        adoptedMapReads++;
        throw 'private adopted stylesheet map reached an author hook';
    };
    try {
        CSSStyleSheet.prototype.__syncAdoptedRoots.call({});
    } finally {
        WeakMap.prototype.get = weakMapGet;
    }
    require(adoptedMapReads === 0, 'adopted stylesheet captured map access');
    const form = new FormData();
    form.append('word', 'value');
    require(form.get('word') === 'value', 'FormData');
    const cycle = {}; cycle.self = cycle;
    const clone = structuredClone(cycle);
    require(clone !== cycle && clone.self === clone, 'structuredClone cycle');
    const canvas = new OffscreenCanvas(2, 2);
    const context = canvas.getContext('2d');
    context.fillStyle = '#ff0000';
    context.fillRect(0, 0, 2, 2);
    const pixel = context.getImageData(0, 0, 1, 1).data;
    require(pixel[0] === 255 && pixel[3] === 255, 'OffscreenCanvas pixels');
    localStorage.setItem('worker-probe', 'local');
    sessionStorage.setItem('worker-probe', 'session');
    require(localStorage.getItem('worker-probe') === 'local', 'localStorage');
    require(sessionStorage.getItem('worker-probe') === 'session', 'sessionStorage');
    const face = new FontFace('WorkerProbe', 'url(/font.woff)');
    require(face.family === 'WorkerProbe', 'FontFace');
    require(!('createElement' in localStorage.__document), 'Storage must not expose a DOM document');
    // Retained APIs must reject invalid non-DOM inputs with their original
    // exception type, even though DOM interface globals are absent.
    const rejectsTypeError = (name, call) => {
        let error;
        try { call(); } catch (caught) { error = caught; }
        require(error instanceof TypeError, name + ' invalid input must throw TypeError');
    };
    for (const value of [{}, {__id: 1, nodeType: 1}, new Proxy({}, {})]) {
        rejectsTypeError('FormData submitter', () => new FormData(undefined, value));
        rejectsTypeError('CommandEvent', () => new CommandEvent('probe', {source:value}));
        rejectsTypeError('KeyframeEffect', () => new KeyframeEffect(value, []));
        rejectsTypeError('ScrollTimeline', () => new ScrollTimeline({source:value}));
        rejectsTypeError('ViewTimeline', () => new ViewTimeline({subject:value}));
    }
    rejectsTypeError('Canvas image', () => context.drawImage({}, 0, 0));
    const copy = new OffscreenCanvas(2, 2).getContext('2d');
    copy.drawImage(canvas, 0, 0);
    require(copy.getImageData(0, 0, 1, 1).data[0] === 255, 'OffscreenCanvas image copy');
    require(__omoikane_webgl_lose_context({}) === false, 'WebGL invalid loss target');
    require(__omoikane_webgl_restore_context({}) === false, 'WebGL invalid restore target');
    return true;
})();
