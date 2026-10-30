// Window, Document, Navigator, Location and Performance members that describe
// the browsing context rather than DOM state (Issue #1291). Runs after the DOM
// bootstrap in each Document Realm.
(() => {
  const nativeHardwareConcurrency = globalThis.__omoikane_navigator_hardware_concurrency;
  const nativeWebdriverActive = globalThis.__omoikane_webdriver_active;
  const nativeLastModified = globalThis.__omoikane_document_last_modified;
  const nativeDocumentOrigin = globalThis.__omoikane_document_origin;
  const nativeDocumentDomain = globalThis.__omoikane_document_domain;
  const nativeAncestorOrigins = globalThis.__omoikane_ancestor_origins;
  const userActivationState = omoikaneUserActivationState;
  if (typeof nativeDocumentOrigin !== "function" || typeof globalThis.Window !== "function") return;

  const constructionToken = {};
  const realmDocumentId = __omoikane_document_id;
  const hasOwn = (target, name) => Object.prototype.hasOwnProperty.call(target, name);
  const requireArguments = (args, count, name) => {
    if (args.length < count) {
      throw new TypeError(`${name} requires ${count} arguments, but only ${args.length} present`);
    }
  };
  // Web IDL `long` conversion (ToInt32 after ToNumber).
  const toLong = value => Number(value) | 0;
  // Web IDL attribute: an enumerable accessor named "get x" / "set x".
  const attribute = (target, name, get, set) => {
    Object.defineProperty(get, "name", { value: `get ${name}` });
    if (set) Object.defineProperty(set, "name", { value: `set ${name}` });
    Object.defineProperty(target, name, { configurable: true, enumerable: true, get, set });
  };
  const operation = (target, name, value) => Object.defineProperty(target, name, {
    configurable: true, enumerable: true, writable: true, value,
  });
  // Shapes a class as a Web IDL interface without a constructor: length 0,
  // enumerable members and a non-enumerable global binding.
  const exposeInterface = (constructor) => {
    Object.defineProperty(constructor, "length", { value: 0 });
    const prototype = constructor.prototype;
    for (const key of Object.getOwnPropertyNames(prototype)) {
      if (key === "constructor") continue;
      Object.defineProperty(prototype, key, {
        ...Object.getOwnPropertyDescriptor(prototype, key), enumerable: true,
      });
    }
    Object.defineProperty(prototype, Symbol.toStringTag, { configurable: true, value: constructor.name });
    Object.defineProperty(globalThis, constructor.name, {
      configurable: true, enumerable: false, writable: true, value: constructor,
    });
  };
  // Platform objects carry their interface brand; members reject other receivers.
  const brands = new WeakMap();
  const requireBrand = (object, constructor) => {
    if (brands.get(object) !== constructor) throw new TypeError("Illegal invocation");
  };
  const create = (constructor, ...args) => {
    const object = new constructor(constructionToken, ...args);
    brands.set(object, constructor);
    return object;
  };

  // DOMStringList is a read-only, indexable list of strings.
  const stringLists = new WeakMap();
  class DOMStringList {
    constructor(token, values) {
      if (token !== constructionToken) throw new TypeError("Illegal constructor");
      const frozen = Object.freeze(Array.from(values, String));
      stringLists.set(this, frozen);
      frozen.forEach((value, index) => Object.defineProperty(this, index, {
        configurable: false, enumerable: true, writable: false, value,
      }));
    }
    get length() {
      requireBrand(this, DOMStringList);
      return stringLists.get(this).length;
    }
    item(index) {
      requireBrand(this, DOMStringList);
      requireArguments(arguments, 1, "DOMStringList.item");
      return stringLists.get(this)[Number(index) >>> 0] ?? null;
    }
    contains(string) {
      requireBrand(this, DOMStringList);
      requireArguments(arguments, 1, "DOMStringList.contains");
      return stringLists.get(this).includes(String(string));
    }
  }
  exposeInterface(DOMStringList);
  Object.defineProperty(DOMStringList.prototype, Symbol.iterator, {
    configurable: true, writable: true, value: Array.prototype.values,
  });

  // Location.ancestorOrigins is [LegacyUnforgeable] and fixed for the
  // Location's Document.
  const location = globalThis.location;
  if (location && typeof nativeAncestorOrigins === "function") {
    let ancestorOrigins;
    const get = function () {
      if (this !== location) throw new TypeError("Illegal invocation");
      ancestorOrigins ||= create(DOMStringList, nativeAncestorOrigins(realmDocumentId));
      return ancestorOrigins;
    };
    Object.defineProperty(get, "name", { value: "get ancestorOrigins" });
    Object.defineProperty(location, "ancestorOrigins", { configurable: false, enumerable: true, get });
  }

  // BarProp reports whether a browser UI element is shown. Omoikane never opens
  // a window as a popup (window.open features are not applied), so each is
  // visible, as HTML specifies for a non-popup top-level traversable.
  class BarProp {
    constructor(token) {
      if (token !== constructionToken) throw new TypeError("Illegal constructor");
    }
    get visible() {
      requireBrand(this, BarProp);
      return true;
    }
  }
  exposeInterface(BarProp);

  // window.external keeps only the obsolete no-op methods HTML still requires.
  class External {
    constructor(token) {
      if (token !== constructionToken) throw new TypeError("Illegal constructor");
    }
    AddSearchProvider() { requireBrand(this, External); }
    IsSearchProviderInstalled() { requireBrand(this, External); }
  }
  exposeInterface(External);

  // Window is a [Global] interface: its attributes and operations are own
  // properties of the global object, and accept the global (or an omitted
  // receiver) as `this`. Auxiliary Windows install their own close/closed
  // members bound to the auxiliary context before this runs.
  const requireWindow = receiver => {
    if (receiver === undefined || receiver === null || receiver === globalThis ||
        receiver instanceof Window) return;
    throw new TypeError("Illegal invocation");
  };
  const windowAttribute = (name, get, set) => attribute(globalThis, name, function () {
    requireWindow(this);
    return get();
  }, set && function (value) {
    requireWindow(this);
    set(value);
  });
  // Web IDL [Replaceable]: assignment shadows the attribute with a data property.
  const windowReplaceable = (name, get) => windowAttribute(name, get, function (value) {
    Object.defineProperty(globalThis, name, { configurable: true, enumerable: true, writable: true, value });
  });
  const windowOperation = (name, body, length = 0) => {
    const method = {
      [name](...args) {
        requireWindow(this);
        return body(...args);
      },
    }[name];
    Object.defineProperty(method, "length", { value: length });
    operation(globalThis, name, method);
  };
  const windowMembers = new Map();
  for (const name of ["locationbar", "menubar", "personalbar", "scrollbars", "statusbar", "toolbar"]) {
    const bar = create(BarProp);
    windowMembers.set(name, () => windowReplaceable(name, () => bar));
  }
  const external = create(External);
  windowMembers.set("external", () => windowReplaceable("external", () => external));
  windowMembers.set("origin", () => windowReplaceable("origin", () => nativeDocumentOrigin(realmDocumentId)));
  windowMembers.set("clientInformation", () =>
    windowReplaceable("clientInformation", () => globalThis.navigator));
  // Agent clusters are origin-keyed; see document.domain below.
  windowMembers.set("originAgentCluster", () => windowAttribute("originAgentCluster", () => true));
  let status = "";
  windowMembers.set("status", () =>
    windowAttribute("status", () => status, value => { status = String(value); }));
  // A top-level or nested Window that was not opened by script stays open:
  // close() is ignored for it, so `closed` remains false.
  windowMembers.set("closed", () => windowAttribute("closed", () => false));
  windowMembers.set("close", () => windowOperation("close", () => {}));
  // Omoikane commits a Document only after its response has been loaded, so
  // no fetch remains for stop() to abort.
  windowMembers.set("stop", () => windowOperation("stop", () => {}));
  // Omoikane has no print layout or print dialog. print() still runs the
  // observable part of the HTML printing steps: beforeprint, then afterprint.
  windowMembers.set("print", () => windowOperation("print", () => {
    const doc = globalThis.document;
    if (!doc || doc.defaultView === null) return;
    globalThis.dispatchEvent(new Event("beforeprint"));
    globalThis.dispatchEvent(new Event("afterprint"));
  }));
  // Legacy no-op operations kept by HTML.
  for (const name of ["captureEvents", "releaseEvents"]) {
    windowMembers.set(name, () => windowOperation(name, () => {}));
  }
  // CSSOM View lets the user agent ignore window geometry requests for a
  // Window that was not opened as an auxiliary popup with its own window.
  // Arguments are still converted, as Web IDL requires.
  for (const name of ["moveTo", "moveBy", "resizeTo", "resizeBy"]) {
    windowMembers.set(name, () => windowOperation(name, (...args) => {
      requireArguments(args, 2, `Window.${name}`);
      toLong(args[0]);
      toLong(args[1]);
    }, 2));
  }
  for (const [name, install] of windowMembers) {
    if (!hasOwn(globalThis, name)) install();
  }

  // UserActivation exposes the same transient and sticky activation that
  // gates fullscreen, focus, pickers, Picture-in-Picture and Storage Access.
  class UserActivation {
    constructor(token) {
      if (token !== constructionToken) throw new TypeError("Illegal constructor");
    }
    get hasBeenActive() {
      requireBrand(this, UserActivation);
      return userActivationState()[0];
    }
    get isActive() {
      requireBrand(this, UserActivation);
      return userActivationState()[1];
    }
  }
  exposeInterface(UserActivation);
  if (typeof globalThis.Navigator === "function") {
    const prototype = Navigator.prototype;
    const navigatorAttribute = (name, value) => attribute(prototype, name, function () {
      if (!(this instanceof Navigator)) throw new TypeError("Illegal invocation");
      return value();
    });
    const concurrency = Number(nativeHardwareConcurrency) || 1;
    navigatorAttribute("hardwareConcurrency", () => concurrency);
    // No touch input device is attached to an Omoikane page.
    navigatorAttribute("maxTouchPoints", () => 0);
    navigatorAttribute("webdriver", () => nativeWebdriverActive() === true);
    // navigator.plugins is empty, so no PDF viewer is advertised.
    navigatorAttribute("pdfViewerEnabled", () => false);
    const userActivation = create(UserActivation);
    navigatorAttribute("userActivation", () => userActivation);
  }

  if (typeof globalThis.Document === "function") {
    const prototype = Document.prototype;
    const pad = (value, width = 2) => String(value).padStart(width, "0");
    // HTML: "MM/DD/YYYY hh:mm:ss" in the user's local time zone.
    const formatLastModified = time => {
      const date = new Date(time);
      return `${pad(date.getMonth() + 1)}/${pad(date.getDate())}/${pad(date.getFullYear(), 4)} ` +
        `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
    };
    const browsingDocumentId = doc => {
      if (!(doc instanceof Document)) throw new TypeError("Illegal invocation");
      return doc.defaultView === null ? null : doc.__id;
    };
    // HTMLCollection attributes are enumerable Web IDL members.
    Object.defineProperty(prototype, "scripts", {
      ...Object.getOwnPropertyDescriptor(prototype, "scripts"), enumerable: true,
    });
    attribute(prototype, "lastModified", function () {
      const id = browsingDocumentId(this);
      const time = id === null ? null : nativeLastModified(id);
      return formatLastModified(time === null ? Date.now() : time);
    });
    // Every agent cluster is origin-keyed, so a permitted value leaves the
    // origin unchanged and same-origin checks keep using the tuple origin.
    attribute(prototype, "domain", function () {
      // A Document without a browsing context has its creator's origin.
      return String(nativeDocumentDomain(browsingDocumentId(this) ?? realmDocumentId));
    }, function (value) {
      const id = browsingDocumentId(this);
      if (id === null || nativeDocumentDomain(id, String(value)) !== true) {
        throw new DOMException("The document.domain value is not allowed", "SecurityError");
      }
    });
  }

  const performance = globalThis.performance;
  if (performance && typeof performance.toJSON !== "function") {
    operation(performance, "toJSON", function toJSON() {
      const json = { timeOrigin: this.timeOrigin };
      for (const name of ["timing", "navigation"]) {
        const value = this[name];
        if (value && typeof value === "object") {
          json[name] = typeof value.toJSON === "function" ? value.toJSON() : { ...value };
        }
      }
      return json;
    });
  }
})();
