(() => {
  "use strict";

  // CSS Custom Highlight API 1: Highlight, HighlightRegistry and
  // CSS.highlights. Registered ranges are resolved to Text intervals and
  // published natively so paint can apply ::highlight() styles.

  const nativeSetTextHighlights = globalThis.__omoikane_set_text_highlights;
  delete globalThis.__omoikane_set_text_highlights;
  const installHooks = globalThis.__omoikane_install_highlight_hooks;
  if (typeof installHooks !== "function" || typeof nativeSetTextHighlights !== "function" ||
      !globalThis.CSS || typeof globalThis.AbstractRange !== "function") {
    return;
  }

  // Capture intrinsics so author changes to Set/Map prototypes cannot affect
  // the setlike and maplike members (WebIDL uses the original operations).
  const uncurry = fn => Function.prototype.call.bind(fn);
  const SetIntrinsic = Set;
  const MapIntrinsic = Map;
  const setAdd = uncurry(Set.prototype.add);
  const setHas = uncurry(Set.prototype.has);
  const setDelete = uncurry(Set.prototype.delete);
  const setClear = uncurry(Set.prototype.clear);
  const setValues = uncurry(Set.prototype.values);
  const setEntries = uncurry(Set.prototype.entries);
  const setForEach = uncurry(Set.prototype.forEach);
  const setSize = uncurry(Object.getOwnPropertyDescriptor(Set.prototype, "size").get);
  const mapGet = uncurry(Map.prototype.get);
  const mapSet = uncurry(Map.prototype.set);
  const mapHas = uncurry(Map.prototype.has);
  const mapDelete = uncurry(Map.prototype.delete);
  const mapClear = uncurry(Map.prototype.clear);
  const mapKeys = uncurry(Map.prototype.keys);
  const mapValues = uncurry(Map.prototype.values);
  const mapEntries = uncurry(Map.prototype.entries);
  const mapForEach = uncurry(Map.prototype.forEach);
  const mapSize = uncurry(Object.getOwnPropertyDescriptor(Map.prototype, "size").get);
  const weakGet = uncurry(WeakMap.prototype.get);
  const weakSet = uncurry(WeakMap.prototype.set);
  const weakHas = uncurry(WeakMap.prototype.has);
  const rangeGetter = name =>
    uncurry(Object.getOwnPropertyDescriptor(globalThis.AbstractRange.prototype, name).get);
  const startContainerOf = rangeGetter("startContainer");
  const startOffsetOf = rangeGetter("startOffset");
  const endContainerOf = rangeGetter("endContainer");
  const endOffsetOf = rangeGetter("endOffset");
  const rangeClientRects = uncurry(globalThis.Range.prototype.getClientRects);
  const RangeIntrinsic = globalThis.Range;
  const TypeErrorIntrinsic = TypeError;
  const isAbstractRange = value => {
    try { startContainerOf(value); return true; } catch (_) { return false; }
  };

  const highlightState = new WeakMap();
  const HIGHLIGHT_TYPES = ["highlight", "spelling-error", "grammar-error"];
  const illegal = () => { throw new TypeErrorIntrinsic("Illegal invocation"); };
  const stateOf = highlight => weakGet(highlightState, highlight) || illegal();
  const requireRange = value => {
    if (!isAbstractRange(value)) throw new TypeErrorIntrinsic("Argument is not an AbstractRange");
    return value;
  };

  let syncScheduled = false;
  const scheduleSync = () => {
    if (syncScheduled) return;
    syncScheduled = true;
    queueMicrotask(syncHighlights);
  };
  let registryHasEntries = false;
  const hooks = installHooks(() => { if (registryHasEntries) scheduleSync(); });

  class Highlight {
    constructor(...initialRanges) {
      if (new.target === undefined) throw new TypeErrorIntrinsic("Constructor requires 'new'");
      const ranges = new SetIntrinsic();
      for (const range of initialRanges) setAdd(ranges, requireRange(range));
      weakSet(highlightState, this, { ranges, priority: 0, type: "highlight" });
    }
    get priority() { return stateOf(this).priority; }
    set priority(value) {
      const state = stateOf(this);
      // WebIDL long conversion.
      state.priority = Number(value) | 0;
      highlightChanged(this);
    }
    get type() { return stateOf(this).type; }
    set type(value) {
      const state = stateOf(this);
      const type = String(value);
      // Invalid enumeration values are ignored.
      if (HIGHLIGHT_TYPES.includes(type)) state.type = type;
    }
    get size() { return setSize(stateOf(this).ranges); }
    has(range) { return setHas(stateOf(this).ranges, range); }
    add(range) {
      const state = stateOf(this);
      setAdd(state.ranges, requireRange(range));
      highlightChanged(this);
      return this;
    }
    delete(range) {
      const removed = setDelete(stateOf(this).ranges, range);
      if (removed) highlightChanged(this);
      return removed;
    }
    clear() {
      setClear(stateOf(this).ranges);
      highlightChanged(this);
    }
    entries() { return setEntries(stateOf(this).ranges); }
    keys() { return setValues(stateOf(this).ranges); }
    values() { return setValues(stateOf(this).ranges); }
    forEach(callback, thisArg = undefined) {
      if (typeof callback !== "function") throw new TypeErrorIntrinsic("Callback is not a function");
      const highlight = this;
      setForEach(stateOf(this).ranges, range => {
        callback.call(thisArg, range, range, highlight);
      });
    }
  }
  Object.defineProperty(Highlight.prototype, Symbol.iterator, {
    configurable: true, writable: true, value: Highlight.prototype.values,
  });

  // The registry keeps registration order, which breaks priority ties.
  const registryState = new WeakMap();
  const registryOf = registry => weakGet(registryState, registry) || illegal();
  let registrationCounter = 0;

  class HighlightRegistry {
    constructor() { throw new TypeErrorIntrinsic("Illegal constructor"); }
    get size() { return mapSize(registryOf(this).map); }
    get(name) { return mapGet(registryOf(this).map, String(name)); }
    has(name) { return mapHas(registryOf(this).map, String(name)); }
    set(name, highlight) {
      const state = registryOf(this);
      const key = String(name);
      if (!weakHas(highlightState, highlight)) {
        throw new TypeErrorIntrinsic("Argument is not a Highlight");
      }
      if (!mapHas(state.map, key) || mapGet(state.map, key) !== highlight) {
        mapSet(state.map, key, highlight);
        mapSet(state.order, key, ++registrationCounter);
        registryChanged();
      }
      return this;
    }
    delete(name) {
      const state = registryOf(this);
      const key = String(name);
      const removed = mapDelete(state.map, key);
      mapDelete(state.order, key);
      if (removed) registryChanged();
      return removed;
    }
    clear() {
      const state = registryOf(this);
      mapClear(state.map);
      mapClear(state.order);
      registryChanged();
    }
    entries() { return mapEntries(registryOf(this).map); }
    keys() { return mapKeys(registryOf(this).map); }
    values() { return mapValues(registryOf(this).map); }
    forEach(callback, thisArg = undefined) {
      if (typeof callback !== "function") throw new TypeErrorIntrinsic("Callback is not a function");
      const registry = this;
      mapForEach(registryOf(this).map, (highlight, name) => {
        callback.call(thisArg, highlight, name, registry);
      });
    }
    highlightsFromPoint(x, y, options = undefined) {
      const state = registryOf(this);
      if (arguments.length < 2) throw new TypeErrorIntrinsic("highlightsFromPoint requires 2 arguments");
      x = Number(x);
      y = Number(y);
      if (!Number.isFinite(x) || !Number.isFinite(y)) {
        throw new TypeErrorIntrinsic("Coordinates must be finite numbers");
      }
      if (options !== undefined && options !== null && typeof options !== "object" &&
          typeof options !== "function") {
        throw new TypeErrorIntrinsic("options must be a dictionary");
      }
      const doc = globalThis.document;
      const root = doc && doc.documentElement;
      if (!root || x < 0 || y < 0 || x > root.clientWidth || y > root.clientHeight) return [];
      const hits = [];
      for (const entry of paintOrder(state)) {
        const ranges = [];
        setForEach(stateOf(entry.highlight).ranges, range => {
          for (const rect of rangeRects(range)) {
            if (x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom) {
              ranges.push(range);
              return;
            }
          }
        });
        if (ranges.length && !hits.some(hit => hit.highlight === entry.highlight)) {
          hits.push({ highlight: entry.highlight, ranges });
        }
      }
      // Topmost first.
      return hits.reverse();
    }
  }
  Object.defineProperty(HighlightRegistry.prototype, Symbol.iterator, {
    configurable: true, writable: true, value: HighlightRegistry.prototype.entries,
  });

  function rangeRects(range) {
    if (range instanceof RangeIntrinsic) return rangeClientRects(range);
    // StaticRange has no geometry API; measure an equivalent live Range.
    const live = new RangeIntrinsic();
    try {
      live.setStart(startContainerOf(range), startOffsetOf(range));
      live.setEnd(endContainerOf(range), endOffsetOf(range));
    } catch (_) {
      return [];
    }
    const rects = rangeClientRects(live);
    live.detach();
    return rects;
  }

  // Registered entries from bottom to top: ascending priority, then
  // registration order.
  function paintOrder(state) {
    const entries = [];
    mapForEach(state.map, (highlight, name) => {
      entries.push({
        name, highlight,
        priority: stateOf(highlight).priority,
        registered: mapGet(state.order, name),
      });
    });
    entries.sort((a, b) => a.priority - b.priority || a.registered - b.registered);
    return entries;
  }

  const registry = Object.create(HighlightRegistry.prototype);
  weakSet(registryState, registry, { map: new MapIntrinsic(), order: new MapIntrinsic() });

  function registryChanged() {
    registryHasEntries = true;
    scheduleSync();
  }
  function highlightChanged(_highlight) {
    if (registryHasEntries) scheduleSync();
  }

  // Text nodes that currently carry published intervals.
  let publishedTextNodes = new SetIntrinsic();

  function syncHighlights() {
    syncScheduled = false;
    const state = registryOf(registry);
    const byNode = new MapIntrinsic();
    const order = paintOrder(state);
    for (let index = 0; index < order.length; index++) {
      const { name, highlight } = order[index];
      setForEach(stateOf(highlight).ranges, range => {
        const intervals = hooks.textIntervals(
          startContainerOf(range), startOffsetOf(range),
          endContainerOf(range), endOffsetOf(range));
        for (const [node, start, end] of intervals) {
          let list = mapGet(byNode, node);
          if (!list) mapSet(byNode, node, list = []);
          list.push([start, end, name, index]);
        }
      });
    }
    setForEach(publishedTextNodes, node => {
      if (!mapHas(byNode, node)) nativeSetTextHighlights(hooks.nodeId(node), []);
    });
    const next = new SetIntrinsic();
    mapForEach(byNode, (list, node) => {
      nativeSetTextHighlights(hooks.nodeId(node), list);
      setAdd(next, node);
    });
    publishedTextNodes = next;
    registryHasEntries = mapSize(state.map) > 0;
  }

  function exposeInterface(name, value) {
    Object.defineProperty(value, "length", { configurable: true, value: 0 });
    Object.defineProperty(globalThis, name, { configurable: true, writable: true, value });
  }
  exposeInterface("Highlight", Highlight);
  exposeInterface("HighlightRegistry", HighlightRegistry);
  const nonEnumerable = new SetIntrinsic(["constructor"]);
  for (const prototype of [Highlight.prototype, HighlightRegistry.prototype]) {
    for (const key of Reflect.ownKeys(prototype)) {
      if (typeof key !== "string" || nonEnumerable.has(key)) continue;
      const descriptor = Object.getOwnPropertyDescriptor(prototype, key);
      descriptor.enumerable = true;
      if (descriptor.get) Object.defineProperty(descriptor.get, "name", { value: "get " + key });
      if (descriptor.set) Object.defineProperty(descriptor.set, "name", { value: "set " + key });
      Object.defineProperty(prototype, key, descriptor);
    }
  }
  for (const [prototype, name] of [[Highlight.prototype, "Highlight"],
    [HighlightRegistry.prototype, "HighlightRegistry"]]) {
    Object.defineProperty(prototype, Symbol.toStringTag, { configurable: true, value: name });
  }
  Object.defineProperty(globalThis.CSS, "highlights", {
    configurable: true, enumerable: true,
    get: function highlights() { return registry; },
  });
  Object.defineProperty(Object.getOwnPropertyDescriptor(globalThis.CSS, "highlights").get,
    "name", { value: "get highlights" });
})();
