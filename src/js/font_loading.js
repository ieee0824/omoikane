// Font Loading API state belongs to each Window realm. Font bytes and renderer
// registrations stay in the host; promises and event listeners stay in JS.
(() => {
  const native = globalThis.__omoikane_font_loading;
  const task = globalThis.__omoikane_queue_font_loading_task;
  delete globalThis.__omoikane_font_loading;
  delete globalThis.__omoikane_queue_font_loading_task;
  const [faces, sets, documents] = globalThis.__omoikane_font_maps;
  delete globalThis.__omoikane_font_maps;
  const defaults = {
    family: '', style: 'normal', weight: 'normal', stretch: 'normal',
    unicodeRange: 'U+0-10FFFF', variant: 'normal', featureSettings: 'normal',
    variationSettings: 'normal', display: 'auto', ascentOverride: 'normal',
    descentOverride: 'normal', lineGapOverride: 'normal', sizeAdjust: '100%'
  };
  const descriptorProperties = {
    family: 'font-family', style: 'font-style', weight: 'font-weight',
    stretch: 'font-stretch', unicodeRange: 'unicode-range', variant: 'font-variant',
    featureSettings: 'font-feature-settings', variationSettings: 'font-variation-settings',
    display: 'font-display', ascentOverride: 'ascent-override',
    descentOverride: 'descent-override', lineGapOverride: 'line-gap-override',
    sizeAdjust: 'size-adjust'
  };
  const propertyDescriptors = Object.fromEntries(
    Object.entries(descriptorProperties).map(([name, property]) => [property, name])
  );
  const cssRuleFaces = new WeakMap();
  const exception = (name, message) => new DOMException(message, name);
  const syntax = () => exception('SyntaxError', 'Invalid font descriptor or source');
  const faceState = face => {
    const state = faces.get(face);
    if (!state) throw new TypeError('Expected a FontFace');
    return state;
  };
  const setState = set => {
    const state = sets.get(set);
    if (!state) throw new TypeError('Expected a FontFaceSet');
    return state;
  };
  function splitList(text) {
    const result = [];
    let start = 0, quote = '', depth = 0, escaped = false;
    for (let i = 0; i < text.length; i++) {
      const char = text[i];
      if (escaped) { escaped = false; continue; }
      if (char === '\\') { escaped = true; continue; }
      if (quote) { if (char === quote) quote = ''; continue; }
      if (char === '"' || char === "'") { quote = char; continue; }
      if (char === '(') depth++;
      else if (char === ')') { if (--depth < 0) throw syntax(); }
      else if (char === ',' && depth === 0) { result.push(text.slice(start, i).trim()); start = i + 1; }
    }
    if (quote || depth || escaped) throw syntax();
    result.push(text.slice(start).trim());
    if (result.some(part => !part)) throw syntax();
    return result;
  }
  function unquote(text) {
    if (text[0] === '"' || text[0] === "'") {
      if (text[text.length - 1] !== text[0]) throw syntax();
      text = text.slice(1, -1);
    }
    return text.replace(/\\([0-9a-f]{1,6})\s?|\\(.)/gi, (_, hex, char) =>
      hex ? String.fromCodePoint(parseInt(hex, 16) || 0xfffd) : char);
  }
  function ranges(value) {
    return splitList(value).map(part => {
      const match = /^U\+([0-9a-f?]{1,6})(?:-([0-9a-f]{1,6}))?$/i.exec(part);
      if (!match || (match[2] && match[1].includes('?')) || !/^[0-9a-f]*\?*$/i.test(match[1])) throw syntax();
      const first = parseInt(match[1].replace(/\?/g, '0'), 16);
      const last = parseInt(match[2] || match[1].replace(/\?/g, 'F'), 16);
      if (first > last || last > 0x10ffff) throw syntax();
      return [first, last];
    });
  }
  function descriptor(name, input) {
    const value = String(input).trim();
    let valid = false;
    if (name === 'family') {
      valid = value.length > 0 && splitList(value).length === 1 &&
        (/^(['"])[\s\S]*\1$/.test(value) || /^[-_a-z\u0080-\uffff][-_a-z0-9\u0080-\uffff]*(?:\s+[-_a-z\u0080-\uffff][-_a-z0-9\u0080-\uffff]*)*$/i.test(value));
      if (/^(inherit|initial|unset|revert|revert-layer)$/i.test(value)) valid = false;
    } else if (name === 'style') valid = /^(normal|italic|oblique(?:\s+-?(?:\d*\.)?\d+deg){0,2})$/.test(value);
    else if (name === 'weight') valid = /^(normal|bold)$/.test(value) || (/^\d+(?:\s+\d+)?$/.test(value) && value.split(/\s+/).every(v => +v >= 1 && +v <= 1000));
    else if (name === 'stretch') valid = /^(normal|(?:ultra-|extra-|semi-)?(?:condensed|expanded))$/.test(value) || (/^\d+(?:\.\d+)?%(?:\s+\d+(?:\.\d+)?%)?$/.test(value) && value.split(/\s+/).every(v => parseFloat(v) > 0));
    else if (name === 'unicodeRange') { ranges(value); valid = true; }
    else if (name === 'display') valid = /^(auto|block|swap|fallback|optional)$/.test(value);
    else if (name.endsWith('Override') || name === 'sizeAdjust') valid = (name !== 'sizeAdjust' && value === 'normal') || /^(?:\d*\.)?\d+%$/.test(value);
    else if (name === 'variant') valid = /^(normal|small-caps)$/.test(value);
    else if (name === 'featureSettings' || name === 'variationSettings') valid = value === 'normal' || splitList(value).every(v => /^(['"])[\x20-\x7e]{4}\1(?:\s+(?:on|off|-?(?:\d*\.)?\d+))?$/.test(v));
    if (!valid) throw syntax();
    return value;
  }
  function sources(value) {
    try { return JSON.parse(native('parse-source', JSON.stringify({input: value}))); }
    catch (_) { throw syntax(); }
  }
  function host(face, operation, binding = native) {
    const state = faceState(face);
    return binding(operation, JSON.stringify({ id: state.id, ...state.descriptors,
      family: unquote(state.descriptors.family) }));
  }
  function beginSet(set, face) {
    const state = setState(set);
    if (state.pending.has(face)) return;
    state.pending.add(face);
    if (state.status === 'loaded') {
      state.status = 'loading';
      state.loaded = []; state.failed = [];
      state.ready = new Promise(resolve => { state.resolve = resolve; });
      state.queue(() => set.dispatchEvent(state.event('loading')));
    }
  }
  function finishSet(set) {
    const state = setState(set);
    if (state.pending.size || state.status !== 'loading' || state.finishing) return;
    state.finishing = true;
    state.queue(() => {
      state.finishing = false;
      if (state.pending.size || state.status !== 'loading') return;
      loadUsedFonts(set);
      if (state.pending.size) return;
      if (state.document && state.document.documentElement) state.document.documentElement.getBoundingClientRect();
      state.status = 'loaded';
      state.resolve(set);
      set.dispatchEvent(state.event('loadingdone', state.loaded));
      if (state.failed.length) set.dispatchEvent(state.event('loadingerror', state.failed));
    });
  }
  function start(face) {
    const state = faceState(face);
    if (state.status !== 'unloaded') return state.promise;
    state.status = 'loading';
    for (const set of state.sets) beginSet(set, face);
    task(() => {
      try { host(face, 'load'); state.status = 'loaded'; state.resolve(face); }
      catch (_) { state.status = 'error'; state.reject(exception(state.binary ? 'SyntaxError' : 'NetworkError', 'Font could not be loaded')); }
      for (const set of state.sets) {
        const membership = setState(set);
        membership.pending.delete(face);
        (state.status === 'loaded' ? membership.loaded : membership.failed).push(face);
        finishSet(set);
      }
    });
    return state.promise;
  }
  class FontFace {
    constructor(family, source, descriptors = {}) {
      if (arguments.length < 2) throw new TypeError('FontFace requires family and source');
      const state = { descriptors: {...defaults}, status: 'unloaded', sets: new Set(), id: null, binary: false };
      state.promise = new Promise((resolve, reject) => { state.resolve = resolve; state.reject = reject; });
      faces.set(this, state);
      let bytes = null;
      if (ArrayBuffer.isView(source)) bytes = new Uint8Array(source.buffer, source.byteOffset, source.byteLength).slice();
      else if (source instanceof ArrayBuffer) bytes = new Uint8Array(source).slice();
      state.binary = bytes !== null;
      try {
        for (const name of Object.keys(defaults)) state.descriptors[name] = descriptor(name, name === 'family' ? family : descriptors[name] === undefined ? defaults[name] : descriptors[name]);
        const parsed = bytes ? [] : sources(String(source));
        state.id = native('create', JSON.stringify({...state.descriptors, family: unquote(state.descriptors.family), sources: parsed}), bytes);
      } catch (error) {
        state.status = 'error';
        for (const name of Object.keys(defaults)) state.descriptors[name] = '';
        state.reject(error.name === 'SyntaxError' ? error : syntax());
      }
      // BufferSource parsing begins in a font-loading task. Keep the
      // constructor-observable state at `unloaded` until that task runs.
      if (state.binary && state.status === 'unloaded') task(() => start(this));
    }
    get status() { return faceState(this).status; }
    get loaded() { return faceState(this).promise; }
    load() { return start(this); }
  }
  for (const name of Object.keys(defaults)) {
    Object.defineProperty(FontFace.prototype, name, {
      enumerable: true, configurable: true,
      get() { return faceState(this).descriptors[name]; },
      set(value) {
        const state = faceState(this);
        const normalized = descriptor(name, value);
        if (state.cssRule) {
          state.cssRule.style.setProperty(descriptorProperties[name], normalized);
          return;
        }
        state.descriptors[name] = normalized;
        if (state.id !== null) host(this, 'update');
      }
    });
  }
  function matched(set, font, text) {
    syncCSS(set);
    let query;
    try { query = JSON.parse(native('parse-font', JSON.stringify({input: String(font)}))); }
    catch (_) { throw syntax(); }
    const families = query.families.map(s => s.toLowerCase());
    const characters = Array.from(String(text), c => c.codePointAt(0));
    const result = [];
    const styleOrder = query.style === 'normal' ? ['normal','oblique','italic'] : query.style === 'italic' ? ['italic','oblique','normal'] : ['oblique','italic','normal'];
    const weightRank = value => {
      const weight = value === 'normal' ? 400 : value === 'bold' ? 700 : Number(value.split(/\s+/)[0]);
      const end = Number(value.split(/\s+/)[1] || weight);
      if (weight <= query.weight && query.weight <= end) return 0;
      const target = query.weight, candidate = target < weight ? weight : end;
      if (target >= 400 && target <= 500) return candidate >= target && candidate <= 500 ? candidate - target : candidate < target ? 1000 + target - candidate : 2000 + candidate - target;
      return target < 400 ? candidate < target ? target - candidate : 1000 + candidate - target : candidate > target ? candidate - target : 1000 + target - candidate;
    };
    for (const family of families) {
      let candidates = Array.from(setState(set).members).filter(face => unquote(faceState(face).descriptors.family).toLowerCase() === family);
      if (!candidates.length) continue;
      const styleRank = face => styleOrder.indexOf(faceState(face).descriptors.style.split(/\s+/)[0]);
      const bestStyle = Math.min(...candidates.map(styleRank));
      candidates = candidates.filter(face => styleRank(face) === bestStyle);
      const bestWeight = Math.min(...candidates.map(face => weightRank(faceState(face).descriptors.weight)));
      candidates = candidates.filter(face => weightRank(faceState(face).descriptors.weight) === bestWeight);
      for (const face of candidates) {
        const coverage = ranges(faceState(face).descriptors.unicodeRange);
        if (characters.some(code => coverage.some(([first,last]) => first <= code && code <= last)) && !result.includes(face)) result.push(face);
      }
    }
    return result;
  }
  function loadUsedFonts(set) {
    const state = setState(set);
    if (!state.document || !Array.from(state.members).some(face => faceState(face).status === 'unloaded')) return;
    for (const sample of JSON.parse(state.native('used', '{}'))) {
      for (const face of matched(set, sample.font, sample.text)) start(face);
    }
  }
  function requestUsedFonts(set) {
    const state = setState(set);
    if (!state.document || state.usedQueued) return;
    state.usedQueued = true;
    state.queue(() => { state.usedQueued = false; loadUsedFonts(set); });
  }
  class FontFaceSet extends EventTarget {
    constructor(initial = []) {
      super();
      const state = { members: new Set(), pending: new Set(), status: 'loaded', loaded: [], failed: [], document: null, finishing: false, css: new Map(), syncing: false };
      state.native = native;
      state.queue = callback => task(callback);
      state.event = (type, fontfaces) => type === 'loading' ? new Event(type) : new FontFaceSetLoadEvent(type, {fontfaces});
      state.refresh = () => syncCSS(this);
      state.ready = Promise.resolve(this);
      sets.set(this, state);
      for (const face of initial) this.add(face);
    }
    get size() { syncCSS(this); return setState(this).members.size; }
    get status() { syncCSS(this); return setState(this).status; }
    get ready() { syncCSS(this); loadUsedFonts(this); return setState(this).ready; }
    add(face) {
      const data = faceState(face), state = setState(this);
      if (data.cssSet && data.cssSet !== this) throw exception('InvalidModificationError', 'CSS-connected face belongs to its stylesheet');
      if (state.members.has(face)) return this;
      if (state.document && data.id !== null) host(face, 'add', state.native);
      state.members.add(face); data.sets.add(this);
      if (data.status === 'loading') beginSet(this, face);
      requestUsedFonts(this);
      return this;
    }
    delete(face) {
      const data = faceState(face), state = setState(this);
      if (data.cssSet === this) return false;
      if (!state.members.delete(face)) return false;
      if (state.document && data.id !== null) host(face, 'delete', state.native);
      data.sets.delete(this); state.pending.delete(face); finishSet(this);
      state.loaded = state.loaded.filter(item => item !== face);
      state.failed = state.failed.filter(item => item !== face);
      return true;
    }
    clear() { for (const face of Array.from(setState(this).members)) this.delete(face); }
    has(face) { syncCSS(this); faceState(face); return setState(this).members.has(face); }
    values() { syncCSS(this); return setState(this).members.values(); }
    keys() { return this.values(); }
    entries() { syncCSS(this); return setState(this).members.entries(); }
    [Symbol.iterator]() { return this.values(); }
    forEach(callback, thisArg) { syncCSS(this); setState(this).members.forEach(face => callback.call(thisArg, face, face, this)); }
    check(font, text = ' ') { if (!arguments.length) throw new TypeError('Font required'); return matched(this, font, text).every(face => face.status === 'loaded'); }
    load(font, text = ' ') {
      if (!arguments.length) return Promise.reject(new TypeError('Font required'));
      try {
        const selected = matched(this, font, text);
        return new Promise((resolve, reject) => task(() => {
          Promise.all(selected.map(face => face.load())).then(resolve, reject);
        }));
      }
      catch (error) { return Promise.reject(error); }
    }
  }
  function syncCSS(set) {
    const state = setState(set);
    if (!state.document || state.syncing) return;
    state.syncing = true;
    try {
      if (typeof globalThis.__omoikane_flush_stylesheets === 'function') {
        globalThis.__omoikane_flush_stylesheets();
      }
      const snapshots = JSON.parse(state.native('css', '{}'));
      const ruleViews = [];
      const collectRules = sheet => {
        if (!sheet) return;
        for (const rule of sheet.cssRules) {
          if (typeof CSSFontFaceRule === 'function' && rule instanceof CSSFontFaceRule) {
            if (rule.style.getPropertyValue('font-family') &&
                rule.style.getPropertyValue('src')) ruleViews.push(rule);
          } else if (rule.cssRules) {
            collectRules(rule);
          }
        }
      };
      for (const sheet of Array.from(state.document.styleSheets || [])) collectRules(sheet);
      for (const sheet of Array.from(state.document.adoptedStyleSheets || [])) collectRules(sheet);
      const current = new Set();
      for (let index = 0; index < snapshots.length; index++) {
        const snapshot = snapshots[index];
        const rule = ruleViews[index] || null;
        current.add(snapshot.id);
        let face = state.css.get(snapshot.id);
        if (!face) {
          face = Object.create(FontFace.prototype);
          const data = {id: snapshot.id, descriptors: {...defaults}, status: 'unloaded', binary: false, sets: new Set([set]), cssSet: set, cssRule: rule};
          data.promise = new Promise((resolve, reject) => { data.resolve = resolve; data.reject = reject; });
          // A stylesheet failure is reported by FontFaceSet's events, even when
          // no author reads the individual CSS-created face's loaded Promise.
          data.promise.catch(() => {});
          faces.set(face, data); state.css.set(snapshot.id, face); state.members.add(face);
        }
        const data = faceState(face);
        if (data.cssRule && data.cssRule !== rule) cssRuleFaces.delete(data.cssRule);
        data.cssRule = rule;
        for (const [name, property] of Object.entries(descriptorProperties)) {
          const raw = rule && rule.style.getPropertyValue(property);
          data.descriptors[name] = descriptor(name,
            raw || (name === 'family' ? snapshot.family : name === 'weight' ? snapshot.weight : name === 'style' ? snapshot.style : defaults[name]));
        }
        if (rule) cssRuleFaces.set(rule, face);
        if (data.status === 'unloaded') start(face);
      }
      for (const [id, face] of state.css) {
        if (current.has(id)) continue;
        const data = faceState(face);
        if (data.cssRule) cssRuleFaces.delete(data.cssRule);
        data.cssRule = null;
        data.cssSet = null; data.sets.delete(set);
        state.pending.delete(face); state.members.delete(face); state.css.delete(id);
      }
      finishSet(set);
    } finally { state.syncing = false; }
  }
  globalThis.__omoikane_font_face_rule_changed = (rule, property) => {
    const face = cssRuleFaces.get(rule);
    if (!face) return;
    const data = faceState(face);
    const set = data.cssSet;
    if (!set) return;
    // `src`, removal of a required descriptor, and whole-block replacement
    // can change which CSS-connected FontFace exists.
    if (property === 'src' || property === null ||
        ((property === 'font-family' || property === 'src') && !rule.style.getPropertyValue(property))) {
      syncCSS(set);
      return;
    }
    const name = propertyDescriptors[property];
    if (!name) return;
    data.descriptors[name] = descriptor(name,
      rule.style.getPropertyValue(property) || defaults[name]);
    host(face, 'update', setState(set).native);
  };
  class FontFaceSetLoadEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      Object.defineProperty(this, 'fontfaces', {value: Object.freeze(Array.from(options.fontfaces || [])), enumerable: true});
    }
  }
  for (const type of ['loading', 'loadingdone', 'loadingerror']) {
    const handlers = new WeakMap();
    Object.defineProperty(FontFaceSet.prototype, 'on' + type, {
      configurable: true, enumerable: true,
      get() { setState(this); return handlers.get(this) || null; },
      set(callback) {
        setState(this); const old = handlers.get(this); if (old) this.removeEventListener(type, old);
        callback = typeof callback === 'function' ? callback : null;
        handlers.set(this, callback); if (callback) this.addEventListener(type, callback);
      }
    });
  }
  Object.defineProperty(Document.prototype, 'fonts', {
    configurable: true, enumerable: true,
    get() {
      if (!(this instanceof Document)) throw new TypeError('Expected a Document');
      if (!documents.has(this)) { const set = new FontFaceSet(); setState(set).document = this; documents.set(this, set); }
      const set = documents.get(this);
      setState(set).refresh();
      return set;
    }
  });
  for (const constructor of [FontFace, FontFaceSet, FontFaceSetLoadEvent]) {
    Object.defineProperty(constructor.prototype, Symbol.toStringTag, {value: constructor.name, configurable: true});
    globalThis[constructor.name] = constructor;
  }
  if (typeof document !== 'undefined' && document) {
    const set = new FontFaceSet();
    setState(set).document = document;
    documents.set(document, set);
    const observer = new MutationObserver(() => {
      if (documents.has(document)) {
        const set = documents.get(document); syncCSS(set); requestUsedFonts(set);
      }
    });
    observer.observe(document, {subtree: true, childList: true, characterData: true,
      attributes: true, attributeFilter: ['href', 'rel', 'media', 'disabled']});
  }
})();
