// DOM Standard pieces implemented in JavaScript: DOMException constants,
// Attr / NamedNodeMap, DOMImplementation / DocumentType, CharacterData
// editing, AbstractRange / StaticRange / Range, Selection, TreeWalker /
// NodeIterator, XPath (XPathEvaluator / XPathExpression / XPathResult),
// XSLTProcessor (inert), moveBefore, MutationRecord attributes.
(function () {
  'use strict';
  const g = globalThis;
  const N = g.__rhNative;
  const define = (name, value) => Object.defineProperty(g, name, { value, writable: true, configurable: true });
  const hidden = (o, k, v) => Object.defineProperty(o, k, { value: v, writable: true, configurable: true });
  const tag = (C, name) => Object.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  const S = Symbol('dom');
  const ILLEGAL = () => { throw new TypeError('Illegal constructor'); };
  const domError = (msg, name) => new g.DOMException(msg, name);
  const method = (C, name, fn, force) => { if (C && (force || !Object.prototype.hasOwnProperty.call(C.prototype, name))) hidden(C.prototype, name, fn); };
  const acc = (C, name, get, set, force) => {
    if (!C || (!force && Object.prototype.hasOwnProperty.call(C.prototype, name))) return;
    Object.defineProperty(C.prototype, name, { get, set, configurable: true, enumerable: true });
  };
  const consts = (C, map) => { for (const [k, v] of Object.entries(map)) { if (!Object.prototype.hasOwnProperty.call(C, k)) Object.defineProperty(C, k, { value: v, enumerable: true }); if (!Object.prototype.hasOwnProperty.call(C.prototype, k)) Object.defineProperty(C.prototype, k, { value: v, enumerable: true }); } };

  // ------------------------------------------------------- DOMException
  const DE = g.DOMException;
  const CODES = { IndexSizeError: 1, DOMStringSizeError: 2, HierarchyRequestError: 3, WrongDocumentError: 4, InvalidCharacterError: 5, NoDataAllowedError: 6, NoModificationAllowedError: 7, NotFoundError: 8, NotSupportedError: 9, InUseAttributeError: 10, InvalidStateError: 11, SyntaxError: 12, InvalidModificationError: 13, NamespaceError: 14, InvalidAccessError: 15, ValidationError: 16, TypeMismatchError: 17, SecurityError: 18, NetworkError: 19, AbortError: 20, URLMismatchError: 21, QuotaExceededError: 22, TimeoutError: 23, InvalidNodeTypeError: 24, DataCloneError: 25 };
  consts(DE, { INDEX_SIZE_ERR: 1, DOMSTRING_SIZE_ERR: 2, HIERARCHY_REQUEST_ERR: 3, WRONG_DOCUMENT_ERR: 4, INVALID_CHARACTER_ERR: 5, NO_DATA_ALLOWED_ERR: 6, NO_MODIFICATION_ALLOWED_ERR: 7, NOT_FOUND_ERR: 8, NOT_SUPPORTED_ERR: 9, INUSE_ATTRIBUTE_ERR: 10, INVALID_STATE_ERR: 11, SYNTAX_ERR: 12, INVALID_MODIFICATION_ERR: 13, NAMESPACE_ERR: 14, INVALID_ACCESS_ERR: 15, VALIDATION_ERR: 16, TYPE_MISMATCH_ERR: 17, SECURITY_ERR: 18, NETWORK_ERR: 19, ABORT_ERR: 20, URL_MISMATCH_ERR: 21, QUOTA_EXCEEDED_ERR: 22, TIMEOUT_ERR: 23, INVALID_NODE_TYPE_ERR: 24, DATA_CLONE_ERR: 25 });
  if (!Object.prototype.hasOwnProperty.call(DE.prototype, 'code')) {
    Object.defineProperty(DE.prototype, 'code', { get() { return CODES[this.name] || 0; }, configurable: true, enumerable: true });
  }
  class QuotaExceededError extends DE {
    constructor(message = '', options = {}) {
      super(message, 'QuotaExceededError');
      const q = options.quota, r = options.requested;
      if ((q !== undefined && q < 0) || (r !== undefined && r < 0)) throw new RangeError('quota and requested must be non-negative');
      hidden(this, S, { quota: q ?? null, requested: r ?? null });
    }
    get quota() { return this[S] ? this[S].quota : null; }
    get requested() { return this[S] ? this[S].requested : null; }
  }
  tag(QuotaExceededError, 'QuotaExceededError');
  define('QuotaExceededError', QuotaExceededError);

  // --------------------------------------------------- Node / NodeFilter
  consts(g.Node, { ENTITY_REFERENCE_NODE: 5, ENTITY_NODE: 6, NOTATION_NODE: 12, DOCUMENT_POSITION_DISCONNECTED: 1, DOCUMENT_POSITION_PRECEDING: 2, DOCUMENT_POSITION_FOLLOWING: 4, DOCUMENT_POSITION_CONTAINS: 8, DOCUMENT_POSITION_CONTAINED_BY: 16, DOCUMENT_POSITION_IMPLEMENTATION_SPECIFIC: 32 });
  const NF = { FILTER_ACCEPT: 1, FILTER_REJECT: 2, FILTER_SKIP: 3, SHOW_ALL: 0xFFFFFFFF, SHOW_ELEMENT: 0x1, SHOW_ATTRIBUTE: 0x2, SHOW_TEXT: 0x4, SHOW_CDATA_SECTION: 0x8, SHOW_ENTITY_REFERENCE: 0x10, SHOW_ENTITY: 0x20, SHOW_PROCESSING_INSTRUCTION: 0x40, SHOW_COMMENT: 0x80, SHOW_DOCUMENT: 0x100, SHOW_DOCUMENT_TYPE: 0x200, SHOW_DOCUMENT_FRAGMENT: 0x400, SHOW_NOTATION: 0x800 };
  const NodeFilter = function NodeFilter() { ILLEGAL(); };
  for (const [k, v] of Object.entries(NF)) Object.defineProperty(NodeFilter, k, { value: v, enumerable: true });
  NodeFilter.prototype.acceptNode = function () { return 1; };
  define('NodeFilter', NodeFilter);

  // ------------------------------------------------------ CharacterData
  const CD = g.CharacterData;
  const u16 = (s) => s.length;
  method(CD, 'insertData', function (offset, data) { this.replaceData(offset, 0, data); });
  method(CD, 'deleteData', function (offset, count) { this.replaceData(offset, count, ''); });
  method(CD, 'replaceData', function (offset, count, data) {
    const d = this.data;
    offset = Number(offset) >>> 0; count = Number(count) >>> 0;
    if (offset > u16(d)) throw domError(`The offset ${offset} is greater than the node's length (${d.length}).`, 'IndexSizeError');
    this.data = d.slice(0, offset) + String(data) + d.slice(offset + count);
  });
  for (const C of [CD]) {
    acc(C, 'previousElementSibling', function () { let n = this.previousSibling; while (n && n.nodeType !== 1) n = n.previousSibling; return n; });
    acc(C, 'nextElementSibling', function () { let n = this.nextSibling; while (n && n.nodeType !== 1) n = n.nextSibling; return n; });
  }

  // ------------------------------------------------- Attr / NamedNodeMap
  const AttrC = g.Attr;
  const attrCache = new WeakMap();
  function attrNode(el, name) {
    let m = attrCache.get(el);
    if (!m) { m = new Map(); attrCache.set(el, m); }
    let a = m.get(name);
    if (!a) { a = Object.create(AttrC.prototype); hidden(a, S, { el, name, value: null }); m.set(name, a); }
    return a;
  }
  const attrState = (a) => {
    if (a[S]) return a[S];
    // Attr objects created natively carry own properties.
    return { el: a.ownerElement || null, name: a.name, value: a.value };
  };
  const ownName = (a) => attrState(a).name;
  for (const k of ['name', 'value', 'localName', 'nodeName', 'nodeValue', 'textContent', 'specified', 'ownerElement']) {
    if (Object.prototype.hasOwnProperty.call(AttrC.prototype, k)) delete AttrC.prototype[k];
  }
  acc(AttrC, 'name', function () { return ownName(this); }, undefined, true);
  acc(AttrC, 'localName', function () { const n = ownName(this); const i = n.indexOf(':'); return i >= 0 ? n.slice(i + 1) : n; }, undefined, true);
  acc(AttrC, 'prefix', function () { const n = ownName(this); const i = n.indexOf(':'); return i >= 0 ? n.slice(0, i) : null; }, undefined, true);
  acc(AttrC, 'namespaceURI', function () { const p = this.prefix; return p === 'xlink' ? 'http://www.w3.org/1999/xlink' : p === 'xml' ? 'http://www.w3.org/XML/1998/namespace' : p === 'xmlns' || ownName(this) === 'xmlns' ? 'http://www.w3.org/2000/xmlns/' : null; }, undefined, true);
  acc(AttrC, 'nodeName', function () { return ownName(this); }, undefined, true);
  const attrValueGet = function () { const s = attrState(this); if (s.el) { const v = s.el.getAttribute(s.name); if (v !== null) return v; } return s.value ?? ''; };
  const attrValueSet = function (v) { const s = attrState(this); v = String(v); if (s.el && s.el.hasAttribute(s.name)) s.el.setAttribute(s.name, v); else if (this[S]) this[S].value = v; };
  acc(AttrC, 'value', attrValueGet, attrValueSet, true);
  acc(AttrC, 'nodeValue', attrValueGet, attrValueSet, true);
  acc(AttrC, 'textContent', attrValueGet, attrValueSet, true);
  acc(AttrC, 'ownerElement', function () { const s = attrState(this); return s.el && s.el.hasAttribute(s.name) ? s.el : null; }, undefined, true);
  acc(AttrC, 'specified', function () { return true; }, undefined, true);
  acc(AttrC, 'nodeType', function () { return 2; }, undefined, true);
  acc(AttrC, 'parentNode', function () { return null; }, undefined, true);
  acc(AttrC, 'childNodes', function () { return Object.setPrototypeOf([], g.NodeList.prototype); }, undefined, true);
  if (g.Document) {
    method(g.Document, 'createAttribute', function (name) {
      name = String(name);
      if (!/^[^\s"'>/=\x00-\x1f]+$/.test(name)) throw domError(`The localName provided ('${name}') contains an invalid character.`, 'InvalidCharacterError');
      const a = Object.create(AttrC.prototype);
      hidden(a, S, { el: null, name: name.toLowerCase(), value: '' });
      return a;
    }, true);
    method(g.Document, 'createAttributeNS', function (ns, qname) { const a = Object.create(AttrC.prototype); hidden(a, S, { el: null, name: String(qname), value: '' }); return a; }, true);
  }
  const E = g.Element;
  method(E, 'getAttributeNode', function (name) { name = String(name).toLowerCase(); return this.hasAttribute(name) ? attrNode(this, name) : null; }, true);
  method(E, 'getAttributeNodeNS', function (ns, local) { return this.getAttributeNode(local); }, true);
  method(E, 'setAttributeNode', function (attr) {
    if (!(attr instanceof AttrC)) throw new TypeError("Failed to execute 'setAttributeNode' on 'Element': parameter 1 is not of type 'Attr'.");
    const s = attrState(attr);
    if (s.el && s.el !== this && s.el.hasAttribute(s.name)) throw domError("The node provided is an attribute node that is already an attribute of another Element; attribute nodes must be explicitly cloned.", 'InUseAttributeError');
    const old = this.hasAttribute(s.name) ? attrNode(this, s.name) : null;
    const oldValue = old ? old.value : null;
    const value = attr.value;
    this.setAttribute(s.name, value);
    if (attr[S]) { attr[S].el = this; }
    let m = attrCache.get(this); if (!m) { m = new Map(); attrCache.set(this, m); }
    if (old && old !== attr) { hidden(old, S, { el: null, name: s.name, value: oldValue }); }
    m.set(s.name, attr);
    return old === attr ? attr : old;
  }, true);
  method(E, 'setAttributeNodeNS', function (attr) { return this.setAttributeNode(attr); }, true);
  method(E, 'removeAttributeNode', function (attr) {
    const s = attrState(attr);
    if (s.el !== this || !this.hasAttribute(s.name)) throw domError("The node provided is owned by another element.", 'NotFoundError');
    const v = this.getAttribute(s.name);
    this.removeAttribute(s.name);
    if (attr[S]) { attr[S].el = null; attr[S].value = v; }
    return attr;
  }, true);

  class NamedNodeMap {
    constructor() { ILLEGAL(); }
    get length() { return this[S].getAttributeNames().length; }
    item(i) { const n = this[S].getAttributeNames()[i]; return n === undefined ? null : attrNode(this[S], n); }
    getNamedItem(name) { return this[S].getAttributeNode(name); }
    getNamedItemNS(ns, local) { return this[S].getAttributeNodeNS(ns, local); }
    setNamedItem(attr) { return this[S].setAttributeNode(attr); }
    setNamedItemNS(attr) { return this[S].setAttributeNode(attr); }
    removeNamedItem(name) { const a = this[S].getAttributeNode(name); if (!a) throw domError(`Failed to execute 'removeNamedItem' on 'NamedNodeMap': No item with name '${name}' was found.`, 'NotFoundError'); return this[S].removeAttributeNode(a); }
    removeNamedItemNS(ns, local) { return this.removeNamedItem(local); }
    *[Symbol.iterator]() { for (const n of this[S].getAttributeNames()) yield attrNode(this[S], n); }
  }
  tag(NamedNodeMap, 'NamedNodeMap');
  define('NamedNodeMap', NamedNodeMap);
  N.registerProto('NamedNodeMap', NamedNodeMap.prototype);
  const nnmCache = new WeakMap();
  acc(E, 'attributes', function () {
    let m = nnmCache.get(this);
    if (m) return m;
    const el = this;
    const base = Object.create(NamedNodeMap.prototype);
    hidden(base, S, el);
    m = new Proxy(base, {
      get(t, k) {
        if (typeof k === 'string') {
          if (/^\d+$/.test(k)) return t.item(+k) ?? undefined;
          if (!(k in t) && el.hasAttribute(k)) return attrNode(el, k.toLowerCase());
        }
        return Reflect.get(t, k, t);
      },
      has(t, k) { return typeof k === 'string' && /^\d+$/.test(k) ? +k < el.getAttributeNames().length : k in t || (typeof k === 'string' && el.hasAttribute(k)); },
      ownKeys(t) { return el.getAttributeNames().map((_, i) => String(i)).concat(Reflect.ownKeys(t)); },
      getOwnPropertyDescriptor(t, k) {
        if (typeof k === 'string' && /^\d+$/.test(k) && +k < el.getAttributeNames().length) return { value: t.item(+k), enumerable: true, configurable: true, writable: false };
        return Reflect.getOwnPropertyDescriptor(t, k);
      },
    });
    nnmCache.set(this, m);
    return m;
  }, undefined, true);

  // ---------------------------------------- DOMImplementation / DocumentType
  // DocumentType nodes are native (parsed `<!DOCTYPE>` and
  // createDocumentType); `document.doctype` / `compatMode` too.
  class DOMImplementation {
    constructor() { ILLEGAL(); }
    createDocumentType(name, publicId, systemId) {
      name = String(name);
      N.validateQName(name);
      return N.createDoctype(this[S], name, String(publicId), String(systemId));
    }
    createDocument(ns, qname, doctype) {
      // DOM §4.5.1: an XML document with the doctype and the element.
      const d = N.createXMLDocument();
      Object.setPrototypeOf(d, XMLDocument.prototype);
      qname = qname === null || qname === undefined ? '' : String(qname);
      const root = qname ? d.createElementNS(ns, qname) : null;
      if (doctype) d.appendChild(doctype);
      if (root) d.appendChild(root);
      return d;
    }
    createHTMLDocument(title) {
      const d = new g.DOMParser().parseFromString('<!doctype html><html><head></head><body></body></html>', 'text/html');
      if (title !== undefined) { const t = d.createElement('title'); t.textContent = String(title); d.head.appendChild(t); }
      return d;
    }
    hasFeature() { return true; }
  }
  tag(DOMImplementation, 'DOMImplementation');
  define('DOMImplementation', DOMImplementation);
  const impls = new WeakMap();
  acc(g.Document, 'implementation', function () { let i = impls.get(this); if (!i) { i = Object.create(DOMImplementation.prototype); hidden(i, S, this); impls.set(this, i); } return i; }, undefined, true);
  class XMLDocument extends g.Document { constructor() { super(); ILLEGAL(); } }
  Object.setPrototypeOf(XMLDocument.prototype, g.Document.prototype);
  tag(XMLDocument, 'XMLDocument');
  define('XMLDocument', XMLDocument);
  // HTMLDocument is the Document interface object (DOM §4.5 legacy alias).
  define('HTMLDocument', g.Document);
  if (g.ProcessingInstruction) {
    acc(g.ProcessingInstruction, 'target', function () { return this.nodeName; });
    for (const k of ['getAttribute', 'getAttributeNames', 'hasAttribute', 'hasAttributes', 'removeAttribute', 'setAttribute', 'toggleAttribute']) {
      method(g.ProcessingInstruction, k, function (name, value) {
        const m = this[S] || hidden(this, S, new Map()) && this[S];
        switch (k) {
          case 'getAttribute': return m.has(String(name)) ? m.get(String(name)) : null;
          case 'getAttributeNames': return Array.from(m.keys());
          case 'hasAttribute': return m.has(String(name));
          case 'hasAttributes': return m.size > 0;
          case 'removeAttribute': m.delete(String(name)); return undefined;
          case 'setAttribute': m.set(String(name), String(value)); return undefined;
          default: if (m.has(String(name)) && value !== true) { m.delete(String(name)); return false; } m.set(String(name), ''); return true;
        }
      });
    }
  }

  // ------------------------------------------------------- moveBefore
  for (const C of [g.Element, g.Document, g.DocumentFragment]) {
    method(C, 'moveBefore', function (node, child) {
      if (!(node instanceof g.Node)) throw new TypeError("Failed to execute 'moveBefore': parameter 1 is not of type 'Node'.");
      if (child !== null && child !== undefined && child.parentNode !== this) throw domError("Failed to execute 'moveBefore': The node before which the new node is to be inserted is not a child of this node.", 'NotFoundError');
      if (node.isConnected !== this.isConnected) throw domError("Failed to execute 'moveBefore': State-preserving atomic move cannot be performed on nodes participating in an invalid hierarchy.", 'HierarchyRequestError');
      this.insertBefore(node, child ?? null);
    });
  }
  // Scoped custom element registries: everything uses the global registry.
  for (const C of [g.Document, g.Element, g.ShadowRoot].filter(Boolean)) acc(C, 'customElementRegistry', function () { return this.nodeType === 1 && !this.localName.includes('-') && !this.shadowRoot ? null : g.customElements; });
  if (g.CustomElementRegistry) method(g.CustomElementRegistry, 'initialize', function (root) { if (root) this.upgrade(root); });

  // MutationRecord: natively built records carry own values.
  for (const [k, v] of Object.entries({ type: '', target: null, addedNodes: null, removedNodes: null, previousSibling: null, nextSibling: null, attributeName: null, attributeNamespace: null, oldValue: null })) {
    acc(g.MutationRecord, k, function () { return v; });
  }
  acc(g.AbortController, 'signal', function () { return null; });

  // ------------------------------------------------------------- ranges
  const index = (n) => { let i = 0; while ((n = n.previousSibling)) i++; return i; };
  const nodeLength = (n) => (n.nodeType === 10 ? 0 : n.nodeType === 3 || n.nodeType === 8 || n.nodeType === 7 || n.nodeType === 4 ? n.data.length : n.childNodes.length);
  const rootOf = (n) => { while (n.parentNode) n = n.parentNode; return n; };
  const isAncestor = (a, b) => { for (let n = b.parentNode; n; n = n.parentNode) if (n === a) return true; return false; };
  const isInclusiveAncestor = (a, b) => a === b || isAncestor(a, b);
  // Compare boundary points (node, offset): -1 before, 0 equal, 1 after.
  function comparePoints(nA, oA, nB, oB) {
    if (nA === nB) return oA === oB ? 0 : oA < oB ? -1 : 1;
    const pos = nB.compareDocumentPosition(nA);
    if (pos & 4) { // A follows B
      const r = comparePoints(nB, oB, nA, oA);
      return -r;
    }
    if (isAncestor(nA, nB)) {
      let child = nB;
      while (child.parentNode !== nA) child = child.parentNode;
      if (index(child) < oA) return 1;
    }
    return -1;
  }
  class AbstractRange {
    constructor() { ILLEGAL(); }
    get startContainer() { return this[S].sc; }
    get startOffset() { return this[S].so; }
    get endContainer() { return this[S].ec; }
    get endOffset() { return this[S].eo; }
    get collapsed() { return this[S].sc === this[S].ec && this[S].so === this[S].eo; }
  }
  tag(AbstractRange, 'AbstractRange');
  class StaticRange extends AbstractRange {
    constructor(init) {
      const r = Object.create(new.target ? new.target.prototype : StaticRange.prototype);
      if (!init || !init.startContainer || !init.endContainer) throw new TypeError("Failed to construct 'StaticRange': required member startContainer is undefined.");
      for (const c of [init.startContainer, init.endContainer]) if (c.nodeType === 10 || c.nodeType === 2) throw domError("Failed to construct 'StaticRange': The supplied node is invalid.", 'InvalidNodeTypeError');
      hidden(r, S, { sc: init.startContainer, so: init.startOffset >>> 0, ec: init.endContainer, eo: init.endOffset >>> 0 });
      return r;
    }
  }
  tag(StaticRange, 'StaticRange');

  // A live range's boundary points are native live points: tree and text
  // mutations update them (DOM §5.5).
  class RangeState {
    constructor(node) { hidden(this, 'a', N.liveBoundary(node, 0)); hidden(this, 'b', N.liveBoundary(node, 0)); }
    get sc() { return this.a.node; }
    set sc(v) { this.a.node = v; }
    get so() { return this.a.offset; }
    set so(v) { this.a.offset = v; }
    get ec() { return this.b.node; }
    set ec(v) { this.b.node = v; }
    get eo() { return this.b.offset; }
    set eo(v) { this.b.offset = v; }
  }
  class Range extends AbstractRange {
    constructor() {
      const r = Object.create(new.target ? new.target.prototype : Range.prototype);
      hidden(r, S, new RangeState(document));
      return r;
    }
    get commonAncestorContainer() { let a = this[S].sc; while (!isInclusiveAncestor(a, this[S].ec)) a = a.parentNode; return a; }
    setStart(node, offset) { setBoundary(this, node, offset, true); }
    setEnd(node, offset) { setBoundary(this, node, offset, false); }
    setStartBefore(node) { const p = parentOrThrow(node); this.setStart(p, index(node)); }
    setStartAfter(node) { const p = parentOrThrow(node); this.setStart(p, index(node) + 1); }
    setEndBefore(node) { const p = parentOrThrow(node); this.setEnd(p, index(node)); }
    setEndAfter(node) { const p = parentOrThrow(node); this.setEnd(p, index(node) + 1); }
    collapse(toStart = false) { const s = this[S]; if (toStart) { s.ec = s.sc; s.eo = s.so; } else { s.sc = s.ec; s.so = s.eo; } }
    selectNode(node) { const p = parentOrThrow(node); const i = index(node); const s = this[S]; s.sc = p; s.so = i; s.ec = p; s.eo = i + 1; }
    selectNodeContents(node) { if (node.nodeType === 10) throw domError("Failed to execute 'selectNodeContents' on 'Range': The node provided is of type 'DocumentType'.", 'InvalidNodeTypeError'); const s = this[S]; s.sc = node; s.so = 0; s.ec = node; s.eo = nodeLength(node); }
    compareBoundaryPoints(how, src) {
      const s = this[S], o = src[S];
      if (rootOf(s.sc) !== rootOf(o.sc)) throw domError("The two Ranges are not in the same tree.", 'WrongDocumentError');
      switch (how) {
        case 0: return comparePoints(s.sc, s.so, o.sc, o.so);
        case 1: return comparePoints(s.ec, s.eo, o.sc, o.so);
        case 2: return comparePoints(s.ec, s.eo, o.ec, o.eo);
        case 3: return comparePoints(s.sc, s.so, o.ec, o.eo);
        default: throw domError('The comparison method provided must be one of START_TO_START, START_TO_END, END_TO_END, or END_TO_START.', 'NotSupportedError');
      }
    }
    deleteContents() { removeContents(this, false); }
    extractContents() { return removeContents(this, true); }
    cloneContents() {
      const frag = document.createDocumentFragment();
      const s = this[S];
      if (this.collapsed) return frag;
      if (s.sc === s.ec && isCharData(s.sc)) { const c = s.sc.cloneNode(); c.data = s.sc.data.slice(s.so, s.eo); frag.appendChild(c); return frag; }
      for (const n of containedTop(this)) frag.appendChild(cloneBounded(this, n));
      return frag;
    }
    insertNode(node) {
      const s = this[S];
      if ((s.sc.nodeType === 8 || s.sc.nodeType === 7) || (s.sc.nodeType === 3 && !s.sc.parentNode) || s.sc === node) throw domError("Failed to execute 'insertNode' on 'Range': The range start is in an invalid position.", 'HierarchyRequestError');
      let ref = null, parent;
      if (s.sc.nodeType === 3) { ref = s.sc.splitText(s.so); parent = ref.parentNode; }
      else { parent = s.sc; ref = s.sc.childNodes[s.so] || null; }
      if (ref === node) ref = node.nextSibling;
      const count = node.nodeType === 11 ? node.childNodes.length : 1;
      parent.insertBefore(node, ref);
      if (this.collapsed) { s.ec = parent; s.eo = (ref ? index(ref) : parent.childNodes.length); }
      void count;
    }
    surroundContents(newParent) {
      const s = this[S];
      for (const n of partiallyContained(this)) if (n.nodeType !== 3) throw domError("Failed to execute 'surroundContents' on 'Range': The Range has partially selected a non-Text node.", 'InvalidStateError');
      if ([9, 10, 11].includes(newParent.nodeType)) throw domError("Failed to execute 'surroundContents' on 'Range': The node provided is of invalid type.", 'InvalidNodeTypeError');
      const frag = this.extractContents();
      while (newParent.firstChild) newParent.firstChild.remove();
      this.insertNode(newParent);
      newParent.appendChild(frag);
      this.selectNode(newParent);
      void s;
    }
    cloneRange() { const r = new Range(); const s = this[S], t = r[S]; t.sc = s.sc; t.so = s.so; t.ec = s.ec; t.eo = s.eo; return r; }
    detach() {}
    isPointInRange(node, offset) {
      const s = this[S];
      if (rootOf(node) !== rootOf(s.sc)) return false;
      checkOffset(node, offset);
      return comparePoints(node, offset, s.sc, s.so) >= 0 && comparePoints(node, offset, s.ec, s.eo) <= 0;
    }
    comparePoint(node, offset) {
      const s = this[S];
      if (rootOf(node) !== rootOf(s.sc)) throw domError("The node provided and the Range are not in the same tree.", 'WrongDocumentError');
      checkOffset(node, offset);
      if (comparePoints(node, offset, s.sc, s.so) < 0) return -1;
      if (comparePoints(node, offset, s.ec, s.eo) > 0) return 1;
      return 0;
    }
    intersectsNode(node) {
      const s = this[S];
      if (rootOf(node) !== rootOf(s.sc)) return false;
      const p = node.parentNode;
      if (!p) return true;
      const i = index(node);
      return comparePoints(p, i, s.ec, s.eo) < 0 && comparePoints(p, i + 1, s.sc, s.so) > 0;
    }
    toString() {
      const s = this[S];
      if (s.sc === s.ec && s.sc.nodeType === 3) return s.sc.data.slice(s.so, s.eo);
      let out = '';
      if (s.sc.nodeType === 3) out += s.sc.data.slice(s.so);
      const walker = document.createTreeWalker(this.commonAncestorContainer, 4);
      let n;
      while ((n = walker.nextNode())) if (n !== s.sc && n !== s.ec && this.intersectsNode(n) && contained(this, n)) out += n.data;
      if (s.ec.nodeType === 3 && s.ec !== s.sc) out += s.ec.data.slice(0, s.eo);
      return out;
    }
    getClientRects() {
      const rects = [];
      const s = this[S];
      const nodes = [];
      if (s.sc === s.ec && (s.sc.nodeType === 3)) nodes.push(s.sc.parentNode);
      else for (const n of containedTop(this)) nodes.push(n.nodeType === 1 ? n : n.parentNode);
      for (const n of nodes) if (n && n.getBoundingClientRect) { const r = n.getBoundingClientRect(); if (r.width || r.height) rects.push(r); }
      if (!rects.length && s.sc.nodeType === 1 && s.sc.getBoundingClientRect) rects.push(s.sc.getBoundingClientRect());
      if (!rects.length && s.sc.parentNode && s.sc.parentNode.getBoundingClientRect) rects.push(s.sc.parentNode.getBoundingClientRect());
      return rects;
    }
    getBoundingClientRect() {
      const rs = this.getClientRects();
      if (!rs.length) return new g.DOMRect(0, 0, 0, 0);
      let l = Infinity, t = Infinity, r = -Infinity, b = -Infinity;
      for (const x of rs) { l = Math.min(l, x.left); t = Math.min(t, x.top); r = Math.max(r, x.right); b = Math.max(b, x.bottom); }
      return new g.DOMRect(l, t, r - l, b - t);
    }
    createContextualFragment(html) {
      const s = this[S];
      let ctx = s.sc.nodeType === 1 ? s.sc : s.sc.parentElement;
      if (!ctx || ctx.localName === 'html') ctx = document.body || document.createElement('body');
      const t = document.createElement('template');
      t.innerHTML = String(html);
      const frag = document.importNode(t.content, true);
      // Scripts created this way run when inserted (unlike innerHTML).
      for (const old of Array.from(frag.querySelectorAll('script'))) {
        const sc = document.createElement('script');
        for (const a of old.getAttributeNames()) sc.setAttribute(a, old.getAttribute(a));
        sc.textContent = old.textContent;
        old.replaceWith(sc);
      }
      return frag;
    }
  }
  consts(Range, { START_TO_START: 0, START_TO_END: 1, END_TO_END: 2, END_TO_START: 3 });
  tag(Range, 'Range');
  const isCharData = (n) => n.nodeType === 3 || n.nodeType === 8 || n.nodeType === 7 || n.nodeType === 4;
  function checkOffset(node, offset) {
    if (node.nodeType === 10) throw domError("The node provided is of type 'DocumentType'.", 'InvalidNodeTypeError');
    if (offset > nodeLength(node)) throw domError(`The offset ${offset} is larger than the node's length (${nodeLength(node)}).`, 'IndexSizeError');
  }
  function parentOrThrow(node) { const p = node.parentNode; if (!p) throw domError('the given Node has no parent.', 'InvalidNodeTypeError'); return p; }
  function setBoundary(range, node, offset, start) {
    if (!(node instanceof g.Node)) throw new TypeError("parameter 1 is not of type 'Node'.");
    offset = Number(offset) >>> 0;
    checkOffset(node, offset);
    const s = range[S];
    if (start) {
      s.sc = node; s.so = offset;
      if (rootOf(s.ec) !== rootOf(node) || comparePoints(node, offset, s.ec, s.eo) > 0) { s.ec = node; s.eo = offset; }
    } else {
      s.ec = node; s.eo = offset;
      if (rootOf(s.sc) !== rootOf(node) || comparePoints(node, offset, s.sc, s.so) < 0) { s.sc = node; s.so = offset; }
    }
  }
  function contained(range, n) {
    const s = range[S];
    const p = n.parentNode;
    if (!p) return false;
    const i = index(n);
    return comparePoints(p, i, s.sc, s.so) >= 0 && comparePoints(p, i + 1, s.ec, s.eo) <= 0;
  }
  function partiallyContained(range) {
    const s = range[S];
    const out = [];
    for (let n = s.sc; n; n = n.parentNode) if (!isInclusiveAncestor(n, s.ec)) out.push(n);
    for (let n = s.ec; n; n = n.parentNode) if (!isInclusiveAncestor(n, s.sc)) out.push(n);
    return out;
  }
  // Children of the common ancestor that are contained or partially
  // contained, in order.
  function containedTop(range) {
    const ca = range.commonAncestorContainer;
    if (isCharData(ca)) return [ca];
    return Array.from(ca.childNodes).filter((c) => range.intersectsNode(c));
  }
  function cloneBounded(range, n) {
    const s = range[S];
    if (contained(range, n)) return n.cloneNode(true);
    if (isCharData(n)) {
      const c = n.cloneNode();
      const from = n === s.sc ? s.so : 0, to = n === s.ec ? s.eo : n.data.length;
      c.data = n.data.slice(from, to);
      return c;
    }
    const c = n.cloneNode(false);
    for (const k of Array.from(n.childNodes)) if (range.intersectsNode(k)) c.appendChild(cloneBounded(range, k));
    return c;
  }
  // DOM §5.5 "extract" / "delete the contents" over explicit boundary
  // points (extraction recurses into partially contained children with a
  // subrange). Contained nodes are moved, not copied.
  function pointsContain(n, sc, so, ec, eo) {
    const p = n.parentNode;
    if (!p) return false;
    const i = index(n);
    return comparePoints(p, i, sc, so) >= 0 && comparePoints(p, i + 1, ec, eo) <= 0;
  }
  function extractPoints(sc, so, ec, eo) {
    const frag = (sc.nodeType === 9 ? sc : sc.ownerDocument).createDocumentFragment();
    if (sc === ec && so === eo) return frag;
    if (sc === ec && isCharData(sc)) {
      const c = sc.cloneNode(); c.data = sc.data.slice(so, eo); frag.appendChild(c);
      sc.replaceData(so, eo - so, '');
      return frag;
    }
    let ca = sc; while (!isInclusiveAncestor(ca, ec)) ca = ca.parentNode;
    const kids = Array.from(ca.childNodes);
    const firstPartial = isInclusiveAncestor(sc, ec) ? null : kids.find((c) => isInclusiveAncestor(c, sc)) || null;
    const lastPartial = isInclusiveAncestor(ec, sc) ? null : kids.find((c) => isInclusiveAncestor(c, ec)) || null;
    const contained = kids.filter((c) => pointsContain(c, sc, so, ec, eo));
    if (contained.some((c) => c.nodeType === 10)) throw domError("Failed to execute 'extractContents' on 'Range': The Range contains a doctype node.", 'HierarchyRequestError');
    if (firstPartial) {
      if (isCharData(firstPartial)) {
        const c = sc.cloneNode(); c.data = sc.data.slice(so); frag.appendChild(c);
        sc.replaceData(so, sc.data.length - so, '');
      } else {
        const c = firstPartial.cloneNode(false); frag.appendChild(c);
        c.appendChild(extractPoints(sc, so, firstPartial, nodeLength(firstPartial)));
      }
    }
    for (const n of contained) frag.appendChild(n);
    if (lastPartial) {
      if (isCharData(lastPartial)) {
        const c = ec.cloneNode(); c.data = ec.data.slice(0, eo); frag.appendChild(c);
        ec.replaceData(0, eo, '');
      } else {
        const c = lastPartial.cloneNode(false); frag.appendChild(c);
        c.appendChild(extractPoints(lastPartial, 0, ec, eo));
      }
    }
    return frag;
  }
  // Where a range collapses after its contents went away.
  function collapsePoint(sc, so, ec) {
    if (isInclusiveAncestor(sc, ec)) return [sc, so];
    let ref = sc;
    while (ref.parentNode && !isInclusiveAncestor(ref.parentNode, ec)) ref = ref.parentNode;
    return [ref.parentNode, index(ref) + 1];
  }
  function removeContents(range, extract) {
    const s = range[S];
    const sc = s.sc, so = s.so, ec = s.ec, eo = s.eo;
    if (sc === ec && so === eo) return extract ? (sc.nodeType === 9 ? sc : sc.ownerDocument).createDocumentFragment() : undefined;
    const [newNode, newOffset] = sc === ec && isCharData(sc) ? [sc, so] : collapsePoint(sc, so, ec);
    let frag;
    if (extract) frag = extractPoints(sc, so, ec, eo);
    else if (sc === ec && isCharData(sc)) sc.replaceData(so, eo - so, '');
    else {
      // Contained nodes (outermost only), then the partial text ends.
      const doomed = [];
      const collect = (n) => { for (const c of Array.from(n.childNodes)) { if (pointsContain(c, sc, so, ec, eo)) doomed.push(c); else if (isInclusiveAncestor(c, sc) || isInclusiveAncestor(c, ec)) collect(c); } };
      let ca = sc; while (!isInclusiveAncestor(ca, ec)) ca = ca.parentNode;
      collect(ca);
      if (isCharData(sc)) sc.replaceData(so, sc.data.length - so, '');
      for (const n of doomed) n.remove();
      if (isCharData(ec)) ec.replaceData(0, eo, '');
    }
    s.sc = newNode; s.so = newOffset; s.ec = newNode; s.eo = newOffset;
    return frag;
  }
  define('AbstractRange', AbstractRange);
  define('StaticRange', StaticRange);
  define('Range', Range);
  N.registerProto('Range', Range.prototype);
  method(g.Document, 'createRange', function () { const r = Object.create(Range.prototype); hidden(r, S, new RangeState(this)); return r; }, true);
  method(g.Document, 'caretRangeFromPoint', function (x, y) {
    const el = this.elementFromPoint(x, y);
    if (!el) return null;
    const r = new Range();
    r.setStart(el, 0);
    return r;
  }, true);
  method(g.Document, 'caretPositionFromPoint', function (x, y) {
    const el = this.elementFromPoint(x, y);
    if (!el) return null;
    return makeCaretPosition(el, 0);
  }, true);
  class CaretPosition {
    constructor() { ILLEGAL(); }
    get offsetNode() { return this[S].node; }
    get offset() { return this[S].offset; }
    getClientRect() { const r = new Range(); r.setStart(this[S].node, this[S].offset); return r.getBoundingClientRect(); }
  }
  tag(CaretPosition, 'CaretPosition');
  define('CaretPosition', CaretPosition);
  function makeCaretPosition(node, offset) { const c = Object.create(CaretPosition.prototype); hidden(c, S, { node, offset }); return c; }

  // ------------------------------------------------------------ Selection
  class Selection {
    constructor() { ILLEGAL(); }
    get anchorNode() { const r = this[S].range; return r ? (this[S].backward ? r.endContainer : r.startContainer) : null; }
    get anchorOffset() { const r = this[S].range; return r ? (this[S].backward ? r.endOffset : r.startOffset) : 0; }
    get focusNode() { const r = this[S].range; return r ? (this[S].backward ? r.startContainer : r.endContainer) : null; }
    get focusOffset() { const r = this[S].range; return r ? (this[S].backward ? r.startOffset : r.endOffset) : 0; }
    get isCollapsed() { const r = this[S].range; return !r || r.collapsed; }
    get rangeCount() { return this[S].range ? 1 : 0; }
    get type() { const r = this[S].range; return !r ? 'None' : r.collapsed ? 'Caret' : 'Range'; }
    get direction() { const r = this[S].range; return !r || r.collapsed ? 'none' : this[S].backward ? 'backward' : 'forward'; }
    getRangeAt(i) { if (i !== 0 || !this[S].range) throw domError(`Failed to execute 'getRangeAt' on 'Selection': ${i} is not a valid index.`, 'IndexSizeError'); return this[S].range; }
    addRange(r) { if (!this[S].range) { this[S].range = r; this[S].backward = false; changed(); } }
    removeRange(r) { if (this[S].range !== r) throw domError("Failed to execute 'removeRange' on 'Selection': The given range isn't in document.", 'NotFoundError'); this[S].range = null; changed(); }
    removeAllRanges() { if (this[S].range) { this[S].range = null; changed(); } }
    empty() { this.removeAllRanges(); }
    getComposedRanges() { const r = this[S].range; return r ? [new StaticRange({ startContainer: r.startContainer, startOffset: r.startOffset, endContainer: r.endContainer, endOffset: r.endOffset })] : []; }
    collapse(node, offset = 0) { if (node === null) { this.removeAllRanges(); return; } const r = new Range(); r.setStart(node, offset); this[S].range = r; this[S].backward = false; changed(); }
    setPosition(node, offset) { this.collapse(node, offset); }
    collapseToStart() { const r = this[S].range; if (!r) throw domError("Failed to execute 'collapseToStart' on 'Selection': there is no selection.", 'InvalidStateError'); this.collapse(r.startContainer, r.startOffset); }
    collapseToEnd() { const r = this[S].range; if (!r) throw domError("Failed to execute 'collapseToEnd' on 'Selection': there is no selection.", 'InvalidStateError'); this.collapse(r.endContainer, r.endOffset); }
    extend(node, offset = 0) {
      const r = this[S].range;
      if (!r) throw domError("Failed to execute 'extend' on 'Selection': This Selection object doesn't have any Ranges.", 'InvalidStateError');
      const an = this.anchorNode, ao = this.anchorOffset;
      const nr = new Range();
      if (comparePoints(node, offset, an, ao) < 0) { nr.setStart(node, offset); nr.setEnd(an, ao); this[S].backward = true; }
      else { nr.setStart(an, ao); nr.setEnd(node, offset); this[S].backward = false; }
      this[S].range = nr; changed();
    }
    setBaseAndExtent(an, ao, fn, fo) { this.collapse(an, ao); this.extend(fn, fo); }
    selectAllChildren(node) { const r = new Range(); r.selectNodeContents(node); this[S].range = r; this[S].backward = false; changed(); }
    modify() {}
    deleteFromDocument() { if (this[S].range) this[S].range.deleteContents(); }
    containsNode(node, allowPartial = false) {
      const r = this[S].range;
      if (!r || rootOf(node) !== rootOf(r.startContainer)) return false;
      if (allowPartial) return r.intersectsNode(node);
      const p = node.parentNode;
      if (!p) return false;
      const i = index(node);
      return comparePoints(p, i, r.startContainer, r.startOffset) >= 0 && comparePoints(p, i + 1, r.endContainer, r.endOffset) <= 0;
    }
    toString() { return this[S].range ? this[S].range.toString() : ''; }
  }
  tag(Selection, 'Selection');
  define('Selection', Selection);
  const selection = Object.create(Selection.prototype);
  hidden(selection, S, { range: null, backward: false });
  let selChangeQueued = false;
  function changed() {
    if (selChangeQueued) return;
    selChangeQueued = true;
    setTimeout(() => { selChangeQueued = false; document.dispatchEvent(new Event('selectionchange')); }, 0);
  }
  method(g.Document, 'getSelection', function () { return this === document ? selection : null; }, true);
  hidden(g, 'getSelection', function getSelection() { return selection; });
  if (g.ShadowRoot) method(g.ShadowRoot, 'getSelection', function () { return selection; }, true);

  // ------------------------------------------------ TreeWalker / NodeIterator
  const SHOW_BIT = (n) => 1 << (n.nodeType - 1);
  function acceptNode(it, node) {
    const st = it[S];
    if (st.active) throw domError('Filter function can\'t be recursive.', 'InvalidStateError');
    if (!(st.what & SHOW_BIT(node))) return 3;
    if (!st.filter) return 1;
    st.active = true;
    try {
      const r = typeof st.filter === 'function' ? st.filter.call(undefined, node) : st.filter.acceptNode(node);
      return Number(r);
    } finally { st.active = false; }
  }
  class TreeWalker {
    constructor() { ILLEGAL(); }
    get root() { return this[S].root; }
    get whatToShow() { return this[S].what; }
    get filter() { return this[S].filter; }
    get currentNode() { return this[S].current; }
    set currentNode(n) { if (!(n instanceof g.Node)) throw new TypeError("Failed to set the 'currentNode' property on 'TreeWalker': The provided value is not of type 'Node'."); this[S].current = n; }
    parentNode() {
      let n = this[S].current;
      while (n && n !== this[S].root) { n = n.parentNode; if (n && acceptNode(this, n) === 1) { this[S].current = n; return n; } }
      return null;
    }
    firstChild() { return traverseChildren(this, true); }
    lastChild() { return traverseChildren(this, false); }
    previousSibling() { return traverseSiblings(this, false); }
    nextSibling() { return traverseSiblings(this, true); }
    previousNode() {
      let node = this[S].current;
      while (node !== this[S].root) {
        let sib = node.previousSibling;
        while (sib) {
          node = sib;
          let r = acceptNode(this, node);
          while (r !== 2 && node.lastChild) { node = node.lastChild; r = acceptNode(this, node); }
          if (r === 1) { this[S].current = node; return node; }
          sib = node.previousSibling;
        }
        if (node === this[S].root || !node.parentNode) return null;
        node = node.parentNode;
        if (acceptNode(this, node) === 1) { this[S].current = node; return node; }
      }
      return null;
    }
    nextNode() {
      let node = this[S].current;
      let r = 1;
      for (;;) {
        while (r !== 2 && node.firstChild) {
          node = node.firstChild;
          r = acceptNode(this, node);
          if (r === 1) { this[S].current = node; return node; }
        }
        let sib = null, tmp = node;
        while (tmp) {
          if (tmp === this[S].root) return null;
          sib = tmp.nextSibling;
          if (sib) break;
          tmp = tmp.parentNode;
        }
        if (!sib) return null;
        node = sib;
        r = acceptNode(this, node);
        if (r === 1) { this[S].current = node; return node; }
      }
    }
  }
  function traverseChildren(w, first) {
    let node = first ? w[S].current.firstChild : w[S].current.lastChild;
    while (node) {
      const r = acceptNode(w, node);
      if (r === 1) { w[S].current = node; return node; }
      if (r === 3) { const c = first ? node.firstChild : node.lastChild; if (c) { node = c; continue; } }
      while (node) {
        const sib = first ? node.nextSibling : node.previousSibling;
        if (sib) { node = sib; break; }
        const p = node.parentNode;
        if (!p || p === w[S].root || p === w[S].current) return null;
        node = p;
      }
    }
    return null;
  }
  function traverseSiblings(w, next) {
    let node = w[S].current;
    if (node === w[S].root) return null;
    for (;;) {
      let sib = next ? node.nextSibling : node.previousSibling;
      while (sib) {
        node = sib;
        const r = acceptNode(w, node);
        if (r === 1) { w[S].current = node; return node; }
        sib = next ? node.firstChild : node.lastChild;
        if (r === 2 || !sib) sib = next ? node.nextSibling : node.previousSibling;
      }
      node = node.parentNode;
      if (!node || node === w[S].root) return null;
      if (acceptNode(w, node) === 1) return null;
    }
  }
  tag(TreeWalker, 'TreeWalker');
  class NodeIterator {
    constructor() { ILLEGAL(); }
    get root() { return this[S].root; }
    get referenceNode() { return this[S].ref.node; }
    get pointerBeforeReferenceNode() { return this[S].ref.before; }
    get whatToShow() { return this[S].what; }
    get filter() { return this[S].filter; }
    nextNode() { return iterate(this, true); }
    previousNode() { return iterate(this, false); }
    detach() {}
  }
  tag(NodeIterator, 'NodeIterator');
  function following(node, root) { if (node.firstChild) return node.firstChild; while (node && node !== root) { if (node.nextSibling) return node.nextSibling; node = node.parentNode; } return null; }
  function preceding(node, root) { if (node === root) return null; let p = node.previousSibling; if (p) { while (p.lastChild) p = p.lastChild; return p; } return node.parentNode; }
  // The reference and the candidate being filtered are live positions:
  // a filter that removes nodes moves them (DOM §6.1 pre-removing steps).
  function iterate(it, forward) {
    const st = it[S], ref = st.ref, cand = st.cand;
    cand.node = ref.node; cand.before = ref.before;
    for (;;) {
      let node = cand.node, before = cand.before;
      if (forward) { if (before) before = false; else { node = following(node, st.root); if (!node) return null; } }
      else { if (!before) before = true; else { node = preceding(node, st.root); if (!node) return null; } }
      cand.node = node; cand.before = before;
      if (acceptNode(it, node) === 1) { ref.node = cand.node; ref.before = cand.before; return node; }
    }
  }
  define('TreeWalker', TreeWalker);
  define('NodeIterator', NodeIterator);
  const filterOf = (f) => (f === undefined ? null : f);
  method(g.Document, 'createTreeWalker', function (root, what = 0xFFFFFFFF, filter = null) {
    if (!(root instanceof g.Node)) throw new TypeError("Failed to execute 'createTreeWalker' on 'Document': parameter 1 is not of type 'Node'.");
    const w = Object.create(TreeWalker.prototype);
    hidden(w, S, { root, what: what >>> 0, filter: filterOf(filter), current: root, active: false });
    return w;
  }, true);
  method(g.Document, 'createNodeIterator', function (root, what = 0xFFFFFFFF, filter = null) {
    if (!(root instanceof g.Node)) throw new TypeError("Failed to execute 'createNodeIterator' on 'Document': parameter 1 is not of type 'Node'.");
    const it = Object.create(NodeIterator.prototype);
    hidden(it, S, { root, what: what >>> 0, filter: filterOf(filter), ref: N.liveIteratorPosition(root, root, true), cand: N.liveIteratorPosition(root, root, true), active: false });
    return it;
  }, true);

  // ---------------------------------------------------------------- XPath
  // XPath 1.0: tokenizer, recursive-descent parser and evaluator over the
  // DOM (HTML documents: names match case-insensitively).
  const XP = { ANY_TYPE: 0, NUMBER_TYPE: 1, STRING_TYPE: 2, BOOLEAN_TYPE: 3, UNORDERED_NODE_ITERATOR_TYPE: 4, ORDERED_NODE_ITERATOR_TYPE: 5, UNORDERED_NODE_SNAPSHOT_TYPE: 6, ORDERED_NODE_SNAPSHOT_TYPE: 7, ANY_UNORDERED_NODE_TYPE: 8, FIRST_ORDERED_NODE_TYPE: 9 };
  function xtokenize(src) {
    const toks = [];
    const re = /\s*(\.\.|::|\/\/|!=|<=|>=|[()[\]@,|/.=<>*+-]|\$[\w.-]+(?::[\w.-]+)?|"[^"]*"|'[^']*'|\d+(?:\.\d*)?|\.\d+|(?:[A-Za-z_][\w.-]*:)?(?:[A-Za-z_][\w.-]*|\*))/y;
    let m;
    re.lastIndex = 0;
    while (re.lastIndex < src.length) {
      const before = re.lastIndex;
      if (/^\s*$/.test(src.slice(before))) break;
      m = re.exec(src);
      if (!m || re.lastIndex === before) throw domError(`Failed to execute 'evaluate' on 'Document': The string '${src}' is not a valid XPath expression.`, 'SyntaxError');
      toks.push(m[1]);
    }
    return toks;
  }
  const AXES = ['ancestor', 'ancestor-or-self', 'attribute', 'child', 'descendant', 'descendant-or-self', 'following', 'following-sibling', 'namespace', 'parent', 'preceding', 'preceding-sibling', 'self'];
  function xparse(src) {
    const t = xtokenize(src);
    let i = 0;
    const peek = (o = 0) => t[i + o];
    const next = () => t[i++];
    const expect = (v) => { if (t[i] !== v) throw domError(`The string '${src}' is not a valid XPath expression.`, 'SyntaxError'); i++; };
    // Disambiguate operator names vs. node names (XPath §3.7).
    const isOpContext = () => { const p = t[i - 1]; return p !== undefined && !['@', '::', '(', '[', ',', '/', '//', '|', '+', '-', '=', '!=', '<', '<=', '>', '>=', 'and', 'or', 'mod', 'div', '*_op'].includes(p); };
    function orExpr() { let l = andExpr(); while (peek() === 'or' && isOpContext()) { next(); l = { op: 'or', l, r: andExpr() }; } return l; }
    function andExpr() { let l = eqExpr(); while (peek() === 'and' && isOpContext()) { next(); l = { op: 'and', l, r: eqExpr() }; } return l; }
    function eqExpr() { let l = relExpr(); while (peek() === '=' || peek() === '!=') { const op = next(); l = { op, l, r: relExpr() }; } return l; }
    function relExpr() { let l = addExpr(); while (['<', '>', '<=', '>='].includes(peek())) { const op = next(); l = { op, l, r: addExpr() }; } return l; }
    function addExpr() { let l = mulExpr(); while (peek() === '+' || peek() === '-') { const op = next(); l = { op, l, r: mulExpr() }; } return l; }
    function mulExpr() {
      let l = unaryExpr();
      while ((peek() === '*' || peek() === 'div' || peek() === 'mod') && isOpContext()) { const op = next(); l = { op: op === '*' ? 'mul' : op, l, r: unaryExpr() }; }
      return l;
    }
    function unaryExpr() { if (peek() === '-') { next(); return { op: 'neg', e: unaryExpr() }; } return unionExpr(); }
    function unionExpr() { let l = pathExpr(); while (peek() === '|') { next(); l = { op: 'union', l, r: pathExpr() }; } return l; }
    function isStepStart(tk) { return tk !== undefined && (tk === '.' || tk === '..' || tk === '@' || tk === '*' || /^[A-Za-z_]/.test(tk)); }
    function pathExpr() {
      const tk = peek();
      if (tk === '/' || tk === '//') {
        next();
        const steps = tk === '//' ? [{ axis: 'descendant-or-self', test: { type: 'node' }, preds: [] }] : [];
        if (tk === '//' || isStepStart(peek())) steps.push(...relPath());
        return { op: 'path', abs: true, steps };
      }
      const isFilter = tk !== undefined && (tk[0] === '"' || tk[0] === "'" || /^[\d.]/.test(tk) && tk !== '.' && tk !== '..' || tk === '(' || tk[0] === '$' || (/^[A-Za-z_]/.test(tk) && peek(1) === '(' && !['node', 'text', 'comment', 'processing-instruction'].includes(tk)));
      if (isFilter) {
        let e = primary();
        const preds = [];
        while (peek() === '[') { next(); preds.push(orExpr()); expect(']'); }
        if (preds.length) e = { op: 'filter', e, preds };
        if (peek() === '/' || peek() === '//') {
          const sep = next();
          const steps = sep === '//' ? [{ axis: 'descendant-or-self', test: { type: 'node' }, preds: [] }] : [];
          steps.push(...relPath());
          return { op: 'path', from: e, steps };
        }
        return e;
      }
      return { op: 'path', abs: false, steps: relPath() };
    }
    function relPath() {
      const steps = [step()];
      while (peek() === '/' || peek() === '//') {
        if (next() === '//') steps.push({ axis: 'descendant-or-self', test: { type: 'node' }, preds: [] });
        steps.push(step());
      }
      return steps;
    }
    function step() {
      if (peek() === '.') { next(); return { axis: 'self', test: { type: 'node' }, preds: [] }; }
      if (peek() === '..') { next(); return { axis: 'parent', test: { type: 'node' }, preds: [] }; }
      let axis = 'child';
      if (peek() === '@') { next(); axis = 'attribute'; }
      else if (AXES.includes(peek()) && peek(1) === '::') { axis = next(); next(); }
      const name = next();
      if (name === undefined) throw domError(`The string '${src}' is not a valid XPath expression.`, 'SyntaxError');
      let test;
      if (['node', 'text', 'comment', 'processing-instruction'].includes(name) && peek() === '(') { next(); if (peek() !== ')') next(); expect(')'); test = { type: name }; }
      else test = { type: 'name', name };
      const preds = [];
      while (peek() === '[') { next(); preds.push(orExpr()); expect(']'); }
      return { axis, test, preds };
    }
    function primary() {
      const tk = next();
      if (tk === '(') { const e = orExpr(); expect(')'); return e; }
      if (tk[0] === '"' || tk[0] === "'") return { op: 'lit', v: tk.slice(1, -1) };
      if (/^[\d.]/.test(tk)) return { op: 'num', v: parseFloat(tk) };
      if (tk[0] === '$') return { op: 'var', name: tk.slice(1) };
      expect('(');
      const args = [];
      if (peek() !== ')') { args.push(orExpr()); while (peek() === ',') { next(); args.push(orExpr()); } }
      expect(')');
      return { op: 'call', name: tk, args };
    }
    const e = orExpr();
    if (i < t.length) throw domError(`The string '${src}' is not a valid XPath expression.`, 'SyntaxError');
    return e;
  }
  const isHTMLDoc = (n) => { const d = n.nodeType === 9 ? n : n.ownerDocument; return !d || !(d instanceof XMLDocument); };
  function nameMatches(node, name, attr) {
    if (name === '*') return attr ? true : node.nodeType === 1;
    const local = name.includes(':') ? name.split(':')[1] : name;
    if (attr) return node.name === name || node.localName === local;
    if (node.nodeType !== 1) return false;
    return isHTMLDoc(node) && (!node.namespaceURI || node.namespaceURI === 'http://www.w3.org/1999/xhtml') ? node.localName === local.toLowerCase() : node.localName === local || node.nodeName === name;
  }
  function testNode(node, test, axis) {
    switch (test.type) {
      case 'node': return true;
      case 'text': return node.nodeType === 3 || node.nodeType === 4;
      case 'comment': return node.nodeType === 8;
      case 'processing-instruction': return node.nodeType === 7;
      default: return axis === 'attribute' ? nameMatches(node, test.name, true) : nameMatches(node, test.name, false);
    }
  }
  function axisNodes(node, axis) {
    const out = [];
    const parentOf = (n) => (n.nodeType === 2 ? n.ownerElement : n.parentNode);
    switch (axis) {
      case 'self': out.push(node); break;
      case 'child': for (let c = node.firstChild; c; c = c.nextSibling) out.push(c); break;
      case 'parent': { const p = parentOf(node); if (p) out.push(p); break; }
      case 'ancestor': case 'ancestor-or-self': { if (axis === 'ancestor-or-self') out.push(node); for (let p = parentOf(node); p; p = p.parentNode) out.push(p); break; }
      case 'descendant': case 'descendant-or-self': {
        if (axis === 'descendant-or-self') out.push(node);
        const walk = (n) => { for (let c = n.firstChild; c; c = c.nextSibling) { out.push(c); walk(c); } };
        if (node.nodeType !== 2) walk(node);
        break;
      }
      case 'following-sibling': if (node.nodeType !== 2) for (let s = node.nextSibling; s; s = s.nextSibling) out.push(s); break;
      case 'preceding-sibling': if (node.nodeType !== 2) for (let s = node.previousSibling; s; s = s.previousSibling) out.push(s); break;
      case 'following': {
        let n = node.nodeType === 2 ? node.ownerElement : node;
        if (node.nodeType !== 2) { while (n && !n.nextSibling) n = n.parentNode; n = n && n.nextSibling; }
        else n = n && (n.firstChild || (() => { let m = n; while (m && !m.nextSibling) m = m.parentNode; return m && m.nextSibling; })());
        while (n) { out.push(n); if (n.firstChild) n = n.firstChild; else { while (n && !n.nextSibling) n = n.parentNode; n = n && n.nextSibling; } }
        break;
      }
      case 'preceding': {
        const anc = new Set();
        for (let p = parentOf(node); p; p = p.parentNode) anc.add(p);
        const root = rootOf(node.nodeType === 2 ? node.ownerElement || node : node);
        const all = [];
        const walk = (n) => { for (let c = n.firstChild; c; c = c.nextSibling) { all.push(c); walk(c); } };
        walk(root);
        const stop = node.nodeType === 2 ? node.ownerElement : node;
        for (const n of all) { if (n === stop) break; if (!anc.has(n)) out.push(n); }
        out.reverse();
        break;
      }
      case 'attribute': if (node.nodeType === 1) for (const a of node.attributes) out.push(a); break;
      default: break;
    }
    return out;
  }
  const REVERSE = new Set(['ancestor', 'ancestor-or-self', 'preceding', 'preceding-sibling']);
  function docOrder(nodes) {
    const uniq = Array.from(new Set(nodes));
    return uniq.sort((a, b) => {
      if (a === b) return 0;
      const pa = a.nodeType === 2 ? a.ownerElement : a, pb = b.nodeType === 2 ? b.ownerElement : b;
      if (pa === pb) return a.nodeType === 2 ? 1 : -1;
      if (!pa || !pb) return 0;
      const pos = pa.compareDocumentPosition(pb);
      return pos & 4 ? -1 : pos & 2 ? 1 : 0;
    });
  }
  const strVal = (n) => (n.nodeType === 2 ? n.value : n.nodeType === 1 || n.nodeType === 9 || n.nodeType === 11 ? n.textContent || '' : n.nodeValue || '');
  const toStr = (v) => (Array.isArray(v) ? (v.length ? strVal(v[0]) : '') : typeof v === 'number' ? (Number.isInteger(v) ? String(v) : String(v)) : String(v));
  const toNum = (v) => (Array.isArray(v) ? Number(toStr(v).trim() || NaN) : typeof v === 'boolean' ? +v : typeof v === 'string' ? (v.trim() === '' ? NaN : Number(v.trim())) : v);
  const toBool = (v) => (Array.isArray(v) ? v.length > 0 : typeof v === 'string' ? v.length > 0 : typeof v === 'number' ? v !== 0 && !Number.isNaN(v) : !!v);
  function compare(op, a, b) {
    const cmp = (x, y) => {
      switch (op) {
        case '=': return x === y;
        case '!=': return x !== y;
        case '<': return toNum(x) < toNum(y);
        case '>': return toNum(x) > toNum(y);
        case '<=': return toNum(x) <= toNum(y);
        default: return toNum(x) >= toNum(y);
      }
    };
    const eq = op === '=' || op === '!=';
    if (Array.isArray(a) && Array.isArray(b)) return a.some((x) => b.some((y) => cmp(strVal(x), strVal(y))));
    if (Array.isArray(a) || Array.isArray(b)) {
      const [set, other, flip] = Array.isArray(a) ? [a, b, false] : [b, a, true];
      return set.some((n) => {
        const sv = typeof other === 'number' ? toNum(strVal(n)) : typeof other === 'boolean' ? toBool([n]) : strVal(n);
        return flip ? cmp(other, sv) : cmp(sv, other);
      }) || (typeof other === 'boolean' && eq && (flip ? cmp(other, toBool(set)) : cmp(toBool(set), other)) && set.length === 0);
    }
    if (eq) {
      if (typeof a === 'boolean' || typeof b === 'boolean') return cmp(toBool(a), toBool(b));
      if (typeof a === 'number' || typeof b === 'number') return cmp(toNum(a), toNum(b));
      return cmp(toStr(a), toStr(b));
    }
    return cmp(toNum(a), toNum(b));
  }
  function xeval(e, ctx) {
    switch (e.op) {
      case 'or': return toBool(xeval(e.l, ctx)) || toBool(xeval(e.r, ctx));
      case 'and': return toBool(xeval(e.l, ctx)) && toBool(xeval(e.r, ctx));
      case '=': case '!=': case '<': case '>': case '<=': case '>=': return compare(e.op, xeval(e.l, ctx), xeval(e.r, ctx));
      case '+': return toNum(xeval(e.l, ctx)) + toNum(xeval(e.r, ctx));
      case '-': return toNum(xeval(e.l, ctx)) - toNum(xeval(e.r, ctx));
      case 'mul': return toNum(xeval(e.l, ctx)) * toNum(xeval(e.r, ctx));
      case 'div': return toNum(xeval(e.l, ctx)) / toNum(xeval(e.r, ctx));
      case 'mod': return toNum(xeval(e.l, ctx)) % toNum(xeval(e.r, ctx));
      case 'neg': return -toNum(xeval(e.e, ctx));
      case 'lit': return e.v;
      case 'num': return e.v;
      case 'var': return (ctx.vars && ctx.vars[e.name]) ?? '';
      case 'union': { const l = xeval(e.l, ctx), r = xeval(e.r, ctx); if (!Array.isArray(l) || !Array.isArray(r)) throw new TypeError('The union operator requires node-sets.'); return docOrder(l.concat(r)); }
      case 'filter': { let set = xeval(e.e, ctx); if (!Array.isArray(set)) throw new TypeError('Predicates apply to node-sets.'); for (const p of e.preds) set = applyPred(set, p, ctx, false); return set; }
      case 'path': {
        let set;
        if (e.from) { set = xeval(e.from, ctx); if (!Array.isArray(set)) throw new TypeError('Path steps apply to node-sets.'); }
        else if (e.abs) { let r = ctx.node; while (r.parentNode) r = r.parentNode; if (r.nodeType === 2) r = rootOf(r.ownerElement); set = [r]; }
        else set = [ctx.node];
        for (const st of e.steps) {
          const out = [];
          for (const n of set) {
            let cand = axisNodes(n, st.axis).filter((c) => testNode(c, st.test, st.axis));
            for (const p of st.preds) cand = applyPred(cand, p, ctx, REVERSE.has(st.axis));
            out.push(...cand);
          }
          set = docOrder(out);
        }
        return set;
      }
      case 'call': return xcall(e, ctx);
      default: throw new TypeError('bad XPath expression');
    }
  }
  function applyPred(nodes, pred, ctx, reverse) {
    const size = nodes.length;
    return nodes.filter((n, i) => {
      const c = { node: n, pos: i + 1, size, vars: ctx.vars, resolver: ctx.resolver };
      const v = xeval(pred, c);
      return typeof v === 'number' ? v === c.pos : toBool(v);
    });
    // `reverse` axes are already produced in reverse document order.
  }
  function xcall(e, ctx) {
    const a = e.args.map((x) => () => xeval(x, ctx));
    const s0 = () => (e.args.length ? toStr(a[0]()) : strVal(ctx.node));
    switch (e.name) {
      case 'last': return ctx.size;
      case 'position': return ctx.pos;
      case 'count': { const v = a[0](); if (!Array.isArray(v)) throw new TypeError('count() requires a node-set.'); return v.length; }
      case 'id': { const v = a[0](); const ids = (Array.isArray(v) ? v.map(strVal).join(' ') : toStr(v)).split(/\s+/).filter(Boolean); const d = ctx.node.nodeType === 9 ? ctx.node : ctx.node.ownerDocument; return docOrder(ids.map((i) => d.getElementById(i)).filter(Boolean)); }
      case 'local-name': { const v = e.args.length ? a[0]() : [ctx.node]; const n = v[0]; return n ? (n.localName || '') : ''; }
      case 'namespace-uri': { const v = e.args.length ? a[0]() : [ctx.node]; const n = v[0]; return n && n.namespaceURI ? n.namespaceURI : ''; }
      case 'name': { const v = e.args.length ? a[0]() : [ctx.node]; const n = v[0]; return n ? (n.nodeType === 1 ? (isHTMLDoc(n) ? n.localName : n.nodeName) : n.nodeType === 2 ? n.name : '') : ''; }
      case 'string': return s0();
      case 'concat': return a.map((f) => toStr(f())).join('');
      case 'starts-with': return toStr(a[0]()).startsWith(toStr(a[1]()));
      case 'ends-with': return toStr(a[0]()).endsWith(toStr(a[1]()));
      case 'contains': return toStr(a[0]()).includes(toStr(a[1]()));
      case 'substring-before': { const s = toStr(a[0]()), t = toStr(a[1]()); const i = s.indexOf(t); return i < 0 ? '' : s.slice(0, i); }
      case 'substring-after': { const s = toStr(a[0]()), t = toStr(a[1]()); const i = s.indexOf(t); return i < 0 ? '' : s.slice(i + t.length); }
      case 'substring': {
        const s = toStr(a[0]()), start = Math.round(toNum(a[1]()));
        const len = e.args.length > 2 ? Math.round(toNum(a[2]())) : Infinity;
        let out = '';
        for (let i = 1; i <= s.length; i++) if (i >= start && i < start + len) out += s[i - 1];
        return out;
      }
      case 'string-length': return s0().length;
      case 'normalize-space': return s0().replace(/[\x20\t\r\n]+/g, ' ').trim();
      case 'translate': { const s = toStr(a[0]()), from = toStr(a[1]()), to = toStr(a[2]()); let out = ''; for (const ch of s) { const i = from.indexOf(ch); if (i < 0) out += ch; else if (i < to.length) out += to[i]; } return out; }
      case 'lower-case': return toStr(a[0]()).toLowerCase();
      case 'upper-case': return toStr(a[0]()).toUpperCase();
      case 'boolean': return toBool(a[0]());
      case 'not': return !toBool(a[0]());
      case 'true': return true;
      case 'false': return false;
      case 'lang': { const want = toStr(a[0]()).toLowerCase(); for (let n = ctx.node; n; n = n.parentNode) { const l = n.getAttribute && n.getAttribute('lang'); if (l) { const v = l.toLowerCase(); return v === want || v.startsWith(want + '-'); } } return false; }
      case 'number': return e.args.length ? toNum(a[0]()) : toNum(strVal(ctx.node));
      case 'sum': { const v = a[0](); return v.reduce((s, n) => s + toNum(strVal(n)), 0); }
      case 'floor': return Math.floor(toNum(a[0]()));
      case 'ceiling': return Math.ceil(toNum(a[0]()));
      case 'round': { const n = toNum(a[0]()); return Number.isNaN(n) || !Number.isFinite(n) ? n : Math.floor(n + 0.5); }
      default: throw domError(`Failed to execute 'evaluate' on 'Document': The function '${e.name}' is not supported.`, 'SyntaxError');
    }
  }

  class XPathResult {
    constructor() { ILLEGAL(); }
    get resultType() { return this[S].type; }
    get numberValue() { if (this[S].type !== 1) throw new TypeError("Failed to read the 'numberValue' property from 'XPathResult': The result type is not a number."); return this[S].value; }
    get stringValue() { if (this[S].type !== 2) throw new TypeError("Failed to read the 'stringValue' property from 'XPathResult': The result type is not a string."); return this[S].value; }
    get booleanValue() { if (this[S].type !== 3) throw new TypeError("Failed to read the 'booleanValue' property from 'XPathResult': The result type is not a boolean."); return this[S].value; }
    get singleNodeValue() { if (![8, 9].includes(this[S].type)) throw new TypeError("Failed to read the 'singleNodeValue' property from 'XPathResult': The result type is not a single node."); return this[S].nodes[0] || null; }
    get invalidIteratorState() { return false; }
    get snapshotLength() { if (![6, 7].includes(this[S].type)) throw new TypeError("Failed to read the 'snapshotLength' property from 'XPathResult': The result type is not a snapshot."); return this[S].nodes.length; }
    iterateNext() { if (![4, 5].includes(this[S].type)) throw new TypeError("Failed to execute 'iterateNext' on 'XPathResult': The result type is not an iterator."); return this[S].nodes[this[S].i++] || null; }
    snapshotItem(i) { if (![6, 7].includes(this[S].type)) throw new TypeError("Failed to execute 'snapshotItem' on 'XPathResult': The result type is not a snapshot."); return this[S].nodes[i] || null; }
  }
  consts(XPathResult, XP);
  tag(XPathResult, 'XPathResult');
  class XPathExpression {
    constructor() { ILLEGAL(); }
    evaluate(contextNode, type = 0, result = null) {
      if (!(contextNode instanceof g.Node) && !(contextNode instanceof g.Attr)) throw new TypeError("Failed to execute 'evaluate' on 'XPathExpression': parameter 1 is not of type 'Node'.");
      const v = xeval(this[S].ast, { node: contextNode, pos: 1, size: 1, resolver: this[S].resolver });
      let t = Number(type) >>> 0;
      if (t === 0) t = Array.isArray(v) ? 4 : typeof v === 'number' ? 1 : typeof v === 'string' ? 2 : 3;
      const r = Object.create(XPathResult.prototype);
      const st = { type: t, i: 0, nodes: [], value: null };
      if (t === 1) st.value = toNum(v);
      else if (t === 2) st.value = toStr(v);
      else if (t === 3) st.value = toBool(v);
      else { if (!Array.isArray(v)) throw new TypeError("Failed to execute 'evaluate': The result is not a node set, and therefore cannot be converted to the desired type."); st.nodes = v; }
      hidden(r, S, st);
      void result;
      return r;
    }
  }
  tag(XPathExpression, 'XPathExpression');
  class XPathEvaluator {
    constructor() { return Object.create(new.target ? new.target.prototype : XPathEvaluator.prototype); }
    createExpression(expr, resolver = null) { const x = Object.create(XPathExpression.prototype); hidden(x, S, { ast: xparse(String(expr)), resolver }); return x; }
    createNSResolver(node) { return node; }
    evaluate(expr, context, resolver = null, type = 0, result = null) { return this.createExpression(expr, resolver).evaluate(context, type, result); }
  }
  tag(XPathEvaluator, 'XPathEvaluator');
  define('XPathResult', XPathResult);
  define('XPathExpression', XPathExpression);
  define('XPathEvaluator', XPathEvaluator);
  for (const k of ['createExpression', 'createNSResolver', 'evaluate']) method(g.Document, k, XPathEvaluator.prototype[k], true);

  // XSLT is not supported: the processor exists for feature detection and
  // its transforms produce no result.
  class XSLTProcessor {
    constructor() { const p = Object.create(new.target ? new.target.prototype : XSLTProcessor.prototype); hidden(p, S, { params: new Map(), sheet: null }); return p; }
    importStylesheet(node) { if (!(node instanceof g.Node)) throw new TypeError("Failed to execute 'importStylesheet' on 'XSLTProcessor': parameter 1 is not of type 'Node'."); this[S].sheet = node; }
    transformToFragment() { return null; }
    transformToDocument() { return null; }
    setParameter(ns, name, value) { this[S].params.set(`${ns || ''}|${name}`, value); }
    getParameter(ns, name) { const k = `${ns || ''}|${name}`; return this[S].params.has(k) ? this[S].params.get(k) : null; }
    removeParameter(ns, name) { this[S].params.delete(`${ns || ''}|${name}`); }
    clearParameters() { this[S].params.clear(); }
    reset() { this[S].params.clear(); this[S].sheet = null; }
  }
  tag(XSLTProcessor, 'XSLTProcessor');
  define('XSLTProcessor', XSLTProcessor);
})();
