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
    if (id === __omoikane_document_id || nodeType === 9) {
      // The top-level document and every sub-browsing-context document (an
      // iframe's contentDocument) are wrapped as Document so their DOM methods
      // are scoped to their own tree.
      node = new Document(id);
    } else if (nodeType === 11) {
      node = new DocumentFragment(id);
    } else if (nodeType === 3) {
      node = new Text(id);
    } else if (nodeType === 8) {
      node = new Comment(id);
    } else if (nodeType === 1) {
      const ctor = ELEMENT_CTORS[(__omoikane_node_name(id) || "").toLowerCase()];
      node = ctor ? new ctor(id) : new Node(id);
    } else {
      node = new Node(id);
    }
    cache.set(id, node);
    return node;
  }

  // Stamps `node` and (for a deep subtree) every descendant with `doc` as its
  // owning document, mirroring how `Document.create*` stamps `__ownerDoc`. Used
  // by `cloneNode`, whose native clone carries no wrapper metadata: without this
  // a detached clone of a sub-document node would fall through the
  // `ownerDocument` getter to the top-level document. Wrappers are cached by id
  // (see `wrapNode`), so the stamp persists across later `childNodes` reads.
  function stampOwnerDoc(node, doc) {
    if (!node) {
      return;
    }
    node.__ownerDoc = doc;
    const children = node.childNodes;
    for (let i = 0; i < children.length; i++) {
      stampOwnerDoc(children[i], doc);
    }
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

  // ── DOMException ────────────────────────────────────────────────────────────
  // Legacy DOM error-code constants (DOM Level 1..3). They are exposed both as
  // static properties on the DOMException constructor and on its prototype so
  // they are reachable from any thrown instance (`e.HIERARCHY_REQUEST_ERR`,
  // `DOMException.NAMESPACE_ERR`, ...), matching the DOM specification.
  const DOMEXCEPTION_CODES = {
    INDEX_SIZE_ERR: 1,
    DOMSTRING_SIZE_ERR: 2,
    HIERARCHY_REQUEST_ERR: 3,
    WRONG_DOCUMENT_ERR: 4,
    INVALID_CHARACTER_ERR: 5,
    NO_DATA_ALLOWED_ERR: 6,
    NO_MODIFICATION_ALLOWED_ERR: 7,
    NOT_FOUND_ERR: 8,
    NOT_SUPPORTED_ERR: 9,
    INUSE_ATTRIBUTE_ERR: 10,
    INVALID_STATE_ERR: 11,
    SYNTAX_ERR: 12,
    INVALID_MODIFICATION_ERR: 13,
    NAMESPACE_ERR: 14,
    INVALID_ACCESS_ERR: 15,
    VALIDATION_ERR: 16,
    TYPE_MISMATCH_ERR: 17,
    SECURITY_ERR: 18,
    NETWORK_ERR: 19,
    ABORT_ERR: 20,
    URL_MISMATCH_ERR: 21,
    QUOTA_EXCEEDED_ERR: 22,
    TIMEOUT_ERR: 23,
    INVALID_NODE_TYPE_ERR: 24,
    DATA_CLONE_ERR: 25,
  };

  // Maps a modern DOMException `name` to its legacy numeric `code`. Names absent
  // from this table carry code 0, per the DOM specification.
  const DOMEXCEPTION_NAME_TO_CODE = {
    IndexSizeError: 1,
    HierarchyRequestError: 3,
    WrongDocumentError: 4,
    InvalidCharacterError: 5,
    NoModificationAllowedError: 7,
    NotFoundError: 8,
    NotSupportedError: 9,
    InUseAttributeError: 10,
    InvalidStateError: 11,
    SyntaxError: 12,
    InvalidModificationError: 13,
    NamespaceError: 14,
    InvalidAccessError: 15,
    TypeMismatchError: 17,
    SecurityError: 18,
    NetworkError: 19,
    AbortError: 20,
    URLMismatchError: 21,
    QuotaExceededError: 22,
    TimeoutError: 23,
    InvalidNodeTypeError: 24,
    DataCloneError: 25,
  };

  class DOMException {
    constructor(message = "", name = "Error") {
      this.message = message == null ? "" : String(message);
      this.name = name;
      this.code = DOMEXCEPTION_NAME_TO_CODE[name] ?? 0;
    }

    toString() {
      return this.name + ": " + this.message;
    }
  }
  for (const constName of Object.keys(DOMEXCEPTION_CODES)) {
    const value = DOMEXCEPTION_CODES[constName];
    DOMException[constName] = value;
    DOMException.prototype[constName] = value;
  }

  // ── XML name / qualified-name validation ────────────────────────────────────
  // Implements the XML 1.0 `NameStartChar` / `NameChar` productions used by
  // `Document.createElement` and `createElementNS` to reject invalid names with
  // an `InvalidCharacterError`, and the DOM Level 3 "validate and extract"
  // namespace rules that raise `NamespaceError` for malformed or inconsistent
  // qualified names.
  const XML_NAMESPACE = "http://www.w3.org/XML/1998/namespace";
  const XMLNS_NAMESPACE = "http://www.w3.org/2000/xmlns/";

  function isXmlNameStartChar(cp) {
    return cp === 0x3a || (cp >= 0x41 && cp <= 0x5a) || cp === 0x5f ||
      (cp >= 0x61 && cp <= 0x7a) || (cp >= 0xc0 && cp <= 0xd6) ||
      (cp >= 0xd8 && cp <= 0xf6) || (cp >= 0xf8 && cp <= 0x2ff) ||
      (cp >= 0x370 && cp <= 0x37d) || (cp >= 0x37f && cp <= 0x1fff) ||
      (cp >= 0x200c && cp <= 0x200d) || (cp >= 0x2070 && cp <= 0x218f) ||
      (cp >= 0x2c00 && cp <= 0x2fef) || (cp >= 0x3001 && cp <= 0xd7ff) ||
      (cp >= 0xf900 && cp <= 0xfdcf) || (cp >= 0xfdf0 && cp <= 0xfffd) ||
      (cp >= 0x10000 && cp <= 0xeffff);
  }

  function isXmlNameChar(cp) {
    return isXmlNameStartChar(cp) || cp === 0x2d || cp === 0x2e ||
      (cp >= 0x30 && cp <= 0x39) || cp === 0xb7 ||
      (cp >= 0x300 && cp <= 0x36f) || (cp >= 0x203f && cp <= 0x2040);
  }

  function isValidXmlName(value) {
    if (!value) return false;
    const chars = Array.from(value);
    if (!isXmlNameStartChar(chars[0].codePointAt(0))) return false;
    for (let i = 1; i < chars.length; i += 1) {
      if (!isXmlNameChar(chars[i].codePointAt(0))) return false;
    }
    return true;
  }

  // Validates a qualified name and splits it into prefix / localName. Throws an
  // InvalidCharacterError if it is not an XML Name, or a NamespaceError if it is
  // a malformed QName (empty/extra colon-delimited parts).
  function validateQualifiedName(qname) {
    if (!isValidXmlName(qname)) {
      throw new DOMException(
        "The qualified name provided ('" + qname + "') is not a valid name.",
        "InvalidCharacterError"
      );
    }
    const parts = qname.split(":");
    if (parts.length === 1) {
      return { prefix: null, localName: parts[0] };
    }
    if (parts.length === 2 &&
        parts[0] !== "" && parts[1] !== "" &&
        isValidXmlName(parts[0]) && isValidXmlName(parts[1])) {
      return { prefix: parts[0], localName: parts[1] };
    }
    throw new DOMException(
      "The qualified name provided ('" + qname + "') is not a valid qualified name.",
      "NamespaceError"
    );
  }

  // The DOM Level 3 "validate and extract" algorithm for createElementNS: after
  // validating the QName, enforces the prefix/namespace consistency rules.
  function validateAndExtractNS(namespace, qname) {
    const { prefix, localName } = validateQualifiedName(qname);
    if (prefix !== null && namespace === null) {
      throw new DOMException(
        "A prefixed qualified name requires a non-null namespace.",
        "NamespaceError"
      );
    }
    if (prefix === "xml" && namespace !== XML_NAMESPACE) {
      throw new DOMException(
        "The 'xml' prefix must use the XML namespace.",
        "NamespaceError"
      );
    }
    if ((qname === "xmlns" || prefix === "xmlns") && namespace !== XMLNS_NAMESPACE) {
      throw new DOMException(
        "The 'xmlns' name/prefix must use the XMLNS namespace.",
        "NamespaceError"
      );
    }
    if (namespace === XMLNS_NAMESPACE && qname !== "xmlns" && prefix !== "xmlns") {
      throw new DOMException(
        "The XMLNS namespace requires the 'xmlns' name or prefix.",
        "NamespaceError"
      );
    }
    return { namespace, prefix, localName };
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

    // Legacy initialiser used by events created via `document.createEvent()`.
    initEvent(type, bubbles, cancelable) {
      this.type = String(type);
      this.bubbles = !!bubbles;
      this.cancelable = !!cancelable;
    }
  }

  class UIEvent extends Event {
    constructor(type, init = {}) {
      super(type, init);
      this.view = init.view ?? null;
      this.detail = init.detail ?? 0;
    }

    // Legacy initialiser used by `document.createEvent('UIEvents')`.
    initUIEvent(type, bubbles, cancelable, view, detail) {
      this.type = String(type);
      this.bubbles = !!bubbles;
      this.cancelable = !!cancelable;
      this.view = view ?? null;
      this.detail = detail ?? 0;
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

    // Throws a HierarchyRequestError if inserting `node` would make the tree
    // cyclic, i.e. `node` is this node or one of its ancestors. Mirrors the DOM
    // "ensure pre-insertion validity" hierarchy check.
    __ensureNotAncestor(node) {
      let ancestor = this;
      while (ancestor) {
        if (node && ancestor.__id === node.__id) {
          throw new DOMException(
            "The new child element contains the parent.",
            "HierarchyRequestError"
          );
        }
        ancestor = ancestor.parentNode;
      }
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
      this.__ensureNotAncestor(child);
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

      // Capture phase
      let stopped = false;
      for (let i = path.length - 1; i >= 1; i -= 1) {
        if (invokeListeners(path[i], dispatchEvent, true, 1)) {
          stopped = true;
          break;
        }
      }

      // Target phase
      if (!stopped) {
        if (invokeListeners(this, dispatchEvent, true, 2)) {
          stopped = true;
        }
      }
      if (!stopped) {
        if (invokeListeners(this, dispatchEvent, false, 2)) {
          stopped = true;
        }
      }

      // Bubble phase
      if (!stopped && dispatchEvent.bubbles) {
        for (let i = 1; i < path.length; i += 1) {
          if (invokeListeners(path[i], dispatchEvent, false, 3)) {
            break;
          }
        }
      }

      // Return value depends only on preventDefault, not stopPropagation
      return !dispatchEvent.defaultPrevented;
    }

    get parentNode() {
      return wrapNode(__omoikane_parent_node(this.__id));
    }

    get nodeName() {
      return __omoikane_node_name(this.__id);
    }

    get localName() {
      // HTML documents lower-case element names; non-element nodes have no
      // local name (DOM: text/comment/document `.localName` is null).
      if (this.nodeType !== 1) {
        return null;
      }
      const name = __omoikane_node_name(this.__id);
      return name ? name.toLowerCase() : null;
    }

    get id() {
      return __omoikane_get_attribute(this.__id, "id");
    }

    set id(value) {
      __omoikane_set_attribute(this.__id, "id", String(value));
    }

    get title() {
      return __omoikane_get_attribute(this.__id, "title") ?? "";
    }

    set title(value) {
      __omoikane_set_attribute(this.__id, "title", String(value));
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
      // `cssFloat` / `styleFloat` alias the CSS `float` property (per CSSOM),
      // so they map to `float` rather than the naive `css-float` kebab form.
      const toKebab = (prop) =>
        (prop === "cssFloat" || prop === "styleFloat")
          ? "float"
          : prop.replace(/[A-Z]/g, m => "-" + m.toLowerCase());
      return new Proxy({}, {
        get(target, prop) {
          if (typeof prop !== "string") return undefined;
          const kebab = toKebab(prop);
          const styleAttr = __omoikane_get_attribute(node.__id, "style") || "";
          const escaped = kebab.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
          const match = styleAttr.match(new RegExp("(?:^|;\\s*)" + escaped + "\\s*:\\s*([^;]+)"));
          return match ? match[1].trim() : "";
        },
        set(target, prop, value) {
          if (typeof prop !== "string") return true;
          const kebab = toKebab(prop);
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
      if (newNode && newNode.nodeType !== 11) {
        this.__ensureNotAncestor(newNode);
      }
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
      // A document node has no owner document.
      if (this.nodeType === 9) {
        return null;
      }
      // Prefer the document at the root of this node's tree: an attached node
      // belongs to whichever document it currently lives in — the top-level
      // document or an iframe sub-document — so parent and child contexts stay
      // separated. A detached node has no document root; fall back to the
      // document that created it (stamped by the Document.create* methods via
      // __own), or the top-level document when nothing else is known.
      const rootId = __omoikane_owner_document(this.__id);
      if (rootId !== null && rootId !== undefined) {
        return wrapNode(rootId);
      }
      return this.__ownerDoc || globalThis.document;
    }

    get nodeType() {
      return __omoikane_node_type(this.__id);
    }

    cloneNode(deep = false) {
      const clone = wrapNode(__omoikane_clone_node(this.__id, !!deep));
      // The clone is detached, so its ownerDocument comes from its creation
      // context, not its (absent) tree root. Propagate this node's owning
      // document to the clone and, for a deep clone, its descendants — so a
      // clone of a sub-document node keeps reporting that sub-document as its
      // ownerDocument rather than defaulting to the top-level document.
      const ownerDoc = this.ownerDocument;
      if (clone && ownerDoc) {
        stampOwnerDoc(clone, ownerDoc);
      }
      return clone;
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

    // Queries the native layout engine for this element's geometry, forcing a
    // synchronous reflow if the DOM changed since the last query.
    __layoutMetrics() {
      try {
        return JSON.parse(__omoikane_layout_metrics(this.__id));
      } catch (e) {
        return {
          x: 0, y: 0, width: 0, height: 0, top: 0, left: 0, right: 0, bottom: 0,
          offsetWidth: 0, offsetHeight: 0, offsetTop: 0, offsetLeft: 0,
          clientWidth: 0, clientHeight: 0, clientTop: 0, clientLeft: 0,
          scrollWidth: 0, scrollHeight: 0, scrollTop: 0, scrollLeft: 0,
          hasBox: false,
        };
      }
    }

    getBoundingClientRect() {
      const m = this.__layoutMetrics();
      return {
        x: m.x, y: m.y, width: m.width, height: m.height,
        top: m.top, left: m.left, bottom: m.bottom, right: m.right,
      };
    }

    getClientRects() {
      // CSSOM: an element with a rendered box returns at least one rectangle,
      // even when the box is zero-sized; an element that generates no box (e.g.
      // `display: none`) returns an empty list. `hasBox` distinguishes the two,
      // which a zero-sized rect alone cannot.
      const m = this.__layoutMetrics();
      if (!m.hasBox) return [];
      return [{
        x: m.x, y: m.y, width: m.width, height: m.height,
        top: m.top, left: m.left, bottom: m.bottom, right: m.right,
      }];
    }

    get offsetWidth() { return this.__layoutMetrics().offsetWidth; }
    get offsetHeight() { return this.__layoutMetrics().offsetHeight; }
    get offsetTop() { return this.__layoutMetrics().offsetTop; }
    get offsetLeft() { return this.__layoutMetrics().offsetLeft; }
    get clientWidth() { return this.__layoutMetrics().clientWidth; }
    get clientHeight() { return this.__layoutMetrics().clientHeight; }
    get clientTop() { return this.__layoutMetrics().clientTop; }
    get clientLeft() { return this.__layoutMetrics().clientLeft; }
    get scrollWidth() { return this.__layoutMetrics().scrollWidth; }
    get scrollHeight() { return this.__layoutMetrics().scrollHeight; }
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

    get name() {
      return __omoikane_get_attribute(this.__id, "name") ?? "";
    }

    set name(v) {
      __omoikane_set_attribute(this.__id, "name", String(v));
    }

    hasChildNodes() {
      return this.childNodes.length > 0;
    }
  }

  // CharacterData is the shared base of Text and Comment nodes. Its `data`
  // property is the node's character content (the same string exposed via
  // `textContent`/`nodeValue` for these node types). Defining it here — rather
  // than on the base Node — keeps `.data` off Element nodes, where e.g.
  // `HTMLObjectElement.data` reflects the `data` content attribute instead.
  class CharacterData extends Node {
    get data() {
      return this.textContent;
    }

    set data(value) {
      this.textContent = value == null ? "" : String(value);
    }

    get length() {
      return this.data.length;
    }
  }

  class Text extends CharacterData {}
  class Comment extends CharacterData {}

  class Document extends Node {
    // Stamps a freshly created node with this document as its owner so
    // `node.ownerDocument` resolves to this document even while the node is
    // detached (before it is inserted into any tree). Once the node is inserted,
    // its tree root wins (see the ownerDocument getter), matching DOM adoption.
    __own(node) {
      if (node) {
        node.__ownerDoc = this;
      }
      return node;
    }

    getElementById(id) {
      // Scope the lookup to this document's own tree. For the top-level
      // document this is identical to the whole-document search; for a
      // sub-browsing-context document (an iframe's contentDocument) it keeps
      // parent and child ids from being confused.
      //
      // Tolerate an unbound call (`var g = document.getElementById; g('x')`)
      // where `this` is not a Document and `this.__id` would be undefined (a
      // NaN id that resolves to null): fall back to the top-level document so
      // the lookup still resolves against the main tree.
      const scope = this instanceof Document ? this : globalThis.document;
      return wrapNode(__omoikane_query_selector(scope.__id, "#" + String(id)));
    }

    createElement(tag) {
      const name = String(tag);
      if (!isValidXmlName(name)) {
        throw new DOMException(
          "The tag name provided ('" + name + "') is not a valid name.",
          "InvalidCharacterError"
        );
      }
      return this.__own(wrapNode(__omoikane_create_element(name)));
    }

    createElementNS(namespace, qualifiedName) {
      const ns = (namespace === undefined || namespace === null || namespace === "")
        ? null
        : String(namespace);
      const qname = String(qualifiedName);
      const info = validateAndExtractNS(ns, qname);
      const node = this.__own(wrapNode(__omoikane_create_element(qname)));
      // Namespaced elements preserve their exact qualified name (no ASCII
      // upper-casing) and expose namespace metadata; shadow the prototype
      // getters with per-instance data properties.
      const define = (key, value) =>
        Object.defineProperty(node, key, { value, configurable: true, enumerable: false });
      define("namespaceURI", info.namespace);
      define("prefix", info.prefix);
      define("localName", info.localName);
      define("tagName", qname);
      define("nodeName", qname);
      return node;
    }

    get implementation() {
      return {
        hasFeature() {
          return true;
        },
        createDocumentType(qualifiedName, publicId, systemId) {
          validateQualifiedName(String(qualifiedName));
          return {
            nodeType: 10,
            name: String(qualifiedName),
            nodeName: String(qualifiedName),
            publicId: publicId == null ? "" : String(publicId),
            systemId: systemId == null ? "" : String(systemId),
            internalSubset: null,
          };
        },
        createHTMLDocument() {
          return globalThis.document;
        },
      };
    }

    createDocumentFragment() {
      return this.__own(wrapNode(__omoikane_create_document_fragment()));
    }

    createTextNode(text) {
      return this.__own(wrapNode(__omoikane_create_text_node(String(text))));
    }

    createComment(data) {
      return this.__own(wrapNode(__omoikane_create_comment(String(data ?? ""))));
    }

    createEvent(type) {
      const t = String(type);
      let evt;
      if (t === "UIEvent" || t === "UIEvents") {
        evt = new UIEvent("");
      } else if (t === "MouseEvent" || t === "MouseEvents") {
        evt = new MouseEvent("");
      } else if (t === "KeyboardEvent" || t === "KeyEvents") {
        evt = new KeyboardEvent("");
      } else if (t === "CustomEvent") {
        evt = new CustomEvent("");
      } else {
        evt = new Event("");
      }
      // A freshly created event has its propagation flags cleared until an
      // init* method is called.
      evt.bubbles = false;
      evt.cancelable = false;
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

    get defaultView() {
      // The Window associated with this document. The top-level document's
      // Window is the global object itself; a sub-browsing-context document (an
      // iframe's contentDocument) routes to its owning iframe's contentWindow
      // facade, so `frame.contentDocument.defaultView === frame.contentWindow`.
      //
      // Compare against the native main-document id rather than
      // `globalThis.document` so this is robust during bootstrap and never
      // treats a reloaded/stale sub-document as the main window: an unknown
      // document (owner iframe not found) reports null, not globalThis.
      if (this.__id === __omoikane_document_id) {
        return globalThis;
      }
      const iframeId = __omoikane_document_owner_iframe(this.__id);
      if (iframeId === null || iframeId === undefined) {
        return null;
      }
      const iframe = wrapNode(iframeId);
      return iframe ? iframe.contentWindow : null;
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

    // Adds markup to the document at the parser's insertion point. During
    // script execution the arguments are concatenated, tokenized as an HTML
    // fragment, and spliced into the tree as the running script's following
    // siblings (matching how a streaming parser resumes at the insertion
    // point). Only inline classic <script> elements in the written markup run
    // synchronously in global scope, in document order (external `src` and
    // `type="module"` scripts are inserted but not executed here), as classic
    // document.write() requires.
    //
    // Known limitations (follow-ups, out of scope for 016-7): when a fragment
    // mixes a <script> with later nodes the spec would run the script before
    // parsing those later nodes, but here all nodes are spliced in first and
    // the scripts run afterward; and there is no recursion-depth guard, so a
    // written script that writes another script recurses unbounded.
    write(...args) {
      let text = "";
      for (let i = 0; i < args.length; i += 1) {
        text += String(args[i]);
      }
      const scriptIds = __omoikane_document_write(this.__id, text);
      if (scriptIds && scriptIds.length) {
        for (let i = 0; i < scriptIds.length; i += 1) {
          const el = wrapNode(scriptIds[i]);
          const code = el ? el.textContent : "";
          if (code) {
            // Indirect eval runs the written script in global scope.
            (0, eval)(code);
          }
        }
      }
    }

    // Like write(), but appends a newline after the concatenated arguments.
    writeln(...args) {
      let text = "";
      for (let i = 0; i < args.length; i += 1) {
        text += String(args[i]);
      }
      return this.write(text + "\n");
    }

    // document.open() replaces the document with an empty one: it removes every
    // existing child so a following write() builds fresh content (HTML's
    // "document open steps"). This works for any Document instance — the main
    // document today, and sub-documents once iframe contentDocument lands in
    // 016-9 — because the reset targets this document node by id. Returns the
    // document, as the spec requires.
    open() {
      __omoikane_document_reset(this.__id);
      return this;
    }

    // document.close() has nothing to flush: write() splices its markup
    // synchronously, so there is no pending parser input to finish.
    close() {}
  }

  class DocumentFragment extends Node {}

  // An <iframe> owns a nested browsing context whose document is reachable via
  // contentDocument (and, as a facade, contentWindow.document). The document is
  // created lazily by the host on first access: an empty/absent src yields an
  // about:blank skeleton, while a src is fetched and parsed (only HTML content
  // types become a real DOM tree). Reading contentDocument again after changing
  // src reloads it.
  class HTMLIFrameElement extends Node {
    get contentDocument() {
      return wrapNode(__omoikane_iframe_content_document(this.__id));
    }

    get contentWindow() {
      // Return one stable Window facade per iframe so that
      // `iframe.contentWindow === iframe.contentWindow` holds and properties
      // assigned to it persist across accesses. `document` is a live getter, so
      // a later `src` change (which reloads the sub-document) is reflected on
      // the next read. A full Window per frame is out of scope here.
      if (!this.__contentWindowFacade) {
        const iframe = this;
        this.__contentWindowFacade = {
          get document() {
            return iframe.contentDocument;
          },
          frameElement: iframe,
          getComputedStyle: globalThis.getComputedStyle,
        };
      }
      return this.__contentWindowFacade;
    }

    get src() {
      return __omoikane_get_attribute(this.__id, "src") || "";
    }

    set src(value) {
      __omoikane_set_attribute(this.__id, "src", String(value));
    }
  }

  // ── HTML element specializations ────────────────────────────────────────────
  // wrapNode() dispatches element nodes to these subclasses by tag name so that
  // element-specific IDL attributes and methods (e.g. HTMLTableElement.rows,
  // HTMLButtonElement.type defaulting to "submit") are available. Elements
  // without a dedicated subclass fall back to the generic Node/Element class.

  function childElementsByTag(node, upperTag) {
    return node.childNodes.filter(c => c.nodeType === 1 && c.tagName === upperTag);
  }

  class HTMLTableElement extends Node {
    get caption() {
      return childElementsByTag(this, "CAPTION")[0] || null;
    }
    // The DOM setters here would replace the corresponding section; Acid3 only
    // performs no-op self-assignment, so they are intentionally inert.
    set caption(_value) {}
    get tHead() {
      return childElementsByTag(this, "THEAD")[0] || null;
    }
    set tHead(_value) {}
    get tFoot() {
      return childElementsByTag(this, "TFOOT")[0] || null;
    }
    set tFoot(_value) {}
    get tBodies() {
      return childElementsByTag(this, "TBODY");
    }
    get rows() {
      const rows = [];
      for (const head of childElementsByTag(this, "THEAD")) {
        rows.push(...childElementsByTag(head, "TR"));
      }
      for (const body of childElementsByTag(this, "TBODY")) {
        rows.push(...childElementsByTag(body, "TR"));
      }
      rows.push(...childElementsByTag(this, "TR"));
      for (const foot of childElementsByTag(this, "TFOOT")) {
        rows.push(...childElementsByTag(foot, "TR"));
      }
      return rows;
    }
    createCaption() {
      const existing = this.caption;
      if (existing) return existing;
      const caption = document.createElement("caption");
      this.insertBefore(caption, this.firstChild);
      return caption;
    }
    createTHead() {
      const existing = this.tHead;
      if (existing) return existing;
      const head = document.createElement("thead");
      const ref = childElementsByTag(this, "TBODY")[0]
        || childElementsByTag(this, "TFOOT")[0]
        || null;
      this.insertBefore(head, ref);
      return head;
    }
    createTFoot() {
      const existing = this.tFoot;
      if (existing) return existing;
      const foot = document.createElement("tfoot");
      this.appendChild(foot);
      return foot;
    }
    deleteCaption() {
      const caption = this.caption;
      if (caption) this.removeChild(caption);
    }
    deleteTHead() {
      const head = this.tHead;
      if (head) this.removeChild(head);
    }
    deleteTFoot() {
      const foot = this.tFoot;
      if (foot) this.removeChild(foot);
    }
  }

  const FORM_CONTROL_TAGS = new Set([
    "INPUT", "SELECT", "TEXTAREA", "BUTTON", "FIELDSET", "OBJECT", "OUTPUT", "KEYGEN",
  ]);

  class HTMLFormElement extends Node {
    __controls() {
      const controls = [];
      const walk = (node) => {
        for (const child of node.childNodes) {
          if (child.nodeType !== 1) continue;
          if (FORM_CONTROL_TAGS.has(child.tagName)) controls.push(child);
          walk(child);
        }
      };
      walk(this);
      return controls;
    }
    // Live HTMLFormControlsCollection: index access, `.length`, and named access
    // by control `name`/`id`. Missing named entries resolve to null.
    get elements() {
      const controls = this.__controls();
      return new Proxy(controls, {
        get(target, prop) {
          if (prop === "length") return target.length;
          if (typeof prop !== "string") return target[prop];
          if (/^\d+$/.test(prop)) return target[Number(prop)] ?? null;
          const named = target.find(
            c => (c.name && c.name === prop) || (c.id && c.id === prop)
          );
          if (named) return named;
          if (prop in target) return target[prop];
          return null;
        },
      });
    }
    get length() {
      return this.__controls().length;
    }
  }

  class HTMLInputElement extends Node {
    get type() {
      const t = (this.getAttribute("type") || "").toLowerCase();
      return t || "text";
    }
    set type(v) {
      this.setAttribute("type", String(v));
    }
    // The `value` IDL attribute is the control's "dirty value": it is held in
    // JS and is NOT reflected to the `value` content attribute. Storing it in
    // JS also preserves lone UTF-16 surrogates that would otherwise be mangled
    // crossing the native boundary.
    get value() {
      if (this.__value !== undefined) return this.__value;
      return this.getAttribute("value") || "";
    }
    set value(v) {
      this.__value = v == null ? "" : String(v);
    }
    get defaultValue() {
      return this.getAttribute("value") || "";
    }
    set defaultValue(v) {
      this.setAttribute("value", String(v));
    }
  }

  class HTMLButtonElement extends Node {
    get type() {
      const t = (this.getAttribute("type") || "").toLowerCase();
      if (t === "submit" || t === "reset" || t === "button" || t === "menu") {
        return t;
      }
      return "submit";
    }
    set type(v) {
      this.setAttribute("type", String(v));
    }
  }

  class HTMLLabelElement extends Node {
    get htmlFor() {
      return this.getAttribute("for") || "";
    }
    set htmlFor(v) {
      this.setAttribute("for", String(v));
    }
  }

  class HTMLMetaElement extends Node {
    get httpEquiv() {
      return this.getAttribute("http-equiv") || "";
    }
    set httpEquiv(v) {
      this.setAttribute("http-equiv", String(v));
    }
  }

  class HTMLSelectElement extends Node {
    get options() {
      return childElementsByTag(this, "OPTION");
    }
    get length() {
      return this.options.length;
    }
    get selectedIndex() {
      const options = this.options;
      for (let i = options.length - 1; i >= 0; i -= 1) {
        if (options[i].selected) return i;
      }
      return -1;
    }
    set selectedIndex(index) {
      this.options.forEach((option, i) => {
        option.selected = i === index;
      });
    }
    add(element, before) {
      if (before === null || before === undefined) {
        this.appendChild(element);
      } else if (typeof before === "number") {
        this.insertBefore(element, this.options[before] || null);
      } else {
        this.insertBefore(element, before);
      }
    }
    remove(index) {
      const option = this.options[index];
      if (option) this.removeChild(option);
    }
  }

  class HTMLOptionElement extends Node {
    get defaultSelected() {
      return this.hasAttribute("selected");
    }
    set defaultSelected(v) {
      if (v) this.setAttribute("selected", "");
      else this.removeAttribute("selected");
    }
    get selected() {
      if (this.__selected !== undefined) return this.__selected;
      return this.hasAttribute("selected");
    }
    set selected(v) {
      this.__selected = !!v;
    }
    get value() {
      if (this.hasAttribute("value")) return this.getAttribute("value");
      return this.textContent;
    }
    set value(v) {
      this.setAttribute("value", String(v));
    }
    get text() {
      return this.textContent;
    }
    set text(v) {
      this.textContent = v;
    }
  }

  // Tag-name → constructor table consulted by wrapNode() for element nodes.
  const ELEMENT_CTORS = {
    table: HTMLTableElement,
    form: HTMLFormElement,
    input: HTMLInputElement,
    button: HTMLButtonElement,
    label: HTMLLabelElement,
    meta: HTMLMetaElement,
    select: HTMLSelectElement,
    option: HTMLOptionElement,
    iframe: HTMLIFrameElement,
  };

  // Standard Node.nodeType constant values, exposed both as static properties
  // on the Node constructor (`Node.ELEMENT_NODE`) and on the prototype so they
  // are reachable from any node instance (`document.DOCUMENT_FRAGMENT_NODE`,
  // `element.COMMENT_NODE`, ...), matching the DOM specification.
  const NODE_TYPE_CONSTANTS = {
    ELEMENT_NODE: 1,
    ATTRIBUTE_NODE: 2,
    TEXT_NODE: 3,
    CDATA_SECTION_NODE: 4,
    ENTITY_REFERENCE_NODE: 5,
    ENTITY_NODE: 6,
    PROCESSING_INSTRUCTION_NODE: 7,
    COMMENT_NODE: 8,
    DOCUMENT_NODE: 9,
    DOCUMENT_TYPE_NODE: 10,
    DOCUMENT_FRAGMENT_NODE: 11,
    NOTATION_NODE: 12,
  };
  for (const constName of Object.keys(NODE_TYPE_CONSTANTS)) {
    const value = NODE_TYPE_CONSTANTS[constName];
    Node[constName] = value;
    Node.prototype[constName] = value;
  }

  globalThis.Node = Node;
  globalThis.Element = Node;
  globalThis.HTMLElement = Node;
  globalThis.CharacterData = CharacterData;
  globalThis.Text = Text;
  globalThis.Comment = Comment;
  globalThis.Document = Document;
  globalThis.DocumentFragment = DocumentFragment;
  globalThis.DOMException = DOMException;
  globalThis.HTMLTableElement = HTMLTableElement;
  globalThis.HTMLFormElement = HTMLFormElement;
  globalThis.HTMLInputElement = HTMLInputElement;
  globalThis.HTMLButtonElement = HTMLButtonElement;
  globalThis.HTMLLabelElement = HTMLLabelElement;
  globalThis.HTMLMetaElement = HTMLMetaElement;
  globalThis.HTMLSelectElement = HTMLSelectElement;
  globalThis.HTMLOptionElement = HTMLOptionElement;
  globalThis.HTMLIFrameElement = HTMLIFrameElement;
  globalThis.Event = Event;
  globalThis.CustomEvent = CustomEvent;
  globalThis.MouseEvent = MouseEvent;
  globalThis.KeyboardEvent = KeyboardEvent;
  globalThis.FocusEvent = FocusEvent;
  globalThis.UIEvent = UIEvent;
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

  // Wire `on*` inline event-handler content attributes (e.g.
  // `<body onload="update()">`, `<h1 onclick="report(event)">`) to real event
  // listeners, matching the HTML spec's event-handler-content-attribute
  // processing. Each attribute value is compiled as the body of
  // `function (event) { ... }`, so a handler can reference the `event` argument
  // and page globals. Handlers for the window-reflected events on
  // `<body>`/`<frameset>` (load, unload, resize, scroll, ...) are registered on
  // the Window so that `<body onload>` fires when the load event is dispatched;
  // all other handlers are registered on the element itself.
  const WINDOW_REFLECTED_HANDLERS = new Set([
    "load", "unload", "beforeunload", "resize", "scroll", "blur", "focus",
    "error", "hashchange", "popstate", "pageshow", "pagehide", "online",
    "offline", "languagechange", "message", "storage",
  ]);
  function compileInlineHandler(source) {
    try {
      return new Function("event", source);
    } catch (e) {
      return null;
    }
  }
  function wireInlineHandlers(node) {
    if (node && node.nodeType === 1) {
      const names = __omoikane_attribute_names(node.__id) || [];
      const tag = (__omoikane_node_name(node.__id) || "").toLowerCase();
      for (const name of names) {
        const lower = String(name).toLowerCase();
        if (lower.length <= 2 || lower.slice(0, 2) !== "on") continue;
        const type = lower.slice(2);
        const source = __omoikane_get_attribute(node.__id, name);
        if (source == null) continue;
        const handler = compileInlineHandler(source);
        if (!handler) continue;
        const reflectToWindow =
          (tag === "body" || tag === "frameset") && WINDOW_REFLECTED_HANDLERS.has(type);
        (reflectToWindow ? globalThis : node).addEventListener(type, handler);
      }
    }
    const kids = node ? node.childNodes : [];
    for (const child of kids) {
      wireInlineHandlers(child);
    }
  }
  globalThis.__omoikane_wire_inline_handlers = function() {
    wireInlineHandlers(globalThis.document);
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
  // Maps a JS-style property name to its CSS (kebab-case) form. `cssFloat` /
  // `styleFloat` alias the `float` property, matching the CSSOM.
  function __styleNameToCss(prop) {
    if (prop === "cssFloat" || prop === "styleFloat") return "float";
    return String(prop).replace(/[A-Z]/g, m => "-" + m.toLowerCase());
  }
  // Parses an inline `style="a: b; c: d"` attribute into a kebab-case map so
  // inline declarations override the cascade result in getComputedStyle.
  function __parseInlineStyle(cssText, map) {
    if (!cssText) return;
    for (const decl of String(cssText).split(";")) {
      const idx = decl.indexOf(":");
      if (idx < 0) continue;
      const name = decl.slice(0, idx).trim().toLowerCase();
      const value = decl.slice(idx + 1).trim();
      if (name) map[name] = value;
    }
  }
  // Builds a read-only CSSStyleDeclaration-like object over `map` (kebab-case
  // property names -> CSS string values). Supports camelCase access
  // (`style.whiteSpace`), `cssFloat`, `getPropertyValue('white-space')`,
  // `length`, and `item(i)`.
  function __makeComputedStyle(map) {
    const decl = {
      getPropertyValue(name) {
        const key = __styleNameToCss(name).toLowerCase();
        return Object.prototype.hasOwnProperty.call(map, key) ? map[key] : "";
      },
      getPropertyPriority() { return ""; },
      get length() { return Object.keys(map).length; },
      item(index) { return Object.keys(map)[index] || ""; },
      get cssText() {
        return Object.keys(map).map(k => k + ": " + map[k] + ";").join(" ");
      },
    };
    return new Proxy(decl, {
      get(target, prop) {
        if (typeof prop === "symbol" || prop in target) return target[prop];
        const key = __styleNameToCss(prop);
        return Object.prototype.hasOwnProperty.call(map, key) ? map[key] : "";
      },
      has(target, prop) {
        // Symbols (e.g. `Symbol.iterator in getComputedStyle(el)`) must never be
        // run through the CSS-name mapping; report membership from the target
        // only, matching the `get` trap's symbol guard.
        if (typeof prop === "symbol") return prop in target;
        if (prop in target) return true;
        return Object.prototype.hasOwnProperty.call(map, __styleNameToCss(prop));
      },
      set() {
        // getComputedStyle returns a read-only CSSStyleDeclaration: its
        // properties are getter-only accessors, so assignments are ignored
        // (silently in sloppy mode, TypeError in strict mode). Returning false
        // reproduces that and keeps the underlying `decl`/`map` unmutated, so a
        // later read still reports the computed value rather than a stale write.
        return false;
      },
    });
  }
  globalThis.getComputedStyle = function(element, pseudoElt) {
    void pseudoElt;
    const map = {};
    if (element && element.__id != null) {
      let cascade;
      try {
        cascade = JSON.parse(__omoikane_computed_style(element.__id));
      } catch (e) {
        cascade = {};
      }
      for (const key in cascade) {
        if (Object.prototype.hasOwnProperty.call(cascade, key)) map[key] = cascade[key];
      }
      // Inline styles take precedence over cascaded values.
      __parseInlineStyle(__omoikane_get_attribute(element.__id, "style"), map);
    }
    return __makeComputedStyle(map);
  };
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
      try {
        const callback = __rafCallbacks.get(id);
        if (typeof callback === "function") {
          callback(Date.now());
        }
      } finally {
        __rafCallbacks.delete(id);
      }
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
