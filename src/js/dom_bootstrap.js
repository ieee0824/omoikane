(() => {
  const cache = new Map();

  function wrapNode(id) {
    if (id === null || id === undefined) {
      return null;
    }
    if (cache.has(id)) {
      return cache.get(id);
    }
    const node = id === __omoikane_document_id ? new Document(id) : new Node(id);
    cache.set(id, node);
    return node;
  }

  function invokeListeners(node, event, capture, phase) {
    const listeners = node.__listeners.get(event.type) || [];
    for (const entry of listeners) {
      if (!!entry.capture === capture) {
        event.currentTarget = node;
        event.eventPhase = phase;
        entry.listener.call(node, event);
        if (event.__stopped) {
          return true;
        }
      }
    }
    return false;
  }

  class Event {
    constructor(type, init = {}) {
      this.type = String(type);
      this.bubbles = init.bubbles ?? true;
      this.cancelable = init.cancelable ?? false;
      this.target = null;
      this.currentTarget = null;
      this.eventPhase = 0;
      this.__stopped = false;
    }

    stopPropagation() {
      this.__stopped = true;
    }
  }

  class Node {
    constructor(id) {
      this.__id = id;
      this.__listeners = new Map();
    }

    appendChild(child) {
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

      return true;
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

    get ownerDocument() {
      return globalThis.document;
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

    get body() {
      return this.querySelector("body");
    }

    get head() {
      return this.querySelector("head");
    }

    get documentElement() {
      return this.querySelector("html");
    }

    getElementsByTagName(tag) {
      return this.querySelectorAll(String(tag));
    }

    getElementsByClassName(cls) {
      return this.querySelectorAll("." + String(cls));
    }
  }

  globalThis.Node = Node;
  globalThis.Document = Document;
  globalThis.Event = Event;
  globalThis.document = wrapNode(__omoikane_document_id);
  globalThis.window = globalThis;
  globalThis.location = { href: __omoikane_location_href };
  globalThis.getComputedStyle = function() { return new Proxy({}, { get() { return ""; } }); };
  globalThis.navigator = { userAgent: __omoikane_navigator_user_agent };
  globalThis.console = {
    log: (...args) => __omoikane_console_log(...args),
  };
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
