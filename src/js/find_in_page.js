// Browser-owned find state. This runs after the DOM bootstrap in each Realm.
(() => {
  let query = "";
  let matches = [];
  let active = -1;
  let highlightedRange = null;
  let highlightedDocument = null;
  const originalSelections = new Map();
  let searchedDocument = null;
  let searchGeneration = null;
  function clearHighlight(restoreOriginal) {
    if (highlightedDocument) {
      const selection = highlightedDocument.getSelection();
      if (selection.rangeCount && selection.getRangeAt(0) === highlightedRange) {
        selection.removeAllRanges();
      }
    }
    highlightedRange = null;
    highlightedDocument = null;
    if (restoreOriginal) {
      for (const [document, range] of originalSelections) {
        if (!range) continue;
        try {
          const selection = document.getSelection();
          selection.removeAllRanges();
          selection.addRange(range);
        } catch (_) { /* the original node was removed */ }
      }
      originalSelections.clear();
    }
  }

  function childrenInOrder(node) {
    if (node.nodeType !== 1) return Array.from(node.childNodes || []);
    const tag = node.localName;
    if (tag === "iframe") {
      const child = node.contentDocument;
      return child ? [child] : [];
    }
    if (node.shadowRoot) return [node.shadowRoot];
    if (tag === "slot") {
      const assigned = node.assignedNodes({ flatten: true });
      if (assigned.length) return assigned;
    }
    return Array.from(node.childNodes || []);
  }

  function textCandidates() {
    const result = [];
    const stack = [{ node: globalThis.document, visible: true }];
    const visited = new Set();
    while (stack.length) {
      const { node, visible: parentVisible, endBoundary } = stack.pop();
      if (endBoundary) {
        result.push(null);
        continue;
      }
      if (!node || visited.has(node)) continue;
      visited.add(node);
      let visible = parentVisible;
      let boundary = false;
      if (node.nodeType === 1) {
        const tag = node.localName;
        if (tag === "script" || tag === "style" || tag === "template" || tag === "noscript") continue;
        const view = node.ownerDocument?.defaultView;
        if (!view) continue;
        const style = view.getComputedStyle(node);
        if (style.display === "none" || style.contentVisibility === "hidden") continue;
        visible = style.visibility !== "hidden" && style.visibility !== "collapse";
        if (!visible && tag === "iframe") continue;
        boundary = tag === "iframe" || tag === "br" ||
          (style.display !== "inline" && style.display !== "contents");
      } else if (node.nodeType === 3 && node.data) {
        if (visible) result.push(node);
        continue;
      } else if (node.nodeType === 11 && node.host) {
        boundary = true;
      }
      if (boundary) result.push(null);
      const children = childrenInOrder(node);
      if (boundary) stack.push({ endBoundary: true });
      for (let i = children.length - 1; i >= 0; i--) stack.push({ node: children[i], visible });
    }
    return result;
  }

  function buildMatches(value) {
    if (!value) return [];
    const escaped = value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    const expression = new RegExp(escaped, "giu");
    const result = [];
    let nodes = [];
    function searchRun() {
      if (!nodes.length) return;
      const parts = [];
      const offsets = [];
      let length = 0;
      for (const node of nodes) {
        offsets.push(length);
        parts.push(node.data);
        length += node.data.length;
      }
      const data = parts.join("");
      function nodeIndex(offset) {
        let low = 0;
        let high = offsets.length;
        while (low + 1 < high) {
          const middle = (low + high) >> 1;
          if (offsets[middle] <= offset) low = middle;
          else high = middle;
        }
        return low;
      }
      expression.lastIndex = 0;
      for (let found; (found = expression.exec(data)); ) {
        const startOffset = found.index;
        const endOffset = found.index + found[0].length;
        const startIndex = nodeIndex(startOffset);
        const endIndex = nodeIndex(endOffset - 1);
        result.push({
          startNode: nodes[startIndex], start: startOffset - offsets[startIndex],
          endNode: nodes[endIndex], end: endOffset - offsets[endIndex]
        });
      }
      nodes = [];
    }
    for (const node of textCandidates()) {
      if (node) nodes.push(node);
      else searchRun();
    }
    searchRun();
    return result;
  }

  function showActive() {
    clearHighlight(false);
    if (active < 0 || active >= matches.length) return;
    const match = matches[active];
    const doc = match.startNode.ownerDocument;
    const selection = doc.getSelection();
    if (!originalSelections.has(doc)) {
      originalSelections.set(doc, selection.rangeCount ? selection.getRangeAt(0) : null);
    }
    const range = doc.createRange();
    range.setStart(match.startNode, match.start);
    range.setEnd(match.endNode, match.end);
    selection.removeAllRanges();
    selection.addRange(range);
    highlightedRange = range;
    highlightedDocument = doc;
    const parent = match.startNode.parentElement || match.startNode.parentNode?.host;
    if (parent?.scrollIntoView) parent.scrollIntoView({ block: "center", inline: "nearest" });
    for (let frame = doc.defaultView?.frameElement; frame; frame = frame.ownerDocument.defaultView?.frameElement) {
      frame.scrollIntoView({ block: "center", inline: "nearest" });
    }
  }

  function result() {
    return { query, matchCount: matches.length, activeMatchOrdinal: active + 1 };
  }

  Object.defineProperty(globalThis, "__omoikane_find_in_page", {
    configurable: false,
    value(action, value = "") {
      if (action === "stop") {
        clearHighlight(true);
        query = "";
        matches = [];
        active = -1;
        searchedDocument = null;
        searchGeneration = null;
        return result();
      }
      if (action !== "start" && action !== "next" && action !== "previous" && action !== "status") {
        throw new TypeError("Unknown find-in-page action");
      }
      if (action === "start") {
        clearHighlight(true);
        query = String(value);
        active = -1;
      }
      const previous = matches[active];
      const document = globalThis.document;
      const generation = __omoikane_layout_metrics_generation();
      if (action === "start" || document !== searchedDocument || generation !== searchGeneration) {
        matches = buildMatches(query);
        searchedDocument = document;
      }
      if (!matches.length) {
        clearHighlight(true);
        active = -1;
        searchGeneration = __omoikane_layout_metrics_generation();
        return result();
      }
      const previousIndex = previous
        ? matches.findIndex(item => item.startNode === previous.startNode && item.start === previous.start)
        : -1;
      if (action === "start") active = 0;
      else if (action === "next") active = (previousIndex + 1) % matches.length;
      else if (action === "previous") active = previousIndex < 0
        ? matches.length - 1 : (previousIndex + matches.length - 1) % matches.length;
      else active = previousIndex;
      const current = matches[active];
      const unchangedRange = previous && current &&
        previous.startNode === current.startNode && previous.start === current.start &&
        previous.endNode === current.endNode && previous.end === current.end;
      if (active < 0) clearHighlight(true);
      else if (action !== "status" || !unchangedRange) showActive();
      searchGeneration = __omoikane_layout_metrics_generation();
      return result();
    }
  });
})();
