// TouchEvent uses genuine Touch payloads and immutable indexed snapshots.
(() => {
  const get = Function.prototype.call.bind(WeakMap.prototype.get);
  const set = Function.prototype.call.bind(WeakMap.prototype.set);
  const listState = new WeakMap(), eventState = new WeakMap(), token = {};
  const read = (state, object) => {
    const value = get(state, object);
    if (!value) throw new TypeError("Illegal invocation");
    return value;
  };
  class TouchList {
    constructor(key, touches) {
      if (key !== token) throw new TypeError("Illegal constructor");
      set(listState, this, touches);
    }
    get length() { return read(listState, this).length; }
    item(index) {
      if (!arguments.length) throw new TypeError("item requires an index");
      return read(listState, this)[(+index) >>> 0] ?? null;
    }
    [Symbol.iterator]() { return read(listState, this).values(); }
  }
  function list(source, isTouch) {
    const touches = [];
    if (source !== undefined) {
      if (source === null || typeof source !== "object" && typeof source !== "function") throw new TypeError("Expected a sequence");
      for (const value of source) {
        if (!isTouch(value)) throw new TypeError("Expected a Touch");
        touches.push(value);
      }
    }
    const target = new TouchList(token, touches);
    const index = key => typeof key === "string" && /^(?:0|[1-9]\d*)$/.test(key) && +key < 4294967295;
    const result = new Proxy(target, {
      get(_target, key, receiver) { return index(key) ? touches[+key] : Reflect.get(target, key, receiver); },
      has(_target, key) { return index(key) ? +key < touches.length : key in target; },
      ownKeys() { return touches.map((_, i) => String(i)).concat(Reflect.ownKeys(target)); },
      getOwnPropertyDescriptor(_target, key) {
        if (index(key) && +key < touches.length) return { value: touches[+key], writable: false, enumerable: true, configurable: true };
        return Reflect.getOwnPropertyDescriptor(target, key);
      },
      set(_target, key, value, receiver) { return !index(key) && Reflect.set(target, key, value, receiver); },
      defineProperty(_target, key, value) { return !index(key) && Reflect.defineProperty(target, key, value); },
      deleteProperty(_target, key) { return !(index(key) && +key < touches.length) && Reflect.deleteProperty(target, key); },
      preventExtensions() { return false; },
    });
    set(listState, result, touches);
    return result;
  }
  function touchEventInterface(context) {
    class TouchEvent extends context.UIEvent {
      constructor(type, init = {}) {
        if (!arguments.length) throw new TypeError("TouchEvent requires a type");
        if (typeof type === "symbol") throw new TypeError("Cannot convert a Symbol to a string");
        const convertedType = String(type);
        if (init != null && typeof init !== "object" && typeof init !== "function") throw new TypeError("Expected a dictionary");
        const source = init ?? {}, base = {};
        for (const key of ["bubbles", "cancelable", "composed"]) base[key] = !!source[key];
        base.detail = (+source.detail) | 0;
        base.view = source.view ?? null;
        const record = {};
        for (const name of ["altKey", "ctrlKey", "metaKey", "modifierAltGraph", "modifierCapsLock", "modifierFn", "modifierFnLock",
          "modifierHyper", "modifierNumLock", "modifierScrollLock", "modifierSuper", "modifierSymbol", "modifierSymbolLock", "shiftKey"]) {
          record[name] = !!source[name];
        }
        for (const name of ["changedTouches", "targetTouches", "touches"]) record[name] = list(source[name], context.isTouch);
        super(convertedType, base);
        set(eventState, this, record);
      }
      getModifierState(key) {
        if (!arguments.length) throw new TypeError("getModifierState requires a key");
        const record = read(eventState, this);
        if (typeof key === "symbol") throw new TypeError("Cannot convert a Symbol to a string");
        const names = { Alt: "altKey", Control: "ctrlKey", Meta: "metaKey", Shift: "shiftKey", AltGraph: "modifierAltGraph",
          CapsLock: "modifierCapsLock", Fn: "modifierFn", FnLock: "modifierFnLock", Hyper: "modifierHyper", NumLock: "modifierNumLock",
          ScrollLock: "modifierScrollLock", Super: "modifierSuper", Symbol: "modifierSymbol", SymbolLock: "modifierSymbolLock" };
        return !!record[names[String(key)]];
      }
    }
    for (const name of ["touches", "targetTouches", "changedTouches", "altKey", "ctrlKey", "metaKey", "shiftKey"]) {
      const getter = function() { return read(eventState, this)[name]; };
      Object.defineProperty(getter, "name", { value: "get " + name, configurable: true });
      Object.defineProperty(TouchEvent.prototype, name, { get: getter, configurable: true, enumerable: true });
    }
    return TouchEvent;
  }
  const install = globalThis.__omoikane_install_touch_events;
  delete globalThis.__omoikane_install_touch_events;
  install(context => {
    const TouchEvent = touchEventInterface(context);
    for (const type of [TouchList, TouchEvent]) {
      if (type === TouchList) Object.defineProperty(type, "length", { value: 0, configurable: true });
      Object.defineProperty(type.prototype, Symbol.toStringTag, { value: type.name, configurable: true });
      for (const name of Object.getOwnPropertyNames(type.prototype)) {
        if (name !== "constructor") Object.defineProperty(type.prototype, name, { enumerable: true });
      }
      Object.defineProperty(globalThis, type.name, { value: type, configurable: true, writable: true, enumerable: false });
    }
    return { TouchList, TouchEvent };
  });
})();
