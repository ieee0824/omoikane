// ClipboardItem representations own promises and MIME records; author expandos
// cannot change the snapshots or forge a platform Blob/ClipboardItem brand.
(() => {
  const install = globalThis.__omoikane_install_clipboard_items;
  delete globalThis.__omoikane_install_clipboard_items;
  const TypeErrorIntrinsic = TypeError, DOMExceptionIntrinsic = DOMException;
  const MapIntrinsic = Map, StringIntrinsic = String;
  const weakGet = Function.prototype.call.bind(WeakMap.prototype.get);
  const weakSet = Function.prototype.call.bind(WeakMap.prototype.set);
  const mapGet = Function.prototype.call.bind(Map.prototype.get);
  const mapSet = Function.prototype.call.bind(Map.prototype.set);
  const mapHas = Function.prototype.call.bind(Map.prototype.has);
  const resolve = Promise.resolve.bind(Promise), reject = Promise.reject.bind(Promise);
  const then = Function.prototype.call.bind(Promise.prototype.then);
  const ownKeys = Reflect.ownKeys, descriptor = Object.getOwnPropertyDescriptor;
  const freeze = Object.freeze, createObject = Object.create;
  const token = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;
  const string = value => {
    if (typeof value === "symbol") throw new TypeErrorIntrinsic("Cannot convert a Symbol to a string");
    return StringIntrinsic(value);
  };
  function dictionary(value) {
    if (value == null) return {};
    if (typeof value !== "object" && typeof value !== "function") throw new TypeErrorIntrinsic("Expected a dictionary");
    return value;
  }
  function mimeParameters(source, position) {
    const parameters = new MapIntrinsic();
    while (position < source.length) {
      position++;
      while (/[\t\n\r ]/.test(source[position] || "")) position++;
      const start = position;
      while (position < source.length && source[position] !== ";" && source[position] !== "=") position++;
      const name = source.slice(start, position).toLowerCase();
      if (source[position] !== "=") continue;
      position++;
      let value = "";
      if (source[position] === '"') {
        position++;
        while (position < source.length) {
          const character = source[position++];
          if (character === '"') break;
          if (character === "\\" && position < source.length) value += source[position++];
          else value += character;
        }
        while (position < source.length && source[position] !== ";") position++;
      } else {
        const start = position;
        while (position < source.length && source[position] !== ";") position++;
        value = source.slice(start, position).replace(/[\t\n\r ]+$/g, "");
        if (value === "") continue;
      }
      if (token.test(name) && /^[\t\x20-\x7e\x80-\xff]*$/.test(value) && !mapHas(parameters, name)) {
        mapSet(parameters, name, value);
      }
    }
    return parameters;
  }
  function mimeType(source) {
    const input = source.replace(/^[\t\n\r ]+|[\t\n\r ]+$/g, "");
    const slash = input.indexOf("/"), semicolon = input.indexOf(";", slash + 1);
    if (slash < 0) return null;
    const type = input.slice(0, slash);
    const subtype = input.slice(slash + 1, semicolon < 0 ? input.length : semicolon).replace(/[\t\n\r ]+$/g, "");
    if (!token.test(type) || !token.test(subtype)) return null;
    const parameters = mimeParameters(input, semicolon < 0 ? input.length : semicolon);
    let serialized = type.toLowerCase() + "/" + subtype.toLowerCase();
    for (const [name, raw] of parameters) {
      const value = token.test(raw) ? raw : '"' + raw.replace(/["\\]/g, "\\$&") + '"';
      serialized += ";" + name + "=" + value;
    }
    return {serialized, parameterCount:parameters.size};
  }
  function representationType(source) {
    const custom = source.startsWith("web ");
    const mime = mimeType(custom ? source.slice(4) : source);
    if (mime === null) throw new TypeErrorIntrinsic("Invalid ClipboardItem MIME type");
    return {mime:mime.serialized, key:(custom ? "web " : "") + mime.serialized};
  }
  function recordEntries(input, isBlob) {
    if (input === null || typeof input !== "object" && typeof input !== "function") {
      throw new TypeErrorIntrinsic("ClipboardItem requires a record");
    }
    const entries = [];
    for (const key of ownKeys(input)) {
      if (!descriptor(input, key)?.enumerable) continue;
      const name = string(key);
      const data = then(resolve(input[key]), value => {
        // Promise resolution must not consult author Object.prototype.then
        // for this private typed record. The public Blob still resolves normally.
        const record = createObject(null);
        const blob = isBlob(value);
        record.kind = blob ? "blob" : "string";
        record.value = blob ? value : string(value);
        return record;
      });
      // The internal representation consumes rejection even before getType is
      // requested; callers observe it through getType's NotFoundError promise.
      then(data, undefined, () => {});
      entries[entries.length] = [name, data];
    }
    return entries;
  }
  install(context => {
    if (!context.secure) return undefined;
    const state = context.states, BlobIntrinsic = context.Blob;
    const read = value => {
      const record = weakGet(state, value);
      if (!record) throw new TypeErrorIntrinsic("Illegal invocation");
      return record;
    };
    class ClipboardItem {
      constructor(items, options = {}) {
        if (!arguments.length) throw new TypeErrorIntrinsic("Missing required argument");
        const entries = recordEntries(items, context.isBlob);
        const rawStyle = dictionary(options).presentationStyle;
        const presentationStyle = rawStyle === undefined ? "unspecified" : string(rawStyle);
        if (!["unspecified", "inline", "attachment"].includes(presentationStyle)) throw new TypeErrorIntrinsic("Invalid PresentationStyle");
        if (!entries.length) throw new TypeErrorIntrinsic("ClipboardItem requires a representation");
        const representations = new MapIntrinsic(), types = [];
        for (const [name, data] of entries) {
          const type = representationType(name);
          if (mapHas(representations, type.key)) throw new TypeErrorIntrinsic("Duplicate ClipboardItem MIME type");
          mapSet(representations, type.key, {...type, data});
          types[types.length] = type.key;
        }
        weakSet(state, this, {presentationStyle, types:freeze(types), representations});
      }
      get presentationStyle() { return read(this).presentationStyle; }
      get types() { return read(this).types; }
      getType(type) {
        try {
          const record = read(this);
          if (!arguments.length) throw new TypeErrorIntrinsic("Missing required argument");
          const key = representationType(string(type)).key;
          const representation = mapGet(record.representations, key);
          if (!representation) return reject(new DOMExceptionIntrinsic("No representation for this type", "NotFoundError"));
          return then(representation.data, data => data.kind === "blob"
            ? data.value : new BlobIntrinsic([data.value], {type:key}),
          () => {throw new DOMExceptionIntrinsic("Representation data was rejected", "NotFoundError");});
        } catch (error) { return reject(error); }
      }
      static supports(type) {
        if (!arguments.length) throw new TypeErrorIntrinsic("Missing required argument");
        const value = string(type);
        if (["text/plain", "text/html", "image/png", "text/uri-list", "image/svg+xml"].includes(value)) return true;
        if (!value.startsWith("web ")) return false;
        const mime = mimeType(value.slice(4));
        return mime !== null && mime.parameterCount === 0;
      }
    }
    Object.defineProperty(ClipboardItem.prototype, Symbol.toStringTag, {value:"ClipboardItem", configurable:true});
    for (const name of ["presentationStyle", "types", "getType"]) {
      Object.defineProperty(ClipboardItem.prototype, name, {...descriptor(ClipboardItem.prototype, name), enumerable:true});
    }
    Object.defineProperty(ClipboardItem, "supports", {...descriptor(ClipboardItem, "supports"), enumerable:true});
    const getType = Function.prototype.call.bind(ClipboardItem.prototype.getType);
    return {
      constructor: ClipboardItem,
      isItem: value => weakGet(state, value) !== undefined,
      types: value => read(value).types,
      getType,
    };
  });
})();
