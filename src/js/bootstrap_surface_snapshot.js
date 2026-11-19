// Diagnostic shape oracle: descriptors are read without invoking property getters.
// This records the existing surface, including accidental Window exposures; it is
// not an exposure allowlist or proof of Web IDL conformance.
(() => {
  const names = Object.getOwnPropertyNames;
  const descriptor = Object.getOwnPropertyDescriptor;
  const prototype = Object.getPrototypeOf;
  const describe = object => names(object).sort().map(name => {
    const property = descriptor(object, name);
    const common = { name, enumerable: property.enumerable, configurable: property.configurable };
    if ("value" in property) {
      return { ...common, kind: "data", writable: property.writable, type: typeof property.value };
    }
    return { ...common, kind: "accessor", get: typeof property.get, set: typeof property.set };
  });
  const chain = object => {
    const levels = [];
    const seen = new Set();
    for (let current = prototype(object); current !== null; current = prototype(current)) {
      if (seen.has(current)) throw new Error("cyclic prototype chain");
      seen.add(current);
      levels.push(describe(current));
    }
    return levels;
  };
  const symbols = object => Object.getOwnPropertySymbols(object).map(symbol => {
    const property = descriptor(object, symbol);
    const common = {
      description: symbol.description ?? null,
      registryKey: Symbol.keyFor(symbol) ?? null,
      enumerable: property.enumerable,
      configurable: property.configurable,
    };
    return "value" in property
      ? { ...common, kind: "data", writable: property.writable, type: typeof property.value }
      : { ...common, kind: "accessor", get: typeof property.get, set: typeof property.set };
  });
  const prototypeSymbols = object => {
    const result = [];
    for (let current = prototype(object); current !== null; current = prototype(current)) {
      result.push(symbols(current));
    }
    return result;
  };
  const interfacePrototypes = {};
  for (const name of ["EventTarget", "Event", "MessageEvent", "AbortSignal", "AbortController",
      "DOMException", "Navigator", "WorkerNavigator", "WorkerGlobalScope",
      "DedicatedWorkerGlobalScope", "SharedWorkerGlobalScope", "WorkletGlobalScope",
      "URL", "Request", "Response", "Headers", "Blob", "MessagePort", "MessageChannel"]) {
    const constructor = descriptor(globalThis, name)?.value;
    const object = typeof constructor === "function" ? descriptor(constructor, "prototype")?.value : null;
    interfacePrototypes[name] = object ? {
      own: describe(object), symbols: symbols(object),
      chain: chain(object), prototypeSymbols: prototypeSymbols(object),
    } : null;
  }
  const globalConstructorChains = [];
  for (const name of names(globalThis).sort()) {
    const constructor = descriptor(globalThis, name)?.value;
    const object = typeof constructor === "function" ? descriptor(constructor, "prototype")?.value : null;
    if (!object) continue;
    const ancestors = [];
    for (let current = object; current !== null; current = prototype(current)) {
      const value = descriptor(current, "constructor")?.value;
      ancestors.push(typeof value === "function" ? descriptor(value, "name")?.value ?? null : null);
    }
    globalConstructorChains.push({ name, ancestors });
  }
  const navigatorDescriptor = descriptor(globalThis, "navigator");
  // navigator is an existing, trusted runtime platform object. This explicit
  // read is separate from the getter-free enumeration above.
  const navigatorObject = typeof navigator === "object" && navigator !== null ? navigator : null;
  return JSON.stringify({
    globals: describe(globalThis),
    globalSymbols: symbols(globalThis),
    globalPrototypeSymbols: prototypeSymbols(globalThis),
    interfacePrototypes,
    globalConstructorChains,
    globalPrototypeChain: chain(globalThis),
    navigatorDescriptor: navigatorDescriptor ? {
      kind: "value" in navigatorDescriptor ? "data" : "accessor",
      enumerable: navigatorDescriptor.enumerable,
      configurable: navigatorDescriptor.configurable,
    } : null,
    navigator: navigatorObject ? describe(navigatorObject) : null,
    navigatorPrototypeChain: navigatorObject ? chain(navigatorObject) : null,
  });
})()
