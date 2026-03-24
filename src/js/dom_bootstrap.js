(() => {
  const cache = new Map();

  function wrapNode(id) {
    if (id === null || id === undefined) {
      return null;
    }
    if (cache.has(id)) {
      return cache.get(id);
    }
    const nodeType = __omoikane_node_type(id);
    let node;
    if (id === __omoikane_document_id) {
      node = new Document(id);
    } else if (nodeType === 11) {
      node = new DocumentFragment(id);
    } else {
      node = new Node(id);
    }
    cache.set(id, node);
    return node;
  }

  function invokeListeners(node, event, capture, phase) {
    const listeners = (node.__listeners.get(event.type) || []).slice();
    for (const entry of listeners) {
      if (!!entry.capture === capture) {
        event.currentTarget = node;
        event.eventPhase = phase;
        entry.listener.call(node, event);
        if (event.__stoppedImmediate) {
          return true;
        }
      }
    }
    // stopPropagation prevents further propagation to other nodes
    // but does NOT prevent other listeners on the same node
    return event.__stopped;
  }

  class Event {
    constructor(type, init = {}) {
      this.type = String(type);
      this.bubbles = init.bubbles ?? true;
      this.cancelable = init.cancelable ?? false;
      this.target = null;
      this.currentTarget = null;
      this.eventPhase = 0;
      this.defaultPrevented = false;
      this.isTrusted = false;
      this.timeStamp = Date.now();
      this.__stopped = false;
      this.__stoppedImmediate = false;
    }

    stopPropagation() {
      this.__stopped = true;
    }

    stopImmediatePropagation() {
      this.__stopped = true;
      this.__stoppedImmediate = true;
    }

    preventDefault() {
      if (this.cancelable) {
        this.defaultPrevented = true;
      }
    }
  }

  class CustomEvent extends Event {
    constructor(type, init = {}) {
      super(type, init);
      this.detail = init.detail ?? null;
    }
  }

  class MouseEvent extends Event {
    constructor(type, init = {}) {
      super(type, init);
      this.clientX = init.clientX ?? 0;
      this.clientY = init.clientY ?? 0;
      this.pageX = init.pageX ?? 0;
      this.pageY = init.pageY ?? 0;
      this.screenX = init.screenX ?? 0;
      this.screenY = init.screenY ?? 0;
      this.button = init.button ?? 0;
      this.buttons = init.buttons ?? 0;
      this.altKey = init.altKey ?? false;
      this.ctrlKey = init.ctrlKey ?? false;
      this.shiftKey = init.shiftKey ?? false;
      this.metaKey = init.metaKey ?? false;
      this.relatedTarget = init.relatedTarget ?? null;
    }
  }

  class KeyboardEvent extends Event {
    constructor(type, init = {}) {
      super(type, init);
      this.key = init.key ?? "";
      this.code = init.code ?? "";
      this.keyCode = init.keyCode ?? 0;
      this.charCode = init.charCode ?? 0;
      this.repeat = init.repeat ?? false;
      this.altKey = init.altKey ?? false;
      this.ctrlKey = init.ctrlKey ?? false;
      this.shiftKey = init.shiftKey ?? false;
      this.metaKey = init.metaKey ?? false;
    }
  }

  class FocusEvent extends Event {
    constructor(type, init = {}) {
      super(type, init);
      this.relatedTarget = init.relatedTarget ?? null;
    }
  }

  class Node {
    constructor(id) {
      this.__id = id;
      this.__listeners = new Map();
    }

    appendChild(child) {
      // DOM semantics: appending a DocumentFragment appends its children
      if (child && child.nodeType === 11) {
        const children = child.childNodes.slice();
        for (const c of children) {
          __omoikane_append_child(this.__id, c.__id);
        }
        return child;
      }
      __omoikane_append_child(this.__id, child.__id);
      return child;
    }

    querySelector(selector) {
      const id = __omoikane_query_selector(this.__id, String(selector));
      return wrapNode(id);
    }

    addEventListener(type, listener, options = false) {
      const capture = typeof options === "boolean" ? options : !!options.capture;
      const key = String(type);
      const list = this.__listeners.get(key) ?? [];
      // Deduplicate: same listener+capture is only registered once (DOM spec).
      if (!list.some(entry => entry.listener === listener && !!entry.capture === capture)) {
        list.push({ listener, capture });
      }
      this.__listeners.set(key, list);
    }

    removeEventListener(type, listener, options = false) {
      const capture = typeof options === "boolean" ? options : !!options.capture;
      const key = String(type);
      const list = this.__listeners.get(key);
      if (!list) return;
      const index = list.findIndex(entry => entry.listener === listener && !!entry.capture === capture);
      if (index !== -1) list.splice(index, 1);
    }

    dispatchEvent(event) {
      const dispatchEvent = event instanceof Event ? event : new Event(event);
      dispatchEvent.target = this;

      const path = [];
      let current = this;
      while (current) {
        path.push(current);
        current = current.parentNode;
      }

      for (let i = path.length - 1; i >= 1; i -= 1) {
        if (invokeListeners(path[i], dispatchEvent, true, 1)) {
          return false;
        }
      }

      if (invokeListeners(this, dispatchEvent, true, 2)) {
        return false;
      }
      if (invokeListeners(this, dispatchEvent, false, 2)) {
        return false;
      }

      if (dispatchEvent.bubbles) {
        for (let i = 1; i < path.length; i += 1) {
          if (invokeListeners(path[i], dispatchEvent, false, 3)) {
            return false;
          }
        }
      }

      return !dispatchEvent.defaultPrevented;
    }

    get parentNode() {
      return wrapNode(__omoikane_parent_node(this.__id));
    }

    get nodeName() {
      return __omoikane_node_name(this.__id);
    }

    get id() {
      return __omoikane_get_attribute(this.__id, "id");
    }

    set id(value) {
      __omoikane_set_attribute(this.__id, "id", String(value));
    }

    getAttribute(name) {
      return __omoikane_get_attribute(this.__id, String(name));
    }

    setAttribute(name, value) {
      __omoikane_set_attribute(this.__id, String(name), String(value));
    }

    get className() {
      return __omoikane_get_attribute(this.__id, "class") || "";
    }

    set className(value) {
      __omoikane_set_attribute(this.__id, "class", String(value));
    }

    get classList() {
      const node = this;
      return {
        add(...classes) {
          const current = new Set((node.className || "").split(/\s+/).filter(Boolean));
          for (const cls of classes) current.add(cls);
          node.className = [...current].join(" ");
        },
        remove(...classes) {
          const current = new Set((node.className || "").split(/\s+/).filter(Boolean));
          for (const cls of classes) current.delete(cls);
          node.className = [...current].join(" ");
        },
        toggle(cls, force) {
          const current = new Set((node.className || "").split(/\s+/).filter(Boolean));
          const has = current.has(cls);
          if (force === undefined) {
            has ? current.delete(cls) : current.add(cls);
          } else if (force) {
            current.add(cls);
          } else {
            current.delete(cls);
          }
          node.className = [...current].join(" ");
          return current.has(cls);
        },
        contains(cls) {
          return (node.className || "").split(/\s+/).filter(Boolean).includes(cls);
        },
        get length() {
          return (node.className || "").split(/\s+/).filter(Boolean).length;
        },
        item(index) {
          return (node.className || "").split(/\s+/).filter(Boolean)[index] || null;
        },
      };
    }

    get style() {
      const node = this;
      return new Proxy({}, {
        get(target, prop) {
          if (typeof prop !== "string") return undefined;
          const kebab = prop.replace(/[A-Z]/g, m => "-" + m.toLowerCase());
          const styleAttr = __omoikane_get_attribute(node.__id, "style") || "";
          const escaped = kebab.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
          const match = styleAttr.match(new RegExp("(?:^|;\\s*)" + escaped + "\\s*:\\s*([^;]+)"));
          return match ? match[1].trim() : "";
        },
        set(target, prop, value) {
          if (typeof prop !== "string") return true;
          const kebab = prop.replace(/[A-Z]/g, m => "-" + m.toLowerCase());
          const shouldRemove = value === null || value === undefined || value === "";
          const strValue = shouldRemove ? "" : String(value);
          let styleAttr = __omoikane_get_attribute(node.__id, "style") || "";
          const escaped = kebab.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
          const regex = new RegExp("(^|;\\s*)" + escaped + "\\s*:[^;]+(;|$)");
          if (regex.test(styleAttr)) {
            if (shouldRemove) {
              styleAttr = styleAttr.replace(regex, "$1");
            } else {
              styleAttr = styleAttr.replace(regex, (m, pre) => pre + kebab + ": " + strValue + ";");
            }
          } else if (!shouldRemove) {
            styleAttr = (styleAttr ? styleAttr.replace(/;?\s*$/, "; ") : "") + kebab + ": " + strValue + ";";
          }
          styleAttr = styleAttr.replace(/^[;\s]+/, "").replace(/[;\s]+$/, "").replace(/;\s*;+/g, ";");
          __omoikane_set_attribute(node.__id, "style", styleAttr.trim());
          return true;
        },
      });
    }

    get textContent() {
      return __omoikane_get_text_content(this.__id);
    }

    set textContent(value) {
      const text = value == null ? "" : String(value);
      __omoikane_set_text_content(this.__id, text);
    }

    get innerHTML() {
      return __omoikane_get_inner_html(this.__id) || "";
    }

    set innerHTML(value) {
      const html = value == null ? "" : String(value);
      __omoikane_set_inner_html(this.__id, html);
    }

    get childNodes() {
      const ids = __omoikane_child_node_ids(this.__id);
      return ids ? ids.map(id => wrapNode(id)) : [];
    }

    get children() {
      return this.childNodes.filter(n => n.nodeType === 1);
    }

    get firstChild() {
      const ids = __omoikane_child_node_ids(this.__id);
      return ids && ids.length > 0 ? wrapNode(ids[0]) : null;
    }

    get lastChild() {
      const ids = __omoikane_child_node_ids(this.__id);
      return ids && ids.length > 0 ? wrapNode(ids[ids.length - 1]) : null;
    }

    get nextSibling() {
      return wrapNode(__omoikane_next_sibling(this.__id));
    }

    get previousSibling() {
      return wrapNode(__omoikane_previous_sibling(this.__id));
    }

    removeChild(child) {
      __omoikane_remove_child(this.__id, child.__id);
      return child;
    }

    insertBefore(newNode, refNode) {
      __omoikane_insert_before(this.__id, newNode.__id, refNode ? refNode.__id : null);
      return newNode;
    }

    querySelectorAll(selector) {
      const ids = __omoikane_query_selector_all(this.__id, String(selector));
      return ids ? ids.map(id => wrapNode(id)) : [];
    }

    getElementsByTagName(tag) {
      return this.querySelectorAll(String(tag));
    }

    getElementsByClassName(cls) {
      return this.querySelectorAll("." + String(cls));
    }

    get ownerDocument() {
      return this.nodeType === 9 ? null : globalThis.document;
    }

    get nodeType() {
      return __omoikane_node_type(this.__id);
    }

    cloneNode(deep = false) {
      return wrapNode(__omoikane_clone_node(this.__id, !!deep));
    }

    hasAttribute(name) {
      return __omoikane_get_attribute(this.__id, String(name)) !== null;
    }

    removeAttribute(name) {
      __omoikane_remove_attribute(this.__id, String(name));
    }

    get tagName() {
      if (this.nodeType !== 1) {
        return undefined;
      }
      const name = __omoikane_node_name(this.__id);
      return name ? name.toUpperCase() : undefined;
    }

    contains(other) {
      if (!other) return false;
      let current = other;
      while (current) {
        if (current.__id === this.__id) return true;
        current = current.parentNode;
      }
      return false;
    }

    get innerText() {
      return this.textContent;
    }

    set innerText(value) {
      this.textContent = value;
    }

    get isConnected() {
      let current = this;
      while (current) {
        if (current.nodeType === 9) return true;
        current = current.parentNode;
      }
      return false;
    }

    get attributes() {
      const node = this;
      // NamedNodeMap-like object backed by getAttribute/setAttribute
      return new Proxy([], {
        get(target, prop) {
          if (prop === "length") {
            // Count by getting all via style hack — not ideal but functional
            return 0;
          }
          if (prop === "getNamedItem") {
            return function(name) {
              const val = node.getAttribute(name);
              return val !== null ? { name, value: val, specified: true } : null;
            };
          }
          if (prop === "setNamedItem") {
            return function(attr) { node.setAttribute(attr.name, attr.value); };
          }
          if (prop === "removeNamedItem") {
            return function(name) { node.removeAttribute(name); };
          }
          if (typeof prop === "string" && !isNaN(prop)) {
            return undefined;
          }
          // Named access: attributes["data-foo"]
          if (typeof prop === "string") {
            const val = node.getAttribute(prop);
            return val !== null ? { name: prop, value: val, specified: true, expando: false } : undefined;
          }
          return undefined;
        }
      });
    }

    get dataset() {
      const node = this;
      return new Proxy({}, {
        get(target, prop) {
          if (typeof prop !== "string") return undefined;
          const attrName = "data-" + prop.replace(/[A-Z]/g, m => "-" + m.toLowerCase());
          const val = node.getAttribute(attrName);
          return val === null ? undefined : val;
        },
        set(target, prop, value) {
          if (typeof prop !== "string") return true;
          const attrName = "data-" + prop.replace(/[A-Z]/g, m => "-" + m.toLowerCase());
          node.setAttribute(attrName, String(value));
          return true;
        },
        deleteProperty(target, prop) {
          if (typeof prop !== "string") return true;
          const attrName = "data-" + prop.replace(/[A-Z]/g, m => "-" + m.toLowerCase());
          node.removeAttribute(attrName);
          return true;
        }
      });
    }

    get nodeValue() {
      const t = this.nodeType;
      if (t === 3 || t === 8) return this.textContent;
      return null;
    }

    set nodeValue(value) {
      const t = this.nodeType;
      if (t === 3 || t === 8) this.textContent = value;
    }

    replaceChild(newChild, oldChild) {
      if (!oldChild || !oldChild.__id) {
        throw new Error("Failed to execute 'replaceChild': parameter is not a Node");
      }
      this.insertBefore(newChild, oldChild);
      this.removeChild(oldChild);
      return oldChild;
    }

    get firstElementChild() {
      return this.children[0] || null;
    }

    get lastElementChild() {
      const c = this.children;
      return c.length > 0 ? c[c.length - 1] : null;
    }

    get childElementCount() {
      return this.children.length;
    }

    get nextElementSibling() {
      let s = this.nextSibling;
      while (s) {
        if (s.nodeType === 1) return s;
        s = s.nextSibling;
      }
      return null;
    }

    get previousElementSibling() {
      let s = this.previousSibling;
      while (s) {
        if (s.nodeType === 1) return s;
        s = s.previousSibling;
      }
      return null;
    }

    closest(selector) {
      let current = this;
      while (current) {
        if (current.nodeType === 1 && current.matches(selector)) return current;
        current = current.parentNode;
      }
      return null;
    }

    matches(selector) {
      if (!selector || this.nodeType !== 1) return false;
      const sel = selector.trim();
      // Tag selector
      if (/^[a-zA-Z][a-zA-Z0-9]*$/.test(sel)) {
        return this.tagName && this.tagName.toLowerCase() === sel.toLowerCase();
      }
      // ID selector
      if (sel.startsWith("#")) {
        return this.id === sel.slice(1);
      }
      // Class selector
      if (sel.startsWith(".")) {
        return this.classList && this.classList.contains(sel.slice(1));
      }
      // Attribute selector [attr] or [attr="value"]
      const attrMatch = /^\[([^\s=\]]+)(?:="([^"]*)")?\]$/.exec(sel);
      if (attrMatch) {
        if (!this.hasAttribute(attrMatch[1])) return false;
        if (attrMatch[2] !== undefined) return this.getAttribute(attrMatch[1]) === attrMatch[2];
        return true;
      }
      // Fallback: use querySelectorAll on parent
      const parent = this.parentNode || this.ownerDocument;
      if (!parent) return false;
      const all = parent.querySelectorAll(sel);
      for (let i = 0; i < all.length; i++) {
        if (all[i].__id === this.__id) return true;
      }
      return false;
    }

    getBoundingClientRect() {
      return { x: 0, y: 0, width: 0, height: 0, top: 0, left: 0, bottom: 0, right: 0 };
    }

    getClientRects() {
      return [];
    }

    get offsetWidth() { return 0; }
    get offsetHeight() { return 0; }
    get offsetTop() { return 0; }
    get offsetLeft() { return 0; }
    get clientWidth() { return 0; }
    get clientHeight() { return 0; }
    get clientTop() { return 0; }
    get clientLeft() { return 0; }
    get scrollWidth() { return 0; }
    get scrollHeight() { return 0; }
    get scrollTop() { return 0; }
    set scrollTop(v) {}
    get scrollLeft() { return 0; }
    set scrollLeft(v) {}
    get offsetParent() { return null; }

    focus() {}
    blur() {}
    click() { this.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true })); }

    get hidden() {
      return this.hasAttribute("hidden");
    }

    set hidden(value) {
      if (value) this.setAttribute("hidden", "");
      else this.removeAttribute("hidden");
    }

    get value() {
      return __omoikane_get_attribute(this.__id, "value") ?? "";
    }

    set value(v) {
      __omoikane_set_attribute(this.__id, "value", String(v));
    }

    get checked() {
      return this.hasAttribute("checked");
    }

    set checked(v) {
      if (v) this.setAttribute("checked", "");
      else this.removeAttribute("checked");
    }

    get disabled() {
      return this.hasAttribute("disabled");
    }

    set disabled(v) {
      if (v) this.setAttribute("disabled", "");
      else this.removeAttribute("disabled");
    }

    get type() {
      return this.getAttribute("type") || "";
    }

    set type(v) {
      this.setAttribute("type", String(v));
    }
  }

  class Document extends Node {
    getElementById(id) {
      return wrapNode(__omoikane_get_element_by_id(String(id)));
    }

    createElement(tag) {
      return wrapNode(__omoikane_create_element(String(tag)));
    }

    createDocumentFragment() {
      return wrapNode(__omoikane_create_document_fragment());
    }

    createTextNode(text) {
      return wrapNode(__omoikane_create_text_node(String(text)));
    }

    createComment(data) {
      return wrapNode(__omoikane_create_comment(String(data ?? "")));
    }

    createEvent(type) {
      const evt = new Event("");
      evt.initEvent = function(t, bubbles, cancelable) {
        this.type = String(t);
        this.bubbles = !!bubbles;
        this.cancelable = !!cancelable;
      };
      return evt;
    }

    get body() {
      return this.querySelector("body");
    }

    get head() {
      return this.querySelector("head");
    }

    get documentElement() {
      return this.querySelector("html");
    }

    get readyState() {
      return "complete";
    }

    get characterSet() {
      return "UTF-8";
    }

    get contentType() {
      return "text/html";
    }

    get URL() {
      return globalThis.location.href;
    }

    get documentURI() {
      return globalThis.location.href;
    }

    get compatMode() {
      return "CSS1Compat";
    }

    hasFocus() {
      return true;
    }

    getElementsByTagName(tag) {
      return this.querySelectorAll(String(tag));
    }

    getElementsByClassName(cls) {
      return this.querySelectorAll("." + String(cls));
    }
  }

  class DocumentFragment extends Node {}

  globalThis.Node = Node;
  globalThis.Element = Node;
  globalThis.HTMLElement = Node;
  globalThis.Document = Document;
  globalThis.DocumentFragment = DocumentFragment;
  globalThis.Event = Event;
  globalThis.CustomEvent = CustomEvent;
  globalThis.MouseEvent = MouseEvent;
  globalThis.KeyboardEvent = KeyboardEvent;
  globalThis.FocusEvent = FocusEvent;
  globalThis.UIEvent = Event;
  globalThis.InputEvent = Event;
  globalThis.WheelEvent = MouseEvent;
  globalThis.PointerEvent = MouseEvent;
  globalThis.TouchEvent = Event;
  globalThis.AnimationEvent = Event;
  globalThis.TransitionEvent = Event;
  globalThis.document = wrapNode(__omoikane_document_id);
  globalThis.window = globalThis;
  globalThis.self = globalThis;
  globalThis.addEventListener = function(type, listener, options) {
    return document.addEventListener(type, listener, options);
  };
  globalThis.removeEventListener = function(type, listener, options) {
    return document.removeEventListener(type, listener, options);
  };
  globalThis.dispatchEvent = function(event) {
    return document.dispatchEvent(event);
  };
  const __loc = { href: __omoikane_location_href, protocol: "", hostname: "", pathname: "/", search: "", hash: "", origin: "", host: "" };
  try {
    const __m = String(__omoikane_location_href).match(/^(.*?):\/\/([^/?#]+)([^?#]*)(\?[^#]*)?(#.*)?$/);
    if (__m) {
      __loc.protocol = (__m[1] || "") + ":";
      __loc.host = __m[2] || "";
      __loc.hostname = (__m[2] || "").replace(/:\d+$/, "");
      __loc.pathname = __m[3] || "/";
      __loc.search = __m[4] || "";
      __loc.hash = __m[5] || "";
      __loc.origin = __loc.protocol + "//" + __loc.host;
    }
  } catch(e) {}
  globalThis.location = __loc;
  globalThis.getComputedStyle = function() { return new Proxy({}, { get() { return ""; } }); };
  globalThis.navigator = { userAgent: __omoikane_navigator_user_agent, language: "en", languages: ["en"], platform: "", cookieEnabled: false, onLine: true };
  globalThis.console = {
    log: (...args) => __omoikane_console_log(...args),
    warn: (...args) => __omoikane_console_log("[warn]", ...args),
    error: (...args) => __omoikane_console_log("[error]", ...args),
    info: (...args) => __omoikane_console_log("[info]", ...args),
    debug: () => {},
    trace: () => {},
    dir: () => {},
    table: () => {},
    group: () => {},
    groupEnd: () => {},
    time: () => {},
    timeEnd: () => {},
    assert: () => {},
    count: () => {},
    clear: () => {},
  };
  globalThis.alert = function() {};
  globalThis.confirm = function() { return false; };
  globalThis.prompt = function() { return null; };
  globalThis.innerWidth = 1280;
  globalThis.innerHeight = 720;
  globalThis.outerWidth = 1280;
  globalThis.outerHeight = 720;
  globalThis.screenX = 0;
  globalThis.screenY = 0;
  globalThis.devicePixelRatio = 1;
  globalThis.scrollX = 0;
  globalThis.scrollY = 0;
  globalThis.pageXOffset = 0;
  globalThis.pageYOffset = 0;
  globalThis.scrollTo = function() {};
  globalThis.scrollBy = function() {};
  globalThis.scroll = function() {};
  globalThis.screen = { width: 1280, height: 720, availWidth: 1280, availHeight: 720, colorDepth: 24, pixelDepth: 24 };

  // requestAnimationFrame: execute callback asynchronously (microtask) with a timestamp
  let __rafId = 0;
  const __rafCallbacks = new Map();
  globalThis.requestAnimationFrame = function(cb) {
    const id = ++__rafId;
    __rafCallbacks.set(id, cb);
    Promise.resolve().then(() => {
      const callback = __rafCallbacks.get(id);
      if (typeof callback === "function") {
        callback(Date.now());
      }
      __rafCallbacks.delete(id);
    });
    return id;
  };
  globalThis.cancelAnimationFrame = function(id) {
    __rafCallbacks.delete(id);
  };

  // matchMedia stub: always returns matches=false
  globalThis.matchMedia = function(query) {
    return {
      matches: false,
      media: String(query),
      onchange: null,
      addListener: function() {},
      removeListener: function() {},
      addEventListener: function() {},
      removeEventListener: function() {},
      dispatchEvent: function() { return true; },
    };
  };

  // In-memory storage stubs
  function createStorage() {
    const store = new Map();
    return {
      getItem(key) { return store.has(key) ? store.get(key) : null; },
      setItem(key, value) { store.set(key, String(value)); },
      removeItem(key) { store.delete(key); },
      clear() { store.clear(); },
      key(index) { return [...store.keys()][index] ?? null; },
      get length() { return store.size; },
    };
  }
  globalThis.localStorage = createStorage();
  globalThis.sessionStorage = createStorage();

  // MutationObserver stub
  globalThis.MutationObserver = class MutationObserver {
    constructor(callback) { this._callback = callback; }
    observe() {}
    disconnect() {}
    takeRecords() { return []; }
  };

  // ResizeObserver stub
  globalThis.ResizeObserver = class ResizeObserver {
    constructor(callback) { this._callback = callback; }
    observe() {}
    unobserve() {}
    disconnect() {}
  };

  // Performance stub
  globalThis.performance = {
    now: () => Date.now(),
    timing: {},
    getEntriesByType: () => [],
    getEntriesByName: () => [],
    mark: () => {},
    measure: () => {},
  };

  // DOMParser stub
  globalThis.DOMParser = class DOMParser {
    parseFromString() { return globalThis.document; }
  };

  // URL and URLSearchParams (basic)
  if (!globalThis.URLSearchParams) {
    globalThis.URLSearchParams = class URLSearchParams {
      constructor(init) {
        this._params = new Map();
        if (typeof init === "string") {
          init.replace(/^\?/, "").split("&").forEach(pair => {
            const [k, v] = pair.split("=");
            if (k) this._params.set(decodeURIComponent(k), decodeURIComponent(v || ""));
          });
        }
      }
      get(name) { return this._params.get(name) ?? null; }
      set(name, value) { this._params.set(name, String(value)); }
      has(name) { return this._params.has(name); }
      delete(name) { this._params.delete(name); }
      toString() { return [...this._params].map(([k, v]) => encodeURIComponent(k) + "=" + encodeURIComponent(v)).join("&"); }
      forEach(cb) { this._params.forEach((v, k) => cb(v, k, this)); }
    };
  }
  globalThis.fetch = function(url) {
    return Promise.resolve(__omoikane_fetch(String(url))).then(raw => {
      const data = JSON.parse(String(raw));
      return {
        status: data.status,
        ok: data.ok,
        url: data.url,
        text() {
          return Promise.resolve(data.bodyText);
        },
      };
    });
  };

  // IntersectionObserver polyfill for headless rendering.
  // All elements are assumed to be within the viewport, so observe()
  // immediately invokes the callback with isIntersecting: true.
  const emptyRect = Object.freeze({ x: 0, y: 0, width: 0, height: 0, top: 0, left: 0, bottom: 0, right: 0 });

  class IntersectionObserverEntry {
    constructor(target) {
      this.target = target;
      this.isIntersecting = true;
      this.intersectionRatio = 1.0;
      this.boundingClientRect = emptyRect;
      this.intersectionRect = emptyRect;
      this.rootBounds = null;
      this.time = Date.now();
    }
  }

  if (!globalThis.IntersectionObserverEntry) {
    globalThis.IntersectionObserverEntry = IntersectionObserverEntry;
  }

  if (!globalThis.IntersectionObserver) {
    globalThis.IntersectionObserver = class IntersectionObserver {
    constructor(callback, options = {}) {
      if (typeof callback !== "function") {
        throw new TypeError("IntersectionObserver constructor: callback must be a function");
      }
      this._callback = callback;
      this._options = options;
      this._targets = new Set();
    }

    observe(target) {
      if (this._targets.has(target)) return;
      this._targets.add(target);
      // Schedule callback asynchronously (microtask) to match real browser behavior.
      // Re-check that target is still observed when microtask runs.
      Promise.resolve().then(() => {
        if (!this._targets.has(target)) return;
        this._callback([new IntersectionObserverEntry(target)], this);
      });
    }

    unobserve(target) {
      this._targets.delete(target);
    }

    disconnect() {
      this._targets.clear();
    }

    takeRecords() {
      return [];
    }
  };
  } // end if (!globalThis.IntersectionObserver)
})();
