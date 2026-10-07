// Construction bridges are consumed and removed by the main DOM bootstrap.
// Platform state lives in WeakMaps rather than writable page-visible fields.
(() => {
  const token = {};
  const TypeErrorIntrinsic = TypeError;
  const StringIntrinsic = String;
  const domString = value => {
    if (typeof value === "symbol") throw new TypeErrorIntrinsic("Cannot convert a Symbol to a string");
    return StringIntrinsic(value);
  };
  let isNodeIntrinsic, nodeTypeIntrinsic, DOMExceptionIntrinsic;
  const ranges = new WeakMap(), collections = new WeakMap();
  const implementations = new WeakMap(), metrics = new WeakMap();
  const datasetCache = new WeakMap();
  const weakGet = Function.prototype.call.bind(WeakMap.prototype.get);
  const weakSet = Function.prototype.call.bind(WeakMap.prototype.set);
  const weakHas = Function.prototype.call.bind(WeakMap.prototype.has);
  const fail = () => { throw new TypeErrorIntrinsic("Illegal invocation"); };
  const read = (map, object) => weakGet(map, object) || fail();
  class AbstractRange {
    constructor(key = undefined) { if (key !== token) throw new TypeErrorIntrinsic("Illegal constructor"); }
    get startContainer() { return read(ranges, this)()[0]; }
    get startOffset() { return read(ranges, this)()[1]; }
    get endContainer() { return read(ranges, this)()[2]; }
    get endOffset() { return read(ranges, this)()[3]; }
    get collapsed() {
      const [start, startOffset, end, endOffset] = read(ranges, this)();
      return start === end && startOffset === endOffset;
    }
  }
  class StaticRange extends AbstractRange {
    constructor(init) {
      super(token);
      if (init === null || (typeof init !== "object" && typeof init !== "function")) {
        throw new TypeErrorIntrinsic("StaticRange requires a dictionary");
      }
      // Required dictionary members are converted in lexicographic order.
      const end = init.endContainer;
      if (!isNodeIntrinsic(end)) throw new TypeErrorIntrinsic("endContainer must be a Node");
      const endValue = init.endOffset;
      if (endValue === undefined) throw new TypeErrorIntrinsic("endOffset is required");
      const endOffset = (+endValue) >>> 0;
      const start = init.startContainer;
      if (!isNodeIntrinsic(start)) throw new TypeErrorIntrinsic("startContainer must be a Node");
      const startValue = init.startOffset;
      if (startValue === undefined) throw new TypeErrorIntrinsic("startOffset is required");
      const startOffset = (+startValue) >>> 0;
      if ([nodeTypeIntrinsic(start), nodeTypeIntrinsic(end)].some(type => type === 2 || type === 10)) {
        throw new DOMExceptionIntrinsic("Invalid boundary node", "InvalidNodeTypeError");
      }
      const boundaries = [start, startOffset, end, endOffset];
      weakSet(ranges, this, () => boundaries);
    }
  }

  const isIndex = key => typeof key === "string" && /^(?:0|[1-9]\d*)$/.test(key) && Number(key) < 4294967295;
  function namedItem(state, name) {
    if (name === "") return null;
    for (const element of state.collect()) {
      if (element.getAttribute("id") === name ||
          (state.allowsName(element) && element.getAttribute("name") === name)) return element;
    }
    return null;
  }
  class HTMLCollection {
    constructor(key = undefined) { if (key !== token) throw new TypeErrorIntrinsic("Illegal constructor"); }
    get length() { return read(collections, this).collect().length; }
    item(index) {
      if (!arguments.length) throw new TypeErrorIntrinsic("item requires an index");
      return read(collections, this).collect()[(+index) >>> 0] ?? null;
    }
    namedItem(name) {
      if (!arguments.length) throw new TypeErrorIntrinsic("namedItem requires a name");
      return namedItem(read(collections, this), domString(name));
    }
    *[Symbol.iterator]() {
      const state = read(collections, this);
      for (let index = 0; index < state.collect().length; index++) yield state.collect()[index];
    }
  }
  function collectionNames(state) {
    const names = new Set();
    for (const element of state.collect()) {
      const id = element.getAttribute("id");
      if (id) names.add(id);
      const name = state.allowsName(element) && element.getAttribute("name");
      if (name) names.add(name);
    }
    return [...names];
  }
  function makeCollection(collect, allowsName, missingValue) {
    const target = new HTMLCollection(token);
    const state = { collect, allowsName };
    const value = key => isIndex(key) ? collect()[Number(key)] :
      typeof key === "string" && !(key in target) ? namedItem(state, key) ?? undefined : undefined;
    const handler = {
      get(_target, key, receiver) {
        const item = value(key);
        if (item !== undefined) return item;
        if (isIndex(key)) return undefined;
        const result = Reflect.get(target, key, receiver);
        return result === undefined && typeof key === "string" ? missingValue : result;
      },
      has(_target, key) { return value(key) !== undefined || key in target; },
      ownKeys() {
        const indices = collect().map((_, index) => String(index));
        const names = collectionNames(state).filter(name => !isIndex(name) && !(name in target));
        return [...new Set([...indices, ...names, ...Reflect.ownKeys(target)])];
      },
      getOwnPropertyDescriptor(_target, key) {
        const item = value(key);
        return item === undefined ? Reflect.getOwnPropertyDescriptor(target, key) :
          { value: item, writable: false, enumerable: isIndex(key), configurable: true };
      },
      set(_target, key, newValue, receiver) {
        if (receiver !== proxy) return Reflect.defineProperty(receiver, key, {
          value: newValue, writable: true, enumerable: true, configurable: true,
        });
        if (isIndex(key) || value(key) !== undefined) return false;
        return Reflect.set(target, key, newValue, receiver);
      },
      defineProperty(_target, key, descriptor) {
        if (isIndex(key) || value(key) !== undefined) return false;
        return Reflect.defineProperty(target, key, descriptor);
      },
      deleteProperty(_target, key) {
        if (value(key) !== undefined) return false;
        return Reflect.deleteProperty(target, key);
      },
      preventExtensions() { return false; },
    };
    const proxy = new Proxy(target, handler);
    weakSet(collections, target, state);
    weakSet(collections, proxy, state);
    return proxy;
  }
  class DOMImplementation {
    constructor(key = undefined, operations) {
      if (key !== token) throw new TypeErrorIntrinsic("Illegal constructor");
      weakSet(implementations, this, operations);
    }
    hasFeature() { read(implementations, this); return true; }
    createDocumentType(qualifiedName, publicId, systemId) {
      if (arguments.length < 3) throw new TypeErrorIntrinsic("createDocumentType requires three arguments");
      return read(implementations, this).createDocumentType(
        domString(qualifiedName), domString(publicId), domString(systemId));
    }
    createDocument(namespace, qualifiedName, doctype = null) {
      if (arguments.length < 2) throw new TypeErrorIntrinsic("createDocument requires two arguments");
      const operations = read(implementations, this);
      const convertedNamespace = namespace == null ? null : domString(namespace);
      const convertedName = qualifiedName === null ? "" : domString(qualifiedName);
      if (doctype !== null && (!isNodeIntrinsic(doctype) || nodeTypeIntrinsic(doctype) !== 10)) {
        throw new TypeErrorIntrinsic("doctype must be a DocumentType");
      }
      return operations.createDocument(convertedNamespace, convertedName, doctype);
    }
    createHTMLDocument(title) {
      const operations = read(implementations, this);
      return title === undefined ? operations.createHTMLDocument() : operations.createHTMLDocument(domString(title));
    }
  }
  class DOMStringMap {
    constructor(key = undefined) { if (key !== token) throw new TypeErrorIntrinsic("Illegal constructor"); }
  }
  function makeDataset(node) {
    if (weakHas(datasetCache, node)) return weakGet(datasetCache, node);
    const target = new DOMStringMap(token);
    const attribute = key => "data-" + key.replace(/[A-Z]/g, letter => "-" + letter.toLowerCase());
    const names = () => Array.from(node.attributes)
      .map(attr => attr.name).filter(name => name.startsWith("data-") && !/[A-Z]/.test(name.slice(5)))
      .map(name => name.slice(5).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase()));
    const handler = {
      get(_target, key, receiver) {
        if (typeof key !== "string") return Reflect.get(target, key, receiver);
        const result = node.getAttribute(attribute(key));
        return result === null ? Reflect.get(target, key, receiver) : result;
      },
      set(_target, key, value) {
        if (typeof key !== "string") return Reflect.set(target, key, value);
        if (/-[a-z]/.test(key)) throw new DOMExceptionIntrinsic("Invalid dataset name", "SyntaxError");
        node.setAttribute(attribute(key), domString(value));
        return true;
      },
      deleteProperty(_target, key) {
        if (typeof key !== "string") return Reflect.deleteProperty(target, key);
        node.removeAttribute(attribute(key));
        return true;
      },
      has(_target, key) { return typeof key === "string" && names().includes(key) || key in target; },
      ownKeys() { return [...new Set([...names(), ...Reflect.ownKeys(target)])]; },
      getOwnPropertyDescriptor(_target, key) {
        return typeof key === "string" && names().includes(key) ?
          { value: node.getAttribute(attribute(key)), writable: true, enumerable: true, configurable: true } :
          Reflect.getOwnPropertyDescriptor(target, key);
      },
      defineProperty(_target, key, descriptor) {
        if (typeof key === "string" && "value" in descriptor) return handler.set(target, key, descriptor.value);
        return false;
      },
      preventExtensions() { return false; },
    };
    const result = new Proxy(target, handler);
    weakSet(datasetCache, node, result);
    return result;
  }
  class TextMetrics {
    constructor(key = undefined, values) {
      if (key !== token) throw new TypeErrorIntrinsic("Illegal constructor");
      weakSet(metrics, this, values);
    }
  }
  for (const name of ["width", "actualBoundingBoxLeft", "actualBoundingBoxRight", "fontBoundingBoxAscent", "fontBoundingBoxDescent",
    "actualBoundingBoxAscent", "actualBoundingBoxDescent", "emHeightAscent", "emHeightDescent", "hangingBaseline", "alphabeticBaseline", "ideographicBaseline"]) {
    Object.defineProperty(TextMetrics.prototype, name, {
      get() { return read(metrics, this)[name] ?? 0; }, configurable: true, enumerable: true,
    });
  }
  for (const type of [AbstractRange, StaticRange, HTMLCollection, DOMImplementation, DOMStringMap, TextMetrics]) {
    Object.defineProperty(type.prototype, Symbol.toStringTag, { value: type.name, configurable: true });
    for (const name of Object.getOwnPropertyNames(type.prototype)) {
      if (name !== "constructor") Object.defineProperty(type.prototype, name, { enumerable: true });
    }
    Object.defineProperty(globalThis, type.name, { value: type, writable: true, configurable: true });
  }
  globalThis.__omoikane_dom_interface_bridge = {
    bindNode: (isNode, nodeType, exception) => { isNodeIntrinsic = isNode; nodeTypeIntrinsic = nodeType; DOMExceptionIntrinsic = exception; },
    rangeToken: token,
    registerRange: (range, boundaries) => weakSet(ranges, range, boundaries),
    registerCollectionAlias: (alias, collection) => weakSet(collections, alias, read(collections, collection)),
    makeCollection, makeDataset,
    makeImplementation: operations => new DOMImplementation(token, operations),
    makeTextMetrics: values => new TextMetrics(token, values),
  };
})();
