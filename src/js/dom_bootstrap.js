(() => {
  // Native bindings are installed before this bootstrap runs. Keep the
  // unfiltered slot lookup private to the event dispatcher so page scripts
  // cannot use it to inspect closed shadow trees.
  const internalAssignedSlot = globalThis.__omoikane_internal_assigned_slot;
  delete globalThis.__omoikane_internal_assigned_slot;
  // Keep the host clipboard bindings private to this bootstrap closure. Page
  // code must go through the Promise-based Clipboard API, where secure-context
  // and permission checks are applied consistently.
  const nativeClipboardReadText = globalThis.__omoikane_clipboard_read_text;
  const nativeClipboardWriteText = globalThis.__omoikane_clipboard_write_text;
  const nativeClipboardPermission = globalThis.__omoikane_clipboard_permission;
  const nativeGeolocationPermission = globalThis.__omoikane_geolocation_permission;
  const nativeIsSecureContext = globalThis.__omoikane_is_secure_context;
  // Worklet lifecycle bindings are private implementation hooks. Keep them in
  // this closure so page code can only reach the standard Worklet methods.
  const nativeCreateWorklet = globalThis.__omoikane_create_worklet;
  const nativeWorkletAddModule = globalThis.__omoikane_worklet_add_module;
  const nativeWorkletRegister = globalThis.__omoikane_worklet_register;
  const nativeWorkletRegisteredNames = globalThis.__omoikane_worklet_registered_names;
  const nativeWorkletModuleCount = globalThis.__omoikane_worklet_module_count;
  const nativeWorkletTeardown = globalThis.__omoikane_worklet_teardown;
  const nativeCryptoRandom = globalThis.__omoikane_crypto_random;
  const nativeCryptoDigest = globalThis.__omoikane_crypto_digest;
  const nativeCryptoHmac = globalThis.__omoikane_crypto_hmac;
  delete globalThis.__omoikane_clipboard_read_text;
  delete globalThis.__omoikane_clipboard_write_text;
  delete globalThis.__omoikane_clipboard_permission;
  delete globalThis.__omoikane_geolocation_permission;
  delete globalThis.__omoikane_is_secure_context;
  delete globalThis.__omoikane_create_worklet;
  delete globalThis.__omoikane_worklet_add_module;
  delete globalThis.__omoikane_worklet_register;
  delete globalThis.__omoikane_worklet_registered_names;
  delete globalThis.__omoikane_worklet_module_count;
  delete globalThis.__omoikane_worklet_teardown;
  delete globalThis.__omoikane_crypto_random;
  delete globalThis.__omoikane_crypto_digest;
  delete globalThis.__omoikane_crypto_hmac;

  // The top-level browsing context is its own parent and top-level context.
  globalThis.parent = globalThis;
  globalThis.top = globalThis;
  const cache = new Map();
  const validatesMaskOrClip = new Set([
    "clip-path", "-webkit-clip-path", "mask", "-webkit-mask",
    "mask-image", "-webkit-mask-image", "mask-mode", "-webkit-mask-mode",
    "mask-composite", "-webkit-mask-composite",
  ]);
  const customElementConstructionStack = [];
  const customElementDefinitionByConstructor = new Map();
  const customElementRegistryByDocument = new WeakMap();
  const knownSlots = [];
  const slotAssignmentSignatures = new WeakMap();
  const pendingSlotChanges = [];
  let pendingSlotChangeSet = new WeakSet();
  let slotChangeMicrotaskQueued = false;
  let slotAssignmentRefreshQueued = false;

  // Window objects are not ordinary DOM wrappers in Omoikane: the top-level
  // Window is Boa's global object, while nested browsing contexts currently
  // expose a small facade. Keep their identity explicit instead of changing
  // the global object's prototype, which could disturb Boa's global property
  // lookup and built-in prototype chain.
  const windowObjects = new WeakSet([globalThis]);
  const hasWeakWindowRegistry = typeof WeakRef === "function";
  const browsingWindowRefs = [];
  const MAX_STRONG_WINDOW_ENTRIES = 1024;
  function registerBrowsingWindow(window) {
    if (hasWeakWindowRegistry) browsingWindowRefs.push(new WeakRef(window));
    else {
      if (browsingWindowRefs.length >= MAX_STRONG_WINDOW_ENTRIES) browsingWindowRefs.shift();
      browsingWindowRefs.push(window);
    }
  }
  function liveBrowsingWindows() {
    const windows = [globalThis];
    for (let index = browsingWindowRefs.length - 1; index >= 0; index--) {
      const window = hasWeakWindowRegistry
        ? browsingWindowRefs[index].deref()
        : browsingWindowRefs[index];
      if (window) windows.push(window);
      else browsingWindowRefs.splice(index, 1);
    }
    return windows;
  }
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

  // Slot assignment is calculated natively from the current light and shadow
  // trees. Keep only the previous id sequence here so `slotchange` can be
  // coalesced at the microtask boundary after any relevant DOM mutation.
  function queueSlotChange(slot) {
    if (!pendingSlotChangeSet.has(slot)) {
      pendingSlotChangeSet.add(slot);
      pendingSlotChanges.push(slot);
    }
    if (slotChangeMicrotaskQueued) return;
    slotChangeMicrotaskQueued = true;
    Promise.resolve().then(() => {
      slotChangeMicrotaskQueued = false;
      const slots = pendingSlotChanges.splice(0);
      pendingSlotChangeSet = new WeakSet();
      for (const changedSlot of slots) {
        changedSlot.dispatchEvent(new Event("slotchange", { bubbles: true }));
      }
    });
  }

  function signalFallbackSlotChanges(node) {
    for (let current = node; current; current = current.parentNode) {
      if (current instanceof HTMLSlotElement &&
          current.getRootNode() instanceof ShadowRoot &&
          current.assignedNodes().length === 0) {
        queueSlotChange(current);
      }
    }
  }

  function refreshSlotAssignments() {
    if (slotAssignmentRefreshQueued) return;
    slotAssignmentRefreshQueued = true;
    Promise.resolve().then(() => {
      slotAssignmentRefreshQueued = false;
      for (let index = 0; index < knownSlots.length; index += 1) {
        const node = knownSlots[index];
        const ids = __omoikane_assigned_nodes(node.__id, false) || [];
        const signature = ids.join(",");
        const previous = slotAssignmentSignatures.get(node);
        slotAssignmentSignatures.set(node, signature);
        if (previous === undefined ? signature === "" : previous === signature) continue;
        queueSlotChange(node);
      }
    });
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
      node = __omoikane_shadow_host(id) === null
        ? new DocumentFragment(id)
        : new ShadowRoot(id, SHADOW_ROOT_CONSTRUCTION);
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
    if (node instanceof HTMLSlotElement) knownSlots.push(node);
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
    if (node.nodeType === 1 && String(node.localName).toLowerCase() === "template") {
      stampOwnerDoc(node.content, templateContentsOwnerDocument(doc));
    }
    const children = node.childNodes;
    for (let i = 0; i < children.length; i++) {
      stampOwnerDoc(children[i], doc);
    }
  }

  // Each Document has a separate inert document that owns the contents of its
  // template elements. Keep it stable and shared by all templates created by
  // that document, matching the HTML template contents owner-document model.
  function templateContentsOwnerDocument(doc) {
    if (!doc.__templateContentsOwnerDocument) {
      const owner = wrapNode(__omoikane_create_document());
      owner.__documentURL = "about:blank";
      doc.__templateContentsOwnerDocument = owner;
    }
    return doc.__templateContentsOwnerDocument;
  }

  function invokeListeners(node, event, capture, phase) {
    const listeners = (node.__listeners.get(event.type) || []).slice();
    for (const entry of listeners) {
      if (!entry.removed && !!entry.capture === capture) {
        if (entry.once) {
          node.removeEventListener(event.type, entry.listener, entry.capture);
        }
        event.currentTarget = node;
        event.eventPhase = phase;
        __omoikane_call_event_listener(
          entry.listener,
          typeof entry.listener === "function" ? node : entry.listener,
          event
        );
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
  const SHADOW_ROOT_CONSTRUCTION = {};

  class Event {
    constructor(type, init = {}) {
      init = init ?? {};
      this.type = String(type);
      this.bubbles = !!init.bubbles;
      this.cancelable = !!init.cancelable;
      this.composed = !!init.composed;
      this.target = null;
      this.currentTarget = null;
      this.eventPhase = 0;
      this.defaultPrevented = false;
      this.isTrusted = false;
      this.timeStamp = Date.now();
      this.__stopped = false;
      this.__stoppedImmediate = false;
      this.__dispatching = false;
      this.__path = [];
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
      if (this.__dispatching) return;
      this.type = String(type);
      this.bubbles = !!bubbles;
      this.cancelable = !!cancelable;
      this.composed = false;
    }

    composedPath() {
      if (!this.currentTarget) return [];
      // Visibility is based on the dispatch path snapshot. DOM mutations from
      // an earlier listener must not reveal or hide closed-root entries for a
      // later listener while the same dispatch is still in progress.
      const currentEntry = this.__path.find(entry => entry.node === this.currentTarget);
      const visibleClosedRoots = currentEntry ? currentEntry.closedRoots : [];
      return this.__path
        .filter(entry => entry.closedRoots.every(root => visibleClosedRoots.includes(root)))
        .map(entry => entry.node);
    }
  }

  class UIEvent extends Event {
    constructor(type, init = {}) {
      init = init ?? {};
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
      init = init ?? {};
      super(type, init);
      this.detail = init.detail ?? null;
    }
  }

  class MessageEvent extends Event {
    constructor(type, init = {}) {
      init = init ?? {};
      super(type, init);
      // Preserve an explicitly-cloned `undefined` payload.  The MessageEvent
      // constructor defaults to null only when `data` is absent; posted-message
      // delivery can legitimately carry the structured-clone value undefined.
      this.data = Object.prototype.hasOwnProperty.call(init, "data") ? init.data : null;
      this.origin = init.origin ?? "";
      this.lastEventId = init.lastEventId ?? "";
      this.source = init.source ?? null;
      this.ports = init.ports ?? [];
    }
  }

  class StorageEvent extends Event {
    constructor(type, init = {}) {
      init = init ?? {};
      super(type, init);
      this.key = init.key === undefined ? null : init.key;
      this.oldValue = init.oldValue === undefined ? null : init.oldValue;
      this.newValue = init.newValue === undefined ? null : init.newValue;
      this.url = String(init.url ?? "");
      this.storageArea = init.storageArea ?? null;
    }
  }

  class MouseEvent extends Event {
    constructor(type, init = {}) {
      init = init ?? {};
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

  class WheelEvent extends MouseEvent {
    constructor(type, init = {}) {
      init = init ?? {};
      super(type, init);
      this.deltaX = Number(init.deltaX ?? 0);
      this.deltaY = Number(init.deltaY ?? 0);
      this.deltaZ = Number(init.deltaZ ?? 0);
      this.deltaMode = Number(init.deltaMode ?? 0);
    }
  }
  WheelEvent.DOM_DELTA_PIXEL = 0;
  WheelEvent.DOM_DELTA_LINE = 1;
  WheelEvent.DOM_DELTA_PAGE = 2;
  WheelEvent.prototype.DOM_DELTA_PIXEL = 0;
  WheelEvent.prototype.DOM_DELTA_LINE = 1;
  WheelEvent.prototype.DOM_DELTA_PAGE = 2;

  class KeyboardEvent extends Event {
    constructor(type, init = {}) {
      init = init ?? {};
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
      this.text = init.text ?? "";
    }
  }

  class InputEvent extends UIEvent {
    constructor(type, init = {}) {
      init = init ?? {};
      super(type, init);
      this.data = init.data === undefined ? null : init.data;
      this.inputType = String(init.inputType ?? "");
      this.isComposing = Boolean(init.isComposing);
    }
  }

  class FocusEvent extends Event {
    constructor(type, init = {}) {
      init = init ?? {};
      super(type, init);
      this.relatedTarget = init.relatedTarget ?? null;
    }
  }

  class TransitionEvent extends Event {
    constructor(type, init = {}) {
      init = init ?? {};
      super(type, init);
      this.propertyName = String(init.propertyName ?? "");
      this.elapsedTime = Number(init.elapsedTime ?? 0);
      this.pseudoElement = String(init.pseudoElement ?? "");
    }
  }

  function shadowRootsContaining(node) {
    if (!(node instanceof Node)) return [];
    const roots = [];
    let current = node;
    while (current) {
      const root = nodeRoot(current);
      if (!(root instanceof ShadowRoot)) break;
      if (root.mode === "closed") roots.push(root);
      current = root.host;
    }
    return roots;
  }

  function isNodeInsideShadowRoot(node, root) {
    if (!(node instanceof Node)) return false;
    for (let current = node; current;) {
      if (current === root) return true;
      current = current.parentNode ||
        (current instanceof ShadowRoot ? current.host : null);
    }
    return false;
  }

  // Retarget a node against the tree in which a listener lives. A listener
  // outside a shadow root observes its host, while listeners in that same root
  // retain the original target. Slotted light-DOM nodes keep their identity
  // because their tree root remains the outer document.
  function retargetNode(node, against) {
    let target = node;
    while (target instanceof Node) {
      const root = nodeRoot(target);
      if (!(root instanceof ShadowRoot)) return target;
      if (isNodeInsideShadowRoot(against, root)) return target;
      target = root.host;
    }
    return target;
  }

  function eventParent(node, event, originalRoot) {
    if (!(node instanceof Node)) return null;
    const assignedSlotId = internalAssignedSlot(node.__id);
    if (assignedSlotId !== null && assignedSlotId !== undefined) {
      return wrapNode(assignedSlotId);
    }
    if (node instanceof ShadowRoot) {
      // A non-composed event only stops at the shadow root in which it was
      // dispatched. Events from slotted light-DOM nodes still leave the root.
      return event.composed || originalRoot !== node ? node.host : null;
    }
    if (node.nodeType === 9) return node.defaultView;
    return node.parentNode;
  }

  function buildEventPath(target, event) {
    const path = [];
    const originalRoot = target instanceof Node ? nodeRoot(target) : null;
    let current = target;
    while (current) {
      const entry = {
        node: current,
        closedRoots: shadowRootsContaining(current),
        target: retargetNode(target, current),
        relatedTarget: retargetNode(event.__originalRelatedTarget, current),
        hostTarget: false,
      };
      // Once target and relatedTarget become indistinguishable at a boundary,
      // the remainder of the event path is omitted. This prevents mouse/focus
      // transitions inside one shadow tree from leaking out as host-to-host
      // transitions.
      if (entry.relatedTarget !== null && entry.relatedTarget === entry.target) break;
      path.push(entry);
      const parent = eventParent(current, event, originalRoot);
      if (parent && current instanceof ShadowRoot &&
          retargetNode(target, parent) === parent) {
        // Capturing and bubbling listeners on a host are invoked as AT_TARGET
        // when an event crosses out of its shadow root.
        const hostEntry = {
          node: parent,
          closedRoots: shadowRootsContaining(parent),
          target: parent,
          relatedTarget: retargetNode(event.__originalRelatedTarget, parent),
          hostTarget: true,
        };
        if (hostEntry.relatedTarget !== null && hostEntry.relatedTarget === hostEntry.target) break;
        path.push(hostEntry);
        current = eventParent(parent, event, originalRoot);
      } else {
        current = parent;
      }
    }
    return path;
  }

  function invokeEventPathEntry(entry, event, capture, phase) {
    event.target = entry.target;
    event.relatedTarget = entry.relatedTarget;
    if (entry.relatedTarget !== null && entry.relatedTarget === entry.target) {
      return event.__stopped;
    }
    return invokeListeners(entry.node, event, capture, entry.hostTarget ? 2 : phase);
  }

  function dispatchEventOnTarget(target, event) {
    if (!(event instanceof Event)) throw new TypeError("dispatchEvent requires an Event");
    if (event.__dispatching || event.type === "") {
      throw new DOMException("The event is already being dispatched or has no type.", "InvalidStateError");
    }
    event.__dispatching = true;
    event.__stopped = false;
    event.__stoppedImmediate = false;
    event.__originalRelatedTarget = event.relatedTarget ?? null;
    const path = buildEventPath(target, event);
    event.__path = path;

    try {
      if (path.length === 0) return !event.defaultPrevented;
      let stopped = false;
      for (let i = path.length - 1; i >= 1; i -= 1) {
        if (invokeEventPathEntry(path[i], event, true, 1)) {
          stopped = true;
          break;
        }
      }

      if (!stopped) {
        invokeEventPathEntry(path[0], event, true, 2);
      }
      if (!stopped && !event.__stoppedImmediate) {
        invokeEventPathEntry(path[0], event, false, 2);
      }
      if (event.__stopped) stopped = true;

      if (!stopped) {
        for (let i = 1; i < path.length; i += 1) {
          const entry = path[i];
          if (!event.bubbles && !entry.hostTarget) continue;
          if (invokeEventPathEntry(entry, event, false, 3)) break;
        }
      }
    } finally {
      event.__dispatching = false;
      event.currentTarget = null;
      event.eventPhase = 0;
      const finalEntry = path[path.length - 1];
      const clearTargets = !finalEntry ||
        (finalEntry.target instanceof Node && nodeRoot(finalEntry.target) instanceof ShadowRoot) ||
        (finalEntry.relatedTarget instanceof Node &&
          nodeRoot(finalEntry.relatedTarget) instanceof ShadowRoot);
      event.target = clearTargets ? null : finalEntry.target;
      event.relatedTarget = clearTargets ? null : finalEntry.relatedTarget;
      event.__path = [];
    }
    return !event.defaultPrevented;
  }

  // --- Document focus state -------------------------------------------------
  //
  // The focused element is stored per Document, on the document wrapper itself
  // (`__focusedElementId`), so an iframe's sub-document keeps its own active
  // element. `focusedDocumentId` is the document whose browsing context holds
  // focus; `null` means the top-level document, which is focused on load.
  //
  // While focus moves between browsing contexts it is `NO_FOCUSED_DOCUMENT`: no
  // document reports focus for the duration of the `blur` pair announcing the
  // context being left, matching Firefox 152.
  const NO_FOCUSED_DOCUMENT = Symbol("no focused document");
  let focusedDocumentId = null;

  // HTML's rules for parsing integers: optional whitespace, an optional sign,
  // then at least one digit. Trailing junk is ignored, so `tabindex="1.5"` is
  // valid while `tabindex="abc"` is not.
  function hasIntegerTabindex(element) {
    const value = element.getAttribute("tabindex");
    return value !== null && /^[ \t\n\f\r]*[+-]?[0-9]/.test(value);
  }

  // An editing host is focusable; nodes merely *inside* one are not.
  function isEditingHost(element) {
    const value = element.getAttribute("contenteditable");
    return value !== null && (value === "" || value.toLowerCase() === "true");
  }

  // Elements that are focusable without a tabindex attribute. Verified against
  // Firefox 152: `object`, `area`, `details`, `dialog`, `img`, `label`, `option`
  // and `fieldset` are not focusable, `a` needs an href, `input` must not be
  // hidden, and media elements need a `controls` attribute.
  function isInherentlyFocusable(element) {
    switch ((element.localName || element.nodeName || "").toLowerCase()) {
      case "a":
        return element.hasAttribute("href");
      case "input":
        return (element.getAttribute("type") || "").toLowerCase() !== "hidden";
      case "button":
      case "select":
      case "textarea":
      case "iframe":
      case "embed":
        return true;
      case "audio":
      case "video":
        return element.hasAttribute("controls");
      case "summary": {
        const parent = element.parentNode;
        return !!parent && parent.nodeType === 1 &&
          (parent.localName || parent.nodeName || "").toLowerCase() === "details";
      }
      default:
        return false;
    }
  }

  // Whether `focus()` on this node designates it as its document's focused area
  // (HTML "focusable area"). Rendered-ness is resolved by the native style
  // resolver because it must account for CSS rules and ancestor display state.
  // A control inside a `<fieldset disabled>` is disabled without carrying the
  // attribute itself — the same gap `:disabled` matching has.
  function canBeFocused(node) {
    if (!(node instanceof Element) || node.nodeType !== 1) return false;
    if (!node.isConnected) return false;
    if (node.__isDisabledControl && node.__isDisabledControl()) return false;
    if (!isRenderedForFocus(node)) return false;
    return Boolean(node.__dialogFocusFallback) || hasIntegerTabindex(node) ||
      isInherentlyFocusable(node) || isEditingHost(node);
  }

  // Returns the focusable areas participating in sequential focus navigation.
  // Positive tabindex values precede the ordinary (zero/implicit) group; tree
  // order breaks ties. A negative tabindex remains programmatically focusable
  // but is deliberately absent from this list.
  function sequentialFocusCandidates(doc) {
    const positive = [];
    const ordinary = [];
    let treeIndex = 0;
    function visit(node) {
      if (node instanceof Element) {
        const index = treeIndex++;
        if (canBeFocused(node)) {
          const explicit = hasIntegerTabindex(node);
          const tabIndex = explicit ? parseInt(node.getAttribute("tabindex"), 10) : 0;
          if (tabIndex > 0) positive.push({ node, tabIndex, index });
          else if (tabIndex === 0) ordinary.push(node);
        }
      }
      for (const child of node.childNodes || []) visit(child);
    }
    visit(doc);
    positive.sort((left, right) =>
      left.tabIndex - right.tabIndex || left.index - right.index
    );
    return positive.map(entry => entry.node).concat(ordinary);
  }

  function performSequentialFocusNavigation(doc, backward) {
    const candidates = sequentialFocusCandidates(doc);
    if (candidates.length === 0) return false;
    const focused = focusedElementOf(doc);
    const current = candidates.indexOf(focused);
    let next;
    if (current < 0) next = backward ? candidates.length - 1 : 0;
    else next = (current + (backward ? -1 : 1) + candidates.length) % candidates.length;
    candidates[next].focus();
    return true;
  }

  function isRenderedForFocus(node) {
    return !!__omoikane_is_rendered_for_focus(node.__id);
  }

  // Resolves the element explicitly focused in `doc`, applying HTML's focus
  // fixup rule: an element removed from the document (or moved into another
  // one) silently stops being the active element, without blur or focusout.
  // Firefox behaves the same; Chromium fires blur from a later rendering
  // update. Both `activeElement` and `focus()`/`blur()` go through here so a
  // detached element can never receive a blur.
  function focusedElementOf(doc) {
    const id = doc.__focusedElementId;
    if (id === null || id === undefined) return null;
    const node = wrapNode(id);
    if (node && node.nodeType === 1 && node.isConnected && node.ownerDocument === doc && isRenderedForFocus(node)) {
      return node;
    }
    doc.__focusedElementId = null;
    return null;
  }

  // `doc` and its ancestor documents, innermost first, each paired with the
  // iframe element hosting it (`null` for the top-level document).
  //
  // Returns `null` when the walk does not reach the top-level document, which
  // means `doc` has no browsing context: a document created by
  // `createHTMLDocument`, or one whose frame was removed or reloaded.
  function documentChain(doc) {
    const chain = [];
    let current = doc;
    while (current instanceof Document) {
      if (current.__id === __omoikane_document_id) {
        chain.push({ document: current, frame: null });
        return chain;
      }
      const frameId = __omoikane_document_owner_iframe(current.__id);
      const frame = (frameId === null || frameId === undefined) ? null : wrapNode(frameId);
      if (!frame) return null;
      chain.push({ document: current, frame });
      current = frame.ownerDocument;
    }
    return null;
  }

  // The focused document followed by its ancestor documents, up to the
  // top-level one. A focused document that is no longer reachable (its iframe
  // was reloaded or removed) hands focus back to the top-level document.
  function focusChainDocuments() {
    if (focusedDocumentId === NO_FOCUSED_DOCUMENT) return [];
    const top = wrapNode(__omoikane_document_id);
    if (focusedDocumentId === null || focusedDocumentId === __omoikane_document_id) {
      return [top];
    }
    const chain = documentChain(wrapNode(focusedDocumentId));
    if (!chain) {
      focusedDocumentId = null;
      return [top];
    }
    return chain.map(entry => entry.document);
  }

  // No focus event is cancelable, and all four are composed so they cross
  // shadow boundaries. Bubbling differs per event — `focusin` and `focusout`
  // bubble, `focus` and `blur` do not — so each caller passes it in.
  function fireFocusEvent(target, type, relatedTarget, bubbles) {
    target.dispatchEvent(new FocusEvent(type, {
      bubbles,
      composed: true,
      relatedTarget,
    }));
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
        const connectedBeforeMove = children.map(node => node.isConnected);
        for (const c of children) {
          notifyImplicitRemoval(c);
          __omoikane_append_child(this.__id, c.__id);
        }
        for (let i = 0; i < children.length; i++) {
          if (connectedBeforeMove[i]) disconnectCustomElementTree(children[i]);
        }
        upgradeInsertedCustomElements(this, children);
        if (children.length) queueMutation(this, "childList", { addedNodes: children, previousSibling });
        refreshSlotAssignments();
        signalFallbackSlotChanges(this);
        return child;
      }
      this.__ensureNotAncestor(child);
      const connectedBeforeMove = !!(child && child.isConnected);
      notifyImplicitRemoval(child);
      __omoikane_append_child(this.__id, child.__id);
      if (connectedBeforeMove) disconnectCustomElementTree(child);
      upgradeInsertedCustomElements(this, [child]);
      queueMutation(this, "childList", { addedNodes: [child], previousSibling });
      refreshSlotAssignments();
      signalFallbackSlotChanges(this);
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
      const capture = typeof options === "boolean" ? options : !!(options && options.capture);
      const once = typeof options === "object" && options !== null && !!options.once;
      const key = String(type);
      const list = this.__listeners.get(key) ?? [];
      // Deduplicate: same listener+capture is only registered once (DOM spec).
      if (!list.some(entry => entry.listener === listener && !!entry.capture === capture)) {
        list.push({ listener, capture, once, removed: false });
      }
      this.__listeners.set(key, list);
    }

    removeEventListener(type, listener, options = false) {
      const capture = typeof options === "boolean" ? options : !!(options && options.capture);
      const key = String(type);
      const list = this.__listeners.get(key);
      if (!list) return;
      const index = list.findIndex(entry => entry.listener === listener && !!entry.capture === capture);
      if (index !== -1) {
        list[index].removed = true;
        list.splice(index, 1);
      }
    }

    dispatchEvent(event) {
      const dispatchEvent = event instanceof Event ? event : new Event(event);
      const notCanceled = dispatchEventOnTarget(this, dispatchEvent);
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

    get assignedSlot() {
      return wrapNode(__omoikane_assigned_slot(this.__id));
    }

    getRootNode(options = {}) {
      let root = this;
      while (root.parentNode) root = root.parentNode;
      if (options && options.composed) {
        while (root instanceof ShadowRoot) {
          root = root.host;
          while (root.parentNode) root = root.parentNode;
        }
      }
      return root;
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
      const newValue = String(value);
      __omoikane_set_attribute(this.__id, attr, newValue);
      queueMutation(this, "attributes", { attributeName: attr, oldValue });
      const callbackName = (this.namespaceURI === null || this.namespaceURI === HTML_NAMESPACE)
        ? attr.replace(/[A-Z]/g, letter => letter.toLowerCase())
        : attr;
      notifyCustomElementAttributeChanged(this, callbackName, oldValue, newValue, null);
      if (callbackName === "slot" ||
          (callbackName === "name" && this instanceof HTMLSlotElement)) {
        refreshSlotAssignments();
      }
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
        node.setAttribute("style", serializeDecls(decls));

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
        value = String(value);
        if (kebab === "transition" || kebab.startsWith("transition-") || validatesMaskOrClip.has(kebab)) {
          const normalized = __omoikane_normalize_style_value(kebab, value);
          if (normalized === null) return;
          value = normalized;
        }
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
        node.setAttribute("style", value == null ? "" : String(value));
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
      const wasConnected = this.isConnected;
      const removedNodes = this.childNodes.slice();
      // Native replaces all children at once. Notify from the end so boundary
      // offsets for later siblings are subsequently decremented by removals of
      // their preceding siblings, matching sequential pre-remove semantics.
      for (const child of this.childNodes.slice().reverse()) preRemove(this, child);
      __omoikane_set_text_content(this.__id, text);
      const addedNodes = this.childNodes.slice();
      if (wasConnected) {
        for (const child of removedNodes) disconnectCustomElementTree(child);
      }
      if (removedNodes.length || addedNodes.length) {
        queueMutation(this, "childList", { addedNodes, removedNodes });
      }
      refreshSlotAssignments();
      signalFallbackSlotChanges(this);
    }

    get innerHTML() {
      return __omoikane_get_inner_html(this.__id) || "";
    }

    set innerHTML(value) {
      const html = value == null ? "" : String(value);
      const wasConnected = this.isConnected;
      const removedNodes = this.childNodes.slice();
      for (const child of removedNodes.slice().reverse()) preRemove(this, child);
      __omoikane_set_inner_html(this.__id, html);
      const addedNodes = this.childNodes.slice();
      if (wasConnected) {
        for (const child of removedNodes) disconnectCustomElementTree(child);
      }
      const owner = this.nodeType === 9 ? this : this.ownerDocument;
      const registry = owner && customElementRegistryByDocument.get(owner);
      if (registry) {
        for (const node of addedNodes) upgradeCustomElementTree(registry, node);
      }
      if (removedNodes.length || addedNodes.length) {
        queueMutation(this, "childList", { addedNodes, removedNodes });
      }
      refreshSlotAssignments();
      signalFallbackSlotChanges(this);
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
      const wasConnected = child.isConnected;
      __omoikane_remove_child(this.__id, child.__id);
      if (wasConnected) disconnectCustomElementTree(child);
      queueMutation(this, "childList", { removedNodes: [child], previousSibling, nextSibling });
      refreshSlotAssignments();
      signalFallbackSlotChanges(this);
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
        const connectedBeforeMove = children.map(node => node.isConnected);
        const previousSibling = refNode ? refNode.previousSibling : this.lastChild;
        for (const child of children) {
          notifyImplicitRemoval(child);
          __omoikane_insert_before(this.__id, child.__id, refNode ? refNode.__id : null);
        }
        for (let i = 0; i < children.length; i++) {
          if (connectedBeforeMove[i]) disconnectCustomElementTree(children[i]);
        }
        upgradeInsertedCustomElements(this, children);
        if (children.length) queueMutation(this, "childList", { addedNodes: children, previousSibling, nextSibling: refNode });
        refreshSlotAssignments();
        signalFallbackSlotChanges(this);
        return newNode;
      }
      const previousSibling = refNode ? refNode.previousSibling : this.lastChild;
      const connectedBeforeMove = !!(newNode && newNode.isConnected);
      notifyImplicitRemoval(newNode);
      __omoikane_insert_before(this.__id, newNode.__id, refNode ? refNode.__id : null);
      if (connectedBeforeMove) disconnectCustomElementTree(newNode);
      upgradeInsertedCustomElements(this, [newNode]);
      queueMutation(this, "childList", { addedNodes: [newNode], previousSibling, nextSibling: refNode });
      refreshSlotAssignments();
      signalFallbackSlotChanges(this);
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
        if (source instanceof HTMLTemplateElement) {
          preserveCharacterData(source.content, target.content);
        }
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
      if (oldValue !== null) {
        queueMutation(this, "attributes", { attributeName: attr, oldValue });
        const callbackName = (this.namespaceURI === null || this.namespaceURI === HTML_NAMESPACE)
          ? attr.replace(/[A-Z]/g, letter => letter.toLowerCase())
          : attr;
        notifyCustomElementAttributeChanged(this, callbackName, oldValue, null, null);
        if (callbackName === "slot" ||
            (callbackName === "name" && this instanceof HTMLSlotElement)) {
          refreshSlotAssignments();
        }
      }
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
      const newValue = String(value);
      this.__namespacedAttributes.set(key, { name, localName, namespaceURI: ns, value: newValue });
      __omoikane_set_attribute(this.__id, name, newValue);
      queueMutation(this, "attributes", { attributeName: localName, attributeNamespace: ns, oldValue });
      notifyCustomElementAttributeChanged(this, localName, oldValue, newValue, ns);
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
      notifyCustomElementAttributeChanged(this, entry.localName, entry.value, null, ns);
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
        current = current.parentNode ||
          (current instanceof ShadowRoot ? current.host : null);
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
      if (this.nodeType !== 1) return false;
      if (arguments.length === 0) {
        throw new TypeError("Element.matches requires a selector");
      }
      try {
        return !!__omoikane_matches_selector(this.__id, String(selector));
      } catch (error) {
        if (error && error.name === "SyntaxError") {
          throw new DOMException(error.message, "SyntaxError");
        }
        throw error;
      }
    }

    // Queries the native layout engine for this element's geometry, forcing a
    // synchronous reflow if the DOM changed since the last query.
    __layoutMetrics() {
      try {
        return JSON.parse(__omoikane_layout_metrics(this.__id));
      } catch (e) {
        return {
          x: 0, y: 0, width: 0, height: 0, top: 0, left: 0, right: 0, bottom: 0,
          contentX: 0, contentY: 0, contentWidth: 0, contentHeight: 0,
          offsetWidth: 0, offsetHeight: 0, offsetTop: 0, offsetLeft: 0,
          clientWidth: 0, clientHeight: 0, clientTop: 0, clientLeft: 0,
          scrollWidth: 0, scrollHeight: 0, scrollTop: 0, scrollLeft: 0,
          hasBox: false, clientRects: [],
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
      return m.clientRects || [{
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

    // The root element's scrolling box is the viewport, so its scroll offset is
    // the Window's. (In quirks mode that role belongs to `<body>` instead;
    // quirks mode is not modelled yet.)
    __isViewportScrollingElement() {
      const document = this.ownerDocument;
      return !!document && document === globalThis.document &&
        document.documentElement === this;
    }

    // Returns the scroll offset in effect: the stored offset clamped to the
    // element's current scrollable extent, or zero when it has no scrolling box.
    __scrollOffset() {
      if (this.__isViewportScrollingElement()) return windowScrollOffset();
      try {
        return JSON.parse(__omoikane_element_scroll_offset(this.__id));
      } catch (error) {
        return { x: 0, y: 0 };
      }
    }

    // Scrolls to (x, y), clamped natively. A changed target is queued there for
    // the next rendering opportunity. An element with no scrolling box is left
    // alone, so nothing is remembered that could not be applied.
    __applyScroll(x, y) {
      if (this.__isViewportScrollingElement()) {
        applyWindowScroll(x, y);
        return;
      }
      try {
        __omoikane_set_element_scroll(this.__id, Number(x), Number(y));
      } catch (error) {
        // An unresolvable node has no scrolling box; nothing to scroll.
      }
    }

    get scrollTop() { return this.__scrollOffset().y; }
    set scrollTop(value) { this.__applyScroll(this.__scrollOffset().x, value); }
    get scrollLeft() { return this.__scrollOffset().x; }
    set scrollLeft(value) { this.__applyScroll(value, this.__scrollOffset().y); }

    // `scroll()` and `scrollTo()` are the same operation. Both accept
    // `(x, y)` or a ScrollToOptions dictionary whose absent members keep the
    // current offset; `behavior` is accepted and ignored because only instant
    // scrolling is implemented.
    scrollTo(xOrOptions, y) {
      const current = this.__scrollOffset();
      if (isScrollOptions(xOrOptions)) {
        this.__applyScroll(
          xOrOptions.left === undefined ? current.x : Number(xOrOptions.left),
          xOrOptions.top === undefined ? current.y : Number(xOrOptions.top)
        );
      } else {
        this.__applyScroll(Number(xOrOptions), Number(y));
      }
    }

    scroll(xOrOptions, y) { this.scrollTo(xOrOptions, y); }

    scrollBy(xOrOptions, y) {
      const current = this.__scrollOffset();
      if (isScrollOptions(xOrOptions)) {
        this.__applyScroll(
          current.x + (xOrOptions.left === undefined ? 0 : Number(xOrOptions.left)),
          current.y + (xOrOptions.top === undefined ? 0 : Number(xOrOptions.top))
        );
      } else {
        this.__applyScroll(current.x + Number(xOrOptions), current.y + Number(y));
      }
    }
    get offsetParent() { return null; }

    // Designates this element as its document's focused area and dispatches
    // `blur`/`focusout` on the previously focused element followed by
    // `focus`/`focusin` on this one (UI Events focus event order).
    //
    // A non-focusable target (disconnected, disabled, or not a focusable area)
    // is ignored, and re-focusing the already focused element dispatches
    // nothing. `options.preventScroll` is accepted and ignored: focusing never
    // scrolls because scrolling is not implemented.
    focus(options) {
      if (!canBeFocused(this)) return;
      const doc = this.ownerDocument;
      if (!(doc instanceof Document)) return;
      const chain = documentChain(doc);
      // A document with no browsing context cannot hold system focus.
      if (!chain) return;

      const previousChain = focusChainDocuments();
      const previousDocument = previousChain[0];
      const previous = focusedElementOf(previousDocument);
      if (previousDocument === doc && previous === this) return;
      // A move across documents hides relatedTarget, because the element on the
      // other side belongs to a different tree.
      const crossesDocuments = previousDocument !== doc;
      const related = crossesDocuments ? null : this;

      // The spec takes focus away from the old element *before* dispatching
      // blur, so the active element is the viewport fallback for that pair.
      previousDocument.__focusedElementId = null;
      if (previous) {
        fireFocusEvent(previous, "blur", related, false);
        fireFocusEvent(previous, "focusout", related, true);
        commitTextControlChange(previous);
      }

      if (crossesDocuments) {
        // Documents dropping out of the chain lose their focused element; the
        // ones that stay are pointed at the frame below them further down, so
        // `parent.document.activeElement` follows the focus inwards.
        for (const document of previousChain) {
          if (!chain.some(entry => entry.document === document)) {
            document.__focusedElementId = null;
          }
        }
        // Each handler must observe the state its own event announces, so focus
        // leaves the old context before its `blur` pair and the new chain is
        // installed before the `focus` pair. Neither pair has a bubbling
        // counterpart, unlike the element events.
        focusedDocumentId = NO_FOCUSED_DOCUMENT;
        fireFocusEvent(previousDocument, "blur", null, false);
        const previousWindow = previousDocument.defaultView;
        if (previousWindow) fireFocusEvent(previousWindow, "blur", null, false);

        for (const entry of chain) {
          if (entry.frame) {
            entry.frame.ownerDocument.__focusedElementId = entry.frame.__id;
          }
        }
        focusedDocumentId = doc.__id;
        fireFocusEvent(doc, "focus", null, false);
        const nextWindow = doc.defaultView;
        if (nextWindow) fireFocusEvent(nextWindow, "focus", null, false);
      }

      doc.__focusedElementId = this.__id;
      focusedDocumentId = doc.__id;
      beginTextControlFocus(this);
      fireFocusEvent(this, "focus", crossesDocuments ? null : previous, false);
      fireFocusEvent(this, "focusin", crossesDocuments ? null : previous, true);
    }

    // Runs the unfocusing steps: focus returns to the document's viewport and
    // this element gets `blur` and `focusout` with a null relatedTarget. The
    // viewport itself receives no focus event. Blurring an element that is not
    // focused does nothing.
    blur() {
      const doc = this.nodeType === 1 ? this.ownerDocument : null;
      if (!(doc instanceof Document)) return;
      if (focusedElementOf(doc) !== this) return;
      doc.__focusedElementId = null;
      fireFocusEvent(this, "blur", null, false);
      fireFocusEvent(this, "focusout", null, true);
      commitTextControlChange(this);
    }

    // True when this form control is actually disabled, including inherited
    // disabledness from a fieldset (with its first-legend exception).
    __isDisabledControl() {
      const DISABLEABLE_TAGS = ["input", "button", "select", "textarea", "option", "optgroup", "fieldset"];
      if (!DISABLEABLE_TAGS.includes(this.nodeName.toLowerCase())) return false;
      return !!__omoikane_is_actually_disabled(this.__id);
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
        new MouseEvent("click", { bubbles: true, cancelable: true, composed: true })
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
  const SHADOW_HOST_NAMES = new Set([
    "article", "aside", "blockquote", "body", "div", "footer", "h1", "h2",
    "h3", "h4", "h5", "h6", "header", "main", "nav", "p", "section", "span",
  ]);
  const RESERVED_CUSTOM_ELEMENT_NAMES = new Set([
    "annotation-xml", "color-profile", "font-face", "font-face-src",
    "font-face-uri", "font-face-format", "font-face-name", "missing-glyph",
  ]);

  class Element extends Node {
    remove() { removeChildNode.call(this); }

    get slot() { return this.getAttribute("slot") || ""; }
    set slot(value) { this.setAttribute("slot", String(value)); }

    attachShadow(init) {
      if (init === null || init === undefined || init.mode === undefined) {
        throw new TypeError("ShadowRootInit.mode is required");
      }
      const mode = String(init.mode);
      if (mode !== "open" && mode !== "closed") {
        throw new TypeError(mode + " is not a valid ShadowRootMode");
      }
      const name = String(this.localName || "").toLowerCase();
      const customName = name.includes("-") && !RESERVED_CUSTOM_ELEMENT_NAMES.has(name);
      const namespace = this.namespaceURI;
      if ((namespace !== null && namespace !== HTML_NAMESPACE) ||
          (!SHADOW_HOST_NAMES.has(name) && !customName)) {
        throw new DOMException("Element cannot host a shadow tree", "NotSupportedError");
      }
      const root = wrapNode(__omoikane_attach_shadow(this.__id, mode === "closed"));
      if (!root) {
        throw new DOMException("Element already hosts a shadow tree", "NotSupportedError");
      }
      const owner = this.ownerDocument;
      if (owner) stampOwnerDoc(root, owner);
      this.__shadowRootInternal = root;
      return root;
    }

    get shadowRoot() {
      const root = wrapNode(__omoikane_shadow_root(this.__id));
      return root && root.mode === "open" ? root : null;
    }
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
    "scroll", "scrollTo", "scrollBy",
    "__scrollOffset", "__applyScroll", "__isViewportScrollingElement",
  ]);

  class HTMLElement extends Element {
    constructor(id) {
      if (id !== undefined) {
        super(id);
        return;
      }

      const entry = customElementConstructionStack[
        customElementConstructionStack.length - 1
      ];
      if (entry && !entry.constructed) {
        entry.constructed = true;
        super(entry.element.__id);
        return entry.element;
      }

      const definition = new.target === HTMLElement
        ? null
        : customElementDefinitionByConstructor.get(new.target);
      if (definition) {
        const element = wrapNode(__omoikane_create_element(definition.name));
        definition.document.__own(element);
        Object.setPrototypeOf(element, new.target.prototype);
        element.__customElementDefinition = definition;
        element.__customElementState = "custom";
        element.__customElementConnected = false;
        super(element.__id);
        return element;
      }

      super(id);
      throw new TypeError("Illegal constructor");
    }
  }
  class HTMLHtmlElement extends HTMLElement {}
  class HTMLHeadElement extends HTMLElement {}
  class HTMLBodyElement extends HTMLElement {}
  class HTMLDivElement extends HTMLElement {}
  class HTMLSpanElement extends HTMLElement {}
  class HTMLParagraphElement extends HTMLElement {}
  class HTMLAnchorElement extends HTMLElement {}
  const modalDialogsByDocument = new WeakMap();

  function modalDialogStack(doc) {
    let stack = modalDialogsByDocument.get(doc);
    if (!stack) {
      stack = [];
      modalDialogsByDocument.set(doc, stack);
    }
    for (let index = stack.length - 1; index >= 0; index--) {
      const dialog = stack[index];
      if (!dialog.isConnected || !dialog.open || !dialog.__dialogModal) {
        stack.splice(index, 1);
      }
    }
    return stack;
  }

  function focusDialog(dialog) {
    let first = null;
    let autofocus = null;
    function visit(node) {
      for (const child of node.childNodes || []) {
        if (child instanceof Element && canBeFocused(child)) {
          if (!first) first = child;
          if (!autofocus && child.hasAttribute("autofocus")) autofocus = child;
        }
        visit(child);
      }
    }
    visit(dialog);
    const target = dialog.hasAttribute("autofocus") ? dialog : (autofocus || first || dialog);
    if (target === dialog && !canBeFocused(dialog)) {
      dialog.__dialogFocusFallback = true;
      try { dialog.focus(); }
      finally { dialog.__dialogFocusFallback = false; }
    } else {
      target.focus();
    }
  }

  function performDialogEscapeDefault(doc) {
    const stack = modalDialogStack(doc);
    const dialog = stack[stack.length - 1];
    if (!dialog) return false;
    const notCanceled = dialog.dispatchEvent(new Event("cancel", { cancelable: true }));
    if (notCanceled) dialog.close();
    return true;
  }

  class HTMLDialogElement extends HTMLElement {
    get open() { return this.hasAttribute("open"); }
    set open(value) {
      if (value) this.setAttribute("open", "");
      else this.removeAttribute("open");
    }
    get returnValue() {
      return Object.prototype.hasOwnProperty.call(this, "__dialogReturnValue")
        ? this.__dialogReturnValue : "";
    }
    set returnValue(value) { this.__dialogReturnValue = String(value); }
    show() {
      if (this.open) {
        if (this.__dialogModal) {
          throw new DOMException("A modal dialog cannot be shown non-modally", "InvalidStateError");
        }
        return;
      }
      this.__dialogModal = false;
      this.__dialogPreviouslyFocused = focusedElementOf(this.ownerDocument);
      this.open = true;
      focusDialog(this);
    }
    showModal() {
      if (!this.isConnected) {
        throw new DOMException("A modal dialog must be connected", "InvalidStateError");
      }
      if (this.open) {
        if (!this.__dialogModal) {
          throw new DOMException("A non-modal dialog is already open", "InvalidStateError");
        }
        return;
      }
      this.__dialogPreviouslyFocused = focusedElementOf(this.ownerDocument);
      this.__dialogModal = true;
      this.open = true;
      modalDialogStack(this.ownerDocument).push(this);
      focusDialog(this);
    }
    close(result) {
      if (!this.open) return;
      if (arguments.length > 0) this.returnValue = result;
      const doc = this.ownerDocument;
      const stack = modalDialogStack(doc);
      const index = stack.indexOf(this);
      if (index >= 0) stack.splice(index, 1);
      const wasModal = Boolean(this.__dialogModal);
      const focused = focusedElementOf(doc);
      this.open = false;
      this.__dialogModal = false;
      const previous = this.__dialogPreviouslyFocused;
      this.__dialogPreviouslyFocused = null;
      if (previous && canBeFocused(previous) &&
          (wasModal || (focused && this.contains(focused)))) {
        previous.focus();
      }
      this.dispatchEvent(new Event("close"));
    }
  }
  class HTMLStyleElement extends HTMLElement {
    get sheet() { return sheetFor(this); }
  }
  class HTMLTemplateElement extends HTMLElement {
    get content() {
      const fragment = wrapNode(__omoikane_template_content(this.__id));
      const owner = this.ownerDocument;
      if (fragment && owner) {
        stampOwnerDoc(fragment, templateContentsOwnerDocument(owner));
      }
      return fragment;
    }
    get innerHTML() {
      return Object.getOwnPropertyDescriptor(Element.prototype, "innerHTML")
        .get.call(this.content);
    }
    set innerHTML(value) {
      Object.getOwnPropertyDescriptor(Element.prototype, "innerHTML")
        .set.call(this.content, value);
    }
  }
  class HTMLSlotElement extends HTMLElement {
    get name() { return this.getAttribute("name") || ""; }
    set name(value) { this.setAttribute("name", String(value)); }
    assignedNodes(options = {}) {
      const flatten = options != null && !!options.flatten;
      const ids = __omoikane_assigned_nodes(this.__id, flatten);
      return makeNodeList(ids ? ids.map(id => wrapNode(id)) : []);
    }
    assignedElements(options = {}) {
      return this.assignedNodes(options).filter(node => node.nodeType === 1);
    }
  }

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

  distributePrototypeMembers(Node.prototype, [Element.prototype, Text.prototype], [
    "assignedSlot",
  ]);

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
      if (this.__selectionDocument && doc !== this.__selectionDocument) {
        throw new DOMException("The range belongs to another Document.", "WrongDocumentError");
      }
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
      selectionRangeMutated(this);
    }
    setEnd(node, offset) {
      offset = this.__validate(node, offset);
      const doc = nodeDocument(node);
      if (this.__selectionDocument && doc !== this.__selectionDocument) {
        throw new DOMException("The range belongs to another Document.", "WrongDocumentError");
      }
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
      selectionRangeMutated(this);
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
      selectionRangeMutated(this);
    }
    selectNodeContents(node) {
      this.__validate(node, 0);
      this.__startContainer = node; this.__startOffset = 0;
      this.__endContainer = node; this.__endOffset = nodeLength(node);
      selectionRangeMutated(this);
    }
    collapse(toStart = false) {
      if (toStart) { this.__endContainer = this.__startContainer; this.__endOffset = this.__startOffset; }
      else { this.__startContainer = this.__endContainer; this.__startOffset = this.__endOffset; }
      selectionRangeMutated(this);
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
      selectionRangeMutated(this);
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
      selectionRangeMutated(this);
    }
    __mergeText(target, removed, offset, parent, index) {
      const adjust = (container, value) => {
        if (container === removed) return [target, offset + value];
        if (container === parent && value === index) return [target, offset];
        return [container, value];
      };
      [this.__startContainer, this.__startOffset] = adjust(this.__startContainer, this.__startOffset);
      [this.__endContainer, this.__endOffset] = adjust(this.__endContainer, this.__endOffset);
      selectionRangeMutated(this);
    }
    __replaceData(node, offset, count, replacementLength) {
      const adjust = (container, value) => {
        if (container !== node || value <= offset) return [container, value];
        if (value <= offset + count) return [container, offset];
        return [container, value + replacementLength - count];
      };
      [this.__startContainer, this.__startOffset] = adjust(this.__startContainer, this.__startOffset);
      [this.__endContainer, this.__endOffset] = adjust(this.__endContainer, this.__endOffset);
      selectionRangeMutated(this);
    }
    __splitText(oldNode,newNode,offset,parent,index) {
      const adjust=(container,value) => container===oldNode && value>offset ? [newNode,value-offset] : [container,value];
      [this.__startContainer,this.__startOffset]=adjust(this.__startContainer,this.__startOffset);
      [this.__endContainer,this.__endOffset]=adjust(this.__endContainer,this.__endOffset);
      if (parent) {
        if (this.__startContainer===parent && this.__startOffset>index) this.__startOffset++;
        if (this.__endContainer===parent && this.__endOffset>index) this.__endOffset++;
      }
      selectionRangeMutated(this);
    }
  }
  Range.START_TO_START=0; Range.START_TO_END=1; Range.END_TO_END=2; Range.END_TO_START=3;
  Range.prototype.START_TO_START=0; Range.prototype.START_TO_END=1; Range.prototype.END_TO_END=2; Range.prototype.END_TO_START=3;

  // A Selection is maintained per wrapped Document. The native DOM already
  // keeps Range boundary points live through tree mutations; Selection only
  // owns the currently exposed range and derives its anchor/focus from that
  // range. This keeps iframe documents isolated because each contentDocument
  // has its own wrapper identity and WeakMap entry.
  const selectionConstructionToken = {};
  const selectionByDocument = new WeakMap();
  const selectionChangeQueued = new WeakSet();

  function queueSelectionChange(doc) {
    if (!doc || selectionChangeQueued.has(doc)) return;
    selectionChangeQueued.add(doc);
    const deliver = () => {
      selectionChangeQueued.delete(doc);
      doc.dispatchEvent(new Event("selectionchange", { bubbles: true }));
    };
    if (typeof __omoikane_queue_dom_manipulation_task === "function") {
      __omoikane_queue_dom_manipulation_task(deliver);
    } else {
      setTimeout(deliver, 0);
    }
  }

  function selectionRangeMutated(range) {
    if (!range.__selectionDocument) return;
    const selection = selectionByDocument.get(range.__selectionDocument);
    if (selection && selection.__range === range) queueSelectionChange(selection.__doc);
  }

  class Selection {
    constructor(token, doc) {
      if (token !== selectionConstructionToken || !(doc instanceof Document)) {
        throw new TypeError("Illegal constructor");
      }
      this.__doc = doc;
      this.__range = null;
      this.__direction = "forward";
    }

    get anchorNode() {
      if (!this.__range) return null;
      return this.__direction === "backward"
        ? this.__range.endContainer : this.__range.startContainer;
    }
    get anchorOffset() {
      if (!this.__range) return 0;
      return this.__direction === "backward"
        ? this.__range.endOffset : this.__range.startOffset;
    }
    get focusNode() {
      if (!this.__range) return null;
      return this.__direction === "backward"
        ? this.__range.startContainer : this.__range.endContainer;
    }
    get focusOffset() {
      if (!this.__range) return 0;
      return this.__direction === "backward"
        ? this.__range.startOffset : this.__range.endOffset;
    }
    get isCollapsed() { return !this.__range || this.__range.collapsed; }
    get rangeCount() { return this.__range ? 1 : 0; }
    get type() {
      if (!this.__range) return "None";
      return this.__range.collapsed ? "Caret" : "Range";
    }
    getRangeAt(index) {
      if ((Number(index) | 0) !== 0 || !this.__range) {
        throw new DOMException("The index is not in the allowed range.", "IndexSizeError");
      }
      return this.__range;
    }
    addRange(range) {
      if (!(range instanceof Range)) throw new TypeError("Selection.addRange requires a Range");
      if (nodeDocument(range.startContainer) !== this.__doc ||
          nodeDocument(range.endContainer) !== this.__doc) {
        throw new DOMException("The range belongs to another Document.", "WrongDocumentError");
      }
      if (this.__range === range) return;
      this.__range = range;
      range.__selectionDocument = this.__doc;
      this.__direction = "forward";
      queueSelectionChange(this.__doc);
    }
    removeRange(range) {
      if (this.__range !== range) {
        throw new DOMException("The range is not in this Selection.", "NotFoundError");
      }
      this.__range.__selectionDocument = null;
      this.__range = null;
      queueSelectionChange(this.__doc);
    }
    removeAllRanges() {
      if (!this.__range) return;
      this.__range.__selectionDocument = null;
      this.__range = null;
      queueSelectionChange(this.__doc);
    }
    empty() { this.removeAllRanges(); }
    collapse(node, offset = 0) {
      if (node === null || node === undefined) {
        this.removeAllRanges();
        return;
      }
      if (!(node instanceof Node) || nodeDocument(node) !== this.__doc) {
        throw new DOMException("The node belongs to another Document.", "WrongDocumentError");
      }
      const range = this.__range || new Range(this.__doc);
      range.setStart(node, offset);
      range.setEnd(node, offset);
      this.__range = range;
      range.__selectionDocument = this.__doc;
      this.__direction = "forward";
      queueSelectionChange(this.__doc);
    }
    collapseToStart() {
      if (!this.__range) throw new DOMException("The Selection is empty.", "InvalidStateError");
      this.collapse(this.__range.startContainer, this.__range.startOffset);
    }
    collapseToEnd() {
      if (!this.__range) throw new DOMException("The Selection is empty.", "InvalidStateError");
      this.collapse(this.__range.endContainer, this.__range.endOffset);
    }
    selectAllChildren(node) {
      if (!(node instanceof Node) || nodeDocument(node) !== this.__doc) {
        throw new DOMException("The node belongs to another Document.", "WrongDocumentError");
      }
      const range = this.__range || new Range(this.__doc);
      range.selectNodeContents(node);
      this.__range = range;
      range.__selectionDocument = this.__doc;
      this.__direction = "forward";
      queueSelectionChange(this.__doc);
    }
    setBaseAndExtent(anchorNode, anchorOffset, focusNode, focusOffset) {
      if (!(anchorNode instanceof Node) || !(focusNode instanceof Node) ||
          nodeDocument(anchorNode) !== this.__doc || nodeDocument(focusNode) !== this.__doc) {
        throw new DOMException("The node belongs to another Document.", "WrongDocumentError");
      }
      const range = this.__range || new Range(this.__doc);
      const order = boundaryCompare(anchorNode, Number(anchorOffset) >>> 0, focusNode, Number(focusOffset) >>> 0);
      if (order <= 0) {
        range.setStart(anchorNode, anchorOffset);
        range.setEnd(focusNode, focusOffset);
        this.__direction = "forward";
      } else {
        range.setStart(focusNode, focusOffset);
        range.setEnd(anchorNode, anchorOffset);
        this.__direction = "backward";
      }
      this.__range = range;
      range.__selectionDocument = this.__doc;
      queueSelectionChange(this.__doc);
    }
    extend(node, offset = 0) {
      if (!this.__range) return;
      if (!(node instanceof Node) || nodeDocument(node) !== this.__doc) {
        throw new DOMException("The node belongs to another Document.", "WrongDocumentError");
      }
      const anchorNode = this.anchorNode;
      const anchorOffset = this.anchorOffset;
      const order = boundaryCompare(anchorNode, anchorOffset, node, Number(offset) >>> 0);
      if (order <= 0) {
        this.__range.setStart(anchorNode, anchorOffset);
        this.__range.setEnd(node, offset);
        this.__direction = "forward";
      } else {
        this.__range.setStart(node, offset);
        this.__range.setEnd(anchorNode, anchorOffset);
        this.__direction = "backward";
      }
      queueSelectionChange(this.__doc);
    }
    containsNode(node, allowPartialContainment = false) {
      if (!(node instanceof Node) || !this.__range || nodeDocument(node) !== this.__doc) return false;
      if (!node.parentNode) return false;
      const parent = node.parentNode;
      const start = [parent, indexOfNode(node)];
      const end = [parent, indexOfNode(node) + 1];
      const afterStart = boundaryCompare(start[0], start[1], this.__range.startContainer, this.__range.startOffset) >= 0;
      const beforeEnd = boundaryCompare(end[0], end[1], this.__range.endContainer, this.__range.endOffset) <= 0;
      if (allowPartialContainment) {
        return boundaryCompare(end[0], end[1], this.__range.startContainer, this.__range.startOffset) > 0 &&
          boundaryCompare(start[0], start[1], this.__range.endContainer, this.__range.endOffset) < 0;
      }
      return afterStart && beforeEnd;
    }
    deleteFromDocument() {
      if (!this.__range) return;
      this.__range.deleteContents();
    }
    toString() { return this.__range ? this.__range.toString() : ""; }
  }

  function selectionForDocument(doc) {
    if (!(doc instanceof Document)) return null;
    let selection = selectionByDocument.get(doc);
    if (!selection) {
      selection = new Selection(selectionConstructionToken, doc);
      selectionByDocument.set(doc, selection);
    }
    return selection;
  }

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

  function findElementById(root, id) {
    const expected = String(id);
    const visit = (node) => {
      for (const child of node.childNodes) {
        if (child.nodeType !== 1) continue;
        if (child.getAttribute("id") === expected) return child;
        const found = visit(child);
        if (found) return found;
      }
      return null;
    };
    return visit(root);
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
      // by falling back to the top-level document when `this` is not a
      // Document instance.
      const scope = this instanceof Document ? this : globalThis.document;
      // Plain tree walk with an id equality check: getElementById needs no
      // selector parsing/matching and no full-document snapshot.
      return findElementById(scope, id);
    }

    createElement(tag) {
      const name = String(tag);
      if (!isValidXmlName(name)) {
        throw new DOMException(
          "The tag name provided ('" + name + "') is not a valid name.",
          "InvalidCharacterError"
        );
      }
      const element = this.__own(wrapNode(__omoikane_create_element(name)));
      const registry = customElementRegistryByDocument.get(this);
      if (registry) considerCustomElement(registry, element);
      return element;
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
      if (info.namespace === HTML_NAMESPACE) {
        const registry = customElementRegistryByDocument.get(this);
        if (registry) considerCustomElement(registry, node);
      }
      return node;
    }

    importNode(node, deep = false) {
      if (!(node instanceof Node)) {
        throw new TypeError("Document.importNode requires a Node");
      }
      if (node.nodeType === 9) {
        throw new DOMException("Documents cannot be imported.", "NotSupportedError");
      }
      const clone = node.cloneNode(!!deep);
      stampOwnerDoc(clone, this);
      return clone;
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

    get adoptedStyleSheets() {
      return adoptedStyleSheetsForRoot(this);
    }
    set adoptedStyleSheets(value) {
      setAdoptedStyleSheets(this, value);
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

    getSelection() { return selectionForDocument(this); }

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

    // The element focused in this document. With nothing focused — on load,
    // after `blur()`, or once the focused element left the document — this is
    // the viewport's stand-in: the body element, or the document element when
    // there is no body.
    get activeElement() {
      const focused = focusedElementOf(this);
      if (focused) return focused;
      return this.body || this.documentElement || null;
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
      const head = this.head;
      const title = head ? head.childNodes.find(node =>
        node.nodeType === 1 && (node.localName || "").toLowerCase() === "title") : null;
      return title ? title.textContent : "";
    }

    set title(value) {
      let head = this.head;
      let title = head ? head.childNodes.find(node =>
        node.nodeType === 1 && (node.localName || "").toLowerCase() === "title") : null;
      if (!title) {
        if (!head) {
          head = this.createElement("head");
          const root = this.documentElement;
          if (!root) return;
          root.insertBefore(head, root.firstChild);
        }
        title = this.createElement("title");
        head.appendChild(title);
      }
      title.textContent = String(value);
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

    set location(value) {
      globalThis.location = value;
    }

    get referrer() {
      return "";
    }

    get contentType() {
      return "text/html";
    }

    // Omoikane's enforced CSP core exposes a stable per-Document snapshot for
    // diagnostics and focused conformance tests.  It is deliberately not a
    // mutable policy object: navigation installs a fresh Document/global.
    get cspViolations() {
      try {
        const violations = JSON.parse(__omoikane_csp_violations(this.__id));
        return violations.map(violation => ({
          ...violation,
          effectiveDirective: violation.effective_directive,
          blockedURI: violation.blocked_uri,
          resourceType: violation.resource_type,
        }));
      } catch (_) {
        return [];
      }
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

    // Whether this document holds system focus. The headless window is always
    // focused, so this is true for the document containing the focused element
    // and for its ancestor documents, and false for every other document —
    // including one created by `createHTMLDocument`, which has no browsing
    // context at all.
    hasFocus() {
      return focusChainDocuments().includes(this);
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

  class DocumentFragment extends Node {
    getElementById(id) { return findElementById(this, id); }
  }

  class ShadowRoot extends DocumentFragment {
    constructor(id, construction) {
      if (construction !== SHADOW_ROOT_CONSTRUCTION) {
        throw new TypeError("Illegal constructor");
      }
      super(id);
    }
    get host() { return wrapNode(__omoikane_shadow_host(this.__id)); }
    get mode() { return __omoikane_shadow_mode(this.__id); }
    get delegatesFocus() { return false; }
    get adoptedStyleSheets() {
      return adoptedStyleSheetsForRoot(this);
    }
    set adoptedStyleSheets(value) {
      setAdoptedStyleSheets(this, value);
    }
    get innerHTML() {
      return Object.getOwnPropertyDescriptor(Element.prototype, "innerHTML")
        .get.call(this);
    }
    set innerHTML(value) {
      Object.getOwnPropertyDescriptor(Element.prototype, "innerHTML")
        .set.call(this, value);
    }
    cloneNode() {
      throw new DOMException("ShadowRoot cannot be cloned", "NotSupportedError");
    }
  }

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
    let start = 0, depth = 0, parenDepth = 0, bracketDepth = 0;
    let quote = "", comment = false;
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
      if (ch === "(") parenDepth++;
      else if (ch === ")") parenDepth = Math.max(0, parenDepth - 1);
      else if (ch === "[") bracketDepth++;
      else if (ch === "]") bracketDepth = Math.max(0, bracketDepth - 1);
      else if (ch === "{" && parenDepth === 0 && bracketDepth === 0) depth++;
      else if (ch === "}" && parenDepth === 0 && bracketDepth === 0) {
        depth--;
        if (depth === 0) {
          const text = css.slice(start, i + 1).trim();
          if (text) rules.push(text);
          start = i + 1;
        }
      } else if (ch === ";" && depth === 0 && parenDepth === 0 && bracketDepth === 0) {
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

  function scopeRulesValid(source) {
    try { return __omoikane_css_scope_rules_valid(String(source)); }
    catch (_) { return false; }
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

  function cssRuleBlockStart(source) {
    let parenDepth = 0, bracketDepth = 0, quote = "", comment = false;
    for (let index = 0; index < source.length; index++) {
      const ch = source[index], next = source[index + 1];
      if (comment) {
        if (ch === "*" && next === "/") { comment = false; index++; }
        continue;
      }
      if (quote) {
        if (ch === "\\") index++;
        else if (ch === quote) quote = "";
        continue;
      }
      if (ch === "/" && next === "*") { comment = true; index++; continue; }
      if (ch === "'" || ch === '"') { quote = ch; continue; }
      if (ch === "(") parenDepth++;
      else if (ch === ")") parenDepth = Math.max(0, parenDepth - 1);
      else if (ch === "[") bracketDepth++;
      else if (ch === "]") bracketDepth = Math.max(0, bracketDepth - 1);
      else if (ch === "{" && parenDepth === 0 && bracketDepth === 0) return index;
    }
    return -1;
  }

  class CSSStyleRule {
    constructor(text, sheet = null, index = -1) {
      this.__text = text;
      this.__sheet = sheet;
      this.__index = index;
      const open = cssRuleBlockStart(this.__text);
      const close = this.__text.lastIndexOf("}");
      this.__hasBlock = open >= 0 && close > open;
      this.__selectorText = this.__hasBlock ? this.__text.slice(0, open).trim() : "";
      this.__style = declarationView(this.__hasBlock ? this.__text.slice(open + 1, close) : "");
      if (this.__sheet) this.__sheet.__registerRuleView(this);
    }
    get selectorText() { return this.__selectorText; }
    set selectorText(value) {
      if (!this.__hasBlock) return;
      const selector = String(value).trim();
      let count;
      try { count = __omoikane_css_rule_count(selector + " {}"); }
      catch (_error) { return; }
      if (count !== 1) return;
      this.__selectorText = selector;
      this.__text = selector + " { " + this.style.cssText + " }";
      if (this.__sheet) this.__sheet.__replaceRule(this.__index, this.__text);
    }
    get cssText() {
      return this.__hasBlock
        ? this.selectorText + " { " + this.style.cssText + " }"
        : this.__text.trim();
    }
    get style() { return this.__style; }
  }

  class CSSGroupingRule {
    // Grouping-rule CSSOM mutations operate on a live child stylesheet.  Keep
    // that child linked to its owning rule so edits are reflected in the
    // containing stylesheet (and therefore in native style resolution) rather
    // than being stranded in a detached CSSRuleList snapshot.
    insertRule(rule, index) {
      if (!this.__innerSheet) {
        throw new DOMException("The rule has no child rule list.", "InvalidStateError");
      }
      return this.__innerSheet.insertRule(rule, index);
    }
    deleteRule(index) {
      if (!this.__innerSheet) {
        throw new DOMException("The rule has no child rule list.", "InvalidStateError");
      }
      return this.__innerSheet.deleteRule(index);
    }
    __syncFromInner() {
      if (!this.__sheet || this.__index < 0 || !this.__innerSheet ||
          typeof this.__serializeCssText !== "function") return;
      this.__text = this.__serializeCssText();
      this.__sheet.__replaceRule(this.__index, this.__text);
    }
  }
  class CSSConditionRule extends CSSGroupingRule {}

  class CSSSupportsRule extends CSSConditionRule {
    constructor(text, sheet = null, index = -1) {
      super();
      this.__text = text;
      this.__sheet = sheet;
      this.__index = index;
      const open = cssRuleBlockStart(this.__text);
      const close = this.__text.lastIndexOf("}");
      this.__hasBlock = open >= 0 && close > open;
      this.__conditionText = this.__hasBlock
        ? this.__text.slice("@supports".length, open).trim()
        : "";
      this.__innerText = this.__hasBlock ? this.__text.slice(open + 1, close) : "";
      this.__innerSheet = new CSSStyleSheet({ textContent: this.__innerText });
      this.__innerSheet.__parentRule = this;
      if (this.__sheet) this.__sheet.__registerRuleView(this);
    }
    get conditionText() { return this.__conditionText; }
    get matches() { return CSS.supports(this.conditionText); }
    get cssRules() { return this.__innerSheet.cssRules; }
    __serializeCssText() {
      const nested = Array.from(this.cssRules, rule => "  " + rule.cssText).join("\n");
      return "@supports" + (this.conditionText ? " " + this.conditionText : "") +
        " {\n" + (nested ? nested + "\n" : "") + "}";
    }
    get cssText() { return this.__serializeCssText(); }
  }

  class CSSContainerRule extends CSSConditionRule {
    constructor(text, sheet = null, index = -1) {
      super();
      this.__text = text;
      this.__sheet = sheet;
      this.__index = index;
      const open = cssRuleBlockStart(this.__text);
      const close = this.__text.lastIndexOf("}");
      this.__hasBlock = open >= 0 && close > open;
      const prelude = (this.__hasBlock
        ? this.__text.slice("@container".length, open)
        : "").replace(/\/\*[\s\S]*?\*\//g, " ").trim();
      const conditionStart = prelude.indexOf("(");
      const prefix = conditionStart >= 0 ? prelude.slice(0, conditionStart).trim() : "";
      this.__containerName = prefix.toLowerCase() === "not" ? "" : prefix;
      this.__containerQuery = conditionStart < 0 ? "" :
        (this.__containerName ? prelude.slice(conditionStart) : prelude);
      this.__innerText = this.__hasBlock ? this.__text.slice(open + 1, close) : "";
      this.__innerSheet = new CSSStyleSheet({ textContent: this.__innerText });
      this.__innerSheet.__parentRule = this;
      if (this.__sheet) this.__sheet.__registerRuleView(this);
    }
    get containerName() { return this.__containerName; }
    get containerQuery() { return this.__containerQuery; }
    get conditionText() {
      return this.containerName
        ? this.containerName + (this.containerQuery ? " " + this.containerQuery : "")
        : this.containerQuery;
    }
    get cssRules() { return this.__innerSheet.cssRules; }
    __serializeCssText() {
      const name = this.containerName ? " " + this.containerName : "";
      const query = this.containerQuery ? " " + this.containerQuery : "";
      const nested = Array.from(this.cssRules, rule => "  " + rule.cssText).join("\n");
      return "@container" + name + query + " {\n" + (nested ? nested + "\n" : "") + "}";
    }
    get cssText() { return this.__serializeCssText(); }
  }

  function scopeBoundaryTexts(prelude) {
    let index = 0;
    const skipWhitespace = () => {
      while (index < prelude.length) {
        while (/\s/.test(prelude[index] || "")) index++;
        if (prelude[index] !== "/" || prelude[index + 1] !== "*") break;
        const close = prelude.indexOf("*/", index + 2);
        index = close < 0 ? prelude.length : close + 2;
      }
    };
    const boundary = () => {
      skipWhitespace();
      if (prelude[index] !== "(") return null;
      const start = ++index;
      let depth = 1, quote = "";
      for (; index < prelude.length; index++) {
        const ch = prelude[index];
        if (quote) {
          if (ch === "\\") index++;
          else if (ch === quote) quote = "";
          continue;
        }
        if (ch === "'" || ch === '"') { quote = ch; continue; }
        if (ch === "(") depth++;
        else if (ch === ")" && --depth === 0) {
          const value = prelude.slice(start, index).trim();
          index++;
          return value;
        }
      }
      return null;
    };
    skipWhitespace();
    const start = prelude[index] === "(" ? boundary() : null;
    skipWhitespace();
    let end = null;
    const afterTo = prelude[index + 2];
    const hasToBoundary = afterTo === undefined || /[\s(]/.test(afterTo) ||
      (afterTo === "/" && prelude[index + 3] === "*");
    if (prelude.slice(index, index + 2).toLowerCase() === "to" && hasToBoundary) {
      index += 2;
      end = boundary();
    }
    return { start, end };
  }

  class CSSScopeRule extends CSSGroupingRule {
    constructor(text, sheet = null, index = -1) {
      super();
      this.__text = text;
      this.__sheet = sheet;
      this.__index = index;
      const open = cssRuleBlockStart(this.__text);
      const close = this.__text.lastIndexOf("}");
      this.__hasBlock = open >= 0 && close > open;
      const prelude = this.__hasBlock
        ? this.__text.slice("@scope".length, open).trim()
        : "";
      const boundaries = scopeBoundaryTexts(prelude);
      this.__start = boundaries.start;
      this.__end = boundaries.end;
      this.__innerText = this.__hasBlock ? this.__text.slice(open + 1, close) : "";
      this.__innerSheet = new CSSStyleSheet({ textContent: this.__innerText });
      this.__innerSheet.__parentRule = this;
      if (this.__sheet) this.__sheet.__registerRuleView(this);
    }
    get start() { return this.__start; }
    get end() { return this.__end; }
    get cssRules() { return this.__innerSheet.cssRules; }
    __serializeCssText() {
      let prelude = "@scope";
      if (this.start !== null) prelude += " (" + this.start + ")";
      if (this.end !== null) prelude += " to (" + this.end + ")";
      const nested = Array.from(this.cssRules, rule => "  " + rule.cssText).join("\n");
      return prelude + " {\n" + (nested ? nested + "\n" : "") + "}";
    }
    get cssText() { return this.__serializeCssText(); }
  }

  function createCssRule(text, sheet = null, index = -1) {
    return /^\s*@container(?=\s|\/\*|\()/i.test(text)
      ? new CSSContainerRule(text, sheet, index)
      : /^\s*@scope(?=\s|\/\*|\(|\{)/i.test(text)
      ? new CSSScopeRule(text, sheet, index)
      : /^\s*@supports(?=\s|\/\*|\()/i.test(text)
      ? new CSSSupportsRule(text, sheet, index)
      : new CSSStyleRule(text, sheet, index);
  }

  class CSSRuleList {
    constructor(sheet) { this.__sheet = sheet; }
    __rules() {
      return this.__sheet.__ruleTexts().map(
        (text, index) => createCssRule(text, this.__sheet, index)
      );
    }
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

  const dirtyStyleSheets = new Set();

  // Constructable stylesheets are kept as JavaScript objects while native
  // style resolution receives a per-root text snapshot through a small host
  // hook. Weak references avoid keeping detached ShadowRoots alive solely
  // because a stylesheet was once adopted by them.
  const adoptedStyleSheetsByRoot = new WeakMap();
  const adoptedRootsBySheet = new WeakMap();
  const adoptedListReplacers = new WeakMap();
  const ADOPTED_LIST_MUTATORS = [
    "copyWithin", "fill", "pop", "push", "reverse", "shift", "sort", "splice", "unshift",
  ];

  function adoptedRootDocument(root) {
    if (root instanceof Document) return root;
    if (root instanceof ShadowRoot) return root.host ? root.host.ownerDocument : null;
    return null;
  }

  function adoptedRefTarget(ref) {
    return typeof WeakRef === "function" && ref instanceof WeakRef ? ref.deref() : ref;
  }

  function adoptedRootsForSheet(sheet) {
    const refs = adoptedRootsBySheet.get(sheet);
    if (!refs) return [];
    const roots = [];
    for (const ref of Array.from(refs)) {
      const root = adoptedRefTarget(ref);
      if (root) roots.push(root);
      else refs.delete(ref);
    }
    return roots;
  }

  function detachAdoptedRootFromSheet(root, sheet) {
    const refs = adoptedRootsBySheet.get(sheet);
    if (!refs) return;
    for (const ref of Array.from(refs)) {
      const target = adoptedRefTarget(ref);
      if (!target || target === root) refs.delete(ref);
    }
  }

  function attachAdoptedRootToSheet(root, sheet) {
    let refs = adoptedRootsBySheet.get(sheet);
    if (!refs) {
      refs = new Set();
      adoptedRootsBySheet.set(sheet, refs);
    }
    if (!adoptedRootsForSheet(sheet).includes(root)) {
      refs.add(typeof WeakRef === "function" ? new WeakRef(root) : root);
    }
  }

  function validateAdoptedStyleSheetValues(root, value) {
    const document = adoptedRootDocument(root);
    if (!document) throw new DOMException("The root has no owner Document.", "NotAllowedError");
    for (const sheet of value) {
      if (!(sheet instanceof CSSStyleSheet)) {
        throw new TypeError("adoptedStyleSheets entries must be CSSStyleSheet objects");
      }
      if (!sheet.__constructed || sheet.__ownerDocument !== document) {
        throw new DOMException("The stylesheet cannot be adopted by this root.", "NotAllowedError");
      }
    }
  }

  function observableArrayIndex(property) {
    if (typeof property !== "string" || !/^(?:0|[1-9]\d*)$/.test(property)) return null;
    const index = Number(property);
    return index <= 0xffffffff - 1 ? index : null;
  }

  function makeAdoptedStyleSheetList(root, values = []) {
    const list = values.slice();
    let observable;

    const commit = candidate => {
      validateAdoptedStyleSheetValues(root, candidate);
      const previous = list.slice();
      list.length = 0;
      for (const sheet of candidate) list.push(sheet);
      try {
        syncAdoptedStyleSheetRoot(root, observable);
      } catch (error) {
        list.length = 0;
        for (const sheet of previous) list.push(sheet);
        throw error;
      }
    };

    const mutate = (method, args) => {
      const candidate = list.slice();
      const result = Array.prototype[method].apply(candidate, args);
      commit(candidate);
      return result === candidate ? observable : result;
    };

    observable = new Proxy(list, {
      get(target, property, receiver) {
        if (ADOPTED_LIST_MUTATORS.includes(property)) {
          return (...args) => mutate(property, args);
        }
        return Reflect.get(target, property, receiver);
      },
      set(target, property, value, receiver) {
        if (property === "length") {
          const length = Number(value);
          if (!Number.isInteger(length) || length < 0 || length > target.length) {
            return false;
          }
          return commit(target.slice(0, length)), true;
        }
        const index = observableArrayIndex(property);
        if (index !== null) {
          if (index > target.length) return false;
          const candidate = target.slice();
          candidate[index] = value;
          commit(candidate);
          return true;
        }
        return Reflect.set(target, property, value, receiver);
      },
      defineProperty(target, property, descriptor) {
        const index = observableArrayIndex(property);
        if (index !== null) {
          if (descriptor.get || descriptor.set || descriptor.configurable === false ||
              descriptor.enumerable === false || descriptor.writable === false) {
            return false;
          }
          if (Object.prototype.hasOwnProperty.call(descriptor, "value")) {
            if (index > target.length) return false;
            const candidate = target.slice();
            candidate[index] = descriptor.value;
            commit(candidate);
          }
          return true;
        }
        if (property === "length" && Object.prototype.hasOwnProperty.call(descriptor, "value")) {
          if (descriptor.writable === false) return false;
          const length = Number(descriptor.value);
          if (!Number.isInteger(length) || length < 0 || length > target.length) return false;
          commit(target.slice(0, length));
          return true;
        }
        return Reflect.defineProperty(target, property, descriptor);
      },
      deleteProperty(target, property) {
        const index = observableArrayIndex(property);
        if (index === null) return Reflect.deleteProperty(target, property);
        if (index >= target.length) return true;
        const candidate = target.slice();
        candidate.splice(index, 1);
        commit(candidate);
        return true;
      },
      preventExtensions() {
        return false;
      },
    });
    adoptedListReplacers.set(observable, candidate => commit(candidate));
    if (values.length) commit(values.slice());
    return observable;
  }

  function syncAdoptedStyleSheetRoot(root, value) {
    validateAdoptedStyleSheetValues(root, value);
    const cssText = JSON.stringify(value.map(sheet => sheet.__cssText()));
    // Keep the JS bookkeeping unchanged if the native root has already been
    // detached or otherwise rejects the update.
    __omoikane_set_adopted_stylesheets(root.__id, cssText);
    const previous = adoptedStyleSheetsByRoot.get(root) || [];
    for (const sheet of previous) detachAdoptedRootFromSheet(root, sheet);
    adoptedStyleSheetsByRoot.set(root, value);
    for (const sheet of value) attachAdoptedRootToSheet(root, sheet);
  }

  function adoptedStyleSheetsForRoot(root) {
    let value = adoptedStyleSheetsByRoot.get(root);
    if (!value) {
      value = makeAdoptedStyleSheetList(root);
      adoptedStyleSheetsByRoot.set(root, value);
    }
    return value;
  }

  function setAdoptedStyleSheets(root, value) {
    if (value === null || value === undefined ||
        (typeof value !== "object" && typeof value !== "function")) {
      throw new TypeError("adoptedStyleSheets must be an iterable or array-like object");
    }
    const values = Array.from(value);
    const list = adoptedStyleSheetsForRoot(root);
    // The ObservableArray setter replaces the backing list, preserving the
    // object returned by earlier getter calls. `commit` validates and rolls
    // back the list itself if the native update fails.
    adoptedListReplacers.get(list)(values.slice());
  }

  class CSSStyleSheet {
    constructor(ownerNode) {
      this.__constructed = !ownerNode || !ownerNode.nodeType;
      this.ownerNode = this.__constructed ? null : ownerNode;
      this.__ownerDocument = this.ownerNode ? nodeDocument(this.ownerNode) : globalThis.document;
      this.href = null;
      const ownerText = ownerNode && typeof ownerNode.textContent === "string"
        ? ownerNode.textContent : "";
      this.__rules = splitCssRules(ownerText);
      this.__ownerText = this.ownerNode ? this.ownerNode.textContent : ownerText;
      this.__ruleViews = new Set();
      this.__cssRules = ruleListProxy(this);
      this.__parentRule = null;
    }
    __syncFromOwner() {
      if (!this.ownerNode) return;
      if (dirtyStyleSheets.has(this)) return;
      const text = this.ownerNode.textContent;
      if (text !== this.__ownerText) {
        for (const rule of this.__ruleViews) {
          rule.__sheet = null;
          rule.__index = -1;
        }
        this.__ruleViews.clear();
        this.__rules = splitCssRules(text);
        this.__ownerText = text;
      }
    }
    __ruleTexts() {
      this.__syncFromOwner();
      return this.__rules;
    }
    __cssText() {
      this.__syncFromOwner();
      return this.__rules.join("\n");
    }
    __syncAdoptedRoots() {
      for (const root of adoptedRootsForSheet(this)) {
        const list = adoptedStyleSheetsByRoot.get(root);
        if (list) syncAdoptedStyleSheetRoot(root, list);
      }
    }
    __markDirty() {
      if (this.__constructed) this.__syncAdoptedRoots();
      else dirtyStyleSheets.add(this);
      if (this.__parentRule) this.__parentRule.__syncFromInner();
    }
    __registerRuleView(rule) { this.__ruleViews.add(rule); }
    __shiftRuleViewsForInsert(index) {
      for (const rule of this.__ruleViews) {
        if (rule.__index >= index) rule.__index++;
      }
    }
    __shiftRuleViewsForDelete(index) {
      for (const rule of Array.from(this.__ruleViews)) {
        if (rule.__index === index) {
          rule.__sheet = null;
          rule.__index = -1;
          this.__ruleViews.delete(rule);
        } else if (rule.__index > index) {
          rule.__index--;
        }
      }
    }
    __replaceRule(index, text) {
      const rules = this.__ruleTexts();
      if (index < 0 || index >= rules.length) return;
      rules[index] = text;
      this.__markDirty();
    }
    __flush() {
      if (!dirtyStyleSheets.delete(this)) return;
      if (!this.ownerNode) return;
      const text = this.__rules.join("\n");
      this.ownerNode.textContent = text;
      this.__ownerText = text;
    }
    get cssRules() { return this.__cssRules; }
    get rules() { return this.__cssRules; }
    insertRule(rule, index) {
      if (this.__replacing) {
        throw new DOMException("The stylesheet is being replaced.", "NotAllowedError");
      }
      const text = String(rule);
      let count;
      try { count = __omoikane_css_rule_count(text); }
      catch (error) { throw new DOMException(error.message || "Invalid CSS rule.", "SyntaxError"); }
      if (count !== 1) throw new DOMException("Exactly one rule is required.", "SyntaxError");
      if (!scopeRulesValid(text)) {
        throw new DOMException("Invalid @scope prelude.", "SyntaxError");
      }
      const rules = this.__ruleTexts();
      const position = index === undefined ? 0 : Number(index);
      if (!Number.isInteger(position) || position < 0 || position > rules.length)
        throw new DOMException("The index is out of range.", "IndexSizeError");
      this.__shiftRuleViewsForInsert(position);
      rules.splice(position, 0, text.trim());
      this.__markDirty();
      return position;
    }
    deleteRule(index) {
      if (this.__replacing) {
        throw new DOMException("The stylesheet is being replaced.", "NotAllowedError");
      }
      const rules = this.__ruleTexts();
      const position = Number(index);
      if (!Number.isInteger(position) || position < 0 || position >= rules.length)
        throw new DOMException("The index is out of range.", "IndexSizeError");
      rules.splice(position, 1);
      this.__shiftRuleViewsForDelete(position);
      this.__markDirty();
    }
    replaceSync(text) {
      if (!this.__constructed || this.__replacing) {
        throw new DOMException("Only constructed stylesheets can be replaced.", "NotAllowedError");
      }
      if (!scopeRulesValid(text)) {
        throw new DOMException("Invalid @scope prelude.", "SyntaxError");
      }
      this.__rules = splitCssRules(String(text));
      for (const rule of this.__ruleViews) {
        rule.__sheet = null;
        rule.__index = -1;
      }
      this.__ruleViews.clear();
      this.__markDirty();
    }
    replace(text) {
      if (!this.__constructed || this.__replacing) {
        return Promise.reject(new DOMException(
          "Only constructed stylesheets can be replaced.", "NotAllowedError"
        ));
      }
      if (!scopeRulesValid(text)) {
        return Promise.reject(new DOMException("Invalid @scope prelude.", "SyntaxError"));
      }
      this.__replacing = true;
      return Promise.resolve().then(() => {
        try {
          this.__rules = splitCssRules(String(text));
          for (const rule of this.__ruleViews) {
            rule.__sheet = null;
            rule.__index = -1;
          }
          this.__ruleViews.clear();
          this.__markDirty();
          return this;
        } finally {
          this.__replacing = false;
        }
      }, error => {
        this.__replacing = false;
        throw error;
      });
    }
  }

  function flushStyleSheets() {
    for (const sheet of Array.from(dirtyStyleSheets)) sheet.__flush();
  }
  globalThis.__omoikane_flush_stylesheets = flushStyleSheets;

  // Minimal CSS namespace used for feature detection and selector escaping.
  // Unsupported declarations conservatively report false so sites choose their
  // fallback path instead of aborting while probing browser capabilities.
  globalThis.CSS = {
    escape(value) {
      const input = String(value);
      let output = "";
      for (let index = 0; index < input.length; index++) {
        const code = input.charCodeAt(index);
        if (code === 0) { output += "\uFFFD"; continue; }
        if ((code >= 1 && code <= 31) || code === 127 ||
            (index === 0 && code >= 48 && code <= 57) ||
            (index === 1 && code >= 48 && code <= 57 && input.charCodeAt(0) === 45)) {
          output += "\\" + code.toString(16) + " ";
          continue;
        }
        if (index === 0 && code === 45 && input.length === 1) { output += "\\-"; continue; }
        if (code >= 128 || code === 45 || code === 95 ||
            (code >= 48 && code <= 57) || (code >= 65 && code <= 90) ||
            (code >= 97 && code <= 122)) output += input[index];
        else output += "\\" + input[index];
      }
      return output;
    },
    supports(propertyOrCondition, value) {
      if (arguments.length >= 2) {
        return __omoikane_css_supports(String(propertyOrCondition), String(value));
      }
      return __omoikane_css_supports_condition(String(propertyOrCondition));
    },
  };

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
          __listeners: new Map(),
          get document() {
            return iframe.contentDocument;
          },
          get customElements() {
            return registryForDocument(iframe.contentDocument);
          },
          get localStorage() {
            return storageForDocument("local", iframe.contentDocument, this);
          },
          get sessionStorage() {
            return storageForDocument("session", iframe.contentDocument, this);
          },
          frameElement: iframe,
          getComputedStyle: globalThis.getComputedStyle,
          addEventListener(type, listener, options) {
            return Node.prototype.addEventListener.call(this, type, listener, options);
          },
          removeEventListener(type, listener, options) {
            return Node.prototype.removeEventListener.call(this, type, listener, options);
          },
          dispatchEvent(event) {
            return dispatchEventOnTarget(this, event);
          },
        };
        windowObjects.add(this.__contentWindowFacade);
        registerBrowsingWindow(this.__contentWindowFacade);
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

  // ── HTML media element playback state ──────────────────────────────────────
  // The embedding deliberately has no audio/video decoder.  These classes
  // nevertheless model the observable HTMLMediaElement state machine: source
  // selection and fetch failures, metadata readiness, Promise-based play(),
  // pause/seek transitions, and task-queued media events.  A successful source
  // uses a short synthetic duration so playback can advance deterministically
  // under the virtual timer used by the runtime.
  const MEDIA_HAVE_NOTHING = 0;
  const MEDIA_HAVE_METADATA = 1;
  const MEDIA_HAVE_CURRENT_DATA = 2;
  const MEDIA_HAVE_FUTURE_DATA = 3;
  const MEDIA_HAVE_ENOUGH_DATA = 4;
  const MEDIA_NETWORK_EMPTY = 0;
  const MEDIA_NETWORK_IDLE = 1;
  const MEDIA_NETWORK_LOADING = 2;
  const MEDIA_NETWORK_NO_SOURCE = 3;
  const MEDIA_TYPE_BY_EXTENSION = new Map([
    ["mp3", "audio/mpeg"], ["m4a", "audio/mp4"], ["aac", "audio/aac"],
    ["wav", "audio/wav"], ["oga", "audio/ogg"], ["ogg", "audio/ogg"],
    ["mp4", "video/mp4"], ["m4v", "video/mp4"], ["webm", "video/webm"],
    ["ogv", "video/ogg"],
  ]);
  const MEDIA_AUDIO_TYPES = new Set([
    "audio/aac", "audio/flac", "audio/m4a", "audio/mp4", "audio/mpeg",
    "audio/ogg", "audio/wav", "audio/wave", "audio/x-wav",
  ]);
  const MEDIA_VIDEO_TYPES = new Set([
    "video/mp4", "video/ogg", "video/webm", "video/mpeg",
  ]);

  const mediaErrorConstructionToken = {};
  class MediaError {
    constructor(token, code, message = "") {
      if (token !== mediaErrorConstructionToken) throw new TypeError("Illegal constructor");
      this.code = Number(code);
      this.message = String(message);
    }
    get [Symbol.toStringTag]() { return "MediaError"; }
  }
  MediaError.MEDIA_ERR_ABORTED = 1;
  MediaError.MEDIA_ERR_NETWORK = 2;
  MediaError.MEDIA_ERR_DECODE = 3;
  MediaError.MEDIA_ERR_SRC_NOT_SUPPORTED = 4;
  for (const name of [
    "MEDIA_ERR_ABORTED", "MEDIA_ERR_NETWORK", "MEDIA_ERR_DECODE", "MEDIA_ERR_SRC_NOT_SUPPORTED",
  ]) MediaError.prototype[name] = MediaError[name];

  function mediaTypeToken(value) {
    return String(value || "").split(";", 1)[0].trim().toLowerCase();
  }

  function mediaTypeFromSource(source) {
    const value = String(source || "");
    const data = value.match(/^data:([^;,]+)/i);
    if (data) return mediaTypeToken(data[1]);
    const path = value.split(/[?#]/, 1)[0].toLowerCase();
    const extension = path.slice(path.lastIndexOf(".") + 1);
    return MEDIA_TYPE_BY_EXTENSION.get(extension) || "";
  }

  function mediaTypeSupported(element, type) {
    const token = mediaTypeToken(type);
    if (!token) return false;
    const isAudio = token.startsWith("audio/");
    const isVideo = token.startsWith("video/");
    if ((!isAudio && !isVideo) || (element.localName === "audio" && !isAudio)) return false;
    return isAudio ? MEDIA_AUDIO_TYPES.has(token) : MEDIA_VIDEO_TYPES.has(token);
  }

  function mediaDurationFromSource(source) {
    const match = String(source || "").match(/[?#&]duration=([0-9]+(?:\.[0-9]+)?)/i);
    const duration = match ? Number(match[1]) : 1;
    return Number.isFinite(duration) && duration >= 0 ? duration : 1;
  }

  function queueMediaTask(callback) {
    if (typeof __omoikane_queue_media_task === "function") {
      __omoikane_queue_media_task(callback);
    } else if (typeof __omoikane_queue_dom_manipulation_task === "function") {
      __omoikane_queue_dom_manipulation_task(callback);
    } else {
      setTimeout(callback, 0);
    }
  }

  class HTMLMediaElement extends HTMLElement {
    constructor(id) {
      super(id);
      this.__mediaLoadId = 0;
      this.__mediaPlaybackId = 0;
      this.__mediaPlaybackTimer = null;
      this.__mediaPlayWaiters = [];
      this.__mediaScheduledPlayWaiters = [];
      this.__mediaCurrentSrc = "";
      this.__mediaCurrentTime = 0;
      this.__mediaDuration = NaN;
      this.__mediaPaused = true;
      this.__mediaEnded = false;
      this.__mediaReadyState = MEDIA_HAVE_NOTHING;
      this.__mediaNetworkState = MEDIA_NETWORK_EMPTY;
      this.__mediaVolume = 1;
      this.__mediaError = null;
    }

    get src() {
      const raw = this.getAttribute("src");
      return raw === null ? "" : __omoikane_resolve_url(raw);
    }
    set src(value) {
      super.setAttribute("src", String(value));
      this.load();
    }
    setAttribute(name, value) {
      const attr = String(name).toLowerCase();
      super.setAttribute(name, value);
      if (attr === "src") this.load();
    }
    get currentSrc() { return this.__mediaCurrentSrc; }
    get currentTime() { return this.__mediaCurrentTime; }
    set currentTime(value) {
      const next = Number(value);
      if (!Number.isFinite(next)) {
        throw new TypeError("currentTime must be finite");
      }
      const bounded = Number.isFinite(this.__mediaDuration)
        ? Math.min(Math.max(next, 0), this.__mediaDuration) : Math.max(next, 0);
      const changed = bounded !== this.__mediaCurrentTime;
      const wasPlaying = !this.__mediaPaused;
      const reachedEnd = Number.isFinite(this.__mediaDuration) &&
        bounded >= this.__mediaDuration;
      this.__mediaCurrentTime = bounded;
      this.__mediaEnded = reachedEnd;
      if (reachedEnd && wasPlaying) {
        this.__mediaPaused = true;
        this.__mediaCancelPlayback();
      }
      if (changed) {
        this.__mediaQueueEvent("seeking");
        this.__mediaQueueEvent("timeupdate");
        this.__mediaQueueEvent("seeked");
        if (reachedEnd && wasPlaying) this.__mediaQueueEvent("ended");
        if (!this.__mediaPaused && !this.__mediaEnded) this.__mediaScheduleEnd();
      }
    }
    get duration() { return this.__mediaDuration; }
    get paused() { return this.__mediaPaused; }
    get ended() { return this.__mediaEnded; }
    get readyState() { return this.__mediaReadyState; }
    get networkState() { return this.__mediaNetworkState; }
    get volume() { return this.__mediaVolume; }
    set volume(value) {
      const next = Number(value);
      if (!Number.isFinite(next) || next < 0 || next > 1) {
        throw new DOMException("The volume must be between 0 and 1.", "IndexSizeError");
      }
      if (next === this.__mediaVolume) return;
      this.__mediaVolume = next;
      this.__mediaQueueEvent("volumechange");
    }
    get muted() { return this.hasAttribute("muted"); }
    set muted(value) {
      const next = Boolean(value);
      if (next === this.hasAttribute("muted")) return;
      if (next) this.setAttribute("muted", "");
      else this.removeAttribute("muted");
      this.__mediaQueueEvent("volumechange");
    }
    get defaultMuted() { return this.hasAttribute("muted"); }
    set defaultMuted(value) {
      const next = Boolean(value);
      if (next === this.hasAttribute("muted")) return;
      if (next) this.setAttribute("muted", "");
      else this.removeAttribute("muted");
      this.__mediaQueueEvent("volumechange");
    }
    get controls() { return this.hasAttribute("controls"); }
    set controls(value) {
      if (value) this.setAttribute("controls", "");
      else this.removeAttribute("controls");
    }
    get error() { return this.__mediaError; }
    get autoplay() { return this.hasAttribute("autoplay"); }
    set autoplay(value) {
      if (value) this.setAttribute("autoplay", "");
      else this.removeAttribute("autoplay");
    }
    get loop() { return this.hasAttribute("loop"); }
    set loop(value) {
      if (value) this.setAttribute("loop", "");
      else this.removeAttribute("loop");
    }
    get playbackRate() { return this.__mediaPlaybackRate || 1; }
    set playbackRate(value) {
      const next = Number(value);
      if (!Number.isFinite(next) || next <= 0) throw new TypeError("Invalid playbackRate");
      this.__mediaPlaybackRate = next;
      this.__mediaQueueEvent("ratechange");
      if (!this.__mediaPaused) this.__mediaScheduleEnd();
    }
    get defaultPlaybackRate() { return this.__mediaDefaultPlaybackRate || 1; }
    set defaultPlaybackRate(value) {
      const next = Number(value);
      if (!Number.isFinite(next) || next <= 0) throw new TypeError("Invalid defaultPlaybackRate");
      this.__mediaDefaultPlaybackRate = next;
    }
    canPlayType(type) {
      const token = mediaTypeToken(type);
      if (!mediaTypeSupported(this, token)) return "";
      return token === "audio/mpeg" || token === "video/mp4" || token === "video/webm"
        ? "probably" : "maybe";
    }

    __mediaQueueEvent(type, loadId = this.__mediaLoadId) {
      queueMediaTask(() => {
        if (loadId !== this.__mediaLoadId) return;
        // HTML media events do not bubble; the default Event flags are
        // intentional here (including play/playing/timeupdate/ended below).
        fireRealtimeEvent(this, new Event(type));
      });
    }

    __mediaQueueReadyEvent(type, readyState, loadId = this.__mediaLoadId, finalState = readyState) {
      queueMediaTask(() => {
        if (loadId !== this.__mediaLoadId) return;
        this.__mediaReadyState = readyState;
        fireRealtimeEvent(this, new Event(type));
        this.__mediaReadyState = finalState;
      });
    }

    __mediaCancelPlayback() {
      this.__mediaPlaybackId += 1;
      if (this.__mediaPlaybackTimer !== null) {
        clearTimeout(this.__mediaPlaybackTimer);
        this.__mediaPlaybackTimer = null;
      }
      const waiters = this.__mediaScheduledPlayWaiters.splice(0);
      const error = new DOMException("The play() request was interrupted.", "AbortError");
      for (const waiter of waiters) waiter.reject(error);
    }

    __mediaRejectWaiters(error) {
      const waiters = this.__mediaPlayWaiters.splice(0);
      for (const waiter of waiters) waiter.reject(error);
    }

    __mediaFailure(loadId, code, name, message) {
      if (loadId !== this.__mediaLoadId) return;
      this.__mediaReadyState = MEDIA_HAVE_NOTHING;
      this.__mediaNetworkState = MEDIA_NETWORK_NO_SOURCE;
      this.__mediaError = new MediaError(mediaErrorConstructionToken, code, message);
      const error = new DOMException(message, name);
      const waiters = this.__mediaPlayWaiters.splice(0);
      this.__mediaQueueEvent("error", loadId);
      queueMediaTask(() => {
        // The request belongs to this failed load even if a newer load has
        // already started by the time the rejection task runs.
        for (const waiter of waiters) waiter.reject(error);
      });
    }

    __mediaReady(loadId, responseType = "") {
      if (loadId !== this.__mediaLoadId) return;
      const type = mediaTypeToken(responseType) || mediaTypeFromSource(this.__mediaCurrentSrc);
      if (!type || !mediaTypeSupported(this, type)) {
        this.__mediaFailure(
          loadId,
          MediaError.MEDIA_ERR_SRC_NOT_SUPPORTED,
          "NotSupportedError",
          "The media resource is not supported.",
        );
        return;
      }
      this.__mediaError = null;
      this.__mediaNetworkState = MEDIA_NETWORK_IDLE;
      this.__mediaReadyState = MEDIA_HAVE_METADATA;
      this.__mediaDuration = mediaDurationFromSource(this.__mediaCurrentSrc);
      this.__mediaCurrentTime = 0;
      this.__mediaEnded = false;
      const waiters = this.__mediaPlayWaiters.splice(0);
      this.__mediaQueueReadyEvent("durationchange", MEDIA_HAVE_METADATA, loadId);
      this.__mediaQueueReadyEvent("loadedmetadata", MEDIA_HAVE_METADATA, loadId);
      this.__mediaQueueReadyEvent("loadeddata", MEDIA_HAVE_CURRENT_DATA, loadId);
      this.__mediaQueueReadyEvent("canplay", MEDIA_HAVE_FUTURE_DATA, loadId, MEDIA_HAVE_ENOUGH_DATA);
      this.__mediaQueueEvent("load", loadId);
      if (waiters.length) this.__mediaStartPlayback(waiters);
    }

    __mediaBeginLoad(loadId, source) {
      if (loadId !== this.__mediaLoadId) return;
      if (!source) {
        return;
      }
      const sourceType = mediaTypeFromSource(source);
      if (/^data:/i.test(source)) {
        queueMediaTask(() => this.__mediaReady(loadId, sourceType));
        return;
      }
      try {
        Promise.resolve(fetch(source)).then(response => {
          if (loadId !== this.__mediaLoadId) return;
          if (!response || !response.ok) {
            this.__mediaFailure(
              loadId,
              MediaError.MEDIA_ERR_NETWORK,
              "NetworkError",
              "The media resource could not be fetched.",
            );
            return;
          }
          const responseType = response.headers && response.headers.get("content-type");
          queueMediaTask(() => this.__mediaReady(loadId, responseType || sourceType));
        }, () => {
          this.__mediaFailure(
            loadId,
            MediaError.MEDIA_ERR_NETWORK,
            "NetworkError",
            "The media resource could not be fetched.",
          );
        });
      } catch (_) {
        this.__mediaFailure(
          loadId,
          MediaError.MEDIA_ERR_NETWORK,
          "NetworkError",
          "The media resource could not be fetched.",
        );
      }
    }

    __mediaLoad() {
      const loadId = ++this.__mediaLoadId;
      this.__mediaCancelPlayback();
      this.__mediaRejectWaiters(new DOMException("The play() request was interrupted.", "AbortError"));
      this.__mediaCurrentSrc = this.src;
      this.__mediaCurrentTime = 0;
      this.__mediaDuration = NaN;
      this.__mediaPaused = true;
      this.__mediaEnded = false;
      this.__mediaReadyState = MEDIA_HAVE_NOTHING;
      this.__mediaNetworkState = this.__mediaCurrentSrc ? MEDIA_NETWORK_LOADING : MEDIA_NETWORK_NO_SOURCE;
      this.__mediaError = null;
      this.__mediaQueueEvent("loadstart", loadId);
      queueMediaTask(() => this.__mediaBeginLoad(loadId, this.__mediaCurrentSrc));
      return loadId;
    }

    load() {
      this.__mediaLoad();
    }

    play() {
      const source = this.src;
      return new Promise((resolve, reject) => {
        if (!source) {
          const loadId = this.__mediaLoad();
          this.__mediaPlayWaiters.push({ resolve, reject });
          queueMediaTask(() => {
            if (loadId !== this.__mediaLoadId || this.__mediaCurrentSrc) return;
            this.__mediaFailure(
              loadId,
              MediaError.MEDIA_ERR_SRC_NOT_SUPPORTED,
              "NotSupportedError",
              "The element has no supported source.",
            );
          });
          return;
        }
        if (this.__mediaPaused === false) {
          resolve();
          return;
        }
        if (this.__mediaNetworkState === MEDIA_NETWORK_EMPTY ||
            this.__mediaCurrentSrc !== source || this.__mediaNetworkState === MEDIA_NETWORK_NO_SOURCE) {
          this.load();
        }
        this.__mediaPlayWaiters.push({ resolve, reject });
        if (this.__mediaReadyState >= MEDIA_HAVE_FUTURE_DATA &&
            this.__mediaNetworkState === MEDIA_NETWORK_IDLE) {
          const waiters = this.__mediaPlayWaiters.splice(0);
          this.__mediaStartPlayback(waiters);
        }
      });
    }

    pause() {
      const wasPlaying = !this.__mediaPaused;
      this.__mediaPaused = true;
      this.__mediaCancelPlayback();
      this.__mediaRejectWaiters(new DOMException("The play() request was interrupted.", "AbortError"));
      if (wasPlaying) this.__mediaQueueEvent("pause");
    }

    __mediaStartPlayback(waiters) {
      if (!this.__mediaPaused) {
        for (const waiter of waiters) waiter.resolve();
        return;
      }
      if (this.__mediaEnded && Number.isFinite(this.__mediaDuration)) {
        this.__mediaCurrentTime = 0;
      }
      this.__mediaPaused = false;
      this.__mediaEnded = false;
      const playbackId = ++this.__mediaPlaybackId;
      this.__mediaScheduledPlayWaiters = waiters;
      queueMediaTask(() => {
        if (playbackId !== this.__mediaPlaybackId || this.__mediaPaused) return;
        this.__mediaScheduledPlayWaiters = [];
        fireRealtimeEvent(this, new Event("play"));
        fireRealtimeEvent(this, new Event("playing"));
        for (const waiter of waiters) waiter.resolve();
        this.__mediaScheduleEnd(playbackId);
      });
    }

    __mediaScheduleEnd(playbackId = this.__mediaPlaybackId) {
      if (this.__mediaPlaybackTimer !== null) clearTimeout(this.__mediaPlaybackTimer);
      const remaining = Math.max(0, (Number.isFinite(this.__mediaDuration)
        ? this.__mediaDuration : 1) - this.__mediaCurrentTime);
      const delay = Math.max(1, remaining * 1000 / this.playbackRate);
      this.__mediaPlaybackTimer = setTimeout(() => {
        this.__mediaPlaybackTimer = null;
        if (playbackId !== this.__mediaPlaybackId || this.__mediaPaused) return;
        this.__mediaCurrentTime = Number.isFinite(this.__mediaDuration) ? this.__mediaDuration : 1;
        this.__mediaEnded = true;
        this.__mediaPaused = true;
        queueMediaTask(() => {
          if (playbackId !== this.__mediaPlaybackId) return;
          fireRealtimeEvent(this, new Event("timeupdate"));
          fireRealtimeEvent(this, new Event("ended"));
        });
      }, delay);
    }

    removeAttribute(name) {
      const attr = String(name).toLowerCase();
      super.removeAttribute(name);
      if (attr === "src") this.load();
    }
  }

  class HTMLAudioElement extends HTMLMediaElement {}
  class HTMLVideoElement extends HTMLMediaElement {
    get width() { return Math.max(0, Math.trunc(Number(this.getAttribute("width")) || 0)); }
    set width(value) { this.setAttribute("width", String(Math.max(0, Math.trunc(Number(value) || 0)))); }
    get height() { return Math.max(0, Math.trunc(Number(this.getAttribute("height")) || 0)); }
    set height(value) { this.setAttribute("height", String(Math.max(0, Math.trunc(Number(value) || 0)))); }
  }

  for (const [name, value] of [
    ["HAVE_NOTHING", MEDIA_HAVE_NOTHING], ["HAVE_METADATA", MEDIA_HAVE_METADATA],
    ["HAVE_CURRENT_DATA", MEDIA_HAVE_CURRENT_DATA], ["HAVE_FUTURE_DATA", MEDIA_HAVE_FUTURE_DATA],
    ["HAVE_ENOUGH_DATA", MEDIA_HAVE_ENOUGH_DATA], ["NETWORK_EMPTY", MEDIA_NETWORK_EMPTY],
    ["NETWORK_IDLE", MEDIA_NETWORK_IDLE], ["NETWORK_LOADING", MEDIA_NETWORK_LOADING],
    ["NETWORK_NO_SOURCE", MEDIA_NETWORK_NO_SOURCE],
  ]) {
    HTMLMediaElement[name] = value;
    HTMLMediaElement.prototype[name] = value;
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

  function normalizeFormLineBreaks(value) {
    return String(value).replace(/\r(?!\n)|(?<!\r)\n/g, "\r\n");
  }

  // Encodings that cannot carry a file — `application/x-www-form-urlencoded` and
  // `text/plain` — submit a File entry as its filename.
  function formEntryValueAsText(value) {
    return value instanceof File ? value.name : String(value);
  }

  function collectFormEntries(form, submitter = null) {
    const entries = [];
    for (const control of form.__controls()) {
      const name = control.getAttribute("name") || "";
      if (!name || control.__isDisabledControl()) continue;
      const tag = control.tagName;
      const type = String(control.type || "").toLowerCase();
      if (tag === "INPUT" && (type === "checkbox" || type === "radio") && !control.checked) continue;
      if (tag === "INPUT" && type === "file") {
        // A file control contributes one entry per selected file. Omoikane has
        // no file picker, so the list is empty and the entry-list algorithm's
        // fallback applies: a single empty File with an empty name.
        const files = control.files ? Array.from(control.files) : [];
        if (files.length === 0) {
          entries.push([name, new File([], "", { type: "application/octet-stream" })]);
        } else {
          for (const file of files) entries.push([name, file]);
        }
        continue;
      }
      if ((tag === "INPUT" && ["submit", "image", "button", "reset"].includes(type)) || tag === "BUTTON") {
        if (control !== submitter || !["submit", "image", ""].includes(type)) continue;
      }
      if (tag === "SELECT") {
        let selected = control.options.filter(option => option.selected && !option.__isDisabledControl());
        if (!control.hasAttribute("multiple") && selected.length === 0) {
          const fallback = control.options.find(option => !option.__isDisabledControl());
          selected = fallback ? [fallback] : [];
        }
        else if (!control.hasAttribute("multiple") && selected.length > 1) selected = [selected[selected.length - 1]];
        for (const option of selected) entries.push([name, String(option.value)]);
      } else {
        entries.push([name, String(control.value ?? control.getAttribute("value") ?? "")]);
      }
    }
    return entries;
  }

  function formUrlEncode(entries) {
    const encode = value => encodeURIComponent(normalizeFormLineBreaks(value))
      .replace(/%20/g, "+").replace(/[!'()~]/g, c => "%" + c.charCodeAt(0).toString(16).toUpperCase());
    return entries.map(([name, value]) => encode(name) + "=" + encode(formEntryValueAsText(value))).join("&");
  }

  function formTextEncode(entries) {
    return entries.map(([name, value]) => normalizeFormLineBreaks(name) + "=" + normalizeFormLineBreaks(formEntryValueAsText(value))).join("\r\n") + (entries.length ? "\r\n" : "");
  }

  // The multipart/form-data encoding algorithm escapes CR, LF and `"` in field
  // names and filenames rather than dropping them, so a name can never break out
  // of its Content-Disposition header.
  function escapeMultipartHeaderValue(value) {
    return String(value).replace(/\r/g, "%0D").replace(/\n/g, "%0A").replace(/"/g, "%22");
  }

  let formDataBoundaryCounter = 0;

  // Returns the body as a list of parts: strings for header lines and text
  // values, and the File entries themselves so their bytes pass through
  // untouched.
  function formMultipartParts(entries, boundary) {
    const parts = [];
    for (const [name, value] of entries) {
      const disposition = "--" + boundary + "\r\nContent-Disposition: form-data; name=\"" +
        escapeMultipartHeaderValue(name) + "\"";
      if (value instanceof Blob) {
        const filename = value instanceof File ? value.name : "blob";
        parts.push(
          disposition + "; filename=\"" + escapeMultipartHeaderValue(filename) + "\"\r\n" +
            "Content-Type: " + (value.type || "application/octet-stream") + "\r\n\r\n",
          value,
          "\r\n",
        );
      } else {
        parts.push(disposition + "\r\n\r\n" + normalizeFormLineBreaks(value) + "\r\n");
      }
    }
    parts.push("--" + boundary + "--\r\n");
    return parts;
  }

  // Per the FormData spec a Blob value is stored as a File: an explicit filename
  // wins, an existing File keeps its own name, and a bare Blob is named "blob".
  // A filename may only accompany a Blob.
  function formDataEntryValue(value, filename) {
    if (value instanceof Blob) {
      if (filename === undefined && value instanceof File) return value;
      return new File([value], filename === undefined ? "blob" : String(filename), {
        type: value.type,
        lastModified: value instanceof File ? value.lastModified : undefined,
      });
    }
    if (filename !== undefined) throw new TypeError("FormData filename requires a Blob value");
    return String(value);
  }

  class FormData {
    constructor(form = undefined) {
      if (form !== undefined && !(form instanceof HTMLFormElement)) throw new TypeError("FormData argument must be a form");
      this.__entries = form ? collectFormEntries(form) : [];
    }
    append(name, value, filename = undefined) {
      this.__entries.push([String(name), formDataEntryValue(value, filename)]);
    }
    delete(name) { name = String(name); this.__entries = this.__entries.filter(entry => entry[0] !== name); }
    get(name) { name = String(name); return this.__entries.find(entry => entry[0] === name)?.[1] ?? null; }
    getAll(name) { name = String(name); return this.__entries.filter(entry => entry[0] === name).map(entry => entry[1]); }
    has(name) { name = String(name); return this.__entries.some(entry => entry[0] === name); }
    set(name, value, filename = undefined) {
      name = String(name); value = formDataEntryValue(value, filename);
      const index = this.__entries.findIndex(entry => entry[0] === name);
      if (index < 0) this.__entries.push([name, value]);
      else {
        this.__entries[index] = [name, value];
        this.__entries = this.__entries.filter((entry, i) => entry[0] !== name || i === index);
      }
    }
    *entries() { yield* this.__entries.map(entry => entry.slice()); }
    *keys() { for (const [name] of this.__entries) yield name; }
    *values() { for (const [, value] of this.__entries) yield value; }
    forEach(callback, thisArg) { for (const [name, value] of this.__entries) callback.call(thisArg, value, name, this); }
    [Symbol.iterator]() { return this.entries(); }
    // `body` is a string while every part is text — which keeps the plain-text
    // submission path allocation-free — and a `Uint8Array` once a file entry
    // makes the payload binary. The host request binding accepts either.
    __multipart(boundary = "----omoikane-formdata-" + (++formDataBoundaryCounter)) {
      const parts = formMultipartParts(this.__entries, boundary);
      const body = parts.every(part => typeof part === "string")
        ? parts.join("")
        : blobPartsToBytes(parts);
      return { body, contentType: "multipart/form-data; boundary=" + boundary };
    }
  }

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
    get action() { return this.getAttribute("action") || document.URL; }
    set action(value) { this.setAttribute("action", String(value)); }
    get method() { return (this.getAttribute("method") || "get").toLowerCase() === "post" ? "post" : "get"; }
    set method(value) { this.setAttribute("method", String(value)); }
    get enctype() {
      const value = (this.getAttribute("enctype") || "application/x-www-form-urlencoded").toLowerCase();
      return ["application/x-www-form-urlencoded", "multipart/form-data", "text/plain"].includes(value) ? value : "application/x-www-form-urlencoded";
    }
    set enctype(value) { this.setAttribute("enctype", String(value)); }
    __navigate(submitter) {
      const data = collectFormEntries(this, submitter);
      let url = __omoikane_resolve_url(this.action);
      if (this.method === "get") {
        const hashIndex = url.indexOf("#");
        const hash = hashIndex < 0 ? "" : url.slice(hashIndex);
        url = (hashIndex < 0 ? url : url.slice(0, hashIndex)).replace(/\?.*$/, "") + "?" + formUrlEncode(data) + hash;
        __omoikane_submit_form(url, "GET", null, null);
        return;
      }
      let body;
      let contentType = this.enctype;
      if (contentType === "multipart/form-data") {
        const encoded = new FormData(); encoded.__entries = data;
        const multipart = encoded.__multipart(); body = multipart.body; contentType = multipart.contentType;
      } else if (contentType === "text/plain") body = formTextEncode(data);
      else body = formUrlEncode(data);
      __omoikane_submit_form(url, "POST", body, contentType);
    }
    __submit(submitter) {
      const event = new Event("submit", { bubbles: true, cancelable: true });
      event.submitter = submitter || null;
      if (this.dispatchEvent(event)) this.__navigate(submitter || null);
    }
    submit() { this.__navigate(null); }
    requestSubmit(submitter = null) {
      if (submitter !== null) {
        if (submitter.__owningForm() !== this) throw new DOMException("Submitter is not owned by this form", "NotFoundError");
        const type = String(submitter.type || "").toLowerCase();
        if (!((submitter.tagName === "INPUT" && ["submit", "image"].includes(type)) || (submitter.tagName === "BUTTON" && type === "submit"))) throw new TypeError("Not a submit button");
      }
      this.__submit(submitter);
    }
    __reset() {
      this.dispatchEvent(new Event("reset", { bubbles: true, cancelable: true }));
    }
  }

  const TEXT_INPUT_TYPES = new Set(["text", "search", "tel", "url", "email", "password"]);

  function isTextControl(control) {
    return control instanceof HTMLTextAreaElement ||
      (control instanceof HTMLInputElement && TEXT_INPUT_TYPES.has(control.type));
  }

  function ensureTextControlSelection(control) {
    if (control.__selectionStart === undefined) {
      control.__selectionStart = 0;
      control.__selectionEnd = 0;
      control.__selectionDirection = "none";
    }
  }

  function syncTextControlNativeState(control, focused = undefined) {
    if (!isTextControl(control)) return;
    ensureTextControlSelection(control);
    const isFocused = focused === undefined
      ? control.ownerDocument && focusedElementOf(control.ownerDocument) === control
      : Boolean(focused);
    __omoikane_set_text_control_state(
      control.__id,
      control.value,
      control.__selectionStart,
      control.__selectionEnd,
      isFocused,
    );
  }

  function setTextControlSelection(control, start, end, direction) {
    const length = String(control.value).length;
    let normalizedEnd = Math.min(Math.max(Number(end) || 0, 0), length);
    let normalizedStart = Math.min(Math.max(Number(start) || 0, 0), normalizedEnd);
    const normalizedDirection = direction === "forward" || direction === "backward"
      ? direction : "none";
    control.__selectionStart = normalizedStart;
    control.__selectionEnd = normalizedEnd;
    control.__selectionDirection = normalizedDirection;
    syncTextControlNativeState(control);
  }

  function setTextControlSelectionRange(control, start, end, direction) {
    if (!isTextControl(control)) throw new DOMException("The input type does not support selection", "InvalidStateError");
    setTextControlSelection(control, start, end, direction);
  }

  function textControlSelectionStart(control) {
    if (!isTextControl(control)) return null;
    ensureTextControlSelection(control);
    return control.__selectionStart;
  }

  function textControlSelectionEnd(control) {
    if (!isTextControl(control)) return null;
    ensureTextControlSelection(control);
    return control.__selectionEnd;
  }

  function textControlSelectionDirection(control) {
    if (!isTextControl(control)) return null;
    ensureTextControlSelection(control);
    return control.__selectionDirection;
  }

  function selectTextControl(control) {
    if (!isTextControl(control)) return;
    setTextControlSelection(control, 0, control.value.length, "none");
    control.dispatchEvent(new Event("select", { bubbles: true }));
  }

  function beginTextControlFocus(control) {
    if (!isTextControl(control)) return;
    ensureTextControlSelection(control);
    control.__focusValue = control.value;
    control.__textEditChanged = false;
    syncTextControlNativeState(control, true);
  }

  function commitTextControlChange(control) {
    if (!isTextControl(control)) return;
    if (control.__textEditChanged && control.value !== control.__focusValue) {
      control.dispatchEvent(new Event("change", { bubbles: true }));
    }
    control.__focusValue = control.value;
    control.__textEditChanged = false;
    syncTextControlNativeState(control, false);
  }

  function dispatchTextControlInput(control, inputType, data, nextValue, caret) {
    const beforeInput = new InputEvent("beforeinput", {
      bubbles: true, cancelable: true, composed: true, inputType, data,
    });
    if (!control.dispatchEvent(beforeInput)) return false;
    control.value = nextValue;
    setTextControlSelection(control, caret, caret, "none");
    control.__textEditChanged = true;
    control.dispatchEvent(new InputEvent("input", {
      bubbles: true, composed: true, inputType, data,
    }));
    return true;
  }

  function moveTextControlCaret(control, destination, extend) {
    const start = control.selectionStart;
    const end = control.selectionEnd;
    const clamped = Math.min(Math.max(destination, 0), control.value.length);
    if (!extend) {
      setTextControlSelection(control, clamped, clamped, "none");
      return;
    }
    const anchor = start === end ? start :
      (control.selectionDirection === "backward" ? end : start);
    setTextControlSelection(
      control,
      Math.min(anchor, clamped),
      Math.max(anchor, clamped),
      clamped < anchor ? "backward" : "forward",
    );
  }

  function performTextControlKeyDefault(control, init) {
    if (!isTextControl(control) || control.readOnly || control.__isDisabledControl()) return;
    ensureTextControlSelection(control);
    const value = control.value;
    const start = control.selectionStart;
    const end = control.selectionEnd;
    const key = String(init.key || "");

    if (key === "Enter" && control instanceof HTMLInputElement) {
      const form = control.__owningForm();
      if (form) {
        const submitter = form.__controls().find(candidate => {
          const type = String(candidate.type || "").toLowerCase();
          return !candidate.__isDisabledControl() && ((candidate.tagName === "INPUT" && ["submit", "image"].includes(type)) || (candidate.tagName === "BUTTON" && type === "submit"));
        }) || null;
        form.requestSubmit(submitter);
      }
      return;
    }

    if (key === "ArrowLeft") {
      const destination = !init.shiftKey && start !== end ? start : Math.max(start - 1, 0);
      moveTextControlCaret(control, destination, Boolean(init.shiftKey));
      return;
    }
    if (key === "ArrowRight") {
      const destination = !init.shiftKey && start !== end ? end : Math.min(end + 1, value.length);
      moveTextControlCaret(control, destination, Boolean(init.shiftKey));
      return;
    }
    if (key === "Home" || key === "End") {
      moveTextControlCaret(control, key === "Home" ? 0 : value.length, Boolean(init.shiftKey));
      return;
    }

    if (key === "Backspace" || key === "Delete") {
      let deleteStart = start;
      let deleteEnd = end;
      const backward = key === "Backspace";
      if (deleteStart === deleteEnd) {
        if (backward && deleteStart > 0) deleteStart--;
        else if (!backward && deleteEnd < value.length) deleteEnd++;
        else return;
      }
      dispatchTextControlInput(
        control,
        backward ? "deleteContentBackward" : "deleteContentForward",
        null,
        value.slice(0, deleteStart) + value.slice(deleteEnd),
        deleteStart,
      );
      return;
    }

    if (init.ctrlKey || init.metaKey || init.altKey) return;
    let text = init.text ? String(init.text) : (Array.from(key).length === 1 ? key : "");
    if (!text) return;
    if (control.maxLength >= 0) {
      const available = Math.max(control.maxLength - (value.length - (end - start)), 0);
      text = text.slice(0, available);
      if (!text) return;
    }
    dispatchTextControlInput(
      control,
      "insertText",
      text,
      value.slice(0, start) + text + value.slice(end),
      start + text.length,
    );
  }

  class HTMLInputElement extends HTMLElement {
    get type() {
      const t = (this.getAttribute("type") || "").toLowerCase();
      return t || "text";
    }
    set type(v) {
      const before = this.type;
      this.setAttribute("type", String(v));
      if (before === "file" && this.type !== "file") this.__files = undefined;
      if (before !== "file" && this.type === "file") this.__files = new FileList();
    }
    // Only a file control has a selected files list. The host has no native
    // picker, but scripts can provide a deterministic synthetic selection via
    // `input.files = new DataTransfer().files`.
    get files() {
      if (this.type !== "file") return null;
      if (this.__files === undefined) this.__files = new FileList();
      return this.__files;
    }
    set files(value) {
      if (this.type !== "file") {
        throw new DOMException("The input is not a file control.", "InvalidStateError");
      }
      if (value !== null && !(value instanceof FileList)) {
        throw new TypeError("HTMLInputElement.files must be a FileList or null");
      }
      const next = value ? Array.from(value) : [];
      if (next.some(file => !(file instanceof File))) {
        throw new TypeError("HTMLInputElement.files contains a non-File value");
      }
      const previous = this.files;
      const unchanged = previous.__files.length === next.length &&
        previous.__files.every((file, index) => file === next[index]);
      if (unchanged) return;
      this.__files = new FileList(next);
      // A synthetic selection is an observable user-input boundary. Keep the
      // event order deterministic and expose no host path: `value` below only
      // ever reports the browser-style fakepath plus the selected filename.
      this.dispatchEvent(new Event("input", { bubbles: true }));
      this.dispatchEvent(new Event("change", { bubbles: true }));
    }
    // The `value` IDL attribute is the control's "dirty value": it is held in
    // JS and is NOT reflected to the `value` content attribute. Storing it in
    // JS also preserves lone UTF-16 surrogates that would otherwise be mangled
    // crossing the native boundary.
    get value() {
      if (this.type === "file") {
        const files = this.files;
        return files.length ? "C:\\fakepath\\" + files[0].name.split(/[\\/]/).pop() : "";
      }
      if (this.__value !== undefined) return this.__value;
      return this.getAttribute("value") || "";
    }
    set value(v) {
      if (this.type === "file") {
        if (String(v) !== "") throw new DOMException("The value of a file input may only be cleared.", "InvalidStateError");
        this.files = null;
        return;
      }
      this.__value = v == null ? "" : String(v);
      setTextControlSelection(this, this.__value.length, this.__value.length, "none");
    }
    get defaultValue() {
      return this.getAttribute("value") || "";
    }
    set defaultValue(v) {
      this.setAttribute("value", String(v));
    }
    get readOnly() { return this.hasAttribute("readonly"); }
    set readOnly(value) {
      if (value) this.setAttribute("readonly", "");
      else this.removeAttribute("readonly");
    }
    get maxLength() {
      const raw = this.getAttribute("maxlength");
      if (raw === null || !/^\d+$/.test(raw)) return -1;
      return Number(raw);
    }
    set maxLength(value) {
      const length = Number(value);
      if (!Number.isInteger(length) || length < 0) throw new DOMException("Invalid maxlength", "IndexSizeError");
      this.setAttribute("maxlength", String(length));
    }
    get selectionStart() { return textControlSelectionStart(this); }
    set selectionStart(value) { this.setSelectionRange(value, this.selectionEnd, this.selectionDirection); }
    get selectionEnd() { return textControlSelectionEnd(this); }
    set selectionEnd(value) { this.setSelectionRange(this.selectionStart, value, this.selectionDirection); }
    get selectionDirection() { return textControlSelectionDirection(this); }
    set selectionDirection(value) { this.setSelectionRange(this.selectionStart, this.selectionEnd, value); }
    setSelectionRange(start, end, direction = "none") {
      setTextControlSelectionRange(this, start, end, direction);
    }
    select() { selectTextControl(this); }
  }

  class HTMLTextAreaElement extends HTMLElement {
    get value() {
      if (this.__value !== undefined) return this.__value;
      const initial = this.textContent || "";
      return initial.startsWith("\r\n") ? initial.slice(2) :
        (initial.startsWith("\n") || initial.startsWith("\r") ? initial.slice(1) : initial);
    }
    set value(value) {
      this.__value = value == null ? "" : String(value);
      setTextControlSelection(this, this.__value.length, this.__value.length, "none");
    }
    get defaultValue() { return this.textContent || ""; }
    set defaultValue(value) { this.textContent = String(value); }
    get readOnly() { return this.hasAttribute("readonly"); }
    set readOnly(value) {
      if (value) this.setAttribute("readonly", "");
      else this.removeAttribute("readonly");
    }
    get maxLength() {
      const raw = this.getAttribute("maxlength");
      if (raw === null || !/^\d+$/.test(raw)) return -1;
      return Number(raw);
    }
    set maxLength(value) {
      const length = Number(value);
      if (!Number.isInteger(length) || length < 0) throw new DOMException("Invalid maxlength", "IndexSizeError");
      this.setAttribute("maxlength", String(length));
    }
    get selectionStart() { return textControlSelectionStart(this); }
    set selectionStart(value) { this.setSelectionRange(value, this.selectionEnd, this.selectionDirection); }
    get selectionEnd() { return textControlSelectionEnd(this); }
    set selectionEnd(value) { this.setSelectionRange(this.selectionStart, value, this.selectionDirection); }
    get selectionDirection() { return textControlSelectionDirection(this); }
    set selectionDirection(value) { this.setSelectionRange(this.selectionStart, this.selectionEnd, value); }
    setSelectionRange(start, end, direction = "none") {
      setTextControlSelectionRange(this, start, end, direction);
    }
    select() { selectTextControl(this); }
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
    setAttribute(name, value) {
      if (String(name).toLowerCase() === "src") noteElementResourceStart(this);
      super.setAttribute(name, value);
    }
    get src() {
      return this.getAttribute("src") || "";
    }
    set src(value) {
      this.setAttribute("src", String(value));
    }
    // Reflected, because how the element is evaluated is decided from the
    // attribute. Scripts that opt into modules by assigning the property
    // (`script.type = "module"`) would otherwise be run as classic scripts.
    get type() {
      return this.getAttribute("type") || "";
    }
    set type(value) {
      this.setAttribute("type", String(value));
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
    setAttribute(name, value) {
      const isSource = String(name).toLowerCase() === "src";
      if (isSource) noteElementResourceStart(this);
      super.setAttribute(name, value);
      if (isSource && /^(?:data:|blob:)/i.test(String(value))) {
        finishElementResourceTiming(this, 200, false);
      }
    }
    get src() {
      return this.getAttribute("src") || "";
    }
    set src(value) {
      this.setAttribute("src", String(value));
    }
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

  class ImageData {
    constructor(dataOrWidth, widthOrHeight, height) {
      if (typeof dataOrWidth === "number") {
        this.width = Math.max(0, Math.trunc(dataOrWidth));
        this.height = Math.max(0, Math.trunc(widthOrHeight));
        this.data = new Uint8ClampedArray(this.width * this.height * 4);
      } else {
        this.data = new Uint8ClampedArray(dataOrWidth);
        this.width = Math.max(0, Math.trunc(widthOrHeight));
        this.height = height === undefined ? this.data.length / 4 / this.width : Math.max(0, Math.trunc(height));
        if (this.data.length !== this.width * this.height * 4) throw new DOMException("Invalid ImageData dimensions", "IndexSizeError");
      }
    }
  }

  const canvasStates = new WeakMap();
  function canvasDimensions(canvas) {
    const integer = (name, fallback) => {
      const value = typeof canvas.getAttribute === "function"
        ? canvas.getAttribute(name) : canvas["__" + name];
      return value === null || value === undefined
        ? fallback : Math.max(0, Math.min(32768, Math.trunc(Number(value)) || 0));
    };
    return [integer("width", 300), integer("height", 150)];
  }
  function canvasColor(value, alpha) {
    const text = String(value).trim().toLowerCase();
    const named = { black:[0,0,0,255], white:[255,255,255,255], red:[255,0,0,255], green:[0,128,0,255], blue:[0,0,255,255], transparent:[0,0,0,0] };
    let color = named[text];
    if (!color && /^#[0-9a-f]{6}$/.test(text)) color = [parseInt(text.slice(1,3),16),parseInt(text.slice(3,5),16),parseInt(text.slice(5,7),16),255];
    if (!color && /^#[0-9a-f]{3}$/.test(text)) color = [...text.slice(1)].map(ch=>parseInt(ch+ch,16)).concat(255);
    const rgb = text.match(/^rgba?\(([^)]+)\)$/);
    if (!color && rgb) { const p=rgb[1].split(",").map(Number); color=[p[0]||0,p[1]||0,p[2]||0,p.length>3?Math.round(Math.max(0,Math.min(1,p[3]))*255):255]; }
    color = color || [0,0,0,255];
    return [color[0],color[1],color[2],Math.round(color[3]*alpha)];
  }
  function initialCanvasState(canvas) {
    const [width,height]=canvasDimensions(canvas);
    return { width,height,pixels:new Uint8ClampedArray(width*height*4), context:null, webgl:null, contextMode:null,
      style:{fillStyle:"#000000",strokeStyle:"#000000",globalAlpha:1,lineWidth:1,lineCap:"butt",lineJoin:"miter",miterLimit:10,font:"10px sans-serif",transform:[1,0,0,1,0,0],clip:null}, stack:[], paths:[], current:null };
  }
  function canvasState(canvas) {
    let state=canvasStates.get(canvas); const [width,height]=canvasDimensions(canvas);
    if (!state || state.width!==width || state.height!==height) {
      const previous = state;
      state=initialCanvasState(canvas);
      if (previous) {
        state.contextMode = previous.contextMode;
        state.context = previous.context;
        state.webgl = previous.webgl;
      }
      canvasStates.set(canvas,state); commitCanvas(canvas,state);
    }
    return state;
  }
  function blendCanvasPixel(state,x,y,color,clear=false) {
    x=Math.floor(x); y=Math.floor(y); if(x<0||y<0||x>=state.width||y>=state.height)return;
    if(state.style.clip && !pointInCanvasPaths(x+.5,y+.5,state.style.clip.paths,state.style.clip.rule))return;
    const i=(y*state.width+x)*4; if(clear){state.pixels.fill(0,i,i+4);return;}
    const sa=color[3]/255, da=state.pixels[i+3]/255, oa=sa+da*(1-sa);
    if(!oa){state.pixels.fill(0,i,i+4);return;}
    for(let c=0;c<3;c++)state.pixels[i+c]=Math.round((color[c]*sa+state.pixels[i+c]*da*(1-sa))/oa);
    state.pixels[i+3]=Math.round(oa*255);
  }
  function transformCanvasPoint(m,x,y){return [m[0]*x+m[2]*y+m[4],m[1]*x+m[3]*y+m[5]];}
  function pointInCanvasPaths(x,y,paths,rule){let winding=0,crossings=0;for(const path of paths){for(let i=0;i<path.length;i++){const a=path[i],b=path[(i+1)%path.length];if((a[1]>y)!==(b[1]>y)){const ix=a[0]+(y-a[1])*(b[0]-a[0])/(b[1]-a[1]);if(ix>x){crossings++;winding+=b[1]>a[1]?1:-1;}}}}return rule==="evenodd"?crossings%2===1:winding!==0;}
  function commitCanvas(canvas,state){
    if (canvas.__id != null) __omoikane_canvas_commit(canvas.__id,state.width,state.height,new Uint8Array(state.pixels.buffer));
  }

  class CanvasRenderingContext2D {
    constructor(canvas){this.canvas=canvas;}
    get __s(){return canvasState(this.canvas);}
    save(){const s=this.__s;s.stack.push({...s.style,transform:s.style.transform.slice(),clip:s.style.clip&&{rule:s.style.clip.rule,paths:s.style.clip.paths.map(p=>p.map(q=>q.slice()))}});}
    restore(){const s=this.__s;if(s.stack.length)s.style=s.stack.pop();}
    translate(x,y){this.transform(1,0,0,1,x,y);} rotate(a){this.transform(Math.cos(a),Math.sin(a),-Math.sin(a),Math.cos(a),0,0);} scale(x,y){this.transform(x,0,0,y,0,0);}
    transform(a,b,c,d,e,f){const s=this.__s,m=s.style.transform;s.style.transform=[m[0]*a+m[2]*b,m[1]*a+m[3]*b,m[0]*c+m[2]*d,m[1]*c+m[3]*d,m[0]*e+m[2]*f+m[4],m[1]*e+m[3]*f+m[5]];}
    setTransform(a,b,c,d,e,f){if(typeof a==="object"){({a,b,c,d,e,f}=a);}this.__s.style.transform=[Number(a),Number(b),Number(c),Number(d),Number(e),Number(f)];}
    resetTransform(){this.__s.style.transform=[1,0,0,1,0,0];}
    clearRect(x,y,w,h){const s=this.__s;for(let py=Math.floor(y);py<Math.ceil(y+h);py++)for(let px=Math.floor(x);px<Math.ceil(x+w);px++){const p=transformCanvasPoint(s.style.transform,px,py);blendCanvasPixel(s,p[0],p[1],[0,0,0,0],true);}commitCanvas(this.canvas,s);}
    fillRect(x,y,w,h){this.beginPath();this.rect(x,y,w,h);this.fill();}
    strokeRect(x,y,w,h){this.beginPath();this.rect(x,y,w,h);this.stroke();}
    beginPath(){const s=this.__s;s.paths=[];s.current=null;}
    moveTo(x,y){const s=this.__s,p=transformCanvasPoint(s.style.transform,+x,+y);s.current=[p];s.paths.push(s.current);}
    lineTo(x,y){const s=this.__s,p=transformCanvasPoint(s.style.transform,+x,+y);if(!s.current)this.moveTo(x,y);else s.current.push(p);}
    closePath(){const s=this.__s;if(s.current&&s.current.length)s.current.push(s.current[0].slice());}
    rect(x,y,w,h){this.moveTo(x,y);this.lineTo(x+w,y);this.lineTo(x+w,y+h);this.lineTo(x,y+h);this.closePath();}
    quadraticCurveTo(cx,cy,x,y){const s=this.__s;if(!s.current||!s.current.length)this.moveTo(0,0);const p0=s.current[s.current.length-1],p=transformCanvasPoint(s.style.transform,+cx,+cy),e=transformCanvasPoint(s.style.transform,+x,+y);for(let i=1;i<=20;i++){const t=i/20,u=1-t;s.current.push([u*u*p0[0]+2*u*t*p[0]+t*t*e[0],u*u*p0[1]+2*u*t*p[1]+t*t*e[1]]);}}
    bezierCurveTo(c1x,c1y,c2x,c2y,x,y){const s=this.__s;if(!s.current||!s.current.length)this.moveTo(0,0);const p0=s.current[s.current.length-1],p1=transformCanvasPoint(s.style.transform,c1x,c1y),p2=transformCanvasPoint(s.style.transform,c2x,c2y),p3=transformCanvasPoint(s.style.transform,x,y);for(let i=1;i<=24;i++){const t=i/24,u=1-t;s.current.push([u*u*u*p0[0]+3*u*u*t*p1[0]+3*u*t*t*p2[0]+t*t*t*p3[0],u*u*u*p0[1]+3*u*u*t*p1[1]+3*u*t*t*p2[1]+t*t*t*p3[1]]);}}
    arc(x,y,r,start,end,ccw=false){const span=ccw?start-end:end-start,steps=Math.max(8,Math.ceil(Math.abs(span)*12));for(let i=0;i<=steps;i++){const a=ccw?start-span*i/steps:start+span*i/steps;i?this.lineTo(x+r*Math.cos(a),y+r*Math.sin(a)):this.moveTo(x+r*Math.cos(a),y+r*Math.sin(a));}}
    ellipse(x,y,rx,ry,rotation,start,end,ccw=false){const span=ccw?start-end:end-start,steps=Math.max(8,Math.ceil(Math.abs(span)*12));for(let i=0;i<=steps;i++){const a=ccw?start-span*i/steps:start+span*i/steps,px=rx*Math.cos(a),py=ry*Math.sin(a),xx=x+px*Math.cos(rotation)-py*Math.sin(rotation),yy=y+px*Math.sin(rotation)+py*Math.cos(rotation);i?this.lineTo(xx,yy):this.moveTo(xx,yy);}}
    arcTo(x1,y1,x2,y2){this.lineTo(x1,y1);this.lineTo(x2,y2);}
    fill(rule="nonzero"){const s=this.__s,c=canvasColor(s.style.fillStyle,s.style.globalAlpha);for(let y=0;y<s.height;y++)for(let x=0;x<s.width;x++)if(pointInCanvasPaths(x+.5,y+.5,s.paths,rule))blendCanvasPixel(s,x,y,c);commitCanvas(this.canvas,s);}
    stroke(){const s=this.__s,c=canvasColor(s.style.strokeStyle,s.style.globalAlpha),radius=Math.max(.5,s.style.lineWidth/2);for(const path of s.paths)for(let i=1;i<path.length;i++){const a=path[i-1],b=path[i],minx=Math.floor(Math.min(a[0],b[0])-radius),maxx=Math.ceil(Math.max(a[0],b[0])+radius),miny=Math.floor(Math.min(a[1],b[1])-radius),maxy=Math.ceil(Math.max(a[1],b[1])+radius),dx=b[0]-a[0],dy=b[1]-a[1],l=dx*dx+dy*dy;for(let y=miny;y<=maxy;y++)for(let x=minx;x<=maxx;x++){const t=l?Math.max(0,Math.min(1,((x+.5-a[0])*dx+(y+.5-a[1])*dy)/l)):0,qx=a[0]+t*dx,qy=a[1]+t*dy;if(Math.hypot(x+.5-qx,y+.5-qy)<=radius)blendCanvasPixel(s,x,y,c);}}commitCanvas(this.canvas,s);}
    clip(rule="nonzero"){const s=this.__s;s.style.clip={rule:rule==="evenodd"?"evenodd":"nonzero",paths:s.paths.map(p=>p.map(q=>q.slice()))};}
    createImageData(a,b){return a instanceof ImageData?new ImageData(a.width,a.height):new ImageData(a,b);}
    getImageData(sx,sy,sw,sh){sw=Number(sw);sh=Number(sh);if(sw<=0||sh<=0)throw new DOMException("ImageData dimensions must be positive","IndexSizeError");const s=this.__s,out=new ImageData(sw,sh);for(let y=0;y<out.height;y++)for(let x=0;x<out.width;x++){const si=((sy+y)*s.width+(sx+x))*4,di=(y*out.width+x)*4;if(sx+x>=0&&sy+y>=0&&sx+x<s.width&&sy+y<s.height)out.data.set(s.pixels.slice(si,si+4),di);}return out;}
    putImageData(image,dx,dy){const s=this.__s;for(let y=0;y<image.height;y++)for(let x=0;x<image.width;x++){const tx=dx+x,ty=dy+y;if(tx>=0&&ty>=0&&tx<s.width&&ty<s.height)s.pixels.set(image.data.slice((y*image.width+x)*4,(y*image.width+x+1)*4),(ty*s.width+tx)*4);}commitCanvas(this.canvas,s);}
    drawImage(source,...args){let src;
      const isHtmlCanvas = typeof HTMLCanvasElement !== "undefined" && source instanceof HTMLCanvasElement;
      if (isHtmlCanvas || source instanceof OffscreenCanvas) src=canvasState(source);
      else if (source instanceof ImageBitmap) {
        if (source.__detached) throw new DOMException("The ImageBitmap is detached","InvalidStateError");
        src={width:source.width,height:source.height,pixels:source.__pixels};
      } else {
        const raw=source&&source.__id!=null?__omoikane_canvas_image_source(source.__id):null;
        if(raw===null)throw new DOMException("Image source is unavailable","InvalidStateError");
        const decoded=JSON.parse(raw);src={width:decoded.width,height:decoded.height,pixels:bytesFromBase64(decoded.pixels)};
      }
      const s=this.__s;let sx=0,sy=0,sw=src.width,sh=src.height,dx,dy,dw,dh;
      if(args.length===2){[dx,dy]=args;dw=sw;dh=sh;}else if(args.length===4){[dx,dy,dw,dh]=args;}else{[sx,sy,sw,sh,dx,dy,dw,dh]=args;}
      for(let y=0;y<dh;y++)for(let x=0;x<dw;x++){const xx=Math.floor(sx+x*sw/dw),yy=Math.floor(sy+y*sh/dh),i=(yy*src.width+xx)*4;blendCanvasPixel(s,dx+x,dy+y,[src.pixels[i],src.pixels[i+1],src.pixels[i+2],Math.round(src.pixels[i+3]*s.style.globalAlpha)]);}commitCanvas(this.canvas,s);}
    measureText(text){const size=parseFloat(this.__s.style.font)||10;return {width:String(text).length*size*.6,actualBoundingBoxAscent:size*.8,actualBoundingBoxDescent:size*.2};}
    fillText(text,x,y){const s=this.__s,size=parseFloat(s.style.font)||10,w=this.measureText(text).width;this.fillRect(x,y-size*.8,w,size);}
    strokeText(text,x,y){const s=this.__s,size=parseFloat(s.style.font)||10,w=this.measureText(text).width;this.strokeRect(x,y-size*.8,w,size);}
  }
  for(const name of ["fillStyle","strokeStyle","globalAlpha","lineWidth","lineCap","lineJoin","miterLimit","font"]){Object.defineProperty(CanvasRenderingContext2D.prototype,name,{get(){return this.__s.style[name];},set(value){this.__s.style[name]=name==="globalAlpha"?Math.max(0,Math.min(1,Number(value))):name==="lineWidth"?Math.max(0,Number(value)):String(value);}});}

  // EventTarget is declared later in this bootstrap (Navigator is constructed
  // before the shared event infrastructure is installed).  The bridge keeps
  // WebGL context construction safe during that initial pass and adopts the
  // real EventTarget prototype once it becomes available below.
  class WebGLEventTarget {
    constructor() { this._listeners = new Map(); }
    addEventListener(...args) { return globalThis.EventTarget.prototype.addEventListener.call(this, ...args); }
    removeEventListener(...args) { return globalThis.EventTarget.prototype.removeEventListener.call(this, ...args); }
    dispatchEvent(...args) { return globalThis.EventTarget.prototype.dispatchEvent.call(this, ...args); }
  }

  const webglContextConstructionToken = {};
  const webglResourceConstructionToken = {};
  class WebGLResource {
    constructor(context, kind, token) {
      if (token !== webglResourceConstructionToken) throw new TypeError("Illegal constructor");
      this.__context = context;
      this.__kind = kind;
      this.__deleted = false;
    }
    get [Symbol.toStringTag]() { return this.__kind; }
  }
  class WebGLBuffer extends WebGLResource {
    constructor(context, token) { super(context, "WebGLBuffer", token); this.__size = 0; this.__usage = 0; }
  }
  class WebGLShader extends WebGLResource {
    constructor(context, type, token) {
      super(context, "WebGLShader", token);
      this.__type = type;
      this.__source = "";
      this.__compiled = false;
      this.__infoLog = "";
    }
  }
  class WebGLProgram extends WebGLResource {
    constructor(context, token) {
      super(context, "WebGLProgram", token);
      this.__shaders = [];
      this.__linked = false;
      this.__infoLog = "";
      this.__attributes = new Map();
      this.__uniforms = new Map();
    }
  }
  class WebGLUniformLocation {
    constructor(program, name, token) {
      if (token !== webglResourceConstructionToken) throw new TypeError("Illegal constructor");
      this.__program = program;
      this.__name = name;
    }
    get [Symbol.toStringTag]() { return "WebGLUniformLocation"; }
  }

  const WEBGL_CONSTANTS = {
    DEPTH_BUFFER_BIT: 0x00000100, STENCIL_BUFFER_BIT: 0x00000400, COLOR_BUFFER_BIT: 0x00004000,
    POINTS: 0x0000, LINES: 0x0001, LINE_LOOP: 0x0002, LINE_STRIP: 0x0003,
    TRIANGLES: 0x0004, TRIANGLE_STRIP: 0x0005, TRIANGLE_FAN: 0x0006,
    ZERO: 0, ONE: 1, SRC_COLOR: 0x0300, ONE_MINUS_SRC_COLOR: 0x0301,
    SRC_ALPHA: 0x0302, ONE_MINUS_SRC_ALPHA: 0x0303, DST_ALPHA: 0x0304,
    ONE_MINUS_DST_ALPHA: 0x0305, DST_COLOR: 0x0306, ONE_MINUS_DST_COLOR: 0x0307,
    SRC_ALPHA_SATURATE: 0x0308, FUNC_ADD: 0x8006, BLEND_EQUATION: 0x8009,
    BLEND_EQUATION_RGB: 0x8009, BLEND_EQUATION_ALPHA: 0x883D,
    FUNC_SUBTRACT: 0x800A, FUNC_REVERSE_SUBTRACT: 0x800B,
    BLEND_DST_RGB: 0x80C8, BLEND_SRC_RGB: 0x80C9, BLEND_DST_ALPHA: 0x80CA,
    BLEND_SRC_ALPHA: 0x80CB, BLEND_COLOR: 0x8005,
    CONSTANT_COLOR: 0x8001, ONE_MINUS_CONSTANT_COLOR: 0x8002,
    CONSTANT_ALPHA: 0x8003, ONE_MINUS_CONSTANT_ALPHA: 0x8004,
    ARRAY_BUFFER: 0x8892, ELEMENT_ARRAY_BUFFER: 0x8893,
    ARRAY_BUFFER_BINDING: 0x8894, ELEMENT_ARRAY_BUFFER_BINDING: 0x8895,
    STREAM_DRAW: 0x88E0, STATIC_DRAW: 0x88E4, DYNAMIC_DRAW: 0x88E8,
    BUFFER_SIZE: 0x8764, BUFFER_USAGE: 0x8765,
    CURRENT_VERTEX_ATTRIB: 0x8626, FRONT: 0x0404, BACK: 0x0405, FRONT_AND_BACK: 0x0408,
    CULL_FACE: 0x0B44, BLEND: 0x0BE2, DITHER: 0x0BD0, STENCIL_TEST: 0x0B90,
    DEPTH_TEST: 0x0B71, SCISSOR_TEST: 0x0C11, POLYGON_OFFSET_FILL: 0x8037,
    SAMPLE_ALPHA_TO_COVERAGE: 0x809E, SAMPLE_COVERAGE: 0x80A0,
    NO_ERROR: 0, INVALID_ENUM: 0x0500, INVALID_VALUE: 0x0501,
    INVALID_OPERATION: 0x0502, OUT_OF_MEMORY: 0x0505,
    INVALID_FRAMEBUFFER_OPERATION: 0x0506, CONTEXT_LOST_WEBGL: 0x9242,
    EXP: 0x0800, EXP2: 0x0801,
    NEVER: 0x0200, LESS: 0x0201, EQUAL: 0x0202, LEQUAL: 0x0203,
    GREATER: 0x0204, NOTEQUAL: 0x0205, GEQUAL: 0x0206, ALWAYS: 0x0207,
    KEEP: 0x1E00, REPLACE: 0x1E01, INCR: 0x1E02, DECR: 0x1E03,
    INVERT: 0x150A, INCR_WRAP: 0x8507, DECR_WRAP: 0x8508,
    VERTEX_ATTRIB_ARRAY_ENABLED: 0x8622, VERTEX_ATTRIB_ARRAY_SIZE: 0x8623,
    VERTEX_ATTRIB_ARRAY_STRIDE: 0x8624, VERTEX_ATTRIB_ARRAY_TYPE: 0x8625,
    VERTEX_ATTRIB_ARRAY_NORMALIZED: 0x886A, VERTEX_ATTRIB_ARRAY_POINTER: 0x8645,
    VERTEX_ATTRIB_ARRAY_BUFFER_BINDING: 0x889F,
    FLOAT: 0x1406, UNSIGNED_BYTE: 0x1401,
    UNSIGNED_SHORT: 0x1403, UNSIGNED_INT: 0x1405,
    VERTEX_SHADER: 0x8B31, FRAGMENT_SHADER: 0x8B30,
    COMPILE_STATUS: 0x8B81, DELETE_STATUS: 0x8B80, SHADER_TYPE: 0x8B4F,
    LINK_STATUS: 0x8B82, VALIDATE_STATUS: 0x8B83, ATTACHED_SHADERS: 0x8B85,
    CURRENT_PROGRAM: 0x8B8D, ACTIVE_ATTRIBUTES: 0x8B89, ACTIVE_UNIFORMS: 0x8B86,
    ACTIVE_TEXTURE: 0x84E0, TEXTURE0: 0x84C0, TEXTURE_BINDING_2D: 0x8069,
    TEXTURE_2D: 0x0DE1, TEXTURE_CUBE_MAP: 0x8513, RGB: 0x1907, RGBA: 0x1908,
    VIEWPORT: 0x0BA2, SCISSOR_BOX: 0x0C10,
    COLOR_CLEAR_VALUE: 0x0C22, COLOR_WRITEMASK: 0x0C23,
    DEPTH_CLEAR_VALUE: 0x0B73, DEPTH_WRITEMASK: 0x0B72,
    STENCIL_CLEAR_VALUE: 0x0B91, STENCIL_WRITEMASK: 0x0B98,
    STENCIL_BACK_WRITEMASK: 0x8CA5, DEPTH_BITS: 0x0D56, STENCIL_BITS: 0x0D57,
    RED_BITS: 0x0D52, GREEN_BITS: 0x0D53, BLUE_BITS: 0x0D54, ALPHA_BITS: 0x0D55,
    MAX_TEXTURE_SIZE: 0x0D33, MAX_CUBE_MAP_TEXTURE_SIZE: 0x851C,
    MAX_VIEWPORT_DIMS: 0x0D3A, MAX_VERTEX_ATTRIBS: 0x8869,
    MAX_VERTEX_UNIFORM_VECTORS: 0x8DFB, MAX_VARYING_VECTORS: 0x8DFC,
    MAX_COMBINED_TEXTURE_IMAGE_UNITS: 0x8B4D, MAX_VERTEX_TEXTURE_IMAGE_UNITS: 0x8B4C,
    MAX_TEXTURE_IMAGE_UNITS: 0x8872, MAX_FRAGMENT_UNIFORM_VECTORS: 0x8DFD,
    SAMPLE_BUFFERS: 0x80A8, SAMPLES: 0x80A9, SUBPIXEL_BITS: 0x0D50,
    RENDERER: 0x1F01, VENDOR: 0x1F00, VERSION: 0x1F02,
    SHADING_LANGUAGE_VERSION: 0x8B8C,
  };

  const WEBGL_CAPABILITIES = Object.freeze([
    WEBGL_CONSTANTS.BLEND, WEBGL_CONSTANTS.CULL_FACE, WEBGL_CONSTANTS.DEPTH_TEST,
    WEBGL_CONSTANTS.DITHER, WEBGL_CONSTANTS.POLYGON_OFFSET_FILL,
    WEBGL_CONSTANTS.SAMPLE_ALPHA_TO_COVERAGE, WEBGL_CONSTANTS.SAMPLE_COVERAGE,
    WEBGL_CONSTANTS.SCISSOR_TEST, WEBGL_CONSTANTS.STENCIL_TEST,
  ]);
  const WEBGL_CLEAR_BITS = WEBGL_CONSTANTS.COLOR_BUFFER_BIT |
    WEBGL_CONSTANTS.DEPTH_BUFFER_BIT | WEBGL_CONSTANTS.STENCIL_BUFFER_BIT;
  const WEBGL_BUFFER_TARGETS = Object.freeze([
    WEBGL_CONSTANTS.ARRAY_BUFFER, WEBGL_CONSTANTS.ELEMENT_ARRAY_BUFFER,
  ]);
  const WEBGL_DRAW_MODES = Object.freeze([
    WEBGL_CONSTANTS.POINTS, WEBGL_CONSTANTS.LINES, WEBGL_CONSTANTS.LINE_LOOP,
    WEBGL_CONSTANTS.LINE_STRIP, WEBGL_CONSTANTS.TRIANGLES,
    WEBGL_CONSTANTS.TRIANGLE_STRIP, WEBGL_CONSTANTS.TRIANGLE_FAN,
  ]);

  function webglClamp(value) {
    const number = Number(value);
    if (Number.isNaN(number)) return 0;
    return Math.max(0, Math.min(1, number));
  }
  function webglError(context, code) {
    if (!context.__state.errors.length) context.__state.errors.push(code);
  }
  function webglActive(context) {
    if (context.__state.lost) return false;
    return true;
  }
  function webglOwned(context, value, constructor, nullable = false, allowDeleted = false) {
    if (value === null && nullable) return null;
    if (!(value instanceof constructor) || value.__context !== context || (!allowDeleted && value.__deleted)) {
      webglError(context, WEBGL_CONSTANTS.INVALID_OPERATION);
      return undefined;
    }
    return value;
  }
  function webglDeletable(context, value, constructor) {
    if (!(value instanceof constructor) || value.__context !== context) {
      webglError(context, WEBGL_CONSTANTS.INVALID_OPERATION);
      return undefined;
    }
    return value;
  }
  function webglSourceNames(source, keyword) {
    const names = [];
    const seen = new Set();
    const expression = new RegExp(
      "\\b" + keyword + "\\s+(?:(?:lowp|mediump|highp)\\s+)?\\w+\\s+(\\w+)(?:\\s*\\[[^\\]]*\\])?\\s*;",
      "g",
    );
    let match;
    while ((match = expression.exec(source))) {
      if (!seen.has(match[1])) { seen.add(match[1]); names.push(match[1]); }
    }
    return names;
  }

  class WebGLRenderingContext extends WebGLEventTarget {
    constructor(canvas, token) {
      if (token !== webglContextConstructionToken) throw new TypeError("Illegal constructor");
      super();
      this.canvas = canvas;
      this.__state = {
        context: this, lost: false, errors: [], clearColor: [0, 0, 0, 0],
        colorMask: [true, true, true, true], depthMask: true,
        stencilMask: 0xffffffff, clearDepth: 1, clearStencil: 0,
        viewport: [0, 0, canvasDimensions(canvas)[0], canvasDimensions(canvas)[1]],
        scissor: [0, 0, canvasDimensions(canvas)[0], canvasDimensions(canvas)[1]],
        enabled: new Set(), buffers: new Set(), shaders: new Set(), programs: new Set(),
        arrayBuffer: null, elementArrayBuffer: null, currentProgram: null,
        activeTexture: WEBGL_CONSTANTS.TEXTURE0,
      };
      this.__oncontextlost = null;
      this.__oncontextrestored = null;
    }
    get drawingBufferWidth() { return canvasDimensions(this.canvas)[0]; }
    get drawingBufferHeight() { return canvasDimensions(this.canvas)[1]; }
    get __s() { return this.__state; }
    __commit() { commitCanvas(this.canvas, canvasState(this.canvas)); }
    __lose() {
      if (this.__state.lost) return false;
      this.__state.lost = true;
      this.dispatchEvent(new Event("webglcontextlost", { cancelable: true }));
      if (this.canvas && typeof this.canvas.dispatchEvent === "function") {
        this.canvas.dispatchEvent(new Event("webglcontextlost", { cancelable: true }));
      }
      return true;
    }
    __restore() {
      if (!this.__state.lost) return false;
      this.__state.lost = false;
      this.__state.errors.length = 0;
      this.__state.clearColor = [0, 0, 0, 0];
      this.__state.clearDepth = 1;
      this.__state.clearStencil = 0;
      this.__state.activeTexture = WEBGL_CONSTANTS.TEXTURE0;
      this.__state.colorMask = [true, true, true, true];
      this.__state.depthMask = true;
      this.__state.stencilMask = 0xffffffff;
      this.__state.viewport = [0, 0, this.drawingBufferWidth, this.drawingBufferHeight];
      this.__state.scissor = [0, 0, this.drawingBufferWidth, this.drawingBufferHeight];
      this.__state.enabled.clear();
      for (const resource of [...this.__state.buffers, ...this.__state.shaders, ...this.__state.programs]) resource.__deleted = true;
      this.__state.buffers.clear();
      this.__state.shaders.clear();
      this.__state.programs.clear();
      this.__state.arrayBuffer = null;
      this.__state.elementArrayBuffer = null;
      this.__state.currentProgram = null;
      this.dispatchEvent(new Event("webglcontextrestored"));
      if (this.canvas && typeof this.canvas.dispatchEvent === "function") {
        this.canvas.dispatchEvent(new Event("webglcontextrestored"));
      }
      return true;
    }
    get oncontextlost() { return this.__oncontextlost || null; }
    set oncontextlost(callback) {
      if (this.__oncontextlost) this.removeEventListener("webglcontextlost", this.__oncontextlost);
      this.__oncontextlost = typeof callback === "function" ? callback : null;
      if (this.__oncontextlost) this.addEventListener("webglcontextlost", this.__oncontextlost);
    }
    get oncontextrestored() { return this.__oncontextrestored || null; }
    set oncontextrestored(callback) {
      if (this.__oncontextrestored) this.removeEventListener("webglcontextrestored", this.__oncontextrestored);
      this.__oncontextrestored = typeof callback === "function" ? callback : null;
      if (this.__oncontextrestored) this.addEventListener("webglcontextrestored", this.__oncontextrestored);
    }
    clearColor(red, green, blue, alpha) {
      if (!webglActive(this)) return;
      this.__state.clearColor = [webglClamp(red), webglClamp(green), webglClamp(blue), webglClamp(alpha)];
    }
    clearDepth(value) {
      if (!webglActive(this)) return;
      this.__state.clearDepth = webglClamp(value);
    }
    clearStencil(value) {
      if (!webglActive(this)) return;
      const number = Number(value);
      if (!Number.isFinite(number)) { webglError(this, WEBGL_CONSTANTS.INVALID_VALUE); return; }
      this.__state.clearStencil = number | 0;
    }
    colorMask(red, green, blue, alpha) {
      if (!webglActive(this)) return;
      this.__state.colorMask = [!!red, !!green, !!blue, !!alpha];
    }
    depthMask(value) { if (webglActive(this)) this.__state.depthMask = !!value; }
    stencilMask(value) {
      if (!webglActive(this)) return;
      const number = Number(value);
      if (!Number.isFinite(number)) { webglError(this, WEBGL_CONSTANTS.INVALID_VALUE); return; }
      this.__state.stencilMask = number >>> 0;
    }
    viewport(x, y, width, height) {
      if (!webglActive(this)) return;
      const values = [x, y, width, height].map(Number);
      if (values.some(value => !Number.isFinite(value) || Math.trunc(value) !== value) || values[2] < 0 || values[3] < 0) {
        webglError(this, WEBGL_CONSTANTS.INVALID_VALUE);
        return;
      }
      this.__state.viewport = values;
    }
    scissor(x, y, width, height) {
      if (!webglActive(this)) return;
      const values = [x, y, width, height].map(Number);
      if (values.some(value => !Number.isFinite(value) || Math.trunc(value) !== value) || values[2] < 0 || values[3] < 0) {
        webglError(this, WEBGL_CONSTANTS.INVALID_VALUE);
        return;
      }
      this.__state.scissor = values;
    }
    clear(mask) {
      if (!webglActive(this)) return;
      const bits = Number(mask);
      if (!Number.isFinite(bits) || (bits & WEBGL_CLEAR_BITS) !== bits) {
        webglError(this, WEBGL_CONSTANTS.INVALID_VALUE);
        return;
      }
      if (bits & WEBGL_CONSTANTS.COLOR_BUFFER_BIT) {
        const state = canvasState(this.canvas);
        const color = this.__state.clearColor.map(value => Math.round(value * 255));
        for (let index = 0; index < state.pixels.length; index += 4) {
          for (let channel = 0; channel < 4; channel++) {
            if (this.__state.colorMask[channel]) state.pixels[index + channel] = color[channel];
          }
        }
        this.__commit();
      }
    }
    enable(capability) {
      if (!webglActive(this)) return;
      if (!WEBGL_CAPABILITIES.includes(capability)) webglError(this, WEBGL_CONSTANTS.INVALID_ENUM);
      else this.__state.enabled.add(capability);
    }
    disable(capability) {
      if (!webglActive(this)) return;
      if (!WEBGL_CAPABILITIES.includes(capability)) webglError(this, WEBGL_CONSTANTS.INVALID_ENUM);
      else this.__state.enabled.delete(capability);
    }
    isEnabled(capability) {
      if (!webglActive(this)) return false;
      if (!WEBGL_CAPABILITIES.includes(capability)) { webglError(this, WEBGL_CONSTANTS.INVALID_ENUM); return false; }
      return this.__state.enabled.has(capability);
    }
    isContextLost() { return this.__state.lost; }
    getError() {
      if (this.__state.lost) return WEBGL_CONSTANTS.CONTEXT_LOST_WEBGL;
      return this.__state.errors.shift() || WEBGL_CONSTANTS.NO_ERROR;
    }
    getParameter(parameter) {
      if (!webglActive(this)) return null;
      parameter = Number(parameter);
      switch (parameter) {
        case WEBGL_CONSTANTS.VIEWPORT: return new Int32Array(this.__state.viewport);
        case WEBGL_CONSTANTS.SCISSOR_BOX: return new Int32Array(this.__state.scissor);
        case WEBGL_CONSTANTS.COLOR_CLEAR_VALUE: return new Float32Array(this.__state.clearColor);
        case WEBGL_CONSTANTS.COLOR_WRITEMASK: return this.__state.colorMask.slice();
        case WEBGL_CONSTANTS.DEPTH_CLEAR_VALUE: return this.__state.clearDepth;
        case WEBGL_CONSTANTS.DEPTH_WRITEMASK: return this.__state.depthMask;
        case WEBGL_CONSTANTS.STENCIL_CLEAR_VALUE: return this.__state.clearStencil;
        case WEBGL_CONSTANTS.STENCIL_WRITEMASK: return this.__state.stencilMask;
        case WEBGL_CONSTANTS.STENCIL_BACK_WRITEMASK: return this.__state.stencilMask;
        case WEBGL_CONSTANTS.ARRAY_BUFFER_BINDING: return this.__state.arrayBuffer && !this.__state.arrayBuffer.__deleted ? this.__state.arrayBuffer : null;
        case WEBGL_CONSTANTS.ELEMENT_ARRAY_BUFFER_BINDING: return this.__state.elementArrayBuffer && !this.__state.elementArrayBuffer.__deleted ? this.__state.elementArrayBuffer : null;
        case WEBGL_CONSTANTS.CURRENT_PROGRAM: return this.__state.currentProgram && !this.__state.currentProgram.__deleted ? this.__state.currentProgram : null;
        case WEBGL_CONSTANTS.ACTIVE_TEXTURE: return this.__state.activeTexture;
        case WEBGL_CONSTANTS.MAX_TEXTURE_SIZE:
        case WEBGL_CONSTANTS.MAX_CUBE_MAP_TEXTURE_SIZE: return 4096;
        case WEBGL_CONSTANTS.MAX_VIEWPORT_DIMS: return new Int32Array([32768, 32768]);
        case WEBGL_CONSTANTS.MAX_VERTEX_ATTRIBS: return 8;
        case WEBGL_CONSTANTS.MAX_VERTEX_UNIFORM_VECTORS: return 128;
        case WEBGL_CONSTANTS.MAX_VARYING_VECTORS: return 8;
        case WEBGL_CONSTANTS.MAX_COMBINED_TEXTURE_IMAGE_UNITS: return 8;
        case WEBGL_CONSTANTS.MAX_VERTEX_TEXTURE_IMAGE_UNITS: return 0;
        case WEBGL_CONSTANTS.MAX_TEXTURE_IMAGE_UNITS: return 8;
        case WEBGL_CONSTANTS.MAX_FRAGMENT_UNIFORM_VECTORS: return 16;
        case WEBGL_CONSTANTS.RED_BITS:
        case WEBGL_CONSTANTS.GREEN_BITS:
        case WEBGL_CONSTANTS.BLUE_BITS:
        case WEBGL_CONSTANTS.ALPHA_BITS: return 8;
        case WEBGL_CONSTANTS.DEPTH_BITS: return 24;
        case WEBGL_CONSTANTS.STENCIL_BITS: return 8;
        case WEBGL_CONSTANTS.SAMPLE_BUFFERS:
        case WEBGL_CONSTANTS.SAMPLES: return 0;
        case WEBGL_CONSTANTS.SUBPIXEL_BITS: return 4;
        case WEBGL_CONSTANTS.RENDERER: return "Omoikane Software WebGL";
        case WEBGL_CONSTANTS.VENDOR: return "Omoikane";
        case WEBGL_CONSTANTS.VERSION: return "WebGL 1.0 Omoikane";
        case WEBGL_CONSTANTS.SHADING_LANGUAGE_VERSION: return "WebGL GLSL ES 1.0 Omoikane";
        case WEBGL_CONSTANTS.BLEND:
        case WEBGL_CONSTANTS.CULL_FACE:
        case WEBGL_CONSTANTS.DEPTH_TEST:
        case WEBGL_CONSTANTS.DITHER:
        case WEBGL_CONSTANTS.POLYGON_OFFSET_FILL:
        case WEBGL_CONSTANTS.SAMPLE_ALPHA_TO_COVERAGE:
        case WEBGL_CONSTANTS.SAMPLE_COVERAGE:
        case WEBGL_CONSTANTS.SCISSOR_TEST:
        case WEBGL_CONSTANTS.STENCIL_TEST: return this.__state.enabled.has(parameter);
        default:
          webglError(this, WEBGL_CONSTANTS.INVALID_ENUM);
          return null;
      }
    }
    createBuffer() {
      if (!webglActive(this)) return null;
      const buffer = new WebGLBuffer(this, webglResourceConstructionToken);
      this.__state.buffers.add(buffer);
      return buffer;
    }
    deleteBuffer(buffer) {
      if (buffer === null) return;
      const value = webglDeletable(this, buffer, WebGLBuffer);
      if (!value) return;
      if (value.__deleted) return;
      value.__deleted = true;
      this.__state.buffers.delete(value);
      if (this.__state.arrayBuffer === value) this.__state.arrayBuffer = null;
      if (this.__state.elementArrayBuffer === value) this.__state.elementArrayBuffer = null;
    }
    bindBuffer(target, buffer) {
      if (!webglActive(this)) return;
      if (!WEBGL_BUFFER_TARGETS.includes(target)) { webglError(this, WEBGL_CONSTANTS.INVALID_ENUM); return; }
      if (buffer !== null && webglOwned(this, buffer, WebGLBuffer) === undefined) return;
      if (target === WEBGL_CONSTANTS.ARRAY_BUFFER) this.__state.arrayBuffer = buffer;
      else this.__state.elementArrayBuffer = buffer;
    }
    bufferData(target, dataOrSize, usage) {
      if (!webglActive(this)) return;
      if (!WEBGL_BUFFER_TARGETS.includes(target)) { webglError(this, WEBGL_CONSTANTS.INVALID_ENUM); return; }
      if (![WEBGL_CONSTANTS.STREAM_DRAW, WEBGL_CONSTANTS.STATIC_DRAW, WEBGL_CONSTANTS.DYNAMIC_DRAW].includes(usage)) { webglError(this, WEBGL_CONSTANTS.INVALID_ENUM); return; }
      const buffer = target === WEBGL_CONSTANTS.ARRAY_BUFFER ? this.__state.arrayBuffer : this.__state.elementArrayBuffer;
      if (!buffer) { webglError(this, WEBGL_CONSTANTS.INVALID_OPERATION); return; }
      let size;
      if (typeof dataOrSize === "number") size = Number(dataOrSize);
      else if (dataOrSize instanceof ArrayBuffer || ArrayBuffer.isView(dataOrSize)) size = dataOrSize.byteLength;
      else { webglError(this, WEBGL_CONSTANTS.INVALID_VALUE); return; }
      if (!Number.isFinite(size) || Math.trunc(size) !== size || size < 0) { webglError(this, WEBGL_CONSTANTS.INVALID_VALUE); return; }
      buffer.__size = size;
      buffer.__usage = usage;
    }
    bufferSubData(target, offset, data) {
      if (!webglActive(this)) return;
      if (!WEBGL_BUFFER_TARGETS.includes(target)) { webglError(this, WEBGL_CONSTANTS.INVALID_ENUM); return; }
      const buffer = target === WEBGL_CONSTANTS.ARRAY_BUFFER ? this.__state.arrayBuffer : this.__state.elementArrayBuffer;
      if (!buffer) { webglError(this, WEBGL_CONSTANTS.INVALID_OPERATION); return; }
      const start = Number(offset);
      const size = data instanceof ArrayBuffer || ArrayBuffer.isView(data) ? data.byteLength : NaN;
      if (!Number.isFinite(start) || Math.trunc(start) !== start || start < 0 || !Number.isFinite(size) || start + size > buffer.__size) webglError(this, WEBGL_CONSTANTS.INVALID_VALUE);
    }
    getBufferParameter(target, parameter) {
      if (!webglActive(this)) return null;
      if (!WEBGL_BUFFER_TARGETS.includes(target)) { webglError(this, WEBGL_CONSTANTS.INVALID_ENUM); return null; }
      const buffer = target === WEBGL_CONSTANTS.ARRAY_BUFFER ? this.__state.arrayBuffer : this.__state.elementArrayBuffer;
      if (!buffer) { webglError(this, WEBGL_CONSTANTS.INVALID_OPERATION); return null; }
      if (parameter === WEBGL_CONSTANTS.BUFFER_SIZE) return buffer.__size;
      if (parameter === WEBGL_CONSTANTS.BUFFER_USAGE) return buffer.__usage;
      webglError(this, WEBGL_CONSTANTS.INVALID_ENUM);
      return null;
    }
    createShader(type) {
      if (!webglActive(this)) return null;
      if (type !== WEBGL_CONSTANTS.VERTEX_SHADER && type !== WEBGL_CONSTANTS.FRAGMENT_SHADER) { webglError(this, WEBGL_CONSTANTS.INVALID_ENUM); return null; }
      const shader = new WebGLShader(this, type, webglResourceConstructionToken);
      this.__state.shaders.add(shader);
      return shader;
    }
    shaderSource(shader, source) {
      if (!webglActive(this)) return;
      const value = webglOwned(this, shader, WebGLShader);
      if (!value) return;
      value.__source = String(source);
      value.__compiled = false;
      value.__infoLog = "";
    }
    compileShader(shader) {
      if (!webglActive(this)) return;
      const value = webglOwned(this, shader, WebGLShader);
      if (!value) return;
      const source = value.__source;
      value.__compiled = /\bvoid\s+main\s*\(/.test(source) && !/\b(?:compile_fail|syntax_error|error)\b/i.test(source);
      value.__infoLog = value.__compiled ? "" : "deterministic shader validation failed";
    }
    getShaderParameter(shader, parameter) {
      if (!webglActive(this)) return null;
      const value = webglOwned(this, shader, WebGLShader, false, true);
      if (!value) return null;
      if (parameter === WEBGL_CONSTANTS.COMPILE_STATUS) return value.__compiled;
      if (parameter === WEBGL_CONSTANTS.DELETE_STATUS) return value.__deleted;
      if (parameter === WEBGL_CONSTANTS.SHADER_TYPE) return value.__type;
      webglError(this, WEBGL_CONSTANTS.INVALID_ENUM);
      return null;
    }
    getShaderInfoLog(shader) {
      if (!webglActive(this)) return null;
      const value = webglOwned(this, shader, WebGLShader, false, true);
      return value ? value.__infoLog : null;
    }
    getShaderSource(shader) {
      if (!webglActive(this)) return null;
      const value = webglOwned(this, shader, WebGLShader, false, true);
      return value ? value.__source : null;
    }
    deleteShader(shader) {
      if (shader === null) return;
      const value = webglDeletable(this, shader, WebGLShader);
      if (!value) return;
      if (value.__deleted) return;
      value.__deleted = true;
      this.__state.shaders.delete(value);
    }
    createProgram() {
      if (!webglActive(this)) return null;
      const program = new WebGLProgram(this, webglResourceConstructionToken);
      this.__state.programs.add(program);
      return program;
    }
    attachShader(program, shader) {
      if (!webglActive(this)) return;
      const target = webglOwned(this, program, WebGLProgram);
      const source = webglOwned(this, shader, WebGLShader);
      if (!target || !source || target.__shaders.includes(source)) return;
      target.__shaders.push(source);
      target.__linked = false;
    }
    detachShader(program, shader) {
      if (!webglActive(this)) return;
      const target = webglOwned(this, program, WebGLProgram);
      const source = webglOwned(this, shader, WebGLShader);
      if (!target || !source) return;
      target.__shaders = target.__shaders.filter(value => value !== source);
      target.__linked = false;
    }
    linkProgram(program) {
      if (!webglActive(this)) return;
      const value = webglOwned(this, program, WebGLProgram);
      if (!value) return;
      const vertex = value.__shaders.find(shader => shader.__type === WEBGL_CONSTANTS.VERTEX_SHADER);
      const fragment = value.__shaders.find(shader => shader.__type === WEBGL_CONSTANTS.FRAGMENT_SHADER);
      value.__linked = !!vertex && !!fragment && vertex.__compiled && fragment.__compiled;
      value.__infoLog = value.__linked ? "" : "deterministic program validation failed";
      value.__attributes = new Map();
      value.__uniforms = new Map();
      if (value.__linked) {
        const names = [];
        const attributes = new Set();
        const uniforms = new Set();
        for (const shader of value.__shaders) {
          for (const name of webglSourceNames(shader.__source, "attribute")) {
            if (!attributes.has(name)) { attributes.add(name); names.push(name); }
          }
          for (const name of webglSourceNames(shader.__source, "uniform")) {
            if (!uniforms.has(name)) { uniforms.add(name); names.push(name); }
          }
        }
        let attributeIndex = 0;
        for (const name of names) {
          if (attributes.has(name)) value.__attributes.set(name, attributeIndex++);
          if (uniforms.has(name)) value.__uniforms.set(name, new WebGLUniformLocation(value, name, webglResourceConstructionToken));
        }
      }
    }
    getProgramParameter(program, parameter) {
      if (!webglActive(this)) return null;
      const value = webglOwned(this, program, WebGLProgram, false, true);
      if (!value) return null;
      if (parameter === WEBGL_CONSTANTS.LINK_STATUS || parameter === WEBGL_CONSTANTS.VALIDATE_STATUS) return value.__linked;
      if (parameter === WEBGL_CONSTANTS.DELETE_STATUS) return value.__deleted;
      if (parameter === WEBGL_CONSTANTS.ATTACHED_SHADERS) return value.__shaders.length;
      if (parameter === WEBGL_CONSTANTS.ACTIVE_ATTRIBUTES) return value.__attributes.size;
      if (parameter === WEBGL_CONSTANTS.ACTIVE_UNIFORMS) return value.__uniforms.size;
      webglError(this, WEBGL_CONSTANTS.INVALID_ENUM);
      return null;
    }
    getProgramInfoLog(program) {
      if (!webglActive(this)) return null;
      const value = webglOwned(this, program, WebGLProgram, false, true);
      return value ? value.__infoLog : null;
    }
    deleteProgram(program) {
      if (program === null) return;
      const value = webglDeletable(this, program, WebGLProgram);
      if (!value) return;
      if (value.__deleted) return;
      value.__deleted = true;
      this.__state.programs.delete(value);
      if (this.__state.currentProgram === value) this.__state.currentProgram = null;
    }
    useProgram(program) {
      if (!webglActive(this)) return;
      if (program === null) { this.__state.currentProgram = null; return; }
      const value = webglOwned(this, program, WebGLProgram);
      if (!value) return;
      if (!value.__linked) { webglError(this, WEBGL_CONSTANTS.INVALID_OPERATION); return; }
      this.__state.currentProgram = value;
    }
    getAttribLocation(program, name) {
      if (!webglActive(this)) return -1;
      const value = webglOwned(this, program, WebGLProgram);
      if (!value) return -1;
      if (!value.__linked) { webglError(this, WEBGL_CONSTANTS.INVALID_OPERATION); return -1; }
      return value.__attributes.has(String(name)) ? value.__attributes.get(String(name)) : -1;
    }
    getUniformLocation(program, name) {
      if (!webglActive(this)) return null;
      const value = webglOwned(this, program, WebGLProgram);
      if (!value) return null;
      if (!value.__linked) { webglError(this, WEBGL_CONSTANTS.INVALID_OPERATION); return null; }
      return value.__uniforms.get(String(name)) || null;
    }
    getActiveAttrib(program, index) {
      if (!webglActive(this)) return null;
      const value = webglOwned(this, program, WebGLProgram);
      if (!value || !value.__linked) return null;
      const entries = Array.from(value.__attributes.entries());
      const entry = entries[Number(index)];
      return entry ? { name: entry[0], size: 1, type: WEBGL_CONSTANTS.FLOAT } : null;
    }
    getActiveUniform(program, index) {
      if (!webglActive(this)) return null;
      const value = webglOwned(this, program, WebGLProgram);
      if (!value || !value.__linked) return null;
      const entries = Array.from(value.__uniforms.keys());
      const name = entries[Number(index)];
      return name === undefined ? null : { name, size: 1, type: WEBGL_CONSTANTS.FLOAT };
    }
    getAttachedShaders(program) {
      if (!webglActive(this)) return null;
      const value = webglOwned(this, program, WebGLProgram);
      return value ? value.__shaders.slice() : null;
    }
    drawArrays(mode, first, count) {
      if (!webglActive(this)) return;
      if (!WEBGL_DRAW_MODES.includes(mode)) { webglError(this, WEBGL_CONSTANTS.INVALID_ENUM); return; }
      if (!Number.isFinite(Number(first)) || !Number.isFinite(Number(count)) || Number(first) < 0 || Number(count) < 0 || Math.trunc(Number(first)) !== Number(first) || Math.trunc(Number(count)) !== Number(count)) { webglError(this, WEBGL_CONSTANTS.INVALID_VALUE); return; }
      if (!this.__state.currentProgram) webglError(this, WEBGL_CONSTANTS.INVALID_OPERATION);
    }
    drawElements(mode, count, type, offset) {
      if (!webglActive(this)) return;
      if (!WEBGL_DRAW_MODES.includes(mode)) { webglError(this, WEBGL_CONSTANTS.INVALID_ENUM); return; }
      if (![WEBGL_CONSTANTS.UNSIGNED_BYTE, WEBGL_CONSTANTS.UNSIGNED_SHORT].includes(type)) { webglError(this, WEBGL_CONSTANTS.INVALID_ENUM); return; }
      if (!Number.isFinite(Number(count)) || Number(count) < 0 || !Number.isFinite(Number(offset)) || Number(offset) < 0) { webglError(this, WEBGL_CONSTANTS.INVALID_VALUE); return; }
      if (!this.__state.currentProgram || !this.__state.elementArrayBuffer) webglError(this, WEBGL_CONSTANTS.INVALID_OPERATION);
    }
    readPixels(x, y, width, height, format, type, pixels) {
      if (!webglActive(this)) return;
      const values = [x, y, width, height].map(Number);
      if (values.some(value => !Number.isFinite(value) || Math.trunc(value) !== value) || values[2] < 0 || values[3] < 0) {
        webglError(this, WEBGL_CONSTANTS.INVALID_VALUE);
        return;
      }
      if (format !== WEBGL_CONSTANTS.RGBA || type !== WEBGL_CONSTANTS.UNSIGNED_BYTE) {
        webglError(this, WEBGL_CONSTANTS.INVALID_ENUM);
        return;
      }
      if (!ArrayBuffer.isView(pixels)) {
        webglError(this, WEBGL_CONSTANTS.INVALID_OPERATION);
        return;
      }
      const required = values[2] * values[3] * 4;
      if (pixels.byteLength < required) {
        webglError(this, WEBGL_CONSTANTS.INVALID_OPERATION);
        return;
      }
      const state = canvasState(this.canvas);
      if (values[0] < 0 || values[1] < 0 || values[0] + values[2] > state.width || values[1] + values[3] > state.height) {
        webglError(this, WEBGL_CONSTANTS.INVALID_VALUE);
        return;
      }
      const bytes = new Uint8Array(pixels.buffer, pixels.byteOffset, pixels.byteLength);
      for (let row = 0; row < values[3]; row++) {
        for (let column = 0; column < values[2]; column++) {
          const sourceX = values[0] + column;
          const sourceY = values[1] + (values[3] - row - 1);
          const target = (row * values[2] + column) * 4;
          const source = (sourceY * state.width + sourceX) * 4;
          bytes.set(state.pixels.subarray(source, source + 4), target);
        }
      }
    }
    getExtension() { return null; }
    getSupportedExtensions() { return this.__state.lost ? null : []; }
  }
  for (const [name, value] of Object.entries(WEBGL_CONSTANTS)) {
    Object.defineProperty(WebGLRenderingContext, name, { value, writable: false, enumerable: false, configurable: false });
    Object.defineProperty(WebGLRenderingContext.prototype, name, { value, writable: false, enumerable: false, configurable: false });
  }

  function resetHtmlCanvasState(canvas) {
    const previous = canvasStates.get(canvas);
    canvasStates.delete(canvas);
    const next = canvasState(canvas);
    if (!previous) return;
    // Resizing resets the backing store but does not release the canvas's
    // context mode. Preserve the context identity so a subsequent getContext
    // call remains exclusive to its original API.
    next.contextMode = previous.contextMode;
    next.context = previous.context;
    next.webgl = previous.webgl;
  }

  class HTMLCanvasElement extends HTMLElement {
    get width(){return canvasDimensions(this)[0];} set width(value){this.setAttribute("width",String(Math.max(0,Math.trunc(Number(value))||0)));resetHtmlCanvasState(this);}
    get height(){return canvasDimensions(this)[1];} set height(value){this.setAttribute("height",String(Math.max(0,Math.trunc(Number(value))||0)));resetHtmlCanvasState(this);}
    getContext(type){
      const requested=String(type).toLowerCase(),s=canvasState(this);
      if(requested==="2d"){
        if(s.contextMode&&s.contextMode!=="2d")return null;
        s.contextMode="2d";
        return s.context||(s.context=new CanvasRenderingContext2D(this));
      }
      if(requested==="webgl"||requested==="experimental-webgl"){
        if(s.contextMode&&s.contextMode!=="webgl")return null;
        s.contextMode="webgl";
        return s.webgl||(s.webgl=new WebGLRenderingContext(this,webglContextConstructionToken));
      }
      return null;
    }
    toDataURL(type="image/png"){canvasState(this);return String(type).toLowerCase()==="image/png"?__omoikane_canvas_data_url(this.__id):__omoikane_canvas_data_url(this.__id);}
  }

  const nativeCanvasPng = globalThis.__omoikane_canvas_png;
  try { delete globalThis.__omoikane_canvas_png; } catch (_) {}
  const imageBitmapConstructionToken = {};
  function offscreenDimension(value, name) {
    const number = Number(value);
    if (!Number.isFinite(number) || Math.trunc(number) !== number || number < 0 || number > 32768) {
      throw new DOMException("The " + name + " dimension is invalid", "IndexSizeError");
    }
    return number;
  }
  function canvasStateChecked(canvas) {
    try {
      return canvasState(canvas);
    } catch (error) {
      if (error instanceof RangeError) {
        throw new DOMException("Canvas dimensions are too large", "IndexSizeError");
      }
      throw error;
    }
  }
  function canvasSnapshot(canvas) {
    try {
      const state = canvasStateChecked(canvas);
      return { width: state.width, height: state.height, pixels: state.pixels.slice() };
    } catch (error) {
      if (error instanceof RangeError) {
        throw new DOMException("Canvas dimensions are too large", "IndexSizeError");
      }
      throw error;
    }
  }
  function imageBitmapSource(source) {
    if (source instanceof ImageBitmap) {
      if (source.__detached) throw new DOMException("The ImageBitmap is detached", "InvalidStateError");
      return { width: source.width, height: source.height, pixels: source.__pixels };
    }
    const isHtmlCanvas = typeof HTMLCanvasElement !== "undefined" && source instanceof HTMLCanvasElement;
    if (isHtmlCanvas || source instanceof OffscreenCanvas) return canvasSnapshot(source);
    const raw = source && source.__id != null ? __omoikane_canvas_image_source(source.__id) : null;
    if (raw === null) throw new DOMException("Image source is unavailable", "InvalidStateError");
    const decoded = JSON.parse(raw);
    return { width: decoded.width, height: decoded.height, pixels: bytesFromBase64(decoded.pixels) };
  }

  class ImageBitmap {
    constructor(token, width, height, pixels) {
      if (token !== imageBitmapConstructionToken) throw new TypeError("Illegal constructor");
      this.__width = width;
      this.__height = height;
      this.__pixels = pixels;
      this.__detached = false;
    }
    get width() { return this.__detached ? 0 : this.__width; }
    get height() { return this.__detached ? 0 : this.__height; }
    close() { this.__detached = true; this.__width = 0; this.__height = 0; this.__pixels = new Uint8ClampedArray(0); }
    get [Symbol.toStringTag]() { return "ImageBitmap"; }
  }

  class OffscreenCanvasRenderingContext2D extends CanvasRenderingContext2D {
    get [Symbol.toStringTag]() { return "OffscreenCanvasRenderingContext2D"; }
  }

  class OffscreenCanvas {
    constructor(width, height) {
      this.__width = offscreenDimension(width, "width");
      this.__height = offscreenDimension(height, "height");
      this.__contextMode = null;
      this.__context = null;
      this.__detached = false;
    }
    get width() { return this.__detached ? 0 : this.__width; }
    set width(value) { this.__resize("width", value); }
    get height() { return this.__detached ? 0 : this.__height; }
    set height(value) { this.__resize("height", value); }
    __resize(name, value) {
      if (this.__detached) throw new DOMException("The OffscreenCanvas is detached", "InvalidStateError");
      const property = "__" + name;
      const previous = this[property];
      this[property] = offscreenDimension(value, name);
      canvasStates.delete(this);
      try {
        canvasStateChecked(this);
      } catch (error) {
        this[property] = previous;
        canvasStates.delete(this);
        throw error;
      }
    }
    getContext(type, options = undefined) {
      if (this.__detached) throw new DOMException("The OffscreenCanvas is detached", "InvalidStateError");
      const requested = String(type).toLowerCase();
      if (requested !== "2d") return null;
      if (this.__contextMode !== null && this.__contextMode !== requested) return null;
      this.__contextMode = requested;
      const state = canvasStateChecked(this);
      if (!this.__context) this.__context = new OffscreenCanvasRenderingContext2D(this, options);
      state.context = this.__context;
      return this.__context;
    }
    transferToImageBitmap() {
      if (this.__detached) throw new DOMException("The OffscreenCanvas is detached", "InvalidStateError");
      const snapshot = canvasSnapshot(this);
      return new ImageBitmap(imageBitmapConstructionToken, snapshot.width, snapshot.height, snapshot.pixels);
    }
    convertToBlob(options = {}) {
      if (this.__detached) return Promise.reject(new DOMException("The OffscreenCanvas is detached", "InvalidStateError"));
      const requested = String(options && options.type || "image/png").toLowerCase();
      const type = requested === "image/png" ? requested : "image/png";
      const snapshot = canvasSnapshot(this);
      if (snapshot.width === 0 || snapshot.height === 0) return Promise.resolve(new Blob([], { type }));
      const encodingError = () => Promise.reject(new DOMException("Unable to encode canvas", "EncodingError"));
      if (typeof nativeCanvasPng !== "function") return encodingError();
      let encoded;
      try {
        encoded = nativeCanvasPng(snapshot.width, snapshot.height, new Uint8Array(snapshot.pixels.buffer));
      } catch (_) {
        return encodingError();
      }
      if (typeof encoded !== "string") return encodingError();
      const comma = encoded.indexOf(",");
      if (comma < 0) return encodingError();
      try {
        return Promise.resolve(new Blob([bytesFromBase64(encoded.slice(comma + 1))], { type }));
      } catch (_) {
        return encodingError();
      }
    }
    get [Symbol.toStringTag]() { return "OffscreenCanvas"; }
  }

  globalThis.createImageBitmap = function createImageBitmap(source, sx = 0, sy = 0, sw = undefined, sh = undefined) {
    try {
      const input = imageBitmapSource(source);
      const cropX = Math.trunc(Number(sx));
      const cropY = Math.trunc(Number(sy));
      const cropWidth = sw === undefined ? input.width - cropX : Math.trunc(Number(sw));
      const cropHeight = sh === undefined ? input.height - cropY : Math.trunc(Number(sh));
      if (!Number.isFinite(cropX) || !Number.isFinite(cropY) || !Number.isFinite(cropWidth) || !Number.isFinite(cropHeight) || cropX < 0 || cropY < 0 || cropWidth <= 0 || cropHeight <= 0 || cropX + cropWidth > input.width || cropY + cropHeight > input.height) {
        throw new DOMException("The crop rectangle is outside the image", "IndexSizeError");
      }
      const pixels = new Uint8ClampedArray(cropWidth * cropHeight * 4);
      for (let y = 0; y < cropHeight; y++) {
        const from = ((cropY + y) * input.width + cropX) * 4;
        pixels.set(input.pixels.slice(from, from + cropWidth * 4), y * cropWidth * 4);
      }
      return Promise.resolve(new ImageBitmap(imageBitmapConstructionToken, cropWidth, cropHeight, pixels));
    } catch (error) {
      return Promise.reject(error instanceof RangeError
        ? new DOMException("Canvas dimensions are too large", "IndexSizeError")
        : error);
    }
  };

  class HTMLLinkElement extends HTMLElement {
    setAttribute(name, value) {
      const attribute = String(name).toLowerCase();
      const isHref = attribute === "href";
      const isRel = attribute === "rel";
      if (isHref) noteElementResourceStart(this);
      super.setAttribute(name, value);
      const href = this.getAttribute("href");
      if ((isHref || isRel) &&
          (this.relList.contains("stylesheet") || this.relList.contains("preload")) &&
          /^(?:data:|blob:)/i.test(String(href || ""))) {
        finishElementResourceTiming(this, 200, false);
      }
    }
    get rel() {
      return this.getAttribute("rel") || "";
    }
    set rel(value) {
      this.setAttribute("rel", String(value));
    }
    get href() {
      const raw = this.getAttribute("href");
      return raw === null ? "" : __omoikane_resolve_url(raw);
    }
    set href(value) {
      this.setAttribute("href", String(value));
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

  // SVG DOM layer. Painting remains owned by src/svg, while these wrappers
  // expose the deterministic geometry interfaces used by scripts.  The
  // native DOM deliberately keeps no layout-specific SVG state, so geometry
  // is derived from the element attributes and the SVG ancestor chain here.
  function finiteSvgNumber(value, fallback = 0) {
    const number = Number(value);
    return Number.isFinite(number) ? number : fallback;
  }

  function svgAttributeNumber(element, name, fallback = 0) {
    const value = element && element.getAttribute(name);
    if (value === null || String(value).trim() === "") return fallback;
    const number = Number.parseFloat(String(value));
    return Number.isFinite(number) ? number : fallback;
  }

  function svgNumberList(value) {
    if (value === null || value === undefined) return [];
    const numbers = String(value).match(/[+-]?(?:\d*\.\d+|\d+\.?)(?:[eE][+-]?\d+)?/g);
    return numbers ? numbers.map(number => Number(number)).filter(Number.isFinite) : [];
  }

  function svgRectBounds(points) {
    if (!points || points.length === 0) return new SVGRect();
    let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
    for (const point of points) {
      const x = finiteSvgNumber(point[0]);
      const y = finiteSvgNumber(point[1]);
      minX = Math.min(minX, x);
      minY = Math.min(minY, y);
      maxX = Math.max(maxX, x);
      maxY = Math.max(maxY, y);
    }
    return new SVGRect(minX, minY, Math.max(0, maxX - minX), Math.max(0, maxY - minY));
  }

  function svgUnionRect(left, right) {
    if (!left) return right;
    if (!right) return left;
    const minX = Math.min(left.x, right.x);
    const minY = Math.min(left.y, right.y);
    const maxX = Math.max(left.x + left.width, right.x + right.width);
    const maxY = Math.max(left.y + left.height, right.y + right.height);
    return new SVGRect(minX, minY, Math.max(0, maxX - minX), Math.max(0, maxY - minY));
  }

  class SVGRect {
    constructor(x = 0, y = 0, width = 0, height = 0) {
      this.x = finiteSvgNumber(x);
      this.y = finiteSvgNumber(y);
      this.width = Math.max(0, finiteSvgNumber(width));
      this.height = Math.max(0, finiteSvgNumber(height));
    }
    get [Symbol.toStringTag]() { return "SVGRect"; }
  }

  class SVGPoint {
    constructor(x = 0, y = 0) {
      this.x = finiteSvgNumber(x);
      this.y = finiteSvgNumber(y);
    }
    matrixTransform(matrix) {
      const m = svgMatrixFrom(matrix);
      return new DOMPoint(
        m.a * this.x + m.c * this.y + m.e,
        m.b * this.x + m.d * this.y + m.f,
      );
    }
    get [Symbol.toStringTag]() { return "SVGPoint"; }
  }

  class DOMPoint extends SVGPoint {
    get [Symbol.toStringTag]() { return "DOMPoint"; }
  }

  function svgMatrixFrom(value) {
    if (value instanceof DOMMatrix) return value;
    if (value && typeof value === "object") {
      return new DOMMatrix([
        finiteSvgNumber(value.a, 1), finiteSvgNumber(value.b),
        finiteSvgNumber(value.c), finiteSvgNumber(value.d, 1),
        finiteSvgNumber(value.e), finiteSvgNumber(value.f),
      ]);
    }
    return new DOMMatrix(value);
  }

  function multiplySvgMatrices(left, right) {
    return [
      left.a * right.a + left.c * right.b,
      left.b * right.a + left.d * right.b,
      left.a * right.c + left.c * right.d,
      left.b * right.c + left.d * right.d,
      left.a * right.e + left.c * right.f + left.e,
      left.b * right.e + left.d * right.f + left.f,
    ];
  }

  class DOMMatrix {
    constructor(init) {
      let values;
      if (init === undefined || init === null) {
        values = [1, 0, 0, 1, 0, 0];
      } else if (typeof init === "string") {
        values = svgTransformMatrix(init).toArray();
      } else if (Array.isArray(init) || ArrayBuffer.isView(init)) {
        values = Array.from(init).map(value => finiteSvgNumber(value));
        if (values.length === 16) {
          values = [values[0], values[1], values[4], values[5], values[12], values[13]];
        }
        if (values.length < 6) values = [1, 0, 0, 1, 0, 0];
      } else if (typeof init === "object") {
        values = [
          finiteSvgNumber(init.a, 1), finiteSvgNumber(init.b),
          finiteSvgNumber(init.c), finiteSvgNumber(init.d, 1),
          finiteSvgNumber(init.e), finiteSvgNumber(init.f),
        ];
      } else {
        values = [1, 0, 0, 1, 0, 0];
      }
      [this.a, this.b, this.c, this.d, this.e, this.f] = values.slice(0, 6);
    }

    get m11() { return this.a; }
    set m11(value) { this.a = finiteSvgNumber(value); }
    get m12() { return this.b; }
    set m12(value) { this.b = finiteSvgNumber(value); }
    get m21() { return this.c; }
    set m21(value) { this.c = finiteSvgNumber(value); }
    get m22() { return this.d; }
    set m22(value) { this.d = finiteSvgNumber(value); }
    get m41() { return this.e; }
    set m41(value) { this.e = finiteSvgNumber(value); }
    get m42() { return this.f; }
    set m42(value) { this.f = finiteSvgNumber(value); }
    get is2D() { return true; }
    get isIdentity() {
      return this.a === 1 && this.b === 0 && this.c === 0 && this.d === 1 &&
        this.e === 0 && this.f === 0;
    }
    multiply(other) {
      const rhs = svgMatrixFrom(other);
      return new DOMMatrix(multiplySvgMatrices(this, rhs));
    }
    translate(tx, ty = 0) {
      return this.multiply(new DOMMatrix([1, 0, 0, 1, finiteSvgNumber(tx), finiteSvgNumber(ty)]));
    }
    scale(scaleX, scaleY = scaleX) {
      return this.multiply(new DOMMatrix([
        finiteSvgNumber(scaleX), 0, 0, finiteSvgNumber(scaleY), 0, 0,
      ]));
    }
    rotate(angle = 0) {
      const radians = finiteSvgNumber(angle) * Math.PI / 180;
      const cosine = Math.cos(radians), sine = Math.sin(radians);
      return this.multiply(new DOMMatrix([cosine, sine, -sine, cosine, 0, 0]));
    }
    inverse() {
      const determinant = this.a * this.d - this.b * this.c;
      if (determinant === 0) {
        throw new DOMException("The matrix is not invertible.", "InvalidStateError");
      }
      return new DOMMatrix([
        this.d / determinant, -this.b / determinant,
        -this.c / determinant, this.a / determinant,
        (this.c * this.f - this.d * this.e) / determinant,
        (this.b * this.e - this.a * this.f) / determinant,
      ]);
    }
    transformPoint(point) {
      return new DOMPoint(
        this.a * finiteSvgNumber(point && point.x) + this.c * finiteSvgNumber(point && point.y) + this.e,
        this.b * finiteSvgNumber(point && point.x) + this.d * finiteSvgNumber(point && point.y) + this.f,
      );
    }
    toArray() { return [this.a, this.b, this.c, this.d, this.e, this.f]; }
    get [Symbol.toStringTag]() { return "DOMMatrix"; }
  }

  class SVGMatrix extends DOMMatrix {
    get [Symbol.toStringTag]() { return "SVGMatrix"; }
  }

  function svgTransformMatrix(value) {
    const result = new DOMMatrix();
    const source = String(value || "");
    const expression = /([a-zA-Z]+)\s*\(([^)]*)\)/g;
    let match;
    while ((match = expression.exec(source))) {
      const name = match[1].toLowerCase();
      const values = svgNumberList(match[2]);
      let operation = null;
      if (name === "matrix" && values.length >= 6) {
        operation = new DOMMatrix(values.slice(0, 6));
      } else if (name === "translate" && values.length >= 1) {
        operation = new DOMMatrix([1, 0, 0, 1, values[0], values[1] || 0]);
      } else if (name === "scale" && values.length >= 1) {
        operation = new DOMMatrix([values[0], 0, 0, values.length > 1 ? values[1] : values[0], 0, 0]);
      } else if (name === "rotate" && values.length >= 1) {
        operation = new DOMMatrix().rotate(values[0]);
        if (values.length >= 3) {
          operation = new DOMMatrix().translate(values[1], values[2])
            .multiply(operation)
            .translate(-values[1], -values[2]);
        }
      } else if (name === "skewx" && values.length >= 1) {
        operation = new DOMMatrix([1, 0, Math.tan(values[0] * Math.PI / 180), 1, 0, 0]);
      } else if (name === "skewy" && values.length >= 1) {
        operation = new DOMMatrix([1, Math.tan(values[0] * Math.PI / 180), 0, 1, 0, 0]);
      }
      if (operation) {
        // SVG transform lists are matrix products in source order.
        const next = multiplySvgMatrices(result, operation);
        [result.a, result.b, result.c, result.d, result.e, result.f] = next;
      }
    }
    return result;
  }

  function svgTransformForElement(element) {
    let matrix = new DOMMatrix();
    const tag = String(element && element.localName || "").toLowerCase();
    if (tag === "svg") {
      matrix = matrix.translate(svgAttributeNumber(element, "x"), svgAttributeNumber(element, "y"));
    }
    const transform = element && element.getAttribute("transform");
    if (transform) matrix = matrix.multiply(svgTransformMatrix(transform));
    if (tag === "svg") {
      const values = svgNumberList(element.getAttribute("viewBox") || element.getAttribute("viewbox"));
      if (values.length >= 4 && values[2] > 0 && values[3] > 0) {
        const width = svgAttributeNumber(element, "width", values[2]);
        const height = svgAttributeNumber(element, "height", values[3]);
        if (width > 0 && height > 0) {
          matrix = matrix.multiply(new DOMMatrix([
            width / values[2], 0, 0, height / values[3],
            -values[0] * width / values[2], -values[1] * height / values[3],
          ]));
        }
      }
    }
    return matrix;
  }

  function svgTransformRect(rect, matrix) {
    if (!rect) return null;
    return svgRectBounds([
      [matrix.a * rect.x + matrix.c * rect.y + matrix.e,
        matrix.b * rect.x + matrix.d * rect.y + matrix.f],
      [matrix.a * (rect.x + rect.width) + matrix.c * rect.y + matrix.e,
        matrix.b * (rect.x + rect.width) + matrix.d * rect.y + matrix.f],
      [matrix.a * rect.x + matrix.c * (rect.y + rect.height) + matrix.e,
        matrix.b * rect.x + matrix.d * (rect.y + rect.height) + matrix.f],
      [matrix.a * (rect.x + rect.width) + matrix.c * (rect.y + rect.height) + matrix.e,
        matrix.b * (rect.x + rect.width) + matrix.d * (rect.y + rect.height) + matrix.f],
    ]);
  }

  function svgPathPoints(value) {
    const tokens = [];
    const expression = /([a-zA-Z])|([+-]?(?:\d*\.\d+|\d+\.?)(?:[eE][+-]?\d+)?)/g;
    let match;
    while ((match = expression.exec(String(value || "")))) {
      tokens.push(match[1] ? { command: match[1] } : { number: Number(match[2]) });
    }
    const parameterCount = { m: 2, l: 2, h: 1, v: 1, c: 6, s: 4, q: 4, t: 2, a: 7 };
    const subpaths = [];
    let points = null;
    let cursorX = 0, cursorY = 0, startX = 0, startY = 0;
    let command = null, previousControl = null, previousCommand = null;
    const add = (x, y) => {
      if (!points) { points = []; subpaths.push(points); }
      points.push([x, y]);
    };
    const addCubic = (x0, y0, x1, y1, x2, y2, x3, y3) => {
      for (let step = 1; step <= 20; step++) {
        const t = step / 20, u = 1 - t;
        add(
          u * u * u * x0 + 3 * u * u * t * x1 + 3 * u * t * t * x2 + t * t * t * x3,
          u * u * u * y0 + 3 * u * u * t * y1 + 3 * u * t * t * y2 + t * t * t * y3,
        );
      }
    };
    const addQuadratic = (x0, y0, x1, y1, x2, y2) => {
      for (let step = 1; step <= 20; step++) {
        const t = step / 20, u = 1 - t;
        add(u * u * x0 + 2 * u * t * x1 + t * t * x2,
          u * u * y0 + 2 * u * t * y1 + t * t * y2);
      }
    };
    while (tokens.length) {
      if (tokens[0].command) {
        command = tokens.shift().command;
        if (command === "Z" || command === "z") {
          if (points && points.length) add(startX, startY);
          cursorX = startX; cursorY = startY;
          previousControl = null; previousCommand = command;
          command = null;
        }
        continue;
      }
      if (!command) { tokens.shift(); continue; }
      const lower = command.toLowerCase();
      const count = parameterCount[lower];
      if (!count || tokens.length < count || tokens.slice(0, count).some(token => token.number === undefined)) {
        command = null;
        continue;
      }
      const values = tokens.splice(0, count).map(token => token.number);
      const relative = command === lower;
      const x = value => relative ? cursorX + value : value;
      const y = value => relative ? cursorY + value : value;
      const oldX = cursorX, oldY = cursorY;
      if (lower === "m") {
        const nextX = x(values[0]), nextY = y(values[1]);
        points = []; subpaths.push(points); points.push([nextX, nextY]);
        cursorX = startX = nextX; cursorY = startY = nextY;
        command = relative ? "l" : "L";
        previousControl = null; previousCommand = "m";
      } else if (lower === "l") {
        cursorX = x(values[0]); cursorY = y(values[1]); add(cursorX, cursorY);
        previousControl = null; previousCommand = lower;
      } else if (lower === "h") {
        cursorX = x(values[0]); add(cursorX, cursorY);
        previousControl = null; previousCommand = lower;
      } else if (lower === "v") {
        cursorY = y(values[0]); add(cursorX, cursorY);
        previousControl = null; previousCommand = lower;
      } else if (lower === "c") {
        const x1 = x(values[0]), y1 = y(values[1]);
        const x2 = x(values[2]), y2 = y(values[3]);
        cursorX = x(values[4]); cursorY = y(values[5]);
        addCubic(oldX, oldY, x1, y1, x2, y2, cursorX, cursorY);
        previousControl = [x2, y2]; previousCommand = lower;
      } else if (lower === "s") {
        const x1 = (previousCommand === "c" || previousCommand === "s") && previousControl
          ? 2 * oldX - previousControl[0] : oldX;
        const y1 = (previousCommand === "c" || previousCommand === "s") && previousControl
          ? 2 * oldY - previousControl[1] : oldY;
        const x2 = x(values[0]), y2 = y(values[1]);
        cursorX = x(values[2]); cursorY = y(values[3]);
        addCubic(oldX, oldY, x1, y1, x2, y2, cursorX, cursorY);
        previousControl = [x2, y2]; previousCommand = lower;
      } else if (lower === "q") {
        const x1 = x(values[0]), y1 = y(values[1]);
        cursorX = x(values[2]); cursorY = y(values[3]);
        addQuadratic(oldX, oldY, x1, y1, cursorX, cursorY);
        previousControl = [x1, y1]; previousCommand = lower;
      } else if (lower === "t") {
        const x1 = (previousCommand === "q" || previousCommand === "t") && previousControl
          ? 2 * oldX - previousControl[0] : oldX;
        const y1 = (previousCommand === "q" || previousCommand === "t") && previousControl
          ? 2 * oldY - previousControl[1] : oldY;
        cursorX = x(values[0]); cursorY = y(values[1]);
        addQuadratic(oldX, oldY, x1, y1, cursorX, cursorY);
        previousControl = [x1, y1]; previousCommand = lower;
      } else if (lower === "a") {
        const rx = Math.abs(values[0]), ry = Math.abs(values[1]);
        cursorX = x(values[5]); cursorY = y(values[6]);
        // The exact elliptical-arc extrema are unnecessary for the DOM
        // surface's deterministic bounds; include the endpoint and the
        // radius envelope so common icon paths remain conservative.
        add(oldX - rx, oldY - ry); add(oldX + rx, oldY + ry);
        add(cursorX - rx, cursorY - ry); add(cursorX + rx, cursorY + ry);
        previousControl = null; previousCommand = lower;
      }
    }
    return subpaths;
  }

  function svgShapeBBox(element) {
    const tag = String(element && element.localName || "").toLowerCase();
    if (tag === "rect") {
      return new SVGRect(
        svgAttributeNumber(element, "x"), svgAttributeNumber(element, "y"),
        Math.max(0, svgAttributeNumber(element, "width")),
        Math.max(0, svgAttributeNumber(element, "height")),
      );
    }
    if (tag === "circle") {
      const radius = Math.max(0, svgAttributeNumber(element, "r"));
      return new SVGRect(
        svgAttributeNumber(element, "cx") - radius,
        svgAttributeNumber(element, "cy") - radius,
        radius * 2, radius * 2,
      );
    }
    if (tag === "ellipse") {
      const rx = Math.max(0, svgAttributeNumber(element, "rx"));
      const ry = Math.max(0, svgAttributeNumber(element, "ry"));
      return new SVGRect(svgAttributeNumber(element, "cx") - rx,
        svgAttributeNumber(element, "cy") - ry, rx * 2, ry * 2);
    }
    if (tag === "line") {
      return svgRectBounds([
        [svgAttributeNumber(element, "x1"), svgAttributeNumber(element, "y1")],
        [svgAttributeNumber(element, "x2"), svgAttributeNumber(element, "y2")],
      ]);
    }
    if (tag === "polyline" || tag === "polygon") {
      const values = svgNumberList(element.getAttribute("points"));
      const points = [];
      for (let index = 0; index + 1 < values.length; index += 2) {
        points.push([values[index], values[index + 1]]);
      }
      return svgRectBounds(points);
    }
    if (tag === "path") {
      const points = [];
      for (const subpath of svgPathPoints(element.getAttribute("d"))) points.push(...subpath);
      return svgRectBounds(points);
    }
    return null;
  }

  function svgElementBBox(element) {
    const own = svgShapeBBox(element);
    if (own) return own;
    let result = null;
    for (const child of element && element.children || []) {
      if (!(child instanceof SVGElement)) continue;
      const childBox = svgElementBBox(child);
      if (childBox) result = svgUnionRect(result, svgTransformRect(childBox, svgTransformForElement(child)));
    }
    return result || new SVGRect();
  }

  function svgAncestorChain(element) {
    const chain = [];
    let current = element;
    while (current && current.nodeType === 1 && current.namespaceURI === SVG_NAMESPACE) {
      chain.push(current);
      current = current.parentNode;
    }
    return chain.reverse();
  }

  class SVGElement extends Element {}
  class SVGGraphicsElement extends SVGElement {
    getBBox(options = {}) {
      const box = svgElementBBox(this);
      const stroke = this.getAttribute("stroke") || (this.style && this.style.stroke) || "";
      if (options && options.stroke && stroke && String(stroke).toLowerCase() !== "none") {
        const width = Math.max(0, svgAttributeNumber(this, "stroke-width", 1));
        return new SVGRect(box.x - width / 2, box.y - width / 2,
          box.width + width, box.height + width);
      }
      return box;
    }
    getCTM() {
      const chain = svgAncestorChain(this);
      if (!chain.length) return null;
      let matrix = new DOMMatrix();
      for (const element of chain) matrix = matrix.multiply(svgTransformForElement(element));
      return matrix;
    }
    getScreenCTM() { return this.getCTM(); }
  }
  class SVGGeometryElement extends SVGGraphicsElement {
    isPointInFill(point) {
      const box = this.getBBox();
      return !!point && point.x >= box.x && point.x <= box.x + box.width &&
        point.y >= box.y && point.y <= box.y + box.height;
    }
    isPointInStroke(point) {
      const box = this.getBBox({ stroke: true });
      return !!point && point.x >= box.x && point.x <= box.x + box.width &&
        point.y >= box.y && point.y <= box.y + box.height;
    }
  }
  class SVGSVGElement extends SVGGraphicsElement {
    createSVGPoint() { return new SVGPoint(); }
    createSVGRect() { return new SVGRect(); }
    getElementById(id) {
      const wanted = String(id);
      let result = null;
      const visit = node => {
        for (const child of node.children || []) {
          if (child.getAttribute("id") === wanted) { result = child; return; }
          visit(child);
          if (result) return;
        }
      };
      visit(this);
      return result;
    }
    get viewBox() {
      if (!this.__viewBox) {
        this.__viewBox = new SVGAnimatedRect(this);
      }
      return this.__viewBox;
    }
  }
  class SVGRectElement extends SVGGeometryElement {}
  class SVGCircleElement extends SVGGeometryElement {}
  class SVGEllipseElement extends SVGGeometryElement {}
  class SVGLineElement extends SVGGeometryElement {}
  class SVGPathElement extends SVGGeometryElement {}
  class SVGPolylineElement extends SVGGeometryElement {}
  class SVGPolygonElement extends SVGGeometryElement {}
  class SVGTextContentElement extends SVGGraphicsElement {
    getNumberOfChars() {
      return String(this.textContent || "").length;
    }
  }
  class SVGTextElement extends SVGTextContentElement {}

  function svgViewBoxValues(element) {
    const values = svgNumberList(element.getAttribute("viewBox") || element.getAttribute("viewbox"));
    return [values[0] || 0, values[1] || 0, values[2] || 0, values[3] || 0];
  }

  function svgWriteViewBoxValue(element, index, value) {
    const values = svgViewBoxValues(element);
    values[index] = finiteSvgNumber(value);
    element.setAttribute("viewBox", values.join(" "));
  }

  function svgMakeViewBoxValue(element, writable) {
    const value = new SVGRect();
    const names = ["x", "y", "width", "height"];
    for (let index = 0; index < names.length; index++) {
      const descriptor = {
        configurable: true,
        enumerable: true,
        get: () => svgViewBoxValues(element)[index],
      };
      if (writable) descriptor.set = next => svgWriteViewBoxValue(element, index, next);
      Object.defineProperty(value, names[index], descriptor);
    }
    return value;
  }

  class SVGAnimatedRect {
    constructor(element) {
      Object.defineProperty(this, "baseVal", {
        configurable: true, enumerable: true, writable: false,
        value: svgMakeViewBoxValue(element, true),
      });
      Object.defineProperty(this, "animVal", {
        configurable: true, enumerable: true, writable: false,
        value: svgMakeViewBoxValue(element, false),
      });
    }
    get [Symbol.toStringTag]() { return "SVGAnimatedRect"; }
  }

  class SVGAnimatedLength {
    constructor(element, name) {
      this.__element = element;
      this.__name = name;
      const value = () => svgAttributeNumber(this.__element, this.__name);
      const baseVal = {};
      Object.defineProperty(baseVal, "value", {
        configurable: true,
        enumerable: true,
        get: value,
        set: next => this.__element.setAttribute(this.__name, String(finiteSvgNumber(next))),
      });
      Object.defineProperty(baseVal, "valueAsString", {
        configurable: true, enumerable: true,
        get: () => this.__element.getAttribute(this.__name) || "0",
        set: next => this.__element.setAttribute(this.__name, String(next)),
      });
      Object.defineProperty(this, "baseVal", {
        configurable: true, enumerable: true, writable: false, value: baseVal,
      });
      Object.defineProperty(this, "animVal", {
        configurable: true, enumerable: true, writable: false, value: baseVal,
      });
    }
    get [Symbol.toStringTag]() { return "SVGAnimatedLength"; }
  }

  function defineSvgAnimatedLengthProperties(ctor, names) {
    for (const name of names) {
      Object.defineProperty(ctor.prototype, name, {
        configurable: true,
        enumerable: false,
        get() {
          const key = "__animated_" + name;
          if (!this[key]) this[key] = new SVGAnimatedLength(this, name);
          return this[key];
        },
      });
    }
  }

  defineSvgAnimatedLengthProperties(SVGRectElement, ["x", "y", "width", "height", "rx", "ry"]);
  defineSvgAnimatedLengthProperties(SVGCircleElement, ["cx", "cy", "r"]);
  defineSvgAnimatedLengthProperties(SVGEllipseElement, ["cx", "cy", "rx", "ry"]);
  defineSvgAnimatedLengthProperties(SVGLineElement, ["x1", "y1", "x2", "y2"]);
  defineSvgAnimatedLengthProperties(SVGSVGElement, ["x", "y", "width", "height"]);

  for (const [ctor, tag] of [
    [SVGElement, "SVGElement"], [SVGGraphicsElement, "SVGGraphicsElement"],
    [SVGGeometryElement, "SVGGeometryElement"], [SVGSVGElement, "SVGSVGElement"],
    [SVGRectElement, "SVGRectElement"], [SVGCircleElement, "SVGCircleElement"],
    [SVGEllipseElement, "SVGEllipseElement"], [SVGLineElement, "SVGLineElement"],
    [SVGPathElement, "SVGPathElement"], [SVGPolylineElement, "SVGPolylineElement"],
    [SVGPolygonElement, "SVGPolygonElement"], [SVGTextContentElement, "SVGTextContentElement"],
    [SVGTextElement, "SVGTextElement"],
  ]) {
    Object.defineProperty(ctor.prototype, Symbol.toStringTag, {
      configurable: true, value: tag,
    });
  }

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
    HTMLSelectElement.prototype, HTMLOptionElement.prototype, HTMLTextAreaElement.prototype,
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
    HTMLSelectElement.prototype, HTMLTextAreaElement.prototype, HTMLIFrameElement.prototype,
    HTMLObjectElement.prototype, HTMLImageElement.prototype,
  ], ["name"]);
  // Input and button provide their own type behavior; the generic fallback must
  // not remain visible on every Node or HTMLElement.
  distributePrototypeMembers(Node.prototype, [], ["type"]);

  const SVG_ELEMENT_CTORS = {
    svg: SVGSVGElement,
    g: SVGGraphicsElement,
    a: SVGGraphicsElement,
    defs: SVGGraphicsElement,
    symbol: SVGGraphicsElement,
    use: SVGGraphicsElement,
    image: SVGGraphicsElement,
    foreignobject: SVGGraphicsElement,
    switch: SVGGraphicsElement,
    rect: SVGRectElement,
    circle: SVGCircleElement,
    ellipse: SVGEllipseElement,
    line: SVGLineElement,
    path: SVGPathElement,
    polyline: SVGPolylineElement,
    polygon: SVGPolygonElement,
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
    textarea: HTMLTextAreaElement,
    button: HTMLButtonElement,
    label: HTMLLabelElement,
    meta: HTMLMetaElement,
    select: HTMLSelectElement,
    option: HTMLOptionElement,
    iframe: HTMLIFrameElement,
    object: HTMLObjectElement,
    audio: HTMLAudioElement,
    video: HTMLVideoElement,
    img: HTMLImageElement,
    canvas: HTMLCanvasElement,
    link: HTMLLinkElement,
    script: HTMLScriptElement,
    style: HTMLStyleElement,
    template: HTMLTemplateElement,
    slot: HTMLSlotElement,
    dialog: HTMLDialogElement,
  };

  const CUSTOM_ELEMENT_REGISTRY_CONSTRUCTION = {};

  function isValidCustomElementName(name) {
    if (name.length < 2 || name.charCodeAt(0) < 0x61 || name.charCodeAt(0) > 0x7a ||
        RESERVED_CUSTOM_ELEMENT_NAMES.has(name)) {
      return false;
    }
    let hasHyphen = false;
    for (let i = 1; i < name.length;) {
      const cp = name.codePointAt(i);
      if (cp === 0x2d) hasHyphen = true;
      const allowed = cp === 0x2d || cp === 0x2e || cp === 0x5f ||
        (cp >= 0x30 && cp <= 0x39) || (cp >= 0x61 && cp <= 0x7a) ||
        cp === 0xb7 || (cp >= 0xc0 && cp <= 0xd6) ||
        (cp >= 0xd8 && cp <= 0xf6) || (cp >= 0xf8 && cp <= 0x37d) ||
        (cp >= 0x37f && cp <= 0x1fff) || (cp >= 0x200c && cp <= 0x200d) ||
        (cp >= 0x203f && cp <= 0x2040) || (cp >= 0x2070 && cp <= 0x218f) ||
        (cp >= 0x2c00 && cp <= 0x2fef) || (cp >= 0x3001 && cp <= 0xd7ff) ||
        (cp >= 0xf900 && cp <= 0xfdcf) || (cp >= 0xfdf0 && cp <= 0xfffd) ||
        (cp >= 0x10000 && cp <= 0xeffff);
      if (!allowed) return false;
      i += cp > 0xffff ? 2 : 1;
    }
    return hasHyphen;
  }

  function isConstructor(value) {
    if (typeof value !== "function") return false;
    try {
      const probe = new Proxy(value, {
        construct() { return {}; },
      });
      new probe();
      return true;
    } catch (_error) {
      return false;
    }
  }

  function invokeCustomElementCallback(element, callbackName, args) {
    if (!element || element.__customElementState !== "custom") return;
    const definition = element.__customElementDefinition;
    const callback = definition && definition.callbacks[callbackName];
    if (!callback) return;
    try {
      Reflect.apply(callback, element, args);
    } catch (error) {
      if (!element.__customElementCallbackErrors) {
        element.__customElementCallbackErrors = [];
      }
      element.__customElementCallbackErrors.push(error);
    }
  }

  function notifyCustomElementAttributeChanged(
    element,
    name,
    oldValue,
    newValue,
    namespace,
  ) {
    if (!element || element.__customElementState !== "custom") return;
    const definition = element.__customElementDefinition;
    if (!definition || !definition.observedAttributes.has(name)) return;
    invokeCustomElementCallback(element, "attributeChangedCallback", [
      name,
      oldValue,
      newValue,
      namespace,
    ]);
  }

  function customElementTreeWalk(root, callback) {
    if (!root) return;
    if (root.nodeType === 1) {
      callback(root);
      if (root.__shadowRootInternal) {
        customElementTreeWalk(root.__shadowRootInternal, callback);
      }
    }
    const children = root.childNodes;
    for (let i = 0; i < children.length; i++) {
      customElementTreeWalk(children[i], callback);
    }
  }

  function connectCustomElement(element) {
    if (!element || element.__customElementState !== "custom" ||
        element.__customElementConnected) {
      return;
    }
    element.__customElementConnected = true;
    invokeCustomElementCallback(element, "connectedCallback", []);
  }

  function disconnectCustomElementTree(root) {
    customElementTreeWalk(root, element => {
      if (element.__customElementState !== "custom" ||
          !element.__customElementConnected) {
        return;
      }
      element.__customElementConnected = false;
      invokeCustomElementCallback(element, "disconnectedCallback", []);
    });
  }

  function upgradeCustomElement(element, definition) {
    if (!element || element.__customElementState === "custom" ||
        element.__customElementState === "precustomized" ||
        element.__customElementState === "failed" ||
        element.namespaceURI !== HTML_NAMESPACE && element.namespaceURI !== null ||
        String(element.localName) !== definition.name) {
      return;
    }

    const wasConnected = element.isConnected;
    const initialAttributes = (__omoikane_attribute_names(element.__id) || [])
      .map(name => ({
        name,
        value: __omoikane_get_attribute(element.__id, name),
      }));
    Object.setPrototypeOf(element, definition.prototype);
    element.__customElementDefinition = definition;
    element.__customElementState = "precustomized";
    const entry = { element, constructed: false };
    customElementConstructionStack.push(entry);
    try {
      const result = Reflect.construct(
        definition.constructor,
        [],
        definition.constructor,
      );
      if (!entry.constructed || result !== element) {
        throw new DOMException(
          "The custom element constructor did not produce the element being upgraded.",
          "InvalidStateError",
        );
      }
      element.__customElementState = "custom";
      element.__customElementConnected = false;
      for (const attribute of initialAttributes) {
        notifyCustomElementAttributeChanged(
          element,
          attribute.name,
          null,
          attribute.value,
          null,
        );
      }
      if (wasConnected) {
        connectCustomElement(element);
        // The upgrade reaction is based on the element's connectivity when the
        // upgrade started. If its constructor detached it, allow a later real
        // insertion to deliver a new connected callback.
        if (!element.isConnected) element.__customElementConnected = false;
      }
    } catch (error) {
      element.__customElementState = "failed";
      element.__customElementError = error;
    } finally {
      customElementConstructionStack.pop();
    }
  }

  function upgradeCustomElementTree(registry, root) {
    if (!root) return;
    const owner = root.nodeType === 9 ? root : root.ownerDocument;
    if (owner !== registry.__document) return;
    if (root.nodeType === 1) {
      const definition = registry.__definitions.get(
        String(root.localName),
      );
      if (definition) upgradeCustomElement(root, definition);
      if (root.__shadowRootInternal) {
        upgradeCustomElementTree(registry, root.__shadowRootInternal);
      }
    }
    const children = root.childNodes;
    for (let i = 0; i < children.length; i++) {
      upgradeCustomElementTree(registry, children[i]);
    }
  }

  function considerCustomElement(registry, element) {
    const name = String(element.localName);
    const definition = registry.__definitions.get(name);
    if (definition) {
      upgradeCustomElement(element, definition);
    }
  }

  function upgradeInsertedCustomElements(parent, nodes) {
    if (!parent || !parent.isConnected) return;
    const owner = parent.nodeType === 9 ? parent : parent.ownerDocument;
    const registry = owner && customElementRegistryByDocument.get(owner);
    if (!registry) return;
    for (const node of nodes) {
      customElementTreeWalk(node, element => {
        if (element.__customElementState === "custom") {
          connectCustomElement(element);
          return;
        }
        const definition = registry.__definitions.get(String(element.localName));
        if (definition) upgradeCustomElement(element, definition);
      });
    }
  }

  function registryForDocument(document) {
    let registry = customElementRegistryByDocument.get(document);
    if (!registry) {
      registry = new CustomElementRegistry(
        CUSTOM_ELEMENT_REGISTRY_CONSTRUCTION,
        document,
      );
      customElementRegistryByDocument.set(document, registry);
    }
    return registry;
  }

  class CustomElementRegistry {
    constructor(token, document) {
      if (token !== CUSTOM_ELEMENT_REGISTRY_CONSTRUCTION) {
        throw new TypeError("Illegal constructor");
      }
      this.__document = document;
      this.__definitions = new Map();
      this.__constructors = new Map();
      this.__whenDefined = new Map();
      this.__definitionRunning = false;
    }

    define(nameValue, constructor, options) {
      const name = String(nameValue);
      if (!isConstructor(constructor)) {
        throw new TypeError("The custom element constructor is not a constructor");
      }
      if (!isValidCustomElementName(name)) {
        throw new DOMException(
          "The custom element name ('" + name + "') is not valid.",
          "SyntaxError",
        );
      }
      if (this.__definitions.has(name) || this.__constructors.has(constructor)) {
        throw new DOMException(
          "The name or constructor has already been registered.",
          "NotSupportedError",
        );
      }
      if (this.__definitionRunning) {
        throw new DOMException(
          "A custom element definition is already running.",
          "NotSupportedError",
        );
      }

      this.__definitionRunning = true;
      let prototype;
      let extendsValue = null;
      const callbacks = {};
      let observedAttributes = [];
      try {
        prototype = constructor.prototype;
        if ((typeof prototype !== "object" && typeof prototype !== "function") ||
            prototype === null) {
          throw new TypeError("The custom element constructor prototype is not an object");
        }
        if (options !== undefined && options !== null) {
          extendsValue = options.extends;
        }
        if (extendsValue !== undefined && extendsValue !== null) {
          throw new DOMException(
            "Customized built-in elements are not supported yet.",
            "NotSupportedError",
          );
        }
        for (const callbackName of [
          "connectedCallback",
          "disconnectedCallback",
          "adoptedCallback",
          "attributeChangedCallback",
        ]) {
          const callback = prototype[callbackName];
          if (callback !== undefined && callback !== null &&
              typeof callback !== "function") {
            throw new TypeError(callbackName + " is not callable");
          }
          callbacks[callbackName] = callback || null;
        }
        if (callbacks.attributeChangedCallback) {
          const observed = constructor.observedAttributes;
          if (observed !== undefined && observed !== null) {
            observedAttributes = Array.from(observed, value => String(value));
          }
        }
      } finally {
        this.__definitionRunning = false;
      }

      const pending = this.__whenDefined.get(name);
      const definition = {
        name,
        constructor,
        prototype,
        callbacks,
        observedAttributes: new Set(observedAttributes),
        document: this.__document,
        promise: pending ? pending.promise : Promise.resolve(constructor),
      };
      this.__definitions.set(name, definition);
      this.__constructors.set(constructor, name);
      if (!customElementDefinitionByConstructor.has(constructor)) {
        customElementDefinitionByConstructor.set(constructor, definition);
      }

      // Existing connected candidates are upgraded in shadow-including tree
      // order. Detached candidates remain undefined until insertion or an
      // explicit upgrade(), as required by the custom-elements algorithm.
      upgradeCustomElementTree(this, this.__document);
      if (pending) {
        pending.resolve(constructor);
        this.__whenDefined.delete(name);
      }
    }

    get(nameValue) {
      const definition = this.__definitions.get(String(nameValue));
      return definition ? definition.constructor : undefined;
    }

    getName(constructor) {
      if (!isConstructor(constructor)) {
        throw new TypeError("The value is not a constructor");
      }
      return this.__constructors.get(constructor) || null;
    }

    whenDefined(nameValue) {
      const name = String(nameValue);
      if (!isValidCustomElementName(name)) {
        return Promise.reject(new DOMException(
          "The custom element name ('" + name + "') is not valid.",
          "SyntaxError",
        ));
      }
      const definition = this.__definitions.get(name);
      if (definition) return definition.promise;
      let pending = this.__whenDefined.get(name);
      if (!pending) {
        let resolve;
        const promise = new Promise(resolver => { resolve = resolver; });
        pending = { promise, resolve };
        this.__whenDefined.set(name, pending);
      }
      return pending.promise;
    }

    upgrade(root) {
      if (!(root instanceof Node)) {
        throw new TypeError("CustomElementRegistry.upgrade requires a Node");
      }
      upgradeCustomElementTree(this, root);
    }
  }

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
    "contextmenu", "wheel", "error", "abort", "slotchange", "scroll",
    "cancel", "close",
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
  globalThis.HTMLDialogElement = HTMLDialogElement;
  globalThis.HTMLStyleElement = HTMLStyleElement;
  globalThis.HTMLTemplateElement = HTMLTemplateElement;
  globalThis.HTMLSlotElement = HTMLSlotElement;
  globalThis.CharacterData = CharacterData;
  globalThis.Text = Text;
  globalThis.CDATASection = CDATASection;
  globalThis.Comment = Comment;
  globalThis.ProcessingInstruction = ProcessingInstruction;
  globalThis.Document = Document;
  globalThis.DocumentFragment = DocumentFragment;
  globalThis.ShadowRoot = ShadowRoot;
  globalThis.DocumentType = DocumentType;
  globalThis.DOMException = DOMException;
  const INTEGER_TYPED_ARRAY_TAGS = new Set([
    "[object Int8Array]", "[object Uint8Array]", "[object Uint8ClampedArray]",
    "[object Int16Array]", "[object Uint16Array]", "[object Int32Array]",
    "[object Uint32Array]", "[object BigInt64Array]", "[object BigUint64Array]",
  ]);

  function copyBufferSourceBytes(data) {
    if (data instanceof ArrayBuffer) return Array.from(new Uint8Array(data));
    if (ArrayBuffer.isView(data)) {
      return Array.from(new Uint8Array(data.buffer, data.byteOffset, data.byteLength));
    }
    throw new TypeError("The provided value is not an ArrayBuffer or view");
  }

  const cryptoConstructionToken = {};
  const cryptoKeyConstructionToken = {};
  const cryptoKeyPairConstructionToken = {};
  const cryptoKeyData = new WeakMap();
  const cryptoHashNames = Object.freeze(["SHA-1", "SHA-256", "SHA-384", "SHA-512"]);
  const cryptoHmacUsages = Object.freeze(["sign", "verify"]);
  let cryptoLifecycleActive = true;

  function cryptoLifecycleError() {
    return new DOMException("The CryptoKey operation belongs to a torn-down document.", "InvalidStateError");
  }

  function normalizeCryptoHash(value) {
    const selected = typeof value === "object" && value !== null ? value.name : value;
    const name = String(selected).toUpperCase();
    if (!cryptoHashNames.includes(name)) {
      throw new DOMException("Unrecognized hash algorithm", "NotSupportedError");
    }
    return name;
  }

  function hashOutputBits(hash) {
    return hash === "SHA-1" ? 160 : hash === "SHA-256" ? 256 : hash === "SHA-384" ? 384 : 512;
  }

  function normalizeHmacAlgorithm(algorithm, requireHash) {
    const objectAlgorithm = typeof algorithm === "object" && algorithm !== null ? algorithm : null;
    const selected = objectAlgorithm ? objectAlgorithm.name : algorithm;
    if (String(selected).toUpperCase() !== "HMAC") {
      throw new DOMException("The requested algorithm is not supported.", "NotSupportedError");
    }
    let hash = null;
    if (objectAlgorithm && objectAlgorithm.hash !== undefined) hash = normalizeCryptoHash(objectAlgorithm.hash);
    if (requireHash && hash === null) {
      throw new DOMException("HMAC requires a hash algorithm.", "NotSupportedError");
    }
    let length = null;
    if (objectAlgorithm && objectAlgorithm.length !== undefined) {
      length = Number(objectAlgorithm.length);
      if (!Number.isSafeInteger(length) || length < 8 || length % 8 !== 0 || length > 524280) {
        throw new DOMException("Invalid HMAC key length.", "OperationError");
      }
    }
    return { name: "HMAC", hash, length };
  }

  function normalizeCryptoUsages(usages, allowed) {
    if (usages === null || usages === undefined || typeof usages === "string") {
      throw new TypeError("keyUsages must be an iterable sequence");
    }
    const iterator = usages[Symbol.iterator];
    if (typeof iterator !== "function") throw new TypeError("keyUsages must be an iterable sequence");
    const result = [];
    for (const usage of usages) {
      const normalized = String(usage);
      if (!allowed.includes(normalized)) {
        throw new DOMException("Invalid key usage.", "SyntaxError");
      }
      if (!result.includes(normalized)) result.push(normalized);
    }
    return result;
  }

  function cryptoBytesToBase64Url(bytes) {
    const chunks = [];
    for (let offset = 0; offset < bytes.length; offset += 0x8000) {
      chunks.push(String.fromCharCode(...bytes.slice(offset, offset + 0x8000)));
    }
    const binary = chunks.join("");
    return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/g, "");
  }

  function cryptoBase64UrlToBytes(value) {
    if (typeof value !== "string" || !/^[A-Za-z0-9_-]*$/.test(value)) {
      throw new DOMException("Invalid JWK key encoding.", "DataError");
    }
    const padding = (4 - (value.length % 4)) % 4;
    try {
      const binary = atob(value.replace(/-/g, "+").replace(/_/g, "/") + "=".repeat(padding));
      return Array.from(binary, character => character.charCodeAt(0));
    } catch (_) {
      throw new DOMException("Invalid JWK key encoding.", "DataError");
    }
  }

  function hmacJwkAlgorithm(hash) {
    return hash === "SHA-256" ? "HS256" : hash === "SHA-384" ? "HS384" : hash === "SHA-512" ? "HS512" : null;
  }

  function hmacAlgorithmForJwk(algorithm) {
    return algorithm === "HS256" ? "SHA-256" : algorithm === "HS384" ? "SHA-384" : algorithm === "HS512" ? "SHA-512" : null;
  }

  function makeHmacKey(bytes, hash, extractable, usages) {
    const keyBytes = Array.from(bytes);
    const algorithm = Object.freeze({
      name: "HMAC",
      hash: Object.freeze({ name: hash }),
      length: keyBytes.length * 8,
    });
    return new CryptoKey(cryptoKeyConstructionToken, {
      extractable: !!extractable,
      algorithm,
      usages,
      bytes: keyBytes,
    });
  }

  function assertCryptoKey(key) {
    if (!(key instanceof CryptoKey) || !cryptoKeyData.has(key)) {
      throw new TypeError("Expected a CryptoKey");
    }
    return cryptoKeyData.get(key);
  }

  function assertHmacKey(key, usage, algorithm) {
    const metadata = assertCryptoKey(key);
    if (metadata.algorithm.name !== "HMAC") {
      throw new DOMException("The key algorithm is incompatible with HMAC.", "InvalidAccessError");
    }
    if (!metadata.usages.includes(usage)) {
      throw new DOMException("The key does not permit this operation.", "InvalidAccessError");
    }
    const normalized = normalizeHmacAlgorithm(algorithm, false);
    if (normalized.hash !== null && normalized.hash !== metadata.algorithm.hash.name) {
      throw new DOMException("The algorithm hash does not match the key.", "InvalidAccessError");
    }
    return metadata;
  }

  function cryptoOperation(callback) {
    return Promise.resolve().then(() => {
      if (!cryptoLifecycleActive) throw cryptoLifecycleError();
      return callback();
    });
  }

  class CryptoKey {
    constructor(token, details) {
      if (token !== cryptoKeyConstructionToken) throw new TypeError("Illegal constructor");
      Object.defineProperties(this, {
        type: { value: "secret", enumerable: true },
        extractable: { value: !!details.extractable, enumerable: true },
        algorithm: { value: details.algorithm, enumerable: true },
        usages: { value: Object.freeze(details.usages.slice()), enumerable: true },
      });
      cryptoKeyData.set(this, {
        bytes: details.bytes.slice(),
        algorithm: details.algorithm,
        extractable: !!details.extractable,
        usages: details.usages.slice(),
      });
    }
    get [Symbol.toStringTag]() { return "CryptoKey"; }
  }

  class CryptoKeyPair {
    constructor(token, details) {
      if (token !== cryptoKeyPairConstructionToken) throw new TypeError("Illegal constructor");
      Object.defineProperties(this, {
        publicKey: { value: details.publicKey, enumerable: true },
        privateKey: { value: details.privateKey, enumerable: true },
      });
    }
    get [Symbol.toStringTag]() { return "CryptoKeyPair"; }
  }

  class SubtleCrypto {
    constructor(token) {
      if (token !== cryptoConstructionToken) throw new TypeError("Illegal constructor");
    }
    digest(algorithm, data) {
      let name;
      let bytes;
      try {
        const selected = typeof algorithm === "object" && algorithm !== null
          ? algorithm.name : algorithm;
        name = String(selected).toUpperCase();
        if (!cryptoHashNames.includes(name)) {
          throw new DOMException("Unrecognized digest algorithm", "NotSupportedError");
        }
        bytes = copyBufferSourceBytes(data);
      } catch (error) {
        return Promise.reject(error);
      }
      return Promise.resolve().then(() => {
        if (!cryptoLifecycleActive) throw cryptoLifecycleError();
        const digest = JSON.parse(nativeCryptoDigest(name, JSON.stringify(bytes)));
        return new Uint8Array(digest).buffer;
      });
    }

    generateKey(algorithm, extractable, keyUsages) {
      return cryptoOperation(() => {
        const normalized = normalizeHmacAlgorithm(algorithm, true);
        const usages = normalizeCryptoUsages(keyUsages, cryptoHmacUsages);
        if (usages.length === 0) throw new DOMException("At least one key usage is required.", "SyntaxError");
        const bits = normalized.length === null ? hashOutputBits(normalized.hash) : normalized.length;
        const bytes = JSON.parse(nativeCryptoRandom(bits / 8));
        if (!Array.isArray(bytes) || bytes.length !== bits / 8) {
          throw new DOMException("Unable to generate an HMAC key.", "OperationError");
        }
        return makeHmacKey(bytes, normalized.hash, extractable, usages);
      });
    }

    importKey(format, keyData, algorithm, extractable, keyUsages) {
      let rawSnapshot = null;
      try {
        if (String(format).toLowerCase() === "raw") rawSnapshot = copyBufferSourceBytes(keyData);
      } catch (error) {
        return Promise.reject(error);
      }
      return cryptoOperation(() => {
        const selectedFormat = String(format).toLowerCase();
        if (selectedFormat !== "raw" && selectedFormat !== "jwk") {
          throw new DOMException("The key format is not supported.", "NotSupportedError");
        }
        const normalized = normalizeHmacAlgorithm(algorithm, true);
        const usages = normalizeCryptoUsages(keyUsages, cryptoHmacUsages);
        if (usages.length === 0) throw new DOMException("At least one key usage is required.", "SyntaxError");
        let bytes;
        if (selectedFormat === "raw") {
          bytes = rawSnapshot;
          if (bytes.length === 0) throw new DOMException("The HMAC key must not be empty.", "DataError");
        } else {
          if (keyData === null || typeof keyData !== "object" || Array.isArray(keyData)) {
            throw new DOMException("The JWK key is invalid.", "DataError");
          }
          if (keyData.kty !== "oct" || typeof keyData.k !== "string") {
            throw new DOMException("The JWK key is invalid.", "DataError");
          }
          bytes = cryptoBase64UrlToBytes(keyData.k);
          if (bytes.length === 0) throw new DOMException("The HMAC key must not be empty.", "DataError");
          if (keyData.alg !== undefined && keyData.alg !== null) {
            const jwkHash = hmacAlgorithmForJwk(String(keyData.alg));
            if (jwkHash === null || jwkHash !== normalized.hash) {
              throw new DOMException("The JWK algorithm does not match HMAC.", "DataError");
            }
          }
          if (keyData.key_ops !== undefined) {
            const normalizedKeyOps = Array.isArray(keyData.key_ops)
              ? keyData.key_ops.map(usage => String(usage)) : null;
            if (normalizedKeyOps === null ||
                normalizedKeyOps.some(usage => !cryptoHmacUsages.includes(usage)) ||
                usages.some(usage => !normalizedKeyOps.includes(usage))) {
              throw new DOMException("The JWK key operations do not permit this key.", "DataError");
            }
          }
          if (keyData.ext === false && extractable) {
            throw new DOMException("The JWK key is not extractable.", "DataError");
          }
        }
        if (normalized.length !== null && normalized.length !== bytes.length * 8) {
          throw new DOMException("The HMAC key length does not match the algorithm.", "DataError");
        }
        return makeHmacKey(bytes, normalized.hash, extractable, usages);
      });
    }

    exportKey(format, key) {
      return cryptoOperation(() => {
        const selectedFormat = String(format).toLowerCase();
        if (selectedFormat !== "raw" && selectedFormat !== "jwk") {
          throw new DOMException("The key format is not supported.", "NotSupportedError");
        }
        const metadata = assertCryptoKey(key);
        if (!metadata.extractable) {
          throw new DOMException("The key is not extractable.", "InvalidAccessError");
        }
        if (metadata.algorithm.name !== "HMAC") {
          throw new DOMException("The key algorithm is not supported.", "NotSupportedError");
        }
        if (selectedFormat === "raw") return new Uint8Array(metadata.bytes).buffer;
        const jwk = {
          kty: "oct",
          k: cryptoBytesToBase64Url(metadata.bytes),
          key_ops: metadata.usages.slice(),
          ext: metadata.extractable,
        };
        const alg = hmacJwkAlgorithm(metadata.algorithm.hash.name);
        if (alg !== null) jwk.alg = alg;
        return jwk;
      });
    }

    sign(algorithm, key, data) {
      let bytes;
      try { bytes = copyBufferSourceBytes(data); }
      catch (error) { return Promise.reject(error); }
      return cryptoOperation(() => {
        const metadata = assertHmacKey(key, "sign", algorithm);
        const signed = JSON.parse(nativeCryptoHmac(
          metadata.algorithm.hash.name,
          JSON.stringify(metadata.bytes),
          JSON.stringify(bytes),
        ));
        return new Uint8Array(signed).buffer;
      });
    }

    verify(algorithm, key, signature, data) {
      let signatureBytes;
      let dataBytes;
      try {
        signatureBytes = copyBufferSourceBytes(signature);
        dataBytes = copyBufferSourceBytes(data);
      } catch (error) {
        return Promise.reject(error);
      }
      return cryptoOperation(() => {
        const metadata = assertHmacKey(key, "verify", algorithm);
        const expected = JSON.parse(nativeCryptoHmac(
          metadata.algorithm.hash.name,
          JSON.stringify(metadata.bytes),
          JSON.stringify(dataBytes),
        ));
        if (signatureBytes.length !== expected.length) return false;
        let difference = 0;
        for (let index = 0; index < expected.length; index++) difference |= signatureBytes[index] ^ expected[index];
        return difference === 0;
      });
    }

    encrypt() { return Promise.reject(new DOMException("The requested algorithm is not supported.", "NotSupportedError")); }
    decrypt() { return Promise.reject(new DOMException("The requested algorithm is not supported.", "NotSupportedError")); }
    deriveBits() { return Promise.reject(new DOMException("The requested algorithm is not supported.", "NotSupportedError")); }
    deriveKey() { return Promise.reject(new DOMException("The requested algorithm is not supported.", "NotSupportedError")); }
    wrapKey() { return Promise.reject(new DOMException("The requested algorithm is not supported.", "NotSupportedError")); }
    unwrapKey() { return Promise.reject(new DOMException("The requested algorithm is not supported.", "NotSupportedError")); }
  }

  class Crypto {
    constructor(token) {
      if (token !== cryptoConstructionToken) throw new TypeError("Illegal constructor");
      this.subtle = new SubtleCrypto(cryptoConstructionToken);
    }
    getRandomValues(array) {
      if (!INTEGER_TYPED_ARRAY_TAGS.has(Object.prototype.toString.call(array))) {
        throw new TypeError("getRandomValues requires an integer TypedArray");
      }
      if (array.byteLength > 65536) {
        throw new DOMException("The requested length exceeds 65,536 bytes", "QuotaExceededError");
      }
      const bytes = JSON.parse(nativeCryptoRandom(array.byteLength));
      new Uint8Array(array.buffer, array.byteOffset, array.byteLength).set(bytes);
      return array;
    }
    randomUUID() {
      const bytes = this.getRandomValues(new Uint8Array(16));
      bytes[6] = (bytes[6] & 0x0f) | 0x40;
      bytes[8] = (bytes[8] & 0x3f) | 0x80;
      const hex = Array.from(bytes, value => value.toString(16).padStart(2, "0"));
      return hex.slice(0, 4).join("") + "-" + hex.slice(4, 6).join("") + "-" +
        hex.slice(6, 8).join("") + "-" + hex.slice(8, 10).join("") + "-" +
        hex.slice(10).join("");
    }
  }
  globalThis.Crypto = Crypto;
  globalThis.SubtleCrypto = SubtleCrypto;
  globalThis.CryptoKey = CryptoKey;
  globalThis.CryptoKeyPair = CryptoKeyPair;
  globalThis.__omoikane_crypto_teardown = () => {
    cryptoLifecycleActive = false;
  };
  // Omoikane does not yet model mixed-content/security contexts, so realms are
  // currently treated as secure and expose the complete core API.
  globalThis.crypto = new Crypto(cryptoConstructionToken);
  globalThis.CustomElementRegistry = CustomElementRegistry;
  globalThis.CSSStyleSheet = CSSStyleSheet;
  globalThis.CSSRuleList = CSSRuleList;
  globalThis.CSSStyleRule = CSSStyleRule;
  globalThis.CSSSupportsRule = CSSSupportsRule;
  globalThis.CSSContainerRule = CSSContainerRule;
  globalThis.CSSGroupingRule = CSSGroupingRule;
  globalThis.CSSConditionRule = CSSConditionRule;
  globalThis.CSSScopeRule = CSSScopeRule;
  globalThis.NodeFilter = NodeFilter;
  globalThis.NodeIterator = NodeIterator;
  globalThis.TreeWalker = TreeWalker;
  globalThis.Range = Range;
  globalThis.Selection = Selection;
  globalThis.getSelection = function getSelection() {
    return selectionForDocument(globalThis.document);
  };
  globalThis.HTMLTableElement = HTMLTableElement;
  globalThis.HTMLTableSectionElement = HTMLTableSectionElement;
  globalThis.HTMLTableRowElement = HTMLTableRowElement;
  globalThis.HTMLFormElement = HTMLFormElement;
  globalThis.HTMLInputElement = HTMLInputElement;
  globalThis.HTMLTextAreaElement = HTMLTextAreaElement;
  globalThis.HTMLButtonElement = HTMLButtonElement;
  globalThis.HTMLLabelElement = HTMLLabelElement;
  globalThis.HTMLMetaElement = HTMLMetaElement;
  globalThis.HTMLSelectElement = HTMLSelectElement;
  globalThis.HTMLOptionElement = HTMLOptionElement;
  globalThis.HTMLImageElement = HTMLImageElement;
  globalThis.HTMLCanvasElement = HTMLCanvasElement;
  globalThis.CanvasRenderingContext2D = CanvasRenderingContext2D;
  globalThis.WebGLRenderingContext = WebGLRenderingContext;
  globalThis.WebGLBuffer = WebGLBuffer;
  globalThis.WebGLShader = WebGLShader;
  globalThis.WebGLProgram = WebGLProgram;
  globalThis.WebGLUniformLocation = WebGLUniformLocation;
  globalThis.__omoikane_webgl_lose_context = function(target) {
    const context = target instanceof WebGLRenderingContext ? target
      : target && target instanceof HTMLCanvasElement ? target.getContext("webgl") : null;
    return !!context && context.__lose();
  };
  globalThis.__omoikane_webgl_restore_context = function(target) {
    const context = target instanceof WebGLRenderingContext ? target
      : target && target instanceof HTMLCanvasElement ? target.getContext("webgl") : null;
    return !!context && context.__restore();
  };
  globalThis.OffscreenCanvas = OffscreenCanvas;
  globalThis.OffscreenCanvasRenderingContext2D = OffscreenCanvasRenderingContext2D;
  globalThis.ImageBitmap = ImageBitmap;
  globalThis.ImageData = ImageData;
  globalThis.Image = function(width, height) {
    const image = document.createElement("img");
    if (width !== undefined) image.width = Number(width);
    if (height !== undefined) image.height = Number(height);
    return image;
  };
  globalThis.Image.prototype = HTMLImageElement.prototype;
  globalThis.HTMLLinkElement = HTMLLinkElement;
  globalThis.HTMLScriptElement = HTMLScriptElement;
  globalThis.HTMLIFrameElement = HTMLIFrameElement;
  globalThis.HTMLObjectElement = HTMLObjectElement;
  globalThis.HTMLMediaElement = HTMLMediaElement;
  globalThis.HTMLAudioElement = HTMLAudioElement;
  globalThis.HTMLVideoElement = HTMLVideoElement;
  globalThis.MediaError = MediaError;
  globalThis.Audio = function Audio(src) {
    const element = document.createElement("audio");
    if (arguments.length > 0) element.src = src;
    return element;
  };
  globalThis.Audio.prototype = HTMLAudioElement.prototype;
  globalThis.SVGElement = SVGElement;
  globalThis.SVGGraphicsElement = SVGGraphicsElement;
  globalThis.SVGGeometryElement = SVGGeometryElement;
  globalThis.SVGSVGElement = SVGSVGElement;
  globalThis.SVGRect = SVGRect;
  globalThis.SVGPoint = SVGPoint;
  globalThis.SVGMatrix = SVGMatrix;
  globalThis.SVGAnimatedLength = SVGAnimatedLength;
  globalThis.SVGAnimatedRect = SVGAnimatedRect;
  globalThis.DOMPoint = DOMPoint;
  globalThis.DOMMatrix = DOMMatrix;
  globalThis.SVGRectElement = SVGRectElement;
  globalThis.SVGCircleElement = SVGCircleElement;
  globalThis.SVGEllipseElement = SVGEllipseElement;
  globalThis.SVGLineElement = SVGLineElement;
  globalThis.SVGPathElement = SVGPathElement;
  globalThis.SVGPolylineElement = SVGPolylineElement;
  globalThis.SVGPolygonElement = SVGPolygonElement;
  globalThis.SVGTextContentElement = SVGTextContentElement;
  globalThis.SVGTextElement = SVGTextElement;
  globalThis.Event = Event;
  globalThis.CustomEvent = CustomEvent;
  globalThis.MessageEvent = MessageEvent;
  globalThis.MouseEvent = MouseEvent;
  globalThis.WheelEvent = WheelEvent;
  globalThis.KeyboardEvent = KeyboardEvent;
  globalThis.FocusEvent = FocusEvent;
  globalThis.UIEvent = UIEvent;
  globalThis.InputEvent = InputEvent;
  globalThis.PointerEvent = MouseEvent;
  globalThis.TouchEvent = Event;
  globalThis.AnimationEvent = Event;
  globalThis.TransitionEvent = TransitionEvent;
  globalThis.document = wrapNode(__omoikane_document_id);
  // Window named properties expose parsed elements with an `id` as global
  // bindings (for example `<style id=theme>` is reachable as `theme`). Keep
  // built-in globals intact. These properties remain writable because an
  // explicit script global takes precedence over Window named access.
  globalThis.__omoikane_install_window_named_properties = function() {
    const visit = node => {
      for (const child of node.childNodes) {
        if (child.nodeType !== 1) continue;
        const id = child.getAttribute("id");
        if (id && !Object.prototype.hasOwnProperty.call(globalThis, id)) {
          Object.defineProperty(globalThis, id, {
            configurable: true,
            enumerable: true,
            writable: true,
            value: child,
          });
        }
        visit(child);
      }
    };
    visit(globalThis.document);
  };
  globalThis.customElements = registryForDocument(globalThis.document);
  globalThis.__omoikane_set_current_script = function(id) {
    globalThis.document.__currentScript =
      id === null || id === undefined ? null : wrapNode(id);
  };
  if (globalThis.window === undefined) {
    globalThis.window = globalThis;
  }
  globalThis.self = globalThis;
  Object.defineProperty(globalThis, "__listeners", {
    configurable: true,
    value: new Map(),
  });
  globalThis.addEventListener = function(type, listener, options) {
    return Node.prototype.addEventListener.call(globalThis, type, listener, options);
  };
  globalThis.removeEventListener = function(type, listener, options) {
    return Node.prototype.removeEventListener.call(globalThis, type, listener, options);
  };
  globalThis.dispatchEvent = function(event) {
    return dispatchEventOnTarget(globalThis, event);
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
  globalThis.__omoikane_dispatch_resource_load = function(id, url = "", redirected = false, elapsedMs = 0) {
    const element = wrapNode(id);
    if (element) {
      finishElementResourceTiming(element, 200, false, { url, redirected, elapsedMs });
      element.dispatchEvent(new Event("load", { bubbles: false }));
    }
  };
  // A resource that could not be fetched fires `error`, not `load`. Loaders that
  // fall back when a script is unavailable listen for exactly this.
  globalThis.__omoikane_dispatch_resource_error = function(id, url = "", redirected = false, elapsedMs = 0) {
    const element = wrapNode(id);
    if (element) {
      finishElementResourceTiming(element, 0, true, { url, redirected, elapsedMs });
      element.dispatchEvent(new Event("error", { bubbles: false }));
    }
  };
  globalThis.__omoikane_dispatch_mouse_input = function(id, type, init, focusTarget) {
    const target = wrapNode(id) || document;
    const notCanceled = target.dispatchEvent(new MouseEvent(type, {
      ...init, bubbles: true, cancelable: true, composed: true,
    }));
    if (notCanceled && focusTarget && target && typeof target.focus === "function") {
      target.focus();
    }
    return notCanceled;
  };
  globalThis.__omoikane_dispatch_wheel_input = function(id, init) {
    const target = wrapNode(id) || document;
    const notCanceled = target.dispatchEvent(new WheelEvent("wheel", {
      ...init, bubbles: true, cancelable: true, composed: true, deltaMode: 0,
    }));
    if (!notCanceled) return false;

    let element = target && target.nodeType === 1 ? target : target.parentNode;
    while (element && element.nodeType === 1) {
      const style = getComputedStyle(element);
      const canScrollX = init.deltaX !== 0 &&
        (style.overflowX === "auto" || style.overflowX === "scroll") &&
        element.scrollWidth > element.clientWidth;
      const canScrollY = init.deltaY !== 0 &&
        (style.overflowY === "auto" || style.overflowY === "scroll") &&
        element.scrollHeight > element.clientHeight;
      if (canScrollX || canScrollY) {
        const beforeX = element.scrollLeft;
        const beforeY = element.scrollTop;
        element.scrollBy(init.deltaX, init.deltaY);
        if (element.scrollLeft !== beforeX || element.scrollTop !== beforeY) return true;
      }
      element = element.parentNode;
    }
    globalThis.scrollBy(init.deltaX, init.deltaY);
    return true;
  };
  globalThis.__omoikane_dispatch_keyboard_input = function(type, init) {
    const focusedDocument = focusChainDocuments()[0] || document;
    const target = focusedElementOf(focusedDocument) || focusedDocument.body ||
      focusedDocument.documentElement || focusedDocument;
    const notCanceled = target.dispatchEvent(new KeyboardEvent(type, {
      ...init, bubbles: true, cancelable: true, composed: true,
    }));
    if (notCanceled && type === "keydown") {
      if (String(init && init.key || "") === "Escape" &&
          performDialogEscapeDefault(focusedDocument)) {
        // The top-most modal dialog consumes Escape as its cancel action.
      } else if (String(init && init.key || "") === "Tab") {
        performSequentialFocusNavigation(focusedDocument, Boolean(init && init.shiftKey));
      } else {
        performTextControlKeyDefault(target, init || {});
      }
    }
    return notCanceled;
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

  let __locationHref = String(__omoikane_location_href);
  const __loc = { protocol: "", hostname: "", pathname: "/", search: "", hash: "", origin: "", host: "" };
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
  Object.defineProperty(__loc, "href", {
    enumerable: true,
    configurable: false,
    get() { return __locationHref; },
    set(url) {
      const href = __applyLocationUrl(url, false);
      if (href !== undefined) __omoikane_schedule_navigation("assign", href);
    },
  });
  Object.defineProperty(globalThis, "location", {
    enumerable: true,
    configurable: false,
    get() { return __loc; },
    set(url) { __loc.assign(url); },
  });
  function __applyLocationUrl(url, requireSameOrigin) {
    if (url == null || String(url) === "") return;
    const raw = String(url);
    const href = String(__omoikane_resolve_url(raw));
    const match = href.match(/^(.*?):\/\/([^/?#]+)([^?#]*)(\?[^#]*)?(#.*)?$/);
    if (!match || (requireSameOrigin && (match[1] + "://" + match[2]) !== __loc.origin)) {
      throw new DOMException("History state URL must be same-origin", "SecurityError");
    }
    __locationHref = href;
    __loc.protocol = match[1] + ":";
    __loc.host = match[2];
    __loc.hostname = match[2].replace(/:\d+$/, "");
    __loc.pathname = match[3] || "/";
    __loc.search = match[4] || "";
    __loc.hash = match[5] || "";
    __loc.origin = match[1] + "://" + match[2];
    return href;
  }
  function __applyHistoryUrl(url) {
    return __applyLocationUrl(url, true);
  }
  __loc.assign = function(url) {
    const href = __applyLocationUrl(url, false);
    if (href !== undefined) __omoikane_schedule_navigation("assign", href);
  };
  __loc.replace = function(url) {
    const href = __applyLocationUrl(url, false);
    if (href !== undefined) __omoikane_schedule_navigation("replace", href);
  };
  __loc.reload = function() {
    __omoikane_schedule_navigation("reload", __loc.href);
  };
  const __historyEntries = [{ state: null, href: __loc.href }];
  let __historyIndex = 0;
  let __sessionHistoryLength = 1;
  globalThis.__omoikane_sync_history = function(length, stateJSON) {
    const numeric = Math.trunc(Number(length));
    if (Number.isFinite(numeric) && numeric > 0) __sessionHistoryLength = numeric;
    if (stateJSON !== undefined) {
      try { __historyEntries[__historyIndex].state = JSON.parse(String(stateJSON)); }
      catch (_) { __historyEntries[__historyIndex].state = null; }
    }
  };
  globalThis.__omoikane_commit_same_document_navigation = function(href, eventType, previousURL) {
    const oldURL = previousURL === undefined ? __loc.href : String(previousURL);
    __applyLocationUrl(href, false);
    const event = new Event(String(eventType || "popstate"));
    event.oldURL = oldURL;
    event.newURL = __loc.href;
    globalThis.dispatchEvent(event);
  };
  globalThis.__omoikane_set_location = function(href) {
    __applyLocationUrl(href, false);
  };
  globalThis.history = {
    scrollRestoration: "auto",
    get length() { return Math.max(__historyEntries.length, __sessionHistoryLength); },
    get state() { return __historyEntries[__historyIndex].state; },
    pushState(state, unused, url) {
      void unused;
      __applyHistoryUrl(url);
      __historyEntries.splice(__historyIndex + 1);
      __historyEntries.push({ state, href: __loc.href });
      __historyIndex = __historyEntries.length - 1;
      const stateJSON = JSON.stringify(state);
      __omoikane_schedule_navigation("push-state", __loc.href, stateJSON === undefined ? "null" : stateJSON);
    },
    replaceState(state, unused, url) {
      void unused;
      __applyHistoryUrl(url);
      __historyEntries[__historyIndex] = { state, href: __loc.href };
      const stateJSON = JSON.stringify(state);
      __omoikane_schedule_navigation("replace-state", __loc.href, stateJSON === undefined ? "null" : stateJSON);
    },
    go(delta = 0) {
      const numeric = Math.trunc(Number(delta || 0));
      if (!Number.isFinite(numeric)) return;
      if (numeric === 0) {
        __omoikane_schedule_navigation("reload", __loc.href);
        return;
      }
      const localTarget = __historyIndex + numeric;
      if (localTarget >= 0 && localTarget < __historyEntries.length) {
        __historyIndex = localTarget;
        __applyHistoryUrl(__historyEntries[localTarget].href);
      }
      __omoikane_schedule_navigation("traverse", String(numeric));
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
  function __dispatchPendingTransitionEvents() {
    let records = [];
    try { records = JSON.parse(__omoikane_take_transition_events()); } catch (_) {}
    for (const record of records) {
      const target = wrapNode(record.nodeId);
      if (!target) continue;
      target.dispatchEvent(new TransitionEvent(record.type, {
        bubbles: true,
        propertyName: record.propertyName,
        elapsedTime: record.elapsedTime,
        pseudoElement: record.pseudoElement,
      }));
    }
  }
  globalThis.__omoikane_sample_css_transitions = function() {
    flushStyleSheets();
    __omoikane_sample_css_transition_styles();
    __dispatchPendingTransitionEvents();
  };
  globalThis.getComputedStyle = function(element, pseudoElt) {
    void pseudoElt;
    flushStyleSheets();
    if (element && element.__id != null) {
      try {
        const style = __makeComputedStyle(JSON.parse(__omoikane_computed_style(element.__id)));
        __dispatchPendingTransitionEvents();
        return style;
      } catch (e) {
        return __makeComputedStyle({});
      }
    }
    return __makeComputedStyle({});
  };
  const navigatorConstructionToken = {};
  class PluginArray {
    constructor(token) {
      if (token !== navigatorConstructionToken) throw new TypeError("Illegal constructor");
    }
    get length() { return 0; }
    item(index) { void index; return null; }
    namedItem(name) { void name; return null; }
    refresh() {}
    *[Symbol.iterator]() {}
    get [Symbol.toStringTag]() { return "PluginArray"; }
  }
  class MimeTypeArray {
    constructor(token) {
      if (token !== navigatorConstructionToken) throw new TypeError("Illegal constructor");
    }
    get length() { return 0; }
    item(index) { void index; return null; }
    namedItem(name) { void name; return null; }
    *[Symbol.iterator]() {}
    get [Symbol.toStringTag]() { return "MimeTypeArray"; }
  }
  const clipboardConstructionToken = {};
  class Clipboard {
    constructor(token) {
      if (token !== clipboardConstructionToken) throw new TypeError("Illegal constructor");
    }
    readText() {
      return Promise.resolve().then(() => {
        if (!nativeIsSecureContext()) {
          throw new DOMException("Clipboard access requires a secure context.", "NotAllowedError");
        }
        const text = nativeClipboardReadText();
        if (text === null) {
          throw new DOMException("Clipboard permission was denied.", "NotAllowedError");
        }
        return String(text);
      });
    }
    writeText(value) {
      if (arguments.length < 1) {
        throw new TypeError("Clipboard.writeText requires one argument");
      }
      return Promise.resolve().then(() => {
        if (!nativeIsSecureContext()) {
          throw new DOMException("Clipboard access requires a secure context.", "NotAllowedError");
        }
        if (!nativeClipboardWriteText(String(value))) {
          throw new DOMException("Clipboard permission was denied.", "NotAllowedError");
        }
      });
    }
    get [Symbol.toStringTag]() { return "Clipboard"; }
  }
  const geolocationConstructionToken = {};
  const nativeGeolocationRequest = globalThis.__omoikane_geolocation_request;
  const nativeGeolocationClearWatch = globalThis.__omoikane_geolocation_clear_watch;
  try { delete globalThis.__omoikane_geolocation_request; } catch (_) {}
  try { delete globalThis.__omoikane_geolocation_clear_watch; } catch (_) {}

  class GeolocationCoordinates {
    constructor(token, data) {
      if (token !== geolocationConstructionToken) throw new TypeError("Illegal constructor");
      const values = data || {};
      const define = (name, value) => Object.defineProperty(this, name, {
        configurable: false, enumerable: true, writable: false, value,
      });
      define("latitude", Number(values.latitude));
      define("longitude", Number(values.longitude));
      define("accuracy", Number(values.accuracy));
      define("altitude", values.altitude == null ? null : Number(values.altitude));
      define("altitudeAccuracy", values.altitudeAccuracy == null ? null : Number(values.altitudeAccuracy));
      define("heading", values.heading == null ? null : Number(values.heading));
      define("speed", values.speed == null ? null : Number(values.speed));
    }
    get [Symbol.toStringTag]() { return "GeolocationCoordinates"; }
  }

  class GeolocationPosition {
    constructor(token, data) {
      if (token !== geolocationConstructionToken) throw new TypeError("Illegal constructor");
      const values = data || {};
      Object.defineProperty(this, "coords", {
        configurable: false, enumerable: true, writable: false,
        value: new GeolocationCoordinates(geolocationConstructionToken, values.coords || {}),
      });
      Object.defineProperty(this, "timestamp", {
        configurable: false, enumerable: true, writable: false, value: Number(values.timestamp),
      });
    }
    get [Symbol.toStringTag]() { return "GeolocationPosition"; }
  }

  class GeolocationPositionError {
    constructor(token, code, message) {
      if (token !== geolocationConstructionToken) throw new TypeError("Illegal constructor");
      Object.defineProperty(this, "code", {
        configurable: false, enumerable: true, writable: false, value: Number(code),
      });
      Object.defineProperty(this, "message", {
        configurable: false, enumerable: true, writable: false, value: String(message || ""),
      });
    }
    get [Symbol.toStringTag]() { return "GeolocationPositionError"; }
  }
  GeolocationPositionError.PERMISSION_DENIED = 1;
  GeolocationPositionError.POSITION_UNAVAILABLE = 2;
  GeolocationPositionError.TIMEOUT = 3;
  for (const name of ["PERMISSION_DENIED", "POSITION_UNAVAILABLE", "TIMEOUT"]) {
    GeolocationPositionError.prototype[name] = GeolocationPositionError[name];
  }

  function normalizeGeolocationOptions(options) {
    if (options == null) return { timeout: Infinity, maximumAge: 0 };
    const value = Object(options);
    let timeout = value.timeout === undefined ? Infinity : Number(value.timeout);
    let maximumAge = value.maximumAge === undefined ? 0 : Number(value.maximumAge);
    if (Number.isNaN(timeout) || timeout < 0 || Number.isNaN(maximumAge) || maximumAge < 0) {
      throw new RangeError("geolocation timeout and maximumAge must be non-negative");
    }
    if (Number.isFinite(timeout)) timeout = Math.floor(timeout);
    if (Number.isFinite(maximumAge)) maximumAge = Math.floor(maximumAge);
    return { timeout, maximumAge };
  }

  class Geolocation {
    constructor(token) {
      if (token !== geolocationConstructionToken) throw new TypeError("Illegal constructor");
      this.__nextWatchId = 1;
    }
    getCurrentPosition(success, error = undefined, options = undefined) {
      if (typeof success !== "function") throw new TypeError("success callback must be callable");
      if (error != null && typeof error !== "function") throw new TypeError("error callback must be callable");
      const normalized = normalizeGeolocationOptions(options);
      nativeGeolocationRequest(success, error == null ? undefined : error, -1,
        normalized.timeout, normalized.maximumAge);
    }
    watchPosition(success, error = undefined, options = undefined) {
      if (typeof success !== "function") throw new TypeError("success callback must be callable");
      if (error != null && typeof error !== "function") throw new TypeError("error callback must be callable");
      const normalized = normalizeGeolocationOptions(options);
      const id = this.__nextWatchId++ >>> 0;
      nativeGeolocationRequest(success, error == null ? undefined : error, id,
        normalized.timeout, normalized.maximumAge);
      return id;
    }
    clearWatch(id) {
      const number = Number(id);
      if (!Number.isFinite(number)) return;
      nativeGeolocationClearWatch(number >>> 0);
    }
    get [Symbol.toStringTag]() { return "Geolocation"; }
  }

  globalThis.__omoikane_dispatch_geolocation_task = function() {
    const status = globalThis.__omoikane_geolocation_status;
    const callback = globalThis.__omoikane_geolocation_callback;
    if (status === "success") {
      if (typeof callback === "function") {
        callback(new GeolocationPosition(
          geolocationConstructionToken,
          JSON.parse(globalThis.__omoikane_geolocation_payload || "{}"),
        ));
      }
    } else if (typeof globalThis.__omoikane_geolocation_error_callback === "function") {
      globalThis.__omoikane_geolocation_error_callback(new GeolocationPositionError(
        geolocationConstructionToken,
        globalThis.__omoikane_geolocation_error_code,
        globalThis.__omoikane_geolocation_error_message,
      ));
    }
  };

  // -------------------------------------------------------------------------
  // Service Worker registration/container lifecycle core.
  //
  // Script execution and fetch interception are intentionally out of scope
  // for this deterministic model.  Registrations still retain their
  // same-origin script/scope URLs, expose the installing/active state
  // transitions, and select the longest matching scope for the current
  // document.  Keeping records in this realm avoids moving Boa objects across
  // runtimes while preserving the Web IDL-facing object graph.
  // -------------------------------------------------------------------------
  const serviceWorkerConstructionToken = {};
  const serviceWorkerContainerConstructionToken = {};

  // EventTarget is declared later in this bootstrap (after Navigator is
  // constructed).  This bridge keeps construction safe during that initial
  // pass, then adopts the real EventTarget prototype once it is installed.
  class ServiceWorkerEventTarget {
    constructor() { this._listeners = new Map(); }
    addEventListener(...args) { return globalThis.EventTarget.prototype.addEventListener.call(this, ...args); }
    removeEventListener(...args) { return globalThis.EventTarget.prototype.removeEventListener.call(this, ...args); }
    dispatchEvent(...args) { return globalThis.EventTarget.prototype.dispatchEvent.call(this, ...args); }
  }

  function serviceWorkerURL(value, base) {
    return new URL(String(value), base || String(globalThis.location && globalThis.location.href || ""));
  }

  function serviceWorkerOrigin(url) {
    return String(url.origin || "");
  }

  function serviceWorkerCurrentOrigin() {
    return String(globalThis.location && globalThis.location.origin || "");
  }

  function serviceWorkerScopeURL(scriptURL, options) {
    const requested = options && options.scope !== undefined
      ? serviceWorkerURL(options.scope, scriptURL.href)
      : serviceWorkerURL(scriptURL.pathname.slice(0, scriptURL.pathname.lastIndexOf("/") + 1), scriptURL.href);
    requested.hash = "";
    return requested;
  }

  class ServiceWorker extends ServiceWorkerEventTarget {
    constructor(record, state, token) {
      if (token !== serviceWorkerConstructionToken) throw new TypeError("Illegal constructor");
      super();
      this.scriptURL = record.scriptURL;
      this._state = String(state);
      this._record = record;
      this._onstatechange = null;
    }
    get state() { return this._state; }
    __setState(state) {
      const next = String(state);
      if (next === this._state) return;
      this._state = next;
      this.dispatchEvent(new Event("statechange"));
    }
    postMessage() {
      throw new DOMException("Service worker script execution is not available.", "InvalidStateError");
    }
    get onstatechange() { return this._onstatechange; }
    set onstatechange(callback) {
      if (this._onstatechange) this.removeEventListener("statechange", this._onstatechange);
      this._onstatechange = typeof callback === "function" ? callback : null;
      if (this._onstatechange) this.addEventListener("statechange", this._onstatechange);
    }
    get [Symbol.toStringTag]() { return "ServiceWorker"; }
  }

  class ServiceWorkerRegistration extends ServiceWorkerEventTarget {
    constructor(record, container, token) {
      if (token !== serviceWorkerContainerConstructionToken) throw new TypeError("Illegal constructor");
      super();
      this._record = record;
      this._container = container;
      this.scope = record.scope;
      this.installing = record.worker;
      this.waiting = null;
      this.active = null;
      this._onupdatefound = null;
    }
    update() {
      return Promise.resolve(undefined);
    }
    unregister() {
      return Promise.resolve().then(() => {
        if (!this._container._records.has(this.scope)) return false;
        this._container._records.delete(this.scope);
        this.installing = null;
        this.waiting = null;
        this.active = null;
        this._container.__updateController();
        return true;
      });
    }
    get onupdatefound() { return this._onupdatefound; }
    set onupdatefound(callback) {
      if (this._onupdatefound) this.removeEventListener("updatefound", this._onupdatefound);
      this._onupdatefound = typeof callback === "function" ? callback : null;
      if (this._onupdatefound) this.addEventListener("updatefound", this._onupdatefound);
    }
    get [Symbol.toStringTag]() { return "ServiceWorkerRegistration"; }
  }

  class ServiceWorkerContainer extends ServiceWorkerEventTarget {
    constructor(token) {
      if (token !== serviceWorkerContainerConstructionToken) throw new TypeError("Illegal constructor");
      super();
      this._records = new Map();
      this.controller = null;
      this._readyResolved = false;
      this._readyResolve = null;
      this.ready = new Promise(resolve => { this._readyResolve = resolve; });
      this._oncontrollerchange = null;
    }
    __validateRegistration(scriptURL, options) {
      const script = serviceWorkerURL(scriptURL);
      const origin = serviceWorkerCurrentOrigin();
      if (!origin || origin === "null" || serviceWorkerOrigin(script) !== origin) {
        throw new DOMException("Service worker script must be same-origin.", "SecurityError");
      }
      const scope = serviceWorkerScopeURL(script, options || {});
      if (serviceWorkerOrigin(scope) !== origin) {
        throw new DOMException("Service worker scope must be same-origin.", "SecurityError");
      }
      return { script: script.href, scope: scope.href };
    }
    register(scriptURL, options = undefined) {
      return Promise.resolve().then(() => {
        if (typeof nativeIsSecureContext !== "function" || !nativeIsSecureContext()) {
          throw new DOMException("Service workers require a secure context.", "SecurityError");
        }
        const validated = this.__validateRegistration(scriptURL, options || {});
        let record = this._records.get(validated.scope);
        if (record) {
          if (record.scriptURL !== validated.script) {
            record.scriptURL = validated.script;
            record.worker = new ServiceWorker(record, "installing", serviceWorkerConstructionToken);
            record.registration.installing = record.worker;
            record.registration.waiting = null;
            record.registration.active = null;
            record.registration.dispatchEvent(new Event("updatefound"));
          }
        } else {
          record = { scriptURL: validated.script, scope: validated.scope, worker: null, registration: null };
          record.worker = new ServiceWorker(record, "installing", serviceWorkerConstructionToken);
          record.registration = new ServiceWorkerRegistration(record, this, serviceWorkerContainerConstructionToken);
          this._records.set(validated.scope, record);
        }
        const registration = record.registration;
        // The core has no script evaluator, so install/activate is a single
        // deterministic microtask transition after registration creation.
        queueMicrotask(() => {
          if (!this._records.has(record.scope)) return;
          record.worker.__setState("activated");
          registration.installing = null;
          registration.waiting = null;
          registration.active = record.worker;
          this.__updateController();
          if (!this._readyResolved && this.controller === record.worker) {
            this._readyResolved = true;
            this._readyResolve(registration);
          }
        });
        return registration;
      });
    }
    getRegistration(clientURL = undefined) {
      return Promise.resolve().then(() => {
        const target = serviceWorkerURL(clientURL === undefined ? globalThis.location.href : clientURL);
        if (serviceWorkerOrigin(target) !== serviceWorkerCurrentOrigin()) return undefined;
        let match = null;
        for (const record of this._records.values()) {
          if (target.href.startsWith(record.scope) && (!match || record.scope.length > match.scope.length)) {
            match = record;
          }
        }
        return match ? match.registration : undefined;
      });
    }
    getRegistrations() {
      return Promise.resolve().then(() => Array.from(this._records.values(), record => record.registration));
    }
    __updateController() {
      const href = String(globalThis.location && globalThis.location.href || "");
      let match = null;
      for (const record of this._records.values()) {
        if (record.registration.active && href.startsWith(record.scope) && (!match || record.scope.length > match.scope.length)) {
          match = record;
        }
      }
      const next = match ? match.registration.active : null;
      if (next === this.controller) return;
      this.controller = next;
      queueMicrotask(() => this.dispatchEvent(new Event("controllerchange")));
    }
    get oncontrollerchange() { return this._oncontrollerchange; }
    set oncontrollerchange(callback) {
      if (this._oncontrollerchange) this.removeEventListener("controllerchange", this._oncontrollerchange);
      this._oncontrollerchange = typeof callback === "function" ? callback : null;
      if (this._oncontrollerchange) this.addEventListener("controllerchange", this._oncontrollerchange);
    }
    get [Symbol.toStringTag]() { return "ServiceWorkerContainer"; }
  }

  globalThis.ServiceWorker = ServiceWorker;
  globalThis.ServiceWorkerRegistration = ServiceWorkerRegistration;
  globalThis.ServiceWorkerContainer = ServiceWorkerContainer;

  // -------------------------------------------------------------------------
  // WebGPU deterministic adapter/device/queue core.
  //
  // Omoikane intentionally does not bind a host GPU backend.  The implementation
  // below nevertheless keeps the WebGPU object graph and the parts of the
  // validation/state machine that are useful to headless callers deterministic:
  // one software adapter, an in-memory device, mapped buffers, queue writes,
  // and copy/clear command buffers.  Shader/pipeline/texture presentation APIs
  // remain outside this core and are rejected through the normal validation
  // error path rather than pretending that a GPU exists.
  // -------------------------------------------------------------------------
  const gpuConstructionToken = {};
  const gpuAdapterConstructionToken = {};
  const gpuDeviceConstructionToken = {};
  const gpuBufferConstructionToken = {};
  const gpuCommandConstructionToken = {};
  const gpuErrorConstructionToken = {};

  // Keep constants as frozen arrays/objects instead of long-lived Set objects.
  // Boa's Set finalizer can otherwise retain a borrow while the web API surface
  // probe tears down a realm.
  const WEBGPU_FEATURES = Object.freeze([]);
  const WEBGPU_LIMITS = Object.freeze({
    maxTextureDimension1D: 8192,
    maxTextureDimension2D: 8192,
    maxTextureDimension3D: 2048,
    maxTextureArrayLayers: 256,
    maxBindGroups: 4,
    maxBindGroupsPlusVertexBuffers: 24,
    maxBindingsPerBindGroup: 1000,
    maxDynamicUniformBuffersPerPipelineLayout: 8,
    maxDynamicStorageBuffersPerPipelineLayout: 4,
    maxSampledTexturesPerShaderStage: 16,
    maxSamplersPerShaderStage: 16,
    maxStorageBuffersPerShaderStage: 8,
    maxStorageTexturesPerShaderStage: 4,
    maxUniformBuffersPerShaderStage: 12,
    maxUniformBufferBindingSize: 65536,
    maxStorageBufferBindingSize: 134217728,
    minUniformBufferOffsetAlignment: 256,
    minStorageBufferOffsetAlignment: 256,
    maxVertexBuffers: 8,
    maxBufferSize: 268435456,
    maxVertexAttributes: 16,
    maxVertexBufferArrayStride: 2048,
    maxInterStageShaderComponents: 60,
    maxInterStageShaderVariables: 16,
    maxColorAttachments: 8,
    maxColorAttachmentBytesPerSample: 32,
    maxComputeWorkgroupStorageSize: 16384,
    maxComputeInvocationsPerWorkgroup: 256,
    maxComputeWorkgroupSizeX: 256,
    maxComputeWorkgroupSizeY: 256,
    maxComputeWorkgroupSizeZ: 64,
    maxComputeWorkgroupsPerDimension: 65535,
  });
  const WEBGPU_BUFFER_USAGE = Object.freeze({
    MAP_READ: 0x0001,
    MAP_WRITE: 0x0002,
    COPY_SRC: 0x0004,
    COPY_DST: 0x0008,
    INDEX: 0x0010,
    VERTEX: 0x0020,
    UNIFORM: 0x0040,
    STORAGE: 0x0080,
    INDIRECT: 0x0100,
    QUERY_RESOLVE: 0x0200,
  });
  const WEBGPU_MAP_MODE = Object.freeze({ READ: 0x0001, WRITE: 0x0002 });
  const WEBGPU_SHADER_STAGE = Object.freeze({ VERTEX: 0x0001, FRAGMENT: 0x0002, COMPUTE: 0x0004 });
  const WEBGPU_TEXTURE_USAGE = Object.freeze({
    COPY_SRC: 0x01, COPY_DST: 0x02, TEXTURE_BINDING: 0x04,
    STORAGE_BINDING: 0x08, RENDER_ATTACHMENT: 0x10,
  });

  // EventTarget is installed later in this bootstrap.  This bridge mirrors the
  // ServiceWorker bridge above so GPUDevice can expose EventTarget semantics as
  // soon as the global constructor is available.
  class GPUEventTarget {
    constructor() { this._listeners = new Map(); }
    addEventListener(...args) { return globalThis.EventTarget.prototype.addEventListener.call(this, ...args); }
    removeEventListener(...args) { return globalThis.EventTarget.prototype.removeEventListener.call(this, ...args); }
    dispatchEvent(...args) { return globalThis.EventTarget.prototype.dispatchEvent.call(this, ...args); }
  }

  class GPUError {
    constructor(message = "") {
      Object.defineProperty(this, "message", {
        configurable: false, enumerable: true, writable: false, value: String(message),
      });
    }
    get [Symbol.toStringTag]() { return "GPUError"; }
  }
  class GPUValidationError extends GPUError {
    get [Symbol.toStringTag]() { return "GPUValidationError"; }
  }
  class GPUOutOfMemoryError extends GPUError {
    get [Symbol.toStringTag]() { return "GPUOutOfMemoryError"; }
  }
  class GPUInternalError extends GPUError {
    get [Symbol.toStringTag]() { return "GPUInternalError"; }
  }
  class GPUDeviceLostInfo {
    constructor(token, reason, message) {
      if (token !== gpuErrorConstructionToken) throw new TypeError("Illegal constructor");
      Object.defineProperties(this, {
        reason: { value: String(reason), enumerable: true },
        message: { value: String(message), enumerable: true },
      });
    }
    get [Symbol.toStringTag]() { return "GPUDeviceLostInfo"; }
  }
  class GPUUncapturedErrorEvent extends Event {
    constructor(type = "uncapturederror", init = {}) {
      super(type, init);
      Object.defineProperty(this, "error", {
        configurable: false, enumerable: true, writable: false, value: init.error || null,
      });
    }
    get [Symbol.toStringTag]() { return "GPUUncapturedErrorEvent"; }
  }

  class GPUSupportedFeatures {
    constructor(token, values = []) {
      if (token !== gpuAdapterConstructionToken) throw new TypeError("Illegal constructor");
      this.__values = Object.freeze(Array.from(values, String));
    }
    get size() { return this.__values.length; }
    has(value) { return this.__values.includes(String(value)); }
    keys() { return this.__values[Symbol.iterator](); }
    values() { return this.__values[Symbol.iterator](); }
    entries() { return this.__values.map(value => [value, value])[Symbol.iterator](); }
    forEach(callback, thisArg = undefined) {
      if (typeof callback !== "function") throw new TypeError("callback must be callable");
      for (const value of this.__values) callback.call(thisArg, value, value, this);
    }
    [Symbol.iterator]() { return this.values(); }
    get [Symbol.toStringTag]() { return "GPUSupportedFeatures"; }
  }

  class GPUSupportedLimits {
    constructor(token, values = WEBGPU_LIMITS) {
      if (token !== gpuAdapterConstructionToken && token !== gpuDeviceConstructionToken) {
        throw new TypeError("Illegal constructor");
      }
      this.__values = Object.freeze({ ...values });
      for (const name of Object.keys(this.__values)) {
        Object.defineProperty(this, name, {
          configurable: false, enumerable: true, get: () => this.__values[name],
        });
      }
    }
    get [Symbol.toStringTag]() { return "GPUSupportedLimits"; }
  }

  class GPUAdapterInfo {
    constructor(token, values) {
      if (token !== gpuAdapterConstructionToken) throw new TypeError("Illegal constructor");
      const source = values || {};
      Object.defineProperties(this, {
        architecture: { value: String(source.architecture || "deterministic"), enumerable: true },
        description: { value: String(source.description || "Omoikane deterministic WebGPU adapter"), enumerable: true },
        device: { value: String(source.device || "software"), enumerable: true },
        vendor: { value: String(source.vendor || "Omoikane"), enumerable: true },
      });
    }
    get [Symbol.toStringTag]() { return "GPUAdapterInfo"; }
  }

  function webgpuOperationError(message) {
    return new DOMException(String(message || "WebGPU operation failed."), "OperationError");
  }
  function webgpuValidationError(message) {
    return new GPUValidationError(String(message || "WebGPU validation failed."));
  }
  function webgpuInteger(value, name) {
    const number = Number(value);
    if (!Number.isFinite(number) || Math.trunc(number) !== number || number < 0) {
      throw new TypeError(name + " must be a non-negative integer");
    }
    return number;
  }
  function webgpuBytes(value) {
    if (value instanceof ArrayBuffer) return new Uint8Array(value);
    if (ArrayBuffer.isView(value)) return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
    throw new TypeError("data must be an ArrayBuffer or ArrayBufferView");
  }
  function webgpuDeviceActive(device, operation) {
    if (!device || device.__destroyed) {
      if (device) webgpuRecordError(device, "validation", (operation || "Operation") + " used a destroyed device.");
      return false;
    }
    return true;
  }
  function webgpuRecordError(device, filter, message) {
    if (!device) return;
    const error = filter === "out-of-memory"
      ? new GPUOutOfMemoryError(message)
      : filter === "internal" ? new GPUInternalError(message) : webgpuValidationError(message);
    const scope = device.__errorScopes.length ? device.__errorScopes[device.__errorScopes.length - 1] : null;
    if (scope && scope.filter === filter) {
      if (!scope.error) scope.error = error;
      return;
    }
    if (scope && scope.filter !== filter && filter === "validation") {
      // A validation error cannot be hidden by an out-of-memory/internal scope.
      // Leave it uncaptured, matching the browser's uncaptured error surface.
    }
    queueMicrotask(() => {
      if (device.__destroyed && device.__uncapturedSuppressed) return;
      const event = new GPUUncapturedErrorEvent("uncapturederror", { error });
      try { device.dispatchEvent(event); } catch (_) {}
    });
  }

  class GPUBuffer {
    constructor(device, descriptor, token, invalid = false) {
      if (token !== gpuBufferConstructionToken) throw new TypeError("Illegal constructor");
      this.__device = device;
      this.__invalid = !!invalid;
      this.__destroyed = false;
      this.__size = Number(descriptor.size) || 0;
      this.__usage = Number(descriptor.usage) || 0;
      this.__label = descriptor.label === undefined ? "" : String(descriptor.label);
      this.__data = new Uint8Array(Math.max(0, this.__size));
      this.__mapState = "unmapped";
      this.__map = null;
      if (!this.__invalid && descriptor.mappedAtCreation) {
        this.__mapState = "mapped";
        this.__map = { mode: WEBGPU_MAP_MODE.WRITE, offset: 0, size: this.__size,
          buffer: this.__data.slice().buffer, views: [] };
      }
    }
    get label() { return this.__label; }
    set label(value) { this.__label = String(value); }
    get size() { return this.__size; }
    get usage() { return this.__usage; }
    get mapState() { return this.__mapState; }
    mapAsync(mode, offset = 0, size = undefined) {
      const normalizedMode = Number(mode);
      if (this.__invalid || this.__destroyed || !webgpuDeviceActive(this.__device, "GPUBuffer.mapAsync")) {
        return Promise.reject(webgpuOperationError("The GPUBuffer is unavailable."));
      }
      if (normalizedMode !== WEBGPU_MAP_MODE.READ && normalizedMode !== WEBGPU_MAP_MODE.WRITE) {
        return Promise.reject(new TypeError("mapMode must be READ or WRITE"));
      }
      if (this.__mapState !== "unmapped") return Promise.reject(webgpuOperationError("The GPUBuffer is already mapped."));
      let start;
      let length;
      try {
        start = webgpuInteger(offset, "offset");
        length = size === undefined ? this.__size - start : webgpuInteger(size, "size");
      } catch (error) { return Promise.reject(error); }
      if (start % 8 !== 0 || length % 4 !== 0 || start > this.__size || start + length > this.__size) {
        return Promise.reject(webgpuOperationError("The mapped range is outside the buffer or is misaligned."));
      }
      if ((normalizedMode === WEBGPU_MAP_MODE.READ && !(this.__usage & WEBGPU_BUFFER_USAGE.MAP_READ)) ||
          (normalizedMode === WEBGPU_MAP_MODE.WRITE && !(this.__usage & WEBGPU_BUFFER_USAGE.MAP_WRITE))) {
        return Promise.reject(webgpuOperationError("The requested map mode is not enabled for this buffer."));
      }
      this.__mapState = "pending";
      return Promise.resolve().then(() => {
        if (this.__destroyed || this.__device.__destroyed) {
          this.__mapState = "unmapped";
          throw webgpuOperationError("The GPUBuffer was destroyed while mapping.");
        }
        this.__mapState = "mapped";
        this.__map = { mode: normalizedMode, offset: start, size: length,
          buffer: this.__data.slice(start, start + length).buffer, views: [] };
      });
    }
    getMappedRange(offset = undefined, size = undefined) {
      if (this.__mapState !== "mapped" || !this.__map) throw webgpuOperationError("The GPUBuffer is not mapped.");
      // WebGPU's getMappedRange offset is relative to the beginning of the
      // mapped range (unlike mapAsync's buffer-relative offset).
      const relative = offset === undefined ? 0 : webgpuInteger(offset, "offset");
      const length = size === undefined ? this.__map.size - relative : webgpuInteger(size, "size");
      if (relative < 0 || relative > this.__map.size || length < 0 || relative + length > this.__map.size ||
          relative % 8 !== 0 || length % 4 !== 0) throw webgpuOperationError("The mapped range is invalid or misaligned.");
      if (relative === 0 && length === this.__map.size) return this.__map.buffer;
      const view = this.__map.buffer.slice(relative, relative + length);
      this.__map.views.push({ offset: relative, buffer: view });
      return view;
    }
    unmap() {
      if (this.__mapState !== "mapped" || !this.__map) return;
      if (this.__map.mode === WEBGPU_MAP_MODE.WRITE && !this.__destroyed) {
        const base = new Uint8Array(this.__map.buffer);
        this.__data.set(base, this.__map.offset);
        for (const view of this.__map.views) {
          this.__data.set(new Uint8Array(view.buffer), this.__map.offset + view.offset);
        }
      }
      this.__map = null;
      this.__mapState = "unmapped";
    }
    destroy() {
      if (this.__destroyed) return;
      this.unmap();
      this.__destroyed = true;
      // WebGPU exposes only unmapped/pending/mapped map states.  A destroyed
      // buffer is no longer mapped, so keep the enum at its unmapped value.
      this.__mapState = "unmapped";
      this.__data = new Uint8Array(0);
    }
    get [Symbol.toStringTag]() { return "GPUBuffer"; }
  }

  class GPUCommandBuffer {
    constructor(device, commands, label, invalid, token) {
      if (token !== gpuCommandConstructionToken) throw new TypeError("Illegal constructor");
      this.__device = device;
      this.__commands = commands;
      this.__invalid = !!invalid;
      this.__submitted = false;
      this.__label = label;
    }
    get label() { return this.__label; }
    set label(value) { this.__label = String(value); }
    get [Symbol.toStringTag]() { return "GPUCommandBuffer"; }
  }

  class GPUCommandEncoder {
    constructor(device, descriptor, token) {
      if (token !== gpuDeviceConstructionToken) throw new TypeError("Illegal constructor");
      this.__device = device;
      this.__commands = [];
      this.__finished = false;
      this.__invalid = false;
      this.__label = descriptor && descriptor.label !== undefined ? String(descriptor.label) : "";
    }
    get label() { return this.__label; }
    set label(value) { this.__label = String(value); }
    __ensureOpen() {
      if (this.__finished) throw webgpuOperationError("The GPUCommandEncoder is already finished.");
      return webgpuDeviceActive(this.__device, "GPUCommandEncoder");
    }
    copyBufferToBuffer(source, sourceOffset, destination, destinationOffset, size) {
      if (!this.__ensureOpen()) return;
      const srcOffset = Number(sourceOffset), dstOffset = Number(destinationOffset), length = Number(size);
      const validBuffer = value => value instanceof GPUBuffer && value.__device === this.__device &&
        !value.__destroyed && !value.__invalid && value.mapState === "unmapped";
      if (!validBuffer(source) || !validBuffer(destination) ||
          !Number.isFinite(srcOffset) || !Number.isFinite(dstOffset) || !Number.isFinite(length) ||
          Math.trunc(srcOffset) !== srcOffset || Math.trunc(dstOffset) !== dstOffset || Math.trunc(length) !== length ||
          srcOffset < 0 || dstOffset < 0 || length < 0 || srcOffset % 4 || dstOffset % 4 || length % 4 ||
          srcOffset + length > source.size || dstOffset + length > destination.size ||
          !(source.usage & WEBGPU_BUFFER_USAGE.COPY_SRC) || !(destination.usage & WEBGPU_BUFFER_USAGE.COPY_DST)) {
        this.__invalid = true;
        webgpuRecordError(this.__device, "validation", "copyBufferToBuffer arguments are invalid.");
        return;
      }
      this.__commands.push({ type: "copy", source, sourceOffset: srcOffset, destination,
        destinationOffset: dstOffset, size: length });
    }
    clearBuffer(buffer, offset = 0, size = undefined) {
      if (!this.__ensureOpen()) return;
      const start = Number(offset);
      const length = size === undefined ? (buffer && buffer.size - start) : Number(size);
      if (!(buffer instanceof GPUBuffer) || buffer.__device !== this.__device || buffer.__destroyed || buffer.__invalid ||
          buffer.mapState !== "unmapped" ||
          !Number.isFinite(start) || !Number.isFinite(length) || Math.trunc(start) !== start || Math.trunc(length) !== length ||
          start < 0 || length < 0 || start % 4 || length % 4 || start + length > buffer.size ||
          !(buffer.usage & WEBGPU_BUFFER_USAGE.COPY_DST)) {
        this.__invalid = true;
        webgpuRecordError(this.__device, "validation", "clearBuffer arguments are invalid.");
        return;
      }
      this.__commands.push({ type: "clear", buffer, offset: start, size: length });
    }
    finish(descriptor = undefined) {
      if (this.__finished) throw webgpuOperationError("The GPUCommandEncoder is already finished.");
      this.__finished = true;
      const label = descriptor && descriptor.label !== undefined ? String(descriptor.label) : this.__label;
      return new GPUCommandBuffer(this.__device, this.__commands.slice(), label, this.__invalid, gpuCommandConstructionToken);
    }
    get [Symbol.toStringTag]() { return "GPUCommandEncoder"; }
  }

  class GPUQueue {
    constructor(device, descriptor, token) {
      if (token !== gpuDeviceConstructionToken) throw new TypeError("Illegal constructor");
      this.__device = device;
      this.__label = descriptor && descriptor.label !== undefined ? String(descriptor.label) : "";
    }
    get label() { return this.__label; }
    set label(value) { this.__label = String(value); }
    writeBuffer(buffer, bufferOffset, data, dataOffset = 0, size = undefined) {
      if (!webgpuDeviceActive(this.__device, "GPUQueue.writeBuffer")) return;
      const bytes = webgpuBytes(data);
      const destination = Number(bufferOffset);
      // Both dataOffset and size are byte counts into the supplied
      // ArrayBuffer/ArrayBufferView, independent of the view's element width.
      const source = Number(dataOffset);
      const length = size === undefined ? bytes.byteLength - source : Number(size);
      if (!(buffer instanceof GPUBuffer) || buffer.__device !== this.__device || buffer.__invalid || buffer.__destroyed ||
          buffer.mapState !== "unmapped" || !Number.isFinite(destination) || !Number.isFinite(source) || !Number.isFinite(length) ||
          Math.trunc(destination) !== destination || Math.trunc(source) !== source || Math.trunc(length) !== length ||
          destination < 0 || source < 0 || length < 0 || destination % 4 || source % 4 || length % 4 ||
          source + length > bytes.byteLength || destination + length > buffer.size ||
          !(buffer.usage & WEBGPU_BUFFER_USAGE.COPY_DST)) {
        webgpuRecordError(this.__device, "validation", "writeBuffer arguments are invalid.");
        return;
      }
      buffer.__data.set(bytes.subarray(source, source + length), destination);
    }
    submit(commandBuffers) {
      if (!webgpuDeviceActive(this.__device, "GPUQueue.submit")) return;
      let list;
      try { list = Array.from(commandBuffers); }
      catch (_) { throw new TypeError("commandBuffers must be iterable"); }
      for (const command of list) {
        if (!(command instanceof GPUCommandBuffer) || command.__device !== this.__device || command.__submitted) {
          webgpuRecordError(this.__device, "validation", "GPUQueue.submit received an invalid command buffer.");
          continue;
        }
        command.__submitted = true;
        if (command.__invalid) continue;
        for (const entry of command.__commands) {
          if (entry.type === "copy") {
            const valid = entry.source instanceof GPUBuffer && entry.destination instanceof GPUBuffer &&
              entry.source.__device === this.__device && entry.destination.__device === this.__device &&
              !entry.source.__destroyed && !entry.destination.__destroyed &&
              !entry.source.__invalid && !entry.destination.__invalid &&
              entry.source.mapState === "unmapped" && entry.destination.mapState === "unmapped" &&
              entry.sourceOffset + entry.size <= entry.source.size && entry.destinationOffset + entry.size <= entry.destination.size;
            if (!valid) {
              webgpuRecordError(this.__device, "validation", "copyBufferToBuffer resources are no longer valid at submit time.");
              continue;
            }
            const bytes = entry.source.__data.slice(entry.sourceOffset, entry.sourceOffset + entry.size);
            entry.destination.__data.set(bytes, entry.destinationOffset);
          } else if (entry.type === "clear") {
            if (!(entry.buffer instanceof GPUBuffer) || entry.buffer.__device !== this.__device ||
                entry.buffer.__destroyed || entry.buffer.__invalid || entry.buffer.mapState !== "unmapped" ||
                entry.offset + entry.size > entry.buffer.size) {
              webgpuRecordError(this.__device, "validation", "clearBuffer resources are no longer valid at submit time.");
              continue;
            }
            entry.buffer.__data.fill(0, entry.offset, entry.offset + entry.size);
          }
        }
      }
    }
    onSubmittedWorkDone() {
      if (this.__device.__destroyed) return Promise.reject(webgpuOperationError("The device is lost."));
      return new Promise(resolve => queueMicrotask(resolve));
    }
    get [Symbol.toStringTag]() { return "GPUQueue"; }
  }

  class GPUDevice extends GPUEventTarget {
    constructor(adapter, descriptor, token) {
      if (token !== gpuDeviceConstructionToken) throw new TypeError("Illegal constructor");
      super();
      this.__adapter = adapter;
      this.__destroyed = false;
      this.__uncapturedSuppressed = false;
      this.__errorScopes = [];
      this.__buffers = [];
      this.__label = descriptor && descriptor.label !== undefined ? String(descriptor.label) : "";
      Object.defineProperties(this, {
        features: {
          configurable: false, enumerable: true, writable: false,
          value: new GPUSupportedFeatures(gpuAdapterConstructionToken, adapter.__features),
        },
        limits: {
          configurable: false, enumerable: true, writable: false,
          value: new GPUSupportedLimits(gpuDeviceConstructionToken, adapter.__limits),
        },
        queue: {
          configurable: false, enumerable: true, writable: false,
          value: new GPUQueue(this, descriptor && descriptor.defaultQueue || {}, gpuDeviceConstructionToken),
        },
      });
      this.__onuncapturederror = null;
      this.__lostResolve = null;
      this.lost = new Promise(resolve => { this.__lostResolve = resolve; });
    }
    get label() { return this.__label; }
    set label(value) { this.__label = String(value); }
    createBuffer(descriptor = undefined) {
      if (!isDictionary(descriptor)) throw new TypeError("GPUBufferDescriptor is required");
      const values = descriptor || {};
      if (!webgpuDeviceActive(this, "GPUDevice.createBuffer")) {
        const buffer = new GPUBuffer(this, { ...values, size: 0, usage: 0 }, gpuBufferConstructionToken, true);
        this.__buffers.push(buffer);
        return buffer;
      }
      const size = Number(values.size);
      const usage = Number(values.usage);
      let invalid = false;
      if (!Number.isFinite(size) || Math.trunc(size) !== size || size <= 0 || size > this.limits.maxBufferSize || size % 4) {
        invalid = true;
        webgpuRecordError(this, "validation", "GPUBuffer size must be a positive multiple of four within maxBufferSize.");
      }
      const knownUsage = Object.values(WEBGPU_BUFFER_USAGE).reduce((bits, value) => bits | value, 0);
      if (!Number.isFinite(usage) || Math.trunc(usage) !== usage || usage <= 0 || (usage & ~knownUsage)) {
        invalid = true;
        webgpuRecordError(this, "validation", "GPUBuffer usage contains unsupported bits.");
      }
      if ((usage & WEBGPU_BUFFER_USAGE.MAP_READ) && (usage & ~ (WEBGPU_BUFFER_USAGE.MAP_READ | WEBGPU_BUFFER_USAGE.COPY_DST))) {
        invalid = true;
        webgpuRecordError(this, "validation", "MAP_READ buffers may only use COPY_DST.");
      }
      if ((usage & WEBGPU_BUFFER_USAGE.MAP_WRITE) && (usage & ~ (WEBGPU_BUFFER_USAGE.MAP_WRITE | WEBGPU_BUFFER_USAGE.COPY_SRC))) {
        invalid = true;
        webgpuRecordError(this, "validation", "MAP_WRITE buffers may only use COPY_SRC.");
      }
      if (values.mappedAtCreation && !(usage & WEBGPU_BUFFER_USAGE.MAP_WRITE)) {
        invalid = true;
        webgpuRecordError(this, "validation", "mappedAtCreation requires MAP_WRITE usage.");
      }
      const buffer = new GPUBuffer(this, { ...values, size: invalid ? 0 : size, usage }, gpuBufferConstructionToken, invalid);
      this.__buffers.push(buffer);
      return buffer;
    }
    createCommandEncoder(descriptor = undefined) {
      if (descriptor !== undefined && !isDictionary(descriptor)) throw new TypeError("GPUCommandEncoderDescriptor must be a dictionary");
      if (!webgpuDeviceActive(this, "GPUDevice.createCommandEncoder")) {
        return new GPUCommandEncoder(this, {}, gpuDeviceConstructionToken);
      }
      return new GPUCommandEncoder(this, descriptor || {}, gpuDeviceConstructionToken);
    }
    pushErrorScope(filter) {
      const value = String(filter);
      if (value !== "validation" && value !== "out-of-memory" && value !== "internal") {
        throw new TypeError("GPU error scope filter is invalid");
      }
      this.__errorScopes.push({ filter: value, error: null });
    }
    popErrorScope() {
      if (!this.__errorScopes.length) return Promise.reject(webgpuOperationError("No GPU error scope is active."));
      const scope = this.__errorScopes.pop();
      return Promise.resolve(scope.error);
    }
    destroy() {
      if (this.__destroyed) return;
      this.__destroyed = true;
      for (const buffer of this.__buffers) buffer.destroy();
      this.__uncapturedSuppressed = true;
      this.__lostResolve(new GPUDeviceLostInfo(gpuErrorConstructionToken, "destroyed", "Device was destroyed."));
    }
    get onuncapturederror() { return this.__onuncapturederror; }
    set onuncapturederror(callback) {
      if (this.__onuncapturederror) this.removeEventListener("uncapturederror", this.__onuncapturederror);
      this.__onuncapturederror = typeof callback === "function" ? callback : null;
      if (this.__onuncapturederror) this.addEventListener("uncapturederror", this.__onuncapturederror);
    }
    get [Symbol.toStringTag]() { return "GPUDevice"; }
  }

  class GPUAdapter {
    constructor(options, token) {
      if (token !== gpuConstructionToken) throw new TypeError("Illegal constructor");
      this.__features = WEBGPU_FEATURES.slice();
      this.__limits = { ...WEBGPU_LIMITS };
      this.__fallback = !!options.forceFallbackAdapter;
      this.__powerPreference = options.powerPreference || "low-power";
      this.__info = new GPUAdapterInfo(gpuAdapterConstructionToken, {
        architecture: "deterministic",
        description: "Omoikane deterministic WebGPU adapter",
        device: "software",
        vendor: "Omoikane",
      });
      Object.defineProperties(this, {
        features: {
          configurable: false, enumerable: true, writable: false,
          value: new GPUSupportedFeatures(gpuAdapterConstructionToken, this.__features),
        },
        limits: {
          configurable: false, enumerable: true, writable: false,
          value: new GPUSupportedLimits(gpuAdapterConstructionToken, this.__limits),
        },
      });
    }
    get isFallbackAdapter() { return this.__fallback; }
    get info() { return this.__info; }
    requestAdapterInfo() { return Promise.resolve(this.__info); }
    requestDevice(descriptor = undefined) {
      return Promise.resolve().then(() => {
        if (descriptor !== undefined && !isDictionary(descriptor)) throw new TypeError("GPUDeviceDescriptor must be a dictionary");
        const values = descriptor || {};
        let features;
        try { features = values.requiredFeatures === undefined ? [] : Array.from(values.requiredFeatures, String); }
        catch (_) { throw new TypeError("requiredFeatures must be iterable"); }
        for (const feature of features) {
          if (!this.__features.includes(feature)) throw new DOMException("Unsupported required feature: " + feature, "NotSupportedError");
        }
        const requiredLimits = values.requiredLimits === undefined || values.requiredLimits === null ? {} : values.requiredLimits;
        if (!isDictionary(requiredLimits)) throw new TypeError("requiredLimits must be a dictionary");
        for (const name of Object.keys(requiredLimits)) {
          if (!Object.prototype.hasOwnProperty.call(this.__limits, name)) {
            throw new TypeError("Unknown GPU limit: " + name);
          }
          const requested = Number(requiredLimits[name]);
          if (!Number.isFinite(requested) || Math.trunc(requested) !== requested || requested < 0 || requested > this.__limits[name]) {
            throw new DOMException("Required GPU limit is unsupported: " + name, "NotSupportedError");
          }
        }
        const queue = values.defaultQueue === undefined || values.defaultQueue === null ? {} : values.defaultQueue;
        if (!isDictionary(queue)) throw new TypeError("defaultQueue must be a dictionary");
        return new GPUDevice(this, { ...values, defaultQueue: queue }, gpuDeviceConstructionToken);
      });
    }
    get [Symbol.toStringTag]() { return "GPUAdapter"; }
  }

  class GPU {
    constructor(token) {
      if (token !== gpuConstructionToken) throw new TypeError("Illegal constructor");
    }
    requestAdapter(options = undefined) {
      return Promise.resolve().then(() => {
        if (options !== undefined && !isDictionary(options)) throw new TypeError("GPURequestAdapterOptions must be a dictionary");
        const values = options || {};
        const power = values.powerPreference === undefined ? "low-power" : String(values.powerPreference);
        if (power !== "low-power" && power !== "high-performance") {
          throw new TypeError("powerPreference must be low-power or high-performance");
        }
        if (values.forceFallbackAdapter !== undefined && typeof values.forceFallbackAdapter !== "boolean") {
          throw new TypeError("forceFallbackAdapter must be boolean");
        }
        if (values.xrCompatible !== undefined && Boolean(values.xrCompatible)) {
          throw new DOMException("XR-compatible adapters are not supported.", "NotSupportedError");
        }
        return new GPUAdapter({ powerPreference: power, forceFallbackAdapter: !!values.forceFallbackAdapter }, gpuConstructionToken);
      });
    }
    getPreferredCanvasFormat() { return "rgba8unorm"; }
    get [Symbol.toStringTag]() { return "GPU"; }
  }

  globalThis.GPU = GPU;
  globalThis.GPUAdapter = GPUAdapter;
  globalThis.GPUAdapterInfo = GPUAdapterInfo;
  globalThis.GPUSupportedFeatures = GPUSupportedFeatures;
  globalThis.GPUSupportedLimits = GPUSupportedLimits;
  globalThis.GPUDevice = GPUDevice;
  globalThis.GPUQueue = GPUQueue;
  globalThis.GPUBuffer = GPUBuffer;
  globalThis.GPUCommandEncoder = GPUCommandEncoder;
  globalThis.GPUCommandBuffer = GPUCommandBuffer;
  globalThis.GPUError = GPUError;
  globalThis.GPUValidationError = GPUValidationError;
  globalThis.GPUOutOfMemoryError = GPUOutOfMemoryError;
  globalThis.GPUInternalError = GPUInternalError;
  globalThis.GPUDeviceLostInfo = GPUDeviceLostInfo;
  globalThis.GPUUncapturedErrorEvent = GPUUncapturedErrorEvent;
  globalThis.GPUBufferUsage = WEBGPU_BUFFER_USAGE;
  globalThis.GPUMapMode = WEBGPU_MAP_MODE;
  globalThis.GPUShaderStage = WEBGPU_SHADER_STAGE;
  globalThis.GPUTextureUsage = WEBGPU_TEXTURE_USAGE;

  class Navigator {
    constructor(token) {
      if (token !== navigatorConstructionToken) throw new TypeError("Illegal constructor");
      this.userAgent = __omoikane_navigator_user_agent;
      this.language = "en";
      this.languages = ["en"];
      this.platform = "";
      this.cookieEnabled = true;
      this.onLine = true;
      this.plugins = new PluginArray(navigatorConstructionToken);
      this.mimeTypes = new MimeTypeArray(navigatorConstructionToken);
      this.clipboard = new Clipboard(clipboardConstructionToken);
      this.geolocation = new Geolocation(geolocationConstructionToken);
      this.serviceWorker = new ServiceWorkerContainer(serviceWorkerContainerConstructionToken);
      // The Permissions object is installed after EventTarget is defined
      // below.  Keep the property present while Navigator is constructed so
      // its shape is stable during bootstrap and in worker globals.
      this.permissions = null;
      this.gpu = new GPU(gpuConstructionToken);
    }
    get [Symbol.toStringTag]() { return "Navigator"; }
  }
  globalThis.PluginArray = PluginArray;
  globalThis.MimeTypeArray = MimeTypeArray;
  globalThis.Navigator = Navigator;
  globalThis.Clipboard = Clipboard;
  globalThis.Geolocation = Geolocation;
  globalThis.GeolocationCoordinates = GeolocationCoordinates;
  globalThis.GeolocationPosition = GeolocationPosition;
  globalThis.GeolocationPositionError = GeolocationPositionError;
  globalThis.navigator = new Navigator(navigatorConstructionToken);
  Object.defineProperty(globalThis, "isSecureContext", {
    configurable: true,
    enumerable: true,
    get() { return nativeIsSecureContext(); },
  });
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
          let code = char.codePointAt(0);
          // An unpaired surrogate has no UTF-8 encoding; the Encoding standard
          // replaces it with U+FFFD rather than emitting a surrogate sequence.
          if (code >= 0xd800 && code <= 0xdfff) code = 0xfffd;
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
  function resolveNetworkUrl(value) {
    const raw = String(value);
    const resolved = String(__omoikane_resolve_url(raw));
    if (/^[A-Za-z][A-Za-z0-9+.-]*:\/\//.test(resolved)) return resolved;
    return new URL(raw, document.URL).href;
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
  // Enqueues `callback` on the microtask queue.
  //
  // Promise reactions already run on that queue, so resolving an
  // already-resolved promise puts the callback exactly one checkpoint away —
  // the same place `queueMicrotask` is defined to put it. Interleaving the two
  // therefore preserves registration order, and a microtask queued from inside
  // a microtask still runs before the next task.
  //
  // The wrapper matters: `then(callback)` would hand the callback the promise's
  // resolution value, while this callback takes no arguments.
  globalThis.queueMicrotask = function queueMicrotask(callback) {
    if (typeof callback !== "function") {
      throw new TypeError("queueMicrotask requires a callback function");
    }
    Promise.resolve().then(() => callback());
  };
  globalThis.alert = function alert(message) {
    return __omoikane_open_javascript_dialog(
      "alert",
      arguments.length === 0 ? "" : String(message),
      "",
    );
  };
  globalThis.confirm = function confirm(message) {
    return __omoikane_open_javascript_dialog(
      "confirm",
      message === undefined ? "" : String(message),
      "",
    );
  };
  globalThis.prompt = function prompt(message, defaultValue) {
    return __omoikane_open_javascript_dialog(
      "prompt",
      message === undefined ? "" : String(message),
      defaultValue === undefined ? "" : String(defaultValue),
    );
  };
  globalThis.innerWidth = 1280;
  globalThis.innerHeight = 720;
  globalThis.outerWidth = 1280;
  globalThis.outerHeight = 720;
  globalThis.screenX = 0;
  globalThis.screenY = 0;
  globalThis.devicePixelRatio = 1;
  function windowScrollOffset() {
    try {
      return JSON.parse(__omoikane_window_scroll_offset());
    } catch (_) {
      return { x: 0, y: 0 };
    }
  }
  function applyWindowScroll(x, y) {
    __omoikane_set_window_scroll(Number(x), Number(y));
  }
  globalThis.__omoikane_dispatch_scroll_event = function(nodeId, viewport) {
    const target = viewport ? document : wrapNode(nodeId);
    if (target) target.dispatchEvent(new Event("scroll", { bubbles: !!viewport }));
  };
  function isScrollOptions(value) {
    return value !== null && (typeof value === "object" || typeof value === "function");
  }
  globalThis.scrollTo = function scrollTo(xOrOptions, y) {
    const current = windowScrollOffset();
    if (isScrollOptions(xOrOptions)) {
      const left = xOrOptions.left === undefined ? current.x : Number(xOrOptions.left);
      const top = xOrOptions.top === undefined ? current.y : Number(xOrOptions.top);
      applyWindowScroll(left, top);
    } else {
      applyWindowScroll(Number(xOrOptions), Number(y));
    }
  };
  globalThis.scroll = globalThis.scrollTo;
  globalThis.scrollBy = function scrollBy(xOrOptions, y) {
    const current = windowScrollOffset();
    if (isScrollOptions(xOrOptions)) {
      const left = xOrOptions.left === undefined ? 0 : Number(xOrOptions.left);
      const top = xOrOptions.top === undefined ? 0 : Number(xOrOptions.top);
      applyWindowScroll(current.x + left, current.y + top);
    } else {
      applyWindowScroll(current.x + Number(xOrOptions), current.y + Number(y));
    }
  };
  Object.defineProperties(globalThis, {
    scrollX: { configurable: true, enumerable: true, get() { return windowScrollOffset().x; } },
    scrollY: { configurable: true, enumerable: true, get() { return windowScrollOffset().y; } },
    pageXOffset: { configurable: true, enumerable: true, get() { return windowScrollOffset().x; } },
    pageYOffset: { configurable: true, enumerable: true, get() { return windowScrollOffset().y; } },
  });
  globalThis.screen = { width: 1280, height: 720, availWidth: 1280, availHeight: 720, colorDepth: 24, pixelDepth: 24 };

  // Origin-scoped Web Storage. The backing areas live in the browser host so
  // localStorage survives Runtime replacement and sessionStorage follows the
  // top-level browsing session. Wrappers remain document/window specific.
  const documentStorage = new WeakMap();
  function storageOrigin(doc) {
    const origin = __omoikane_storage_origin(doc.__id);
    if (origin === null) {
      throw new DOMException("Storage is unavailable for an opaque origin.", "SecurityError");
    }
    return origin;
  }
  function dispatchStorageChange(kind, sourceDocument, sourceWindow, key, oldValue, newValue) {
    const origin = storageOrigin(sourceDocument);
    for (const targetWindow of liveBrowsingWindows()) {
      if (targetWindow === sourceWindow) continue;
      const targetDocument = targetWindow.document;
      if (!targetDocument || __omoikane_storage_origin(targetDocument.__id) !== origin) continue;
      const storageArea = storageForDocument(kind, targetDocument, targetWindow);
      targetWindow.dispatchEvent(new StorageEvent("storage", {
        key,
        oldValue,
        newValue,
        url: sourceDocument.URL,
        storageArea,
      }));
    }
  }
  class Storage {
    constructor(kind, doc, ownerWindow) {
      this.__kind = kind;
      this.__document = doc;
      this.__ownerWindow = ownerWindow;
    }
    get length() {
      storageOrigin(this.__document);
      return __omoikane_storage_length(this.__kind, this.__document.__id);
    }
    key(index) {
      storageOrigin(this.__document);
      return __omoikane_storage_key(this.__kind, this.__document.__id, Number(index) >>> 0);
    }
    getItem(key) {
      storageOrigin(this.__document);
      return __omoikane_storage_get(this.__kind, this.__document.__id, String(key));
    }
    setItem(key, value) {
      storageOrigin(this.__document);
      key = String(key);
      value = String(value);
      const oldValue = __omoikane_storage_set(this.__kind, this.__document.__id, key, value);
      if (oldValue !== value) {
        dispatchStorageChange(
          this.__kind, this.__document, this.__ownerWindow, key, oldValue, value
        );
      }
    }
    removeItem(key) {
      storageOrigin(this.__document);
      key = String(key);
      const oldValue = __omoikane_storage_remove(this.__kind, this.__document.__id, key);
      if (oldValue !== null) {
        dispatchStorageChange(
          this.__kind, this.__document, this.__ownerWindow, key, oldValue, null
        );
      }
    }
    clear() {
      storageOrigin(this.__document);
      if (__omoikane_storage_clear(this.__kind, this.__document.__id)) {
        dispatchStorageChange(
          this.__kind, this.__document, this.__ownerWindow, null, null, null
        );
      }
    }
  }
  function storageForDocument(kind, doc, ownerWindow) {
    let areas = documentStorage.get(doc);
    if (!areas) {
      areas = {};
      documentStorage.set(doc, areas);
    }
    return areas[kind] || (areas[kind] = new Storage(kind, doc, ownerWindow));
  }
  globalThis.Storage = Storage;
  globalThis.StorageEvent = StorageEvent;
  globalThis.localStorage = storageForDocument("local", document, globalThis);
  globalThis.sessionStorage = storageForDocument("session", document, globalThis);

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
    if (typeof globalThis.__omoikane_layout_observers_changed === "function") {
      globalThis.__omoikane_layout_observers_changed();
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

  // Boa does not currently implement locale-aware Date formatting. Pages commonly
  // use this API for diagnostic timestamps, so provide a deterministic fallback.
  Date.prototype.toLocaleTimeString = function() {
    const hours = String(this.getHours()).padStart(2, "0");
    const minutes = String(this.getMinutes()).padStart(2, "0");
    const seconds = String(this.getSeconds()).padStart(2, "0");
    return hours + ":" + minutes + ":" + seconds;
  };

  // Performance Timeline, User Timing, Resource/Navigation Timing, and
  // observation of all entries implemented by this runtime.  Network timing is
  // recorded at the JS/native boundary below (fetch/XHR and resource element
  // completion), so every entry uses the host's monotonic clock and the same
  // time origin as User Timing.
  const performanceEntryToken = Symbol("PerformanceEntry internal constructor");
  const nativePerformanceNow = __omoikane_performance_now;
  const navigationTimingValues = new WeakMap();

  function navigationTimingState(target) {
    let state = navigationTimingValues.get(target);
    if (!state) {
      state = Object.create(null);
      navigationTimingValues.set(target, state);
    }
    return state;
  }

  function defineNavigationTimingField(target, field, value) {
    const state = navigationTimingState(target);
    state[field] = value;
    if (Object.prototype.hasOwnProperty.call(target, field)) return;
    Object.defineProperty(target, field, {
      get() {
        const state = navigationTimingValues.get(this);
        return state ? state[field] : 0;
      },
      enumerable: true,
      configurable: false,
    });
  }

  function isDictionary(value) {
    return value !== null && (typeof value === "object" || typeof value === "function");
  }

  class PerformanceEntry {
    constructor(token, name, entryType, startTime, duration) {
      if (token !== performanceEntryToken) throw new TypeError("Illegal constructor");
      Object.defineProperties(this, {
        name: { value: String(name), enumerable: true },
        entryType: { value: String(entryType), enumerable: true },
        startTime: { value: Number(startTime), enumerable: true },
      });
      if (String(entryType) === "navigation") {
        defineNavigationTimingField(this, "duration", Number(duration));
      } else {
        Object.defineProperty(this, "duration", {
          value: Number(duration),
          enumerable: true,
        });
      }
    }
    toJSON() {
      return {
        name: this.name,
        entryType: this.entryType,
        startTime: this.startTime,
        duration: this.duration,
      };
    }
  }

  class PerformanceMark extends PerformanceEntry {
    constructor(name, options = {}) {
      options = options ?? {};
      if (!isDictionary(options)) throw new TypeError("PerformanceMark options must be a dictionary");
      const startTime = options.startTime === undefined
        ? nativePerformanceNow()
        : Number(options.startTime);
      if (Number.isNaN(startTime) || startTime < 0) {
        throw new TypeError("PerformanceMark startTime must be a non-negative number");
      }
      super(performanceEntryToken, name, "mark", startTime, 0);
      Object.defineProperty(this, "detail", { value: options.detail ?? null, enumerable: true });
    }
    toJSON() { return { ...super.toJSON(), detail: this.detail }; }
  }

  class PerformanceMeasure extends PerformanceEntry {
    constructor(token, name, startTime, duration, detail = null) {
      if (token !== performanceEntryToken) throw new TypeError("Illegal constructor");
      super(performanceEntryToken, name, "measure", startTime, duration);
      Object.defineProperty(this, "detail", { value: detail, enumerable: true });
    }
    toJSON() { return { ...super.toJSON(), detail: this.detail }; }
  }

  const performanceResourceTimingNumericFields = [
    "workerStart", "redirectStart", "redirectEnd", "fetchStart",
    "domainLookupStart", "domainLookupEnd", "connectStart", "connectEnd",
    "secureConnectionStart", "requestStart", "responseStart", "responseEnd",
    "transferSize", "encodedBodySize", "decodedBodySize", "responseStatus",
  ];
  const performanceNavigationTimingNumericFields = [
    "unloadEventStart", "unloadEventEnd", "domInteractive",
    "domContentLoadedEventStart", "domContentLoadedEventEnd", "domComplete",
    "loadEventStart", "loadEventEnd", "activationStart", "criticalCHRestart",
  ];
  const performanceResourceTimingStringFields = [
    "initiatorType", "nextHopProtocol", "renderBlockingStatus",
  ];

  function definePerformanceTimingFields(target, options, fields) {
    const isNavigation = options.entryType === "navigation" ||
      fields === performanceNavigationTimingNumericFields;
    for (const field of fields) {
      const value = options[field] === undefined ? 0 : Number(options[field]);
      const normalized = Number.isFinite(value) && value >= 0 ? value : 0;
      if (isNavigation) {
        defineNavigationTimingField(target, field, normalized);
      } else {
        Object.defineProperty(target, field, {
          value: normalized,
          enumerable: true,
        });
      }
    }
    for (const field of performanceResourceTimingStringFields) {
      if (fields !== performanceNavigationTimingNumericFields &&
          options[field] === undefined && field === "renderBlockingStatus") continue;
      const value = options[field] === undefined ? "" : String(options[field]);
      if (isNavigation) {
        defineNavigationTimingField(target, field, value);
      } else {
        Object.defineProperty(target, field, {
          value,
          enumerable: true,
        });
      }
    }
  }

  function performanceTimingJSON(entry, fields) {
    const result = entry.toJSONBase();
    for (const field of performanceResourceTimingNumericFields) result[field] = entry[field];
    for (const field of performanceNavigationTimingNumericFields) {
      if (field in entry) result[field] = entry[field];
    }
    for (const field of performanceResourceTimingStringFields) {
      if (field in entry) result[field] = entry[field];
    }
    if (fields === "navigation") {
      result.type = entry.type;
      result.redirectCount = entry.redirectCount;
    }
    if ("serverTiming" in entry) result.serverTiming = entry.serverTiming.slice();
    return result;
  }

  class PerformanceResourceTiming extends PerformanceEntry {
    constructor(token, options = {}) {
      if (token !== performanceEntryToken) throw new TypeError("Illegal constructor");
      const startTime = Number(options.startTime ?? 0);
      const responseEnd = Number(options.responseEnd ?? startTime);
      const safeStart = Number.isFinite(startTime) && startTime >= 0 ? startTime : 0;
      const safeEnd = Number.isFinite(responseEnd) && responseEnd >= safeStart ? responseEnd : safeStart;
      super(performanceEntryToken, options.name ?? "", options.entryType ?? "resource", safeStart, safeEnd - safeStart);
      definePerformanceTimingFields(this, { ...options, responseEnd: safeEnd }, performanceResourceTimingNumericFields);
      Object.defineProperty(this, "serverTiming", {
        value: Object.freeze(Array.isArray(options.serverTiming) ? options.serverTiming.slice() : []),
        enumerable: true,
      });
    }
    toJSONBase() { return super.toJSON(); }
    toJSON() { return performanceTimingJSON(this, "resource"); }
  }

  class PerformanceNavigationTiming extends PerformanceResourceTiming {
    constructor(token, options = {}) {
      if (token !== performanceEntryToken) throw new TypeError("Illegal constructor");
      super(token, { ...options, entryType: "navigation" });
      definePerformanceTimingFields(this, options, performanceNavigationTimingNumericFields);
      Object.defineProperty(this, "type", {
        value: options.type === undefined ? "navigate" : String(options.type),
        enumerable: true,
      });
      Object.defineProperty(this, "redirectCount", {
        value: Math.max(0, Math.trunc(Number(options.redirectCount) || 0)),
        enumerable: true,
      });
    }
    toJSON() { return performanceTimingJSON(this, "navigation"); }
  }

  const performanceEntries = [];
  let performanceEntrySequence = 0;
  const performanceObservers = [];
  let pendingPerformanceObservers = [];
  let performanceObserverDeliveryScheduled = false;
  let resourceTimingBufferSize = 250;
  let resourceTimingBufferFull = false;
  let resourceEntryCount = 0;
  const resourceTimingTextEncoder = new TextEncoder();
  const finishedResourceTimings = new WeakSet();

  function sortedPerformanceEntries(entries) {
    return entries.slice().sort((a, b) =>
      a.startTime - b.startTime || a.__sequence - b.__sequence);
  }

  const performanceObserverEntryListToken = Symbol("PerformanceObserverEntryList internal constructor");
  class PerformanceObserverEntryList {
    constructor(token, entries) {
      if (token !== performanceObserverEntryListToken) throw new TypeError("Illegal constructor");
      this._entries = sortedPerformanceEntries(entries);
    }
    getEntries() { return this._entries.slice(); }
    getEntriesByType(type) {
      const normalized = String(type);
      return this._entries.filter(entry => entry.entryType === normalized);
    }
    getEntriesByName(name, type) {
      const normalizedName = String(name);
      const normalizedType = type === undefined ? null : String(type);
      return this._entries.filter(entry =>
        entry.name === normalizedName && (normalizedType === null || entry.entryType === normalizedType));
    }
  }

  function schedulePerformanceObserver(observer) {
    if (!pendingPerformanceObservers.includes(observer)) pendingPerformanceObservers.push(observer);
    if (performanceObserverDeliveryScheduled) return;
    performanceObserverDeliveryScheduled = true;
    Promise.resolve().then(() => {
      performanceObserverDeliveryScheduled = false;
      const pending = pendingPerformanceObservers;
      pendingPerformanceObservers = [];
      for (const current of pending) {
        const records = current.takeRecords();
        if (!records.length) continue;
        const list = new PerformanceObserverEntryList(performanceObserverEntryListToken, records);
        // An exception from one callback must not prevent other observers, or a
        // later delivery to the same observer, from running.
        try { current._callback.call(undefined, list, current); } catch (_) {}
      }
    });
  }

  class PerformanceObserver {
    constructor(callback) {
      if (typeof callback !== "function") throw new TypeError("PerformanceObserver callback must be callable");
      this._callback = callback;
      this._queue = [];
      this._entryTypes = new Set();
      this._mode = undefined;
      performanceObservers.push(this);
    }
    observe(options) {
      if (arguments.length === 0 || !isDictionary(options)) {
        throw new TypeError("PerformanceObserver options must be a dictionary");
      }
      const hasEntryTypes = options.entryTypes !== undefined;
      const hasType = options.type !== undefined;
      if (hasEntryTypes === hasType) {
        throw new TypeError("Specify exactly one of entryTypes or type");
      }
      if (hasEntryTypes) {
        if (this._mode === "single") {
          throw new DOMException("Observer mode cannot be changed", "InvalidModificationError");
        }
        const requested = Array.from(options.entryTypes, type => String(type));
        if (!requested.length) throw new TypeError("entryTypes must not be empty");
        this._mode = "multiple";
        this._entryTypes = new Set(requested.filter(type =>
          PerformanceObserver.supportedEntryTypes.includes(type)));
        return;
      }
      if (this._mode === "multiple") {
        throw new DOMException("Observer mode cannot be changed", "InvalidModificationError");
      }
      const type = String(options.type);
      if (!PerformanceObserver.supportedEntryTypes.includes(type)) return;
      this._mode = "single";
      this._entryTypes.add(type);
      if (Boolean(options.buffered)) {
        this._queue.push(...performanceEntries.filter(entry => entry.entryType === type));
        if (this._queue.length) schedulePerformanceObserver(this);
      }
    }
    disconnect() {
      this._queue = [];
      this._entryTypes.clear();
      this._mode = undefined;
      const pendingIndex = pendingPerformanceObservers.indexOf(this);
      if (pendingIndex >= 0) pendingPerformanceObservers.splice(pendingIndex, 1);
    }
    takeRecords() {
      const records = sortedPerformanceEntries(this._queue);
      this._queue = [];
      return records;
    }
  }
  Object.defineProperty(PerformanceObserver, "supportedEntryTypes", {
    get() { return Object.freeze(["navigation", "resource", "mark", "measure"]); },
    enumerable: true,
  });

  function addPerformanceEntry(entry) {
    if (entry.entryType === "resource") {
      if (resourceEntryCount >= resourceTimingBufferSize) {
        if (!resourceTimingBufferFull) {
          resourceTimingBufferFull = true;
          Promise.resolve().then(() => {
            const handler = performance.onresourcetimingbufferfull;
            if (typeof handler === "function") {
              const event = new Event("resourcetimingbufferfull");
              // This event is delivered through the `on…` property rather
              // than `dispatchEvent`, so seed the propagation path that
              // `composedPath()` uses while the handler is running.
              event.__path = [{
                node: performance,
                closedRoots: [],
                target: performance,
                relatedTarget: null,
                hostTarget: false,
              }];
              event.target = performance;
              event.currentTarget = performance;
              event.eventPhase = 2;
              event.__dispatching = true;
              try { handler.call(performance, event); } catch (_) {} finally {
                event.__dispatching = false;
                event.currentTarget = null;
                event.eventPhase = 0;
                event.__path = [];
              }
            }
          });
        }
        return null;
      }
    }
    Object.defineProperty(entry, "__sequence", { value: performanceEntrySequence++ });
    performanceEntries.push(entry);
    if (entry.entryType === "resource") resourceEntryCount += 1;
    for (const observer of performanceObservers) {
      if (!observer._entryTypes.has(entry.entryType)) continue;
      observer._queue.push(entry);
      schedulePerformanceObserver(observer);
    }
    return entry;
  }

  function normalizeTimingNumber(value, fallback = 0) {
    const number = Number(value);
    return Number.isFinite(number) && number >= 0 ? number : fallback;
  }

  function resourceTimingBase64ByteLength(value) {
    const normalized = String(value).replace(/[^A-Za-z0-9+/=]/g, "");
    const padding = normalized.endsWith("==") ? 2 : normalized.endsWith("=") ? 1 : 0;
    return Math.max(0, Math.floor(normalized.length * 3 / 4) - padding);
  }

  function recordResourceTiming(options = {}) {
    const now = nativePerformanceNow();
    const startTime = normalizeTimingNumber(options.startTime, now);
    const responseEnd = normalizeTimingNumber(options.responseEnd, now);
    const responseStart = normalizeTimingNumber(options.responseStart, responseEnd);
    const fetchStart = normalizeTimingNumber(options.fetchStart, startTime);
    const requestStart = normalizeTimingNumber(options.requestStart, fetchStart);
    const timing = {
      ...options,
      name: String(options.name ?? ""),
      startTime,
      fetchStart,
      requestStart,
      responseStart: Math.max(responseStart, requestStart),
      responseEnd: Math.max(responseEnd, responseStart, requestStart),
    };
    return addPerformanceEntry(new PerformanceResourceTiming(performanceEntryToken, timing));
  }

  function beginResourceTiming(name, initiatorType, startAt = undefined) {
    const sampledStart = startAt === undefined ? nativePerformanceNow() : Number(startAt);
    const startTime = Number.isFinite(sampledStart) && sampledStart >= 0
      ? sampledStart
      : Math.max(0, nativePerformanceNow());
    return {
      name: String(name),
      initiatorType: String(initiatorType),
      startTime,
      fetchStart: startTime,
      requestStart: startTime,
      domainLookupStart: startTime,
      domainLookupEnd: startTime,
      connectStart: startTime,
      connectEnd: startTime,
      secureConnectionStart: 0,
      responseStart: startTime,
    };
  }

  function finishResourceTiming(timing, data = {}, error = false) {
    if (!timing || finishedResourceTimings.has(timing)) return null;
    finishedResourceTimings.add(timing);
    const responseEnd = data && data.responseEnd !== undefined
      ? normalizeTimingNumber(data.responseEnd, normalizeTimingNumber(timing.responseEnd, 0))
      : nativePerformanceNow();
    const responseStart = data && data.responseStart !== undefined
      ? normalizeTimingNumber(data.responseStart, responseEnd)
      : responseEnd;
    const bodyText = data && data.bodyText !== undefined ? String(data.bodyText) : "";
    const bodyBytes = data && data.bodyBase64 !== undefined && data.bodyBase64 !== null
      ? resourceTimingBase64ByteLength(data.bodyBase64)
      : resourceTimingTextEncoder.encode(bodyText).length;
    const status = error ? 0 : normalizeTimingNumber(data.responseStatus ?? data.status, 0);
    const redirected = Boolean(data.redirected);
    const redirectStart = normalizeTimingNumber(timing.startTime, 0);
    const effectiveResponseStart = redirected
      ? Math.max(redirectStart, responseStart)
      : responseStart;
    const redirectEnd = redirected
      ? Math.min(effectiveResponseStart, Math.max(redirectStart, responseEnd - 0.001))
      : 0;
    const fetchStart = redirected
      ? Math.max(normalizeTimingNumber(timing.fetchStart, redirectStart), redirectEnd)
      : timing.fetchStart;
    const requestStart = redirected
      ? Math.max(normalizeTimingNumber(timing.requestStart, fetchStart), fetchStart)
      : timing.requestStart;
    const responseName = data && data.url !== undefined && data.url !== null
      ? String(data.url)
      : "";
    return recordResourceTiming({
      ...timing,
      responseStart: effectiveResponseStart,
      responseEnd,
      fetchStart,
      requestStart,
      redirectStart: redirected ? redirectStart : 0,
      redirectEnd,
      transferSize: error ? 0 : bodyBytes,
      encodedBodySize: error ? 0 : bodyBytes,
      decodedBodySize: error ? 0 : bodyBytes,
      responseStatus: status,
      name: responseName || timing.name,
    });
  }

  function finishElementResourceTiming(element, status = 200, error = false, timing = {}) {
    if (!element || element.__resourceTimingRecorded) return;
    const fallbackSource = typeof element.getAttribute === "function"
      ? element.getAttribute("src")
      : "";
    const rawName = element.src || element.data || element.href || fallbackSource || "";
    if (!rawName) return;
    const timingData = timing && typeof timing === "object" ? timing : {};
    const effectiveName = timingData.url === undefined || timingData.url === null
      ? ""
      : String(timingData.url);
    const name = effectiveName || String(__omoikane_resolve_url(rawName));
    if (!name) return;
    const startTime = element.__resourceTimingStart === undefined
      ? Math.max(0, nativePerformanceNow() - 0.001)
      : element.__resourceTimingStart;
    const responseTime = nativePerformanceNow();
    const elapsed = normalizeTimingNumber(timingData.elapsedMs, 0);
    const responseStart = timingData.responseStart === undefined
      ? Math.max(startTime, responseTime - elapsed)
      : normalizeTimingNumber(timingData.responseStart, responseTime);
    const redirected = Boolean(timingData.redirected);
    const redirectEnd = redirected
      ? Math.min(Math.max(startTime, responseStart), Math.max(startTime, responseTime - 0.001))
      : 0;
    const fetchStart = redirected ? Math.max(startTime, redirectEnd) : startTime;
    const requestStart = redirected ? Math.max(startTime, fetchStart) : startTime;
    setElementResourceTimingState(element, "__resourceTimingRecorded", true);
    recordResourceTiming({
      name,
      initiatorType: String(element.localName || "other"),
      startTime,
      fetchStart,
      requestStart,
      responseStart,
      responseEnd: responseTime,
      redirectStart: redirected ? startTime : 0,
      redirectEnd,
      responseStatus: error ? 0 : status,
      transferSize: 0,
      encodedBodySize: 0,
      decodedBodySize: 0,
    });
  }

  function noteElementResourceStart(element) {
    if (!element) return;
    setElementResourceTimingState(element, "__resourceTimingStart", nativePerformanceNow());
    setElementResourceTimingState(element, "__resourceTimingRecorded", false);
  }

  function setElementResourceTimingState(element, field, value) {
    try {
      Object.defineProperty(element, field, {
        value,
        writable: true,
        enumerable: false,
        configurable: true,
      });
    } catch (_) {
      // If page code made the internal slot non-configurable, preserve the
      // previous best-effort behavior without breaking resource completion.
      try { element[field] = value; } catch (_) { void 0; }
    }
  }

  function resolvePerformanceTime(value, defaultValue) {
    if (value === undefined) return defaultValue;
    if (typeof value === "string") {
      for (let index = performanceEntries.length - 1; index >= 0; index--) {
        const entry = performanceEntries[index];
        if (entry.entryType === "mark" && entry.name === value) return entry.startTime;
      }
      throw new DOMException("The mark '" + value + "' does not exist.", "SyntaxError");
    }
    const time = Number(value);
    if (Number.isNaN(time) || time < 0) {
      throw new TypeError("Performance measure timestamps must be non-negative numbers");
    }
    return time;
  }

  // A fresh global starts with one navigation entry.  The native runtime has
  // already established the URL and time origin before this bootstrap runs, so
  // the initial navigation is represented by the same zero-based clock as all
  // subsequent resource entries.  Load-pipeline code may add resource entries
  // later without replacing this entry.
  let navigationEntryForLifecycle = null;
  const initialNavigationEntry = new PerformanceNavigationTiming(
    performanceEntryToken,
    {
      name: String(__omoikane_location_href),
      startTime: 0,
      fetchStart: 0,
      requestStart: 0,
      responseStart: 0,
      responseEnd: 0,
      type: "navigate",
      redirectCount: 0,
      initiatorType: "navigation",
    },
  );

  const performance = {
    timing: {},
    now() { return nativePerformanceNow(); },
    mark(name, options = {}) {
      return addPerformanceEntry(new PerformanceMark(name, options));
    },
    measure(name, startOrOptions, endMark) {
      let startTime;
      let endTime;
      let detail = null;
      if (isDictionary(startOrOptions)) {
        const options = startOrOptions;
        const hasStart = options.start !== undefined;
        const hasEnd = options.end !== undefined;
        const hasDuration = options.duration !== undefined;
        if ((!hasStart && !hasEnd) || (hasStart && hasEnd && hasDuration)) {
          throw new TypeError("PerformanceMeasure options require two of start, end, and duration");
        }
        startTime = resolvePerformanceTime(options.start, 0);
        endTime = resolvePerformanceTime(options.end, this.now());
        if (hasDuration) {
          const duration = Number(options.duration);
          if (Number.isNaN(duration) || duration < 0) {
            throw new TypeError("PerformanceMeasure duration must be a non-negative number");
          }
          if (hasStart) endTime = startTime + duration;
          else startTime = endTime - duration;
        }
        detail = options.detail ?? null;
      } else {
        startTime = resolvePerformanceTime(startOrOptions, 0);
        endTime = resolvePerformanceTime(endMark, this.now());
      }
      if (startTime < 0 || endTime < startTime) {
        throw new TypeError("PerformanceMeasure end must not precede start");
      }
      return addPerformanceEntry(new PerformanceMeasure(
        performanceEntryToken, String(name), startTime, endTime - startTime, detail));
    },
    getEntries() { return sortedPerformanceEntries(performanceEntries); },
    getEntriesByType(type) {
      const normalized = String(type);
      return sortedPerformanceEntries(performanceEntries.filter(entry => entry.entryType === normalized));
    },
    getEntriesByName(name, type) {
      const normalizedName = String(name);
      const normalizedType = type === undefined ? null : String(type);
      return sortedPerformanceEntries(performanceEntries.filter(entry =>
        entry.name === normalizedName && (normalizedType === null || entry.entryType === normalizedType)));
    },
    clearMarks(name) {
      const normalized = name === undefined ? null : String(name);
      for (let index = performanceEntries.length - 1; index >= 0; index--) {
        if (performanceEntries[index].entryType === "mark" &&
            (normalized === null || performanceEntries[index].name === normalized)) {
          performanceEntries.splice(index, 1);
        }
      }
    },
    clearMeasures(name) {
      const normalized = name === undefined ? null : String(name);
      for (let index = performanceEntries.length - 1; index >= 0; index--) {
        if (performanceEntries[index].entryType === "measure" &&
            (normalized === null || performanceEntries[index].name === normalized)) {
          performanceEntries.splice(index, 1);
        }
      }
    },
    setResourceTimingBufferSize(maxSize) {
      const numeric = Number(maxSize);
      if (!Number.isFinite(numeric) || numeric < 0) {
        throw new TypeError("The resource timing buffer size must be non-negative");
      }
      resourceTimingBufferSize = Math.floor(numeric);
      if (resourceEntryCount < resourceTimingBufferSize) {
        resourceTimingBufferFull = false;
      }
    },
    clearResourceTimings() {
      let writeIndex = 0;
      for (let readIndex = 0; readIndex < performanceEntries.length; readIndex += 1) {
        const entry = performanceEntries[readIndex];
        if (entry.entryType !== "resource") {
          performanceEntries[writeIndex] = entry;
          writeIndex += 1;
        }
      }
      performanceEntries.length = writeIndex;
      resourceEntryCount = 0;
      resourceTimingBufferFull = false;
    },
  };
  Object.defineProperty(performance, "onresourcetimingbufferfull", {
    value: null,
    writable: true,
    enumerable: true,
    configurable: true,
  });
  Object.defineProperty(performance, "timeOrigin", {
    value: Number(__omoikane_performance_time_origin),
    enumerable: true,
  });
  globalThis.PerformanceEntry = PerformanceEntry;
  globalThis.PerformanceMark = PerformanceMark;
  globalThis.PerformanceMeasure = PerformanceMeasure;
  globalThis.PerformanceResourceTiming = PerformanceResourceTiming;
  globalThis.PerformanceNavigationTiming = PerformanceNavigationTiming;
  globalThis.PerformanceObserver = PerformanceObserver;
  globalThis.PerformanceObserverEntryList = PerformanceObserverEntryList;
  globalThis.performance = performance;
  addPerformanceEntry(initialNavigationEntry);
  navigationEntryForLifecycle = initialNavigationEntry;
  globalThis.__omoikane_performance_navigation_event = function(type) {
    if (!navigationEntryForLifecycle) return;
    const state = navigationTimingValues.get(navigationEntryForLifecycle);
    if (!state) return;
    const now = nativePerformanceNow();
    const update = field => {
      const previous = Number(state[field]) || 0;
      state[field] = Math.max(previous, now);
    };
    const updateDuration = () => {
      const startTime = Number(navigationEntryForLifecycle.startTime) || 0;
      const loadEventEnd = Number(state.loadEventEnd) || 0;
      state.duration = Math.max(0, loadEventEnd - startTime);
    };
    switch (String(type)) {
      case "domInteractive": update("domInteractive"); break;
      case "domContentLoadedStart": update("domContentLoadedEventStart"); break;
      case "domContentLoadedEnd": update("domContentLoadedEventEnd"); break;
      case "domComplete": update("domComplete"); break;
      case "loadStart": update("loadEventStart"); break;
      case "loadEnd": update("loadEventEnd"); updateDuration(); break;
      default: break;
    }
  };
  // Host-side document/module loaders complete outside the JS fetch wrapper.
  // They report their terminal status through this small host bridge so
  // initial parser-discovered resources participate in the same timeline.
  globalThis.__omoikane_record_resource_timing = function(name, initiatorType, status, error, redirected = false, elapsedMs = 0) {
    const responseEnd = nativePerformanceNow();
    const elapsed = normalizeTimingNumber(elapsedMs, 0);
    const startTime = Math.max(0, responseEnd - elapsed);
    const timing = beginResourceTiming(String(name), String(initiatorType || "other"), startTime);
    finishResourceTiming(timing, {
      status: Number(status) || 0,
      redirected: Boolean(redirected),
      responseStart: responseEnd,
      responseEnd,
      url: String(name),
    }, Boolean(error));
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
      const parsedId = __omoikane_parse_xml(input);
      if (parsedId !== null && parsedId !== undefined) {
        const parsed = wrapNode(parsedId);
        parsed.__documentURL = "about:blank";
        return parsed;
      }
      const error = document.implementation.createDocument("", "parsererror", null);
      error.__documentURL = "about:blank";
      error.documentElement.textContent = "XML parse error";
      return error;
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
      this._responseText = "";
      this._responseType = "";
      this.response = null;
      this.responseURL = "";
      this.timeout = 0;
      this.withCredentials = false;
      this.onreadystatechange = null;
      this.onload = null;
      this.onerror = null;
      this.onloadend = null;
      this._headers = {};
      this._responseHeaders = [];
      this._requestId = 0;
      this._sendFlag = false;
      this.__resourceTiming = null;
      Object.defineProperty(this, "responseText", {
        enumerable: true,
        configurable: true,
        get() {
          if (this._responseType !== "" && this._responseType !== "text") {
            throw new DOMException("responseText is unavailable for this responseType", "InvalidStateError");
          }
          return this._responseText;
        },
        // Keep legacy embedders that assign this diagnostic property from
        // throwing, while all network state changes use the internal slot.
        set(value) { this._responseText = String(value); },
      });
      Object.defineProperty(this, "responseType", {
        enumerable: true,
        configurable: true,
        get() { return this._responseType; },
        set(value) {
          if (this._sendFlag || this.readyState >= XMLHttpRequest.HEADERS_RECEIVED) {
            throw new DOMException("responseType cannot change after loading starts", "InvalidStateError");
          }
          const normalized = String(value);
          if (!["", "text", "arraybuffer", "blob", "document", "json"].includes(normalized)) {
            throw new DOMException("Unsupported responseType", "SyntaxError");
          }
          this._responseType = normalized;
        },
      });
    }
    open(method, url, async = true) {
      // Re-opening an active request aborts the old fetch.  Preserve the
      // Resource Timing entry even though no terminal network payload arrives.
      if (this.__resourceTiming) {
        finishResourceTiming(this.__resourceTiming, {}, true);
        this.__resourceTiming = null;
      }
      this._requestId++;
      this.status = 0;
      this.statusText = "";
      this._responseText = "";
      this.response = null;
      this.responseURL = "";
      this._headers = {};
      this._responseHeaders = [];
      this._sendFlag = false;
      this._method = String(method).toUpperCase();
      this._url = isBlobUrl(url) ? String(url) : resolveNetworkUrl(url);
      this._async = async !== false;
      this.readyState = 1;
      this._notify("readystatechange");
    }
    setRequestHeader(name, value) {
      if (this.readyState !== 1 || this._sendFlag) throw new Error("InvalidStateError");
      const key = String(name).toLowerCase();
      const text = String(value).trim();
      this._headers[key] = key in this._headers ? this._headers[key] + ", " + text : text;
    }
    getAllResponseHeaders() {
      if (this.readyState < 2) return "";
      return this._responseHeaders
        .filter(([name]) => {
          const key = String(name).toLowerCase();
          return key !== "set-cookie" && key !== "set-cookie2";
        })
        .map(([name, value]) => String(name).toLowerCase() + ": " + value + "\r\n")
        .join("");
    }
    getResponseHeader(name) {
      if (this.readyState < 2) return null;
      const key = String(name).toLowerCase();
      if (key === "set-cookie" || key === "set-cookie2") return null;
      const values = this._responseHeaders
        .filter(([headerName]) => String(headerName).toLowerCase() === key)
        .map(([, value]) => String(value));
      return values.length ? values.join(", ") : null;
    }
    addEventListener(type, callback) {
      (this._listeners[type] ||= []).push(callback);
    }
    removeEventListener(type, callback) {
      this._listeners[type] = (this._listeners[type] || []).filter(item => item !== callback);
    }
    abort() {
      if (this.__resourceTiming) {
        finishResourceTiming(this.__resourceTiming, {}, true);
        this.__resourceTiming = null;
      }
      this._requestId++;
      this.readyState = 0;
      this.status = 0;
      this.statusText = "";
      this._responseText = "";
      this.response = null;
      this.responseURL = "";
      this._responseHeaders = [];
      this._sendFlag = false;
      this._notify("abort");
      this._notify("loadend");
    }
    send(body = null) {
      if (this.readyState !== 1 || this._sendFlag) throw new Error("InvalidStateError");
      this._sendFlag = true;
      const requestId = this._requestId;
      this.__resourceTiming = beginResourceTiming(this._url, "xmlhttprequest");
      const requestBody = this._method === "GET" || this._method === "HEAD"
        ? EMPTY_BODY
        : extractBody(body);
      if (requestBody.contentType !== null && !("content-type" in this._headers)) {
        this._headers["content-type"] = requestBody.contentType;
      }
      // A blob URL resolves from the object URL store; anything else goes to the
      // host fetch binding. Both settle to the same payload shape.
      const payload = isBlobUrl(this._url)
        ? Promise.resolve().then(() => {
            const blob = this._method === "GET" ? objectUrls.get(this._url) : undefined;
            if (blob === undefined) throw new TypeError("Failed to fetch blob URL");
            return {
              status: 200,
              statusText: "OK",
              url: this._url,
              redirected: false,
              type: "basic",
              headers: [["content-type", blob.type], ["content-length", String(blob.size)]],
              bodyText: blob.__text(),
              bodyBase64: ["arraybuffer", "blob"].includes(this._responseType)
                ? base64FromBytes(blob.__bytes)
                : null,
              bodyPresent: true,
            };
          })
        : Promise.resolve().then(() =>
            __omoikane_fetch(
              this._url,
              this._method,
              JSON.stringify(Object.entries(this._headers)),
              bodyAsPayload(requestBody),
              "cors",
              this.withCredentials ? "include" : "same-origin",
              "follow",
            )
          ).then(raw => JSON.parse(String(raw)));
      payload.then(data => {
        if (requestId !== this._requestId) return;
        finishResourceTiming(this.__resourceTiming, data, false);
        this.__resourceTiming = null;
        this.status = data.status;
        this.statusText = data.statusText;
        this.responseURL = data.url;
        this._responseHeaders = data.headers;
        this.readyState = 2;
        this._notify("readystatechange");
        this.readyState = 3;
        this._notify("readystatechange");
        this._responseText = data.bodyText === undefined ? "" : String(data.bodyText);
        const bytes = xhrResponseBytes(data);
        switch (this._responseType) {
          case "arraybuffer":
            this.response = bytes.slice().buffer;
            break;
          case "blob":
            this.response = new Blob([bytes], { type: xhrResponseMime(data.headers) });
            break;
          case "json":
            try { this.response = JSON.parse(this._responseText); }
            catch (_) { this.response = null; }
            break;
          case "document": {
            const contentType = xhrResponseMime(data.headers);
            const mime = contentType.includes("xml") ? "text/xml" : "text/html";
            try { this.response = new DOMParser().parseFromString(this._responseText, mime); }
            catch (_) { this.response = null; }
            break;
          }
          default:
            this.response = this._responseText;
            break;
        }
        this.readyState = 4;
        this._sendFlag = false;
        this._notify("readystatechange");
        this._notify("load");
        this._notify("loadend");
      }).catch(() => {
        if (requestId !== this._requestId) return;
        finishResourceTiming(this.__resourceTiming, {}, true);
        this.__resourceTiming = null;
        this.status = 0;
        this.statusText = "";
        this._responseText = "";
        this.response = null;
        this.responseURL = "";
        this._responseHeaders = [];
        this.readyState = 4;
        this._sendFlag = false;
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

  function xhrResponseBytes(data) {
    if (!data || data.bodyPresent === false) return new Uint8Array();
    if (data.bodyBase64 !== undefined && data.bodyBase64 !== null) {
      return bytesFromBase64(data.bodyBase64);
    }
    return blobTextEncoder.encode(data.bodyText === undefined ? "" : String(data.bodyText));
  }

  function xhrResponseMime(headers) {
    const entry = (headers || []).find(([name]) => String(name).toLowerCase() === "content-type");
    return entry ? String(entry[1]).split(";", 1)[0].trim().toLowerCase() : "";
  }

  globalThis.XMLHttpRequest.UNSENT = 0;
  globalThis.XMLHttpRequest.OPENED = 1;
  globalThis.XMLHttpRequest.HEADERS_RECEIVED = 2;
  globalThis.XMLHttpRequest.LOADING = 3;
  globalThis.XMLHttpRequest.DONE = 4;

  class ReadableStreamDefaultController {
    constructor(stream) { this._stream = stream; }
    enqueue(chunk) {
      const stream = this._stream;
      if (stream._closed || stream._errorSet) throw new TypeError("ReadableStream is closed");
      const waiter = stream._waiters.shift();
      if (waiter) waiter.resolve({ value: chunk, done: false });
      else stream._queue.push(chunk);
    }
    close() {
      const stream = this._stream;
      if (stream._closed || stream._errorSet) return;
      stream._closed = true;
      if (stream._closedResolve) {
        stream._closedResolve();
        stream._closedResolve = null;
        stream._closedReject = null;
      }
      for (const waiter of stream._waiters.splice(0)) {
        waiter.resolve({ value: undefined, done: true });
      }
    }
    error(reason) {
      const stream = this._stream;
      if (stream._closed || stream._errorSet) return;
      stream._error = reason;
      stream._errorSet = true;
      stream._closed = true;
      stream._queue.length = 0;
      if (stream._closedReject) {
        stream._closedReject(reason);
        stream._closedResolve = null;
        stream._closedReject = null;
      }
      for (const waiter of stream._waiters.splice(0)) waiter.reject(reason);
    }
    get desiredSize() { return this._stream._closed || this._stream._errorSet ? 0 : 1; }
  }
  class ReadableStreamDefaultReader {
    constructor(stream) {
      if (!(stream instanceof ReadableStream) || stream.locked) throw new TypeError("Invalid or locked stream");
      this._stream = stream;
      stream._reader = this;
      this.closed = stream._closed
        ? (stream._errorSet ? Promise.reject(stream._error) : Promise.resolve())
        : new Promise((resolve, reject) => {
            stream._closedResolve = resolve;
            stream._closedReject = reject;
          });
    }
    read() {
      const stream = this._stream;
      if (!stream) return Promise.reject(new TypeError("Reader has no stream"));
      stream._markDisturbed();
      if (stream._queue.length) return Promise.resolve({ value: stream._queue.shift(), done: false });
      if (stream._errorSet) return Promise.reject(stream._error);
      if (stream._closed) return Promise.resolve({ value: undefined, done: true });
      return new Promise((resolve, reject) => stream._waiters.push({ resolve, reject }));
    }
    cancel(reason) { return this._stream ? this._stream._cancel(reason) : Promise.reject(new TypeError("Reader has no stream")); }
    releaseLock() {
      const stream = this._stream;
      if (!stream) return;
      if (stream._reader === this) {
        stream._reader = null;
        const reject = stream._closedReject;
        stream._closedResolve = null;
        stream._closedReject = null;
        if (reject && !stream._closed && !stream._errorSet) {
          reject(new TypeError("Reader lock was released"));
        }
      }
      this._stream = null;
    }
  }
  class ReadableStream {
    constructor(underlyingSource = {}) {
      this._queue = []; this._waiters = []; this._reader = null;
      this._closed = false; this._error = undefined; this._errorSet = false;
      this._source = underlyingSource || {};
      this._closedResolve = null; this._closedReject = null;
      this._disturbed = false; this._onDisturb = null; this._cancelled = false;
      this._controller = new ReadableStreamDefaultController(this);
      if (typeof this._source.start === "function") {
        try {
          Promise.resolve(this._source.start(this._controller)).catch(e => this._controller.error(e));
        } catch (error) {
          this._controller.error(error);
        }
      }
    }
    get locked() { return this._reader !== null; }
    get [Symbol.toStringTag]() { return "ReadableStream"; }
    _markDisturbed() {
      if (this._disturbed) return;
      this._disturbed = true;
      if (typeof this._onDisturb === "function") this._onDisturb();
    }
    getReader() { return new ReadableStreamDefaultReader(this); }
    cancel(reason) {
      if (this.locked) return Promise.reject(new TypeError("ReadableStream is locked"));
      return this._cancel(reason);
    }
    _cancel(reason) {
      if (this._cancelled) return Promise.resolve();
      this._markDisturbed();
      this._cancelled = true;
      this._queue.length = 0;
      if (!this._closed && !this._errorSet) this._controller.close();
      try {
        return Promise.resolve(typeof this._source.cancel === "function" ? this._source.cancel(reason) : undefined);
      } catch (error) {
        return Promise.reject(error);
      }
    }
    pipeTo(destination) {
      const reader = this.getReader(); const writer = destination.getWriter();
      const pump = () => reader.read().then(result => result.done
        ? writer.close()
        : Promise.resolve(writer.write(result.value)).then(pump));
      return pump().catch(error => Promise.resolve(writer.abort(error)).then(() => { throw error; }))
        .finally(() => reader.releaseLock());
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

  // Body streams are byte streams in the Fetch/File APIs.  The current host
  // keeps response bytes in memory, so one immutable chunk is sufficient while
  // preserving the observable ReadableStream lifecycle (locked, disturbed,
  // cancel and close) for consumers.
  function readableByteStream(bytes, onDisturb = null) {
    const snapshot = bytes instanceof Uint8Array ? bytes.slice() : new Uint8Array(bytes || []);
    const stream = new ReadableStream({
      start(controller) {
        if (snapshot.length) controller.enqueue(snapshot);
        controller.close();
      },
    });
    stream._onDisturb = onDisturb;
    return stream;
  }

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
        __omoikane_call_event_listener(
          entry.callback,
          typeof entry.callback === "function" ? this : entry.callback,
          event
        );
        if (event.__stoppedImmediate) break;
      }
      event.currentTarget = null;
      return !event.defaultPrevented;
    }
  }

  // Notifications are modeled as a deterministic, task-queued lifecycle.
  // Omoikane does not own an OS notification backend, so permission and
  // show/click/close state remain observable in the realm without opening a
  // platform window.  The engine hook below lets conformance tests inject a
  // granted/denied decision without making page code responsible for a UI
  // prompt.
  const nativeNotificationPermission = globalThis.__omoikane_notification_permission;
  const nativeNotificationRequestPermission = globalThis.__omoikane_notification_request_permission;
  try { delete globalThis.__omoikane_notification_permission; } catch (_) {}
  try { delete globalThis.__omoikane_notification_request_permission; } catch (_) {}
  const notificationPermissionValues = new Set(["default", "granted", "denied"]);
  const normalizeNotificationPermission = value => {
    const normalized = String(value);
    return notificationPermissionValues.has(normalized) ? normalized : "default";
  };
  const currentNotificationPermission = () => {
    try {
      return typeof nativeNotificationPermission === "function"
        ? normalizeNotificationPermission(nativeNotificationPermission()) : "default";
    } catch (_) {
      return "default";
    }
  };
  const requestNotificationPermission = () => {
    const before = currentNotificationPermission();
    let result;
    try {
      result = typeof nativeNotificationRequestPermission === "function"
        ? normalizeNotificationPermission(nativeNotificationRequestPermission())
        : currentNotificationPermission();
    } catch (_) {
      result = currentNotificationPermission();
    }
    if (result !== before && typeof globalThis.__omoikane_permission_changed === "function") {
      globalThis.__omoikane_permission_changed("notifications", result === "default" ? "prompt" : result);
    }
    return result;
  };
  const closedNotifications = new WeakSet();
  const notificationTask = callback => {
    if (typeof __omoikane_queue_dom_manipulation_task === "function") {
      __omoikane_queue_dom_manipulation_task(callback);
    } else {
      setTimeout(callback, 0);
    }
  };
  // Web Audio is represented as a deterministic graph/state model.  The
  // runtime has no platform audio sink, but keeping context time, node
  // connections, AudioParam automation, and oscillator lifecycle observable
  // lets applications exercise the API without producing host audio.
  const audioConstructionToken = {};
  const nativeAudioEventLoopNow = globalThis.__omoikane_event_loop_now;
  try { delete globalThis.__omoikane_event_loop_now; } catch (_) {}
  const audioTask = callback => {
    if (typeof __omoikane_queue_dom_manipulation_task === "function") {
      __omoikane_queue_dom_manipulation_task(callback);
    } else {
      setTimeout(callback, 0);
    }
  };
  const audioClockNow = () => typeof nativeAudioEventLoopNow === "function"
    ? Number(nativeAudioEventLoopNow()) : 0;
  const audioNow = context => {
    if (context.state !== "running") return context.__currentTime;
    const now = audioClockNow();
    return context.__currentTime + Math.max(0, now - context.__runningAt) / 1000;
  };
  function audioNumber(value, name) {
    const number = Number(value);
    if (!Number.isFinite(number)) throw new TypeError(name + " must be finite");
    return number;
  }
  function audioTime(value, name) {
    const time = audioNumber(value, name);
    if (time < 0) throw new RangeError(name + " must be non-negative");
    return time;
  }
  function audioContextValue(context) {
    if (!(context instanceof AudioContext)) throw new TypeError("An AudioContext is required");
    return context;
  }

  class AudioParam {
    constructor(token, context, defaultValue, minValue, maxValue) {
      if (token !== audioConstructionToken) throw new TypeError("Illegal constructor");
      this.__context = context;
      Object.defineProperty(this, "defaultValue", {
        configurable: false,
        enumerable: true,
        writable: false,
        value: Number(defaultValue),
      });
      Object.defineProperty(this, "minValue", {
        configurable: false,
        enumerable: true,
        writable: false,
        value: Number(minValue),
      });
      Object.defineProperty(this, "maxValue", {
        configurable: false,
        enumerable: true,
        writable: false,
        value: Number(maxValue),
      });
      this.__value = this.defaultValue;
      this.__events = [];
      this.automationRate = "a-rate";
    }
    __valueAt(time) {
      let value = this.__value;
      let lastTime = 0;
      for (let index = 0; index < this.__events.length; index++) {
        const event = this.__events[index];
        if (event.time > time) {
          if (event.kind === "linear" || event.kind === "exponential") {
            const progress = Math.min(1, Math.max(0, (time - lastTime) / (event.time - lastTime)));
            return event.kind === "exponential" && value > 0 && event.value > 0
              ? value * Math.pow(event.value / value, progress)
              : value + (event.value - value) * progress;
          }
          return value;
        }
        if (event.kind === "target") {
          const nextTime = index + 1 < this.__events.length ? this.__events[index + 1].time : time;
          const evaluatedTime = Math.min(time, nextTime);
          value = event.value + (value - event.value) * Math.exp(-(evaluatedTime - event.time) / event.timeConstant);
          lastTime = evaluatedTime;
          if (time < nextTime) return value;
        } else {
          value = event.value;
          lastTime = event.time;
        }
      }
      return value;
    }
    get value() { return this.__valueAt(audioNow(this.__context)); }
    set value(next) {
      this.__value = Math.min(this.maxValue, Math.max(this.minValue, audioNumber(next, "AudioParam.value")));
    }
    __schedule(kind, value, time) {
      const numeric = audioNumber(value, "AudioParam value");
      const at = audioTime(time, "AudioParam time");
      const event = { kind, value: numeric, time: at };
      this.__events = this.__events.filter(existing => existing.time !== at);
      this.__events.push(event);
      this.__events.sort((left, right) => left.time - right.time);
      return this;
    }
    setValueAtTime(value, startTime) { return this.__schedule("set", value, startTime); }
    linearRampToValueAtTime(value, endTime) { return this.__schedule("linear", value, endTime); }
    exponentialRampToValueAtTime(value, endTime) {
      const numeric = audioNumber(value, "AudioParam value");
      const at = audioTime(endTime, "AudioParam time");
      if (numeric <= 0 || this.__valueAt(at) <= 0) throw new RangeError("Exponential values must be positive.");
      return this.__schedule("exponential", numeric, at);
    }
    setTargetAtTime(target, startTime, timeConstant) {
      const value = audioNumber(target, "AudioParam target");
      const start = audioTime(startTime, "AudioParam time");
      const constant = audioNumber(timeConstant, "AudioParam timeConstant");
      if (constant <= 0) throw new RangeError("AudioParam timeConstant must be positive");
      this.__events = this.__events.filter(existing => existing.time !== start);
      this.__events.push({ kind: "target", value, time: start, timeConstant: constant });
      this.__events.sort((left, right) => left.time - right.time);
      return this;
    }
    cancelScheduledValues(cancelTime) {
      const at = audioTime(cancelTime, "AudioParam time");
      this.__events = this.__events.filter(event => event.time < at);
      return this;
    }
    cancelAndHoldAtTime(cancelTime) {
      const at = audioTime(cancelTime, "AudioParam time");
      const held = this.__valueAt(at);
      this.cancelScheduledValues(at);
      this.__events.push({ kind: "set", value: held, time: at });
      this.__events.sort((left, right) => left.time - right.time);
      return this;
    }
    get [Symbol.toStringTag]() { return "AudioParam"; }
  }

  class AudioNode extends EventTarget {
    constructor(token, context, inputs = 1, outputs = 1) {
      super();
      if (token !== audioConstructionToken) throw new TypeError("Illegal constructor");
      Object.defineProperty(this, "context", {
        configurable: false,
        enumerable: true,
        writable: false,
        value: context,
      });
      this.__context = context;
      Object.defineProperty(this, "numberOfInputs", {
        configurable: false,
        enumerable: true,
        writable: false,
        value: inputs,
      });
      Object.defineProperty(this, "numberOfOutputs", {
        configurable: false,
        enumerable: true,
        writable: false,
        value: outputs,
      });
      this.channelCount = 2;
      this.channelCountMode = "max";
      this.channelInterpretation = "speakers";
      this.__connections = [];
      if (context && context.__nodes) context.__nodes.add(this);
    }
    connect(destination, output = 0, input = 0) {
      if (!(destination instanceof AudioNode) && !(destination instanceof AudioParam)) {
        throw new TypeError("AudioNode.connect destination must be an AudioNode or AudioParam");
      }
      if (destination.__context !== this.__context) {
        throw new DOMException("Nodes belong to different AudioContexts.", "InvalidAccessError");
      }
      const outNumber = Number(output);
      const inputNumber = Number(input);
      const out = Math.trunc(outNumber);
      const inputIndex = Math.trunc(inputNumber);
      const destinationInputCount = destination instanceof AudioNode ? destination.numberOfInputs : 1;
      if (!Number.isFinite(outNumber) || !Number.isFinite(inputNumber) || out < 0 || out >= this.numberOfOutputs || inputIndex < 0 || inputIndex >= destinationInputCount) {
        throw new DOMException("The AudioNode output or input is invalid.", "IndexSizeError");
      }
      this.__connections.push({ destination, output: out, input: inputIndex });
      return destination instanceof AudioParam ? undefined : destination;
    }
    disconnect(destination = undefined, output = undefined, input = undefined) {
      if (destination === undefined) {
        this.__connections = [];
        return;
      }
      if (typeof destination === "number" && output === undefined && input === undefined) {
        const outputNumber = Number(destination);
        const outputIndex = Math.trunc(outputNumber);
        if (!Number.isFinite(outputNumber) || outputIndex < 0 || outputIndex >= this.numberOfOutputs) {
          throw new DOMException("The AudioNode output is invalid.", "IndexSizeError");
        }
        const before = this.__connections.length;
        this.__connections = this.__connections.filter(connection => connection.output !== outputIndex);
        if (this.__connections.length === before) {
          throw new DOMException("The specified connection was not found.", "InvalidAccessError");
        }
        return;
      }
      if (!(destination instanceof AudioNode) && !(destination instanceof AudioParam)) {
        throw new TypeError("AudioNode.disconnect destination must be an AudioNode or AudioParam");
      }
      if (destination.__context !== this.__context) {
        throw new DOMException("Nodes belong to different AudioContexts.", "InvalidAccessError");
      }
      const hasOutput = output !== undefined;
      const hasInput = input !== undefined;
      let outputIndex;
      let inputIndex;
      if (hasOutput) {
        const outputNumber = Number(output);
        outputIndex = Math.trunc(outputNumber);
        if (!Number.isFinite(outputNumber) || outputIndex < 0 || outputIndex >= this.numberOfOutputs) {
          throw new DOMException("The AudioNode output is invalid.", "IndexSizeError");
        }
      }
      if (hasInput) {
        const inputNumber = Number(input);
        inputIndex = Math.trunc(inputNumber);
        const destinationInputCount = destination instanceof AudioNode ? destination.numberOfInputs : 1;
        if (!Number.isFinite(inputNumber) || inputIndex < 0 || inputIndex >= destinationInputCount) {
          throw new DOMException("The AudioNode input is invalid.", "IndexSizeError");
        }
      }
      const before = this.__connections.length;
      this.__connections = this.__connections.filter(connection => (
        connection.destination !== destination ||
        (hasOutput && connection.output !== outputIndex) ||
        (hasInput && connection.input !== inputIndex)
      ));
      if (this.__connections.length === before) {
        throw new DOMException("The specified connection was not found.", "InvalidAccessError");
      }
    }
    get [Symbol.toStringTag]() { return "AudioNode"; }
  }

  class AudioDestinationNode extends AudioNode {
    constructor(token, context) {
      super(token, context, 1, 0);
      this.maxChannelCount = 2;
    }
    get [Symbol.toStringTag]() { return "AudioDestinationNode"; }
  }

  class GainNode extends AudioNode {
    constructor(context, options = {}) {
      const owner = audioContextValue(context);
      const init = options === null || options === undefined ? {} : options;
      if (typeof init !== "object" && typeof init !== "function") throw new TypeError("GainNode options must be a dictionary");
      super(audioConstructionToken, owner, 1, 1);
      Object.defineProperty(this, "gain", {
        configurable: false,
        enumerable: true,
        writable: false,
        value: new AudioParam(audioConstructionToken, owner, init.gain ?? 1, -3.402823466e38, 3.402823466e38),
      });
    }
    get [Symbol.toStringTag]() { return "GainNode"; }
  }

  class OscillatorNode extends AudioNode {
    constructor(context, options = {}) {
      const owner = audioContextValue(context);
      const init = options === null || options === undefined ? {} : options;
      if (typeof init !== "object" && typeof init !== "function") throw new TypeError("OscillatorNode options must be a dictionary");
      super(audioConstructionToken, owner, 0, 1);
      this.type = init.type === undefined ? "sine" : String(init.type);
      if (!["sine", "square", "sawtooth", "triangle", "custom"].includes(this.type)) {
        throw new TypeError("Unsupported oscillator type");
      }
      Object.defineProperty(this, "frequency", {
        configurable: false,
        enumerable: true,
        writable: false,
        value: new AudioParam(audioConstructionToken, owner, init.frequency ?? 440, -3.402823466e38, 3.402823466e38),
      });
      Object.defineProperty(this, "detune", {
        configurable: false,
        enumerable: true,
        writable: false,
        value: new AudioParam(audioConstructionToken, owner, init.detune ?? 0, -3.402823466e38, 3.402823466e38),
      });
      this.onended = null;
      this.__started = false;
      this.__stopped = false;
      this.__stopCalled = false;
      this.__startRequested = 0;
      this.__stopTimer = null;
    }
    start(when = 0) {
      if (this.__started) throw new DOMException("OscillatorNode.start() was already called.", "InvalidStateError");
      const startAt = audioNumber(when, "OscillatorNode start time");
      if (startAt < 0) throw new RangeError("OscillatorNode start time must be non-negative");
      this.__startRequested = startAt;
      this.__started = true;
      this.__schedulePendingStop();
    }
    stop(when = 0) {
      if (this.__stopCalled) throw new DOMException("OscillatorNode.stop() was already called.", "InvalidStateError");
      const stopAt = audioNumber(when, "OscillatorNode stop time");
      if (stopAt < 0) throw new RangeError("OscillatorNode stop time must be non-negative");
      this.__stopCalled = true;
      this.__stopRequested = stopAt;
      this.__schedulePendingStop();
    }
    __pauseScheduledStop() {
      if (this.__stopTimer !== null) clearTimeout(this.__stopTimer);
      this.__stopTimer = null;
    }
    __schedulePendingStop() {
      if (this.__stopped || !this.__started || this.__stopRequested === undefined || this.__context.state !== "running") return;
      if (this.__stopTimer !== null) clearTimeout(this.__stopTimer);
      const effectiveStop = Math.max(this.__stopRequested, this.__startRequested);
      const delay = Math.max(0, (effectiveStop - audioNow(this.__context)) * 1000);
      if (delay <= 0) {
        this.__finish();
        return;
      }
      this.__stopTimer = setTimeout(() => {
        this.__stopTimer = null;
        if (this.__context.state === "running") this.__finish();
      }, delay);
    }
    __finish() {
      if (this.__stopped) return;
      this.__stopped = true;
      this.__stopTimer = null;
      audioTask(() => {
        const event = new Event("ended");
        if (typeof this.onended === "function") this.onended.call(this, event);
        this.dispatchEvent(event);
      });
    }
    get [Symbol.toStringTag]() { return "OscillatorNode"; }
  }

  class AudioContext extends EventTarget {
    constructor(options = {}) {
      super();
      const init = options === null || options === undefined ? {} : Object(options);
      const sampleRate = init.sampleRate === undefined ? 44100 : audioNumber(init.sampleRate, "AudioContext sampleRate");
      if (sampleRate <= 0) throw new DOMException("The sampleRate must be positive.", "NotSupportedError");
      Object.defineProperty(this, "sampleRate", {
        configurable: false,
        enumerable: true,
        writable: false,
        value: sampleRate,
      });
      Object.defineProperty(this, "baseLatency", {
        configurable: false,
        enumerable: true,
        writable: false,
        value: 0,
      });
      Object.defineProperty(this, "outputLatency", {
        configurable: false,
        enumerable: true,
        writable: false,
        value: 0,
      });
      this.latencyHint = init.latencyHint === undefined ? "interactive" : init.latencyHint;
      this.__state = "suspended";
      this.__currentTime = 0;
      this.__runningAt = 0;
      this.__nodes = new Set();
      this.__destination = new AudioDestinationNode(audioConstructionToken, this);
      this.listener = Object.create(null);
      this.onstatechange = null;
    }
    get state() { return this.__state; }
    get currentTime() { return audioNow(this); }
    get destination() { return this.__destination; }
    resume() {
      if (this.__state === "closed") return Promise.reject(new DOMException("The AudioContext is closed.", "InvalidStateError"));
      if (this.__state === "running") return Promise.resolve();
      this.__runningAt = audioClockNow();
      this.__state = "running";
      for (const node of this.__nodes) if (typeof node.__schedulePendingStop === "function") node.__schedulePendingStop();
      const context = this;
      return new Promise(resolve => audioTask(() => {
        const event = new Event("statechange");
        try {
          const handler = context.onstatechange;
          if (typeof handler === "function") handler.call(context, event);
          context.dispatchEvent(event);
        } finally {
          resolve();
        }
      }));
    }
    suspend() {
      if (this.__state === "closed") return Promise.reject(new DOMException("The AudioContext is closed.", "InvalidStateError"));
      if (this.__state === "suspended") return Promise.resolve();
      this.__currentTime = this.currentTime;
      this.__state = "suspended";
      for (const node of this.__nodes) if (typeof node.__pauseScheduledStop === "function") node.__pauseScheduledStop();
      const context = this;
      return new Promise(resolve => audioTask(() => {
        const event = new Event("statechange");
        try {
          const handler = context.onstatechange;
          if (typeof handler === "function") handler.call(context, event);
          context.dispatchEvent(event);
        } finally {
          resolve();
        }
      }));
    }
    close() {
      if (this.__state === "closed") return Promise.resolve();
      this.__currentTime = this.currentTime;
      this.__state = "closed";
      for (const node of this.__nodes) if (typeof node.__pauseScheduledStop === "function") node.__pauseScheduledStop();
      const context = this;
      return new Promise(resolve => audioTask(() => {
        const event = new Event("statechange");
        try {
          const handler = context.onstatechange;
          if (typeof handler === "function") handler.call(context, event);
          context.dispatchEvent(event);
        } finally {
          resolve();
        }
      }));
    }
    createGain() { if (this.__state === "closed") throw new DOMException("The AudioContext is closed.", "InvalidStateError"); return new GainNode(this); }
    createOscillator() { if (this.__state === "closed") throw new DOMException("The AudioContext is closed.", "InvalidStateError"); return new OscillatorNode(this); }
    get [Symbol.toStringTag]() { return "AudioContext"; }
  }
  globalThis.AudioParam = AudioParam;
  globalThis.AudioNode = AudioNode;
  globalThis.AudioDestinationNode = AudioDestinationNode;
  globalThis.GainNode = GainNode;
  globalThis.OscillatorNode = OscillatorNode;
  globalThis.AudioContext = AudioContext;
  function notificationOptionString(options, name, fallback = "") {
    const value = options && options[name];
    return value === undefined ? fallback : String(value);
  }
  function notificationOptionBoolean(options, name, fallback = false) {
    return options && options[name] === undefined ? fallback : !!(options && options[name]);
  }
  class Notification extends EventTarget {
    constructor(title, options = {}) {
      super();
      if (arguments.length < 1) throw new TypeError("Notification requires a title");
      if (!nativeIsSecureContext()) {
        throw new DOMException("Notifications require a secure context.", "NotAllowedError");
      }
      if (currentNotificationPermission() !== "granted") {
        throw new DOMException("Notification permission is not granted.", "NotAllowedError");
      }
      const init = options === null || options === undefined ? {} : Object(options);
      const direction = notificationOptionString(init, "dir", "auto").toLowerCase();
      const data = init.data === undefined ? null : globalThis.structuredClone(init.data);
      const actions = Array.isArray(init.actions)
        ? init.actions.map(action => ({
            action: notificationOptionString(action, "action"),
            title: notificationOptionString(action, "title"),
            icon: notificationOptionString(action, "icon"),
          }))
        : [];
      const values = {
        title: String(title),
        dir: ["auto", "ltr", "rtl"].includes(direction) ? direction : "auto",
        lang: notificationOptionString(init, "lang"),
        body: notificationOptionString(init, "body"),
        tag: notificationOptionString(init, "tag"),
        image: notificationOptionString(init, "image"),
        icon: notificationOptionString(init, "icon"),
        badge: notificationOptionString(init, "badge"),
        vibrate: init.vibrate === undefined ? [] : (Array.isArray(init.vibrate) ? init.vibrate.slice() : [init.vibrate]),
        timestamp: init.timestamp === undefined ? Date.now() : Number(init.timestamp),
        renotify: notificationOptionBoolean(init, "renotify"),
        silent: notificationOptionBoolean(init, "silent"),
        requireInteraction: notificationOptionBoolean(init, "requireInteraction"),
        data,
        actions,
      };
      for (const [name, value] of Object.entries(values)) {
        Object.defineProperty(this, name, {
          configurable: false,
          enumerable: true,
          writable: false,
          value,
        });
      }
      for (const type of ["show", "click", "error", "close"]) {
        Object.defineProperty(this, "on" + type, {
          configurable: true,
          enumerable: false,
          get: () => this["__on" + type] || null,
          set: callback => { this["__on" + type] = typeof callback === "function" ? callback : null; },
        });
      }
      notificationTask(() => {
        if (!closedNotifications.has(this)) fireRealtimeEvent(this, new Event("show"));
      });
    }
    close() {
      if (closedNotifications.has(this)) return;
      closedNotifications.add(this);
      notificationTask(() => fireRealtimeEvent(this, new Event("close")));
    }
    get [Symbol.toStringTag]() { return "Notification"; }
    static get permission() { return currentNotificationPermission(); }
    static requestPermission(callback) {
      const result = Promise.resolve().then(() => {
        if (!nativeIsSecureContext()) {
          throw new DOMException("Notifications require a secure context.", "NotAllowedError");
        }
        // There is no native prompt in this headless engine. A default
        // decision therefore follows the browser's non-granting fallback.
        return requestNotificationPermission();
      });
      if (typeof callback === "function") result.then(callback);
      return result;
    }
  }
  globalThis.Notification = Notification;
  globalThis.__omoikane_dispatch_notification_click = notification => {
    if (!(notification instanceof Notification) || closedNotifications.has(notification)) return false;
    notificationTask(() => {
      if (!closedNotifications.has(notification)) fireRealtimeEvent(notification, new Event("click"));
    });
    return true;
  };

  // -------------------------------------------------------------------------
  // Permissions API query/lifecycle core.
  //
  // The host has deterministic permission state for the APIs that already
  // exist in this runtime (Notifications, Geolocation, and Async Clipboard).
  // Keep PermissionStatus objects in this realm and fan out host transitions
  // through a small weak registry, so no Boa object crosses a runtime boundary
  // and stale statuses do not keep a document alive after teardown.
  // -------------------------------------------------------------------------
  const permissionsConstructionToken = {};
  const permissionStatusEntries = new Map();
  const permissionStatusUsesWeakRefs = typeof WeakRef === "function";
  let permissionLifecycleActive = true;
  const supportedPermissionNames = Object.freeze([
    "notifications", "geolocation", "clipboard-read", "clipboard-write",
  ]);

  function permissionDescriptorName(descriptor) {
    if (descriptor === null || descriptor === undefined) {
      throw new TypeError("Permissions.query requires a permission descriptor");
    }
    const value = Object(descriptor);
    const name = String(value.name);
    if (!supportedPermissionNames.includes(name)) {
      throw new DOMException(`The permission descriptor '${name}' is not supported.`, "NotSupportedError");
    }
    return name;
  }

  function permissionStateFor(name) {
    if (name === "notifications") {
      const permission = currentNotificationPermission();
      return permission === "default" ? "prompt" : permission;
    }
    if (name === "geolocation") {
      try {
        return nativeGeolocationPermission() ? "granted" : "denied";
      } catch (_) {
        return "denied";
      }
    }
    try {
      return nativeIsSecureContext() && nativeClipboardPermission() ? "granted" : "denied";
    } catch (_) {
      return "denied";
    }
  }

  function registerPermissionStatus(status) {
    const entries = permissionStatusEntries.get(status.__permissionName) || [];
    entries.push(permissionStatusUsesWeakRefs ? new WeakRef(status) : status);
    permissionStatusEntries.set(status.__permissionName, entries);
  }

  function notifyPermissionStatuses(name, state) {
    if (!permissionLifecycleActive || !supportedPermissionNames.includes(name)) return;
    const entries = permissionStatusEntries.get(name) || [];
    const retained = [];
    for (const entry of entries) {
      const status = permissionStatusUsesWeakRefs ? entry.deref() : entry;
      if (!status) continue;
      if (status.__active) status.__setState(state);
      if (status.__active) retained.push(entry);
    }
    permissionStatusEntries.set(name, retained);
  }

  class PermissionStatus extends EventTarget {
    constructor(name, state) {
      super();
      if (!supportedPermissionNames.includes(name)) {
        throw new TypeError("Illegal constructor");
      }
      this.__permissionName = name;
      this.__state = state;
      this.__active = true;
      this.__onchange = null;
    }
    get state() { return this.__state; }
    get onchange() { return this.__onchange; }
    set onchange(callback) {
      this.__onchange = typeof callback === "function" ? callback : null;
    }
    __setState(state) {
      if (!this.__active || state === this.__state) return;
      this.__state = state;
      queueMicrotask(() => {
        if (this.__active) fireRealtimeEvent(this, new Event("change"));
      });
    }
    __teardown() {
      this.__active = false;
      this.__onchange = null;
      this._listeners.clear();
    }
    get [Symbol.toStringTag]() { return "PermissionStatus"; }
  }

  class Permissions {
    constructor(token) {
      if (token !== permissionsConstructionToken) throw new TypeError("Illegal constructor");
    }
    query(descriptor) {
      // Dictionary conversion and descriptor validation happen in the
      // promise job, preserving the asynchronous ordering of the platform
      // API and making getter exceptions observable as rejections.
      return Promise.resolve().then(() => {
        const name = permissionDescriptorName(descriptor);
        const status = new PermissionStatus(name, permissionStateFor(name));
        registerPermissionStatus(status);
        return status;
      });
    }
    get [Symbol.toStringTag]() { return "Permissions"; }
  }

  globalThis.PermissionStatus = PermissionStatus;
  globalThis.Permissions = Permissions;
  navigator.permissions = new Permissions(permissionsConstructionToken);
  globalThis.__omoikane_permission_changed = (name, _state) => {
    const normalizedName = String(name);
    if (!supportedPermissionNames.includes(normalizedName)) return;
    // The host transition argument is only a notification hint.  Always
    // recompute from the authoritative source so page code cannot forge a
    // granted/denied state by calling this private-looking global directly.
    notifyPermissionStatuses(normalizedName, permissionStateFor(normalizedName));
  };
  globalThis.__omoikane_permission_teardown = () => {
    if (!permissionLifecycleActive) return;
    permissionLifecycleActive = false;
    for (const entries of permissionStatusEntries.values()) {
      for (const entry of entries) {
        const status = permissionStatusUsesWeakRefs ? entry.deref() : entry;
        if (status) status.__teardown();
      }
    }
    permissionStatusEntries.clear();
  };

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
  Object.setPrototypeOf(WebGLEventTarget.prototype, EventTarget.prototype);
  Object.setPrototypeOf(ServiceWorkerEventTarget.prototype, EventTarget.prototype);
  Object.setPrototypeOf(GPUEventTarget.prototype, EventTarget.prototype);
  globalThis.AbortSignal = AbortSignal;
  globalThis.AbortController = AbortController;

  function dataCloneError(message = "The value could not be cloned.") {
    return new DOMException(message, "DataCloneError");
  }

  function cloneStructuredValue(value, memory) {
    if (value === null || typeof value === "undefined" ||
        typeof value === "boolean" || typeof value === "number" ||
        typeof value === "string" || typeof value === "bigint") return value;
    if (typeof value === "symbol" || typeof value === "function") throw dataCloneError();
    if (value instanceof Node || value instanceof EventTarget) throw dataCloneError();
    if (memory.has(value)) return memory.get(value);

    if (value instanceof Date) {
      const result = new Date(value.getTime()); memory.set(value, result); return result;
    }
    if (value instanceof RegExp) {
      const result = new RegExp(value.source, value.flags); memory.set(value, result); return result;
    }
    if (value instanceof ArrayBuffer) {
      const result = value.slice(0); memory.set(value, result); return result;
    }
    if (ArrayBuffer.isView(value)) {
      const buffer = cloneStructuredValue(value.buffer, memory);
      const result = value instanceof DataView
        ? new DataView(buffer, value.byteOffset, value.byteLength)
        : new value.constructor(buffer, value.byteOffset, value.length);
      memory.set(value, result); return result;
    }
    if (value instanceof Map) {
      const result = new Map(); memory.set(value, result);
      for (const [key, item] of value) {
        result.set(cloneStructuredValue(key, memory), cloneStructuredValue(item, memory));
      }
      return result;
    }
    if (value instanceof Set) {
      const result = new Set(); memory.set(value, result);
      for (const item of value) result.add(cloneStructuredValue(item, memory));
      return result;
    }
    if (Array.isArray(value)) {
      const result = new Array(value.length); memory.set(value, result);
      for (let index = 0; index < value.length; index++) {
        if (Object.prototype.hasOwnProperty.call(value, index)) {
          result[index] = cloneStructuredValue(value[index], memory);
        }
      }
      return result;
    }
    const prototype = Object.getPrototypeOf(value);
    if (prototype !== Object.prototype && prototype !== null) throw dataCloneError();
    if (Object.getOwnPropertySymbols(value).length) throw dataCloneError();
    const result = Object.create(prototype); memory.set(value, result);
    for (const key of Object.keys(value)) {
      result[key] = cloneStructuredValue(value[key], memory);
    }
    return result;
  }

  globalThis.structuredClone = function(value, options = undefined) {
    if (options != null && options.transfer != null && Array.from(options.transfer).length) {
      throw dataCloneError("Transfer lists are not supported yet.");
    }
    return cloneStructuredValue(value, new Map());
  };

  // -------------------------------------------------------------------------
  // WebRTC deterministic signaling/data-channel core.
  //
  // Omoikane deliberately does not open sockets or run ICE/DTLS.  The model
  // below keeps the Web IDL-facing state machine useful by pairing two
  // RTCPeerConnection objects in the same JS realm.  Offers carry an internal
  // owner marker (and are also indexed by their deterministic SDP), so the
  // normal offer/answer exchange can discover the in-memory peer without
  // exposing a cross-realm object.  `RTCPeerConnection.createPair()` and the
  // `__omoikane_create_webrtc_peer_pair` hook provide an explicit test seam.
  // -------------------------------------------------------------------------
  const webrtcDescriptionOwners = new Map();
  let webrtcConnectionSerial = 1;
  let webrtcDataChannelSerial = 0;
  const WEBRTC_DESCRIPTION_TYPES = new Set(["offer", "answer", "pranswer", "rollback"]);
  const WEBRTC_BUNDLE_POLICIES = new Set(["balanced", "max-compat", "max-bundle"]);
  const WEBRTC_ICE_TRANSPORT_POLICIES = new Set(["all", "relay"]);
  const WEBRTC_RTCP_MUX_POLICIES = new Set(["negotiate", "require"]);

  function webrtcQueue(callback) {
    queueMicrotask(callback);
  }

  function webrtcEventHandler(target, eventType, property, callback) {
    const previous = target[property];
    if (previous) target.removeEventListener(eventType, previous);
    const next = typeof callback === "function" ? callback : null;
    target[property] = next;
    if (next) target.addEventListener(eventType, next);
  }

  function webrtcState(target, property, eventType, value) {
    if (target[property] === value) return false;
    target[property] = value;
    webrtcQueue(() => target.dispatchEvent(new Event(eventType)));
    return true;
  }

  function webrtcInvalidState(message) {
    return new DOMException(message || "The RTCPeerConnection is closed.", "InvalidStateError");
  }

  function webrtcSessionDescription(type, sdp, owner = null) {
    const description = new RTCSessionDescription({ type, sdp });
    if (owner) {
      Object.defineProperty(description, "__owner", {
        configurable: false, enumerable: false, writable: false, value: owner,
      });
      webrtcDescriptionOwners.set(description.sdp, owner);
    }
    return description;
  }

  function webrtcDescriptionOwner(description) {
    return (description && description.__owner) ||
      (description && webrtcDescriptionOwners.get(description.sdp)) || null;
  }

  function webrtcNormalizeSessionDescription(value) {
    if (value instanceof RTCSessionDescription) return value;
    if (value == null || typeof value !== "object") {
      throw new TypeError("An RTCSessionDescriptionInit dictionary is required");
    }
    return new RTCSessionDescription(value);
  }

  class RTCSessionDescription {
    constructor(init = {}) {
      if (init == null || typeof init !== "object") {
        throw new TypeError("RTCSessionDescriptionInit must be an object");
      }
      const type = String(init.type === undefined ? "" : init.type);
      if (!WEBRTC_DESCRIPTION_TYPES.has(type)) {
        throw new TypeError("Invalid RTCSessionDescription type");
      }
      const sdp = init.sdp === undefined || init.sdp === null ? "" : String(init.sdp);
      Object.defineProperty(this, "type", {
        configurable: false, enumerable: true, writable: false, value: type,
      });
      Object.defineProperty(this, "sdp", {
        configurable: false, enumerable: true, writable: false, value: sdp,
      });
    }
    toJSON() { return { type: this.type, sdp: this.sdp }; }
    get [Symbol.toStringTag]() { return "RTCSessionDescription"; }
  }

  class RTCIceCandidate {
    constructor(init = {}) {
      if (typeof init === "string") init = { candidate: init };
      if (init == null || typeof init !== "object") {
        throw new TypeError("RTCIceCandidateInit must be an object");
      }
      const candidate = init.candidate === undefined || init.candidate === null
        ? "" : String(init.candidate);
      const sdpMid = init.sdpMid === undefined || init.sdpMid === null ? null : String(init.sdpMid);
      const sdpMLineIndex = init.sdpMLineIndex === undefined || init.sdpMLineIndex === null
        ? null : Number(init.sdpMLineIndex);
      if (sdpMLineIndex !== null && (!Number.isInteger(sdpMLineIndex) || sdpMLineIndex < 0)) {
        throw new TypeError("sdpMLineIndex must be a non-negative integer or null");
      }
      const usernameFragment = init.usernameFragment === undefined || init.usernameFragment === null
        ? null : String(init.usernameFragment);
      Object.defineProperty(this, "candidate", {
        configurable: false, enumerable: true, writable: false, value: candidate,
      });
      Object.defineProperty(this, "sdpMid", {
        configurable: false, enumerable: true, writable: false, value: sdpMid,
      });
      Object.defineProperty(this, "sdpMLineIndex", {
        configurable: false, enumerable: true, writable: false, value: sdpMLineIndex,
      });
      Object.defineProperty(this, "foundation", {
        configurable: false, enumerable: true, writable: false,
        value: candidate ? "0" : null,
      });
      Object.defineProperty(this, "component", {
        configurable: false, enumerable: true, writable: false,
        value: candidate ? 1 : null,
      });
      Object.defineProperty(this, "priority", {
        configurable: false, enumerable: true, writable: false,
        value: candidate ? 1 : null,
      });
      Object.defineProperty(this, "address", {
        configurable: false, enumerable: true, writable: false,
        value: candidate ? "127.0.0.1" : null,
      });
      Object.defineProperty(this, "protocol", {
        configurable: false, enumerable: true, writable: false,
        value: candidate ? "udp" : null,
      });
      Object.defineProperty(this, "port", {
        configurable: false, enumerable: true, writable: false,
        value: candidate ? 9 : null,
      });
      Object.defineProperty(this, "type", {
        configurable: false, enumerable: true, writable: false,
        value: candidate ? "host" : null,
      });
      Object.defineProperty(this, "tcpType", {
        configurable: false, enumerable: true, writable: false, value: null,
      });
      Object.defineProperty(this, "relatedAddress", {
        configurable: false, enumerable: true, writable: false, value: null,
      });
      Object.defineProperty(this, "relatedPort", {
        configurable: false, enumerable: true, writable: false, value: null,
      });
      Object.defineProperty(this, "usernameFragment", {
        configurable: false, enumerable: true, writable: false, value: usernameFragment,
      });
    }
    toJSON() {
      return {
        candidate: this.candidate,
        sdpMid: this.sdpMid,
        sdpMLineIndex: this.sdpMLineIndex,
        usernameFragment: this.usernameFragment,
      };
    }
    get [Symbol.toStringTag]() { return "RTCIceCandidate"; }
  }

  class RTCPeerConnectionIceEvent extends Event {
    constructor(type, init = {}) {
      super(type, init);
      this.candidate = init.candidate === undefined ? null : init.candidate;
      this.url = init.url === undefined || init.url === null ? null : String(init.url);
    }
    get [Symbol.toStringTag]() { return "RTCPeerConnectionIceEvent"; }
  }

  function webrtcChannelBytes(value) {
    if (typeof value === "string") return value.length;
    if (value instanceof ArrayBuffer) return value.byteLength;
    if (ArrayBuffer.isView(value)) return value.byteLength;
    if (typeof Blob === "function" && value instanceof Blob) return value.size;
    return 0;
  }

  function webrtcChannelPayload(value) {
    if (typeof value === "string") return value;
    if (value instanceof ArrayBuffer) return value.slice(0);
    if (ArrayBuffer.isView(value)) {
      return value.buffer.slice(value.byteOffset, value.byteOffset + value.byteLength);
    }
    if (typeof Blob === "function" && value instanceof Blob) return value;
    throw new TypeError("RTCDataChannel.send requires a string or binary data");
  }

  const webrtcDataChannelConstructionToken = {};
  class RTCDataChannel extends EventTarget {
    constructor(token, owner, label, options, id) {
      if (token !== webrtcDataChannelConstructionToken) throw new TypeError("Illegal constructor");
      super();
      this.__owner = owner;
      this.__peer = null;
      this.__announced = false;
      this.__closedEvent = false;
      this.__onopen = null;
      this.__onmessage = null;
      this.__onclose = null;
      this.__onerror = null;
      this.__onbufferedamountlow = null;
      this.label = String(label);
      this.ordered = options.ordered !== false;
      this.maxPacketLifeTime = options.maxPacketLifeTime == null ? null : Number(options.maxPacketLifeTime);
      this.maxRetransmits = options.maxRetransmits == null ? null : Number(options.maxRetransmits);
      this.protocol = options.protocol === undefined ? "" : String(options.protocol);
      this.negotiated = options.negotiated === true;
      this.id = id;
      this.readyState = "connecting";
      this.bufferedAmount = 0;
      this.bufferedAmountLowThreshold = 0;
      this.__binaryType = "blob";
    }
    get binaryType() { return this.__binaryType; }
    set binaryType(value) {
      const next = String(value);
      if (next !== "blob" && next !== "arraybuffer") throw new TypeError("Invalid binaryType");
      this.__binaryType = next;
    }
    send(data) {
      if (this.readyState !== "open") throw webrtcInvalidState("RTCDataChannel is not open");
      const payload = webrtcChannelPayload(data);
      const bytes = webrtcChannelBytes(payload);
      this.bufferedAmount += bytes;
      const peer = this.__peer;
      if (!peer || peer.readyState !== "open") {
        this.__drainBufferedAmount(bytes);
        throw webrtcInvalidState("The peer RTCDataChannel is closed");
      }
      let cloned;
      try {
        cloned = typeof payload === "string" ? payload
          : payload instanceof ArrayBuffer ? payload.slice(0)
          : (typeof Blob === "function" && payload instanceof Blob) ? payload.slice(0)
          : cloneStructuredValue(payload, new Map());
      } catch (error) {
        this.__drainBufferedAmount(bytes);
        throw error;
      }
      webrtcQueue(() => {
        this.__drainBufferedAmount(bytes);
        if (peer.readyState !== "open") return;
        let dataForPeer = cloned;
        if (peer.binaryType === "arraybuffer" && typeof Blob === "function" && cloned instanceof Blob) {
          // Blob.arrayBuffer() is asynchronous in the host model; retaining a
          // Blob here is deterministic and still a valid binary message.
          dataForPeer = cloned;
        }
        peer.dispatchEvent(new MessageEvent("message", { data: dataForPeer }));
      });
    }
    close() {
      if (this.readyState === "closed" || this.readyState === "closing") return;
      this.readyState = "closing";
      const peer = this.__peer;
      webrtcQueue(() => {
        this.__closeInternal();
        if (peer) peer.__closeInternal();
      });
    }
    __drainBufferedAmount(bytes) {
      const previous = this.bufferedAmount;
      this.bufferedAmount = Math.max(0, this.bufferedAmount - bytes);
      if (previous > this.bufferedAmountLowThreshold &&
          this.bufferedAmount <= this.bufferedAmountLowThreshold) {
        webrtcQueue(() => this.dispatchEvent(new Event("bufferedamountlow")));
      }
    }
    __open() {
      if (this.readyState !== "connecting") return;
      this.readyState = "open";
      webrtcQueue(() => this.dispatchEvent(new Event("open")));
    }
    __closeInternal() {
      if (this.__closedEvent) return;
      this.readyState = "closed";
      this.__closedEvent = true;
      webrtcQueue(() => this.dispatchEvent(new Event("close")));
    }
    __error(error) {
      webrtcQueue(() => this.dispatchEvent(new Event("error")));
      return error;
    }
    get onopen() { return this.__onopen; }
    set onopen(callback) { webrtcEventHandler(this, "open", "__onopen", callback); }
    get onmessage() { return this.__onmessage; }
    set onmessage(callback) { webrtcEventHandler(this, "message", "__onmessage", callback); }
    get onclose() { return this.__onclose; }
    set onclose(callback) { webrtcEventHandler(this, "close", "__onclose", callback); }
    get onerror() { return this.__onerror; }
    set onerror(callback) { webrtcEventHandler(this, "error", "__onerror", callback); }
    get onbufferedamountlow() { return this.__onbufferedamountlow; }
    set onbufferedamountlow(callback) {
      webrtcEventHandler(this, "bufferedamountlow", "__onbufferedamountlow", callback);
    }
    get [Symbol.toStringTag]() { return "RTCDataChannel"; }
  }

  function webrtcValidateConfiguration(configuration) {
    if (configuration == null) return {};
    if (typeof configuration !== "object") throw new TypeError("RTCConfiguration must be an object");
    const result = {};
    if (configuration.bundlePolicy !== undefined) {
      result.bundlePolicy = String(configuration.bundlePolicy);
      if (!WEBRTC_BUNDLE_POLICIES.has(result.bundlePolicy)) throw new TypeError("Invalid bundlePolicy");
    } else result.bundlePolicy = "balanced";
    if (configuration.iceTransportPolicy !== undefined) {
      result.iceTransportPolicy = String(configuration.iceTransportPolicy);
      if (!WEBRTC_ICE_TRANSPORT_POLICIES.has(result.iceTransportPolicy)) throw new TypeError("Invalid iceTransportPolicy");
    } else result.iceTransportPolicy = "all";
    if (configuration.rtcpMuxPolicy !== undefined) {
      result.rtcpMuxPolicy = String(configuration.rtcpMuxPolicy);
      if (!WEBRTC_RTCP_MUX_POLICIES.has(result.rtcpMuxPolicy)) throw new TypeError("Invalid rtcpMuxPolicy");
    } else result.rtcpMuxPolicy = "require";
    const poolSize = configuration.iceCandidatePoolSize === undefined
      ? 0 : Number(configuration.iceCandidatePoolSize);
    if (!Number.isInteger(poolSize) || poolSize < 0 || poolSize > 255) {
      throw new TypeError("iceCandidatePoolSize must be an integer from 0 to 255");
    }
    result.iceCandidatePoolSize = poolSize;
    if (configuration.iceServers !== undefined) {
      if (!Array.isArray(configuration.iceServers)) throw new TypeError("iceServers must be an array");
      result.iceServers = configuration.iceServers.map(server => {
        if (typeof server === "string") return { urls: server };
        if (server == null || typeof server !== "object") throw new TypeError("Invalid ice server");
        const urls = server.urls === undefined ? [] : server.urls;
        if (!(typeof urls === "string" || Array.isArray(urls))) throw new TypeError("Invalid ice server urls");
        return { urls: Array.isArray(urls) ? urls.map(String) : String(urls),
          username: server.username === undefined ? undefined : String(server.username),
          credential: server.credential === undefined ? undefined : server.credential };
      });
    } else result.iceServers = [];
    return result;
  }

  function webrtcSdp(id, type) {
    return [
      "v=0",
      "o=- " + id + " 2 IN IP4 127.0.0.1",
      "s=omoikane-deterministic-webrtc",
      "t=0 0",
      "a=group:BUNDLE 0",
      "m=application 9 UDP/DTLS/SCTP webrtc-datachannel",
      "a=mid:0",
      "a=sctp-port:5000",
      "a=setup:" + (type === "offer" ? "actpass" : "active"),
    ].join("\r\n") + "\r\n";
  }

  class RTCPeerConnection extends EventTarget {
    constructor(configuration = {}) {
      super();
      this.__configuration = webrtcValidateConfiguration(configuration);
      this.__serial = webrtcConnectionSerial++;
      this.__peer = null;
      this.__closed = false;
      this.__channels = new Set();
      this.__localDescription = null;
      this.__remoteDescription = null;
      this.__pendingLocalDescription = null;
      this.__pendingRemoteDescription = null;
      this.__connectionState = "new";
      this.__iceConnectionState = "new";
      this.__iceGatheringState = "new";
      this.__signalingState = "stable";
      this.__onconnectionstatechange = null;
      this.__onicecandidate = null;
      this.__onicecandidateerror = null;
      this.__oniceconnectionstatechange = null;
      this.__onicegatheringstatechange = null;
      this.__onnegotiationneeded = null;
      this.__onsignalingstatechange = null;
      this.__ondatachannel = null;
      this.__ontrack = null;
    }
    get canTrickleIceCandidates() { return this.__remoteDescription ? true : null; }
    get connectionState() { return this.__connectionState; }
    get iceConnectionState() { return this.__iceConnectionState; }
    get iceGatheringState() { return this.__iceGatheringState; }
    get signalingState() { return this.__signalingState; }
    get localDescription() { return this.__localDescription; }
    get currentLocalDescription() {
      return this.__signalingState === "stable" ? this.__localDescription : null;
    }
    get pendingLocalDescription() { return this.__pendingLocalDescription; }
    get remoteDescription() { return this.__remoteDescription; }
    get currentRemoteDescription() {
      return this.__signalingState === "stable" ? this.__remoteDescription : null;
    }
    get pendingRemoteDescription() { return this.__pendingRemoteDescription; }
    get sctp() { return null; }
    getConfiguration() { return JSON.parse(JSON.stringify(this.__configuration)); }
    setConfiguration(configuration) {
      if (this.__closed) throw webrtcInvalidState();
      this.__configuration = webrtcValidateConfiguration(configuration);
    }
    createOffer() {
      if (this.__closed) return Promise.reject(webrtcInvalidState());
      const description = webrtcSessionDescription("offer", webrtcSdp(this.__serial, "offer"), this);
      return Promise.resolve(description);
    }
    createAnswer() {
      if (this.__closed) return Promise.reject(webrtcInvalidState());
      if (this.__signalingState !== "have-remote-offer" && this.__signalingState !== "have-local-pranswer") {
        return Promise.reject(new DOMException("Cannot create an answer in the current signaling state.", "InvalidStateError"));
      }
      const description = webrtcSessionDescription("answer", webrtcSdp(this.__serial, "answer"), this);
      return Promise.resolve(description);
    }
    setLocalDescription(description = undefined) {
      if (this.__closed) return Promise.reject(webrtcInvalidState());
      let normalized;
      try {
        if (description === undefined) {
          normalized = this.__signalingState === "stable"
            ? webrtcSessionDescription("offer", webrtcSdp(this.__serial, "offer"), this)
            : this.__signalingState === "have-remote-offer"
              ? webrtcSessionDescription("answer", webrtcSdp(this.__serial, "answer"), this)
              : (() => { throw new DOMException("Cannot infer a local description.", "InvalidStateError"); })();
        } else normalized = webrtcNormalizeSessionDescription(description);
        this.__applyLocalDescription(normalized);
      } catch (error) {
        return Promise.reject(error);
      }
      return Promise.resolve();
    }
    setRemoteDescription(description) {
      if (this.__closed) return Promise.reject(webrtcInvalidState());
      let normalized;
      try {
        normalized = webrtcNormalizeSessionDescription(description);
        this.__applyRemoteDescription(normalized);
      } catch (error) {
        return Promise.reject(error);
      }
      return Promise.resolve();
    }
    addIceCandidate(candidate = null) {
      if (this.__closed) return Promise.reject(webrtcInvalidState());
      if (candidate !== null && candidate !== undefined &&
          !(candidate instanceof RTCIceCandidate) && typeof candidate !== "object") {
        return Promise.reject(new TypeError("RTCIceCandidateInit must be an object"));
      }
      if (!this.__remoteDescription && candidate != null && candidate.candidate !== "") {
        return Promise.reject(new DOMException("Remote description is not set.", "InvalidStateError"));
      }
      return Promise.resolve();
    }
    createDataChannel(label, options = undefined) {
      if (this.__closed) throw webrtcInvalidState();
      const value = options == null ? {} : Object(options);
      if (value.maxPacketLifeTime != null && value.maxRetransmits != null) {
        throw new TypeError("maxPacketLifeTime and maxRetransmits are mutually exclusive");
      }
      const lifetime = value.maxPacketLifeTime == null ? null : Number(value.maxPacketLifeTime);
      const retransmits = value.maxRetransmits == null ? null : Number(value.maxRetransmits);
      if (lifetime !== null && (!Number.isInteger(lifetime) || lifetime < 0)) throw new TypeError("Invalid maxPacketLifeTime");
      if (retransmits !== null && (!Number.isInteger(retransmits) || retransmits < 0)) throw new TypeError("Invalid maxRetransmits");
      const normalized = {
        ordered: value.ordered !== false,
        maxPacketLifeTime: lifetime,
        maxRetransmits: retransmits,
        protocol: value.protocol === undefined ? "" : String(value.protocol),
        negotiated: value.negotiated === true,
      };
      const id = value.negotiated ? (value.id === undefined ? null : Number(value.id)) : webrtcDataChannelSerial++;
      if (normalized.negotiated && (!Number.isInteger(id) || id < 0 || id > 65534)) {
        throw new TypeError("negotiated data channels require an id from 0 to 65534");
      }
      const channel = new RTCDataChannel(webrtcDataChannelConstructionToken, this, label, normalized, id);
      this.__channels.add(channel);
      this.__attachChannel(channel);
      return channel;
    }
    restartIce() {
      if (this.__closed) throw webrtcInvalidState();
      if (this.__signalingState === "stable") webrtcQueue(() => this.dispatchEvent(new Event("negotiationneeded")));
    }
    getStats() {
      if (this.__closed) return Promise.reject(webrtcInvalidState());
      return Promise.resolve(new Map());
    }
    close() {
      if (this.__closed) return;
      this.__closed = true;
      this.__connectionState = "closed";
      this.__iceConnectionState = "closed";
      this.__signalingState = "closed";
      webrtcQueue(() => {
        this.dispatchEvent(new Event("connectionstatechange"));
        this.dispatchEvent(new Event("iceconnectionstatechange"));
        this.dispatchEvent(new Event("signalingstatechange"));
      });
      for (const channel of this.__channels) channel.__closeInternal();
      if (this.__peer && !this.__peer.__closed) {
        const peer = this.__peer;
        this.__peer = null;
        peer.__peer = null;
        peer.__closeFromPeer();
      }
      this.__channels.clear();
    }
    __closeFromPeer() {
      if (this.__closed) return;
      this.__closed = true;
      this.__connectionState = "closed";
      this.__iceConnectionState = "closed";
      this.__signalingState = "closed";
      webrtcQueue(() => {
        this.dispatchEvent(new Event("connectionstatechange"));
        this.dispatchEvent(new Event("iceconnectionstatechange"));
        this.dispatchEvent(new Event("signalingstatechange"));
      });
      for (const channel of this.__channels) {
        channel.__closeInternal();
        if (channel.__peer) channel.__peer.__closeInternal();
      }
      this.__channels.clear();
    }
    __applyLocalDescription(description) {
      const type = description.type;
      if (type === "rollback") {
        if (this.__signalingState !== "have-local-offer" && this.__signalingState !== "have-remote-pranswer") {
          throw new DOMException("Cannot rollback in the current signaling state.", "InvalidStateError");
        }
        this.__pendingLocalDescription = null;
        this.__pendingRemoteDescription = null;
        if (this.__localDescription && (this.__localDescription.type === "offer" ||
            this.__localDescription.type === "pranswer")) this.__localDescription = null;
        this.__setSignalingState("stable");
        return;
      }
      const valid = (type === "offer" && this.__signalingState === "stable") ||
        (type === "answer" && this.__signalingState === "have-remote-offer") ||
        (type === "pranswer" && this.__signalingState === "have-remote-offer");
      if (!valid) throw new DOMException("Invalid local description for the current signaling state.", "InvalidStateError");
      this.__localDescription = description;
      this.__pendingLocalDescription = type === "offer" || type === "pranswer" ? description : null;
      if (type === "offer") this.__setSignalingState("have-local-offer");
      else if (type === "pranswer") this.__setSignalingState("have-local-pranswer");
      else {
        this.__pendingLocalDescription = null;
        this.__setSignalingState("stable");
      }
      this.__beginIceGathering();
      this.__maybeConnect();
    }
    __applyRemoteDescription(description) {
      const type = description.type;
      if (type === "rollback") {
        if (this.__signalingState !== "have-remote-offer" && this.__signalingState !== "have-local-pranswer") {
          throw new DOMException("Cannot rollback in the current signaling state.", "InvalidStateError");
        }
        this.__pendingRemoteDescription = null;
        this.__pendingLocalDescription = null;
        if (this.__remoteDescription && (this.__remoteDescription.type === "offer" ||
            this.__remoteDescription.type === "pranswer")) this.__remoteDescription = null;
        this.__setSignalingState("stable");
        return;
      }
      const valid = (type === "offer" && this.__signalingState === "stable") ||
        (type === "answer" && this.__signalingState === "have-local-offer") ||
        (type === "pranswer" && this.__signalingState === "have-local-offer");
      if (!valid) throw new DOMException("Invalid remote description for the current signaling state.", "InvalidStateError");
      this.__remoteDescription = description;
      this.__pendingRemoteDescription = type === "offer" || type === "pranswer" ? description : null;
      const owner = webrtcDescriptionOwner(description);
      if (owner && owner !== this) this.__linkPeer(owner);
      if (type === "offer") this.__setSignalingState("have-remote-offer");
      else if (type === "pranswer") this.__setSignalingState("have-remote-pranswer");
      else {
        this.__pendingRemoteDescription = null;
        this.__setSignalingState("stable");
      }
      this.__maybeConnect();
    }
    __setSignalingState(value) {
      if (this.__signalingState === value) return;
      this.__signalingState = value;
      webrtcQueue(() => this.dispatchEvent(new Event("signalingstatechange")));
    }
    __beginIceGathering() {
      if (this.__iceGatheringState !== "new") return;
      this.__iceGatheringState = "gathering";
      webrtcQueue(() => {
        this.dispatchEvent(new Event("icegatheringstatechange"));
        const candidateEvent = new RTCPeerConnectionIceEvent("icecandidate", { candidate: null });
        this.dispatchEvent(candidateEvent);
        this.__iceGatheringState = "complete";
        this.dispatchEvent(new Event("icegatheringstatechange"));
      });
    }
    __attachChannel(channel) {
      if (!this.__peer) return;
      this.__pairChannel(channel, this.__peer);
    }
    __pairChannel(channel, peer) {
      if (channel.__peer || !peer || peer.__closed) return;
      const remote = new RTCDataChannel(
        webrtcDataChannelConstructionToken, peer, channel.label,
        { ordered: channel.ordered, maxPacketLifeTime: channel.maxPacketLifeTime,
          maxRetransmits: channel.maxRetransmits, protocol: channel.protocol,
          negotiated: channel.negotiated }, channel.id,
      );
      channel.__peer = remote;
      remote.__peer = channel;
      peer.__channels.add(remote);
      if (peer.__connectionState === "connected") peer.__openChannels();
    }
    __linkPeer(peer) {
      if (!peer || peer === this || peer.__closed || this.__closed) return;
      if (this.__peer === peer) return;
      this.__peer = peer;
      if (peer.__peer !== this) peer.__peer = this;
      for (const channel of this.__channels) this.__pairChannel(channel, peer);
      for (const channel of peer.__channels) peer.__pairChannel(channel, this);
      this.__maybeConnect();
      peer.__maybeConnect();
    }
    __maybeConnect() {
      const peer = this.__peer;
      if (!peer || this.__closed || peer.__closed) return;
      const localType = this.__localDescription && this.__localDescription.type;
      const remoteType = this.__remoteDescription && this.__remoteDescription.type;
      const negotiated = (localType === "offer" && remoteType === "answer") ||
        (localType === "answer" && remoteType === "offer");
      if (this.__signalingState !== "stable" || peer.__signalingState !== "stable" || !negotiated) {
        if (this.__signalingState === "have-local-offer" || this.__signalingState === "have-remote-offer") {
          webrtcState(this, "__connectionState", "connectionstatechange", "connecting");
          webrtcState(this, "__iceConnectionState", "iceconnectionstatechange", "checking");
        }
        return;
      }
      webrtcQueue(() => {
        if (this.__closed || peer.__closed) return;
        for (const connection of [this, peer]) {
          webrtcState(connection, "__connectionState", "connectionstatechange", "connected");
          webrtcState(connection, "__iceConnectionState", "iceconnectionstatechange", "completed");
        }
        this.__openChannels();
        peer.__openChannels();
      });
    }
    __openChannels() {
      for (const channel of this.__channels) {
        if (!channel.__peer || channel.__announced) continue;
        channel.__announced = true;
        const remote = channel.__peer;
        remote.__announced = true;
        const remoteOwner = remote.__owner;
        webrtcQueue(() => {
          if (remote.readyState === "connecting") {
            const event = new MessageEvent("datachannel", { data: undefined });
            event.channel = remote;
            remoteOwner.dispatchEvent(event);
            remote.__open();
            channel.__open();
          }
        });
      }
    }
    get onconnectionstatechange() { return this.__onconnectionstatechange; }
    set onconnectionstatechange(callback) { webrtcEventHandler(this, "connectionstatechange", "__onconnectionstatechange", callback); }
    get onicecandidate() { return this.__onicecandidate; }
    set onicecandidate(callback) { webrtcEventHandler(this, "icecandidate", "__onicecandidate", callback); }
    get onicecandidateerror() { return this.__onicecandidateerror; }
    set onicecandidateerror(callback) { webrtcEventHandler(this, "icecandidateerror", "__onicecandidateerror", callback); }
    get oniceconnectionstatechange() { return this.__oniceconnectionstatechange; }
    set oniceconnectionstatechange(callback) { webrtcEventHandler(this, "iceconnectionstatechange", "__oniceconnectionstatechange", callback); }
    get onicegatheringstatechange() { return this.__onicegatheringstatechange; }
    set onicegatheringstatechange(callback) { webrtcEventHandler(this, "icegatheringstatechange", "__onicegatheringstatechange", callback); }
    get onnegotiationneeded() { return this.__onnegotiationneeded; }
    set onnegotiationneeded(callback) { webrtcEventHandler(this, "negotiationneeded", "__onnegotiationneeded", callback); }
    get onsignalingstatechange() { return this.__onsignalingstatechange; }
    set onsignalingstatechange(callback) { webrtcEventHandler(this, "signalingstatechange", "__onsignalingstatechange", callback); }
    get ondatachannel() { return this.__ondatachannel; }
    set ondatachannel(callback) { webrtcEventHandler(this, "datachannel", "__ondatachannel", callback); }
    get ontrack() { return this.__ontrack; }
    set ontrack(callback) { webrtcEventHandler(this, "track", "__ontrack", callback); }
    get [Symbol.toStringTag]() { return "RTCPeerConnection"; }
    static createPair(leftConfiguration = {}, rightConfiguration = {}) {
      const left = new RTCPeerConnection(leftConfiguration);
      const right = new RTCPeerConnection(rightConfiguration);
      left.__linkPeer(right);
      const pair = [left, right];
      pair.left = left;
      pair.right = right;
      pair.local = left;
      pair.remote = right;
      pair.peers = pair;
      return pair;
    }
    static createDeterministicPair(leftConfiguration = {}, rightConfiguration = {}) {
      return RTCPeerConnection.createPair(leftConfiguration, rightConfiguration);
    }
  }

  globalThis.RTCSessionDescription = RTCSessionDescription;
  globalThis.RTCIceCandidate = RTCIceCandidate;
  globalThis.RTCPeerConnectionIceEvent = RTCPeerConnectionIceEvent;
  globalThis.RTCDataChannel = RTCDataChannel;
  globalThis.RTCPeerConnection = RTCPeerConnection;
  globalThis.__omoikane_create_webrtc_peer_pair = function(leftConfiguration = {}, rightConfiguration = {}) {
    return RTCPeerConnection.createPair(leftConfiguration, rightConfiguration);
  };
  globalThis.__omoikane_create_webrtc_pair = globalThis.__omoikane_create_webrtc_peer_pair;
  globalThis.__omoikane_connect_webrtc_peers = function(left, right) {
    if (!(left instanceof RTCPeerConnection) || !(right instanceof RTCPeerConnection)) {
      throw new TypeError("RTCPeerConnection peers are required");
    }
    left.__linkPeer(right);
    return [left, right];
  };
  RTCPeerConnection.__createPair = RTCPeerConnection.createPair;
  RTCPeerConnection.__createPeerPair = RTCPeerConnection.createPair;
  RTCPeerConnection.connectPeers = function(left, right) {
    return globalThis.__omoikane_connect_webrtc_peers(left, right);
  };

  // -------------------------------------------------------------------------
  // WebTransport deterministic client/session/stream core.
  //
  // The real WebTransport transport is QUIC/HTTP-3 based and therefore cannot
  // be provided by the in-process engine without introducing a network
  // backend.  This model keeps the Web IDL-facing lifecycle useful by
  // connecting two clients through an explicit same-realm pair hook.  Every
  // byte delivered through that hook is copied before it is queued, so tests
  // observe the same ownership boundary as a structured-clone/message path.
  // There is deliberately no network, TLS, proxy, congestion, or certificate
  // implementation here.
  // -------------------------------------------------------------------------
  const WEBTRANSPORT_CLOSE_CODE_MAX = 0xFFFFFFFF;
  const WEBTRANSPORT_DEFAULT_HIGH_WATER_MARK = 16;
  const WEBTRANSPORT_MAX_PENDING_WRITES = 64;
  const WEBTRANSPORT_MAX_DATAGRAM_SIZE = 65536;
  const WEBTRANSPORT_CONGESTION_CONTROLS = ["default", "throughput"];
  const WEBTRANSPORT_STREAM_SOURCES = ["stream", "session"];

  function webTransportInvalidState(message = "The WebTransport is closed.") {
    return new DOMException(message, "InvalidStateError");
  }

  function webTransportBufferSource(value) {
    if (value instanceof ArrayBuffer) return new Uint8Array(value.slice(0));
    if (ArrayBuffer.isView(value)) {
      return new Uint8Array(value.buffer, value.byteOffset, value.byteLength).slice();
    }
    throw new TypeError("WebTransport data must be an ArrayBuffer or ArrayBufferView");
  }

  function webTransportNumber(value, fallback, name, max = Infinity) {
    if (value === undefined) return fallback;
    const number = Number(value);
    if (!Number.isInteger(number) || number < 0 || number > max) {
      throw new TypeError(`${name} must be a non-negative integer`);
    }
    return number;
  }

  function webTransportReadable(hwm = WEBTRANSPORT_DEFAULT_HIGH_WATER_MARK, onConsume = null, onCancel = null, target = null) {
    const stream = target || new ReadableStream();
    stream.__webTransportHighWaterMark = hwm;
    stream.__webTransportClosed = false;
    stream.__webTransportPending = [];
    stream.__webTransportCanEnqueue = () =>
      !stream.__webTransportClosed && stream._queue.length < stream.__webTransportHighWaterMark;
    stream.__webTransportEnqueue = value => {
      if (!stream.__webTransportCanEnqueue()) return false;
      stream._controller.enqueue(value);
      return true;
    };
    stream.__webTransportFlush = () => {
      while (stream.__webTransportPending.length && stream.__webTransportCanEnqueue()) {
        const pending = stream.__webTransportPending.shift();
        if (stream.__webTransportEnqueue(pending.value)) pending.resolve();
        else pending.reject(webTransportInvalidState());
      }
    };
    stream.__webTransportClose = () => {
      if (stream.__webTransportClosed) return;
      stream.__webTransportClosed = true;
      stream._controller.close();
      for (const pending of stream.__webTransportPending.splice(0)) {
        pending.reject(webTransportInvalidState());
      }
    };
    stream.__webTransportError = reason => {
      if (stream.__webTransportClosed) return;
      stream.__webTransportClosed = true;
      stream._controller.error(reason);
      for (const pending of stream.__webTransportPending.splice(0)) pending.reject(reason);
    };
    const originalGetReader = stream.getReader.bind(stream);
    stream.getReader = function(...args) {
      const reader = originalGetReader(...args);
      const originalRead = reader.read.bind(reader);
      reader.read = function(...readArgs) {
        return originalRead(...readArgs).then(result => {
          if (result && !result.done) {
            if (typeof onConsume === "function") onConsume();
            stream.__webTransportFlush();
          }
          return result;
        });
      };
      const originalCancel = reader.cancel.bind(reader);
      reader.cancel = function(reason) {
        return originalCancel(reason);
      };
      return reader;
    };
    const originalCancel = stream.cancel.bind(stream);
    stream.cancel = function(reason) {
      if (!stream.__webTransportClosed) {
        if (typeof onCancel === "function") onCancel(reason);
        stream.__webTransportClose();
      }
      return originalCancel(reason);
    };
    return stream;
  }

  class WebTransportError extends Error {
    constructor(init = {}) {
      const source = typeof init === "object" && init !== null ? init : { message: init };
      super(source.message === undefined ? "" : String(source.message));
      this.name = "WebTransportError";
      this.source = WEBTRANSPORT_STREAM_SOURCES.includes(source.source) ? source.source : "stream";
      this.streamErrorCode = source.streamErrorCode === undefined || source.streamErrorCode === null
        ? null : webTransportNumber(source.streamErrorCode, 0, "streamErrorCode", WEBTRANSPORT_CLOSE_CODE_MAX);
    }
    get [Symbol.toStringTag]() { return "WebTransportError"; }
  }

  class WebTransportCloseInfo {
    constructor(init = {}) {
      if (init == null || typeof init !== "object") throw new TypeError("WebTransportCloseInfo must be an object");
      const closeCode = webTransportNumber(init.closeCode, 0, "closeCode", WEBTRANSPORT_CLOSE_CODE_MAX);
      const reason = init.reason === undefined ? "" : String(init.reason);
      if (reason.length > 1024) throw new TypeError("WebTransport close reason is too long");
      Object.defineProperty(this, "closeCode", { configurable: false, enumerable: true, writable: false, value: closeCode });
      Object.defineProperty(this, "reason", { configurable: false, enumerable: true, writable: false, value: reason });
    }
    toJSON() { return { closeCode: this.closeCode, reason: this.reason }; }
    get [Symbol.toStringTag]() { return "WebTransportCloseInfo"; }
  }

  class WebTransportReadableStream extends ReadableStream {
    constructor(hwm, onConsume = null, onCancel = null) {
      super();
      return webTransportReadable(hwm, onConsume, onCancel, this);
    }
    get [Symbol.toStringTag]() { return "ReadableStream"; }
  }

  class WebTransportWritableStream extends WritableStream {
    constructor(send, close, abort) {
      super({});
      this.__webTransportSend = send;
      this.__webTransportClose = close;
      this.__webTransportAbort = abort;
    }
    _write(chunk) {
      if (this._closed) return Promise.reject(webTransportInvalidState());
      try { return Promise.resolve(this.__webTransportSend(chunk)); }
      catch (error) { return Promise.reject(error); }
    }
    _close() {
      if (this._closed) return Promise.resolve();
      this._closed = true;
      let result;
      try { result = this.__webTransportClose(); }
      catch (error) { this._closedResolve(); return Promise.reject(error); }
      this._closedResolve();
      return Promise.resolve(result);
    }
    abort(reason) {
      if (this._closed) return Promise.resolve();
      this._closed = true;
      let result;
      try { result = this.__webTransportAbort(reason); }
      catch (error) { this._closedResolve(); return Promise.reject(error); }
      this._closedResolve();
      return Promise.resolve(result);
    }
    get [Symbol.toStringTag]() { return "WritableStream"; }
  }

  class WebTransportReceiveStream extends WebTransportReadableStream {
    get [Symbol.toStringTag]() { return "WebTransportReceiveStream"; }
  }

  class WebTransportSendStream extends WebTransportWritableStream {
    get [Symbol.toStringTag]() { return "WebTransportSendStream"; }
  }

  class WebTransportDatagrams {
    constructor(owner) {
      this.__owner = owner;
      this.__incomingHighWaterMark = WEBTRANSPORT_DEFAULT_HIGH_WATER_MARK;
      this.__outgoingHighWaterMark = WEBTRANSPORT_DEFAULT_HIGH_WATER_MARK;
      this.incomingMaxAge = 0;
      this.outgoingMaxAge = 0;
      this.incomingHighWaterMark = this.__incomingHighWaterMark;
      this.outgoingHighWaterMark = this.__outgoingHighWaterMark;
      this.readable = owner.__datagramReadable;
      this.writable = new WebTransportWritableStream(
        value => owner.__writeDatagram(value),
        () => owner.__closeDatagrams(),
        reason => owner.__abortDatagrams(reason),
      );
    }
    get incomingHighWaterMark() { return this.__incomingHighWaterMark; }
    set incomingHighWaterMark(value) {
      this.__incomingHighWaterMark = webTransportNumber(value, WEBTRANSPORT_DEFAULT_HIGH_WATER_MARK, "incomingHighWaterMark", 65536);
      if (this.readable) this.readable.__webTransportHighWaterMark = this.__incomingHighWaterMark;
    }
    get outgoingHighWaterMark() { return this.__outgoingHighWaterMark; }
    set outgoingHighWaterMark(value) {
      this.__outgoingHighWaterMark = webTransportNumber(value, WEBTRANSPORT_DEFAULT_HIGH_WATER_MARK, "outgoingHighWaterMark", 65536);
    }
    get [Symbol.toStringTag]() { return "WebTransportDatagrams"; }
  }

  class WebTransportBidirectionalStream {
    constructor(owner, readHighWaterMark = WEBTRANSPORT_DEFAULT_HIGH_WATER_MARK) {
      this.__owner = owner;
      this.__peerStream = null;
      this.__writableClosed = false;
      this.__readableClosed = false;
      this.readable = new WebTransportReceiveStream(
        readHighWaterMark,
        () => this.__flushReadable(),
        reason => this.__cancelReadable(reason),
      );
      this.writable = new WebTransportSendStream(
        value => this.__write(value),
        () => this.__closeWritable(),
        reason => this.__abortWritable(reason),
      );
    }
    __write(value) {
      if (this.__writableClosed || this.__owner.__closed) return Promise.reject(webTransportInvalidState());
      const bytes = webTransportBufferSource(value);
      if (!this.__peerStream || this.__peerStream.__owner.__closed) return Promise.reject(webTransportInvalidState("The peer WebTransport is closed."));
      return this.__peerStream.__enqueueReadable(bytes);
    }
    __closeWritable() {
      if (this.__writableClosed) return Promise.resolve();
      this.__writableClosed = true;
      if (this.__peerStream) this.__peerStream.__closeReadable();
      return Promise.resolve();
    }
    __abortWritable(reason) {
      if (this.__writableClosed) return Promise.resolve();
      this.__writableClosed = true;
      if (this.__peerStream) this.__peerStream.__errorReadable(new WebTransportError({
        source: "stream", message: reason === undefined ? "The stream was aborted." : String(reason),
      }));
      return Promise.resolve();
    }
    __enqueueReadable(value) {
      if (this.__readableClosed) return Promise.reject(webTransportInvalidState("The receive stream is closed."));
      const pending = this.readable.__webTransportEnqueue(value);
      if (pending) return Promise.resolve();
      if (this.readable.__webTransportPending.length >= WEBTRANSPORT_MAX_PENDING_WRITES) {
        return Promise.reject(new WebTransportError({ source: "stream", message: "The receive stream backpressure limit was exceeded." }));
      }
      return new Promise((resolve, reject) => {
        this.readable.__webTransportPending.push({ value, resolve, reject });
      });
    }
    __flushReadable() { this.readable.__webTransportFlush(); }
    __closeReadable() {
      if (this.__readableClosed) return;
      this.__readableClosed = true;
      this.readable.__webTransportClose();
    }
    __errorReadable(reason) {
      if (this.__readableClosed) return;
      this.__readableClosed = true;
      this.readable.__webTransportError(reason);
    }
    __cancelReadable(reason) {
      if (this.__peerStream && !this.__peerStream.__writableClosed) this.__peerStream.__abortWritable(reason);
    }
    __closeInternal(reason = undefined) {
      if (!this.__writableClosed) {
        this.__writableClosed = true;
        if (this.__peerStream && !this.__peerStream.__readableClosed) {
          if (reason === undefined) this.__peerStream.__closeReadable();
          else this.__peerStream.__errorReadable(reason);
        }
      }
      this.__closeReadable();
    }
    get [Symbol.toStringTag]() { return "WebTransportBidirectionalStream"; }
  }

  class WebTransport extends EventTarget {
    constructor(url, options = {}) {
      super();
      if (options == null || typeof options !== "object") throw new TypeError("WebTransport options must be an object");
      this.url = WebTransport.__normalizeURL(url);
      const congestionControl = options.congestionControl === undefined ? "default" : String(options.congestionControl);
      if (!WEBTRANSPORT_CONGESTION_CONTROLS.includes(congestionControl)) {
        throw new TypeError("Invalid WebTransport congestionControl");
      }
      if (options.requireUnreliable !== undefined && typeof options.requireUnreliable !== "boolean") {
        throw new TypeError("WebTransport requireUnreliable must be a boolean");
      }
      if (options.allowPooling !== undefined && typeof options.allowPooling !== "boolean") {
        throw new TypeError("WebTransport allowPooling must be a boolean");
      }
      if (options.serverCertificateHashes !== undefined) {
        if (options.serverCertificateHashes == null || typeof options.serverCertificateHashes[Symbol.iterator] !== "function") {
          throw new TypeError("serverCertificateHashes must be iterable");
        }
        for (const hash of options.serverCertificateHashes) {
          if (hash == null || typeof hash !== "object" || typeof hash.algorithm !== "string" || hash.value === undefined) {
            throw new TypeError("Invalid serverCertificateHashes entry");
          }
          webTransportBufferSource(hash.value);
        }
      }
      this.congestionControl = congestionControl;
      this.requireUnreliable = options.requireUnreliable === true;
      this.allowPooling = options.allowPooling !== false;
      this.__state = "connecting";
      this.__peer = null;
      this.__closed = false;
      this.__streams = new Set();
      this.__pendingDatagrams = [];
      this.__pendingIncomingBidirectional = [];
      this.__pendingIncomingUnidirectional = [];
      this.__datagramReadable = webTransportReadable(WEBTRANSPORT_DEFAULT_HIGH_WATER_MARK,
        () => this.__flushDatagrams(), reason => this.__abortDatagrams(reason));
      this.__datagramReadable.__webTransportHighWaterMark = WEBTRANSPORT_DEFAULT_HIGH_WATER_MARK;
      this.datagrams = new WebTransportDatagrams(this);
      this.incomingBidirectionalStreams = webTransportReadable(WEBTRANSPORT_DEFAULT_HIGH_WATER_MARK,
        () => this.__flushIncoming(this.__pendingIncomingBidirectional, this.incomingBidirectionalStreams));
      this.incomingUnidirectionalStreams = webTransportReadable(WEBTRANSPORT_DEFAULT_HIGH_WATER_MARK,
        () => this.__flushIncoming(this.__pendingIncomingUnidirectional, this.incomingUnidirectionalStreams));
      this.__incomingBidirectional = this.incomingBidirectionalStreams;
      this.__incomingUnidirectional = this.incomingUnidirectionalStreams;
      this.__ready = new Promise((resolve, reject) => { this.__readyResolve = resolve; this.__readyReject = reject; });
      this.__closedPromise = new Promise(resolve => { this.__closedResolve = resolve; });
      this.ready = this.__ready;
      this.closed = this.__closedPromise;
      this.draining = false;
      this.__closeInfo = null;
      this.__onclose = null;
      this.__onstatechange = null;
      this.__onerror = null;
    }
    static __normalizeURL(value) {
      let parsed;
      try {
        parsed = value instanceof URL ? new URL(value.href) : new URL(String(value), globalThis.location && globalThis.location.href || "https://omoikane.invalid/");
      } catch (_) {
        throw new TypeError("Invalid WebTransport URL");
      }
      if (parsed.protocol !== "https:") throw new DOMException("WebTransport requires a secure URL.", "SecurityError");
      if (parsed.username || parsed.password || parsed.hash) throw new TypeError("Invalid WebTransport URL");
      return parsed.href;
    }
    __dispatchState() {
      const event = new Event("statechange");
      this.dispatchEvent(event);
      if (typeof this.__onstatechange === "function") this.__onstatechange.call(this, event);
    }
    __connect(peer) {
      if (this.__closed || !peer || peer.__closed) return;
      if (this.__peer === peer && this.__state === "connected") return;
      this.__peer = peer;
      this.__state = "connected";
      queueMicrotask(() => {
        if (this.__closed) return;
        this.__readyResolve(this);
        this.__dispatchState();
      });
    }
    __whenReady(action) {
      if (this.__closed) return Promise.reject(webTransportInvalidState());
      if (this.__state === "connected") return Promise.resolve().then(action);
      return this.ready.then(() => action());
    }
    __writeDatagram(value) {
      if (this.__closed) return Promise.reject(webTransportInvalidState());
      let bytes;
      try { bytes = webTransportBufferSource(value); }
      catch (error) { return Promise.reject(error); }
      if (bytes.byteLength > WEBTRANSPORT_MAX_DATAGRAM_SIZE) {
        return Promise.reject(new DOMException("The datagram is too large.", "QuotaExceededError"));
      }
      if (this.__state !== "connected") return this.ready.then(() => this.__writeDatagram(bytes));
      if (!this.__peer || this.__peer.__closed) return Promise.reject(webTransportInvalidState("The peer WebTransport is closed."));
      return this.__peer.__enqueueDatagram(bytes);
    }
    __enqueueDatagram(bytes) {
      if (this.__closed) return Promise.reject(webTransportInvalidState("The peer WebTransport is closed."));
      if (this.__datagramReadable.__webTransportEnqueue(new Uint8Array(bytes))) return Promise.resolve();
      if (this.__pendingDatagrams.length >= WEBTRANSPORT_MAX_PENDING_WRITES) {
        return Promise.reject(new DOMException("The incoming datagram queue is full.", "QuotaExceededError"));
      }
      return new Promise((resolve, reject) => this.__pendingDatagrams.push({ value: new Uint8Array(bytes), resolve, reject }));
    }
    __flushDatagrams() {
      while (this.__pendingDatagrams.length && this.__datagramReadable.__webTransportCanEnqueue()) {
        const pending = this.__pendingDatagrams.shift();
        if (this.__datagramReadable.__webTransportEnqueue(pending.value)) pending.resolve();
        else pending.reject(webTransportInvalidState());
      }
    }
    __closeDatagrams() { return Promise.resolve(); }
    __abortDatagrams(reason) {
      const error = reason instanceof WebTransportError ? reason : new WebTransportError({
        source: "session", message: String(reason === undefined ? "Datagrams were aborted." : reason),
      });
      this.__datagramReadable.__webTransportError(error);
      for (const pending of this.__pendingDatagrams.splice(0)) pending.reject(error);
      return Promise.resolve();
    }
    __flushIncoming(pending, readable) {
      while (pending.length && readable.__webTransportCanEnqueue()) {
        const stream = pending.shift();
        if (!readable.__webTransportEnqueue(stream)) break;
      }
    }
    __enqueueIncoming(readable, pending, stream) {
      if (this.__closed) {
        if (typeof stream.__closeInternal === "function") stream.__closeInternal(webTransportInvalidState());
        else if (typeof stream.__webTransportError === "function") stream.__webTransportError(webTransportInvalidState());
        return;
      }
      if (!readable.__webTransportEnqueue(stream)) pending.push(stream);
    }
    createBidirectionalStream(options = {}) {
      if (options == null || typeof options !== "object") return Promise.reject(new TypeError("Stream options must be an object"));
      if (options.sendOrder !== undefined) {
        try { webTransportNumber(options.sendOrder, 0, "sendOrder"); }
        catch (error) { return Promise.reject(error); }
      }
      return this.__whenReady(() => {
        if (this.__closed || !this.__peer || this.__peer.__closed) throw webTransportInvalidState();
        const local = new WebTransportBidirectionalStream(this);
        const remote = new WebTransportBidirectionalStream(this.__peer);
        local.__peerStream = remote; remote.__peerStream = local;
        this.__streams.add(local); this.__peer.__streams.add(remote);
        this.__peer.__enqueueIncoming(this.__peer.__incomingBidirectional, this.__peer.__pendingIncomingBidirectional, remote);
        return local;
      });
    }
    createUnidirectionalStream(options = {}) {
      if (options == null || typeof options !== "object") return Promise.reject(new TypeError("Stream options must be an object"));
      if (options.sendOrder !== undefined) {
        try { webTransportNumber(options.sendOrder, 0, "sendOrder"); }
        catch (error) { return Promise.reject(error); }
      }
      return this.__whenReady(() => {
        if (this.__closed || !this.__peer || this.__peer.__closed) throw webTransportInvalidState();
        const local = { __owner: this, __writableClosed: false, __peerStream: null };
        const remote = new WebTransportReceiveStream(WEBTRANSPORT_DEFAULT_HIGH_WATER_MARK,
          () => remote.__webTransportFlush(), reason => { if (local.__peerStream) local.__peerStream.__abort(reason); });
        remote.__closeInternal = reason => {
          if (reason === undefined) remote.__webTransportClose();
          else remote.__webTransportError(reason);
        };
        const send = new WebTransportSendStream(
          value => {
            if (local.__writableClosed || this.__closed) return Promise.reject(webTransportInvalidState());
            const bytes = webTransportBufferSource(value);
            return remote.__webTransportEnqueue(bytes) ? Promise.resolve() : new Promise((resolve, reject) => {
              if (remote.__webTransportPending.length >= WEBTRANSPORT_MAX_PENDING_WRITES) reject(new WebTransportError({ source: "stream", message: "The receive stream backpressure limit was exceeded." }));
              else remote.__webTransportPending.push({ value: bytes, resolve, reject });
            });
          },
          () => { local.__writableClosed = true; remote.__webTransportClose(); },
          reason => { local.__writableClosed = true; remote.__webTransportError(new WebTransportError({ source: "stream", message: String(reason === undefined ? "The stream was aborted." : reason) })); },
        );
        local.__closeInternal = reason => {
          if (local.__writableClosed) return;
          local.__writableClosed = true;
          if (reason === undefined) remote.__webTransportClose();
          else remote.__webTransportError(reason);
        };
        local.writable = send;
        local.__peerStream = remote;
        this.__streams.add(local);
        this.__peer.__streams.add(remote);
        this.__peer.__enqueueIncoming(this.__peer.__incomingUnidirectional, this.__peer.__pendingIncomingUnidirectional, remote);
        return send;
      });
    }
    close(info = {}) {
      if (info == null || typeof info !== "object") throw new TypeError("WebTransportCloseInfo must be an object");
      if (this.__closed) return;
      const closeInfo = new WebTransportCloseInfo(info);
      this.__closed = true;
      this.__state = "closed";
      this.__closeInfo = closeInfo;
      this.draining = false;
      this.__readyReject(webTransportInvalidState("The WebTransport was closed before it became ready."));
      this.__datagramReadable.__webTransportClose();
      this.__incomingBidirectional.__webTransportClose();
      this.__incomingUnidirectional.__webTransportClose();
      this.__datagramReadable._queue.length = 0;
      this.__incomingBidirectional._queue.length = 0;
      this.__incomingUnidirectional._queue.length = 0;
      if (this.datagrams && this.datagrams.writable && !this.datagrams.writable._closed) {
        this.datagrams.writable._close();
      }
      for (const pending of this.__pendingDatagrams.splice(0)) pending.reject(webTransportInvalidState());
      for (const stream of this.__streams) stream.__closeInternal();
      this.__pendingIncomingBidirectional.length = 0;
      this.__pendingIncomingUnidirectional.length = 0;
      this.__streams.clear();
      this.__closedResolve(closeInfo);
      queueMicrotask(() => {
        const event = new Event("close");
        this.dispatchEvent(event);
        if (typeof this.__onclose === "function") this.__onclose.call(this, event);
      });
      if (this.__peer && !this.__peer.__closed) this.__peer.__closeFromPeer(closeInfo);
      this.__peer = null;
    }
    __closeFromPeer(closeInfo) {
      if (this.__closed) return;
      this.__closed = true;
      this.__state = "closed";
      this.__closeInfo = new WebTransportCloseInfo(closeInfo);
      this.__readyReject(webTransportInvalidState("The peer WebTransport was closed before this transport became ready."));
      this.__datagramReadable.__webTransportClose();
      this.__incomingBidirectional.__webTransportClose();
      this.__incomingUnidirectional.__webTransportClose();
      this.__datagramReadable._queue.length = 0;
      this.__incomingBidirectional._queue.length = 0;
      this.__incomingUnidirectional._queue.length = 0;
      if (this.datagrams && this.datagrams.writable && !this.datagrams.writable._closed) {
        this.datagrams.writable._close();
      }
      for (const pending of this.__pendingDatagrams.splice(0)) pending.reject(webTransportInvalidState());
      for (const stream of this.__streams) stream.__closeInternal();
      this.__pendingIncomingBidirectional.length = 0;
      this.__pendingIncomingUnidirectional.length = 0;
      this.__streams.clear();
      this.__closedResolve(this.__closeInfo);
      queueMicrotask(() => {
        const event = new Event("close");
        this.dispatchEvent(event);
        if (typeof this.__onclose === "function") this.__onclose.call(this, event);
      });
      this.__peer = null;
    }
    get maxDatagramSize() { return WEBTRANSPORT_MAX_DATAGRAM_SIZE; }
    get onclose() { return this.__onclose; }
    set onclose(callback) { this.__onclose = typeof callback === "function" ? callback : null; }
    get onstatechange() { return this.__onstatechange; }
    set onstatechange(callback) { this.__onstatechange = typeof callback === "function" ? callback : null; }
    get onerror() { return this.__onerror; }
    set onerror(callback) { this.__onerror = typeof callback === "function" ? callback : null; }
    get [Symbol.toStringTag]() { return "WebTransport"; }
    static createPair(leftURL = "https://omoikane.invalid/transport", rightURL = leftURL, leftOptions = {}, rightOptions = {}) {
      const left = new WebTransport(leftURL, leftOptions);
      const right = new WebTransport(rightURL, rightOptions);
      left.__connect(right); right.__connect(left);
      const pair = [left, right];
      pair.left = left; pair.right = right; pair.local = left; pair.remote = right; pair.peers = pair;
      return pair;
    }
    static createDeterministicPair(leftURL, rightURL, leftOptions, rightOptions) {
      return WebTransport.createPair(leftURL, rightURL, leftOptions, rightOptions);
    }
  }

  globalThis.WebTransportError = WebTransportError;
  globalThis.WebTransportCloseInfo = WebTransportCloseInfo;
  globalThis.WebTransportDatagrams = WebTransportDatagrams;
  globalThis.WebTransportBidirectionalStream = WebTransportBidirectionalStream;
  globalThis.WebTransportReceiveStream = WebTransportReceiveStream;
  globalThis.WebTransportSendStream = WebTransportSendStream;
  globalThis.WebTransport = WebTransport;
  globalThis.__omoikane_create_webtransport_pair = function(leftURL, rightURL, leftOptions, rightOptions) {
    return WebTransport.createPair(leftURL, rightURL, leftOptions, rightOptions);
  };
  globalThis.__omoikane_connect_webtransport_peers = function(left, right) {
    if (!(left instanceof WebTransport) || !(right instanceof WebTransport)) throw new TypeError("WebTransport peers are required");
    left.__connect(right); right.__connect(left);
    return [left, right];
  };
  WebTransport.__createPair = WebTransport.createPair;
  WebTransport.__createPeerPair = WebTransport.createPair;
  WebTransport.connectPeers = globalThis.__omoikane_connect_webtransport_peers;

  // Dedicated workers execute in a separate Boa realm. Passing a JsValue
  // object directly between those realms would retain the sender's
  // prototypes, so worker messages use this context-independent wire format.
  // The graph table preserves cycles, shared references, and the structured
  // clone built-ins supported above; the destination reconstructs every object
  // with its own realm's constructors and prototypes.
  function encodeWorkerNumber(value) {
    if (Number.isNaN(value)) return "NaN";
    if (value === Infinity) return "Infinity";
    if (value === -Infinity) return "-Infinity";
    if (Object.is(value, -0)) return "-0";
    return String(value);
  }

  function decodeWorkerNumber(value) {
    if (value === "NaN") return NaN;
    if (value === "Infinity") return Infinity;
    if (value === "-Infinity") return -Infinity;
    if (value === "-0") return -0;
    return Number(value);
  }

  function encodeWorkerMessage(value, options = undefined) {
    if (Array.isArray(options)) throw dataCloneError("Transfer lists are not supported yet.");
    const cloned = globalThis.structuredClone(value, options);
    const nodes = [];
    const memory = new Map();
    const visit = item => {
      if (item === undefined) return ["u"];
      if (item === null) return ["z"];
      switch (typeof item) {
        case "boolean": return ["b", item];
        case "number": return ["n", encodeWorkerNumber(item)];
        case "string": return ["s", item];
        case "bigint": return ["i", String(item)];
        default: break;
      }
      const known = memory.get(item);
      if (known !== undefined) return ["r", known];
      const id = nodes.length;
      memory.set(item, id);
      nodes.push(null);
      if (item instanceof Date) {
        nodes[id] = ["d", encodeWorkerNumber(item.getTime())];
      } else if (item instanceof RegExp) {
        nodes[id] = ["x", item.source, item.flags];
      } else if (item instanceof ArrayBuffer) {
        nodes[id] = ["q", Array.from(new Uint8Array(item))];
      } else if (ArrayBuffer.isView(item)) {
        const name = item instanceof DataView ? "DataView" : item.constructor.name;
        nodes[id] = ["v", name, visit(item.buffer), item.byteOffset,
          item instanceof DataView ? item.byteLength : item.length];
      } else if (item instanceof Map) {
        nodes[id] = ["m", Array.from(item, pair => [visit(pair[0]), visit(pair[1])])];
      } else if (item instanceof Set) {
        nodes[id] = ["t", Array.from(item, entry => visit(entry))];
      } else if (Array.isArray(item)) {
        const entries = [];
        for (let index = 0; index < item.length; index++) {
          if (Object.prototype.hasOwnProperty.call(item, index)) entries.push([index, visit(item[index])]);
        }
        nodes[id] = ["a", item.length, entries];
      } else {
        const prototype = Object.getPrototypeOf(item);
        if (prototype !== Object.prototype && prototype !== null) throw dataCloneError();
        if (Object.getOwnPropertySymbols(item).length) throw dataCloneError();
        nodes[id] = ["o", prototype === null ? 0 : 1,
          Object.keys(item).map(key => [key, visit(item[key])])];
      }
      return ["r", id];
    };
    return JSON.stringify({ version: 1, root: visit(cloned), nodes });
  }

  function decodeWorkerMessage(wire) {
    const encoded = JSON.parse(String(wire));
    if (!encoded || encoded.version !== 1 || !Array.isArray(encoded.nodes)) throw dataCloneError();
    const nodes = encoded.nodes;
    const objects = new Array(nodes.length);
    const resolvePrimitive = token => {
      if (!Array.isArray(token)) throw dataCloneError();
      switch (token[0]) {
        case "u": return undefined;
        case "z": return null;
        case "b": return !!token[1];
        case "n": return decodeWorkerNumber(token[1]);
        case "s": return String(token[1]);
        case "i": return BigInt(token[1]);
        case "r": {
          const object = objects[token[1]];
          if (object === undefined) throw dataCloneError();
          return object;
        }
        default: throw dataCloneError();
      }
    };
    for (let index = 0; index < nodes.length; index++) {
      const node = nodes[index];
      if (!Array.isArray(node)) throw dataCloneError();
      switch (node[0]) {
        case "d": objects[index] = new Date(decodeWorkerNumber(node[1])); break;
        case "x": objects[index] = new RegExp(String(node[1]), String(node[2])); break;
        case "q": objects[index] = new Uint8Array(node[1]).buffer; break;
        case "m": objects[index] = new Map(); break;
        case "t": objects[index] = new Set(); break;
        case "a": objects[index] = new Array(node[1]); break;
        case "o": objects[index] = Object.create(node[1] === 0 ? null : Object.prototype); break;
        case "v": break;
        default: throw dataCloneError();
      }
    }
    const typedArrayConstructors = {
      Int8Array, Uint8Array, Uint8ClampedArray, Int16Array, Uint16Array,
      Int32Array, Uint32Array, Float32Array, Float64Array, BigInt64Array,
      BigUint64Array,
    };
    for (let index = 0; index < nodes.length; index++) {
      const node = nodes[index];
      if (node[0] !== "v") continue;
      const buffer = resolvePrimitive(node[2]);
      if (!(buffer instanceof ArrayBuffer)) throw dataCloneError();
      if (node[1] === "DataView") {
        objects[index] = new DataView(buffer, node[3], node[4]);
      } else {
        const Constructor = typedArrayConstructors[node[1]];
        if (!Constructor) throw dataCloneError();
        objects[index] = new Constructor(buffer, node[3], node[4]);
      }
    }
    for (let index = 0; index < nodes.length; index++) {
      const node = nodes[index];
      const target = objects[index];
      switch (node[0]) {
        case "m":
          for (const pair of node[1]) target.set(resolvePrimitive(pair[0]), resolvePrimitive(pair[1]));
          break;
        case "t":
          for (const entry of node[1]) target.add(resolvePrimitive(entry));
          break;
        case "a":
          for (const pair of node[2]) target[pair[0]] = resolvePrimitive(pair[1]);
          break;
        case "o":
          for (const pair of node[2]) Object.defineProperty(target, pair[0], {
            value: resolvePrimitive(pair[1]), enumerable: true, writable: true, configurable: true,
          });
          break;
        default: break;
      }
    }
    return resolvePrimitive(encoded.root);
  }

  globalThis.__omoikane_encode_worker_message = encodeWorkerMessage;
  globalThis.__omoikane_decode_worker_message = decodeWorkerMessage;

  const messagePortConstructionToken = {};
  class MessagePort extends EventTarget {
    constructor(token) {
      super();
      if (token !== messagePortConstructionToken) throw new TypeError("Illegal constructor");
      this._entangled = null;
      this._started = false;
      this._closed = false;
      this._pendingMessages = [];
      this._onmessage = null;
      this._onmessageerror = null;
    }
    postMessage(message, options = undefined) {
      const data = globalThis.structuredClone(message, options);
      const destination = this._entangled;
      if (this._closed || !destination || destination._closed) return;
      destination._queueMessage(data);
    }
    start() {
      if (this._closed || this._started) return;
      this._started = true;
      for (const data of this._pendingMessages.splice(0)) {
        __omoikane_enqueue_posted_message(this, data);
      }
    }
    close() {
      this._closed = true;
      this._pendingMessages.length = 0;
    }
    _queueMessage(data) {
      if (this._closed) return;
      if (!this._started) { this._pendingMessages.push(data); return; }
      __omoikane_enqueue_posted_message(this, data);
    }
    _acceptMessage(data) {
      if (this._closed) return;
      this.dispatchEvent(new MessageEvent("message", {
        data,
        origin: "",
        source: null,
        ports: [],
      }));
    }
    get onmessage() { return this._onmessage; }
    set onmessage(callback) {
      if (this._onmessage) this.removeEventListener("message", this._onmessage);
      this._onmessage = typeof callback === "function" ? callback : null;
      if (this._onmessage) {
        this.addEventListener("message", this._onmessage);
        this.start();
      }
    }
    get onmessageerror() { return this._onmessageerror; }
    set onmessageerror(callback) {
      if (this._onmessageerror) this.removeEventListener("messageerror", this._onmessageerror);
      this._onmessageerror = typeof callback === "function" ? callback : null;
      if (this._onmessageerror) this.addEventListener("messageerror", this._onmessageerror);
    }
  }

  class MessageChannel {
    constructor() {
      this.port1 = new MessagePort(messagePortConstructionToken);
      this.port2 = new MessagePort(messagePortConstructionToken);
      this.port1._entangled = this.port2;
      this.port2._entangled = this.port1;
    }
  }
  globalThis.MessagePort = MessagePort;
  globalThis.MessageChannel = MessageChannel;

  // SharedWorker ports are message endpoints whose other side lives in a
  // dedicated shared-worker runtime.  The native bridge carries only the
  // context-independent structured-clone wire; this realm owns the endpoint
  // object and its started/queued state.
  const createSharedWorkerPort = (() => {
    // Keep the concrete endpoint private.  SharedWorker callers obtain it via
    // `worker.port` / connect events, while its MessagePort superclass still
    // provides the expected `instanceof MessagePort` behavior.
    const sharedWorkerPortConstructionToken = {};
    class SharedWorkerPort extends MessagePort {
      constructor(token, id) {
        if (token !== sharedWorkerPortConstructionToken) throw new TypeError("Illegal constructor");
        super(messagePortConstructionToken);
        this._id = String(id);
        this._started = false;
        this._closed = false;
        this._pendingMessages = [];
        this._onmessage = null;
        this._onmessageerror = null;
      }
      postMessage(message, options = undefined) {
        if (this._closed) throw new DOMException("The SharedWorker port is closed.", "InvalidStateError");
        const wire = __omoikane_encode_worker_message(message, options);
        __omoikane_shared_worker_port_post(this._id, wire);
      }
      start() {
        if (this._closed || this._started) return;
        this._started = true;
        for (const data of this._pendingMessages.splice(0)) {
          __omoikane_enqueue_posted_message(this, data);
        }
      }
      close() {
        if (this._closed) return;
        this._closed = true;
        this._pendingMessages.length = 0;
        __omoikane_shared_worker_port_close(this._id);
        if (typeof globalThis.__omoikane_remove_shared_worker_port === "function") {
          globalThis.__omoikane_remove_shared_worker_port(this._id, this);
        }
      }
      _queueMessage(data) {
        if (this._closed) return;
        if (!this._started) { this._pendingMessages.push(data); return; }
        __omoikane_enqueue_posted_message(this, data);
      }
      _acceptMessage(data) {
        if (this._closed) return;
        this.dispatchEvent(new MessageEvent("message", {
          data,
          origin: "",
          source: null,
          ports: [],
        }));
      }
      get onmessage() { return this._onmessage; }
      set onmessage(callback) {
        if (this._onmessage) this.removeEventListener("message", this._onmessage);
        this._onmessage = typeof callback === "function" ? callback : null;
        if (this._onmessage) {
          this.addEventListener("message", this._onmessage);
          this.start();
        }
      }
      get onmessageerror() { return this._onmessageerror; }
      set onmessageerror(callback) {
        if (this._onmessageerror) this.removeEventListener("messageerror", this._onmessageerror);
        this._onmessageerror = typeof callback === "function" ? callback : null;
        if (this._onmessageerror) this.addEventListener("messageerror", this._onmessageerror);
      }
      get [Symbol.toStringTag]() { return "MessagePort"; }
    }
    return function createSharedWorkerPort(id) {
      return new SharedWorkerPort(sharedWorkerPortConstructionToken, id);
    };
  })();
  globalThis.MessagePort = MessagePort;

  class SharedWorker extends EventTarget {
    constructor(url, options = undefined) {
      super();
      if (arguments.length < 1) throw new TypeError("SharedWorker requires a script URL");
      const requested = String(url);
      let name = "";
      if (typeof options === "string") name = options;
      else if (options && options.name !== undefined) name = String(options.name);
      if (options && options.type !== undefined && String(options.type) !== "classic") {
        throw new TypeError("Only classic SharedWorkers are supported");
      }
      this.url = requested;
      this.name = name;
      this.__closed = false;
      this.__id = String(__omoikane_shared_worker_connect(requested, name));
      this.port = createSharedWorkerPort(this.__id);
      __omoikane_shared_worker_bind_port(this.__id, this.port, this);
      this.onerror = null;
    }
    get [Symbol.toStringTag]() { return "SharedWorker"; }
    set onerror(callback) {
      if (this.__onerror) this.removeEventListener("error", this.__onerror);
      this.__onerror = typeof callback === "function" ? callback : null;
      if (this.__onerror) this.addEventListener("error", this.__onerror);
    }
    get onerror() { return this.__onerror || null; }
  }
  globalThis.SharedWorker = SharedWorker;

  // BroadcastChannel endpoints are registered natively by origin/name.  The
  // native side only queues a context-independent structured-clone wire; the
  // target runtime decodes it when its posted-message task runs, preserving
  // realm-local prototypes and asynchronous task ordering.
  class BroadcastChannel extends EventTarget {
    constructor(name) {
      super();
      if (arguments.length < 1) throw new TypeError("BroadcastChannel requires a name");
      this._name = String(name);
      this._closed = false;
      this._onmessage = null;
      this._onmessageerror = null;
      // Native delivery retains only this weak endpoint reference when the
      // realm supports WeakRef, so an unreferenced channel can be collected
      // without waiting for close(). Older Boa builds without WeakRef retain
      // the endpoint directly as a strong compatibility fallback.
      const endpoint = typeof WeakRef === "function" ? new WeakRef(this) : this;
      this._id = __omoikane_broadcast_channel_register(this._name, endpoint);
    }
    get name() { return this._name; }
    postMessage(message, options = undefined) {
      if (this._closed) throw new DOMException("The BroadcastChannel is closed.", "InvalidStateError");
      // Encoding performs the structured-clone operation synchronously, so
      // functions, symbols, DOM objects, and unsupported transfer lists throw
      // DataCloneError before any recipient is queued.
      const wire = __omoikane_encode_worker_message(message, options);
      __omoikane_broadcast_channel_post(this._id, wire);
    }
    close() {
      if (this._closed) return;
      this._closed = true;
      __omoikane_broadcast_channel_close(this._id);
    }
    get onmessage() { return this._onmessage; }
    set onmessage(callback) {
      if (this._onmessage) this.removeEventListener("message", this._onmessage);
      this._onmessage = typeof callback === "function" ? callback : null;
      if (this._onmessage) this.addEventListener("message", this._onmessage);
    }
    get onmessageerror() { return this._onmessageerror; }
    set onmessageerror(callback) {
      if (this._onmessageerror) this.removeEventListener("messageerror", this._onmessageerror);
      this._onmessageerror = typeof callback === "function" ? callback : null;
      if (this._onmessageerror) this.addEventListener("messageerror", this._onmessageerror);
    }
    get [Symbol.toStringTag]() { return "BroadcastChannel"; }
  }
  globalThis.BroadcastChannel = BroadcastChannel;

  // -------------------------------------------------------------------------
  // Dedicated Worker / WorkerGlobalScope core.
  //
  // The native side allocates a separate JsRuntime for each classic worker.
  // This wrapper owns only the page-facing endpoint; message payloads are
  // cloned before they enter either runtime and are delivered by the native
  // posted-message task source.
  // -------------------------------------------------------------------------
  class Worker extends EventTarget {
    constructor(url, options = undefined) {
      super();
      if (options && options.type !== undefined && String(options.type) !== "classic") {
        throw new TypeError("Only classic Dedicated Workers are supported");
      }
      const requested = String(url);
      this.url = requested;
      this.onmessage = null;
      this.onerror = null;
      this.__terminated = false;
      this.__id = __omoikane_create_worker(requested);
      __omoikane_bind_worker_owner(this.__id, this);
    }
    postMessage(message, options = undefined) {
      if (this.__terminated) return;
      const data = __omoikane_encode_worker_message(message, options);
      __omoikane_worker_post_message(this.__id, data);
    }
    terminate() {
      if (this.__terminated) return;
      this.__terminated = true;
      __omoikane_terminate_worker(this.__id);
    }
    get [Symbol.toStringTag]() { return "Worker"; }
    set onmessage(callback) {
      if (this.__onmessage) this.removeEventListener("message", this.__onmessage);
      this.__onmessage = typeof callback === "function" ? callback : null;
      if (this.__onmessage) this.addEventListener("message", this.__onmessage);
    }
    get onmessage() { return this.__onmessage || null; }
    set onerror(callback) {
      if (this.__onerror) this.removeEventListener("error", this.__onerror);
      this.__onerror = typeof callback === "function" ? callback : null;
      if (this.__onerror) this.addEventListener("error", this.__onerror);
    }
    get onerror() { return this.__onerror || null; }
  }
  globalThis.Worker = Worker;

  globalThis.__omoikane_install_worker_global = function(url, workerId) {
    // A worker global is not a Window and cannot reach the page DOM. The
    // bootstrap has already installed shared language primitives (Event,
    // MessageEvent, structuredClone, timers, URL, and navigator); remove the
    // browsing-context objects before user script evaluates.
    try { delete globalThis.document; } catch (_) { globalThis.document = undefined; }
    try { delete globalThis.window; } catch (_) { globalThis.window = undefined; }
    try { delete globalThis.customElements; } catch (_) { globalThis.customElements = undefined; }
    for (const domName of [
      "Node", "Element", "HTMLElement", "Document", "DocumentFragment", "Text",
      "CharacterData", "Attr", "ShadowRoot", "HTMLCollection", "NodeList", "Range",
      "MutationObserver", "ResizeObserver", "IntersectionObserver", "CustomElementRegistry",
      "HTMLDivElement", "HTMLSpanElement", "HTMLBodyElement", "HTMLCanvasElement",
      "HTMLImageElement", "HTMLIFrameElement", "HTMLScriptElement", "SVGElement",
      "SVGSVGElement", "HTMLTemplateElement", "HTMLFormElement", "HTMLInputElement",
      "HTMLTextAreaElement", "HTMLButtonElement", "HTMLSelectElement", "HTMLOptionElement",
      "HTMLMediaElement", "HTMLAudioElement", "HTMLVideoElement", "Audio",
      "MediaError", "AudioContext", "AudioNode", "AudioParam", "AudioDestinationNode",
      "GainNode", "OscillatorNode",
      "Geolocation", "GeolocationCoordinates", "GeolocationPosition",
      "GeolocationPositionError",
      "Notification",
    ]) {
      try { delete globalThis[domName]; } catch (_) { globalThis[domName] = undefined; }
    }
    try { delete globalThis.getComputedStyle; } catch (_) { globalThis.getComputedStyle = undefined; }
    try { delete globalThis.history; } catch (_) { globalThis.history = undefined; }
    // Async Clipboard is a Window-only surface in this runtime. Keep the
    // worker navigator object, but do not expose a page clipboard handle from
    // a DedicatedWorkerGlobalScope.
    try { if (globalThis.navigator) delete globalThis.navigator.clipboard; } catch (_) {}
    try { if (globalThis.navigator) delete globalThis.navigator.geolocation; } catch (_) {}
    try { delete globalThis.Clipboard; } catch (_) { globalThis.Clipboard = undefined; }
    try { delete globalThis.__omoikane_dispatch_notification_click; } catch (_) {}
    for (const name of ["Geolocation", "GeolocationCoordinates", "GeolocationPosition", "GeolocationPositionError"]) {
      try { delete globalThis[name]; } catch (_) { globalThis[name] = undefined; }
    }
    try { delete globalThis.__omoikane_install_window_named_properties; } catch (_) {}
    Object.defineProperty(globalThis, "isSecureContext", {
      configurable: true,
      enumerable: true,
      get() { return nativeIsSecureContext(); },
    });
    Object.defineProperty(globalThis, "_listeners", {
      configurable: true,
      value: new Map(),
    });
    globalThis.self = globalThis;
    globalThis.WorkerGlobalScope = Object;
    globalThis.addEventListener = EventTarget.prototype.addEventListener;
    globalThis.removeEventListener = EventTarget.prototype.removeEventListener;
    globalThis.dispatchEvent = EventTarget.prototype.dispatchEvent;
    const workerLocation = Object.freeze({
      href: String(url),
      origin: (() => { try { return new URL(String(url)).origin; } catch (_) { return ""; } })(),
      protocol: (() => { try { return new URL(String(url)).protocol; } catch (_) { return ""; } })(),
      host: (() => { try { return new URL(String(url)).host; } catch (_) { return ""; } })(),
      pathname: (() => { try { return new URL(String(url)).pathname; } catch (_) { return ""; } })(),
      search: (() => { try { return new URL(String(url)).search; } catch (_) { return ""; } })(),
      hash: (() => { try { return new URL(String(url)).hash; } catch (_) { return ""; } })(),
      toString() { return this.href; },
    });
    try {
      Object.defineProperty(globalThis, "location", {
        configurable: true,
        writable: true,
        value: workerLocation,
      });
    } catch (_) {
      // Older Boa versions expose location as a non-configurable accessor;
      // retaining the existing object is still preferable to exposing a
      // mutable Window location in a worker global.
    }
    let workerOnMessage = null;
    Object.defineProperty(globalThis, "onmessage", {
      configurable: true,
      get() { return workerOnMessage; },
      set(callback) {
        if (workerOnMessage) EventTarget.prototype.removeEventListener.call(globalThis, "message", workerOnMessage);
        workerOnMessage = typeof callback === "function" ? callback : null;
        if (workerOnMessage) EventTarget.prototype.addEventListener.call(globalThis, "message", workerOnMessage);
      },
    });
    globalThis.postMessage = function(message, options = undefined) {
      const data = __omoikane_encode_worker_message(message, options);
      __omoikane_worker_owner_post_message(workerId, data);
    };
    globalThis.close = function() {
      __omoikane_worker_close();
    };
  };

  globalThis.__omoikane_install_shared_worker_global = function(url, sharedWorkerId) {
    // Reuse the dedicated-worker global sanitisation and then install the
    // SharedWorkerGlobalScope-specific connect/port surface.
    globalThis.__omoikane_install_worker_global(url, sharedWorkerId);
    try { delete globalThis.Worker; } catch (_) { globalThis.Worker = undefined; }
    try { delete globalThis.SharedWorker; } catch (_) { globalThis.SharedWorker = undefined; }
    try { delete globalThis.SharedWorkerPort; } catch (_) { globalThis.SharedWorkerPort = undefined; }
    globalThis.SharedWorkerGlobalScope = Object;
    const ports = new Map();
    globalThis.__omoikane_get_shared_worker_port = function(connectionId) {
      const id = String(connectionId);
      let port = ports.get(id);
      if (!port) {
        port = createSharedWorkerPort(id);
        ports.set(id, port);
      }
      return port;
    };
    globalThis.__omoikane_remove_shared_worker_port = function(connectionId, port) {
      const id = String(connectionId);
      if (ports.get(id) === port) ports.delete(id);
    };
    let workerOnConnect = null;
    Object.defineProperty(globalThis, "onconnect", {
      configurable: true,
      get() { return workerOnConnect; },
      set(callback) {
        if (workerOnConnect) EventTarget.prototype.removeEventListener.call(globalThis, "connect", workerOnConnect);
        workerOnConnect = typeof callback === "function" ? callback : null;
        if (workerOnConnect) EventTarget.prototype.addEventListener.call(globalThis, "connect", workerOnConnect);
      },
    });
    globalThis.__omoikane_dispatch_shared_worker_connect = function(connectionId) {
      const port = globalThis.__omoikane_get_shared_worker_port(connectionId);
      globalThis.dispatchEvent(new MessageEvent("connect", { ports: [port] }));
    };
    // SharedWorkerGlobalScope communicates through the ports in connect
    // events; a global postMessage call is therefore intentionally inert.
    globalThis.postMessage = function() {
      throw new DOMException("SharedWorkerGlobalScope has no owner window.", "InvalidStateError");
    };
  };

  // -------------------------------------------------------------------------
  // Worklet / WorkletGlobalScope core.
  //
  // A Worklet owns a separate, same-origin Boa runtime.  `addModule()` is
  // promise-shaped at the page boundary, while the native side evaluates the
  // fetched module in FIFO order and drains that realm's microtasks before
  // resolving the page promise.  The global deliberately exposes no Window or
  // DOM objects; paint registration is retained as metadata for the
  // deterministic CSS.paintWorklet seam rather than invoking a renderer.
  // -------------------------------------------------------------------------
  const workletConstructionToken = {};
  class WorkletGlobalScope {
    constructor() { throw new TypeError("Illegal constructor"); }
    get [Symbol.toStringTag]() { return "WorkletGlobalScope"; }
  }

  class Worklet {
    constructor(token) {
      if (token !== workletConstructionToken) throw new TypeError("Illegal constructor");
      const id = nativeCreateWorklet();
      Object.defineProperty(this, "__id", {
        configurable: false,
        enumerable: false,
        writable: false,
        value: String(id),
      });
      Object.defineProperty(this, "__terminated", {
        configurable: false,
        enumerable: false,
        writable: true,
        value: false,
      });
    }
    addModule(url, options = undefined) {
      if (this.__terminated) {
        return Promise.reject(new DOMException("Worklet has been torn down.", "InvalidStateError"));
      }
      if (arguments.length < 1) return Promise.reject(new TypeError("Worklet.addModule requires a module URL"));
      if (options !== undefined && (options === null || typeof options !== "object")) {
        return Promise.reject(new TypeError("Worklet.addModule options must be an object"));
      }
      if (options && options.credentials !== undefined &&
          !["omit", "same-origin", "include"].includes(String(options.credentials))) {
        return Promise.reject(new TypeError("Unsupported Worklet credentials mode"));
      }
      let status;
      try {
        status = JSON.parse(nativeWorkletAddModule(this.__id, String(url)));
      } catch (error) {
        return Promise.reject(error);
      }
      return Promise.resolve().then(() => {
        if (!status || status.ok !== true) {
          const name = status && status.name ? String(status.name) : "OperationError";
          const message = status && status.message ? String(status.message) : "Worklet module failed";
          throw new DOMException(message, name);
        }
        return undefined;
      });
    }
    get registeredNames() {
      try {
        const names = JSON.parse(nativeWorkletRegisteredNames(this.__id));
        return Object.freeze(Array.isArray(names) ? names.map(String) : []);
      } catch (_) {
        return Object.freeze([]);
      }
    }
    get moduleCount() {
      try { return Number(nativeWorkletModuleCount(this.__id)); }
      catch (_) { return 0; }
    }
    teardown() {
      if (this.__terminated) return Promise.resolve();
      let result = false;
      try { result = !!nativeWorkletTeardown(this.__id); } catch (_) {}
      this.__terminated = true;
      // Teardown is deliberately promise-shaped so callers can order it after
      // the final addModule() checkpoint and observe it deterministically.
      return Promise.resolve().then(() => undefined);
    }
    terminate() { return this.teardown(); }
    get [Symbol.toStringTag]() { return "Worklet"; }
  }
  globalThis.Worklet = Worklet;
  globalThis.WorkletGlobalScope = WorkletGlobalScope;

  // CSS.paintWorklet is a stable Worklet instance.  Keep this assignment near
  // the Worklet definition because the CSS namespace itself is created much
  // earlier in the bootstrap.
  if (globalThis.CSS && typeof globalThis.CSS === "object") {
    Object.defineProperty(globalThis.CSS, "paintWorklet", {
      configurable: true,
      enumerable: true,
      writable: false,
      value: new Worklet(workletConstructionToken),
    });
  }

  globalThis.__omoikane_install_worklet_global = function(url, workletId) {
    // Worklets share language primitives with workers but are not message
    // endpoints. Reusing the sanitizer guarantees `document` and `window` are
    // absent while preserving timers, URL, structuredClone and microtasks.
    globalThis.__omoikane_install_worker_global(url, workletId);
    try { delete globalThis.Worker; } catch (_) { globalThis.Worker = undefined; }
    try { delete globalThis.SharedWorker; } catch (_) { globalThis.SharedWorker = undefined; }
    try { delete globalThis.SharedWorkerGlobalScope; } catch (_) {}
    try { delete globalThis.Worklet; } catch (_) { globalThis.Worklet = undefined; }
    try { delete globalThis.WorkletGlobalScope; } catch (_) {}
    try { delete globalThis.CSS; } catch (_) { globalThis.CSS = undefined; }
    try { delete globalThis.onmessage; } catch (_) {}
    try { delete globalThis.postMessage; } catch (_) {}
    try { delete globalThis.close; } catch (_) {}
    const nativeRegister = nativeWorkletRegister;
    const registrations = new Map();
    const scope = class WorkletGlobalScope {};
    globalThis.WorkletGlobalScope = scope;
    globalThis.__omoikane_worklet_registrations = registrations;
    globalThis.registerPaint = function(name, paintCtor, inputProperties = [], inputArguments = []) {
      const key = String(name);
      if (!key) throw new DOMException("A paint worklet name is required.", "SyntaxError");
      if (typeof paintCtor !== "function") throw new TypeError("Paint definition must be callable");
      if (registrations.has(key)) throw new DOMException("Paint name already registered.", "InvalidModificationError");
      if (!Array.isArray(inputProperties) || !Array.isArray(inputArguments)) {
        throw new TypeError("Paint input lists must be arrays");
      }
      if (typeof nativeRegister !== "function" || !nativeRegister(workletId, key)) {
        throw new DOMException("WorkletGlobalScope has been torn down.", "InvalidStateError");
      }
      registrations.set(key, Object.freeze({
        name: key,
        constructor: paintCtor,
        inputProperties: Object.freeze(inputProperties.map(String)),
        inputArguments: Object.freeze(inputArguments.map(String)),
      }));
    };
    // Generic registration is useful to deterministic Worklet consumers that
    // do not need the paint-specific constructor contract.
    globalThis.registerWorklet = function(name, value = undefined) {
      const key = String(name);
      if (!key) throw new DOMException("A worklet name is required.", "SyntaxError");
      if (registrations.has(key)) throw new DOMException("Worklet name already registered.", "InvalidModificationError");
      if (typeof nativeRegister !== "function" || !nativeRegister(workletId, key)) {
        throw new DOMException("WorkletGlobalScope has been torn down.", "InvalidStateError");
      }
      registrations.set(key, Object.freeze({ name: key, value }));
    };
  };

  const mediaQueryListRefs = [];

  class MediaQueryListEvent extends Event {
    constructor(type, init = {}) {
      init = init ?? {};
      super(type, {
        bubbles: init.bubbles ?? false,
        cancelable: init.cancelable ?? false,
      });
      this.matches = !!init.matches;
      this.media = String(init.media ?? "");
    }
  }

  class MediaQueryList extends EventTarget {
    constructor(query) {
      super();
      this.media = String(query);
      this.onchange = null;
      this._matches = __omoikane_match_media(this.media);
    }
    get matches() { return this._matches; }
    addListener(callback) { this.addEventListener("change", callback); }
    removeListener(callback) { this.removeEventListener("change", callback); }
    __reevaluate() {
      const matches = __omoikane_match_media(this.media);
      if (matches === this._matches) return;
      this._matches = matches;
      const event = new MediaQueryListEvent("change", { matches, media: this.media });
      this.dispatchEvent(event);
      if (typeof this.onchange === "function") this.onchange.call(this, event);
    }
  }

  globalThis.matchMedia = function(query) {
    const list = new MediaQueryList(query);
    mediaQueryListRefs.push(typeof WeakRef === "function" ? new WeakRef(list) : list);
    return list;
  };
  globalThis.__omoikane_media_query_viewport_changed = function() {
    for (let index = mediaQueryListRefs.length - 1; index >= 0; index--) {
      const entry = mediaQueryListRefs[index];
      const list = typeof WeakRef === "function" ? entry.deref() : entry;
      if (list) list.__reevaluate();
      else mediaQueryListRefs.splice(index, 1);
    }
  };
  globalThis.MediaQueryList = MediaQueryList;
  globalThis.MediaQueryListEvent = MediaQueryListEvent;

  // ---------------------------------------------------------------------------
  // File API data primitives.
  //
  // `Blob` owns a snapshot of its bytes as a `Uint8Array`, which makes it the
  // single binary representation the rest of the platform builds on: `File`,
  // `FileReader`, object URLs, `FormData` file entries and fetch bodies all read
  // through it. Because the bytes are already in memory, everything here is
  // synchronous internally; only the spec'd entry points return promises or
  // dispatch events.
  // ---------------------------------------------------------------------------

  const blobTextEncoder = new TextEncoder();
  const blobTextDecoder = new TextDecoder();

  const BLOB_BASE64_ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  const BLOB_BASE64_VALUES = (() => {
    const table = new Uint8Array(128);
    for (let index = 0; index < BLOB_BASE64_ALPHABET.length; index++) {
      table[BLOB_BASE64_ALPHABET.charCodeAt(index)] = index;
    }
    return table;
  })();

  function base64FromBytes(bytes) {
    const output = [];
    for (let index = 0; index < bytes.length; index += 3) {
      const remaining = bytes.length - index;
      const bits = (bytes[index] << 16) |
        ((remaining > 1 ? bytes[index + 1] : 0) << 8) |
        (remaining > 2 ? bytes[index + 2] : 0);
      output.push(
        BLOB_BASE64_ALPHABET[(bits >> 18) & 63],
        BLOB_BASE64_ALPHABET[(bits >> 12) & 63],
        remaining > 1 ? BLOB_BASE64_ALPHABET[(bits >> 6) & 63] : "=",
        remaining > 2 ? BLOB_BASE64_ALPHABET[bits & 63] : "=",
      );
    }
    return output.join("");
  }

  // Decodes into a preallocated buffer rather than through `atob`, so a binary
  // response body never builds an intermediate string.
  function bytesFromBase64(text) {
    const source = String(text).replace(/[^A-Za-z0-9+/]/g, "");
    const bytes = new Uint8Array((source.length * 3) >> 2);
    let buffer = 0;
    let bits = 0;
    let offset = 0;
    for (let index = 0; index < source.length; index++) {
      buffer = (buffer << 6) | BLOB_BASE64_VALUES[source.charCodeAt(index)];
      bits += 6;
      if (bits >= 8) {
        bits -= 8;
        bytes[offset++] = (buffer >> bits) & 255;
      }
    }
    return offset === bytes.length ? bytes : bytes.slice(0, offset);
  }

  // A blob's `type` is kept only when every character is printable ASCII
  // (U+0020..U+007E), and is then ASCII-lowercased. This is not MIME
  // validation: "foo bar" survives unchanged, while a tab or a non-ASCII
  // character drops the type entirely.
  function normalizeBlobType(value) {
    const text = String(value);
    for (let index = 0; index < text.length; index++) {
      const code = text.charCodeAt(index);
      if (code < 0x20 || code > 0x7e) return "";
    }
    return text.toLowerCase();
  }

  // `sequence<BlobPart>` accepts any iterable object. A bare string is iterable
  // but is not an object, so `new Blob("abc")` is a TypeError rather than three
  // one-character parts.
  function blobPartSequence(parts) {
    if (parts === undefined) return [];
    if (parts === null || typeof parts !== "object" || typeof parts[Symbol.iterator] !== "function") {
      throw new TypeError("Blob parts must be given as a sequence");
    }
    return Array.from(parts);
  }

  // Flattens blob parts into one buffer. Strings are UTF-8 encoded (so a lone
  // surrogate becomes U+FFFD), buffer sources are copied so later writes to the
  // caller's view cannot change the blob, and nested blobs contribute their
  // bytes without their type.
  //
  // A nested blob's buffer is adopted rather than copied. Nothing in the engine
  // writes to a blob's bytes after construction, and no public API hands the
  // buffer out (`arrayBuffer()` and `bytes()` both copy), so the bytes behave as
  // the immutable snapshot the File API defines while `new Blob([blob])` and a
  // `Blob` request body stay allocation-free.
  function blobPartsToBytes(parts) {
    const chunks = [];
    let length = 0;
    for (const part of parts) {
      let chunk;
      if (part instanceof Blob) chunk = part.__bytes;
      else if (part instanceof ArrayBuffer) chunk = new Uint8Array(part).slice();
      else if (ArrayBuffer.isView(part)) {
        chunk = new Uint8Array(part.buffer, part.byteOffset, part.byteLength).slice();
      } else chunk = blobTextEncoder.encode(String(part));
      chunks.push(chunk);
      length += chunk.length;
    }
    if (chunks.length === 1) return chunks[0];
    const bytes = new Uint8Array(length);
    let offset = 0;
    for (const chunk of chunks) {
      bytes.set(chunk, offset);
      offset += chunk.length;
    }
    return bytes;
  }

  // `Blob.slice()` takes `[Clamp] long long` offsets: a fractional value rounds
  // to the nearest integer with ties to even, so `slice(1.5)` and `slice(2.5)`
  // both start at byte 2.
  function clampToInteger(value) {
    const number = Number(value);
    if (Number.isNaN(number)) return 0;
    if (!Number.isFinite(number)) return number > 0 ? Number.MAX_SAFE_INTEGER : -Number.MAX_SAFE_INTEGER;
    const floor = Math.floor(number);
    const fraction = number - floor;
    if (fraction > 0.5) return floor + 1;
    if (fraction < 0.5) return floor;
    return floor % 2 === 0 ? floor : floor + 1;
  }

  function relativeBlobOffset(offset, size) {
    return offset < 0 ? Math.max(size + offset, 0) : Math.min(offset, size);
  }

  class Blob {
    constructor(parts = undefined, options = undefined) {
      const bytes = blobPartsToBytes(blobPartSequence(parts));
      // Non-enumerable and non-writable: the backing store is not part of a
      // blob's observable shape, and nothing rebinds it after construction.
      Object.defineProperty(this, "__bytes", { value: bytes });
      Object.defineProperty(this, "__type", {
        value: normalizeBlobType((options ?? {}).type ?? ""),
      });
    }
    get size() { return this.__bytes.length; }
    get type() { return this.__type; }
    get [Symbol.toStringTag]() { return "Blob"; }
    slice(start = undefined, end = undefined, contentType = undefined) {
      const size = this.__bytes.length;
      const from = relativeBlobOffset(start === undefined ? 0 : clampToInteger(start), size);
      const to = relativeBlobOffset(end === undefined ? size : clampToInteger(end), size);
      const span = Math.max(to - from, 0);
      return new Blob([this.__bytes.subarray(from, from + span)], {
        type: contentType === undefined ? "" : contentType,
      });
    }
    text() { return Promise.resolve(this.__text()); }
    arrayBuffer() { return Promise.resolve(this.__bytes.slice().buffer); }
    bytes() { return Promise.resolve(this.__bytes.slice()); }
    stream() { return readableByteStream(this.__bytes); }
    // Synchronous UTF-8 decode used by the platform internals that already hold
    // the bytes (`XMLHttpRequest.responseText`, fetch body text).
    __text() { return blobTextDecoder.decode(this.__bytes); }
  }

  class File extends Blob {
    constructor(parts, name, options = undefined) {
      if (arguments.length < 2) throw new TypeError("File requires fileBits and fileName");
      super(parts, options);
      const lastModified = (options ?? {}).lastModified;
      Object.defineProperty(this, "__name", { value: String(name) });
      Object.defineProperty(this, "__lastModified", {
        value: lastModified === undefined ? Date.now() : Math.trunc(Number(lastModified)) || 0,
      });
    }
    // The file name is not sanitized: a path-like name is preserved verbatim.
    get name() { return this.__name; }
    get lastModified() { return this.__lastModified; }
    get webkitRelativePath() { return ""; }
    get [Symbol.toStringTag]() { return "File"; }
  }

  // Read-only, index-accessible list of files. The list itself is immutable from
  // script, while DataTransfer keeps one live instance and refreshes its indexed
  // view when items are added or removed.
  class FileList {
    constructor(files = []) {
      Object.defineProperty(this, "__files", { value: [], configurable: false });
      this.__replace(files);
    }
    __replace(files) {
      for (let index = 0; index < this.__files.length; index++) delete this[index];
      this.__files.splice(0, this.__files.length, ...Array.from(files));
      for (let index = 0; index < this.__files.length; index++) {
        Object.defineProperty(this, index, {
          configurable: true, enumerable: true, value: this.__files[index], writable: false,
        });
      }
    }
    get length() { return this.__files.length; }
    item(index) {
      const number = Number(index);
      if (!Number.isFinite(number) || number < 0) return null;
      return this.__files[Math.trunc(number)] ?? null;
    }
    [Symbol.iterator]() { return this.__files[Symbol.iterator](); }
    get [Symbol.toStringTag]() { return "FileList"; }
  }

  class DataTransferItem {
    constructor(kind, type, value) {
      this.__kind = kind;
      this.__type = type;
      this.__value = value;
    }
    get kind() { return this.__kind; }
    get type() { return this.__type; }
    getAsFile() { return this.__kind === "file" ? this.__value : null; }
    getAsString(callback) {
      if (typeof callback !== "function") throw new TypeError("getAsString callback must be callable");
      if (this.__kind !== "string") {
        queueMicrotask(() => callback(null));
        return;
      }
      const value = this.__value;
      queueMicrotask(() => callback(value));
    }
    webkitGetAsEntry() { return null; }
    get [Symbol.toStringTag]() { return "DataTransferItem"; }
  }

  class DataTransferItemList {
    constructor(owner) {
      this.__owner = owner;
      this.__items = [];
    }
    __syncIndices() {
      for (const key of Object.keys(this)) {
        if (/^\d+$/.test(key)) delete this[key];
      }
      for (let index = 0; index < this.__items.length; index++) {
        Object.defineProperty(this, index, {
          configurable: true, enumerable: true, get: () => this.__items[index],
        });
      }
    }
    get length() { return this.__items.length; }
    item(index) {
      const number = Number(index);
      if (!Number.isFinite(number) || number < 0) return null;
      return this.__items[Math.trunc(number)] ?? null;
    }
    add(data, type = undefined) {
      let item;
      if (data instanceof File) {
        if (type !== undefined) throw new TypeError("A File item does not accept a type argument");
        item = new DataTransferItem("file", data.type, data);
      } else {
        if (type === undefined) throw new TypeError("DataTransferItemList.add requires a File or string type");
        const mime = String(type).toLowerCase();
        if (!mime) throw new DOMException("The item type must not be empty.", "InvalidStateError");
        if (this.__items.some(existing => existing.kind === "string" && existing.type === mime)) {
          throw new DOMException("An item of this type already exists.", "NotSupportedError");
        }
        item = new DataTransferItem("string", mime, String(data));
      }
      this.__items.push(item);
      this.__syncIndices();
      this.__owner.__syncFiles();
      return item;
    }
    remove(index) {
      const number = Number(index);
      const position = Math.trunc(number);
      if (!Number.isFinite(number) || position < 0 || position >= this.__items.length) {
        throw new DOMException("The item index is out of range.", "IndexSizeError");
      }
      this.__items.splice(position, 1);
      this.__syncIndices();
      this.__owner.__syncFiles();
    }
    clear() {
      this.__items.length = 0;
      this.__syncIndices();
      this.__owner.__syncFiles();
    }
    [Symbol.iterator]() { return this.__items[Symbol.iterator](); }
    get [Symbol.toStringTag]() { return "DataTransferItemList"; }
  }

  class DataTransfer {
    constructor() {
      this.dropEffect = "none";
      this.effectAllowed = "none";
      this.__files = new FileList();
      this.items = new DataTransferItemList(this);
    }
    __syncFiles() {
      this.__files.__replace(this.items.__items
        .filter(item => item.kind === "file")
        .map(item => item.getAsFile()));
    }
    get files() { return this.__files; }
    get types() {
      const types = [];
      for (const item of this.items) {
        if (item.kind === "string" && !types.includes(item.type)) types.push(item.type);
      }
      if (this.__files.length > 0) types.push("Files");
      return types;
    }
    setData(format, data) {
      const type = String(format).toLowerCase();
      const existing = this.items.__items.find(item => item.kind === "string" && item.type === type);
      if (existing) existing.__value = String(data);
      else this.items.add(String(data), type);
    }
    getData(format) {
      const type = String(format).toLowerCase();
      return this.items.__items.find(item => item.kind === "string" && item.type === type)?.__value || "";
    }
    clearData(format = undefined) {
      if (format === undefined) {
        this.items.__items = this.items.__items.filter(item => item.kind !== "string");
      } else {
        const type = String(format).toLowerCase();
        this.items.__items = this.items.__items.filter(item => !(item.kind === "string" && item.type === type));
      }
      this.items.__syncIndices();
      this.__syncFiles();
    }
    setDragImage(_element, _x, _y) {}
    get [Symbol.toStringTag]() { return "DataTransfer"; }
  }

  class ProgressEvent extends Event {
    constructor(type, init = {}) {
      init = init ?? {};
      super(type, init);
      this.lengthComputable = !!init.lengthComputable;
      this.loaded = Number(init.loaded) || 0;
      this.total = Number(init.total) || 0;
    }
    get [Symbol.toStringTag]() { return "ProgressEvent"; }
  }

  const FILE_READER_EMPTY = 0;
  const FILE_READER_LOADING = 1;
  const FILE_READER_DONE = 2;

  // The File API falls back to UTF-8 when the encoding label is absent or
  // unsupported. Omoikane's TextDecoder implements UTF-8 only, so every other
  // label takes the fallback path.
  function decodeBlobText(bytes, encoding) {
    if (encoding !== undefined && encoding !== null && String(encoding) !== "") {
      try { return new TextDecoder(String(encoding)).decode(bytes); }
      catch (_) { /* unsupported label: fall back to UTF-8 */ }
    }
    return blobTextDecoder.decode(bytes);
  }

  function fileReaderResult(blob, kind, encoding) {
    if (kind === "arraybuffer") return blob.__bytes.slice().buffer;
    if (kind === "dataurl") {
      // A blob with no type is exposed as application/octet-stream.
      return "data:" + (blob.type || "application/octet-stream") + ";base64," +
        base64FromBytes(blob.__bytes);
    }
    if (kind === "binarystring") {
      let result = "";
      for (const byte of blob.__bytes) result += String.fromCharCode(byte);
      return result;
    }
    return decodeBlobText(blob.__bytes, encoding);
  }

  class FileReader extends EventTarget {
    constructor() {
      super();
      this.readyState = FILE_READER_EMPTY;
      this.result = null;
      this.error = null;
      this.onloadstart = null;
      this.onprogress = null;
      this.onload = null;
      this.onabort = null;
      this.onerror = null;
      this.onloadend = null;
      // Bumped by every read and by `abort()`, so a superseded or aborted read
      // stops dispatching as soon as control returns to it.
      this.__readId = 0;
    }
    readAsText(blob, encoding = undefined) { this.__read(blob, "text", encoding); }
    readAsArrayBuffer(blob) { this.__read(blob, "arraybuffer"); }
    readAsDataURL(blob) { this.__read(blob, "dataurl"); }
    readAsBinaryString(blob) { this.__read(blob, "binarystring"); }
    abort() {
      if (this.readyState !== FILE_READER_LOADING) {
        this.result = null;
        return;
      }
      this.__readId++;
      this.readyState = FILE_READER_DONE;
      this.result = null;
      this.error = new DOMException("The read operation was aborted", "AbortError");
      this.__fire("abort", 0, 0);
      this.__fire("loadend", 0, 0);
    }
    __read(blob, kind, encoding = undefined) {
      if (!(blob instanceof Blob)) throw new TypeError("FileReader requires a Blob");
      if (this.readyState === FILE_READER_LOADING) {
        throw new DOMException("A read is already in progress", "InvalidStateError");
      }
      this.readyState = FILE_READER_LOADING;
      this.result = null;
      this.error = null;
      const readId = ++this.__readId;
      const total = blob.size;
      // The bytes are already in memory, but the File API delivers results from
      // the file reading task source: a script that starts a read never sees its
      // result before yielding to the event loop.
      __omoikane_queue_file_reading_task(() => {
        // An `abort()` or a superseding read cancels whatever this one has not
        // dispatched yet.
        if (readId !== this.__readId) return;
        this.__fire("loadstart", 0, total);
        if (readId !== this.__readId) return;
        this.__fire("progress", total, total);
        if (readId !== this.__readId) return;
        this.result = fileReaderResult(blob, kind, encoding);
        this.readyState = FILE_READER_DONE;
        // `load` and `loadend` are the same read operation, so a read started
        // from the `load` handler must not swallow this read's `loadend`.
        this.__fire("load", total, total);
        this.__fire("loadend", total, total);
      });
    }
    __fire(type, loaded, total) {
      const event = new ProgressEvent(type, { lengthComputable: true, loaded, total });
      const handler = this["on" + type];
      if (typeof handler === "function") handler.call(this, event);
      this.dispatchEvent(event);
    }
  }
  FileReader.EMPTY = FILE_READER_EMPTY;
  FileReader.LOADING = FILE_READER_LOADING;
  FileReader.DONE = FILE_READER_DONE;
  FileReader.prototype.EMPTY = FILE_READER_EMPTY;
  FileReader.prototype.LOADING = FILE_READER_LOADING;
  FileReader.prototype.DONE = FILE_READER_DONE;

  // Object URLs are minted here and mirrored into the host store. The map keeps
  // the `Blob` itself so `fetch()` and `XMLHttpRequest` read the original bytes;
  // the host copy exists for loads that run after script has finished, such as
  // `<img src>` and CSS `url(...)`, which layout resolves synchronously.
  const objectUrls = new Map();

  function isBlobUrl(url) {
    return /^blob:/i.test(String(url));
  }

  function createObjectURL(object) {
    if (!(object instanceof Blob)) {
      throw new TypeError("createObjectURL requires a Blob");
    }
    const origin = (globalThis.location && globalThis.location.origin) || "null";
    const url = "blob:" + origin + "/" + crypto.randomUUID();
    objectUrls.set(url, object);
    __omoikane_register_object_url(url, object.__bytes, object.type);
    return url;
  }

  function revokeObjectURL(url) {
    const key = String(url);
    objectUrls.delete(key);
    __omoikane_revoke_object_url(key);
  }

  globalThis.Blob = Blob;
  globalThis.File = File;
  globalThis.FileList = FileList;
  globalThis.DataTransfer = DataTransfer;
  globalThis.DataTransferItem = DataTransferItem;
  globalThis.DataTransferItemList = DataTransferItemList;
  globalThis.FileReader = FileReader;
  globalThis.ProgressEvent = ProgressEvent;
  globalThis.URL.createObjectURL = createObjectURL;
  globalThis.URL.revokeObjectURL = revokeObjectURL;

  function fireRealtimeEvent(target, event) {
    const handler = target["on" + event.type];
    if (typeof handler === "function") handler.call(target, event);
    target.dispatchEvent(event);
  }

  function realtimeOrigin(url) {
    const match = String(url).match(/^([a-z][a-z0-9+.-]*:\/\/[^/?#]+)/i);
    return match ? match[1] : "";
  }

  class CloseEvent extends Event {
    constructor(type, init = {}) {
      super(type, init);
      this.wasClean = !!init.wasClean;
      this.code = Number(init.code) || 0;
      this.reason = String(init.reason || "");
    }
  }

  class WebSocket extends EventTarget {
    constructor(url, protocols = []) {
      super();
      this.url = String(url);
      const list = protocols === undefined ? [] : (Array.isArray(protocols) ? protocols.map(String) : [String(protocols)]);
      if (new Set(list).size !== list.length) throw new DOMException("Duplicate subprotocol", "SyntaxError");
      this.readyState = WebSocket.CONNECTING;
      this.bufferedAmount = 0;
      this.extensions = "";
      this.protocol = "";
      this.binaryType = "blob";
      this.onopen = this.onmessage = this.onerror = this.onclose = null;
      try {
        const result = JSON.parse(__omoikane_websocket_connect(this.url, JSON.stringify(list)));
        this.__id = result.id;
        this.protocol = result.protocol;
        __omoikane_queue_networking_task(() => {
          if (this.readyState !== WebSocket.CONNECTING) return;
          this.readyState = WebSocket.OPEN;
          fireRealtimeEvent(this, new Event("open"));
          this.__pollTimer = setInterval(() => this.__poll(), 1);
        });
      } catch (error) {
        __omoikane_queue_networking_task(() => {
          this.readyState = WebSocket.CLOSED;
          fireRealtimeEvent(this, new Event("error"));
          fireRealtimeEvent(this, new CloseEvent("close", { code: 1006, wasClean: false }));
        });
      }
    }
    send(data) {
      if (this.readyState === WebSocket.CONNECTING) throw new DOMException("WebSocket is connecting", "InvalidStateError");
      if (this.readyState !== WebSocket.OPEN) return;
      let bytes, binary = false;
      if (typeof data === "string") bytes = new TextEncoder().encode(data);
      else if (data instanceof ArrayBuffer) { bytes = new Uint8Array(data); binary = true; }
      else if (ArrayBuffer.isView(data)) { bytes = new Uint8Array(data.buffer, data.byteOffset, data.byteLength); binary = true; }
      else if (data instanceof Blob) { bytes = data.__bytes; binary = true; }
      else bytes = new TextEncoder().encode(String(data));
      try {
        __omoikane_websocket_send(this.__id, base64FromBytes(bytes), binary);
      } catch (_) {
        __omoikane_queue_networking_task(() => fireRealtimeEvent(this, new Event("error")));
      }
    }
    close(code = 1000, reason = "") {
      code = Number(code);
      reason = String(reason);
      if (code !== 1000 && (code < 3000 || code > 4999)) throw new DOMException("Invalid close code", "InvalidAccessError");
      if (new TextEncoder().encode(reason).length > 123) throw new DOMException("Close reason is too long", "SyntaxError");
      if (this.readyState >= WebSocket.CLOSING) return;
      this.readyState = WebSocket.CLOSING;
      if (this.__pollTimer !== undefined) clearInterval(this.__pollTimer);
      try { __omoikane_websocket_close(this.__id, code, reason); } catch (_) {}
      __omoikane_queue_networking_task(() => {
        this.readyState = WebSocket.CLOSED;
        fireRealtimeEvent(this, new CloseEvent("close", { code, reason, wasClean: true }));
      });
    }
    __poll() {
      let results;
      try { results = JSON.parse(__omoikane_websocket_poll(this.__id)); }
      catch (_) { results = [{ kind: "error" }]; }
      for (const result of results) {
        __omoikane_queue_networking_task(() => this.__deliver(result));
      }
    }
    __deliver(result) {
      if (result.kind === "error") {
        fireRealtimeEvent(this, new Event("error"));
        return;
      }
      if (result.kind === "close") {
        if (this.__pollTimer !== undefined) clearInterval(this.__pollTimer);
        this.readyState = WebSocket.CLOSED;
        fireRealtimeEvent(this, new CloseEvent("close", { code: result.code, reason: result.reason, wasClean: true }));
        return;
      }
      let data = result.data;
      if (result.kind === "binary") {
        const bytes = bytesFromBase64(data);
        data = this.binaryType === "arraybuffer" ? bytes.buffer : new Blob([bytes]);
      }
      fireRealtimeEvent(this, new MessageEvent("message", { data, origin: realtimeOrigin(this.url) }));
    }
  }
  WebSocket.CONNECTING = WebSocket.prototype.CONNECTING = 0;
  WebSocket.OPEN = WebSocket.prototype.OPEN = 1;
  WebSocket.CLOSING = WebSocket.prototype.CLOSING = 2;
  WebSocket.CLOSED = WebSocket.prototype.CLOSED = 3;

  class EventSource extends EventTarget {
    constructor(url, init = {}) {
      super();
      this.url = String(url);
      this.withCredentials = !!(init && init.withCredentials);
      this.readyState = EventSource.CONNECTING;
      this.onopen = this.onmessage = this.onerror = null;
      this.__closed = false;
      this.__lastEventId = "";
      this.__retry = 3000;
      this.__connect();
    }
    close() { this.__closed = true; this.readyState = EventSource.CLOSED; }
    __connect() {
      if (this.__closed) return;
      try {
        const text = __omoikane_event_source_fetch(this.url, this.__lastEventId, this.withCredentials);
        __omoikane_queue_networking_task(() => {
          if (this.__closed) return;
          this.readyState = EventSource.OPEN;
          fireRealtimeEvent(this, new Event("open"));
          let data = [], type = "", retry = null;
          const dispatch = () => {
            if (!data.length) return;
            fireRealtimeEvent(this, new MessageEvent(type || "message", { data: data.join("\n"), origin: realtimeOrigin(this.url), lastEventId: this.__lastEventId }));
            data = []; type = "";
          };
          for (const line of text.replace(/\r\n|\r/g, "\n").split("\n")) {
            if (line === "") { dispatch(); continue; }
            if (line[0] === ":") continue;
            const colon = line.indexOf(":");
            const field = colon < 0 ? line : line.slice(0, colon);
            let value = colon < 0 ? "" : line.slice(colon + 1);
            if (value[0] === " ") value = value.slice(1);
            if (field === "data") data.push(value);
            else if (field === "event") type = value;
            else if (field === "id" && !value.includes("\0")) this.__lastEventId = value;
            else if (field === "retry" && /^\d+$/.test(value)) retry = Number(value);
          }
          dispatch();
          if (retry !== null) this.__retry = retry;
          if (!this.__closed) { this.readyState = EventSource.CONNECTING; setTimeout(() => this.__connect(), this.__retry); }
        });
      } catch (_) {
        __omoikane_queue_networking_task(() => {
          if (this.__closed) return;
          this.readyState = EventSource.CONNECTING;
          fireRealtimeEvent(this, new Event("error"));
          setTimeout(() => this.__connect(), this.__retry);
        });
      }
    }
  }
  EventSource.CONNECTING = EventSource.prototype.CONNECTING = 0;
  EventSource.OPEN = EventSource.prototype.OPEN = 1;
  EventSource.CLOSED = EventSource.prototype.CLOSED = 2;
  globalThis.WebSocket = WebSocket;
  globalThis.EventSource = EventSource;
  globalThis.CloseEvent = CloseEvent;

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
  // Fetch's "extract a body", reduced to the body types Omoikane models.
  //
  // A body is kept in the form it arrived in: as text for strings and network
  // responses (whose UTF-8 decoding is exactly what `text()` is defined to
  // return), and as bytes for blobs, buffer sources and file-bearing form data.
  // Keeping both forms available means `text()` never re-decodes a text body,
  // while `blob()` and `arrayBuffer()` never lose bytes.
  // A body record is shared by every request or response cloned from the one
  // that extracted it, so it is frozen: nothing downstream may repoint it at
  // different text or bytes. The bytes themselves are a blob's buffer, which the
  // engine never writes to after construction.
  const EMPTY_BODY = Object.freeze({ text: null, bytes: null, contentType: null });

  function bodyRecord(text, bytes, contentType) {
    return Object.freeze({ text, bytes, contentType });
  }

  function extractBody(source) {
    if (source === null || source === undefined) return EMPTY_BODY;
    if (source instanceof ReadableStream) {
      if (source.locked || source._disturbed) throw new TypeError("ReadableStream body is unusable");
      if (source._errorSet) throw source._error;
      // The host keeps Fetch bodies as immutable snapshots.  A stream whose
      // producer has not closed yet cannot be synchronously extracted without
      // dropping future chunks, so reject it instead of silently truncating it.
      if (!source._closed) throw new TypeError("ReadableStream body is not ready");
      const chunks = source._queue.splice(0);
      source._markDisturbed();
      return bodyRecord(null, blobPartsToBytes(chunks), null);
    }
    if (source instanceof Blob) {
      return bodyRecord(null, source.__bytes, source.type || null);
    }
    if (source instanceof ArrayBuffer || ArrayBuffer.isView(source)) {
      return bodyRecord(null, blobPartsToBytes([source]), null);
    }
    if (source instanceof FormData) {
      const encoded = source.__multipart();
      return typeof encoded.body === "string"
        ? bodyRecord(encoded.body, null, encoded.contentType)
        : bodyRecord(null, encoded.body, encoded.contentType);
    }
    if (globalThis.URLSearchParams !== undefined && source instanceof URLSearchParams) {
      return bodyRecord(source.toString(), null, "application/x-www-form-urlencoded;charset=UTF-8");
    }
    return bodyRecord(String(source), null, "text/plain;charset=UTF-8");
  }

  function bodyIsEmpty(body) {
    return body.text === null && body.bytes === null;
  }

  function bodyAsText(body) {
    if (body.text !== null) return body.text;
    return body.bytes === null ? "" : blobTextDecoder.decode(body.bytes);
  }

  function bodyAsBytes(body) {
    if (body.bytes !== null) return body.bytes;
    return blobTextEncoder.encode(body.text ?? "");
  }

  // The payload handed to the host fetch binding: a string for text bodies (the
  // common case, unchanged) and a `Uint8Array` when the bytes cannot be
  // represented as text.
  function bodyAsPayload(body) {
    if (bodyIsEmpty(body)) return null;
    return body.text !== null ? body.text : body.bytes;
  }

  // `Response.blob()` takes its type from the Content-Type header rather than
  // from the body it was built with, so an overridden header wins.
  function bodyAsBlob(body, headers) {
    return new Blob([bodyAsBytes(body)], { type: headers.get("content-type") ?? "" });
  }

  function bodyStreamFor(owner) {
    if (bodyIsEmpty(owner.__body)) return null;
    if (owner.__stream === null) {
      owner.__stream = readableByteStream(bodyAsBytes(owner.__body), () => {
        owner.__bodyUsed = true;
      });
      if (owner.bodyUsed) {
        owner.__stream._markDisturbed();
        owner.__stream._queue.length = 0;
        owner.__stream._cancelled = true;
      }
    }
    return owner.__stream;
  }

  function bodyIsDisturbed(owner) {
    return owner.bodyUsed || (owner.__stream !== null &&
      (owner.__stream._disturbed || owner.__stream.locked));
  }

  function disturbBody(owner) {
    owner.__bodyUsed = true;
    if (owner.__stream !== null) {
      owner.__stream._markDisturbed();
      owner.__stream._queue.length = 0;
      owner.__stream._cancelled = true;
      if (!owner.__stream._closed && !owner.__stream._errorSet) owner.__stream._controller.close();
    }
  }

  // The convenience methods consume the same body represented by `.body`.
  // Marking an exposed-but-not-yet-read stream as consumed prevents a later
  // reader from observing a second copy of the payload.
  function consumeBody(owner, callback) {
    if (bodyIsDisturbed(owner)) {
      return Promise.reject(new TypeError("Body is unusable"));
    }
    disturbBody(owner);
    return Promise.resolve().then(() => callback(owner.__body));
  }

  function formDataPercentDecode(value) {
    try {
      return decodeURIComponent(String(value).replace(/\+/g, " "));
    } catch (_) {
      throw new TypeError("Invalid application/x-www-form-urlencoded body");
    }
  }

  function formDataFromUrlEncoded(body) {
    const form = new FormData();
    const text = bodyAsText(body);
    if (text === "") return form;
    for (const pair of text.split("&")) {
      if (pair === "") continue;
      const separator = pair.indexOf("=");
      const key = separator < 0 ? pair : pair.slice(0, separator);
      const value = separator < 0 ? "" : pair.slice(separator + 1);
      form.append(formDataPercentDecode(key), formDataPercentDecode(value));
    }
    return form;
  }

  function multipartHeaderParameter(header, name) {
    const pattern = new RegExp("(?:^|;)\\s*" + name + "\\s*=\\s*(?:\\\"([^\\\"]*)\\\"|([^;\\s]*))", "i");
    const match = pattern.exec(header);
    return match ? (match[1] ?? match[2] ?? "") : null;
  }

  function byteSequenceIndexOf(bytes, needle, start = 0) {
    if (needle.length === 0) return Math.min(start, bytes.length);
    const limit = bytes.length - needle.length;
    for (let offset = Math.max(0, start); offset <= limit; offset++) {
      let match = true;
      for (let index = 0; index < needle.length; index++) {
        if (bytes[offset + index] !== needle[index]) {
          match = false;
          break;
        }
      }
      if (match) return offset;
    }
    return -1;
  }

  function byteSequenceAfter(bytes, offset, sequence) {
    if (offset < 0 || offset + sequence.length > bytes.length) return false;
    for (let index = 0; index < sequence.length; index++) {
      if (bytes[offset + index] !== sequence[index]) return false;
    }
    return true;
  }

  function formDataFromMultipart(body, boundary) {
    if (!boundary) throw new TypeError("Multipart boundary is missing");
    const bytes = bodyAsBytes(body);
    const delimiter = blobTextEncoder.encode("--" + boundary);
    const delimiterWithPrefix = new Uint8Array(delimiter.length + 2);
    delimiterWithPrefix.set([13, 10]);
    delimiterWithPrefix.set(delimiter, 2);
    const form = new FormData();
    let cursor = byteSequenceIndexOf(bytes, delimiter);
    if (cursor < 0) throw new TypeError("Malformed multipart body");
    while (cursor >= 0) {
      let after = cursor + delimiter.length;
      if (byteSequenceAfter(bytes, after, new Uint8Array([45, 45]))) break;
      if (!byteSequenceAfter(bytes, after, new Uint8Array([13, 10]))) {
        throw new TypeError("Malformed multipart boundary");
      }
      const headerStart = after + 2;
      const headerEnd = byteSequenceIndexOf(bytes, new Uint8Array([13, 10, 13, 10]), headerStart);
      if (headerEnd < 0) throw new TypeError("Malformed multipart headers");
      const headerText = blobTextDecoder.decode(bytes.slice(headerStart, headerEnd));
      const headers = new Map();
      for (const line of headerText.split("\r\n")) {
        const separator = line.indexOf(":");
        if (separator <= 0) throw new TypeError("Malformed multipart header");
        headers.set(line.slice(0, separator).trim().toLowerCase(), line.slice(separator + 1).trim());
      }
      const disposition = headers.get("content-disposition") || "";
      if (!/^form-data\s*(?:;|$)/i.test(disposition)) throw new TypeError("Invalid multipart disposition");
      const name = multipartHeaderParameter(disposition, "name");
      if (name === null) throw new TypeError("Multipart field name is missing");
      const bodyStart = headerEnd + 4;
      const bodyEnd = byteSequenceIndexOf(bytes, delimiterWithPrefix, bodyStart);
      if (bodyEnd < 0) throw new TypeError("Malformed multipart body");
      const partBytes = bytes.slice(bodyStart, bodyEnd);
      const filename = multipartHeaderParameter(disposition, "filename");
      if (filename !== null) {
        form.append(name, new File([partBytes], filename, {
          type: headers.get("content-type") || "application/octet-stream",
        }));
      } else {
        form.append(name, blobTextDecoder.decode(partBytes));
      }
      cursor = bodyEnd + 2;
    }
    return form;
  }

  function formDataFromBody(body, contentType) {
    const type = String(contentType || "").split(";", 1)[0].trim().toLowerCase();
    if (type === "application/x-www-form-urlencoded") return formDataFromUrlEncoded(body);
    if (type === "multipart/form-data") {
      const match = /(?:^|;)\s*boundary\s*=\s*(?:\"([^\"]*)\"|([^;\s]*))/i.exec(String(contentType));
      return formDataFromMultipart(body, match ? (match[1] ?? match[2] ?? "") : "");
    }
    throw new TypeError("Unsupported form data content type");
  }

  class Request {
    constructor(input, init = {}) {
      const source = input instanceof Request ? input : null;
      // A blob URL is already absolute; relative resolution only understands
      // http(s) and would have to round-trip it through two parsers to get the
      // same string back.
      this.url = source ? source.url
        : isBlobUrl(input) ? String(input)
        : resolveNetworkUrl(input);
      this.method = String(init.method || (source && source.method) || "GET").toUpperCase();
      this.headers = new Headers(init.headers || (source && source.headers));
      const body = init.body === undefined
        ? (source ? source.__body : EMPTY_BODY)
        : extractBody(init.body);
      if (source && init.body === undefined && bodyIsDisturbed(source)) {
        throw new TypeError("Cannot construct a Request from a used body");
      }
      this.__body = body;
      this.__stream = null;
      this.__bodyUsed = false;
      if (body.contentType !== null && !this.headers.has("content-type")) {
        this.headers.set("content-type", body.contentType);
      }
      this.credentials = init.credentials ?? (source && source.credentials) ?? "same-origin";
      this.mode = init.mode ?? (source && source.mode) ?? "cors";
      this.redirect = init.redirect ?? (source && source.redirect) ?? "follow";
      this.signal = init.signal ?? (source && source.signal) ?? null;
      if (!["omit", "same-origin", "include"].includes(this.credentials)) throw new TypeError("Invalid credentials mode");
      if (!["same-origin", "cors", "no-cors"].includes(this.mode)) throw new TypeError("Invalid request mode");
      if (!["follow", "error", "manual"].includes(this.redirect)) throw new TypeError("Invalid redirect mode");
      if (this.mode === "no-cors" && !["GET", "HEAD", "POST"].includes(this.method)) {
        throw new TypeError("Method is not allowed in no-cors mode");
      }
      if ((this.method === "GET" || this.method === "HEAD") && !bodyIsEmpty(body)) {
        throw new TypeError("Request with GET/HEAD method cannot have body");
      }
    }
    get bodyUsed() { return this.__bodyUsed; }
    get body() { return bodyStreamFor(this); }
    text() { return consumeBody(this, bodyAsText); }
    json() { return this.text().then(JSON.parse); }
    arrayBuffer() { return consumeBody(this, body => bodyAsBytes(body).slice().buffer); }
    blob() { return consumeBody(this, body => bodyAsBlob(body, this.headers)); }
    formData() { return consumeBody(this, body => formDataFromBody(body, this.headers.get("content-type"))); }
    clone() {
      if (bodyIsDisturbed(this)) throw new TypeError("Cannot clone a used body");
      return new Request(this);
    }
  }
  class Response {
    constructor(body = null, init = {}) {
      const extracted = extractBody(body);
      this.__body = extracted;
      this.status = init.status === undefined ? 200 : Number(init.status);
      this.statusText = init.statusText || "";
      this.headers = new Headers(init.headers);
      if (extracted.contentType !== null && !this.headers.has("content-type")) {
        this.headers.set("content-type", extracted.contentType);
      }
      this.url = init.url || "";
      this.type = "basic";
      this.redirected = Boolean(init.redirected);
      this.__stream = null;
      this.__bodyUsed = false;
    }
    get bodyUsed() { return this.__bodyUsed; }
    get ok() { return this.status >= 200 && this.status <= 299; }
    get body() { return bodyStreamFor(this); }
    text() { return consumeBody(this, bodyAsText); }
    json() { return this.text().then(JSON.parse); }
    arrayBuffer() { return consumeBody(this, body => bodyAsBytes(body).slice().buffer); }
    blob() { return consumeBody(this, body => bodyAsBlob(body, this.headers)); }
    formData() { return consumeBody(this, body => formDataFromBody(body, this.headers.get("content-type"))); }
    clone() {
      if (bodyIsDisturbed(this)) throw new TypeError("Cannot clone a used body");
      const response = new Response(null, { status: this.status, statusText: this.statusText, headers: this.headers, url: this.url, redirected: this.redirected });
      // A body is an immutable snapshot, so both responses can share it.
      response.__body = this.__body;
      response.type = this.type;
      return response;
    }
    static json(data, init = {}) {
      const headers = new Headers(init.headers);
      if (!headers.has("content-type")) headers.set("content-type", "application/json");
      return new Response(JSON.stringify(data), { ...init, headers });
    }
    static redirect(url, status = 302) { return new Response(null, { status, headers: { location: String(url) } }); }
    static error() { const response = new Response(null, { status: 0 }); response.type = "error"; return response; }
  }
  globalThis.Headers = Headers;
  globalThis.FormData = FormData;
  globalThis.Request = Request;
  globalThis.Response = Response;

  // Builds a Response from a host fetch payload. `bodyText` is the lossy UTF-8
  // decoding the text path is defined in terms of; `bodyBase64` is present only
  // when the payload was not valid UTF-8, and then carries the original bytes so
  // `blob()` and `arrayBuffer()` stay exact.
  function responseFromFetchPayload(data) {
    const response = new Response(null, {
      status: data.status,
      statusText: data.statusText,
      headers: data.headers,
      url: data.url,
      redirected: data.redirected,
    });
    // Both forms are retained: `text()` must not re-decode, and for a payload
    // that is not valid UTF-8 the lossy decoding is still the defined text
    // result while the bytes remain available to `blob()`/`arrayBuffer()`.
    response.__body = data.bodyPresent === false || data.type === "opaque" || data.type === "opaqueredirect"
      ? EMPTY_BODY
      : bodyRecord(
          data.bodyText,
          data.bodyBase64 == null ? null : bytesFromBase64(data.bodyBase64),
          null,
        );
    response.type = data.type;
    return response;
  }

  // Fetching a blob URL is a store lookup rather than a network request. Only
  // GET is defined for it, and an unknown or revoked URL is a network error.
  function blobUrlResponse(url, method) {
    const blob = objectUrls.get(String(url));
    if (blob === undefined || String(method).toUpperCase() !== "GET") return null;
    const headers = new Headers();
    headers.set("content-type", blob.type);
    headers.set("content-length", String(blob.size));
    return new Response(blob, { status: 200, statusText: "OK", headers, url: String(url) });
  }

  globalThis.fetch = function(input, init = {}) {
    const source = input instanceof Request ? input : null;
    const request = new Request(input, init);
    if (source && init.body === undefined && !bodyIsEmpty(source.__body)) disturbBody(source);
    if (request.signal && request.signal.aborted) return Promise.reject(request.signal.reason);
    const resourceTiming = beginResourceTiming(request.url, "fetch");
    if (isBlobUrl(request.url)) {
      return Promise.resolve().then(() => {
        if (request.signal && request.signal.aborted) throw request.signal.reason;
        const response = blobUrlResponse(request.url, request.method);
        if (response === null) throw new TypeError("Failed to fetch blob URL");
        return response;
      }).then(response => {
        finishResourceTiming(resourceTiming, {
          status: response.status,
          bodyText: response.__body && response.__body.text,
          url: response.url,
        });
        return response;
      }, error => {
        finishResourceTiming(resourceTiming, {}, true);
        throw error;
      });
    }
    return Promise.resolve().then(() => {
      if (request.signal && request.signal.aborted) throw request.signal.reason;
      return __omoikane_fetch(
        request.url,
        request.method,
        JSON.stringify([...request.headers]),
        bodyAsPayload(request.__body),
        request.mode,
        request.credentials,
        request.redirect,
      );
    }).then(raw => {
      if (request.signal && request.signal.aborted) throw request.signal.reason;
      const data = JSON.parse(String(raw));
      finishResourceTiming(resourceTiming, data, false);
      return responseFromFetchPayload(data);
    }).catch(error => {
      finishResourceTiming(resourceTiming, {}, true);
      throw error;
    });
  };

  // -------------------------------------------------------------------------
  // Cache Storage
  // -------------------------------------------------------------------------
  //
  // Cache and CacheStorage objects are realm-local wrappers around a native
  // origin-partitioned snapshot store.  A cache operation is deliberately
  // delivered through the networking task source: native-boundary operations
  // observe a pending Promise until the next event-loop turn, and Promise
  // reactions run at the normal checkpoint after that task.  Input validation
  // may reject before a task is queued.  Only JSON snapshots cross the native
  // boundary; Request/Response objects and their bodies never do.

  const CACHE_CONSTRUCTION_TOKEN = {};
  const CACHE_STORAGE_CONSTRUCTION_TOKEN = {};

  function cacheNative(operation, name = "", payload = "") {
    // Window realms use the same origin validation as Web Storage.  Keep this
    // inside the networking task callback (all callers reach this helper from
    // queueCacheTask) so opaque-origin failures reject the operation's Promise
    // with the standard SecurityError DOMException.  Dedicated workers do not
    // expose `document`; their host-side binding still applies the worker
    // origin partition and validation.
    if (typeof document !== "undefined" && document) {
      storageOrigin(document);
    }
    const raw = __omoikane_cache_storage(
      String(operation),
      String(name),
      typeof payload === "string" ? payload : JSON.stringify(payload),
    );
    return JSON.parse(String(raw));
  }

  function queueCacheTask(callback) {
    return new Promise((resolve, reject) => {
      try {
        __omoikane_queue_networking_task(() => {
          try {
            resolve(callback());
          } catch (error) {
            reject(error);
          }
        });
      } catch (error) {
        reject(error);
      }
    });
  }

  function cacheBodySnapshot(body) {
    return {
      text: body.text,
      bytes: body.bytes === null ? null : base64FromBytes(body.bytes),
      contentType: body.contentType,
    };
  }

  function cacheBodyFromSnapshot(snapshot) {
    if (!snapshot || typeof snapshot !== "object") return EMPTY_BODY;
    return bodyRecord(
      snapshot.text == null ? null : String(snapshot.text),
      snapshot.bytes == null ? null : bytesFromBase64(snapshot.bytes),
      snapshot.contentType == null ? null : String(snapshot.contentType),
    );
  }

  function cacheRequestSnapshot(request) {
    return {
      url: String(request.url),
      method: String(request.method).toUpperCase(),
      headers: Array.from(request.headers.entries()),
      credentials: request.credentials,
      mode: request.mode,
      redirect: request.redirect,
      body: cacheBodySnapshot(request.__body),
    };
  }

  function cacheResponseSnapshot(response) {
    return {
      status: Number(response.status),
      statusText: String(response.statusText),
      headers: Array.from(response.headers.entries()),
      url: String(response.url || ""),
      type: String(response.type || "basic"),
      redirected: Boolean(response.redirected),
      body: cacheBodySnapshot(response.__body),
    };
  }

  function cacheRestoreRequest(snapshot) {
    const request = new Request(snapshot.url, {
      method: snapshot.method,
      headers: snapshot.headers,
      credentials: snapshot.credentials,
      mode: snapshot.mode,
      redirect: snapshot.redirect,
    });
    // Cache entries are immutable snapshots.  Assigning this private record
    // avoids re-extracting (and potentially re-encoding) a binary body.
    request.__body = cacheBodyFromSnapshot(snapshot.body);
    request.__bodyUsed = false;
    return request;
  }

  function cacheRestoreResponse(snapshot) {
    const response = new Response(null, {
      status: snapshot.status,
      statusText: snapshot.statusText,
      headers: snapshot.headers,
      url: snapshot.url,
      redirected: snapshot.redirected,
    });
    response.__body = cacheBodyFromSnapshot(snapshot.body);
    response.type = snapshot.type || "basic";
    response.__bodyUsed = false;
    return response;
  }

  function cacheRequestForInput(input, init = undefined) {
    if (input instanceof Request) {
      return init === undefined ? input : new Request(input, init);
    }
    return new Request(input, init || {});
  }

  function cacheURLKey(url, ignoreSearch) {
    try {
      const parsed = new URL(String(url));
      return parsed.protocol + "//" + parsed.host + parsed.pathname +
        (ignoreSearch ? "" : parsed.search);
    } catch (_) {
      const withoutFragment = String(url).split("#", 1)[0];
      return ignoreSearch ? withoutFragment.split("?", 1)[0] : withoutFragment;
    }
  }

  function cacheHeaderValue(headers, name) {
    const found = headers.find(entry => String(entry[0]).toLowerCase() === name);
    return found === undefined ? null : String(found[1]);
  }

  function cacheEntryMatches(entry, request, options = {}) {
    let storedRequest;
    let storedResponse;
    try {
      storedRequest = JSON.parse(String(entry.request));
      storedResponse = JSON.parse(String(entry.response));
    } catch (_) {
      return false;
    }
    const ignoreMethod = Boolean(options && options.ignoreMethod);
    const ignoreSearch = Boolean(options && options.ignoreSearch);
    if (!ignoreMethod && String(storedRequest.method).toUpperCase() !== String(request.method).toUpperCase()) {
      return false;
    }
    if (cacheURLKey(storedRequest.url, ignoreSearch) !== cacheURLKey(request.url, ignoreSearch)) {
      return false;
    }
    if (Boolean(options && options.ignoreVary)) return true;

    const vary = cacheHeaderValue(storedResponse.headers || [], "vary");
    if (vary === null) return true;
    for (const field of vary.split(",")) {
      const name = field.trim().toLowerCase();
      if (!name) continue;
      if (name === "*") return false;
      const storedValue = cacheHeaderValue(storedRequest.headers || [], name);
      const currentValue = request.headers.get(name);
      if (storedValue !== currentValue) return false;
    }
    return true;
  }

  function cacheMatchingEntries(entries, request, options = {}) {
    return entries.filter(entry => cacheEntryMatches(entry, request, options));
  }

  class Cache {
    constructor(name, token) {
      if (token !== CACHE_CONSTRUCTION_TOKEN) {
        throw new TypeError("Cache objects cannot be constructed directly");
      }
      this._name = String(name);
    }

    get [Symbol.toStringTag]() { return "Cache"; }

    match(request, options = {}) {
      let normalized;
      try {
        normalized = cacheRequestForInput(request);
      } catch (error) {
        return Promise.reject(error);
      }
      return queueCacheTask(() => {
        const entries = cacheNative("entries", this._name);
        const match = cacheMatchingEntries(entries, normalized, options)[0];
        if (!match) return undefined;
        return cacheRestoreResponse(JSON.parse(String(match.response)));
      });
    }

    matchAll(request = undefined, options = {}) {
      let normalized = null;
      try {
        if (request !== undefined) normalized = cacheRequestForInput(request);
      } catch (error) {
        return Promise.reject(error);
      }
      return queueCacheTask(() => {
        const entries = cacheNative("entries", this._name);
        const matches = normalized === null
          ? entries
          : cacheMatchingEntries(entries, normalized, options);
        return matches.map(entry => cacheRestoreResponse(JSON.parse(String(entry.response))));
      });
    }

    put(request, response) {
      let normalizedRequest;
      let requestSnapshot;
      let responseSnapshot;
      try {
        normalizedRequest = cacheRequestForInput(request);
        if (!(response instanceof Response)) {
          throw new TypeError("Cache.put requires a Response");
        }
        if (normalizedRequest.method !== "GET") {
          throw new TypeError("Cache.put only supports GET requests");
        }
        if (["opaque", "opaqueredirect", "error"].includes(String(response.type))) {
          throw new TypeError("Cache.put cannot store an opaque response");
        }
        if (Number(response.status) === 0 || Number(response.status) === 206) {
          throw new TypeError("Cache.put cannot store a partial response");
        }
        const vary = response.headers.get("vary");
        if (vary !== null && vary.split(",").some(field => field.trim() === "*")) {
          throw new TypeError("Cache.put cannot store a Vary: * response");
        }
        requestSnapshot = cacheRequestSnapshot(normalizedRequest);
        responseSnapshot = cacheResponseSnapshot(response);
      } catch (error) {
        return Promise.reject(error);
      }
      return queueCacheTask(() => {
        cacheNative("put", this._name, {
          request: JSON.stringify(requestSnapshot),
          response: JSON.stringify(responseSnapshot),
        });
        return undefined;
      });
    }

    add(request) {
      let normalized;
      try {
        normalized = cacheRequestForInput(request);
        if (normalized.method !== "GET") {
          throw new TypeError("Cache.add only supports GET requests");
        }
      } catch (error) {
        return Promise.reject(error);
      }
      // Fetch errors and non-successful responses are surfaced as Promise
      // rejections, matching Cache.add rather than silently caching failures.
      return fetch(normalized.clone()).then(response => {
        if (!response.ok || ["opaque", "opaqueredirect", "error"].includes(String(response.type))) {
          throw new TypeError("Cache.add received a non-successful response");
        }
        return this.put(normalized, response);
      });
    }

    addAll(requests) {
      let values;
      try {
        values = Array.from(requests, request => cacheRequestForInput(request));
        for (const request of values) {
          if (request.method !== "GET") throw new TypeError("Cache.addAll only supports GET requests");
        }
      } catch (error) {
        return Promise.reject(error);
      }
      // Fetch the complete batch before mutating the cache.  In particular, a
      // later network failure must not leave an earlier request half-installed.
      return Promise.all(values.map(request => fetch(request.clone()).then(response => {
        if (!response.ok || ["opaque", "opaqueredirect", "error"].includes(String(response.type))) {
          throw new TypeError("Cache.addAll received a non-successful response");
        }
        return response;
      }))).then(responses => {
        let result = Promise.resolve();
        responses.forEach((response, index) => {
          result = result.then(() => this.put(values[index], response));
        });
        return result.then(() => undefined);
      });
    }

    delete(request, options = {}) {
      let normalized;
      try {
        normalized = cacheRequestForInput(request);
      } catch (error) {
        return Promise.reject(error);
      }
      return queueCacheTask(() => {
        const entries = cacheNative("entries", this._name);
        const matches = cacheMatchingEntries(entries, normalized, options);
        let deleted = false;
        for (const entry of matches) {
          deleted = cacheNative("delete-entry", this._name, String(entry.id)) || deleted;
        }
        return deleted;
      });
    }

    keys(request = undefined, options = {}) {
      let normalized = null;
      try {
        if (request !== undefined) normalized = cacheRequestForInput(request);
      } catch (error) {
        return Promise.reject(error);
      }
      return queueCacheTask(() => {
        const entries = cacheNative("entries", this._name);
        const matches = normalized === null
          ? entries
          : cacheMatchingEntries(entries, normalized, options);
        return matches.map(entry => cacheRestoreRequest(JSON.parse(String(entry.request))));
      });
    }
  }

  class CacheStorage {
    constructor(token) {
      if (token !== CACHE_STORAGE_CONSTRUCTION_TOKEN) {
        throw new TypeError("CacheStorage objects cannot be constructed directly");
      }
      this._cacheObjects = new Map();
    }

    get [Symbol.toStringTag]() { return "CacheStorage"; }

    _cache(name) {
      const key = String(name);
      let cache = this._cacheObjects.get(key);
      if (!cache) {
        cache = new Cache(key, CACHE_CONSTRUCTION_TOKEN);
        this._cacheObjects.set(key, cache);
      }
      return cache;
    }

    open(name) {
      const key = String(name);
      return queueCacheTask(() => {
        cacheNative("open", key);
        return this._cache(key);
      });
    }

    has(name) {
      return queueCacheTask(() => Boolean(cacheNative("has", String(name))));
    }

    keys() {
      return queueCacheTask(() => cacheNative("keys"));
    }

    delete(name) {
      const key = String(name);
      return queueCacheTask(() => {
        const deleted = Boolean(cacheNative("delete", key));
        if (deleted) this._cacheObjects.delete(key);
        return deleted;
      });
    }

    match(request, options = {}) {
      let normalized;
      try {
        normalized = cacheRequestForInput(request);
      } catch (error) {
        return Promise.reject(error);
      }
      return queueCacheTask(() => {
        const names = cacheNative("keys");
        for (const name of names) {
          const entries = cacheNative("entries", name);
          const match = cacheMatchingEntries(entries, normalized, options)[0];
          if (match) return cacheRestoreResponse(JSON.parse(String(match.response)));
        }
        return undefined;
      });
    }
  }

  globalThis.Cache = Cache;
  globalThis.CacheStorage = CacheStorage;
  globalThis.caches = new CacheStorage(CACHE_STORAGE_CONSTRUCTION_TOKEN);

  // -------------------------------------------------------------------------
  // IndexedDB
  // -------------------------------------------------------------------------
  //
  // IndexedDB is intentionally an in-memory, origin-partitioned model here.
  // The observable request/transaction and schema semantics are kept separate
  // from durability so pages can exercise the API without making the JS realm
  // depend on a host database or a process-global lock.

  const IDB_CONSTRUCTION_TOKEN = {};
  const indexedDatabaseRecords = new Map();

  function idbQueueTask(callback) {
    if (typeof __omoikane_queue_dom_manipulation_task === "function") {
      __omoikane_queue_dom_manipulation_task(callback);
    } else {
      setTimeout(callback, 0);
    }
  }

  function idbOriginKey(name) {
    const origin = globalThis.location && globalThis.location.origin
      ? String(globalThis.location.origin) : "null";
    return origin + "\u0000" + String(name);
  }

  function idbError(name, message) {
    return new DOMException(message || name, name);
  }

  function idbDispatch(target, event) {
    // fireRealtimeEvent invokes an `on*` property before EventTarget's normal
    // dispatch path assigns target. IndexedDB handlers commonly read
    // event.target.result, so establish the target before invoking them.
    event.target = target;
    fireRealtimeEvent(target, event);
  }

  function idbNormalizeKey(value) {
    if (value instanceof Date) {
      const time = value.getTime();
      if (!Number.isFinite(time)) throw idbError("DataError", "The key is not valid.");
      return time === 0 ? 0 : time;
    }
    if (typeof value === "number") {
      if (!Number.isFinite(value) || Number.isNaN(value)) {
        throw idbError("DataError", "The key is not valid.");
      }
      return Object.is(value, -0) ? 0 : value;
    }
    if (typeof value === "string") return value;
    if (Array.isArray(value)) return value.map(idbNormalizeKey);
    throw idbError("DataError", "The key is not valid.");
  }

  function idbKeyToken(key) {
    if (typeof key === "number") return "number:" + String(key);
    if (typeof key === "string") return "string:" + key;
    return "array:" + JSON.stringify(key);
  }

  function idbCompareKey(left, right) {
    const a = idbNormalizeKey(left);
    const b = idbNormalizeKey(right);
    const rank = value => typeof value === "number" ? 0 : typeof value === "string" ? 1 : 2;
    const ar = rank(a);
    const br = rank(b);
    if (ar !== br) return ar - br;
    if (ar === 0 || ar === 1) return a < b ? -1 : a > b ? 1 : 0;
    const length = Math.min(a.length, b.length);
    for (let index = 0; index < length; index++) {
      const compared = idbCompareKey(a[index], b[index]);
      if (compared !== 0) return compared;
    }
    return a.length - b.length;
  }

  function idbExtractKey(value, keyPath) {
    if (keyPath === null || keyPath === undefined) return undefined;
    let current = value;
    for (const part of String(keyPath).split(".")) {
      if (current === null || current === undefined) return undefined;
      current = current[part];
    }
    return current;
  }

  function idbAssignKey(value, keyPath, key) {
    const parts = String(keyPath).split(".");
    let current = value;
    for (let index = 0; index < parts.length - 1; index++) {
      const part = parts[index];
      if (current[part] === undefined) current[part] = {};
      if (current[part] === null || typeof current[part] !== "object") {
        throw idbError("DataError", "The key path cannot be assigned.");
      }
      current = current[part];
    }
    current[parts[parts.length - 1]] = key;
  }

  function idbNameList(names) {
    const values = Array.from(names, String).sort();
    const result = {
      get length() { return values.length; },
      item(index) { return values[Math.trunc(Number(index))] ?? null; },
      contains(name) { return values.includes(String(name)); },
      [Symbol.iterator]() { return values[Symbol.iterator](); },
      get [Symbol.toStringTag]() { return "DOMStringList"; },
    };
    for (let index = 0; index < values.length; index++) {
      Object.defineProperty(result, index, { enumerable: true, value: values[index] });
    }
    return result;
  }

  class IDBVersionChangeEvent extends Event {
    constructor(type, init = {}) {
      super(type, init);
      this.oldVersion = Number(init.oldVersion) || 0;
      this.newVersion = init.newVersion === null || init.newVersion === undefined
        ? null : Number(init.newVersion);
    }
    get [Symbol.toStringTag]() { return "IDBVersionChangeEvent"; }
  }

  class IDBRequest extends EventTarget {
    constructor(source = null, transaction = null) {
      super();
      this.result = undefined;
      this.error = null;
      this.source = source;
      this.transaction = transaction;
      this.readyState = "pending";
      this.onsuccess = null;
      this.onerror = null;
      this.__finished = false;
    }
    __success(value) {
      if (this.__finished) return;
      this.__finished = true;
      this.result = value;
      this.error = null;
      this.readyState = "done";
      idbDispatch(this, new Event("success"));
    }
    __error(error) {
      if (this.__finished) return;
      this.__finished = true;
      this.result = undefined;
      this.error = error instanceof DOMException ? error : idbError("UnknownError", String(error));
      this.readyState = "done";
      const event = new Event("error", { cancelable: true });
      idbDispatch(this, event);
      if (!event.defaultPrevented && this.transaction) this.transaction.abort(this.error);
    }
    get [Symbol.toStringTag]() { return "IDBRequest"; }
  }

  class IDBOpenDBRequest extends IDBRequest {
    constructor() {
      super(null, null);
      this.onblocked = null;
      this.onupgradeneeded = null;
    }
    get [Symbol.toStringTag]() { return "IDBOpenDBRequest"; }
  }

  class IDBKeyRange {
    constructor(lower, upper, lowerOpen = false, upperOpen = false) {
      this.lower = lower === undefined ? undefined : idbNormalizeKey(lower);
      this.upper = upper === undefined ? undefined : idbNormalizeKey(upper);
      this.lowerOpen = Boolean(lowerOpen);
      this.upperOpen = Boolean(upperOpen);
      if (this.lower !== undefined && this.upper !== undefined && idbCompareKey(this.lower, this.upper) > 0) {
        throw idbError("DataError", "The lower key is greater than the upper key.");
      }
    }
    includes(value) {
      const key = idbNormalizeKey(value);
      if (this.lower !== undefined) {
        const compared = idbCompareKey(key, this.lower);
        if (compared < 0 || (this.lowerOpen && compared === 0)) return false;
      }
      if (this.upper !== undefined) {
        const compared = idbCompareKey(key, this.upper);
        if (compared > 0 || (this.upperOpen && compared === 0)) return false;
      }
      return true;
    }
    static bound(lower, upper, lowerOpen = false, upperOpen = false) {
      return new IDBKeyRange(lower, upper, lowerOpen, upperOpen);
    }
    static lowerBound(lower, open = false) { return new IDBKeyRange(lower, undefined, open, false); }
    static upperBound(upper, open = false) { return new IDBKeyRange(undefined, upper, false, open); }
    static only(value) { return new IDBKeyRange(value, value, false, false); }
    get [Symbol.toStringTag]() { return "IDBKeyRange"; }
  }

  class IDBTransaction extends EventTarget {
    constructor(database, storeNames, mode, token) {
      if (token !== IDB_CONSTRUCTION_TOKEN) throw new TypeError("Illegal constructor");
      super();
      this.db = database;
      this.objectStoreNames = idbNameList(storeNames);
      this.mode = String(mode);
      this.durability = "default";
      this.error = null;
      this.__state = "active";
      this.__pending = 0;
      this.__completionQueued = false;
      this.__completionCallbacks = [];
      this.onabort = null;
      this.oncomplete = null;
      this.onerror = null;
    }
    objectStore(name) {
      if (this.__state === "finished") throw idbError("InvalidStateError", "The transaction is finished.");
      const key = String(name);
      if (!this.objectStoreNames.contains(key)) throw idbError("NotFoundError", "The object store was not found.");
      return new IDBObjectStore(this.db.__record.stores.get(key), this);
    }
    __request(source, action) {
      if (this.__state !== "active") throw idbError("TransactionInactiveError", "The transaction is inactive.");
      const request = new IDBRequest(source, this);
      this.__pending++;
      idbQueueTask(() => {
        if (this.__state === "finished") return;
        try {
          request.__success(action());
        } catch (error) {
          request.__error(error);
        } finally {
          this.__pending--;
          this.__maybeComplete();
        }
      });
      return request;
    }
    __maybeComplete() {
      if (this.__pending !== 0 || this.__completionQueued || this.__state !== "active") return;
      this.__completionQueued = true;
      idbQueueTask(() => {
        this.__completionQueued = false;
        if (this.__pending !== 0 || this.__state !== "active") return;
        this.__state = "finished";
        idbDispatch(this, new Event("complete"));
        const callbacks = this.__completionCallbacks.splice(0);
        for (const callback of callbacks) callback();
      });
    }
    __afterComplete(callback) {
      if (this.__state === "finished") callback();
      else this.__completionCallbacks.push(callback);
      this.__maybeComplete();
    }
    abort(reason = idbError("AbortError", "The transaction was aborted.")) {
      if (this.__state === "finished") return;
      this.__state = "finished";
      this.error = reason instanceof DOMException ? reason : idbError("AbortError", String(reason));
      idbDispatch(this, new Event("abort"));
      const callbacks = this.__completionCallbacks.splice(0);
      for (const callback of callbacks) callback();
    }
    get [Symbol.toStringTag]() { return "IDBTransaction"; }
  }
  IDBTransaction.READ_ONLY = "readonly";
  IDBTransaction.READ_WRITE = "readwrite";
  IDBTransaction.VERSION_CHANGE = "versionchange";
  IDBTransaction.prototype.READ_ONLY = "readonly";
  IDBTransaction.prototype.READ_WRITE = "readwrite";
  IDBTransaction.prototype.VERSION_CHANGE = "versionchange";

  function idbStoreKey(store, value, explicitKey, forAdd) {
    let key = explicitKey;
    if (store.keyPath !== null) {
      const embedded = idbExtractKey(value, store.keyPath);
      if (embedded !== undefined) {
        if (key !== undefined) throw idbError("DataError", "A key was supplied for an inline key path.");
        key = embedded;
      } else if (store.autoIncrement) {
        key = store.nextKey++;
        idbAssignKey(value, store.keyPath, key);
      }
    }
    if (key === undefined) {
      if (!store.autoIncrement) throw idbError("DataError", "A key is required.");
      key = store.nextKey++;
    }
    key = idbNormalizeKey(key);
    const token = idbKeyToken(key);
    if (forAdd && store.records.has(token)) throw idbError("ConstraintError", "The key already exists.");
    return { key, token };
  }

  function idbQueryKey(query) {
    if (query === undefined) return null;
    return query instanceof IDBKeyRange ? query : IDBKeyRange.only(query);
  }

  function idbSortedRecords(store, query = undefined) {
    const range = idbQueryKey(query);
    return Array.from(store.records.values())
      .filter(entry => range === null || range.includes(entry.key))
      .sort((left, right) => idbCompareKey(left.key, right.key));
  }

  function idbKeyPathMatches(value, keyPath, query) {
    const key = idbExtractKey(value, keyPath);
    if (key === undefined) return false;
    try { return query === null || query.includes(key); } catch (_) { return false; }
  }

  class IDBIndex {
    constructor(store, definition, token) {
      if (token !== IDB_CONSTRUCTION_TOKEN) throw new TypeError("Illegal constructor");
      this.objectStore = store;
      this.__definition = definition;
      this.name = definition.name;
      this.keyPath = definition.keyPath;
      this.multiEntry = Boolean(definition.multiEntry);
      this.unique = Boolean(definition.unique);
    }
    get(query) {
      const range = idbQueryKey(query);
      return this.objectStore.transaction.__request(this, () => {
        const entry = idbSortedRecords(this.objectStore.__record).find(item =>
          idbKeyPathMatches(item.value, this.keyPath, range));
        return entry === undefined ? undefined : structuredClone(entry.value);
      });
    }
    getAll(query = undefined, count = undefined) {
      const range = idbQueryKey(query);
      return this.objectStore.transaction.__request(this, () => {
        const values = idbSortedRecords(this.objectStore.__record)
          .filter(item => idbKeyPathMatches(item.value, this.keyPath, range))
          .map(item => structuredClone(item.value));
        return count === undefined ? values : values.slice(0, Math.max(0, Math.trunc(Number(count))));
      });
    }
    count(query = undefined) {
      const range = idbQueryKey(query);
      return this.objectStore.transaction.__request(this, () =>
        idbSortedRecords(this.objectStore.__record)
          .filter(item => idbKeyPathMatches(item.value, this.keyPath, range)).length);
    }
    get [Symbol.toStringTag]() { return "IDBIndex"; }
  }

  class IDBObjectStore {
    constructor(record, transaction) {
      this.__record = record;
      this.transaction = transaction;
      this.name = record.name;
      this.keyPath = record.keyPath;
      this.autoIncrement = Boolean(record.autoIncrement);
    }
    get indexNames() { return idbNameList(this.__record.indexes.keys()); }
    add(value, key = undefined) { return this.__write(value, key, true); }
    put(value, key = undefined) { return this.__write(value, key, false); }
    __write(value, explicitKey, forAdd) {
      const store = this.__record;
      return this.transaction.__request(this, () => {
        const cloned = structuredClone(value);
        const shaped = idbStoreKey(store, cloned, explicitKey, forAdd);
        store.records.set(shaped.token, { key: shaped.key, value: cloned });
        return shaped.key;
      });
    }
    get(query) {
      const store = this.__record;
      const range = idbQueryKey(query);
      return this.transaction.__request(this, () => {
        if (range === null) throw idbError("DataError", "A key is required.");
        const entry = idbSortedRecords(store).find(item => range.includes(item.key));
        return entry === undefined ? undefined : structuredClone(entry.value);
      });
    }
    getKey(query) {
      const range = idbQueryKey(query);
      return this.transaction.__request(this, () => {
        if (range === null) throw idbError("DataError", "A key is required.");
        const entry = idbSortedRecords(this.__record).find(item => range.includes(item.key));
        return entry === undefined ? undefined : entry.key;
      });
    }
    getAll(query = undefined, count = undefined) {
      const values = () => idbSortedRecords(this.__record, query).map(item => structuredClone(item.value));
      return this.transaction.__request(this, () => {
        const result = values();
        return count === undefined ? result : result.slice(0, Math.max(0, Math.trunc(Number(count))));
      });
    }
    count(query = undefined) {
      return this.transaction.__request(this, () => idbSortedRecords(this.__record, query).length);
    }
    delete(query) {
      const range = idbQueryKey(query);
      return this.transaction.__request(this, () => {
        if (range === null) throw idbError("DataError", "A key is required.");
        const matches = idbSortedRecords(this.__record).filter(item => range.includes(item.key));
        for (const entry of matches) this.__record.records.delete(idbKeyToken(entry.key));
        return undefined;
      });
    }
    clear() {
      return this.transaction.__request(this, () => {
        this.__record.records.clear();
        return undefined;
      });
    }
    createIndex(name, keyPath, options = {}) {
      if (this.transaction.mode !== "versionchange") {
        throw idbError("InvalidStateError", "Indexes can only be created during a version change.");
      }
      const key = String(name);
      if (this.__record.indexes.has(key)) throw idbError("ConstraintError", "The index already exists.");
      if (Array.isArray(keyPath)) throw idbError("NotSupportedError", "Array key paths are not supported.");
      const definition = { name: key, keyPath: String(keyPath), unique: Boolean(options.unique), multiEntry: Boolean(options.multiEntry) };
      this.__record.indexes.set(key, definition);
      return new IDBIndex(this, definition, IDB_CONSTRUCTION_TOKEN);
    }
    deleteIndex(name) {
      if (this.transaction.mode !== "versionchange") {
        throw idbError("InvalidStateError", "Indexes can only be deleted during a version change.");
      }
      if (!this.__record.indexes.delete(String(name))) throw idbError("NotFoundError", "The index was not found.");
    }
    index(name) {
      const definition = this.__record.indexes.get(String(name));
      if (!definition) throw idbError("NotFoundError", "The index was not found.");
      return new IDBIndex(this, definition, IDB_CONSTRUCTION_TOKEN);
    }
    get [Symbol.toStringTag]() { return "IDBObjectStore"; }
  }

  class IDBDatabase extends EventTarget {
    constructor(record, token) {
      if (token !== IDB_CONSTRUCTION_TOKEN) throw new TypeError("Illegal constructor");
      super();
      this.__record = record;
      this.name = record.name;
      this.version = record.version;
      this.onabort = null;
      this.onerror = null;
      this.onclose = null;
      this.onversionchange = null;
      this.__closed = false;
      record.connections.add(this);
    }
    get objectStoreNames() { return idbNameList(this.__record.stores.keys()); }
    createObjectStore(name, options = {}) {
      if (this.__upgradeTransaction === undefined || this.__upgradeTransaction.__state === "finished") {
        throw idbError("InvalidStateError", "Object stores can only be created during a version change.");
      }
      const key = String(name);
      if (this.__record.stores.has(key)) throw idbError("ConstraintError", "The object store already exists.");
      let keyPath = null;
      if (options.keyPath !== undefined && options.keyPath !== null) {
        if (Array.isArray(options.keyPath)) throw idbError("NotSupportedError", "Array key paths are not supported.");
        keyPath = String(options.keyPath);
      }
      const record = { name: key, keyPath, autoIncrement: Boolean(options.autoIncrement), nextKey: 1, records: new Map(), indexes: new Map() };
      this.__record.stores.set(key, record);
      return new IDBObjectStore(record, this.__upgradeTransaction);
    }
    deleteObjectStore(name) {
      if (this.__upgradeTransaction === undefined || this.__upgradeTransaction.__state === "finished") {
        throw idbError("InvalidStateError", "Object stores can only be deleted during a version change.");
      }
      if (!this.__record.stores.delete(String(name))) throw idbError("NotFoundError", "The object store was not found.");
    }
    transaction(storeNames, mode = "readonly", options = undefined) {
      if (this.__closed) throw idbError("InvalidStateError", "The database connection is closed.");
      const names = typeof storeNames === "string" || storeNames instanceof String ? [String(storeNames)] : Array.from(storeNames || [], String);
      if (names.length === 0) throw idbError("InvalidAccessError", "At least one object store is required.");
      const selected = Array.from(new Set(names));
      for (const name of selected) if (!this.__record.stores.has(name)) throw idbError("NotFoundError", "The object store was not found.");
      const selectedMode = String(mode);
      if (!["readonly", "readwrite"].includes(selectedMode)) throw idbError("TypeError", "Invalid transaction mode.");
      const transaction = new IDBTransaction(this, selected, selectedMode, IDB_CONSTRUCTION_TOKEN);
      void options;
      transaction.__maybeComplete();
      return transaction;
    }
    close() {
      if (this.__closed) return;
      this.__closed = true;
      this.__record.connections.delete(this);
      idbDispatch(this, new Event("close"));
    }
    get [Symbol.toStringTag]() { return "IDBDatabase"; }
  }

  function idbOpenDatabase(name, version, request) {
    const key = idbOriginKey(name);
    let record = indexedDatabaseRecords.get(key);
    const requestedVersion = version === undefined ? undefined : Number(version);
    if (requestedVersion !== undefined && (!Number.isFinite(requestedVersion) || requestedVersion <= 0 || Math.floor(requestedVersion) !== requestedVersion)) {
      throw idbError("TypeError", "The database version must be a positive integer.");
    }
    if (!record) {
      record = { key, name: String(name), version: requestedVersion === undefined ? 1 : requestedVersion, stores: new Map(), connections: new Set() };
      indexedDatabaseRecords.set(key, record);
      const database = new IDBDatabase(record, IDB_CONSTRUCTION_TOKEN);
      const transaction = new IDBTransaction(database, [], "versionchange", IDB_CONSTRUCTION_TOKEN);
      database.__upgradeTransaction = transaction;
      request.result = database;
      request.transaction = transaction;
      idbDispatch(request, new IDBVersionChangeEvent("upgradeneeded", { oldVersion: 0, newVersion: record.version }));
      transaction.__afterComplete(() => {
        delete database.__upgradeTransaction;
        request.transaction = null;
        request.__success(database);
      });
      transaction.__maybeComplete();
      return;
    }
    if (requestedVersion !== undefined && requestedVersion < record.version) {
      request.__error(idbError("VersionError", "The requested version is lower than the existing version."));
      return;
    }
    if (requestedVersion !== undefined && requestedVersion > record.version) {
      const oldVersion = record.version;
      record.version = requestedVersion;
      const database = new IDBDatabase(record, IDB_CONSTRUCTION_TOKEN);
      const transaction = new IDBTransaction(database, [], "versionchange", IDB_CONSTRUCTION_TOKEN);
      database.__upgradeTransaction = transaction;
      request.result = database;
      request.transaction = transaction;
      for (const connection of Array.from(record.connections)) {
        if (connection !== database) idbDispatch(connection, new IDBVersionChangeEvent("versionchange", { oldVersion, newVersion: requestedVersion }));
      }
      idbDispatch(request, new IDBVersionChangeEvent("upgradeneeded", { oldVersion, newVersion: requestedVersion }));
      transaction.__afterComplete(() => {
        delete database.__upgradeTransaction;
        request.transaction = null;
        request.__success(database);
      });
      transaction.__maybeComplete();
      return;
    }
    request.__success(new IDBDatabase(record, IDB_CONSTRUCTION_TOKEN));
  }

  class IDBFactory {
    open(name, version = undefined) {
      const request = new IDBOpenDBRequest();
      const databaseName = String(name);
      if (databaseName.length === 0) {
        request.__error(idbError("TypeError", "The database name must not be empty."));
        return request;
      }
      idbQueueTask(() => {
        try { idbOpenDatabase(databaseName, version, request); }
        catch (error) { request.__error(error); }
      });
      return request;
    }
    deleteDatabase(name) {
      const request = new IDBOpenDBRequest();
      const key = idbOriginKey(String(name));
      idbQueueTask(() => {
        const record = indexedDatabaseRecords.get(key);
        if (record) {
          for (const connection of Array.from(record.connections)) {
            idbDispatch(connection, new IDBVersionChangeEvent("versionchange", { oldVersion: record.version, newVersion: null }));
            connection.close();
          }
          indexedDatabaseRecords.delete(key);
        }
        request.__success(undefined);
      });
      return request;
    }
    databases() {
      return Promise.resolve(Array.from(indexedDatabaseRecords.values())
        .filter(record => record.key.startsWith((globalThis.location && globalThis.location.origin || "null") + "\u0000"))
        .map(record => ({ name: record.name, version: record.version })));
    }
    get [Symbol.toStringTag]() { return "IDBFactory"; }
  }

  globalThis.IDBVersionChangeEvent = IDBVersionChangeEvent;
  globalThis.IDBRequest = IDBRequest;
  globalThis.IDBOpenDBRequest = IDBOpenDBRequest;
  globalThis.IDBKeyRange = IDBKeyRange;
  globalThis.IDBTransaction = IDBTransaction;
  globalThis.IDBDatabase = IDBDatabase;
  globalThis.IDBObjectStore = IDBObjectStore;
  globalThis.IDBIndex = IDBIndex;
  globalThis.IDBFactory = IDBFactory;
  globalThis.indexedDB = new IDBFactory();

  function observerRect(x, y, width, height) {
    x = Number.isFinite(Number(x)) ? Number(x) : 0;
    y = Number.isFinite(Number(y)) ? Number(y) : 0;
    width = Math.max(0, Number.isFinite(Number(width)) ? Number(width) : 0);
    height = Math.max(0, Number.isFinite(Number(height)) ? Number(height) : 0);
    return Object.freeze({
      x, y, width, height,
      top: y, left: x, right: x + width, bottom: y + height,
    });
  }

  class ResizeObserverSize {
    constructor(inlineSize, blockSize) {
      this.inlineSize = inlineSize;
      this.blockSize = blockSize;
      Object.freeze(this);
    }
  }

  class ResizeObserverEntry {
    constructor(target, metrics) {
      const contentX = metrics.contentX - metrics.x;
      const contentY = metrics.contentY - metrics.y;
      this.target = target;
      this.contentRect = observerRect(
        contentX,
        contentY,
        metrics.contentWidth,
        metrics.contentHeight
      );
      this.contentBoxSize = Object.freeze([
        new ResizeObserverSize(metrics.contentWidth, metrics.contentHeight),
      ]);
      this.borderBoxSize = Object.freeze([
        new ResizeObserverSize(metrics.width, metrics.height),
      ]);
      this.devicePixelContentBoxSize = Object.freeze([
        new ResizeObserverSize(
          metrics.contentWidth * globalThis.devicePixelRatio,
          metrics.contentHeight * globalThis.devicePixelRatio
        ),
      ]);
    }
  }

  const activeResizeObservers = new Set();

  class ResizeObserver {
    constructor(callback) {
      if (arguments.length < 1 || typeof callback !== "function") {
        throw new TypeError("ResizeObserver callback must be callable");
      }
      this._callback = callback;
      this._targets = new Map();
      this._scheduled = false;
    }
    observe(target, options = {}) {
      if (!(target instanceof Element)) throw new TypeError("ResizeObserver target must be an Element");
      const box = options.box === undefined ? "content-box" : String(options.box);
      if (![
        "content-box", "border-box", "device-pixel-content-box",
      ].includes(box)) throw new TypeError("Unsupported ResizeObserver box option");
      const previous = this._targets.get(target);
      this._targets.set(target, { box, size: previous && previous.box === box ? previous.size : null });
      activeResizeObservers.add(this);
      this.__queueCheck();
    }
    unobserve(target) {
      this._targets.delete(target);
      if (this._targets.size === 0) activeResizeObservers.delete(this);
    }
    disconnect() {
      this._targets.clear();
      activeResizeObservers.delete(this);
    }
    __queueCheck() {
      if (this._scheduled || this._targets.size === 0) return;
      this._scheduled = true;
      Promise.resolve().then(() => {
        this._scheduled = false;
        const entries = [];
        for (const [target, observation] of this._targets) {
          const metrics = target.__layoutMetrics();
          let inlineSize = metrics.contentWidth;
          let blockSize = metrics.contentHeight;
          if (observation.box === "border-box") {
            inlineSize = metrics.width;
            blockSize = metrics.height;
          } else if (observation.box === "device-pixel-content-box") {
            inlineSize *= globalThis.devicePixelRatio;
            blockSize *= globalThis.devicePixelRatio;
          }
          const size = inlineSize + ":" + blockSize;
          if (size === observation.size) continue;
          observation.size = size;
          entries.push(new ResizeObserverEntry(target, metrics));
        }
        if (entries.length) this._callback.call(this, entries, this);
      });
    }
  }

  function parseRootMargin(input) {
    const tokens = String(input).trim().split(/\s+/).filter(Boolean);
    if (tokens.length < 1 || tokens.length > 4) {
      throw new DOMException("Invalid rootMargin", "SyntaxError");
    }
    const parsed = tokens.map(token => {
      const match = /^([+-]?(?:\d+(?:\.\d*)?|\.\d+))(px|%)$/i.exec(token);
      if (!match) throw new DOMException("Invalid rootMargin", "SyntaxError");
      return { value: Number(match[1]), unit: match[2].toLowerCase() };
    });
    const expanded = parsed.length === 1 ? [parsed[0], parsed[0], parsed[0], parsed[0]]
      : parsed.length === 2 ? [parsed[0], parsed[1], parsed[0], parsed[1]]
      : parsed.length === 3 ? [parsed[0], parsed[1], parsed[2], parsed[1]]
      : parsed;
    return {
      values: expanded,
      serialized: expanded.map(margin => margin.value + margin.unit).join(" "),
    };
  }

  function intersectionRootRect(observer) {
    let rect;
    let hasBox = true;
    if (observer.root === null || observer.root instanceof Document) {
      rect = observerRect(0, 0, globalThis.innerWidth, globalThis.innerHeight);
    } else {
      const metrics = observer.root.__layoutMetrics();
      hasBox = metrics.hasBox;
      rect = observerRect(
        metrics.x + metrics.clientLeft,
        metrics.y + metrics.clientTop,
        metrics.clientWidth,
        metrics.clientHeight
      );
    }
    const margins = observer._rootMarginValues.map(margin =>
      margin.unit === "%" ? margin.value * rect.width / 100 : margin.value
    );
    return {
      hasBox,
      rect: observerRect(
        rect.left - margins[3],
        rect.top - margins[0],
        rect.width + margins[1] + margins[3],
        rect.height + margins[0] + margins[2]
      ),
    };
  }

  function intersectionEntry(observer, target) {
    const targetSource = target.__layoutMetrics();
    const targetRect = observerRect(
      targetSource.x, targetSource.y, targetSource.width, targetSource.height
    );
    const root = intersectionRootRect(observer);
    const rootRect = root.rect;
    const left = Math.max(targetRect.left, rootRect.left);
    const top = Math.max(targetRect.top, rootRect.top);
    const right = Math.min(targetRect.right, rootRect.right);
    const bottom = Math.min(targetRect.bottom, rootRect.bottom);
    const isIntersecting = targetSource.hasBox && root.hasBox && right >= left && bottom >= top;
    const intersectionRect = isIntersecting
      ? observerRect(left, top, right - left, bottom - top)
      : observerRect(0, 0, 0, 0);
    const targetArea = targetRect.width * targetRect.height;
    const intersectionArea = intersectionRect.width * intersectionRect.height;
    const ratio = targetArea === 0 ? (isIntersecting ? 1 : 0) : intersectionArea / targetArea;
    return new IntersectionObserverEntry({
      target,
      boundingClientRect: targetRect,
      intersectionRect,
      rootBounds: rootRect,
      isIntersecting,
      intersectionRatio: ratio,
    });
  }

  class IntersectionObserverEntry {
    constructor(init) {
      this.time = globalThis.performance.now();
      this.target = init.target;
      this.rootBounds = init.rootBounds;
      this.boundingClientRect = init.boundingClientRect;
      this.intersectionRect = init.intersectionRect;
      this.isIntersecting = init.isIntersecting;
      this.intersectionRatio = init.intersectionRatio;
    }
  }

  const activeIntersectionObservers = new Set();

  class IntersectionObserver {
    constructor(callback, options = {}) {
      if (typeof callback !== "function") {
        throw new TypeError("IntersectionObserver callback must be callable");
      }
      const root = options.root === undefined ? null : options.root;
      if (root !== null && !(root instanceof Element) && !(root instanceof Document)) {
        throw new TypeError("IntersectionObserver root must be an Element or Document");
      }
      const margin = parseRootMargin(options.rootMargin === undefined ? "0px" : options.rootMargin);
      const thresholdInput = options.threshold === undefined ? [0] :
        (Array.isArray(options.threshold) ? options.threshold : [options.threshold]);
      const thresholds = thresholdInput.map(Number);
      if (thresholds.some(value => !Number.isFinite(value) || value < 0 || value > 1)) {
        throw new RangeError("IntersectionObserver threshold must be between 0 and 1");
      }
      this.root = root;
      this.rootMargin = margin.serialized;
      this.thresholds = Object.freeze([...new Set(thresholds)].sort((a, b) => a - b));
      this._rootMarginValues = margin.values;
      this._callback = callback;
      this._targets = new Map();
      this._records = [];
      this._scheduled = false;
      this._deliveryScheduled = false;
    }
    observe(target) {
      if (!(target instanceof Element)) throw new TypeError("IntersectionObserver target must be an Element");
      if (this._targets.has(target)) return;
      this._targets.set(target, null);
      activeIntersectionObservers.add(this);
      this.__queueCheck();
    }
    unobserve(target) {
      this._targets.delete(target);
      this._records = this._records.filter(entry => entry.target !== target);
      if (this._targets.size === 0) activeIntersectionObservers.delete(this);
    }
    disconnect() {
      this._targets.clear();
      this._records = [];
      activeIntersectionObservers.delete(this);
    }
    takeRecords() {
      const records = this._records;
      this._records = [];
      return records;
    }
    __queueCheck() {
      if (this._scheduled || this._targets.size === 0) return;
      this._scheduled = true;
      Promise.resolve().then(() => {
        this._scheduled = false;
        for (const [target, previousState] of this._targets) {
          const entry = intersectionEntry(this, target);
          const thresholdIndex = this.thresholds.findIndex(value => value > entry.intersectionRatio);
          const state = entry.isIntersecting + ":" + thresholdIndex;
          if (state === previousState) continue;
          this._targets.set(target, state);
          this._records.push(entry);
        }
        if (this._records.length && !this._deliveryScheduled) {
          this._deliveryScheduled = true;
          Promise.resolve().then(() => {
            this._deliveryScheduled = false;
            const records = this.takeRecords();
            if (records.length) this._callback.call(this, records, this);
          });
        }
      });
    }
  }

  globalThis.__omoikane_layout_observers_changed = function() {
    for (const observer of activeResizeObservers) observer.__queueCheck();
    for (const observer of activeIntersectionObservers) observer.__queueCheck();
  };
  globalThis.ResizeObserver = ResizeObserver;
  globalThis.ResizeObserverEntry = ResizeObserverEntry;
  globalThis.ResizeObserverSize = ResizeObserverSize;
  globalThis.IntersectionObserver = IntersectionObserver;
  globalThis.IntersectionObserverEntry = IntersectionObserverEntry;
})();
