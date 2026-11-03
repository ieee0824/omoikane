// Target-surface control for the compact worker bootstrap (Issue #1689).
// This adapter alone does not reduce parsing or establish full DOM isolation.
// Window and ServiceWorker do not include this source fragment.
(() => {
  const installWorker = globalThis.__omoikane_install_worker_global;
  const apply = Reflect.apply;
  const remove = Reflect.deleteProperty;
  const TypeErrorIntrinsic = TypeError;
  const domGlobals = [
    "Highlight",
    "HighlightRegistry",
    "XPathEvaluator",
    "XPathExpression",
    "XPathResult",
    "__omoikane_find_in_page",
    "CDATASection",
    "Comment",
    "DOMParser",
    "DocumentType",
    "HTMLAnchorElement",
    "HTMLAreaElement",
    "HTMLDataListElement",
    "HTMLDetailsElement",
    "HTMLDialogElement",
    "HTMLEmbedElement",
    "HTMLFieldSetElement",
    "HTMLHeadElement",
    "HTMLHtmlElement",
    "HTMLLabelElement",
    "HTMLLinkElement",
    "HTMLMapElement",
    "HTMLMetaElement",
    "HTMLMeterElement",
    "HTMLObjectElement",
    "HTMLOptGroupElement",
    "HTMLOutputElement",
    "HTMLParagraphElement",
    "HTMLPictureElement",
    "HTMLProgressElement",
    "HTMLSelectedContentElement",
    "HTMLSlotElement",
    "HTMLSourceElement",
    "HTMLStyleElement",
    "HTMLTableCellElement",
    "HTMLTableElement",
    "HTMLTableRowElement",
    "HTMLTableSectionElement",
    "HTMLTrackElement",
    "HTMLUnknownElement",
    "Image",
    "MathMLAnchorElement",
    "MathMLElement",
    "NodeIterator",
    "Option",
    "ProcessingInstruction",
    "SVGCircleElement",
    "SVGEllipseElement",
    "SVGGeometryElement",
    "SVGGraphicsElement",
    "SVGImageElement",
    "SVGLineElement",
    "SVGPathElement",
    "SVGPolygonElement",
    "SVGPolylineElement",
    "SVGRectElement",
    "SVGTSpanElement",
    "SVGTextContentElement",
    "SVGTextElement",
    "SVGTextPathElement",
    "SVGTextPositioningElement",
    "Selection",
    "TreeWalker",
    "XMLSerializer",
    "getSelection"
];
  globalThis.__omoikane_install_worker_global = function(url, workerId) {
    const result = apply(installWorker, this, [url, workerId]);
    for (const name of domGlobals) {
      if (!remove(globalThis, name)) {
        throw new TypeErrorIntrinsic("Cannot remove worker DOM global: " + name);
      }
    }
    return result;
  };
})();
