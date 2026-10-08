// Secure-context motion event payloads. Permission stays with the shared
// permission manager; constructing an event does not access a sensor.
(() => {
  const get = Function.prototype.call.bind(WeakMap.prototype.get);
  const set = Function.prototype.call.bind(WeakMap.prototype.set);
  const nullableNumber = value => {
    if (value == null) return null;
    const result = +value;
    if (!Number.isFinite(result)) throw new TypeError("Expected a finite number");
    return result;
  };
  const dictionary = value => {
    if (value == null) return {};
    if (typeof value !== "object" && typeof value !== "function") throw new TypeError("Expected a dictionary");
    return value;
  };
  function expose(Type, state, fields, secure) {
    Object.defineProperty(Type.prototype, Symbol.toStringTag, { value: Type.name, configurable: true });
    for (const name of fields) {
      const getter = function() {
        const record = get(state, this);
        if (!record) throw new TypeError("Illegal invocation");
        return record[name];
      };
      Object.defineProperty(getter, "name", { value: "get " + name, configurable: true });
      Object.defineProperty(Type.prototype, name, { configurable: true, enumerable: true, get: getter });
    }
    if (secure) Object.defineProperty(globalThis, Type.name, { value: Type, configurable: true, writable: true });
  }
  function vectorInterface(name, fields, secure) {
    const state = new WeakMap(), token = {};
    const Type = class {
      constructor(key, init) {
        if (key !== token) throw new TypeError("Illegal constructor");
        const source = dictionary(init), record = {};
        for (const name of fields) record[name] = nullableNumber(source[name]);
        set(state, this, record);
      }
    };
    Object.defineProperty(Type, "name", { value: name, configurable: true });
    Object.defineProperty(Type, "length", { value: 0, configurable: true });
    expose(Type, state, fields, secure);
    return value => value === undefined ? null : new Type(token, value);
  }
  function eventType(context, name, members, permissions) {
    const state = new WeakMap();
    const Type = class extends context.Event {
      constructor(type, init = {}) {
        if (!arguments.length) throw new TypeError("Missing event type");
        if (typeof type === "symbol") throw new TypeError("Cannot convert a Symbol to a string");
        const convertedType = String(type), source = dictionary(init), base = {};
        for (const key of ["bubbles", "cancelable", "composed"]) base[key] = !!source[key];
        const record = {};
        for (const [key, convert] of members) record[key] = convert(source[key]);
        super(convertedType, base);
        set(state, this, record);
      }
    };
    Object.defineProperty(Type, "name", { value: name, configurable: true });
    Object.defineProperty(Type, "requestPermission", {
      configurable: true, writable: true, enumerable: true,
      value: function requestPermission(absolute = false) {
        return context.sensorPermission(name === "DeviceOrientationEvent" && !!absolute ? [...permissions, "magnetometer"] : permissions);
      },
    });
    expose(Type, state, members.map(member => member[0]), context.secure);
    return Type;
  }
  const install = globalThis.__omoikane_install_device_events;
  delete globalThis.__omoikane_install_device_events;
  install(context => {
    const acceleration = vectorInterface("DeviceMotionEventAcceleration", ["x", "y", "z"], context.secure);
    const rotation = vectorInterface("DeviceMotionEventRotationRate", ["alpha", "beta", "gamma"], context.secure);
    return {
      DeviceOrientationEvent: eventType(context, "DeviceOrientationEvent", [
        ["absolute", Boolean], ["alpha", nullableNumber], ["beta", nullableNumber], ["gamma", nullableNumber],
      ], ["accelerometer", "gyroscope"]),
      DeviceMotionEvent: eventType(context, "DeviceMotionEvent", [
        ["acceleration", acceleration], ["accelerationIncludingGravity", acceleration],
        ["interval", value => nullableNumber(value === undefined ? 0 : +value)], ["rotationRate", rotation],
      ], ["accelerometer", "gyroscope"]),
    };
  });
})();
