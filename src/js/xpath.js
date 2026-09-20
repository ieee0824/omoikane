(() => {
  "use strict";

  // DOM Level 3 XPath bindings backed by the existing JavaScript DOM wrappers.
  // Keeping evaluation in this layer preserves wrapper identity for node-set
  // results while the native tree remains the source of DOM structure.

  const HTML_NS = "http://www.w3.org/1999/xhtml";
  const XML_NS = "http://www.w3.org/XML/1998/namespace";
  const versions = new WeakMap();
  const iteratorDocuments = new WeakSet();
  let hasIteratorDocuments = false;
  const installMutationHook = globalThis.__omoikane_install_xpath_mutation_hook;

  function ownerDocument(node) {
    if (!node) return null;
    if (node.nodeType === 9) return node;
    return node.ownerDocument || (node.ownerElement && node.ownerElement.ownerDocument) || null;
  }

  installMutationHook(target => {
    // Most pages never create an XPath iterator. Keep their DOM mutation path
    // unchanged: resolving ownerDocument for every mutation is only necessary
    // once an iterator exists and can become invalid.
    if (!hasIteratorDocuments) return;
    const doc = ownerDocument(target);
    if (doc && iteratorDocuments.has(doc)) {
      versions.set(doc, (versions.get(doc) || 0) + 1);
    }
  });

  function namespaceElement(node) {
    if (!node) return null;
    if (node.nodeType === 9) return node.documentElement;
    if (node.nodeType === 2) return node.ownerElement;
    if (node.nodeType === 1) return node;
    return node.parentElement;
  }

  if (!Node.prototype.lookupNamespaceURI) {
    Object.defineProperty(Node.prototype, "lookupNamespaceURI", {
      configurable: true, enumerable: true, writable: true,
      value(prefix) {
        prefix = prefix == null || prefix === "" ? null : String(prefix);
        let element = namespaceElement(this);
        if (!element) return null;
        if (prefix === "xml") return XML_NS;
        for (; element; element = element.parentElement) {
          if ((element.prefix || null) === prefix && element.namespaceURI) return element.namespaceURI;
          const name = prefix === null ? "xmlns" : "xmlns:" + prefix;
          if (element.hasAttribute(name)) return element.getAttribute(name) || null;
        }
        return null;
      },
    });
  }

  const NODE_TYPES = new Set(["node", "text", "comment", "processing-instruction"]);
  const asciiLower = text => String(text).replace(/[A-Z]/g, char => String.fromCharCode(char.charCodeAt(0) + 32));

  function syntax(message = "The XPath expression is not valid.") {
    throw new DOMException(message, "SyntaxError");
  }

  function tokenize(source) {
    const tokens = [];
    let index = 0;
    const isSpace = char => char === " " || char === "\t" || char === "\r" || char === "\n";
    const isNameStart = char => !!char && (/[A-Za-z_]/.test(char) || char.toUpperCase() !== char.toLowerCase());
    const isNameChar = char => !!char && (isNameStart(char) || /[0-9.\-]/.test(char));
    while (index < source.length) {
      const char = source[index];
      if (isSpace(char)) { index++; continue; }
      const pair = source.slice(index, index + 2);
      if (["//", "..", "::", "!=", "<=", ">="].includes(pair)) {
        tokens.push({ type: "symbol", value: pair }); index += 2; continue;
      }
      if (char === "'" || char === '"') {
        const quote = char;
        const end = source.indexOf(quote, index + 1);
        if (end < 0) syntax("Unterminated XPath string literal.");
        tokens.push({ type: "string", value: source.slice(index + 1, end) });
        index = end + 1; continue;
      }
      if (/[0-9]/.test(char) || (char === "." && /[0-9]/.test(source[index + 1] || ""))) {
        const start = index;
        if (char === ".") index++;
        while (/[0-9]/.test(source[index] || "")) index++;
        if (source[index] === ".") {
          index++;
          while (/[0-9]/.test(source[index] || "")) index++;
        }
        tokens.push({ type: "number", value: Number(source.slice(start, index)) });
        continue;
      }
      if (isNameStart(char)) {
        const start = index++;
        while (isNameChar(source[index])) index++;
        if (source[index] === ":" && source[index + 1] !== ":") {
          index++;
          if (!isNameStart(source[index])) syntax("Invalid qualified name in XPath expression.");
          index++;
          while (isNameChar(source[index])) index++;
        }
        tokens.push({ type: "name", value: source.slice(start, index) });
        continue;
      }
      if ("/[]()@,|+-*=<>.$".includes(char)) {
        tokens.push({ type: "symbol", value: char }); index++; continue;
      }
      syntax("Unexpected character in XPath expression.");
    }
    tokens.push({ type: "eof", value: "" });
    return tokens;
  }

  class Parser {
    constructor(source) { this.tokens = tokenize(String(source)); this.index = 0; }
    peek(value) { const token = this.tokens[this.index]; return value === undefined ? token : token.value === value; }
    take(value) { if (!this.peek(value)) return false; this.index++; return true; }
    expect(value) { if (!this.take(value)) syntax("Expected '" + value + "'."); }
    parse() { const result = this.or(); if (this.peek().type !== "eof") syntax(); return result; }
    binary(next, operators) {
      let left = next.call(this);
      while (operators.includes(this.peek().value)) {
        const op = this.tokens[this.index++].value;
        left = { kind: "binary", op, left, right: next.call(this) };
      }
      return left;
    }
    or() { return this.binary(this.and, ["or"]); }
    and() { return this.binary(this.equality, ["and"]); }
    equality() { return this.binary(this.relational, ["=", "!="]); }
    relational() { return this.binary(this.additive, ["<", ">", "<=", ">="]); }
    additive() { return this.binary(this.multiplicative, ["+", "-"]); }
    multiplicative() { return this.binary(this.unary, ["*", "div", "mod"]); }
    unary() { return this.take("-") ? { kind: "unary", value: this.unary() } : this.union(); }
    union() { return this.binary(this.path, ["|"]); }
    path() {
      if (this.peek("/") || this.peek("//") || this.locationStart()) return this.location();
      let base = this.primary();
      base = this.predicates(base);
      if (this.peek("/") || this.peek("//")) {
        const steps = [];
        this.pathSeparator(steps);
        this.relativeSteps(steps);
        return { kind: "pathFrom", base, steps };
      }
      return base;
    }
    locationStart() {
      const token = this.peek();
      if ([".", "..", "@", "*"].includes(token.value)) return true;
      if (token.type !== "name") return false;
      const next = this.tokens[this.index + 1].value;
      return next !== "(" || NODE_TYPES.has(token.value);
    }
    location() {
      const steps = [];
      let absolute = false;
      if (this.peek("/") || this.peek("//")) {
        absolute = true;
        this.pathSeparator(steps);
        if (this.peek().type === "eof" || this.peek(")") || this.peek("]")) {
          if (steps.length) syntax();
          return { kind: "path", absolute, steps };
        }
      }
      this.relativeSteps(steps);
      return { kind: "path", absolute, steps };
    }
    pathSeparator(steps) {
      if (this.take("//")) steps.push({ axis: "descendant-or-self", test: { type: "node" }, predicates: [] });
      else this.expect("/");
    }
    relativeSteps(steps) {
      steps.push(this.step());
      while (this.peek("/") || this.peek("//")) {
        this.pathSeparator(steps);
        steps.push(this.step());
      }
    }
    step() {
      if (this.take(".")) return { axis: "self", test: { type: "node" }, predicates: [] };
      if (this.take("..")) return { axis: "parent", test: { type: "node" }, predicates: [] };
      let axis = "child";
      if (this.take("@")) axis = "attribute";
      else if (this.peek().type === "name" && this.tokens[this.index + 1].value === "::") {
        axis = this.tokens[this.index++].value; this.index++;
      }
      const supported = ["ancestor", "ancestor-or-self", "attribute", "child", "descendant",
        "descendant-or-self", "following", "following-sibling", "parent", "preceding",
        "preceding-sibling", "self"];
      if (!supported.includes(axis)) syntax("Unsupported XPath axis '" + axis + "'.");
      let test;
      if (this.take("*")) test = { type: "wildcard" };
      else {
        const token = this.peek();
        if (token.type !== "name") syntax("Expected an XPath node test.");
        this.index++;
        if (this.take("(")) {
          if (!NODE_TYPES.has(token.value)) syntax();
          let target = null;
          if (token.value === "processing-instruction" && this.peek().type === "string") target = this.tokens[this.index++].value;
          this.expect(")");
          test = { type: token.value, target };
        } else test = { type: "name", name: token.value };
      }
      const predicates = [];
      while (this.take("[")) { predicates.push(this.or()); this.expect("]"); }
      return { axis, test, predicates };
    }
    predicates(base) {
      const list = [];
      while (this.take("[")) { list.push(this.or()); this.expect("]"); }
      return list.length ? { kind: "filter", base, predicates: list } : base;
    }
    primary() {
      const token = this.peek();
      if (this.take("$")) {
        if (this.peek().type !== "name") syntax("Expected an XPath variable name.");
        this.index++;
        return { kind: "variable" };
      }
      if (token.type === "string") { this.index++; return { kind: "literal", value: token.value }; }
      if (token.type === "number") { this.index++; return { kind: "number", value: token.value }; }
      if (this.take("(")) { const value = this.or(); this.expect(")"); return value; }
      if (token.type === "name" && this.tokens[this.index + 1].value === "(") {
        this.index += 2;
        const args = [];
        if (!this.take(")")) {
          do { args.push(this.or()); } while (this.take(","));
          this.expect(")");
        }
        return { kind: "function", name: token.value, args };
      }
      syntax("Expected an XPath expression.");
    }
  }

  function rootOf(node) {
    if (node.nodeType === 2 && node.ownerElement) node = node.ownerElement;
    while (node.parentNode) node = node.parentNode;
    return node;
  }
  function parentOf(node) { return node.nodeType === 2 ? node.ownerElement : node.parentNode; }
  function descendants(node, includeSelf = false) {
    const result = includeSelf ? [node] : [];
    for (const child of node.childNodes || []) result.push(child, ...descendants(child));
    return result;
  }
  function attributes(node) { return node.nodeType === 1 ? Array.from(node.attributes) : []; }
  function documentOrder(root) {
    const result = [];
    const visit = node => {
      result.push(node);
      if (node.nodeType === 1) result.push(...attributes(node));
      for (const child of node.childNodes || []) visit(child);
    };
    visit(root);
    return result;
  }
  function unique(nodes) { return Array.from(new Set(nodes)); }
  function ordered(nodes) {
    nodes = unique(nodes);
    if (nodes.length < 2) return nodes;
    const roots = new Map();
    const key = node => {
      const root = rootOf(node);
      if (!roots.has(root)) roots.set(root, new Map(documentOrder(root).map((item, index) => [item, index])));
      return roots.get(root).get(node) ?? Number.MAX_SAFE_INTEGER;
    };
    return nodes.sort((left, right) => key(left) - key(right));
  }
  function stringValue(node) {
    if (!node) return "";
    if (node.nodeType === 2) return node.value;
    if ([3, 4, 7, 8].includes(node.nodeType)) return node.nodeValue || "";
    let value = "";
    for (const descendant of descendants(node)) if (descendant.nodeType === 3 || descendant.nodeType === 4) value += descendant.nodeValue || "";
    return value;
  }
  const isNodeSet = value => Array.isArray(value);
  function asString(value) {
    if (isNodeSet(value)) return value.length ? stringValue(ordered(value)[0]) : "";
    if (typeof value === "boolean") return value ? "true" : "false";
    if (typeof value === "number") {
      if (Number.isNaN(value)) return "NaN";
      if (value === Infinity) return "Infinity";
      if (value === -Infinity) return "-Infinity";
      if (Object.is(value, -0)) return "0";
      return String(value);
    }
    return String(value);
  }
  function asNumber(value) {
    if (typeof value === "number") return value;
    if (typeof value === "boolean") return value ? 1 : 0;
    const text = asString(value).trim();
    if (!/^-?(?:\d+(?:\.\d*)?|\.\d+)$/.test(text)) return NaN;
    return Number(text);
  }
  function asBoolean(value) {
    if (isNodeSet(value)) return value.length > 0;
    if (typeof value === "boolean") return value;
    if (typeof value === "number") return value !== 0 && !Number.isNaN(value);
    return value.length > 0;
  }

  function axisNodes(node, axis) {
    const parent = parentOf(node);
    if (axis === "self") return [node];
    if (axis === "parent") return parent ? [parent] : [];
    if (axis === "child") return Array.from(node.childNodes || []);
    if (axis === "attribute") return attributes(node);
    if (axis === "descendant") return descendants(node);
    if (axis === "descendant-or-self") return descendants(node, true);
    if (axis === "ancestor" || axis === "ancestor-or-self") {
      const result = axis === "ancestor-or-self" ? [node] : [];
      for (let item = parent; item; item = parentOf(item)) result.push(item);
      return result;
    }
    if (axis === "following-sibling" || axis === "preceding-sibling") {
      if (!parent || node.nodeType === 2) return [];
      const siblings = Array.from(parent.childNodes || []);
      const index = siblings.indexOf(node);
      return axis === "following-sibling" ? siblings.slice(index + 1) : siblings.slice(0, index).reverse();
    }
    const all = documentOrder(rootOf(node)).filter(item => item.nodeType !== 2);
    const index = all.indexOf(node);
    if (axis === "following") {
      const own = new Set(descendants(node, true));
      return all.slice(index + 1).filter(item => !own.has(item));
    }
    if (axis === "preceding") {
      const ancestors = new Set(axisNodes(node, "ancestor"));
      return all.slice(0, index).filter(item => !ancestors.has(item)).reverse();
    }
    return [];
  }

  function resolvedName(name, context) {
    const colon = name.indexOf(":");
    if (colon < 0) return { namespace: null, local: name, prefixed: false };
    const prefix = name.slice(0, colon);
    let namespace;
    if (context.namespaces.has(prefix)) namespace = context.namespaces.get(prefix);
    else {
      const resolver = context.resolver;
      if (resolver == null) throw new DOMException("Unresolved XPath namespace prefix.", "NamespaceError");
      let value;
      try {
        value = typeof resolver === "function"
          ? resolver(prefix)
          : resolver.lookupNamespaceURI(prefix);
      } catch (error) {
        Promise.resolve().then(() => {
          const event = new Event("error");
          Object.defineProperty(event, "error", { value: error });
          globalThis.dispatchEvent(event);
        });
        throw new DOMException("Unresolved XPath namespace prefix.", "NamespaceError");
      }
      if (value === null || value === undefined) {
        throw new DOMException("Unresolved XPath namespace prefix.", "NamespaceError");
      }
      try {
        if (typeof value === "symbol") throw new TypeError("Namespace URI cannot be a symbol");
        namespace = String(value);
      } catch (error) {
        Promise.resolve().then(() => {
          const event = new Event("error");
          Object.defineProperty(event, "error", { value: error });
          globalThis.dispatchEvent(event);
        });
        throw new DOMException("Unresolved XPath namespace prefix.", "NamespaceError");
      }
      context.namespaces.set(prefix, namespace);
    }
    return { namespace, local: name.slice(colon + 1), prefixed: true };
  }

  function matchesTest(node, test, axis, context) {
    const principal = axis === "attribute" ? 2 : 1;
    if (test.type === "node") return true;
    if (test.type === "text") return node.nodeType === 3 || node.nodeType === 4;
    if (test.type === "comment") return node.nodeType === 8;
    if (test.type === "processing-instruction") return node.nodeType === 7 && (test.target === null || node.nodeName === test.target);
    if (node.nodeType !== principal) return false;
    if (test.type === "wildcard") return true;
    const expected = resolvedName(test.name, context);
    const htmlElement = node.nodeType === 1 ? node : node.ownerElement;
    const isHtmlElement = htmlElement instanceof HTMLElement;
    const htmlName = context.document && context.document.contentType === "text/html" &&
      isHtmlElement && !expected.prefixed;
    const svgAdjustedAttribute = node.nodeType === 2 && htmlElement &&
      htmlElement.namespaceURI === "http://www.w3.org/2000/svg" && node.localName === "refx";
    const local = htmlName ? asciiLower(node.localName) : svgAdjustedAttribute ? "refX" : String(node.localName);
    let wanted = htmlName ? asciiLower(expected.local) : expected.local;
    if (local !== wanted) return false;
    if (expected.prefixed) {
      const namespace = node.namespaceURI || (node.nodeType === 1 && node instanceof HTMLElement ? HTML_NS : null);
      return namespace === expected.namespace;
    }
    if (node.nodeType === 2 && (node.name === "xmlns" || node.prefix === "xmlns")) return false;
    return htmlName ? true : node.namespaceURI === null;
  }

  function filterPredicates(nodes, predicates, context) {
    let result = nodes;
    for (const predicate of predicates) {
      const size = result.length;
      result = result.filter((node, index) => {
        const value = evaluate(predicate, { ...context, node, position: index + 1, size });
        return typeof value === "number" ? index + 1 === value : asBoolean(value);
      });
    }
    return result;
  }
  function applySteps(input, steps, context) {
    let current = input;
    for (const step of steps) {
      const next = [];
      for (const node of current) {
        let candidates = axisNodes(node, step.axis).filter(item => matchesTest(item, step.test, step.axis, context));
        candidates = filterPredicates(candidates, step.predicates, context);
        next.push(...candidates);
      }
      current = ordered(next);
    }
    return current;
  }

  function compare(left, right, op) {
    if (isNodeSet(left) || isNodeSet(right)) {
      if (isNodeSet(left) && isNodeSet(right)) {
        for (const a of left) for (const b of right) if (compare(stringValue(a), stringValue(b), op)) return true;
        return false;
      }
      const nodes = isNodeSet(left) ? left : right;
      const scalar = isNodeSet(left) ? right : left;
      if (typeof scalar === "boolean") return compare(asBoolean(nodes), scalar, op);
      for (const node of nodes) {
        const value = typeof scalar === "number" || !["=", "!="].includes(op) ? asNumber(stringValue(node)) : stringValue(node);
        if (isNodeSet(left) ? compare(value, scalar, op) : compare(scalar, value, op)) return true;
      }
      return false;
    }
    if (op === "=" || op === "!=") {
      let equal;
      if (typeof left === "boolean" || typeof right === "boolean") equal = asBoolean(left) === asBoolean(right);
      else if (typeof left === "number" || typeof right === "number") equal = asNumber(left) === asNumber(right);
      else equal = asString(left) === asString(right);
      return op === "=" ? equal : !equal;
    }
    const a = asNumber(left), b = asNumber(right);
    return op === "<" ? a < b : op === ">" ? a > b : op === "<=" ? a <= b : a >= b;
  }

  function xpathRound(value) { return value >= -0.5 && value < 0 ? -0 : Math.floor(value + 0.5); }
  function callFunction(name, args, context) {
    const values = () => args.map(arg => evaluate(arg, context));
    if (name.includes(":")) throw new DOMException("Unsupported XPath function namespace.", "NamespaceError");
    if (name === "last") return context.size;
    if (name === "position") return context.position;
    if (name === "count") { const value = evaluate(args[0], context); if (!isNodeSet(value)) syntax(); return value.length; }
    if (name === "local-name" || name === "namespace-uri" || name === "name") {
      const value = args.length ? evaluate(args[0], context) : [context.node];
      if (!isNodeSet(value)) syntax();
      const node = ordered(value)[0];
      if (!node) return "";
      return name === "local-name" ? (node.localName || "") : name === "namespace-uri" ? (node.namespaceURI || "") : (node.nodeName || "");
    }
    if (name === "string") return asString(args.length ? evaluate(args[0], context) : [context.node]);
    if (name === "concat") return values().map(asString).join("");
    if (name === "starts-with") { const v = values(); return asString(v[0]).startsWith(asString(v[1])); }
    if (name === "contains") { const v = values(); return asString(v[0]).includes(asString(v[1])); }
    if (name === "substring-before") { const v = values(), a = asString(v[0]), b = asString(v[1]), i = a.indexOf(b); return i < 0 ? "" : a.slice(0, i); }
    if (name === "substring-after") { const v = values(), a = asString(v[0]), b = asString(v[1]), i = a.indexOf(b); return i < 0 ? "" : a.slice(i + b.length); }
    if (name === "substring") {
      const v = values(), text = asString(v[0]), start = xpathRound(asNumber(v[1]));
      if (v.length < 3) return text.slice(Math.max(start - 1, 0));
      const end = start + xpathRound(asNumber(v[2]));
      let result = "";
      for (let i = 0; i < text.length; i++) if (i + 1 >= start && i + 1 < end) result += text[i];
      return result;
    }
    if (name === "string-length") return asString(args.length ? evaluate(args[0], context) : [context.node]).length;
    if (name === "normalize-space") return asString(args.length ? evaluate(args[0], context) : [context.node]).replace(/^[\x20\x09\x0d\x0a]+|[\x20\x09\x0d\x0a]+$/g, "").replace(/[\x20\x09\x0d\x0a]+/g, " ");
    if (name === "translate") {
      const v = values(), text = asString(v[0]), from = asString(v[1]), to = asString(v[2]);
      let result = "";
      for (const char of text) { const i = from.indexOf(char); if (i < 0) result += char; else if (i < to.length) result += to[i]; }
      return result;
    }
    if (name === "boolean") return asBoolean(evaluate(args[0], context));
    if (name === "not") return !asBoolean(evaluate(args[0], context));
    if (name === "true") return true;
    if (name === "false") return false;
    if (name === "number") return asNumber(args.length ? evaluate(args[0], context) : [context.node]);
    if (name === "sum") { const value = evaluate(args[0], context); if (!isNodeSet(value)) syntax(); return value.reduce((sum, node) => sum + asNumber(stringValue(node)), 0); }
    if (name === "floor") return Math.floor(asNumber(evaluate(args[0], context)));
    if (name === "ceiling") return Math.ceil(asNumber(evaluate(args[0], context)));
    if (name === "round") return xpathRound(asNumber(evaluate(args[0], context)));
    if (name === "id") {
      const value = evaluate(args[0], context);
      const strings = isNodeSet(value) ? value.map(stringValue) : [asString(value)];
      const ids = new Set(strings.flatMap(text => text.trim().split(/[\x20\x09\x0d\x0a]+/)).filter(Boolean));
      return descendants(rootOf(context.node), true).filter(node => node.nodeType === 1 && ids.has(node.getAttribute("id")));
    }
    if (name === "lang") {
      const language = asciiLower(asString(evaluate(args[0], context)));
      let node = context.node.nodeType === 2 ? context.node.ownerElement : context.node;
      while (node && node.nodeType !== 9) {
        if (node.nodeType === 1) {
          const value = node.getAttributeNS(XML_NS, "lang");
          if (value !== null) { const actual = asciiLower(value); return actual === language || actual.startsWith(language + "-"); }
        }
        node = node.parentNode;
      }
      return false;
    }
    syntax("Unknown XPath function '" + name + "'.");
  }

  function evaluate(ast, context) {
    if (ast.kind === "variable") return [];
    if (ast.kind === "literal" || ast.kind === "number") return ast.value;
    if (ast.kind === "unary") return -asNumber(evaluate(ast.value, context));
    if (ast.kind === "function") return callFunction(ast.name, ast.args, context);
    if (ast.kind === "filter") {
      const value = evaluate(ast.base, context);
      if (!isNodeSet(value)) syntax("XPath predicates require a node-set.");
      return filterPredicates(ordered(value), ast.predicates, context);
    }
    if (ast.kind === "path" || ast.kind === "pathFrom") {
      let input;
      if (ast.kind === "pathFrom") {
        input = evaluate(ast.base, context);
        if (!isNodeSet(input)) syntax("XPath paths require a node-set.");
      } else input = ast.absolute ? [rootOf(context.node)] : [context.node];
      return applySteps(input, ast.steps, context);
    }
    if (ast.kind === "binary") {
      if (ast.op === "or") return asBoolean(evaluate(ast.left, context)) || asBoolean(evaluate(ast.right, context));
      if (ast.op === "and") return asBoolean(evaluate(ast.left, context)) && asBoolean(evaluate(ast.right, context));
      const left = evaluate(ast.left, context), right = evaluate(ast.right, context);
      if (["=", "!=", "<", ">", "<=", ">="].includes(ast.op)) return compare(left, right, ast.op);
      if (ast.op === "|") { if (!isNodeSet(left) || !isNodeSet(right)) syntax(); return ordered([...left, ...right]); }
      const a = asNumber(left), b = asNumber(right);
      return ast.op === "+" ? a + b : ast.op === "-" ? a - b : ast.op === "*" ? a * b : ast.op === "div" ? a / b : a % b;
    }
    syntax();
  }

  const RESULT_TOKEN = {};
  class XPathResult {
    constructor(token, value, type, doc) {
      if (token !== RESULT_TOKEN) throw new TypeError("Illegal constructor");
      this.resultType = type;
      this._value = value;
      this._index = 0;
      this._document = doc;
      this._version = doc ? versions.get(doc) || 0 : 0;
      if ((type === 4 || type === 5) && doc) {
        iteratorDocuments.add(doc);
        hasIteratorDocuments = true;
      }
    }
    get numberValue() { if (this.resultType !== 1) throw new TypeError("Result is not a number"); return this._value; }
    get stringValue() { if (this.resultType !== 2) throw new TypeError("Result is not a string"); return this._value; }
    get booleanValue() { if (this.resultType !== 3) throw new TypeError("Result is not a boolean"); return this._value; }
    get singleNodeValue() { if (this.resultType !== 8 && this.resultType !== 9) throw new TypeError("Result is not a single node"); return this._value[0] || null; }
    get snapshotLength() { if (this.resultType !== 6 && this.resultType !== 7) throw new TypeError("Result is not a snapshot"); return this._value.length; }
    get invalidIteratorState() {
      return (this.resultType === 4 || this.resultType === 5) && !!this._document && (versions.get(this._document) || 0) !== this._version;
    }
    iterateNext() {
      if (this.resultType !== 4 && this.resultType !== 5) throw new TypeError("Result is not an iterator");
      if (this.invalidIteratorState) throw new DOMException("The document has changed.", "InvalidStateError");
      return this._value[this._index++] || null;
    }
    snapshotItem(index) {
      if (this.resultType !== 6 && this.resultType !== 7) throw new TypeError("Result is not a snapshot");
      return this._value[Number(index) >>> 0] || null;
    }
  }
  const constants = {
    ANY_TYPE: 0, NUMBER_TYPE: 1, STRING_TYPE: 2, BOOLEAN_TYPE: 3,
    UNORDERED_NODE_ITERATOR_TYPE: 4, ORDERED_NODE_ITERATOR_TYPE: 5,
    UNORDERED_NODE_SNAPSHOT_TYPE: 6, ORDERED_NODE_SNAPSHOT_TYPE: 7,
    ANY_UNORDERED_NODE_TYPE: 8, FIRST_ORDERED_NODE_TYPE: 9,
  };
  for (const [name, value] of Object.entries(constants)) {
    Object.defineProperty(XPathResult, name, { value, enumerable: true });
    Object.defineProperty(XPathResult.prototype, name, { value, enumerable: true });
  }

  function makeResult(value, requested, doc) {
    let type = Number(requested);
    if (!Number.isInteger(type) || type < 0 || type > 9) throw new TypeError("Invalid XPath result type");
    if (type === 0) type = isNodeSet(value) ? 4 : typeof value === "number" ? 1 : typeof value === "string" ? 2 : 3;
    if (type === 1) value = asNumber(value);
    else if (type === 2) value = asString(value);
    else if (type === 3) value = asBoolean(value);
    else {
      if (!isNodeSet(value)) throw new TypeError("The XPath result is not a node-set");
      value = ordered(value);
    }
    return new XPathResult(RESULT_TOKEN, value, type, doc);
  }

  function validateContext(node) {
    if (!(node instanceof Node) || ![1, 2, 3, 4, 7, 8, 9].includes(node.nodeType)) {
      throw new DOMException("The context node type is not supported.", "NotSupportedError");
    }
  }
  function evaluateCompiled(ast, resolver, contextNode, type = 0) {
    validateContext(contextNode);
    const doc = ownerDocument(contextNode);
    const value = evaluate(ast, { node: contextNode, position: 1, size: 1, document: doc, resolver, namespaces: new Map() });
    return makeResult(value, type, doc);
  }

  const EXPRESSION_TOKEN = {};
  class XPathExpression {
    constructor(token, ast, resolver) {
      if (token !== EXPRESSION_TOKEN) throw new TypeError("Illegal constructor");
      this._ast = ast; this._resolver = resolver;
    }
    evaluate(contextNode, type = 0, result = null) {
      return evaluateCompiled(this._ast, this._resolver, contextNode, type);
    }
  }
  class XPathEvaluator {
    constructor() { if (!new.target) throw new TypeError("XPathEvaluator requires new"); }
    createExpression(expression, resolver = null) {
      return new XPathExpression(EXPRESSION_TOKEN, new Parser(expression).parse(), resolver);
    }
    createNSResolver(nodeResolver) {
      if (!(nodeResolver instanceof Node)) throw new TypeError("The resolver must be a Node");
      return nodeResolver;
    }
    evaluate(expression, contextNode, resolver = null, type = 0, result = null) {
      return evaluateCompiled(new Parser(expression).parse(), resolver, contextNode, type);
    }
  }

  Object.defineProperties(Document.prototype, {
    createExpression: { configurable: true, enumerable: true, writable: true, value: XPathEvaluator.prototype.createExpression },
    createNSResolver: { configurable: true, enumerable: true, writable: true, value: XPathEvaluator.prototype.createNSResolver },
    evaluate: { configurable: true, enumerable: true, writable: true, value: XPathEvaluator.prototype.evaluate },
  });
  globalThis.XPathEvaluator = XPathEvaluator;
  globalThis.XPathExpression = XPathExpression;
  globalThis.XPathResult = XPathResult;
})();
