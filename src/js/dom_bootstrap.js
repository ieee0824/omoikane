(() => {
  // The top-level browsing context is its own parent.
  globalThis.parent = globalThis;
  const cache = new Map();

  // Window objects are not ordinary DOM wrappers in Omoikane: the top-level
  // Window is Boa's global object, while nested browsing contexts currently
  // expose a small facade. Keep their identity explicit instead of changing
  // the global object's prototype, which could disturb Boa's global property
  // lookup and built-in prototype chain.
  const windowObjects = new WeakSet([globalThis]);
  class Window {
    constructor() {
      throw new TypeError("Illegal constructor");
    }
  }
  Object.defineProperty(Window, Symbol.hasInstance, {
    value(candidate) {
      return (typeof candidate === "object" || typeof candidate === "function") &&
        candidate !== null && windowObjects.has(candidate);
    },
  });

  class NodeList extends Array {
    constructor() {
      throw new TypeError("Illegal constructor");
    }

    static get [Symbol.species]() {
      return Array;
    }

    item(index) {
      const value = Number(index);
      if (!Number.isFinite(value) || value < 0) return null;
      return this[Math.trunc(value)] ?? null;
    }
  }
  Object.defineProperty(NodeList.prototype, Symbol.toStringTag, {
    configurable: true,
    value: "NodeList",
  });

  function makeNodeList(nodes) {
    return Object.setPrototypeOf(nodes, NodeList.prototype);
  }

  function removeChildNode() {
    const parent = this.parentNode;
    if (parent) parent.removeChild(this);
  }

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
    } else if (nodeType === 7) {
      node = new ProcessingInstruction(id);
    } else if (nodeType === 3) {
      node = new Text(id);
    } else if (nodeType === 8) {
      node = new Comment(id);
    } else if (nodeType === 1) {
      const namespace = __omoikane_node_namespace_uri(id);
      const htmlElement = namespace == null || namespace === HTML_NAMESPACE;
      const constructors = namespace === SVG_NAMESPACE
        ? SVG_ELEMENT_CTORS
        : htmlElement ? ELEMENT_CTORS : {};
      const ctor = constructors[(__omoikane_node_local_name(id) || __omoikane_node_name(id) || "").toLowerCase()];
      node = ctor ? new ctor(id)
        : namespace === SVG_NAMESPACE ? new SVGElement(id)
        : htmlElement ? new HTMLElement(id) : new Element(id);
    } else if (nodeType === 10) {
      node = new DocumentType(id);
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
        if (typeof entry.listener === "function") {
          entry.listener.call(node, event);
        } else if (entry.listener && typeof entry.listener.handleEvent === "function") {
          entry.listener.handleEvent.call(entry.listener, event);
        }
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

  function isValidXmlName(value) {
    return __omoikane_is_valid_xml_name(value);
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

  const HTML_NAMESPACE = "http://www.w3.org/1999/xhtml";
  const SVG_NAMESPACE = "http://www.w3.org/2000/svg";

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

  class MessageEvent extends Event {
    constructor(type, init = {}) {
      super(type, init);
      this.data = init.data ?? null;
      this.origin = init.origin ?? "";
      this.lastEventId = init.lastEventId ?? "";
      this.source = init.source ?? null;
      this.ports = init.ports ?? [];
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

  // Boa 0.21 exposes WeakMap/WeakRef (but not FinalizationRegistry), so the
  // traversal registry must not itself keep Documents, NodeIterators or Ranges
  // alive. Dead weak references are swept on registration, mutation and
  // detach. The bounded strong-reference fallback is only for JS engines that
  // lack WeakRef; bounding it trades updates to very old traversal objects for
  // avoiding the permanent, unbounded leak that the old Map<id, Set> caused.
  const hasWeakTraversalRegistry = typeof WeakMap === "function" && typeof WeakRef === "function";
  const traversalByDocument = hasWeakTraversalRegistry ? new WeakMap() : new Map();
  const MAX_STRONG_TRAVERSAL_ENTRIES = 1024;
  const TRAVERSAL_SWEEP_INTERVAL = 64;
  function traversalDocumentKey(doc) {
    return hasWeakTraversalRegistry ? doc : doc.__id;
  }
  function traversalState(doc) {
    const key = traversalDocumentKey(doc);
    let state = traversalByDocument.get(key);
    if (!state) {
      state = { iterators: [], ranges: [] };
      traversalByDocument.set(key, state);
    }
    return state;
  }

  function traversalEntries(entries) {
    if (!hasWeakTraversalRegistry) return entries.slice();
    const live = [];
    for (let i = entries.length - 1; i >= 0; i--) {
      const ref = entries[i];
      const value = ref.deref();
      if (value) live.push(value);
      else entries.splice(i, 1);
    }
    return live;
  }

  function registerTraversal(doc, kind, value) {
    const entries = traversalState(doc)[kind];
    // WeakRef targets stay alive through the current ECMAScript job. Sweeping
    // every insertion would therefore be quadratic in createRange-heavy code;
    // periodic registration sweeps plus every mutation/detach keep it bounded
    // without penalising a burst of live traversal objects.
    if (entries.length % TRAVERSAL_SWEEP_INTERVAL === 0) traversalEntries(entries);
    if (hasWeakTraversalRegistry) entries.push(new WeakRef(value));
    else {
      if (entries.length >= MAX_STRONG_TRAVERSAL_ENTRIES) entries.shift();
      entries.push(value);
    }
  }

  function unregisterTraversal(doc, kind, value) {
    const entries = traversalState(doc)[kind];
    if (!hasWeakTraversalRegistry) {
      const index = entries.indexOf(value);
      if (index !== -1) entries.splice(index, 1);
    }
    else {
      for (let i = entries.length - 1; i >= 0; i--) {
        const ref = entries[i];
        const target = ref.deref();
        if (!target || target === value) entries.splice(i, 1);
      }
    }
  }

  // Internal diagnostic used by regression tests; reading it also performs the
  // same dead-reference sweep as normal registry operations.
  globalThis.__omoikane_traversal_registry_counts = doc => {
    const state = traversalByDocument.get(traversalDocumentKey(doc));
    return state ? {
      iterators: traversalEntries(state.iterators).length,
      ranges: traversalEntries(state.ranges).length,
    } : { iterators: 0, ranges: 0 };
  };

  function nodeDocument(node) {
    if (!node) return globalThis.document;
    return node.nodeType === 9 ? node : (node.ownerDocument || globalThis.document);
  }

  function nodeRoot(node) {
    let root = node;
    while (root && root.parentNode) root = root.parentNode;
    return root;
  }

  function isInclusiveDescendant(node, ancestor) {
    for (let n = node; n; n = n.parentNode) if (n === ancestor) return true;
    return false;
  }

  function indexOfNode(node) {
    const parent = node && node.parentNode;
    return parent ? parent.childNodes.indexOf(node) : -1;
  }

  function preRemove(parent, removed) {
    const doc = nodeDocument(parent);
    const state = traversalByDocument.get(traversalDocumentKey(doc));
    if (!state) return;
    for (const iterator of traversalEntries(state.iterators)) iterator.__preRemove(removed);
    for (const range of traversalEntries(state.ranges)) range.__preRemove(parent, removed);
  }

  function notifyImplicitRemoval(node) {
    if (!node || !node.parentNode) return;
    const parent = node.parentNode;
    const previousSibling = node.previousSibling;
    const nextSibling = node.nextSibling;
    preRemove(parent, node);
    queueMutation(parent, "childList", { removedNodes: [node], previousSibling, nextSibling });
  }

  // Split a CSS declaration block only at top-level semicolons. Data URLs,
  // functions and quoted strings may legally contain semicolons; a plain
  // String.split would corrupt those values during a CSSStyleDeclaration
  // read-modify-write cycle.
  function splitCssDeclarations(input) {
    const parts = [];
    let start = 0;
    let quote = "";
    let depth = 0;
    for (let i = 0; i < input.length; i++) {
      const ch = input[i];
      if (ch === "\\") {
        // A CSS escape consumes the following code unit both inside and outside
        // strings, so an escaped quote/semicolon cannot affect scanner state.
        if (i + 1 < input.length) i++;
        continue;
      }
      if (quote) {
        if (ch === quote) quote = "";
        continue;
      }
      if (ch === "\"" || ch === "'") {
        quote = ch;
      } else if (ch === "(") {
        depth++;
      } else if (ch === ")" && depth > 0) {
        depth--;
      } else if (ch === ";" && depth === 0) {
        parts.push(input.slice(start, i));
        start = i + 1;
      }
    }
    parts.push(input.slice(start));
    return parts;
  }

  class Node {
    constructor(id) {
      this.__id = id;
      this.__listeners = new Map();
      this.__onload = null;
    }

    get onload() {
      return this.__onload;
    }

    set onload(handler) {
      if (this.__onload) this.removeEventListener("load", this.__onload);
      this.__onload = typeof handler === "function" ? handler : null;
      if (this.__onload) this.addEventListener("load", this.__onload);
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
      const previousSibling = this.lastChild;
      if (child && child.nodeType === 11) {
        const children = child.childNodes.slice();
        for (const c of children) {
          notifyImplicitRemoval(c);
          __omoikane_append_child(this.__id, c.__id);
        }
        if (children.length) queueMutation(this, "childList", { addedNodes: children, previousSibling });
        return child;
      }
      this.__ensureNotAncestor(child);
      notifyImplicitRemoval(child);
      __omoikane_append_child(this.__id, child.__id);
      queueMutation(this, "childList", { addedNodes: [child], previousSibling });
      return child;
    }

    querySelector(selector) {
      try {
        const id = __omoikane_query_selector(this.__id, String(selector));
        return wrapNode(id);
      } catch (error) {
        if (error && error.name === "SyntaxError") {
          throw new DOMException(error.message, "SyntaxError");
        }
        throw error;
      }
    }

    addEventListener(type, listener, options = false) {
      if (listener == null ||
          (typeof listener !== "function" && typeof listener.handleEvent !== "function")) {
        return;
      }
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
      const notCanceled = !dispatchEvent.defaultPrevented;
      // Activation behavior is the default action of a click event: a submit or
      // reset button whose click was not canceled submits or resets its owning
      // form. Running it here (rather than only in click()) means a synthetic
      // click dispatched directly through dispatchEvent behaves like a real one.
      if (notCanceled && dispatchEvent.type === "click" && this.nodeType === 1 &&
          typeof this.__runActivationBehavior === "function") {
        this.__runActivationBehavior();
      }
      return notCanceled;
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
      return __omoikane_node_local_name(this.__id);
    }

    get namespaceURI() {
      return __omoikane_node_namespace_uri(this.__id);
    }

    get prefix() {
      return __omoikane_node_prefix(this.__id);
    }

    get publicId() {
      return this.nodeType === 10 ? __omoikane_doctype_public_id(this.__id) : undefined;
    }

    get systemId() {
      return this.nodeType === 10 ? __omoikane_doctype_system_id(this.__id) : undefined;
    }

    get internalSubset() {
      return this.nodeType === 10 ? null : undefined;
    }

    get id() {
      return __omoikane_get_attribute(this.__id, "id");
    }

    set id(value) { this.setAttribute("id", value); }

    get title() {
      // DOMString-reflecting attributes use the empty string when absent.
      return __omoikane_get_attribute(this.__id, "title") ?? "";
    }

    set title(value) { this.setAttribute("title", value); }

    getAttribute(name) {
      return __omoikane_get_attribute(this.__id, String(name));
    }

    setAttribute(name, value) {
      const attr = String(name);
      const oldValue = __omoikane_get_attribute(this.__id, attr);
      __omoikane_set_attribute(this.__id, attr, String(value));
      queueMutation(this, "attributes", { attributeName: attr, oldValue });
      // A dynamically set `on*` content attribute is wired to a listener here
      // (parse-time attributes go through the initial wireInlineHandlers pass).
      if (/^on./i.test(attr)) applyInlineHandlerAttribute(this, attr);
    }

    get className() {
      return __omoikane_get_attribute(this.__id, "class") || "";
    }

    set className(value) { this.setAttribute("class", value); }

    get classList() {
      const node = this;
      const validate = classes => classes.map(value => {
        const token = String(value);
        if (token === "") throw new DOMException("The token must not be empty.", "SyntaxError");
        if (/\s/.test(token)) throw new DOMException("The token contains whitespace.", "InvalidCharacterError");
        return token;
      });
      return {
        add(...classes) {
          classes = validate(classes);
          const current = new Set((node.className || "").split(/\s+/).filter(Boolean));
          for (const cls of classes) current.add(cls);
          node.className = [...current].join(" ");
        },
        remove(...classes) {
          classes = validate(classes);
          const current = new Set((node.className || "").split(/\s+/).filter(Boolean));
          for (const cls of classes) current.delete(cls);
          node.className = [...current].join(" ");
        },
        toggle(cls, force) {
          [cls] = validate([cls]);
          const current = new Set((node.className || "").split(/\s+/).filter(Boolean));
          const has = current.has(cls);
          if (force === undefined) {
            has ? current.delete(cls) : current.add(cls);
          } else if (!!force === has) {
            return has;
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
          : String(prop).replace(/[A-Z]/g, m => "-" + m.toLowerCase());
      // Normalizes a CSSOM method argument (`setProperty`/`removeProperty`/…) to
      // its canonical CSS (kebab, lowercase) form, matching getComputedStyle's
      // `__styleNameToCss(name).toLowerCase()` so camelCase and kebab-case access
      // resolve to the same declaration.
      const toCssName = (name) => {
        const s = String(name);
        // Custom properties (`--foo`) are case-sensitive and already in CSS
        // form, so they pass through untouched — never camelCase→kebab folded,
        // which would otherwise corrupt `--Foo` into `---foo` and break the
        // set/get/removeProperty round-trip.
        if (s.startsWith("--")) return s;
        return toKebab(s).toLowerCase();
      };

      // Reads the current `style` attribute into an ordered list of
      // `{ name, value, priority }` declarations. `name` is the kebab-case CSS
      // property, `priority` is "important" or "". Both the camelCase accessors
      // and the CSSStyleDeclaration methods below operate over this list so the
      // two views stay consistent and every mutation re-serializes through the
      // one `__omoikane_set_attribute` path (driving cascade/layout dirtying).
      const parseDecls = () => {
        const attr = __omoikane_get_attribute(node.__id, "style") || "";
        const decls = [];
        for (const part of splitCssDeclarations(attr)) {
          const idx = part.indexOf(":");
          if (idx < 0) continue;
          const rawName = part.slice(0, idx).trim();
          // Custom properties are case-sensitive; only standard property names
          // fold to lowercase so `--Foo` survives a serialize/parse round-trip.
          const name = rawName.startsWith("--") ? rawName : rawName.toLowerCase();
          let value = part.slice(idx + 1).trim();
          if (!name || value === "") continue;
          let priority = "";
          const stripped = value.replace(/\s*!\s*important\s*$/i, "");
          if (stripped !== value) { priority = "important"; value = stripped.trim(); }
          if (value === "") continue;
          decls.push({ name, value, priority });
        }
        return decls;
      };
      const serializeDecls = (decls) =>
        decls
          .map(d => d.name + ": " + d.value + (d.priority ? " !" + d.priority : "") + ";")
          .join(" ");
      const writeDecls = (decls) =>
        __omoikane_set_attribute(node.__id, "style", serializeDecls(decls));

      // Returns the declared value for a kebab-case property ("" if absent).
      // Later declarations win, matching the inline cascade.
      const getValue = (kebab) => {
        const decls = parseDecls();
        for (let i = decls.length - 1; i >= 0; i--) {
          if (decls[i].name === kebab) return decls[i].value;
        }
        return "";
      };
      const getPriority = (kebab) => {
        const decls = parseDecls();
        for (let i = decls.length - 1; i >= 0; i--) {
          if (decls[i].name === kebab) return decls[i].priority;
        }
        return "";
      };
      // Sets a kebab-case property. When the property is already declared, the
      // last (winning) occurrence is updated in place and any earlier duplicates
      // are dropped, so a block with redundant declarations (e.g. from cssText)
      // normalizes to a single declaration that matches the last-wins reads of
      // getValue/getPriority.
      const setValue = (kebab, value, priority) => {
        const decls = parseDecls();
        const matches = decls.filter(d => d.name === kebab);
        if (matches.length > 0) {
          const winner = matches[matches.length - 1];
          winner.value = value;
          winner.priority = priority || "";
          writeDecls(decls.filter(d => d.name !== kebab || d === winner));
        } else {
          decls.push({ name: kebab, value, priority: priority || "" });
          writeDecls(decls);
        }
      };
      // Removes a kebab-case property and returns its previous value ("" if it
      // was not set), per CSSOM `removeProperty`. The returned value is the
      // last-wins (winning) declaration's value, and every occurrence — not
      // just the first — is removed.
      const removeValue = (kebab) => {
        const decls = parseDecls();
        const remaining = decls.filter(d => d.name !== kebab);
        if (remaining.length === decls.length) return "";
        let old = "";
        for (let i = decls.length - 1; i >= 0; i--) {
          if (decls[i].name === kebab) { old = decls[i].value; break; }
        }
        writeDecls(remaining);
        return old;
      };

      // CSSStyleDeclaration surface. `length`/`cssText` are accessors so they
      // reflect the live attribute; `cssText` writes go through the proxy `set`
      // trap below (which routes to `setCssText`).
      const setCssText = (value) => {
        __omoikane_set_attribute(
          node.__id,
          "style",
          value == null ? "" : String(value),
        );
      };
      const decl = {
        getPropertyValue(name) { return getValue(toCssName(name)); },
        getPropertyPriority(name) { return getPriority(toCssName(name)); },
        setProperty(name, value, priority) {
          const kebab = toCssName(name);
          if (value == null || value === "") { removeValue(kebab); return; }
          // Per CSSOM only "important" (ASCII case-insensitive) is a valid
          // priority; any other non-empty token is treated as no priority
          // rather than being serialized as a bogus `!foo`.
          const prio =
            priority != null && String(priority).toLowerCase() === "important"
              ? "important"
              : "";
          setValue(kebab, String(value), prio);
        },
        removeProperty(name) { return removeValue(toCssName(name)); },
        item(index) {
          const decls = parseDecls();
          const i = Number(index) | 0;
          return (i >= 0 && i < decls.length) ? decls[i].name : "";
        },
        get length() { return parseDecls().length; },
        get cssText() { return serializeDecls(parseDecls()); },
        set cssText(value) { setCssText(value); },
      };

      return new Proxy(decl, {
        get(target, prop) {
          // Symbols and the CSSStyleDeclaration members (methods, `length`,
          // `cssText`, and inherited Object members) resolve from the target;
          // everything else is treated as a camelCase CSS property name.
          if (typeof prop === "symbol" || prop in target) return target[prop];
          return getValue(toKebab(prop));
        },
        has(target, prop) {
          if (typeof prop === "symbol") return prop in target;
          if (prop in target) return true;
          return getValue(toKebab(prop)) !== "";
        },
        set(target, prop, value) {
          if (typeof prop !== "string") return true;
          // `cssText` replaces the whole declaration block.
          if (prop === "cssText") { setCssText(value); return true; }
          // Other CSSStyleDeclaration members are read-only; ignore writes so a
          // stray `style.length = …` does not create a bogus declaration.
          if (prop in target) return true;
          const kebab = toKebab(prop);
          if (value === null || value === undefined || value === "") {
            removeValue(kebab);
          } else {
            // A plain camelCase assignment carries no priority (CSSOM), so any
            // prior `!important` on this property is dropped.
            setValue(kebab, String(value), "");
          }
          return true;
        },
      });
    }

    get textContent() {
      const type = this.nodeType;
      if (type === 9 || type === 10) return null;
      return __omoikane_get_text_content(this.__id);
    }

    set textContent(value) {
      const type = this.nodeType;
      if (type === 9 || type === 10) return;
      const text = value == null ? "" : String(value);
      const removedNodes = this.childNodes.slice();
      // Native replaces all children at once. Notify from the end so boundary
      // offsets for later siblings are subsequently decremented by removals of
      // their preceding siblings, matching sequential pre-remove semantics.
      for (const child of this.childNodes.slice().reverse()) preRemove(this, child);
      __omoikane_set_text_content(this.__id, text);
      const addedNodes = this.childNodes.slice();
      if (removedNodes.length || addedNodes.length) {
        queueMutation(this, "childList", { addedNodes, removedNodes });
      }
    }

    get innerHTML() {
      return __omoikane_get_inner_html(this.__id) || "";
    }

    set innerHTML(value) {
      const html = value == null ? "" : String(value);
      const removedNodes = this.childNodes.slice();
      for (const child of removedNodes.slice().reverse()) preRemove(this, child);
      __omoikane_set_inner_html(this.__id, html);
      const addedNodes = this.childNodes.slice();
      if (removedNodes.length || addedNodes.length) {
        queueMutation(this, "childList", { addedNodes, removedNodes });
      }
    }

    get childNodes() {
      const ids = __omoikane_child_node_ids(this.__id);
      return makeNodeList(ids ? ids.map(id => wrapNode(id)) : []);
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
      const previousSibling = child.previousSibling;
      const nextSibling = child.nextSibling;
      preRemove(this, child);
      __omoikane_remove_child(this.__id, child.__id);
      queueMutation(this, "childList", { removedNodes: [child], previousSibling, nextSibling });
      return child;
    }

    insertBefore(newNode, refNode) {
      if (refNode !== null && refNode.parentNode !== this) {
        throw new DOMException("The reference node is not a child of this node.", "NotFoundError");
      }
      if (newNode && newNode.nodeType !== 11) {
        this.__ensureNotAncestor(newNode);
      }
      if (newNode && newNode.nodeType === 11) {
        const children = newNode.childNodes.slice();
        const previousSibling = refNode ? refNode.previousSibling : this.lastChild;
        for (const child of children) {
          notifyImplicitRemoval(child);
          __omoikane_insert_before(this.__id, child.__id, refNode ? refNode.__id : null);
        }
        if (children.length) queueMutation(this, "childList", { addedNodes: children, previousSibling, nextSibling: refNode });
        return newNode;
      }
      const previousSibling = refNode ? refNode.previousSibling : this.lastChild;
      notifyImplicitRemoval(newNode);
      __omoikane_insert_before(this.__id, newNode.__id, refNode ? refNode.__id : null);
      queueMutation(this, "childList", { addedNodes: [newNode], previousSibling, nextSibling: refNode });
      return newNode;
    }

    querySelectorAll(selector) {
      try {
        const ids = __omoikane_query_selector_all(this.__id, String(selector));
        return makeNodeList(ids ? ids.map(id => wrapNode(id)) : []);
      } catch (error) {
        if (error && error.name === "SyntaxError") {
          throw new DOMException(error.message, "SyntaxError");
        }
        throw error;
      }
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
      if (this.__cdataSection) return 4;
      return __omoikane_node_type(this.__id);
    }

    normalize() {
      for (const child of this.childNodes.slice()) {
        if (child.parentNode !== this) continue;
        if (child.nodeType !== 3) {
          child.normalize();
          continue;
        }
        if (child.length === 0) {
          this.removeChild(child);
          continue;
        }
        while (child.nextSibling && child.nextSibling.nodeType === 3) {
          const next = child.nextSibling;
          const offset = child.length;
          const index = indexOfNode(next);
          child.appendData(next.data);
          const state = traversalByDocument.get(traversalDocumentKey(nodeDocument(this)));
          if (state) {
            for (const range of traversalEntries(state.ranges)) {
              range.__mergeText(child, next, offset, this, index);
            }
          }
          this.removeChild(next);
        }
      }
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
      // Preserve JS UTF-16 code units that Rust UTF-8 strings cannot represent,
      // including CharacterData descendants of a deep clone.
      const preserveCharacterData = (source, target) => {
        if (source instanceof CDATASection) {
          Object.setPrototypeOf(target, CDATASection.prototype);
          Object.defineProperty(target, "__cdataSection", { value: true, configurable: true });
        }
        if (source instanceof CharacterData) target.textContent = source.textContent;
        if (!deep) return;
        const sourceChildren = source.childNodes;
        const targetChildren = target.childNodes;
        for (let i = 0; i < sourceChildren.length; i++) {
          preserveCharacterData(sourceChildren[i], targetChildren[i]);
        }
      };
      if (clone) preserveCharacterData(this, clone);
      return clone;
    }

    hasAttribute(name) {
      return __omoikane_get_attribute(this.__id, String(name)) !== null;
    }

    removeAttribute(name) {
      const attr = String(name);
      const oldValue = __omoikane_get_attribute(this.__id, attr);
      __omoikane_remove_attribute(this.__id, attr);
      if (oldValue !== null) queueMutation(this, "attributes", { attributeName: attr, oldValue });
      // Removing an `on*` content attribute detaches the listener it wired.
      if (/^on./i.test(attr)) applyInlineHandlerAttribute(this, attr);
    }

    setAttributeNS(namespace, qualifiedName, value) {
      const ns = namespace == null || namespace === "" ? null : String(namespace);
      const name = String(qualifiedName);
      const { localName } = validateAndExtractNS(ns, name);
      if (ns === null) {
        this.setAttribute(name, value);
        return;
      }
      if (!this.__namespacedAttributes) this.__namespacedAttributes = new Map();
      const key = String(ns) + "|" + localName;
      const previous = this.__namespacedAttributes.get(key);
      const oldValue = previous ? previous.value : null;
      if (previous && previous.name !== name) {
        __omoikane_remove_attribute(this.__id, previous.name);
      }
      this.__namespacedAttributes.set(key, { name, localName, namespaceURI: ns, value: String(value) });
      __omoikane_set_attribute(this.__id, name, String(value));
      queueMutation(this, "attributes", { attributeName: localName, attributeNamespace: ns, oldValue });
    }

    getAttributeNS(namespace, localName) {
      const ns = namespace == null || namespace === "" ? null : String(namespace);
      if (ns === null) return this.getAttribute(localName);
      const entry = this.__namespacedAttributes && this.__namespacedAttributes.get(String(ns) + "|" + String(localName));
      return entry ? entry.value : null;
    }

    removeAttributeNS(namespace, localName) {
      const ns = namespace == null || namespace === "" ? null : String(namespace);
      if (ns === null) {
        this.removeAttribute(localName);
        return;
      }
      const key = String(ns) + "|" + String(localName);
      const entry = this.__namespacedAttributes && this.__namespacedAttributes.get(key);
      if (!entry) return;
      this.__namespacedAttributes.delete(key);
      __omoikane_remove_attribute(this.__id, entry.name);
      queueMutation(this, "attributes", { attributeName: entry.localName, attributeNamespace: ns, oldValue: entry.value });
    }

    get tagName() {
      if (this.nodeType !== 1) {
        return undefined;
      }
      return __omoikane_node_name(this.__id) ?? undefined;
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
      const names = () => __omoikane_attribute_names(node.__id) || [];
      const makeAttr = name => {
        const attr = { name, localName: name, specified: true, expando: false };
        Object.defineProperty(attr, "value", {
          enumerable: true,
          get() { return node.getAttribute(name); },
          set(value) { node.setAttribute(name, value); },
        });
        return attr;
      };
      return new Proxy([], {
        get(_target, prop) {
          const list = names();
          if (prop === "length") return list.length;
          if (prop === "item") return index => list[Number(index)] === undefined ? null : makeAttr(list[Number(index)]);
          if (prop === "getNamedItem") return name => node.hasAttribute(name) ? makeAttr(String(name)) : null;
          if (prop === "setNamedItem") return attr => { node.setAttribute(attr.name, attr.value); return attr; };
          if (prop === "removeNamedItem") return name => {
            name = String(name);
            if (!node.hasAttribute(name)) {
              throw new DOMException("The requested attribute does not exist.", "NotFoundError");
            }
            const old = makeAttr(name);
            node.removeAttribute(name);
            return old;
          };
          if (typeof prop === "string" && /^(?:0|[1-9]\d*)$/.test(prop)) {
            return list[Number(prop)] === undefined ? undefined : makeAttr(list[Number(prop)]);
          }
          if (typeof prop === "string" && node.hasAttribute(prop)) return makeAttr(prop);
          return Array.prototype[prop];
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
      if (t === 3 || t === 7 || t === 8) return this.textContent;
      return null;
    }

    set nodeValue(value) {
      const t = this.nodeType;
      if (t === 3 || t === 7 || t === 8) this.data = value;
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

    // True when this element is a form control on which the `disabled`
    // attribute has meaning and is set. A stray `<div disabled>` is not a
    // disabled control; only these tags honour the attribute.
    __isDisabledControl() {
      const DISABLEABLE_TAGS = ["input", "button", "select", "textarea", "option", "optgroup", "fieldset"];
      return this.disabled && DISABLEABLE_TAGS.includes(this.nodeName.toLowerCase());
    }

    click() {
      // A disabled form control is not activated at all: it does not even
      // dispatch a click event. A stray `<div disabled>` must still dispatch.
      if (this.__isDisabledControl()) return;
      if (this.nodeName === "INPUT") {
        const type = this.type.toLowerCase();
        if (type === "checkbox") this.checked = !this.checked;
        else if (type === "radio") this.checked = true;
      }
      // The click event's activation behavior (form submit/reset) is its
      // default action; dispatchEvent runs it when the event is not canceled.
      this.dispatchEvent(
        new MouseEvent("click", { bubbles: true, cancelable: true })
      );
    }

    // Nearest ancestor <form>, or null. The `form` content-attribute
    // association is not modeled; ancestry suffices for our needs.
    __owningForm() {
      let node = this.parentNode;
      while (node) {
        if (node.nodeType === 1 && node.tagName === "FORM") return node;
        node = node.parentNode;
      }
      return null;
    }

    __runActivationBehavior() {
      // A disabled form control has no activation behavior, so it never submits
      // or resets its form -- not even for a synthetic click dispatched
      // directly through dispatchEvent.
      if (this.__isDisabledControl()) return;
      const tag = this.nodeName;
      let type = "";
      try {
        type = (this.type || "").toLowerCase();
      } catch (_e) {
        type = "";
      }
      const isSubmit =
        (tag === "INPUT" && (type === "submit" || type === "image")) ||
        (tag === "BUTTON" && (type === "submit" || type === ""));
      const isReset =
        (tag === "INPUT" || tag === "BUTTON") && type === "reset";
      if (!isSubmit && !isReset) return;
      const form = this.__owningForm();
      if (!form) return;
      if (isSubmit) form.__submit(this);
      else form.__reset();
    }

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
      return __omoikane_get_checked(this.__id);
    }

    set checked(v) {
      const checked = !!v;
      if (checked && this.nodeName === "INPUT" && this.type.toLowerCase() === "radio") {
        const document = this.ownerDocument;
        const name = this.name;
        if (document) {
          for (const radio of document.querySelectorAll("input[type=radio]")) {
            if (radio.__id !== this.__id && radio.name === name) {
              __omoikane_set_checked(radio.__id, false);
            }
          }
        }
      }
      __omoikane_set_checked(this.__id, checked);
    }

    get defaultChecked() {
      return this.hasAttribute("checked");
    }

    set defaultChecked(v) {
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
      if (this.nodeType === 10) return this.nodeName;
      return __omoikane_get_attribute(this.__id, "name") ?? "";
    }

    set name(v) {
      if (this.nodeType === 10) return;
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
  class Element extends Node {
    remove() { removeChildNode.call(this); }
  }

  // The bootstrap originally implemented every DOM wrapper through one large
  // Node class. Keep the implementation bodies together for now, but expose
  // Element's WebIDL surface from the correct prototype. Moving the complete
  // descriptors preserves getters/setters and their attributes while ensuring
  // Document, DocumentFragment and CharacterData do not accidentally expose
  // element-only APIs through Node.prototype.
  function distributePrototypeMembers(source, targets, names) {
    for (const name of names) {
      const descriptor = Object.getOwnPropertyDescriptor(source, name);
      if (!descriptor) continue;
      for (const target of targets) Object.defineProperty(target, name, descriptor);
      if (!delete source[name]) {
        throw new TypeError("Failed to move " + name + " off its source prototype");
      }
    }
  }

  distributePrototypeMembers(Node.prototype, [Element.prototype], [
    "namespaceURI", "prefix", "localName", "tagName",
    "id", "className", "classList",
    "getAttribute", "setAttribute", "hasAttribute", "removeAttribute",
    "setAttributeNS", "getAttributeNS", "removeAttributeNS", "attributes",
    "matches", "closest",
    "__layoutMetrics", "getBoundingClientRect", "getClientRects",
    "clientWidth", "clientHeight", "clientTop", "clientLeft",
    "scrollWidth", "scrollHeight", "scrollTop", "scrollLeft",
  ]);

  class HTMLElement extends Element {}
  class HTMLHtmlElement extends HTMLElement {}
  class HTMLHeadElement extends HTMLElement {}
  class HTMLBodyElement extends HTMLElement {}
  class HTMLDivElement extends HTMLElement {}
  class HTMLSpanElement extends HTMLElement {}
  class HTMLParagraphElement extends HTMLElement {}
  class HTMLAnchorElement extends HTMLElement {}

  distributePrototypeMembers(Node.prototype, [HTMLElement.prototype], [
    "title", "innerText",
    "offsetWidth", "offsetHeight", "offsetTop", "offsetLeft", "offsetParent",
    "focus", "blur", "click", "hidden",
    "__isDisabledControl", "__owningForm", "__runActivationBehavior",
  ]);

  class CharacterData extends Node {
    remove() { removeChildNode.call(this); }
    get textContent() {
      if (Object.prototype.hasOwnProperty.call(this, "__characterData")) return this.__characterData;
      return super.textContent;
    }

    set textContent(value) {
      const text = value == null ? "" : String(value);
      // Preserve the authoritative WTF-16 value on the JS wrapper because
      // the native Rust DOM cannot represent unpaired UTF-16 surrogates.
      this.__characterData = text;
      super.textContent = text;
    }

    get data() {
      const value = this.textContent;
      return value == null ? "" : String(value);
    }

    set data(value) {
      this.replaceData(0, this.length, value === null ? "" : String(value));
    }

    get length() {
      return this.data.length;
    }

    appendData(data) {
      if (arguments.length < 1) throw new TypeError("appendData requires 1 argument");
      this.replaceData(this.length, 0, String(data));
    }

    substringData(offset, count) {
      if (arguments.length < 2) throw new TypeError("substringData requires 2 arguments");
      offset = Number(offset) >>> 0;
      count = Number(count) >>> 0;
      if (offset > this.length) throw new DOMException("Offset is outside the data.", "IndexSizeError");
      return this.data.slice(offset, Math.min(this.length, offset + count));
    }

    insertData(offset, data) {
      if (arguments.length < 2) throw new TypeError("insertData requires 2 arguments");
      this.replaceData(Number(offset) >>> 0, 0, String(data));
    }

    deleteData(offset, count) {
      if (arguments.length < 2) throw new TypeError("deleteData requires 2 arguments");
      this.replaceData(Number(offset) >>> 0, Number(count) >>> 0, "");
    }

    replaceData(offset, count, data) {
      if (arguments.length < 3) throw new TypeError("replaceData requires 3 arguments");
      offset = Number(offset) >>> 0;
      count = Number(count) >>> 0;
      data = String(data);
      const length = this.length;
      if (offset > length) throw new DOMException("Offset is outside the data.", "IndexSizeError");
      count = Math.min(count, length - offset);
      const state = traversalByDocument.get(traversalDocumentKey(nodeDocument(this)));
      if (state) {
        for (const range of traversalEntries(state.ranges)) {
          range.__replaceData(this, offset, count, data.length);
        }
      }
      const current = this.data;
      queueMutation(this, "characterData", { oldValue: current });
      this.textContent = current.slice(0, offset) + data + current.slice(offset + count);
    }
  }

  class Text extends CharacterData {
    splitText(offset) {
      offset = Number(offset) >>> 0;
      if (offset > this.length) throw new DOMException("Offset is outside the data.", "IndexSizeError");
      const oldData = this.data;
      const newNode = nodeDocument(this).createTextNode(oldData.slice(offset));
      const parent = this.parentNode;
      const index = indexOfNode(this);
      if (parent) parent.insertBefore(newNode, this.nextSibling);
      const state = traversalByDocument.get(traversalDocumentKey(nodeDocument(this)));
      if (state) {
        for (const range of traversalEntries(state.ranges)) range.__splitText(this, newNode, offset, parent, index);
      }
      this.replaceData(offset, this.length - offset, "");
      return newNode;
    }
  }
  class CDATASection extends Text {}
  class Comment extends CharacterData {}
  class ProcessingInstruction extends CharacterData {
    get target() { return this.nodeName; }
  }

  const NodeFilter = {
    FILTER_ACCEPT: 1, FILTER_REJECT: 2, FILTER_SKIP: 3,
    SHOW_ALL: 0xffffffff, SHOW_ELEMENT: 0x1, SHOW_ATTRIBUTE: 0x2,
    SHOW_TEXT: 0x4, SHOW_CDATA_SECTION: 0x8,
    SHOW_ENTITY_REFERENCE: 0x10, SHOW_ENTITY: 0x20,
    SHOW_PROCESSING_INSTRUCTION: 0x40, SHOW_COMMENT: 0x80,
    SHOW_DOCUMENT: 0x100, SHOW_DOCUMENT_TYPE: 0x200,
    SHOW_DOCUMENT_FRAGMENT: 0x400, SHOW_NOTATION: 0x800,
  };

  function filterNode(node, whatToShow, filter) {
    const bit = 1 << (node.nodeType - 1);
    if ((whatToShow & bit) === 0) return NodeFilter.FILTER_SKIP;
    if (filter == null) return NodeFilter.FILTER_ACCEPT;
    const result = typeof filter === "function"
      ? filter(node)
      : filter.acceptNode(node);
    return Number(result);
  }

  function nextInTree(node, root) {
    if (node.firstChild) return node.firstChild;
    while (node && node !== root) {
      if (node.nextSibling) return node.nextSibling;
      node = node.parentNode;
    }
    return null;
  }

  function previousInTree(node, root) {
    if (!node || node === root) return null;
    if (node.previousSibling) {
      node = node.previousSibling;
      while (node.lastChild) node = node.lastChild;
      return node;
    }
    return node.parentNode;
  }

  class NodeIterator {
    constructor(root, whatToShow, filter) {
      this.root = root;
      this.whatToShow = whatToShow >>> 0;
      this.filter = filter || null;
      this.referenceNode = root;
      this.pointerBeforeReferenceNode = true;
      registerTraversal(nodeDocument(root), "iterators", this);
    }

    nextNode() {
      let candidate = this.pointerBeforeReferenceNode
        ? this.referenceNode : nextInTree(this.referenceNode, this.root);
      while (candidate) {
        const fallbackNext = nextInTree(candidate, this.root);
        const oldReference = this.referenceNode, oldPointer = this.pointerBeforeReferenceNode;
        const result = filterNode(candidate, this.whatToShow, this.filter);
        const adjusted = this.referenceNode !== oldReference || this.pointerBeforeReferenceNode !== oldPointer;
        if (!adjusted) {
          if (candidate !== this.root && !isInclusiveDescendant(candidate, this.root) && fallbackNext) {
            this.referenceNode = fallbackNext; this.pointerBeforeReferenceNode = true;
          } else { this.referenceNode = candidate; this.pointerBeforeReferenceNode = false; }
        }
        if (result === NodeFilter.FILTER_ACCEPT) return candidate;
        candidate = adjusted
          ? (this.pointerBeforeReferenceNode ? this.referenceNode : nextInTree(this.referenceNode, this.root))
          : nextInTree(candidate, this.root);
      }
      return null;
    }

    previousNode() {
      let candidate = this.pointerBeforeReferenceNode
        ? previousInTree(this.referenceNode, this.root) : this.referenceNode;
      while (candidate) {
        const fallbackPrevious = previousInTree(candidate, this.root);
        const oldReference = this.referenceNode, oldPointer = this.pointerBeforeReferenceNode;
        const result = filterNode(candidate, this.whatToShow, this.filter);
        const adjusted = this.referenceNode !== oldReference || this.pointerBeforeReferenceNode !== oldPointer;
        if (!adjusted) {
          if (candidate !== this.root && !isInclusiveDescendant(candidate, this.root) && fallbackPrevious) {
            this.referenceNode = fallbackPrevious; this.pointerBeforeReferenceNode = false;
          } else { this.referenceNode = candidate; this.pointerBeforeReferenceNode = true; }
        }
        if (result === NodeFilter.FILTER_ACCEPT) return candidate;
        candidate = adjusted
          ? (this.pointerBeforeReferenceNode ? previousInTree(this.referenceNode, this.root) : this.referenceNode)
          : previousInTree(candidate, this.root);
      }
      return null;
    }

    detach() { unregisterTraversal(nodeDocument(this.root), "iterators", this); }

    __preRemove(removed) {
      if (!isInclusiveDescendant(this.referenceNode, removed) || isInclusiveDescendant(this.root, removed)) return;
      if (this.pointerBeforeReferenceNode) {
        let next = removed.nextSibling;
        let parent = removed.parentNode;
        while (!next && parent && parent !== this.root) {
          next = parent.nextSibling;
          parent = parent.parentNode;
        }
        if (next) {
          this.referenceNode = next;
          return;
        }
        this.pointerBeforeReferenceNode = false;
      }
      let previous = removed.previousSibling;
      if (!previous) previous = removed.parentNode;
      else while (previous.lastChild) previous = previous.lastChild;
      if (previous) this.referenceNode = previous;
    }
  }

  class TreeWalker {
    constructor(root, whatToShow, filter) {
      this.root = root;
      this.whatToShow = whatToShow >>> 0;
      this.filter = filter || null;
      this.currentNode = root;
    }
    __accept(node) { return filterNode(node, this.whatToShow, this.filter); }
    parentNode() {
      let n = this.currentNode;
      while (n && n !== this.root) {
        n = n.parentNode;
        if (this.__accept(n) === NodeFilter.FILTER_ACCEPT) return (this.currentNode = n);
      }
      return null;
    }
    firstChild() { return this.__child(false); }
    lastChild() { return this.__child(true); }
    __child(reverse) {
      let n = reverse ? this.currentNode.lastChild : this.currentNode.firstChild;
      while (n) {
        const result = this.__accept(n);
        if (result === NodeFilter.FILTER_ACCEPT) return (this.currentNode = n);
        if (result === NodeFilter.FILTER_SKIP) {
          const child = reverse ? n.lastChild : n.firstChild;
          if (child) { n = child; continue; }
        }
        while (n && n !== this.currentNode) {
          const sibling = reverse ? n.previousSibling : n.nextSibling;
          if (sibling) { n = sibling; break; }
          n = n.parentNode;
        }
        if (n === this.currentNode) return null;
      }
      return null;
    }
    nextSibling() { return this.__sibling(false); }
    previousSibling() { return this.__sibling(true); }
    __sibling(reverse) {
      let n = this.currentNode;
      if (n === this.root) return null;
      while (n && n !== this.root) {
        let sibling = reverse ? n.previousSibling : n.nextSibling;
        while (sibling) {
          n = sibling;
          const result = this.__accept(n);
          if (result === NodeFilter.FILTER_ACCEPT) return (this.currentNode = n);
          if (result === NodeFilter.FILTER_SKIP) {
            const child = reverse ? n.lastChild : n.firstChild;
            if (child) { sibling = child; continue; }
          }
          sibling = reverse ? n.previousSibling : n.nextSibling;
        }
        n = n.parentNode;
        if (reverse) return null;
        if (n && this.__accept(n) === NodeFilter.FILTER_ACCEPT) return null;
      }
      return null;
    }
    nextNode() {
      let n = this.currentNode;
      let descend = true;
      while (n) {
        if (descend) {
          const result = n === this.currentNode ? NodeFilter.FILTER_SKIP : this.__accept(n);
          if (n !== this.currentNode && result === NodeFilter.FILTER_ACCEPT) return (this.currentNode = n);
          if (result !== NodeFilter.FILTER_REJECT && n.firstChild) { n = n.firstChild; descend = true; continue; }
        }
        if (n === this.root) return null;
        if (n.nextSibling) { n = n.nextSibling; descend = true; continue; }
        n = n.parentNode; descend = false;
      }
      return null;
    }
    previousNode() {
      let n = this.currentNode;
      while (n && n !== this.root) {
        if (n.previousSibling) {
          n = n.previousSibling;
          while (true) {
            const result = this.__accept(n);
            if (result !== NodeFilter.FILTER_REJECT && n.lastChild) { n = n.lastChild; continue; }
            if (result === NodeFilter.FILTER_ACCEPT) return (this.currentNode = n);
            break;
          }
        } else {
          n = n.parentNode;
          if (n && this.__accept(n) === NodeFilter.FILTER_ACCEPT) return (this.currentNode = n);
        }
      }
      return null;
    }
  }

  function nodeLength(node) {
    return node.nodeType === 3 || node.nodeType === 7 || node.nodeType === 8
      ? node.length
      : node.childNodes.length;
  }

  function boundaryCompare(aNode, aOffset, bNode, bOffset) {
    if (aNode === bNode) return aOffset < bOffset ? -1 : (aOffset > bOffset ? 1 : 0);
    if (isInclusiveDescendant(bNode, aNode)) {
      let child = bNode;
      while (child.parentNode !== aNode) child = child.parentNode;
      return aOffset <= indexOfNode(child) ? -1 : 1;
    }
    if (isInclusiveDescendant(aNode, bNode)) {
      let child = aNode;
      while (child.parentNode !== bNode) child = child.parentNode;
      return indexOfNode(child) < bOffset ? -1 : 1;
    }
    const aPath = [], bPath = [];
    for (let n = aNode; n; n = n.parentNode) aPath.unshift(n);
    for (let n = bNode; n; n = n.parentNode) bPath.unshift(n);
    let i = 0;
    while (i < aPath.length && aPath[i] === bPath[i]) i++;
    if (i === 0) return 0;
    return indexOfNode(aPath[i]) < indexOfNode(bPath[i]) ? -1 : 1;
  }

  function commonAncestor(a, b) {
    const ancestors = new Set();
    for (let n = a; n; n = n.parentNode) ancestors.add(n);
    for (let n = b; n; n = n.parentNode) if (ancestors.has(n)) return n;
    return null;
  }

  class Range {
    constructor(doc) {
      this.__doc = doc;
      this.__startContainer = doc; this.__startOffset = 0;
      this.__endContainer = doc; this.__endOffset = 0;
      registerTraversal(doc, "ranges", this);
    }
    get startContainer() { return this.__startContainer; }
    get startOffset() { return this.__startOffset; }
    get endContainer() { return this.__endContainer; }
    get endOffset() { return this.__endOffset; }
    get collapsed() { return this.__startContainer === this.__endContainer && this.__startOffset === this.__endOffset; }
    get commonAncestorContainer() { return commonAncestor(this.__startContainer, this.__endContainer); }
    __validate(node, offset) {
      if (!node || node.nodeType === 10) throw new DOMException("Invalid boundary node.", "InvalidNodeTypeError");
      offset = Number(offset) >>> 0;
      if (offset > nodeLength(node)) throw new DOMException("Offset is outside the node.", "IndexSizeError");
      return offset;
    }
    setStart(node, offset) {
      offset = this.__validate(node, offset);
      const doc = nodeDocument(node);
      if (doc !== this.__doc) {
        unregisterTraversal(this.__doc, "ranges", this);
        this.__doc = doc;
        registerTraversal(doc, "ranges", this);
      }
      // Living DOM reroots (collapses) a range when the new point and the
      // opposite point have different roots; it does not throw the DOM2
      // WrongDocumentError. Updating __doc above keeps mutation tracking on the
      // same Document as both newly collapsed boundary points.
      if (nodeRoot(node) !== nodeRoot(this.__endContainer) ||
          boundaryCompare(node, offset, this.__endContainer, this.__endOffset) > 0) {
        this.__endContainer = node; this.__endOffset = offset;
      }
      this.__startContainer = node; this.__startOffset = offset;
    }
    setEnd(node, offset) {
      offset = this.__validate(node, offset);
      const doc = nodeDocument(node);
      if (doc !== this.__doc) {
        unregisterTraversal(this.__doc, "ranges", this);
        this.__doc = doc;
        registerTraversal(doc, "ranges", this);
      }
      if (nodeRoot(node) !== nodeRoot(this.__startContainer) ||
          boundaryCompare(node, offset, this.__startContainer, this.__startOffset) < 0) {
        this.__startContainer = node; this.__startOffset = offset;
      }
      this.__endContainer = node; this.__endOffset = offset;
    }
    __beforeAfter(node, delta, start) {
      if (!node || !node.parentNode) throw new DOMException("Node has no parent.", "InvalidNodeTypeError");
      const parent = node.parentNode, offset = indexOfNode(node) + delta;
      start ? this.setStart(parent, offset) : this.setEnd(parent, offset);
    }
    setStartBefore(node) { this.__beforeAfter(node, 0, true); }
    setStartAfter(node) { this.__beforeAfter(node, 1, true); }
    setEndBefore(node) { this.__beforeAfter(node, 0, false); }
    setEndAfter(node) { this.__beforeAfter(node, 1, false); }
    selectNode(node) {
      if (!node || !node.parentNode) throw new DOMException("Node has no parent.", "InvalidNodeTypeError");
      const parent = node.parentNode, index = indexOfNode(node);
      this.__startContainer = parent; this.__startOffset = index;
      this.__endContainer = parent; this.__endOffset = index + 1;
    }
    selectNodeContents(node) {
      this.__validate(node, 0);
      this.__startContainer = node; this.__startOffset = 0;
      this.__endContainer = node; this.__endOffset = nodeLength(node);
    }
    collapse(toStart = false) {
      if (toStart) { this.__endContainer = this.__startContainer; this.__endOffset = this.__startOffset; }
      else { this.__startContainer = this.__endContainer; this.__startOffset = this.__endOffset; }
    }
    cloneRange() {
      const r = new Range(this.__doc);
      r.__startContainer=this.__startContainer; r.__startOffset=this.__startOffset;
      r.__endContainer=this.__endContainer; r.__endOffset=this.__endOffset;
      return r;
    }
    compareBoundaryPoints(how, source) {
      if (how === Range.START_TO_START) return boundaryCompare(this.__startContainer,this.__startOffset,source.__startContainer,source.__startOffset);
      if (how === Range.START_TO_END) return boundaryCompare(this.__endContainer,this.__endOffset,source.__startContainer,source.__startOffset);
      if (how === Range.END_TO_END) return boundaryCompare(this.__endContainer,this.__endOffset,source.__endContainer,source.__endOffset);
      if (how === Range.END_TO_START) return boundaryCompare(this.__startContainer,this.__startOffset,source.__endContainer,source.__endOffset);
      throw new DOMException("Invalid comparison mode.", "NotSupportedError");
    }
    __nodeRelation(node) {
      const parent = node.parentNode;
      if (!parent) return 0;
      const i = indexOfNode(node);
      const startsBeforeEnd = boundaryCompare(parent,i,this.__endContainer,this.__endOffset) < 0;
      const endsAfterStart = boundaryCompare(parent,i+1,this.__startContainer,this.__startOffset) > 0;
      if (!startsBeforeEnd || !endsAfterStart) return 0;
      const full = boundaryCompare(this.__startContainer,this.__startOffset,parent,i) <= 0 &&
        boundaryCompare(parent,i+1,this.__endContainer,this.__endOffset) <= 0;
      return full ? 2 : 1;
    }
    __characterSlice(node) {
      let from = 0, to = node.length;
      if (node === this.__startContainer) from = this.__startOffset;
      if (node === this.__endContainer) to = this.__endOffset;
      return [from, to];
    }
    __copyNode(node, extract) {
      if (node.nodeType === 3 || node.nodeType === 7 || node.nodeType === 8) {
        const [from,to] = this.__characterSlice(node);
        const data = node.data.slice(from,to);
        const copy = node.nodeType === 3
          ? this.__doc.createTextNode(data)
          : node.nodeType === 7
            ? this.__doc.createProcessingInstruction(node.target, data)
            : this.__doc.createComment(data);
        if (extract) node.data = node.data.slice(0,from) + node.data.slice(to);
        return copy;
      }
      const relation = this.__nodeRelation(node);
      if (relation === 2) {
        if (extract) return node;
        return node.cloneNode(true);
      }
      const copy = node.cloneNode(false);
      for (const child of node.childNodes.slice()) {
        const rel = this.__nodeRelation(child);
        if (rel) copy.appendChild(rel === 2 && extract ? child : this.__copyNode(child, extract));
      }
      return copy;
    }
    __contents(extract) {
      const fragment = this.__doc.createDocumentFragment();
      if (this.collapsed) return fragment;
      if (this.__startContainer === this.__endContainer && (this.__startContainer.nodeType === 3 || this.__startContainer.nodeType === 7 || this.__startContainer.nodeType === 8)) {
        fragment.appendChild(this.__copyNode(this.__startContainer, extract));
      } else {
        const common = this.commonAncestorContainer;
        if (common.nodeType === 3 || common.nodeType === 7 || common.nodeType === 8) fragment.appendChild(this.__copyNode(common, extract));
        else for (const child of common.childNodes.slice()) if (this.__nodeRelation(child)) fragment.appendChild(this.__copyNode(child, extract));
      }
      if (extract) this.collapse(true);
      return fragment;
    }
    cloneContents() { return this.__contents(false); }
    extractContents() { return this.__contents(true); }
    deleteContents() { this.__contents(true); }
    insertNode(node) {
      let parent, reference;
      if (this.__startContainer.nodeType === 3) {
        reference = this.__startContainer.splitText(this.__startOffset);
        parent = reference.parentNode;
      } else {
        parent = this.__startContainer;
        reference = parent.childNodes[this.__startOffset] || null;
      }
      if (!parent) throw new DOMException("Cannot insert at this boundary.", "HierarchyRequestError");
      if (parent.nodeType === 9 && node.nodeType === 1 && parent.documentElement) throw new DOMException("Document already has an element.", "HierarchyRequestError");
      const count = node.nodeType === 11 ? node.childNodes.length : 1;
      parent.insertBefore(node, reference);
      if (this.collapsed) { this.__endContainer = parent; this.__endOffset = indexOfNode(reference) < 0 ? parent.childNodes.length : indexOfNode(reference); }
      else if (this.__endContainer === parent && this.__endOffset === this.__startOffset) this.__endOffset += count;
    }
    surroundContents(newParent) {
      if ([9,10,11].includes(newParent.nodeType)) throw new DOMException("Invalid wrapper.", "InvalidNodeTypeError");
      if (this.__startContainer !== this.__endContainer &&
          ((this.__startContainer.nodeType === 3 || this.__startContainer.nodeType === 7 || this.__startContainer.nodeType === 8) ||
           (this.__endContainer.nodeType === 3 || this.__endContainer.nodeType === 7 || this.__endContainer.nodeType === 8))) {
        throw new DOMException("Range partially contains a non-Text node.", "InvalidStateError");
      }
      const fragment = this.extractContents();
      this.insertNode(newParent);
      newParent.textContent = "";
      newParent.appendChild(fragment);
      this.selectNode(newParent);
    }
    toString() {
      if (this.collapsed) return "";
      let result = "";
      const root = this.commonAncestorContainer;
      const visit = node => {
        if (node.nodeType === 3) {
          let from=0,to=node.length;
          if (node===this.__startContainer) from=this.__startOffset;
          if (node===this.__endContainer) to=this.__endOffset;
          if ((node===this.__startContainer || node===this.__endContainer || this.__nodeRelation(node))) result += node.data.slice(from,to);
        } else for (const child of node.childNodes) visit(child);
      };
      visit(root); return result;
    }
    detach() { unregisterTraversal(this.__doc, "ranges", this); }
    __preRemove(parent, removed) {
      const index = indexOfNode(removed);
      const adjust = (container, offset) => {
        if (isInclusiveDescendant(container, removed)) return [parent,index];
        if (container === parent && offset > index) return [container,offset-1];
        return [container,offset];
      };
      [this.__startContainer,this.__startOffset]=adjust(this.__startContainer,this.__startOffset);
      [this.__endContainer,this.__endOffset]=adjust(this.__endContainer,this.__endOffset);
    }
    __mergeText(target, removed, offset, parent, index) {
      const adjust = (container, value) => {
        if (container === removed) return [target, offset + value];
        if (container === parent && value === index) return [target, offset];
        return [container, value];
      };
      [this.__startContainer, this.__startOffset] = adjust(this.__startContainer, this.__startOffset);
      [this.__endContainer, this.__endOffset] = adjust(this.__endContainer, this.__endOffset);
    }
    __replaceData(node, offset, count, replacementLength) {
      const adjust = (container, value) => {
        if (container !== node || value <= offset) return [container, value];
        if (value <= offset + count) return [container, offset];
        return [container, value + replacementLength - count];
      };
      [this.__startContainer, this.__startOffset] = adjust(this.__startContainer, this.__startOffset);
      [this.__endContainer, this.__endOffset] = adjust(this.__endContainer, this.__endOffset);
    }
    __splitText(oldNode,newNode,offset,parent,index) {
      const adjust=(container,value) => container===oldNode && value>offset ? [newNode,value-offset] : [container,value];
      [this.__startContainer,this.__startOffset]=adjust(this.__startContainer,this.__startOffset);
      [this.__endContainer,this.__endOffset]=adjust(this.__endContainer,this.__endOffset);
      if (parent) {
        if (this.__startContainer===parent && this.__startOffset>index) this.__startOffset++;
        if (this.__endContainer===parent && this.__endOffset>index) this.__endOffset++;
      }
    }
  }
  Range.START_TO_START=0; Range.START_TO_END=1; Range.END_TO_END=2; Range.END_TO_START=3;
  Range.prototype.START_TO_START=0; Range.prototype.START_TO_END=1; Range.prototype.END_TO_END=2; Range.prototype.END_TO_START=3;

  // Elements whose `name` content attribute participates in HTMLCollection
  // named access, in addition to `id` (which applies to every element). Per the
  // HTML spec these are the "named" elements exposed on collections such as
  // `document.forms` / `document.images` / `document.anchors`.
  const COLLECTION_NAME_TAGS = new Set([
    "A", "AREA", "FORM", "IMG", "OBJECT", "EMBED", "IFRAME", "INPUT", "MAP",
  ]);

  // Walks `root`'s subtree in tree (document) order and returns every element
  // for which `predicate` holds. Used to build the live document HTMLCollections
  // (forms / links / images / anchors), scoped to `root`'s own document tree so
  // an iframe's contentDocument never leaks nodes into the main document.
  function collectElements(root, predicate) {
    const out = [];
    const walk = (node) => {
      for (const child of node.childNodes) {
        if (child.nodeType !== 1) continue;
        if (predicate(child)) out.push(child);
        walk(child);
      }
    };
    walk(root);
    return out;
  }

  // Builds a live HTMLCollection over the elements returned by `collect()`.
  // `collect()` is re-invoked on every access so the collection always reflects
  // the current tree (DOM "live" semantics), even when the collection object is
  // retained across mutations. Supports `.length`, integer index access,
  // `item(index)`, `namedItem(name)`, iteration, and named property access by
  // `id` (any element) or `name` (elements in COLLECTION_NAME_TAGS). Out-of-range
  // index access resolves to `null`.
  function makeHTMLCollection(collect) {
    // Per spec an id match wins over a name match; both scan in tree order.
    const byName = (list, key) => {
      let named = null;
      for (const el of list) {
        if (!el.getAttribute) continue;
        if (el.getAttribute("id") === key) return el;
        if (named === null &&
            COLLECTION_NAME_TAGS.has(el.tagName) &&
            el.getAttribute("name") === key) {
          named = el;
        }
      }
      return named;
    };
    const isIndex = (prop) =>
      typeof prop === "string" && /^(?:0|[1-9]\d*)$/.test(prop);
    return new Proxy([], {
      get(_target, prop) {
        const list = collect();
        if (prop === "length") return list.length;
        if (prop === "item") return (index) => list[Number(index) | 0] ?? null;
        if (prop === "namedItem") return (name) => byName(list, String(name));
        if (isIndex(prop)) return list[Number(prop)] ?? null;
        if (typeof prop === "string") {
          const named = byName(list, prop);
          if (named) return named;
        }
        // Array prototype members (Symbol.iterator, forEach, ...) and anything
        // else resolve against the live snapshot; bind methods to it.
        const value = list[prop];
        return typeof value === "function" ? value.bind(list) : value;
      },
      has(_target, prop) {
        const list = collect();
        if (prop === "length" || prop === "item" || prop === "namedItem") {
          return true;
        }
        if (isIndex(prop)) return Number(prop) < list.length;
        if (typeof prop === "string" && byName(list, prop)) return true;
        return prop in list;
      },
    });
  }

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
      const expected = String(id);
      // Plain tree walk with an id equality check: getElementById needs no
      // selector parsing/matching and no full-document snapshot.
      const findById = (node) => {
        for (const child of node.childNodes) {
          if (child.nodeType !== 1) continue;
          if (child.getAttribute("id") === expected) return child;
          const found = findById(child);
          if (found) return found;
        }
        return null;
      };
      return findById(scope);
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
      if (info.namespace === SVG_NAMESPACE) {
        const ctor = SVG_ELEMENT_CTORS[info.localName.toLowerCase()] || SVGElement;
        Object.setPrototypeOf(node, ctor.prototype);
      } else if (info.namespace === HTML_NAMESPACE) {
        const ctor = ELEMENT_CTORS[info.localName.toLowerCase()] || HTMLElement;
        Object.setPrototypeOf(node, ctor.prototype);
      } else {
        Object.setPrototypeOf(node, Element.prototype);
      }
      return node;
    }

    get implementation() {
      const ownerDocument = this;
      return {
        hasFeature() {
          return true;
        },
        createDocumentType(qualifiedName, publicId, systemId) {
          const name = String(qualifiedName);
          validateQualifiedName(name);
          const publicIdentifier = publicId == null ? "" : String(publicId);
          const systemIdentifier = systemId == null ? "" : String(systemId);
          const node = ownerDocument.__own(wrapNode(
            __omoikane_create_document_type(name, publicIdentifier, systemIdentifier)
          ));
          Object.defineProperties(node, {
            name: { value: name, configurable: true },
            publicId: { value: publicIdentifier, configurable: true },
            systemId: { value: systemIdentifier, configurable: true },
            internalSubset: { value: null, configurable: true },
          });
          return node;
        },
        createDocument(namespace, qualifiedName, doctype) {
          const ns = (namespace === undefined || namespace === null || namespace === "")
            ? null
            : String(namespace);
          const qname = qualifiedName == null ? "" : String(qualifiedName);
          if (qname !== "") validateAndExtractNS(ns, qname);

          const doc = wrapNode(__omoikane_create_document());
          let root = null;
          if (qname !== "") root = doc.createElementNS(ns, qname);
          if (doctype !== undefined && doctype !== null) doc.appendChild(doctype);
          if (root) doc.appendChild(root);
          return doc;
        },
        createHTMLDocument(title) {
          const doc = wrapNode(__omoikane_create_document());
          doc.__documentURL = "about:blank";
          const doctype = doc.implementation.createDocumentType("html", "", "");
          const html = doc.createElement("html");
          const head = doc.createElement("head");
          const titleElement = doc.createElement("title");
          const body = doc.createElement("body");

          if (title !== undefined) {
            titleElement.appendChild(doc.createTextNode(String(title)));
          }
          head.appendChild(titleElement);
          html.appendChild(head);
          html.appendChild(body);
          doc.appendChild(doctype);
          doc.appendChild(html);
          return doc;
        },
      };
    }

    createDocumentFragment() {
      return this.__own(wrapNode(__omoikane_create_document_fragment()));
    }

    get styleSheets() {
      if (!this.__styleSheets) this.__styleSheets = makeStyleSheetList(this);
      return this.__styleSheets;
    }

    createTextNode(text) {
      return this.__own(wrapNode(__omoikane_create_text_node(String(text))));
    }

    createCDATASection(data) {
      if (arguments.length < 1) throw new TypeError("createCDATASection requires 1 argument");
      const text = String(data);
      if (text.includes("]]>")) {
        throw new DOMException("CDATA data must not contain ]]>", "InvalidCharacterError");
      }
      const node = this.createTextNode(text);
      Object.setPrototypeOf(node, CDATASection.prototype);
      Object.defineProperty(node, "__cdataSection", { value: true, configurable: true });
      return node;
    }

    createProcessingInstruction(target, data) {
      const normalizedTarget = String(target);
      const normalizedData = String(data);
      if (!isValidXmlName(normalizedTarget) || normalizedData.includes("?>")) {
        throw new DOMException(
          "The processing instruction target or data is invalid.",
          "InvalidCharacterError"
        );
      }
      return this.__own(wrapNode(
        __omoikane_create_processing_instruction(normalizedTarget, normalizedData)
      ));
    }

    createComment(data) {
      return this.__own(wrapNode(__omoikane_create_comment(String(data ?? ""))));
    }

    createNodeIterator(root, whatToShow = NodeFilter.SHOW_ALL, filter = null) {
      return new NodeIterator(root, whatToShow, filter);
    }

    createTreeWalker(root, whatToShow = NodeFilter.SHOW_ALL, filter = null) {
      return new TreeWalker(root, whatToShow, filter);
    }

    createRange() { return new Range(this); }

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
      for (const child of this.childNodes) {
        if (child.nodeType === 1) return child;
      }
      return null;
    }

    get doctype() {
      for (const child of this.childNodes) {
        if (child.nodeType === 10) return child;
      }
      return null;
    }

    get title() {
      const title = this.getElementsByTagName("title")[0];
      return title ? title.textContent : "";
    }

    set title(value) {
      const title = this.getElementsByTagName("title")[0];
      if (title) title.textContent = String(value);
    }

    // Live HTMLCollection of every <form> in this document, in tree order, with
    // index / `length` / `item` / `namedItem` and named access by `name` or `id`
    // (e.g. `document.forms.myForm`). Scoped to this document's own tree, so an
    // iframe's contentDocument resolves its own forms.
    get forms() {
      const root = this;
      return makeHTMLCollection(() =>
        collectElements(root, (el) =>
          el.tagName === "FORM" ||
          (el.localName === "form" && el.namespaceURI === "http://www.w3.org/1999/xhtml")));
    }

    // Live HTMLCollection of the <a> and <area> elements that carry an `href`
    // content attribute, in tree order.
    get links() {
      const root = this;
      return makeHTMLCollection(() =>
        collectElements(
          root,
          (el) =>
            (el.tagName === "A" || el.tagName === "AREA") &&
            el.hasAttribute("href"),
        ));
    }

    // Live HTMLCollection of every <img> in this document, in tree order.
    get images() {
      const root = this;
      return makeHTMLCollection(() =>
        collectElements(root, (el) => el.tagName === "IMG"));
    }

    // Live HTMLCollection of the <a> elements that carry a `name` content
    // attribute, in tree order.
    get anchors() {
      const root = this;
      return makeHTMLCollection(() =>
        collectElements(
          root,
          (el) => el.tagName === "A" && el.hasAttribute("name"),
        ));
    }

    get readyState() {
      return this.__readyState || "complete";
    }

    get characterSet() {
      return "UTF-8";
    }

    get charset() {
      return this.characterSet;
    }

    get location() {
      return globalThis.location;
    }

    get referrer() {
      return "";
    }

    get contentType() {
      return "text/html";
    }

    get URL() {
      return this.__documentURL || globalThis.location.href;
    }

    get documentURI() {
      return this.__documentURL || globalThis.location.href;
    }

    get compatMode() {
      return "CSS1Compat";
    }

    get currentScript() {
      return this.__currentScript || null;
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

    getElementsByName(name) {
      const expected = String(name);
      return this.querySelectorAll("[name]").filter(element =>
        element.getAttribute("name") === expected
      );
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

  class DocumentType extends Node {
    remove() { removeChildNode.call(this); }
  }

  // ParentNode is included by Document, DocumentFragment and Element. The two
  // collection helpers are instead declared directly by Document and Element.
  distributePrototypeMembers(Node.prototype, [
    Element.prototype, Document.prototype, DocumentFragment.prototype,
  ], [
    "querySelector", "querySelectorAll", "children",
    "firstElementChild", "lastElementChild", "childElementCount",
  ]);
  distributePrototypeMembers(Node.prototype, [Element.prototype, Document.prototype], [
    "getElementsByTagName", "getElementsByClassName", "innerHTML",
  ]);
  distributePrototypeMembers(Node.prototype, [DocumentType.prototype], [
    "publicId", "systemId", "internalSubset",
  ]);

  // NonDocumentTypeChildNode is included by Element and CharacterData.
  distributePrototypeMembers(Node.prototype, [Element.prototype, CharacterData.prototype], [
    "nextElementSibling", "previousElementSibling",
  ]);

  // ── Minimal CSSOM ─────────────────────────────────────────────────────────
  // CSS syntax is accepted/rejected by the native engine parser. This scanner
  // only preserves each accepted top-level rule's source text for CSSOM
  // serialization; it understands strings, comments and nested blocks.
  function splitCssRules(source) {
    const css = String(source || "");
    let expected;
    try {
      expected = __omoikane_css_rule_count(css);
    } catch (error) {
      throw new DOMException(error.message || "Invalid CSS rule.", "SyntaxError");
    }
    const rules = [];
    let start = 0, depth = 0, quote = "", comment = false;
    for (let i = 0; i < css.length; i++) {
      const ch = css[i], next = css[i + 1];
      if (comment) {
        if (ch === "*" && next === "/") { comment = false; i++; }
        continue;
      }
      if (quote) {
        if (ch === "\\") i++;
        else if (ch === quote) quote = "";
        continue;
      }
      if (ch === "/" && next === "*") { comment = true; i++; continue; }
      if (ch === "'" || ch === '"') { quote = ch; continue; }
      if (ch === "{") depth++;
      else if (ch === "}") {
        depth--;
        if (depth === 0) {
          const text = css.slice(start, i + 1).trim();
          if (text) rules.push(text);
          start = i + 1;
        }
      } else if (ch === ";" && depth === 0) {
        const text = css.slice(start, i + 1).trim();
        if (text) rules.push(text);
        start = i + 1;
      }
    }
    const tail = css.slice(start).trim();
    if (tail) rules.push(tail);
    if (rules.length !== expected) {
      throw new DOMException("Unable to enumerate stylesheet rules.", "SyntaxError");
    }
    return rules;
  }

  function declarationView(block) {
    const declarations = [];
    for (const part of block.split(";")) {
      const colon = part.indexOf(":");
      if (colon < 0) continue;
      const name = part.slice(0, colon).trim().toLowerCase();
      const value = part.slice(colon + 1).trim();
      if (name && value) declarations.push({ name, value });
    }
    const target = {
      getPropertyValue(name) {
        const key = String(name).toLowerCase();
        const found = declarations.filter(d => d.name === key);
        return found.length ? found[found.length - 1].value : "";
      },
      item(index) { return declarations[Number(index) | 0]?.name || ""; },
      get length() { return declarations.length; },
      get cssText() { return declarations.map(d => d.name + ": " + d.value + ";").join(" "); },
    };
    return new Proxy(target, {
      get(object, prop) {
        if (typeof prop === "symbol" || prop in object) return object[prop];
        const name = String(prop).replace(/[A-Z]/g, m => "-" + m.toLowerCase());
        return object.getPropertyValue(name);
      },
    });
  }

  class CSSStyleRule {
    constructor(text) {
      this.__text = text;
      const open = this.__text.indexOf("{");
      const close = this.__text.lastIndexOf("}");
      this.__hasBlock = open >= 0 && close > open;
      this.__selectorText = this.__hasBlock ? this.__text.slice(0, open).trim() : "";
      this.__style = declarationView(this.__hasBlock ? this.__text.slice(open + 1, close) : "");
    }
    get selectorText() { return this.__selectorText; }
    get cssText() {
      return this.__hasBlock
        ? this.selectorText + " { " + this.style.cssText + " }"
        : this.__text.trim();
    }
    get style() { return this.__style; }
  }

  class CSSRuleList {
    constructor(sheet) { this.__sheet = sheet; }
    __rules() { return this.__sheet.__ruleTexts().map(text => new CSSStyleRule(text)); }
    item(index) { return this.__rules()[Number(index) | 0] || null; }
    get length() { return this.__rules().length; }
  }

  function ruleListProxy(sheet) {
    const list = new CSSRuleList(sheet);
    return new Proxy(list, {
      get(target, prop) {
        if (typeof prop === "string" && /^(?:0|[1-9]\d*)$/.test(prop)) return target.item(Number(prop));
        if (prop === Symbol.iterator) return target.__rules()[Symbol.iterator].bind(target.__rules());
        return target[prop];
      },
    });
  }

  class CSSStyleSheet {
    constructor(ownerNode) {
      this.ownerNode = ownerNode;
      this.href = null;
      this.__cssRules = ruleListProxy(this);
    }
    __ruleTexts() { return splitCssRules(this.ownerNode.textContent); }
    get cssRules() { return this.__cssRules; }
    insertRule(rule, index) {
      const text = String(rule);
      let count;
      try { count = __omoikane_css_rule_count(text); }
      catch (error) { throw new DOMException(error.message || "Invalid CSS rule.", "SyntaxError"); }
      if (count !== 1) throw new DOMException("Exactly one rule is required.", "SyntaxError");
      const rules = this.__ruleTexts();
      const position = index === undefined ? 0 : Number(index);
      if (!Number.isInteger(position) || position < 0 || position > rules.length)
        throw new DOMException("The index is out of range.", "IndexSizeError");
      rules.splice(position, 0, text.trim());
      this.ownerNode.textContent = rules.join("\n");
      return position;
    }
    deleteRule(index) {
      const rules = this.__ruleTexts();
      const position = Number(index);
      if (!Number.isInteger(position) || position < 0 || position >= rules.length)
        throw new DOMException("The index is out of range.", "IndexSizeError");
      rules.splice(position, 1);
      this.ownerNode.textContent = rules.join("\n");
    }
  }

  const styleSheetCache = new WeakMap();
  function sheetFor(style) {
    if (!styleSheetCache.has(style)) styleSheetCache.set(style, new CSSStyleSheet(style));
    return styleSheetCache.get(style);
  }
  function makeStyleSheetList(doc) {
    const collect = () => Array.from(doc.querySelectorAll("style"), sheetFor);
    return new Proxy([], {
      get(_target, prop) {
        const list = collect();
        if (prop === "length") return list.length;
        if (prop === "item") return index => list[Number(index) | 0] || null;
        const value = list[prop];
        return typeof value === "function" ? value.bind(list) : value;
      },
    });
  }

  // An <iframe> owns a nested browsing context whose document is reachable via
  // contentDocument (and, as a facade, contentWindow.document). The document is
  // created lazily by the host on first access: an empty/absent src yields an
  // about:blank skeleton, while a src is fetched and parsed (only HTML content
  // types become a real DOM tree). Reading contentDocument again after changing
  // src reloads it.
  class HTMLIFrameElement extends HTMLElement {
    get contentDocument() {
      return wrapNode(__omoikane_iframe_content_document(this.__id));
    }

    getSVGDocument() {
      const document = this.contentDocument;
      const root = document && document.documentElement;
      return root && root.namespaceURI === SVG_NAMESPACE && root.localName === "svg"
        ? document
        : null;
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
        windowObjects.add(this.__contentWindowFacade);
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

  class HTMLObjectElement extends HTMLElement {
    get contentDocument() {
      return wrapNode(__omoikane_iframe_content_document(this.__id));
    }

    getSVGDocument() {
      const document = this.contentDocument;
      const root = document && document.documentElement;
      return root && root.namespaceURI === SVG_NAMESPACE && root.localName === "svg"
        ? document
        : null;
    }

    get data() {
      // URL-reflecting IDL attribute: resolve the raw `data` value against the
      // document base URL so callers see an absolute URL. An absent attribute
      // reflects as the empty string.
      const raw = __omoikane_get_attribute(this.__id, "data");
      return raw === null ? "" : __omoikane_resolve_url(raw);
    }

    set data(value) {
      __omoikane_set_attribute(this.__id, "data", String(value));
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

  class HTMLTableElement extends HTMLElement {
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
    // Per HTML spec: tr children of thead elements (in tree order) come first,
    // then those whose parent is the table itself or a tbody (interleaved in
    // tree order), then those in tfoot elements.
    get rows() {
      const heads = [];
      const bodies = [];
      const feet = [];
      for (const child of this.childNodes) {
        if (child.nodeType !== 1) continue;
        const tag = child.tagName;
        if (tag === "THEAD") heads.push(...childElementsByTag(child, "TR"));
        else if (tag === "TFOOT") feet.push(...childElementsByTag(child, "TR"));
        else if (tag === "TBODY") bodies.push(...childElementsByTag(child, "TR"));
        else if (tag === "TR") bodies.push(child);
      }
      return heads.concat(bodies, feet);
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
    // HTMLTableElement.insertRow(index): creates a tr and places it per the HTML
    // spec's insertion rules — auto-creating a tbody for an empty table, else
    // appending to the last tbody, else positioning relative to the rows
    // collection. index defaults to -1 (append).
    insertRow(index = -1) {
      const rows = this.rows;
      if (index < -1 || index > rows.length) {
        throw new DOMException("The index is out of range.", "IndexSizeError");
      }
      const tr = document.createElement("tr");
      const tbodies = childElementsByTag(this, "TBODY");
      if (rows.length === 0 && tbodies.length === 0) {
        const tbody = document.createElement("tbody");
        tbody.appendChild(tr);
        this.appendChild(tbody);
      } else if (rows.length === 0) {
        tbodies[tbodies.length - 1].appendChild(tr);
      } else if (index === -1 || index === rows.length) {
        rows[rows.length - 1].parentNode.appendChild(tr);
      } else {
        const ref = rows[index];
        ref.parentNode.insertBefore(tr, ref);
      }
      return tr;
    }
    deleteRow(index) {
      const rows = this.rows;
      if (index < -1 || index >= rows.length) {
        throw new DOMException("The index is out of range.", "IndexSizeError");
      }
      if (index === -1) {
        if (rows.length === 0) return;
        index = rows.length - 1;
      }
      const row = rows[index];
      row.parentNode.removeChild(row);
    }
  }

  // thead / tbody / tfoot share the HTMLTableSectionElement interface.
  class HTMLTableSectionElement extends HTMLElement {
    get rows() {
      return childElementsByTag(this, "TR");
    }
    insertRow(index = -1) {
      const rows = this.rows;
      if (index < -1 || index > rows.length) {
        throw new DOMException("The index is out of range.", "IndexSizeError");
      }
      const tr = document.createElement("tr");
      if (index === -1 || index === rows.length) {
        this.appendChild(tr);
      } else {
        this.insertBefore(tr, rows[index]);
      }
      return tr;
    }
    deleteRow(index) {
      const rows = this.rows;
      if (index < -1 || index >= rows.length) {
        throw new DOMException("The index is out of range.", "IndexSizeError");
      }
      if (index === -1) {
        if (rows.length === 0) return;
        index = rows.length - 1;
      }
      this.removeChild(rows[index]);
    }
  }

  class HTMLTableRowElement extends HTMLElement {
    // td and th children, in tree order.
    get cells() {
      return this.childNodes.filter(
        c => c.nodeType === 1 && (c.tagName === "TD" || c.tagName === "TH")
      );
    }
    // Index of this row in its owning table's rows collection (thead, then
    // body/tbody, then tfoot ordering), or -1 if not in a table.
    get rowIndex() {
      const parent = this.parentNode;
      if (!parent) return -1;
      let table = null;
      const ptag = parent.tagName;
      if (ptag === "TABLE") {
        table = parent;
      } else if (
        (ptag === "THEAD" || ptag === "TBODY" || ptag === "TFOOT") &&
        parent.parentNode &&
        parent.parentNode.tagName === "TABLE"
      ) {
        table = parent.parentNode;
      }
      if (!table) return -1;
      return table.rows.findIndex(r => r.__id === this.__id);
    }
    // Index of this row among its parent section's rows, or -1 when the row is
    // not in a table section. Per the HTML spec this is defined only within a
    // thead/tbody/tfoot; a row that is a direct child of the table (with no
    // intervening section) returns -1.
    get sectionRowIndex() {
      const parent = this.parentNode;
      if (!parent) return -1;
      const ptag = parent.tagName;
      if (ptag !== "THEAD" && ptag !== "TBODY" && ptag !== "TFOOT") {
        return -1;
      }
      return childElementsByTag(parent, "TR").findIndex(r => r.__id === this.__id);
    }
    insertCell(index = -1) {
      const cells = this.cells;
      if (index < -1 || index > cells.length) {
        throw new DOMException("The index is out of range.", "IndexSizeError");
      }
      const td = document.createElement("td");
      if (index === -1 || index === cells.length) {
        this.appendChild(td);
      } else {
        this.insertBefore(td, cells[index]);
      }
      return td;
    }
    deleteCell(index) {
      const cells = this.cells;
      if (index === -1) index = cells.length - 1;
      if (index < 0 || index >= cells.length) {
        throw new DOMException("The index is out of range.", "IndexSizeError");
      }
      this.removeChild(cells[index]);
    }
  }

  const FORM_CONTROL_TAGS = new Set([
    "INPUT", "SELECT", "TEXTAREA", "BUTTON", "FIELDSET", "OBJECT", "OUTPUT", "KEYGEN",
  ]);

  class HTMLFormElement extends HTMLElement {
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
    // Fires a cancelable `submit` event (the form-submission entry point used by
    // a submit button's activation behavior). Actual navigation is out of scope;
    // a handler calling preventDefault simply suppresses the (absent) default.
    __submit(submitter) {
      const event = new Event("submit", { bubbles: true, cancelable: true });
      event.submitter = submitter || null;
      this.dispatchEvent(event);
    }
    __reset() {
      this.dispatchEvent(new Event("reset", { bubbles: true, cancelable: true }));
    }
  }

  class HTMLInputElement extends HTMLElement {
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

  class HTMLButtonElement extends HTMLElement {
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

  class HTMLLabelElement extends HTMLElement {
    get htmlFor() {
      return this.getAttribute("for") || "";
    }
    set htmlFor(v) {
      this.setAttribute("for", String(v));
    }
  }

  class HTMLMetaElement extends HTMLElement {
    get httpEquiv() {
      return this.getAttribute("http-equiv") || "";
    }
    set httpEquiv(v) {
      this.setAttribute("http-equiv", String(v));
    }
  }

  class HTMLSelectElement extends HTMLElement {
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

  class HTMLOptionElement extends HTMLElement {
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

  class HTMLScriptElement extends HTMLElement {
    get src() {
      return this.getAttribute("src") || "";
    }
    set src(value) {
      this.setAttribute("src", String(value));
    }
    get async() {
      return this.hasAttribute("async");
    }
    set async(value) {
      if (value) this.setAttribute("async", "");
      else this.removeAttribute("async");
    }
    get defer() {
      return this.hasAttribute("defer");
    }
    set defer(value) {
      if (value) this.setAttribute("defer", "");
      else this.removeAttribute("defer");
    }
  }

  class HTMLImageElement extends HTMLElement {
    get height() {
      const attr = this.getAttribute("height");
      if (attr !== null && attr !== "") return Math.max(0, Number.parseInt(attr, 10) || 0);
      const value = globalThis.getComputedStyle(this).height;
      return Math.max(0, Number.parseFloat(value) || 0);
    }
    set height(value) { this.setAttribute("height", String(Math.max(0, Number(value) || 0))); }
    get width() {
      const attr = this.getAttribute("width");
      if (attr !== null && attr !== "") return Math.max(0, Number.parseInt(attr, 10) || 0);
      const value = globalThis.getComputedStyle(this).width;
      return Math.max(0, Number.parseFloat(value) || 0);
    }
    set width(value) { this.setAttribute("width", String(Math.max(0, Number(value) || 0))); }
  }

  class HTMLLinkElement extends HTMLElement {
    get rel() {
      return this.getAttribute("rel") || "";
    }
    set rel(value) {
      this.setAttribute("rel", String(value));
    }
    get relList() {
      const link = this;
      const tokens = () => link.rel.split(/\s+/).filter(Boolean);
      return {
        contains(token) {
          return tokens().includes(String(token));
        },
        supports(token) {
          const value = String(token).toLowerCase();
          return ["dns-prefetch", "modulepreload", "preconnect", "preload", "stylesheet"]
            .includes(value);
        },
        get length() {
          return tokens().length;
        },
        item(index) {
          return tokens()[Number(index)] ?? null;
        },
      };
    }
  }

  // Minimal SVG DOM layer. Rendering remains owned by src/svg; these wrappers
  // only provide the interfaces exercised by script and Acid3.
  class SVGElement extends Element {}
  class SVGSVGElement extends SVGElement {}
  class SVGRectElement extends SVGElement {
    get width() {
      if (!this.__width) this.__width = {};
      return this.__width;
    }
  }
  class SVGTextContentElement extends SVGElement {
    getNumberOfChars() {
      return String(this.textContent || "").length;
    }
  }
  class SVGTextElement extends SVGTextContentElement {}

  // HTMLOrSVGElement and ElementCSSInlineStyle are shared by HTML and SVG
  // elements, but not by arbitrary Node objects.
  distributePrototypeMembers(Node.prototype, [HTMLElement.prototype, SVGElement.prototype], [
    "dataset", "style",
  ]);

  // Form-control state belongs to the corresponding HTML interfaces. Keep the
  // existing implementation descriptors while removing them from Node.
  distributePrototypeMembers(Node.prototype, [HTMLInputElement.prototype], [
    "checked", "defaultChecked",
  ]);
  distributePrototypeMembers(Node.prototype, [
    HTMLInputElement.prototype, HTMLButtonElement.prototype,
    HTMLSelectElement.prototype, HTMLOptionElement.prototype,
  ], ["disabled"]);
  distributePrototypeMembers(Node.prototype, [HTMLSelectElement.prototype, HTMLButtonElement.prototype], ["value"]);
  // DocumentType.name is the declared doctype name, unrelated to form-control
  // name reflection. Copy it before the form interfaces consume and remove the
  // shared implementation descriptor from Node.prototype.
  Object.defineProperty(
    DocumentType.prototype,
    "name",
    Object.getOwnPropertyDescriptor(Node.prototype, "name"),
  );
  distributePrototypeMembers(Node.prototype, [
    HTMLFormElement.prototype, HTMLInputElement.prototype, HTMLButtonElement.prototype,
    HTMLSelectElement.prototype, HTMLIFrameElement.prototype,
    HTMLObjectElement.prototype, HTMLImageElement.prototype,
  ], ["name"]);
  // Input and button provide their own type behavior; the generic fallback must
  // not remain visible on every Node or HTMLElement.
  distributePrototypeMembers(Node.prototype, [], ["type"]);

  const SVG_ELEMENT_CTORS = {
    svg: SVGSVGElement,
    rect: SVGRectElement,
    text: SVGTextElement,
  };

  // Tag-name → constructor table consulted by wrapNode() for element nodes.
  const ELEMENT_CTORS = {
    html: HTMLHtmlElement,
    head: HTMLHeadElement,
    body: HTMLBodyElement,
    div: HTMLDivElement,
    span: HTMLSpanElement,
    p: HTMLParagraphElement,
    a: HTMLAnchorElement,
    table: HTMLTableElement,
    thead: HTMLTableSectionElement,
    tbody: HTMLTableSectionElement,
    tfoot: HTMLTableSectionElement,
    tr: HTMLTableRowElement,
    form: HTMLFormElement,
    input: HTMLInputElement,
    button: HTMLButtonElement,
    label: HTMLLabelElement,
    meta: HTMLMetaElement,
    select: HTMLSelectElement,
    option: HTMLOptionElement,
    iframe: HTMLIFrameElement,
    object: HTMLObjectElement,
    img: HTMLImageElement,
    link: HTMLLinkElement,
    script: HTMLScriptElement,
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

  // Event handler IDL attributes (onclick, onsubmit, ...). Assigning a function
  // registers a single event listener for the matching type and replaces any
  // previously assigned handler, so `form.onsubmit = fn` behaves like
  // `addEventListener("submit", fn)`. `onload` is defined directly on Node
  // (with Window reflection for <body>) and is intentionally excluded here.
  const EVENT_HANDLER_TYPES = [
    "click", "dblclick", "mousedown", "mouseup", "mouseover", "mousemove",
    "mouseout", "mouseenter", "mouseleave", "submit", "reset", "change",
    "input", "focus", "blur", "keydown", "keyup", "keypress", "select",
    "contextmenu", "wheel", "error", "abort",
  ];
  for (const type of EVENT_HANDLER_TYPES) {
    const key = "__on_" + type;
    Object.defineProperty(Node.prototype, "on" + type, {
      configurable: true,
      enumerable: false,
      get() {
        return this[key] || null;
      },
      set(handler) {
        if (this[key]) this.removeEventListener(type, this[key]);
        this[key] = typeof handler === "function" ? handler : null;
        if (this[key]) this.addEventListener(type, this[key]);
      },
    });
  }

  globalThis.Node = Node;
  globalThis.NodeList = NodeList;
  globalThis.Window = Window;
  globalThis.Element = Element;
  globalThis.HTMLElement = HTMLElement;
  globalThis.HTMLHtmlElement = HTMLHtmlElement;
  globalThis.HTMLHeadElement = HTMLHeadElement;
  globalThis.HTMLBodyElement = HTMLBodyElement;
  globalThis.HTMLDivElement = HTMLDivElement;
  globalThis.HTMLSpanElement = HTMLSpanElement;
  globalThis.HTMLParagraphElement = HTMLParagraphElement;
  globalThis.HTMLAnchorElement = HTMLAnchorElement;
  globalThis.CharacterData = CharacterData;
  globalThis.Text = Text;
  globalThis.CDATASection = CDATASection;
  globalThis.Comment = Comment;
  globalThis.ProcessingInstruction = ProcessingInstruction;
  globalThis.Document = Document;
  globalThis.DocumentFragment = DocumentFragment;
  globalThis.DocumentType = DocumentType;
  globalThis.DOMException = DOMException;
  globalThis.CSSStyleSheet = CSSStyleSheet;
  globalThis.CSSRuleList = CSSRuleList;
  globalThis.CSSStyleRule = CSSStyleRule;
  globalThis.NodeFilter = NodeFilter;
  globalThis.NodeIterator = NodeIterator;
  globalThis.TreeWalker = TreeWalker;
  globalThis.Range = Range;
  globalThis.HTMLTableElement = HTMLTableElement;
  globalThis.HTMLTableSectionElement = HTMLTableSectionElement;
  globalThis.HTMLTableRowElement = HTMLTableRowElement;
  globalThis.HTMLFormElement = HTMLFormElement;
  globalThis.HTMLInputElement = HTMLInputElement;
  globalThis.HTMLButtonElement = HTMLButtonElement;
  globalThis.HTMLLabelElement = HTMLLabelElement;
  globalThis.HTMLMetaElement = HTMLMetaElement;
  globalThis.HTMLSelectElement = HTMLSelectElement;
  globalThis.HTMLOptionElement = HTMLOptionElement;
  globalThis.HTMLImageElement = HTMLImageElement;
  globalThis.HTMLLinkElement = HTMLLinkElement;
  globalThis.HTMLScriptElement = HTMLScriptElement;
  globalThis.HTMLIFrameElement = HTMLIFrameElement;
  globalThis.HTMLObjectElement = HTMLObjectElement;
  globalThis.SVGElement = SVGElement;
  globalThis.SVGSVGElement = SVGSVGElement;
  globalThis.SVGRectElement = SVGRectElement;
  globalThis.SVGTextContentElement = SVGTextContentElement;
  globalThis.SVGTextElement = SVGTextElement;
  globalThis.Event = Event;
  globalThis.CustomEvent = CustomEvent;
  globalThis.MessageEvent = MessageEvent;
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
  globalThis.__omoikane_set_current_script = function(id) {
    globalThis.document.__currentScript =
      id === null || id === undefined ? null : wrapNode(id);
  };
  if (globalThis.window === undefined) {
    globalThis.window = globalThis;
  }
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
  // Returns the event type reflected by an `on*` content attribute name (e.g.
  // "onload" -> "load"), or `null` when `name` is not an event-handler
  // attribute. Names shorter than three characters (e.g. "on") reflect nothing.
  function inlineHandlerEventType(name) {
    const lower = String(name).toLowerCase();
    if (lower.length <= 2 || lower.slice(0, 2) !== "on") return null;
    return lower.slice(2);
  }
  // (Re)wires a single `on*` content attribute on `node` to a real event
  // listener, keeping the current attribute value authoritative. This is the
  // per-attribute step shared by the initial full-tree pass and the dynamic
  // `setAttribute`/`removeAttribute` paths, so a handler attached after the
  // page loads (Acid3 test 48's `iframe.setAttribute("onload", ...)`) behaves
  // like one present at parse time.
  //
  // At most one listener per (node, event type) is retained through this path:
  // the previously installed handler is removed before the new value is
  // compiled and attached, so re-setting the attribute — or the initial pass
  // followed by a later `setAttribute` — never leaves two listeners registered.
  // Removing the attribute (value becomes `null`) just detaches the handler.
  function applyInlineHandlerAttribute(node, name) {
    if (!node || node.nodeType !== 1) return;
    const type = inlineHandlerEventType(name);
    if (!type) return;
    const tag = (__omoikane_node_name(node.__id) || "").toLowerCase();
    const reflectToWindow =
      (tag === "body" || tag === "frameset") && WINDOW_REFLECTED_HANDLERS.has(type);
    // Null-prototype dictionary: the key is the event type derived from the
    // attribute name (e.g. `setAttribute('on__proto__', ...)` -> `"__proto__"`),
    // which is attacker-influenced, so a plain `{}` would let such a name write
    // through to `Object.prototype` (prototype pollution) or resolve inherited
    // members (`constructor`, `toString`) as bogus "previous" handlers.
    const store =
      node.__contentAttrHandlers ||
      (node.__contentAttrHandlers = Object.create(null));
    const previous = store[type];
    if (previous) {
      previous.target.removeEventListener(type, previous.handler);
      delete store[type];
    }
    const source = __omoikane_get_attribute(node.__id, name);
    if (source == null) return;
    const handler = compileInlineHandler(source);
    if (!handler) return;
    const target = reflectToWindow ? globalThis : node;
    target.addEventListener(type, handler);
    store[type] = { handler, target };
  }
  function wireInlineHandlers(node) {
    if (node && node.nodeType === 1) {
      const names = __omoikane_attribute_names(node.__id) || [];
      for (const name of names) {
        applyInlineHandlerAttribute(node, name);
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
  globalThis.__omoikane_dispatch_resource_load = function(id) {
    const element = wrapNode(id);
    if (element) element.dispatchEvent(new Event("load", { bubbles: false }));
  };
  const __documentCookies = new Map();
  Object.defineProperty(Document.prototype, "cookie", {
    configurable: true,
    enumerable: true,
    get() {
      return Array.from(__documentCookies.entries())
        .map(entry => entry[0] + "=" + entry[1])
        .join("; ");
    },
    set(serialized) {
      const parts = String(serialized).split(";");
      const pair = parts.shift() || "";
      const separator = pair.indexOf("=");
      if (separator <= 0) return;
      const name = pair.slice(0, separator).trim();
      const value = pair.slice(separator + 1).trim();
      const expired = parts.some(part => /^\s*max-age\s*=\s*0\s*$/i.test(part));
      if (expired) __documentCookies.delete(name);
      else __documentCookies.set(name, value);
    },
  });

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
  function __applyHistoryUrl(url) {
    if (url == null || String(url) === "") return;
    const raw = String(url);
    let href = raw;
    if (!/^[A-Za-z][A-Za-z0-9+.-]*:\/\//.test(raw)) {
      href = raw.startsWith("/") ? __loc.origin + raw :
        __loc.origin + (__loc.pathname.replace(/[^/]*$/, "")) + raw;
    }
    const match = href.match(/^(.*?):\/\/([^/?#]+)([^?#]*)(\?[^#]*)?(#.*)?$/);
    if (!match || (match[1] + "://" + match[2]) !== __loc.origin) {
      throw new DOMException("History state URL must be same-origin", "SecurityError");
    }
    __loc.href = href;
    __loc.protocol = match[1] + ":";
    __loc.host = match[2];
    __loc.hostname = match[2].replace(/:\d+$/, "");
    __loc.pathname = match[3] || "/";
    __loc.search = match[4] || "";
    __loc.hash = match[5] || "";
  }
  __loc.assign = function(url) {
    __applyHistoryUrl(url);
  };
  __loc.replace = function(url) {
    __applyHistoryUrl(url);
  };
  __loc.reload = function() {
    // A navigation-capable embedder can replace this with a full document
    // reload. The synchronous Location API itself returns undefined.
  };
  const __historyEntries = [{ state: null, href: __loc.href }];
  let __historyIndex = 0;
  globalThis.history = {
    scrollRestoration: "auto",
    get length() { return __historyEntries.length; },
    get state() { return __historyEntries[__historyIndex].state; },
    pushState(state, unused, url) {
      void unused;
      __applyHistoryUrl(url);
      __historyEntries.splice(__historyIndex + 1);
      __historyEntries.push({ state, href: __loc.href });
      __historyIndex = __historyEntries.length - 1;
    },
    replaceState(state, unused, url) {
      void unused;
      __applyHistoryUrl(url);
      __historyEntries[__historyIndex] = { state, href: __loc.href };
    },
    go(delta = 0) {
      const target = __historyIndex + Number(delta || 0);
      if (target < 0 || target >= __historyEntries.length || target === __historyIndex) return;
      __historyIndex = target;
      __applyHistoryUrl(__historyEntries[target].href);
      globalThis.dispatchEvent(new Event("popstate"));
    },
    back() { this.go(-1); },
    forward() { this.go(1); },
  };
  // Maps a JS-style property name to its CSS (kebab-case) form. `cssFloat` /
  // `styleFloat` alias the `float` property, matching the CSSOM.
  function __styleNameToCss(prop) {
    if (prop === "cssFloat" || prop === "styleFloat") return "float";
    return String(prop).replace(/[A-Z]/g, m => "-" + m.toLowerCase());
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
    if (element && element.__id != null) {
      try {
        return __makeComputedStyle(JSON.parse(__omoikane_computed_style(element.__id)));
      } catch (e) {
        return __makeComputedStyle({});
      }
    }
    return __makeComputedStyle({});
  };
  globalThis.navigator = { userAgent: __omoikane_navigator_user_agent, language: "en", languages: ["en"], platform: "", cookieEnabled: true, onLine: true };
  if (globalThis.Intl === undefined) {
    class IntlFormatter {
      constructor(locales, options) {
        this.locales = locales;
        this.options = options || {};
      }
      resolvedOptions() {
        return { locale: "en-US", ...this.options };
      }
      static supportedLocalesOf(locales) {
        if (locales === undefined) return [];
        return Array.isArray(locales) ? locales.map(String) : [String(locales)];
      }
    }
    class NumberFormat extends IntlFormatter {
      format(value) { return String(Number(value)); }
      formatToParts(value) { return [{ type: "integer", value: this.format(value) }]; }
      formatRange(start, end) { return this.format(start) + "–" + this.format(end); }
      formatRangeToParts(start, end) {
        return [{ type: "integer", value: this.formatRange(start, end), source: "shared" }];
      }
    }
    class DateTimeFormat extends IntlFormatter {
      format(value) {
        const date = value === undefined ? new Date() : new Date(value);
        return Number.isNaN(date.getTime()) ? "Invalid Date" : date.toISOString();
      }
      formatToParts(value) { return [{ type: "literal", value: this.format(value) }]; }
      formatRange(start, end) { return this.format(start) + " – " + this.format(end); }
      formatRangeToParts(start, end) {
        return [{ type: "literal", value: this.formatRange(start, end), source: "shared" }];
      }
    }
    class PluralRules extends IntlFormatter {
      select(value) { return Number(value) === 1 ? "one" : "other"; }
      selectRange() { return "other"; }
    }
    class RelativeTimeFormat extends IntlFormatter {
      format(value, unit) { return String(value) + " " + String(unit); }
      formatToParts(value, unit) {
        return [{ type: "integer", value: String(value), unit: String(unit) }];
      }
    }
    class ListFormat extends IntlFormatter {
      format(values) { return Array.from(values, String).join(", "); }
      formatToParts(values) {
        return [{ type: "element", value: this.format(values) }];
      }
    }
    class Collator extends IntlFormatter {
      compare(left, right) {
        const a = String(left), b = String(right);
        return a < b ? -1 : a > b ? 1 : 0;
      }
    }
    class DisplayNames extends IntlFormatter {
      of(code) { return String(code); }
    }
    class Locale {
      constructor(tag) { this.baseName = String(tag); }
      toString() { return this.baseName; }
      maximize() { return this; }
      minimize() { return this; }
    }
    const callableFormatter = Constructor => {
      function Formatter(...args) { return new Constructor(...args); }
      Formatter.prototype = Constructor.prototype;
      Formatter.supportedLocalesOf = IntlFormatter.supportedLocalesOf;
      return Formatter;
    };
    globalThis.Intl = {
      NumberFormat: callableFormatter(NumberFormat),
      DateTimeFormat: callableFormatter(DateTimeFormat),
      PluralRules: callableFormatter(PluralRules),
      RelativeTimeFormat: callableFormatter(RelativeTimeFormat),
      ListFormat: callableFormatter(ListFormat),
      Collator: callableFormatter(Collator),
      DisplayNames: callableFormatter(DisplayNames),
      Locale,
      getCanonicalLocales(locales) {
        if (locales === undefined) return [];
        return Array.isArray(locales) ? locales.map(String) : [String(locales)];
      },
    };
  }
  if (globalThis.TextEncoder === undefined) {
    globalThis.TextEncoder = class TextEncoder {
      get encoding() { return "utf-8"; }
      encode(input = "") {
        const bytes = [];
        for (const char of String(input)) {
          const code = char.codePointAt(0);
          if (code <= 0x7f) bytes.push(code);
          else if (code <= 0x7ff) {
            bytes.push(0xc0 | (code >> 6), 0x80 | (code & 0x3f));
          } else if (code <= 0xffff) {
            bytes.push(0xe0 | (code >> 12), 0x80 | ((code >> 6) & 0x3f), 0x80 | (code & 0x3f));
          } else {
            bytes.push(0xf0 | (code >> 18), 0x80 | ((code >> 12) & 0x3f),
              0x80 | ((code >> 6) & 0x3f), 0x80 | (code & 0x3f));
          }
        }
        return new Uint8Array(bytes);
      }
      encodeInto(source, destination) {
        const bytes = this.encode(source);
        const written = Math.min(bytes.length, destination.length);
        for (let index = 0; index < written; index++) destination[index] = bytes[index];
        return { read: String(source).length, written };
      }
    };
  }
  if (globalThis.TextDecoder === undefined) {
    globalThis.TextDecoder = class TextDecoder {
      constructor(label = "utf-8", options = {}) {
        const normalized = String(label).toLowerCase().replace(/[_\s]/g, "-");
        if (normalized !== "utf-8" && normalized !== "utf8") throw new RangeError("unsupported encoding");
        this.encoding = "utf-8";
        this.fatal = Boolean(options.fatal);
        this.ignoreBOM = Boolean(options.ignoreBOM);
      }
      decode(input = new Uint8Array()) {
        const bytes = input instanceof ArrayBuffer ? new Uint8Array(input) : new Uint8Array(input.buffer || input, input.byteOffset || 0, input.byteLength === undefined ? input.length : input.byteLength);
        let result = "";
        for (let index = 0; index < bytes.length;) {
          const first = bytes[index++];
          if (first <= 0x7f) { result += String.fromCodePoint(first); continue; }
          let code, needed;
          if ((first & 0xe0) === 0xc0) { code = first & 0x1f; needed = 1; }
          else if ((first & 0xf0) === 0xe0) { code = first & 0x0f; needed = 2; }
          else if ((first & 0xf8) === 0xf0) { code = first & 0x07; needed = 3; }
          else { if (this.fatal) throw new TypeError("invalid UTF-8"); result += "\ufffd"; continue; }
          if (index + needed > bytes.length) { if (this.fatal) throw new TypeError("invalid UTF-8"); result += "\ufffd"; break; }
          let valid = true;
          for (let offset = 0; offset < needed; offset++) {
            const next = bytes[index++];
            if ((next & 0xc0) !== 0x80) { valid = false; break; }
            code = (code << 6) | (next & 0x3f);
          }
          if (!valid) { if (this.fatal) throw new TypeError("invalid UTF-8"); result += "\ufffd"; }
          else result += String.fromCodePoint(code);
        }
        return result;
      }
    };
  }
  if (globalThis.btoa === undefined) {
    const base64Alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    globalThis.btoa = function(input) {
      const value = String(input);
      let output = "";
      for (let index = 0; index < value.length; index += 3) {
        const a = value.charCodeAt(index);
        const b = index + 1 < value.length ? value.charCodeAt(index + 1) : 0;
        const c = index + 2 < value.length ? value.charCodeAt(index + 2) : 0;
        if (a > 255 || b > 255 || c > 255) throw new DOMException("invalid character", "InvalidCharacterError");
        const bits = (a << 16) | (b << 8) | c;
        output += base64Alphabet[(bits >> 18) & 63];
        output += base64Alphabet[(bits >> 12) & 63];
        output += index + 1 < value.length ? base64Alphabet[(bits >> 6) & 63] : "=";
        output += index + 2 < value.length ? base64Alphabet[bits & 63] : "=";
      }
      return output;
    };
    globalThis.atob = function(input) {
      const value = String(input).replace(/[\t\n\f\r ]/g, "").replace(/=+$/, "");
      if (value.length % 4 === 1 || /[^A-Za-z0-9+/]/.test(value)) {
        throw new DOMException("invalid character", "InvalidCharacterError");
      }
      let output = "", buffer = 0, bits = 0;
      for (const char of value) {
        buffer = (buffer << 6) | base64Alphabet.indexOf(char);
        bits += 6;
        if (bits >= 8) {
          bits -= 8;
          output += String.fromCharCode((buffer >> bits) & 255);
        }
      }
      return output;
    };
  }
  if (globalThis.URL === undefined) {
    class URLSearchParams {
      constructor(init = "") {
        this._entries = [];
        const source = String(init).replace(/^\?/, "");
        if (source) for (const pair of source.split("&")) {
          const separator = pair.indexOf("=");
          const key = separator < 0 ? pair : pair.slice(0, separator);
          const value = separator < 0 ? "" : pair.slice(separator + 1);
          this.append(decodeURIComponent(key.replace(/\+/g, " ")), decodeURIComponent(value.replace(/\+/g, " ")));
        }
      }
      append(name, value) { this._entries.push([String(name), String(value)]); }
      set(name, value) {
        this.delete(name);
        this.append(name, value);
      }
      get(name) {
        const found = this._entries.find(entry => entry[0] === String(name));
        return found ? found[1] : null;
      }
      getAll(name) { return this._entries.filter(entry => entry[0] === String(name)).map(entry => entry[1]); }
      has(name) { return this._entries.some(entry => entry[0] === String(name)); }
      delete(name) { this._entries = this._entries.filter(entry => entry[0] !== String(name)); }
      *entries() { yield* this._entries; }
      *keys() { for (const entry of this._entries) yield entry[0]; }
      *values() { for (const entry of this._entries) yield entry[1]; }
      [Symbol.iterator]() { return this.entries(); }
      toString() {
        return this._entries.map(entry => encodeURIComponent(entry[0]).replace(/%20/g, "+") + "=" + encodeURIComponent(entry[1]).replace(/%20/g, "+")).join("&");
      }
    }
    class URL {
      constructor(input, base) {
        let value = String(input);
        if (!/^[A-Za-z][A-Za-z0-9+.-]*:/.test(value)) {
          const baseValue = base === undefined ? globalThis.location.href : String(base);
          const match = baseValue.match(/^([A-Za-z][A-Za-z0-9+.-]*:)(?:\/\/([^/?#]*))?([^?#]*)/);
          if (!match) throw new TypeError("invalid base URL");
          if (value.startsWith("//")) value = match[1] + value;
          else if (value.startsWith("/")) value = match[1] + "//" + (match[2] || "") + value;
          else {
            const directory = (match[3] || "/").replace(/[^/]*$/, "");
            value = match[1] + "//" + (match[2] || "") + directory + value;
          }
        }
        const parsed = value.match(/^([A-Za-z][A-Za-z0-9+.-]*:)(?:\/\/([^/?#]*))?([^?#]*)(\?[^#]*)?(#.*)?$/);
        if (!parsed) throw new TypeError("invalid URL");
        this.protocol = parsed[1];
        this.host = parsed[2] || "";
        this.hostname = this.host.replace(/:\d+$/, "");
        this.port = this.host.slice(this.hostname.length).replace(/^:/, "");
        this.pathname = parsed[3] || (this.host ? "/" : "");
        this.search = parsed[4] || "";
        this.hash = parsed[5] || "";
        this.origin = this.host ? this.protocol + "//" + this.host : "null";
        this.searchParams = new URLSearchParams(this.search);
        this.href = this.toString();
      }
      toString() { return this.protocol + (this.host ? "//" + this.host : "") + this.pathname + this.search + this.hash; }
      toJSON() { return this.toString(); }
      static canParse(input, base) { try { new URL(input, base); return true; } catch (_) { return false; } }
    }
    globalThis.URL = URL;
    globalThis.URLSearchParams = URLSearchParams;
  }
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

  const mutationObservers = [];

  class MutationRecord {
    constructor(type, target, init = {}) {
      this.type = type;
      this.target = target;
      this.addedNodes = init.addedNodes || [];
      this.removedNodes = init.removedNodes || [];
      this.previousSibling = init.previousSibling || null;
      this.nextSibling = init.nextSibling || null;
      this.attributeName = init.attributeName || null;
      this.attributeNamespace = init.attributeNamespace || null;
      this.oldValue = init.oldValue === undefined ? null : init.oldValue;
    }
  }

  function queueMutation(target, type, init = {}) {
    for (const observer of mutationObservers) {
      let matched = false;
      let includeOldValue = false;
      for (const registration of observer._registrations) {
        const inScope = registration.target === target ||
          (registration.options.subtree && isInclusiveDescendant(target, registration.target));
        if (!inScope || !registration.options[type]) continue;
        if (type === "attributes" && registration.options.attributeFilter &&
            !registration.options.attributeFilter.includes(init.attributeName)) continue;
        matched = true;
        if (type === "attributes" && registration.options.attributeOldValue) includeOldValue = true;
        if (type === "characterData" && registration.options.characterDataOldValue) includeOldValue = true;
      }
      if (!matched) continue;
      const recordInit = { ...init };
      if ((type === "attributes" || type === "characterData") && !includeOldValue) {
        recordInit.oldValue = null;
      }
      observer._records.push(new MutationRecord(type, target, recordInit));
      observer._schedule();
    }
  }

  globalThis.MutationRecord = MutationRecord;
  globalThis.MutationObserver = class MutationObserver {
    constructor(callback) {
      if (arguments.length < 1 || typeof callback !== "function") throw new TypeError("MutationObserver callback must be callable");
      this._callback = callback;
      this._records = [];
      this._registrations = [];
      this._scheduled = false;
    }
    observe(target, options) {
      if (arguments.length < 2 || !target || options === null || typeof options !== "object") throw new TypeError("observe requires target and options");
      const normalized = {
        childList: !!options.childList, subtree: !!options.subtree,
        attributes: options.attributes === undefined
          ? ("attributeOldValue" in options || "attributeFilter" in options)
          : !!options.attributes,
        characterData: options.characterData === undefined
          ? ("characterDataOldValue" in options)
          : !!options.characterData,
        attributeOldValue: !!options.attributeOldValue,
        characterDataOldValue: !!options.characterDataOldValue,
        attributeFilter: options.attributeFilter === undefined
          ? null : Array.from(options.attributeFilter, String),
      };
      if ((!normalized.attributes && (normalized.attributeOldValue || normalized.attributeFilter)) ||
          (!normalized.characterData && normalized.characterDataOldValue) ||
          (!normalized.childList && !normalized.attributes && !normalized.characterData)) {
        throw new TypeError("At least one mutation type must be observed");
      }
      if (!mutationObservers.includes(this)) mutationObservers.push(this);
      const existing = this._registrations.find(entry => entry.target === target);
      if (existing) existing.options = normalized;
      else this._registrations.push({ target, options: normalized });
    }
    disconnect() {
      this._registrations = [];
      this._records = [];
      const index = mutationObservers.indexOf(this);
      if (index >= 0) mutationObservers.splice(index, 1);
    }
    takeRecords() { const records = this._records; this._records = []; return records; }
    _schedule() {
      if (this._scheduled) return;
      this._scheduled = true;
      Promise.resolve().then(() => {
        this._scheduled = false;
        const records = this.takeRecords();
        if (records.length) this._callback.call(this, records, this);
      });
    }
  };

  // ResizeObserver stub
  globalThis.ResizeObserver = class ResizeObserver {
    constructor(callback) { this._callback = callback; }
    observe() {}
    unobserve() {}
    disconnect() {}
  };

  // Boa does not currently implement locale-aware Date formatting. Pages commonly
  // use this API for diagnostic timestamps, so provide a deterministic fallback.
  Date.prototype.toLocaleTimeString = function() {
    const hours = String(this.getHours()).padStart(2, "0");
    const minutes = String(this.getMinutes()).padStart(2, "0");
    const seconds = String(this.getSeconds()).padStart(2, "0");
    return hours + ":" + minutes + ":" + seconds;
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

  globalThis.DOMParser = class DOMParser {
    parseFromString(source, type) {
      if (arguments.length < 2) throw new TypeError("parseFromString requires 2 arguments");
      const input = String(source);
      const mime = String(type).toLowerCase();
      const supported = ["text/html", "text/xml", "application/xml", "application/xhtml+xml", "image/svg+xml"];
      if (!supported.includes(mime)) throw new TypeError("Unsupported DOMParser MIME type: " + mime);
      if (mime === "text/html") {
        const parsed = document.implementation.createHTMLDocument("");
        parsed.body.innerHTML = input;
        return parsed;
      }
      const match = input.match(/<([A-Za-z_][A-Za-z0-9_.:-]*)(?:\s[^<>]*)?\s*\/?\s*>/);
      if (!match) return document.implementation.createDocument("", "parsererror", null);
      return document.implementation.createDocument("", match[1], null);
    }
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
  globalThis.XMLHttpRequest = class XMLHttpRequest {
    constructor() {
      this._listeners = {};
      this.readyState = 0;
      this.status = 0;
      this.statusText = "";
      this.responseText = "";
      this.response = "";
      this.responseType = "";
      this.timeout = 0;
      this.withCredentials = false;
      this.onreadystatechange = null;
      this.onload = null;
      this.onerror = null;
      this.onloadend = null;
      this._headers = {};
      this._requestId = 0;
    }
    open(method, url, async = true) {
      this._requestId++;
      this.status = 0;
      this.statusText = "";
      this.responseText = "";
      this.response = "";
      this._headers = {};
      this._method = String(method).toUpperCase();
      this._url = String(url);
      this._async = async !== false;
      this.readyState = 1;
      this._notify("readystatechange");
    }
    setRequestHeader(name, value) {
      this._headers[String(name).toLowerCase()] = String(value);
    }
    getAllResponseHeaders() { return ""; }
    getResponseHeader() { return null; }
    addEventListener(type, callback) {
      (this._listeners[type] ||= []).push(callback);
    }
    removeEventListener(type, callback) {
      this._listeners[type] = (this._listeners[type] || []).filter(item => item !== callback);
    }
    abort() {
      this._requestId++;
      this.readyState = 0;
      this._notify("abort");
      this._notify("loadend");
    }
    send() {
      if (this.readyState !== 1) throw new Error("InvalidStateError");
      const requestId = this._requestId;
      if (this._method !== "GET") {
        this.readyState = 4;
        this._notify("readystatechange");
        this._notify("error");
        this._notify("loadend");
        return;
      }
      Promise.resolve(__omoikane_fetch(this._url)).then(raw => {
        if (requestId !== this._requestId) return;
        const data = JSON.parse(String(raw));
        this.status = data.status;
        this.statusText = data.ok ? "OK" : "";
        this.responseText = data.bodyText;
        this.response = this.responseText;
        this.readyState = 4;
        this._notify("readystatechange");
        this._notify("load");
        this._notify("loadend");
      }).catch(() => {
        if (requestId !== this._requestId) return;
        this.readyState = 4;
        this._notify("readystatechange");
        this._notify("error");
        this._notify("loadend");
      });
    }
    _notify(type) {
      const event = new Event(type);
      const handler = this["on" + type];
      if (typeof handler === "function") handler.call(this, event);
      for (const callback of this._listeners[type] || []) callback.call(this, event);
    }
  };
  globalThis.XMLHttpRequest.UNSENT = 0;
  globalThis.XMLHttpRequest.OPENED = 1;
  globalThis.XMLHttpRequest.HEADERS_RECEIVED = 2;
  globalThis.XMLHttpRequest.LOADING = 3;
  globalThis.XMLHttpRequest.DONE = 4;

  class ReadableStreamDefaultController {
    constructor(stream) { this._stream = stream; }
    enqueue(chunk) {
      if (this._stream._closed) throw new TypeError("ReadableStream is closed");
      const waiter = this._stream._waiters.shift();
      if (waiter) waiter.resolve({ value: chunk, done: false });
      else this._stream._queue.push(chunk);
    }
    close() {
      if (this._stream._closed) return;
      this._stream._closed = true;
      for (const waiter of this._stream._waiters.splice(0)) {
        waiter.resolve({ value: undefined, done: true });
      }
    }
    error(reason) {
      this._stream._error = reason;
      this._stream._closed = true;
      for (const waiter of this._stream._waiters.splice(0)) waiter.reject(reason);
    }
    get desiredSize() { return this._stream._closed ? 0 : 1; }
  }
  class ReadableStreamDefaultReader {
    constructor(stream) {
      if (!(stream instanceof ReadableStream) || stream.locked) throw new TypeError("Invalid or locked stream");
      this._stream = stream;
      stream._reader = this;
      this.closed = stream._closed ? Promise.resolve() : new Promise(resolve => { stream._closedResolve = resolve; });
    }
    read() {
      const stream = this._stream;
      if (!stream) return Promise.reject(new TypeError("Reader has no stream"));
      if (stream._queue.length) return Promise.resolve({ value: stream._queue.shift(), done: false });
      if (stream._error !== undefined) return Promise.reject(stream._error);
      if (stream._closed) return Promise.resolve({ value: undefined, done: true });
      return new Promise((resolve, reject) => stream._waiters.push({ resolve, reject }));
    }
    cancel(reason) { return this._stream ? this._stream.cancel(reason) : Promise.reject(new TypeError("Reader has no stream")); }
    releaseLock() { if (this._stream) this._stream._reader = null; this._stream = null; }
  }
  class ReadableStream {
    constructor(underlyingSource = {}) {
      this._queue = []; this._waiters = []; this._reader = null;
      this._closed = false; this._error = undefined; this._source = underlyingSource || {};
      this._controller = new ReadableStreamDefaultController(this);
      if (typeof this._source.start === "function") {
        Promise.resolve(this._source.start(this._controller)).catch(e => this._controller.error(e));
      }
    }
    get locked() { return this._reader !== null; }
    getReader() { return new ReadableStreamDefaultReader(this); }
    cancel(reason) {
      this._queue.length = 0; this._controller.close();
      return Promise.resolve(typeof this._source.cancel === "function" ? this._source.cancel(reason) : undefined);
    }
    pipeTo(destination) {
      const reader = this.getReader(); const writer = destination.getWriter();
      const pump = () => reader.read().then(result => result.done ? writer.close() : Promise.resolve(writer.write(result.value)).then(pump));
      return pump().finally(() => reader.releaseLock());
    }
    pipeThrough(pair) { this.pipeTo(pair.writable); return pair.readable; }
  }
  class WritableStreamDefaultWriter {
    constructor(stream) { this._stream = stream; this.closed = stream._closedPromise; this.ready = Promise.resolve(); }
    write(chunk) { return this._stream._write(chunk); }
    close() { return this._stream._close(); }
    abort(reason) { return this._stream.abort(reason); }
    releaseLock() { this._stream._writer = null; this._stream = null; }
  }
  class WritableStream {
    constructor(underlyingSink = {}) {
      this._sink = underlyingSink || {}; this._writer = null; this._closed = false;
      this._closedPromise = new Promise(resolve => { this._closedResolve = resolve; });
      if (typeof this._sink.start === "function") Promise.resolve(this._sink.start(this));
    }
    get locked() { return this._writer !== null; }
    getWriter() { if (this.locked) throw new TypeError("WritableStream is locked"); this._writer = new WritableStreamDefaultWriter(this); return this._writer; }
    _write(chunk) { if (this._closed) return Promise.reject(new TypeError("WritableStream is closed")); return Promise.resolve(typeof this._sink.write === "function" ? this._sink.write(chunk, this) : undefined); }
    _close() { this._closed = true; const result = typeof this._sink.close === "function" ? this._sink.close() : undefined; this._closedResolve(); return Promise.resolve(result); }
    abort(reason) { this._closed = true; this._closedResolve(); return Promise.resolve(typeof this._sink.abort === "function" ? this._sink.abort(reason) : undefined); }
  }
  class TransformStreamSource {
    constructor(owner) { this._owner = owner; }
    start(controller) { this._owner._readableController = controller; }
  }
  class TransformStreamSink {
    constructor(owner) { this._owner = owner; }
    write(chunk) {
      const owner = this._owner;
      if (typeof owner._transformer.transform === "function") {
        return owner._transformer.transform(chunk, owner._readableController);
      }
      owner._readableController.enqueue(chunk);
    }
    close() {
      const owner = this._owner;
      if (typeof owner._transformer.flush === "function") {
        owner._transformer.flush(owner._readableController);
      }
      owner._readableController.close();
    }
  }
  class TransformStream {
    constructor(transformer = {}) {
      this._transformer = transformer || {};
      this._readableController = null;
      this.readable = new ReadableStream(new TransformStreamSource(this));
      this.writable = new WritableStream(new TransformStreamSink(this));
    }
  }
  globalThis.ReadableStream = ReadableStream;
  globalThis.ReadableStreamDefaultReader = ReadableStreamDefaultReader;
  globalThis.ReadableStreamDefaultController = ReadableStreamDefaultController;
  globalThis.WritableStream = WritableStream;
  globalThis.WritableStreamDefaultWriter = WritableStreamDefaultWriter;
  globalThis.TransformStream = TransformStream;

  class EventTarget {
    constructor() { this._listeners = new Map(); }
    addEventListener(type, callback, options = {}) {
      if (callback == null) return;
      const key = String(type);
      const capture = typeof options === "boolean" ? options : !!options.capture;
      const once = typeof options === "object" && !!options.once;
      const listeners = this._listeners.get(key) || [];
      if (!listeners.some(entry => entry.callback === callback && entry.capture === capture)) {
        listeners.push({ callback, capture, once });
      }
      this._listeners.set(key, listeners);
    }
    removeEventListener(type, callback, options = {}) {
      const listeners = this._listeners.get(String(type));
      if (!listeners) return;
      const capture = typeof options === "boolean" ? options : !!options.capture;
      const index = listeners.findIndex(entry => entry.callback === callback && entry.capture === capture);
      if (index >= 0) listeners.splice(index, 1);
    }
    dispatchEvent(event) {
      if (!(event instanceof Event)) throw new TypeError("dispatchEvent requires an Event");
      event.target = this;
      event.currentTarget = this;
      for (const entry of (this._listeners.get(event.type) || []).slice()) {
        if (entry.once) this.removeEventListener(event.type, entry.callback, entry.capture);
        if (typeof entry.callback === "function") entry.callback.call(this, event);
        else if (typeof entry.callback.handleEvent === "function") entry.callback.handleEvent(event);
        if (event.__stoppedImmediate) break;
      }
      event.currentTarget = null;
      return !event.defaultPrevented;
    }
  }

  class Animation extends EventTarget {
    constructor(target, keyframes, options = {}) {
      super();
      const timing = typeof options === "number" ? { duration: options } : (options || {});
      this.effect = { target, getKeyframes: () => this._keyframes.slice() };
      this.timeline = globalThis.document && globalThis.document.timeline || null;
      this.id = String(timing.id || "");
      this.currentTime = 0;
      this.startTime = null;
      this.playbackRate = 1;
      this.playState = "idle";
      this.replaceState = "active";
      this.pending = false;
      this.onfinish = null;
      this.oncancel = null;
      this._target = target;
      this._keyframes = Array.isArray(keyframes) ? keyframes.slice() : [keyframes || {}];
      this._duration = Math.max(0, Number(timing.duration) || 0);
      this._delay = Math.max(0, Number(timing.delay) || 0);
      this._iterations = timing.iterations === Infinity ? Infinity : Math.max(0, Number(timing.iterations) || 1);
      this._timer = null;
      this._finishedResolve = null;
      this.finished = new Promise(resolve => { this._finishedResolve = resolve; });
      this.ready = Promise.resolve(this);
      this.play();
    }
    _applyFinalKeyframe() {
      const frame = this._keyframes[this._keyframes.length - 1];
      if (!frame || !this._target || !this._target.style) return;
      for (const name of Object.keys(frame)) {
        if (name === "offset" || name === "easing" || name === "composite") continue;
        this._target.style[name] = frame[name];
      }
    }
    _complete() {
      if (this.playState === "finished" || this.playState === "idle") return;
      this._timer = null;
      this.currentTime = this._duration * this._iterations;
      this.playState = "finished";
      this._applyFinalKeyframe();
      this._finishedResolve(this);
      const event = new Event("finish");
      this.dispatchEvent(event);
      if (typeof this.onfinish === "function") this.onfinish.call(this, event);
    }
    play() {
      if (this._timer !== null) clearTimeout(this._timer);
      this.playState = "running";
      this.pending = false;
      if (this._iterations !== Infinity) {
        const total = this._delay + this._duration * this._iterations;
        this._timer = setTimeout(() => this._complete(), total);
      }
    }
    pause() {
      if (this._timer !== null) clearTimeout(this._timer);
      this._timer = null;
      this.playState = "paused";
    }
    reverse() { this.playbackRate = -Math.abs(this.playbackRate || 1); this.play(); }
    finish() { if (this._timer !== null) clearTimeout(this._timer); this._complete(); }
    cancel() {
      if (this._timer !== null) clearTimeout(this._timer);
      this._timer = null;
      this.currentTime = null;
      this.playState = "idle";
      const event = new Event("cancel");
      this.dispatchEvent(event);
      if (typeof this.oncancel === "function") this.oncancel.call(this, event);
    }
    updatePlaybackRate(rate) { this.playbackRate = Number(rate); }
    persist() { this.replaceState = "persisted"; }
    commitStyles() { this._applyFinalKeyframe(); }
  }

  Element.prototype.animate = function(keyframes, options) {
    const animation = new Animation(this, keyframes, options);
    if (!this.__animations) this.__animations = [];
    this.__animations.push(animation);
    return animation;
  };
  Element.prototype.getAnimations = function() {
    return (this.__animations || []).filter(animation => animation.playState !== "idle").slice();
  };
  Document.prototype.getAnimations = function() {
    const animations = [];
    const visit = node => {
      if (node && typeof node.getAnimations === "function") animations.push(...node.getAnimations());
      for (const child of node && node.childNodes || []) visit(child);
    };
    visit(this.documentElement);
    return animations;
  };
  if (!globalThis.document.timeline) {
    globalThis.document.timeline = { currentTime: 0 };
  }
  globalThis.Animation = Animation;

  class AbortSignal extends EventTarget {
    constructor() {
      super();
      this.aborted = false;
      this.reason = undefined;
      this.onabort = null;
    }
    throwIfAborted() { if (this.aborted) throw this.reason; }
    static abort(reason = new DOMException("The operation was aborted.", "AbortError")) {
      const controller = new AbortController();
      controller.abort(reason);
      return controller.signal;
    }
    static timeout(milliseconds) {
      const controller = new AbortController();
      setTimeout(() => controller.abort(new DOMException("The operation timed out.", "TimeoutError")), Number(milliseconds));
      return controller.signal;
    }
    static any(signals) {
      const controller = new AbortController();
      for (const signal of signals) {
        if (signal.aborted) { controller.abort(signal.reason); break; }
        signal.addEventListener("abort", () => controller.abort(signal.reason), { once: true });
      }
      return controller.signal;
    }
  }

  class AbortController {
    constructor() { this.signal = new AbortSignal(); }
    abort(reason = new DOMException("The operation was aborted.", "AbortError")) {
      if (this.signal.aborted) return;
      this.signal.aborted = true;
      this.signal.reason = reason;
      const event = new Event("abort");
      this.signal.dispatchEvent(event);
      if (typeof this.signal.onabort === "function") this.signal.onabort.call(this.signal, event);
    }
  }
  globalThis.EventTarget = EventTarget;
  globalThis.AbortSignal = AbortSignal;
  globalThis.AbortController = AbortController;

  class Headers {
    constructor(init = undefined) {
      this._headers = new Map();
      if (init instanceof Headers) init.forEach((value, name) => this.append(name, value));
      else if (Array.isArray(init)) for (const entry of init) this.append(entry[0], entry[1]);
      else if (init && typeof init === "object") for (const name of Object.keys(init)) this.append(name, init[name]);
    }
    append(name, value) {
      const key = String(name).toLowerCase();
      const text = String(value).trim();
      this._headers.set(key, this._headers.has(key) ? this._headers.get(key) + ", " + text : text);
    }
    set(name, value) { this._headers.set(String(name).toLowerCase(), String(value).trim()); }
    get(name) { return this._headers.get(String(name).toLowerCase()) ?? null; }
    has(name) { return this._headers.has(String(name).toLowerCase()); }
    delete(name) { this._headers.delete(String(name).toLowerCase()); }
    forEach(callback, thisArg) { for (const [name, value] of this._headers) callback.call(thisArg, value, name, this); }
    *entries() { yield* this._headers.entries(); }
    *keys() { yield* this._headers.keys(); }
    *values() { yield* this._headers.values(); }
    [Symbol.iterator]() { return this.entries(); }
  }
  class Request {
    constructor(input, init = {}) {
      const source = input instanceof Request ? input : null;
      this.url = source ? source.url : String(input);
      this.method = String(init.method || (source && source.method) || "GET").toUpperCase();
      this.headers = new Headers(init.headers || (source && source.headers));
      this.body = init.body === undefined ? (source && source.body) : init.body;
      this.credentials = init.credentials || (source && source.credentials) || "same-origin";
      this.mode = init.mode || (source && source.mode) || "cors";
      this.signal = init.signal || (source && source.signal) || null;
    }
    clone() { return new Request(this); }
  }
  class Response {
    constructor(body = null, init = {}) {
      this._body = body === null ? "" : String(body);
      this.status = init.status === undefined ? 200 : Number(init.status);
      this.statusText = init.statusText || "";
      this.headers = new Headers(init.headers);
      this.url = init.url || "";
      this.type = "basic";
      this.redirected = false;
      this.bodyUsed = false;
    }
    get ok() { return this.status >= 200 && this.status <= 299; }
    text() { this.bodyUsed = true; return Promise.resolve(this._body); }
    json() { return this.text().then(JSON.parse); }
    arrayBuffer() { return Promise.resolve(new TextEncoder().encode(this._body).buffer); }
    clone() { return new Response(this._body, { status: this.status, statusText: this.statusText, headers: this.headers, url: this.url }); }
    static json(data, init = {}) {
      const headers = new Headers(init.headers);
      if (!headers.has("content-type")) headers.set("content-type", "application/json");
      return new Response(JSON.stringify(data), { ...init, headers });
    }
    static redirect(url, status = 302) { return new Response(null, { status, headers: { location: String(url) } }); }
    static error() { const response = new Response(null, { status: 0 }); response.type = "error"; return response; }
  }
  globalThis.Headers = Headers;
  globalThis.Request = Request;
  globalThis.Response = Response;

  globalThis.fetch = function(input, init = {}) {
    const request = input instanceof Request ? new Request(input, init) : new Request(input, init);
    if (request.signal && request.signal.aborted) return Promise.reject(request.signal.reason);
    return Promise.resolve(__omoikane_fetch(request.url)).then(raw => {
      if (request.signal && request.signal.aborted) throw request.signal.reason;
      const data = JSON.parse(String(raw));
      return new Response(data.bodyText, { status: data.status, url: data.url });
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
