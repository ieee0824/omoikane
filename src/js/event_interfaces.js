// Install with the bootstrap's intrinsic Event and native interface brands.
// Event payloads are snapshots stored independently of author expandos.
(() => {
  const weakGet = Function.prototype.call.bind(WeakMap.prototype.get);
  const weakSet = Function.prototype.call.bind(WeakMap.prototype.set);
  const TypeErrorIntrinsic = TypeError;
  const string = value => {
    if (typeof value === "symbol") throw new TypeErrorIntrinsic("Cannot convert a Symbol to a string");
    return String(value);
  };
  const dictionary = value => {
    if (value == null) return {};
    if (typeof value !== "object" && typeof value !== "function") throw new TypeErrorIntrinsic("Expected a dictionary");
    return value;
  };
  const finite = value => {
    const converted = +value;
    if (!Number.isFinite(converted)) throw new TypeErrorIntrinsic("Expected a finite number");
    return converted;
  };
  function eventInit(init) {
    return { bubbles: !!init.bubbles, cancelable: !!init.cancelable, composed: !!init.composed };
  }
  function expose(type, properties, state) {
    Object.defineProperty(type.prototype, Symbol.toStringTag, { value: type.name, configurable: true });
    for (const name of properties) {
      const getter = function() {
        const record = weakGet(state, this);
        if (!record) throw new TypeErrorIntrinsic("Illegal invocation");
        return record[name];
      };
      Object.defineProperty(getter, "name", { value: "get " + name, configurable: true });
      Object.defineProperty(type.prototype, name, { configurable: true, enumerable: true, get: getter });
    }
    Object.defineProperty(globalThis, type.name, { value: type, writable: true, configurable: true });
    return type;
  }
  function payloadEvent(Event, name, members, requiredInit = false) {
    const state = new WeakMap();
    const Type = class extends Event {
      constructor(type, init = {}) {
        if (!arguments.length || requiredInit && arguments.length < 2) throw new TypeErrorIntrinsic("Missing required argument");
        const convertedType = string(type), source = dictionary(init), base = eventInit(source), record = {};
        for (const [key, convert, fallback, required] of members) {
          const value = source[key];
          if (required && value === undefined) throw new TypeErrorIntrinsic(key + " is required");
          record[key] = value === undefined ? fallback : convert(value);
        }
        super(convertedType, base);
        weakSet(state, this, record);
      }
    };
    Object.defineProperty(Type, "name", { value: name, configurable: true });
    if (requiredInit) Object.defineProperty(Type, "length", { value: 2, configurable: true });
    return expose(Type, members.map(member => member[0]), state);
  }
  function navigationEvents(context) {
    const { Event } = context;
    const usvString = value => string(value).toWellFormed();
    return {
      ErrorEvent: payloadEvent(Event, "ErrorEvent", [
        ["colno", value => (+value) >>> 0, 0], ["error", value => value, undefined],
        ["filename", usvString, ""], ["lineno", value => (+value) >>> 0, 0], ["message", string, ""],
      ]),
      PopStateEvent: payloadEvent(Event, "PopStateEvent", [
        ["hasUAVisualTransition", Boolean, false], ["state", value => value, null],
      ]),
      HashChangeEvent: payloadEvent(Event, "HashChangeEvent", [
        ["newURL", usvString, ""], ["oldURL", usvString, ""],
      ]),
      PageTransitionEvent: payloadEvent(Event, "PageTransitionEvent", [["persisted", Boolean, false]]),
      SubmitEvent: payloadEvent(Event, "SubmitEvent", [["submitter", value => {
        if (value == null) return null;
        if (!context.isHTMLElement(value)) throw new TypeErrorIntrinsic("submitter must be an HTMLElement");
        return value;
      }, null]]),
      FormDataEvent: payloadEvent(Event, "FormDataEvent", [["formData", value => {
        if (!context.isFormData(value)) throw new TypeErrorIntrinsic("formData must be a FormData");
        return value;
      }, undefined, true]], true),
      ContentVisibilityAutoStateChangeEvent: payloadEvent(Event, "ContentVisibilityAutoStateChangeEvent", [["skipped", Boolean, false]]),
    };
  }
  function beforeUnloadInterface(Event) {
    const state = new WeakMap(), token = {};
    class BeforeUnloadEvent extends Event {
      constructor(key, type, init) {
        if (key !== token) throw new TypeErrorIntrinsic("Illegal constructor");
        super(type, init);
        weakSet(state, this, "");
      }
      get returnValue() {
        const value = weakGet(state, this);
        if (value === undefined) throw new TypeErrorIntrinsic("Illegal invocation");
        return value;
      }
      set returnValue(value) {
        if (weakGet(state, this) === undefined) throw new TypeErrorIntrinsic("Illegal invocation");
        weakSet(state, this, string(value));
      }
    }
    expose(BeforeUnloadEvent, [], state);
    Object.defineProperty(BeforeUnloadEvent, "length", { value: 0, configurable: true });
    Object.defineProperty(BeforeUnloadEvent.prototype, "returnValue", { enumerable: true });
    return { BeforeUnloadEvent, makeBeforeUnload: (type, init) => new BeforeUnloadEvent(token, type, init) };
  }
  function touchInterfaces(context) {
    const state = new WeakMap();
    const properties = ["altitudeAngle", "azimuthAngle", "clientX", "clientY", "force", "identifier", "pageX", "pageY",
      "radiusX", "radiusY", "rotationAngle", "screenX", "screenY", "target", "touchType"];
    class Touch {
      constructor(init) {
        if (!arguments.length) throw new TypeErrorIntrinsic("Touch requires a dictionary");
        const source = dictionary(init), record = {};
        for (const name of properties) {
          const value = source[name];
          if ((name === "identifier" || name === "target") && value === undefined) throw new TypeErrorIntrinsic(name + " is required");
          if (name === "target") {
            if (!context.isEventTarget(value)) throw new TypeErrorIntrinsic("target must be an EventTarget");
            record[name] = value;
          } else if (name === "touchType") {
            record[name] = value === undefined ? "direct" : string(value);
            if (!["direct", "stylus"].includes(record[name])) throw new TypeErrorIntrinsic("Invalid TouchType");
          } else if (name === "identifier") record[name] = (+value) | 0;
          else {
            const number = value === undefined ? 0 : finite(value);
            record[name] = ["force", "radiusX", "radiusY", "rotationAngle"].includes(name) ? Math.fround(number) : number;
            if (!Number.isFinite(record[name])) throw new TypeErrorIntrinsic("Expected a finite float");
          }
        }
        weakSet(state, this, record);
      }
    }
    expose(Touch, properties, state);
    return { Touch, isTouch: object => weakGet(state, object) !== undefined };
  }
  const install = globalThis.__omoikane_install_event_interfaces;
  delete globalThis.__omoikane_install_event_interfaces;
  install(context => ({ ...navigationEvents(context), ...beforeUnloadInterface(context.Event), ...touchInterfaces(context) }));
})();
