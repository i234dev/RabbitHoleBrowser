// CSSOM (https://drafts.csswg.org/cssom/): StyleSheet, CSSStyleSheet
// (constructable, replace/replaceSync, insertRule/deleteRule), CSSRuleList,
// the CSSRule hierarchy, MediaList, StyleSheetList, `<style>/<link>.sheet`,
// `document.styleSheets` and `adoptedStyleSheets`.
//
// Sheets are parsed here from their text; changes made through the CSSOM
// are serialized back and handed to the style engine through
// __rhNative.setSheetText / setAdoptedStyles.
(function () {
  'use strict';
  const g = globalThis;
  const N = g.__rhNative;
  const define = (name, value) => Object.defineProperty(g, name, { value, writable: true, configurable: true });
  const hidden = (o, k, v) => Object.defineProperty(o, k, { value: v, writable: true, configurable: true });
  const tag = (C, name) => Object.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  const ILLEGAL = () => { throw new TypeError('Illegal constructor'); };
  const domError = (msg, name) => new DOMException(msg, name);
  const S = Symbol('cssom');

  // ------------------------------------------------------------ parsing

  // Split CSS source into top-level chunks: { prelude, block } (block is the
  // text inside braces, or null for statements ending with ';').
  function splitRules(src) {
    const out = [];
    let i = 0, n = src.length, start = 0;
    let depth = 0, prelude = null, blockStart = 0;
    while (i < n) {
      const c = src[i];
      if (c === '/' && src[i + 1] === '*') {
        const e = src.indexOf('*/', i + 2);
        if (depth === 0 && prelude === null) {
          // Drop comments from preludes.
          src = src.slice(0, i) + ' ' + src.slice(e < 0 ? n : e + 2);
          n = src.length;
          continue;
        }
        i = e < 0 ? n : e + 2;
        continue;
      }
      if (c === '"' || c === "'") {
        i++;
        while (i < n && src[i] !== c) { if (src[i] === '\\') i++; i++; }
        i++;
        continue;
      }
      if (c === '{') {
        if (depth === 0) { prelude = src.slice(start, i); blockStart = i + 1; }
        depth++;
      } else if (c === '}') {
        depth--;
        if (depth === 0) {
          out.push({ prelude: prelude.trim(), block: src.slice(blockStart, i) });
          prelude = null;
          start = i + 1;
        } else if (depth < 0) { depth = 0; start = i + 1; }
      } else if (c === ';' && depth === 0) {
        const p = src.slice(start, i).trim();
        if (p) out.push({ prelude: p, block: null });
        start = i + 1;
      }
      i++;
    }
    if (depth > 0 && prelude !== null) out.push({ prelude: prelude.trim(), block: src.slice(blockStart) });
    return out;
  }

  // Parse "a: b; c: d !important" into [[name, value, important]].
  function parseDecls(text) {
    const out = [];
    let i = 0, start = 0, depth = 0;
    const n = text.length;
    const flush = (end) => {
      const part = text.slice(start, end).replace(/\/\*[\s\S]*?\*\//g, '').trim();
      start = end + 1;
      if (!part) return;
      const colon = part.indexOf(':');
      if (colon <= 0) return;
      let name = part.slice(0, colon).trim();
      let value = part.slice(colon + 1).trim();
      let important = false;
      const m = /!\s*important\s*$/i.exec(value);
      if (m) { important = true; value = value.slice(0, m.index).trim(); }
      if (!name.startsWith('--')) name = name.toLowerCase();
      out.push([name, value, important]);
    };
    while (i < n) {
      const c = text[i];
      if (c === '"' || c === "'") { i++; while (i < n && text[i] !== c) { if (text[i] === '\\') i++; i++; } }
      else if (c === '(' || c === '[' || c === '{') depth++;
      else if (c === ')' || c === ']' || c === '}') depth--;
      else if (c === ';' && depth <= 0) flush(i);
      i++;
    }
    flush(n);
    return out;
  }

  const kebab = (p) => p === 'cssFloat' ? 'float' : p.startsWith('--') ? p : p.replace(/^(webkit|moz|ms)(?=[A-Z])/, '-$1').replace(/[A-Z]/g, (m) => '-' + m.toLowerCase());

  // ------------------------------------------------ CSSStyleDeclaration

  // Declarations of a rule: a Proxy so `style.color = ...` works like the
  // element.style object (whose prototype, CSSStyleDeclaration, it shares).
  function makeDecl(rule, text) {
    // Parsed on first use: most rules are only serialized back.
    const st = { raw: text || '', parsed: null, rule };
    Object.defineProperty(st, 'decls', { get() { if (!this.parsed) this.parsed = parseDecls(this.raw); return this.parsed; }, set(v) { this.parsed = v; } });
    const changed = () => { if (st.rule) touch(st.rule); };
    const get = (p) => { for (let i = st.decls.length - 1; i >= 0; i--) if (st.decls[i][0] === p) return st.decls[i][1]; return ''; };
    const set = (p, v, imp) => {
      v = v == null ? '' : String(v).trim();
      st.decls = st.decls.filter((d) => d[0] !== p);
      if (v !== '') st.decls.push([p, v, !!imp]);
      changed();
    };
    const cssText = () => {
      if (st.parsed) return st.parsed.map(([k, v, i]) => `${k}: ${v}${i ? ' !important' : ''};`).join(' ');
      const t = st.raw.trim();
      return t && !t.endsWith(';') ? t + ';' : t;
    };
    const methods = {
      getPropertyValue: (p) => get(String(p).startsWith('--') ? String(p) : String(p).toLowerCase()),
      getPropertyPriority: (p) => (st.decls.find((d) => d[0] === String(p).toLowerCase() && d[2]) ? 'important' : ''),
      setProperty: (p, v, prio) => set(String(p).startsWith('--') ? String(p) : String(p).toLowerCase(), v, String(prio || '').toLowerCase() === 'important'),
      removeProperty: (p) => { p = String(p).toLowerCase(); const old = get(p); st.decls = st.decls.filter((d) => d[0] !== p); changed(); return old; },
      item: (i) => (st.decls[i] ? st.decls[i][0] : ''),
      toString: () => '[object CSSStyleDeclaration]',
    };
    const target = Object.create(g.CSSStyleDeclaration ? g.CSSStyleDeclaration.prototype : Object.prototype);
    for (const k in methods) hidden(target, k, methods[k]);
    hidden(target, S, st);
    const proxy = new Proxy(target, {
      get(t, k) {
        if (typeof k === 'symbol') return t[k];
        if (k in methods) return methods[k];
        if (k === 'cssText') return cssText();
        if (k === 'length') return st.decls.length;
        if (k === 'parentRule') return st.rule || null;
        if (k === 'cssFloat') return get('float');
        if (/^\d+$/.test(k)) return methods.item(+k);
        if (k in Object.prototype || k === 'constructor') return t[k];
        return get(kebab(k));
      },
      set(t, k, v) {
        if (typeof k === 'symbol') { t[k] = v; return true; }
        if (k === 'cssText') { st.decls = parseDecls(String(v)); changed(); return true; }
        set(kebab(k), v, false);
        return true;
      },
      has(t, k) { return typeof k === 'string' ? true : k in t; },
      ownKeys() { return st.decls.map((_, i) => String(i)); },
      getOwnPropertyDescriptor(t, k) {
        if (typeof k === 'string' && /^\d+$/.test(k) && +k < st.decls.length) return { value: st.decls[+k][0], enumerable: true, configurable: true, writable: false };
        return Object.getOwnPropertyDescriptor(t, k);
      },
    });
    return proxy;
  }

  // ------------------------------------------------------------ MediaList

  class MediaList {
    constructor() { ILLEGAL(); }
    get mediaText() { return this[S].list.join(', '); }
    set mediaText(v) { this[S].list = String(v).split(',').map((s) => s.trim()).filter(Boolean); this[S].changed(); }
    get length() { return this[S].list.length; }
    item(i) { return this[S].list[i] ?? null; }
    appendMedium(m) { m = String(m).trim(); if (!this[S].list.includes(m)) { this[S].list.push(m); this[S].changed(); } }
    deleteMedium(m) {
      const i = this[S].list.indexOf(String(m).trim());
      if (i < 0) throw domError("Failed to execute 'deleteMedium' on 'MediaList': Failed to delete '" + m + "'.", 'NotFoundError');
      this[S].list.splice(i, 1); this[S].changed();
    }
    toString() { return this.mediaText; }
    [Symbol.iterator]() { return this[S].list[Symbol.iterator](); }
  }
  tag(MediaList, 'MediaList');
  function makeMediaList(text, changed) {
    const m = Object.create(MediaList.prototype);
    hidden(m, S, { list: String(text || '').split(',').map((s) => s.trim()).filter(Boolean), changed: changed || (() => {}) });
    return new Proxy(m, { get(t, k) { return typeof k === 'string' && /^\d+$/.test(k) ? t.item(+k) : Reflect.get(t, k, t); } });
  }

  // --------------------------------------------------------- CSSRuleList

  class CSSRuleList {
    constructor() { ILLEGAL(); }
    get length() { return this[S]().length; }
    item(i) { return this[S]()[i] ?? null; }
    [Symbol.iterator]() { return this[S]()[Symbol.iterator](); }
  }
  tag(CSSRuleList, 'CSSRuleList');
  function makeRuleList(getter) {
    const l = Object.create(CSSRuleList.prototype);
    hidden(l, S, getter);
    return new Proxy(l, {
      get(t, k) { return typeof k === 'string' && /^\d+$/.test(k) ? (getter()[+k] ?? undefined) : Reflect.get(t, k, t); },
      has(t, k) { return typeof k === 'string' && /^\d+$/.test(k) ? +k < getter().length : k in t; },
      ownKeys(t) { return getter().map((_, i) => String(i)); },
      getOwnPropertyDescriptor(t, k) {
        if (typeof k === 'string' && /^\d+$/.test(k) && +k < getter().length) return { value: getter()[+k], enumerable: true, configurable: true, writable: false };
        return Object.getOwnPropertyDescriptor(t, k);
      },
    });
  }

  // -------------------------------------------------------------- CSSRule

  const TYPES = { STYLE_RULE: 1, CHARSET_RULE: 2, IMPORT_RULE: 3, MEDIA_RULE: 4, FONT_FACE_RULE: 5, PAGE_RULE: 6, KEYFRAMES_RULE: 7, KEYFRAME_RULE: 8, MARGIN_RULE: 9, NAMESPACE_RULE: 10, COUNTER_STYLE_RULE: 11, SUPPORTS_RULE: 12, FONT_FEATURE_VALUES_RULE: 14 };
  class CSSRule {
    constructor() { ILLEGAL(); }
    get type() { return this[S].type; }
    get cssText() { return this[S].text(); }
    set cssText(_) {}
    get parentRule() { return this[S].parentRule || null; }
    get parentStyleSheet() { return this[S].sheet || null; }
  }
  for (const [k, v] of Object.entries(TYPES)) {
    Object.defineProperty(CSSRule, k, { value: v, enumerable: true });
    Object.defineProperty(CSSRule.prototype, k, { value: v, enumerable: true });
  }
  tag(CSSRule, 'CSSRule');

  function touch(rule) {
    let r = rule;
    while (r && r[S].parentRule) r = r[S].parentRule;
    const sheet = r && r[S].sheet;
    if (sheet) sheetChanged(sheet);
  }

  class CSSStyleRule extends CSSRule {
    get selectorText() { return this[S].selector; }
    set selectorText(v) { this[S].selector = String(v).trim(); touch(this); }
    get style() { return this[S].style; }
    set style(v) { this[S].style.cssText = v; }
    get styleMap() { return null; }
    get cssRules() { return makeRuleList(() => this[S].children); }
    insertRule(text, index = 0) { return insertInto(this, this[S].children, text, index); }
    deleteRule(index) { deleteFrom(this, this[S].children, index); }
  }
  tag(CSSStyleRule, 'CSSStyleRule');

  class CSSGroupingRule extends CSSRule {
    get cssRules() { return makeRuleList(() => this[S].children); }
    insertRule(text, index = 0) { return insertInto(this, this[S].children, text, index); }
    deleteRule(index) { deleteFrom(this, this[S].children, index); }
  }
  tag(CSSGroupingRule, 'CSSGroupingRule');
  // CSS Nesting: style rules are grouping rules.
  Object.setPrototypeOf(CSSStyleRule.prototype, CSSGroupingRule.prototype);
  Object.setPrototypeOf(CSSStyleRule, CSSGroupingRule);
  class CSSConditionRule extends CSSGroupingRule {
    get conditionText() { return this[S].condition; }
  }
  tag(CSSConditionRule, 'CSSConditionRule');
  class CSSMediaRule extends CSSConditionRule {
    get media() { return this[S].media || (this[S].media = makeMediaList(this[S].condition, () => { this[S].condition = this[S].media.mediaText; touch(this); })); }
    get matches() { try { return g.matchMedia(this[S].condition).matches; } catch (e) { return false; } }
  }
  tag(CSSMediaRule, 'CSSMediaRule');
  class CSSSupportsRule extends CSSConditionRule {
    get matches() { try { return g.CSS.supports(this[S].condition); } catch (e) { return false; } }
  }
  tag(CSSSupportsRule, 'CSSSupportsRule');
  class CSSContainerRule extends CSSConditionRule {
    get containerName() { const m = /^([a-zA-Z_-][\w-]*)\s+(?=\()/.exec(this[S].condition); return m ? m[1] : ''; }
    get containerQuery() { return this[S].condition.replace(/^([a-zA-Z_-][\w-]*)\s+(?=\()/, ''); }
  }
  tag(CSSContainerRule, 'CSSContainerRule');
  class CSSLayerBlockRule extends CSSGroupingRule { get name() { return this[S].condition; } }
  tag(CSSLayerBlockRule, 'CSSLayerBlockRule');
  class CSSLayerStatementRule extends CSSRule { get nameList() { return Object.freeze(this[S].condition.split(',').map((s) => s.trim()).filter(Boolean)); } }
  tag(CSSLayerStatementRule, 'CSSLayerStatementRule');
  class CSSScopeRule extends CSSGroupingRule {
    get start() { const m = /^\(([^)]*)\)/.exec(this[S].condition); return m ? m[1].trim() : null; }
    get end() { const m = /to\s*\(([^)]*)\)/.exec(this[S].condition); return m ? m[1].trim() : null; }
  }
  tag(CSSScopeRule, 'CSSScopeRule');
  class CSSStartingStyleRule extends CSSGroupingRule {}
  tag(CSSStartingStyleRule, 'CSSStartingStyleRule');
  class CSSPageRule extends CSSGroupingRule {
    get selectorText() { return this[S].selector; }
    set selectorText(v) { this[S].selector = String(v).trim(); touch(this); }
    get style() { return this[S].style; }
  }
  tag(CSSPageRule, 'CSSPageRule');
  class CSSMarginRule extends CSSRule {
    get name() { return this[S].condition; }
    get style() { return this[S].style; }
  }
  tag(CSSMarginRule, 'CSSMarginRule');
  class CSSFontFaceRule extends CSSRule { get style() { return this[S].style; } }
  tag(CSSFontFaceRule, 'CSSFontFaceRule');
  class CSSImportRule extends CSSRule {
    get href() { const m = /^(?:url\(\s*)?["']?([^"')\s]+)["']?\s*\)?/.exec(this[S].condition); return m ? m[1] : ''; }
    get media() { return makeMediaList(this[S].condition.replace(/^(?:url\([^)]*\)|"[^"]*"|'[^']*')\s*/, '').replace(/(layer|supports)\([^)]*\)\s*/g, '')); }
    get styleSheet() { return null; }
    get layerName() { const m = /layer(?:\(([^)]*)\))?/.exec(this[S].condition); return m ? (m[1] || '').trim() : null; }
    get supportsText() { const m = /supports\(([^)]*)\)/.exec(this[S].condition); return m ? m[1].trim() : null; }
  }
  tag(CSSImportRule, 'CSSImportRule');
  class CSSNamespaceRule extends CSSRule {
    get namespaceURI() { const m = /(?:url\(\s*)?["']([^"']*)["']/.exec(this[S].condition); return m ? m[1] : ''; }
    get prefix() { const m = /^([a-zA-Z_][\w-]*)\s+/.exec(this[S].condition); return m && !/^url$/i.test(m[1]) ? m[1] : ''; }
  }
  tag(CSSNamespaceRule, 'CSSNamespaceRule');
  class CSSKeyframeRule extends CSSRule {
    get keyText() { return this[S].selector; }
    set keyText(v) { this[S].selector = String(v).trim(); touch(this); }
    get style() { return this[S].style; }
  }
  tag(CSSKeyframeRule, 'CSSKeyframeRule');
  class CSSKeyframesRule extends CSSRule {
    get name() { return this[S].condition; }
    set name(v) { this[S].condition = String(v).trim(); touch(this); }
    get cssRules() { return makeRuleList(() => this[S].children); }
    get length() { return this[S].children.length; }
    appendRule(text) { const r = parseChunk(splitRules(`${text}`)[0] || { prelude: '', block: '' }, this[S].sheet, this, true); if (r) { this[S].children.push(r); touch(this); } }
    deleteRule(key) { const i = this[S].children.findIndex((r) => r[S].selector === normKey(key)); if (i >= 0) { this[S].children.splice(i, 1); touch(this); } }
    findRule(key) { const k = normKey(key); for (let i = this[S].children.length - 1; i >= 0; i--) if (this[S].children[i][S].selector === k) return this[S].children[i]; return null; }
  }
  tag(CSSKeyframesRule, 'CSSKeyframesRule');
  const normKey = (k) => String(k).split(',').map((s) => { s = s.trim().toLowerCase(); return s === 'from' ? '0%' : s === 'to' ? '100%' : s; }).join(', ');
  class CSSCounterStyleRule extends CSSRule {
    get name() { return this[S].condition; }
  }
  tag(CSSCounterStyleRule, 'CSSCounterStyleRule');
  class CSSFontFeatureValuesRule extends CSSRule { get fontFamily() { return this[S].condition; } }
  tag(CSSFontFeatureValuesRule, 'CSSFontFeatureValuesRule');
  class CSSPropertyRule extends CSSRule {
    get name() { return this[S].condition; }
    get syntax() { return unq(this[S].style.getPropertyValue('syntax')); }
    get inherits() { return this[S].style.getPropertyValue('inherits').trim() === 'true'; }
    get initialValue() { const v = this[S].style.getPropertyValue('initial-value'); return v === '' ? null : v; }
  }
  tag(CSSPropertyRule, 'CSSPropertyRule');
  const unq = (s) => String(s).trim().replace(/^["']|["']$/g, '');
  class CSSNestedDeclarations extends CSSRule { get style() { return this[S].style; } }
  tag(CSSNestedDeclarations, 'CSSNestedDeclarations');

  function newRule(C, type, data) {
    const r = Object.create(C.prototype);
    hidden(r, S, Object.assign({ type, children: [], condition: '', selector: '', sheet: null, parentRule: null }, data));
    return r;
  }

  function serializeChildren(r) { return r[S].children.map((c) => '  ' + c.cssText).join('\n'); }

  // Build a rule object from a split chunk.
  function parseChunk(chunk, sheet, parent, inKeyframes) {
    const { prelude, block } = chunk;
    let r;
    if (inKeyframes) {
      r = newRule(CSSKeyframeRule, TYPES.KEYFRAME_RULE, { selector: normKey(prelude) });
      r[S].style = makeDecl(r, block || '');
      r[S].text = () => `${r[S].selector} { ${r[S].style.cssText} }`;
    } else if (prelude[0] === '@') {
      const m = /^@([-\w]+)\s*([\s\S]*)$/.exec(prelude);
      const name = m ? m[1].toLowerCase() : '';
      const rest = m ? m[2].trim() : '';
      const group = (C, type, kw) => {
        const rr = newRule(C, type, { condition: rest });
        rr[S].children = parseList(block || '', sheet, rr);
        rr[S].text = () => `@${kw} ${rr[S].condition} {\n${serializeChildren(rr)}\n}`;
        return rr;
      };
      switch (block === null ? name + ';' : name) {
        case 'media': r = group(CSSMediaRule, TYPES.MEDIA_RULE, 'media'); break;
        case 'supports': r = group(CSSSupportsRule, TYPES.SUPPORTS_RULE, 'supports'); break;
        case 'container': r = group(CSSContainerRule, 0, 'container'); break;
        case 'layer': r = group(CSSLayerBlockRule, 0, 'layer'); if (!rest) r[S].text = () => `@layer {\n${serializeChildren(r)}\n}`; break;
        case 'scope': r = group(CSSScopeRule, 0, 'scope'); break;
        case 'starting-style': r = group(CSSStartingStyleRule, 0, 'starting-style'); r[S].text = () => `@starting-style {\n${serializeChildren(r)}\n}`; break;
        case 'document': case '-moz-document': r = group(CSSGroupingRule, 0, name); break;
        case 'keyframes': case '-webkit-keyframes': case '-moz-keyframes': {
          r = newRule(CSSKeyframesRule, TYPES.KEYFRAMES_RULE, { condition: unq(rest) });
          r[S].children = splitRules(block || '').map((c) => parseChunk(c, sheet, r, true));
          const kw = name;
          r[S].text = () => `@${kw} ${r[S].condition} {\n${serializeChildren(r)}\n}`;
          break;
        }
        case 'font-face':
          r = newRule(CSSFontFaceRule, TYPES.FONT_FACE_RULE, {});
          r[S].style = makeDecl(r, block);
          r[S].text = () => `@font-face { ${r[S].style.cssText} }`;
          break;
        case 'page': {
          r = newRule(CSSPageRule, TYPES.PAGE_RULE, { selector: rest });
          r[S].style = makeDecl(r, block);
          r[S].text = () => `@page ${r[S].selector ? r[S].selector + ' ' : ''}{ ${r[S].style.cssText} }`;
          break;
        }
        case 'property':
          r = newRule(CSSPropertyRule, 0, { condition: rest });
          r[S].style = makeDecl(r, block);
          r[S].text = () => `@property ${r[S].condition} { ${r[S].style.cssText} }`;
          break;
        case 'counter-style':
          r = newRule(CSSCounterStyleRule, TYPES.COUNTER_STYLE_RULE, { condition: rest });
          r[S].style = makeDecl(r, block);
          r[S].text = () => `@counter-style ${r[S].condition} { ${r[S].style.cssText} }`;
          break;
        case 'font-feature-values':
          r = newRule(CSSFontFeatureValuesRule, TYPES.FONT_FEATURE_VALUES_RULE, { condition: rest });
          r[S].raw = block;
          r[S].text = () => `@font-feature-values ${r[S].condition} {${r[S].raw}}`;
          break;
        case 'import;':
          r = newRule(CSSImportRule, TYPES.IMPORT_RULE, { condition: rest });
          r[S].text = () => `@import ${r[S].condition};`;
          break;
        case 'namespace;':
          r = newRule(CSSNamespaceRule, TYPES.NAMESPACE_RULE, { condition: rest });
          r[S].text = () => `@namespace ${r[S].condition};`;
          break;
        case 'layer;':
          r = newRule(CSSLayerStatementRule, 0, { condition: rest });
          r[S].text = () => `@layer ${r[S].condition};`;
          break;
        case 'charset;':
          return null;
        default:
          if (/^(top|bottom|left|right)-/.test(name) && parent && parent instanceof CSSPageRule) {
            r = newRule(CSSMarginRule, TYPES.MARGIN_RULE, { condition: name });
            r[S].style = makeDecl(r, block);
            r[S].text = () => `@${name} { ${r[S].style.cssText} }`;
          } else {
            // Unknown at-rules are dropped, as by a real CSS parser.
            return null;
          }
      }
    } else {
      if (!prelude) return null;
      r = newRule(CSSStyleRule, TYPES.STYLE_RULE, { selector: prelude.replace(/\s+/g, ' ') });
      // CSS nesting: nested rules inside the block.
      const nested = block.indexOf('{') >= 0 ? splitNested(block) : null;
      r[S].style = makeDecl(r, nested ? nested.decls : block);
      if (nested) r[S].children = nested.rules.map((c) => parseChunk(c, sheet, r, false)).filter(Boolean);
      r[S].text = () => {
        const decls = r[S].style.cssText;
        if (!r[S].children.length) return `${r[S].selector} { ${decls}${decls ? ' ' : ''}}`;
        return `${r[S].selector} {${decls ? ' ' + decls : ''}\n${serializeChildren(r)}\n}`;
      };
    }
    r[S].sheet = sheet;
    r[S].parentRule = parent || null;
    for (const c of r[S].children) { c[S].parentRule = r; c[S].sheet = sheet; }
    return r;
  }

  // Separate declarations from nested rules inside a style rule block.
  function splitNested(block) {
    let decls = '';
    const rules = [];
    for (const ch of splitRules(block)) {
      if (ch.block === null) decls += ch.prelude + ';';
      else {
        // "color: red; .x" → the part before the last ';' are declarations.
        const k = ch.prelude.lastIndexOf(';');
        if (k >= 0) { decls += ch.prelude.slice(0, k + 1); ch.prelude = ch.prelude.slice(k + 1).trim(); }
        rules.push(ch);
      }
    }
    return { decls, rules };
  }

  function parseList(text, sheet, parent) {
    return splitRules(String(text)).map((c) => parseChunk(c, sheet, parent, false)).filter(Boolean);
  }

  function insertInto(owner, list, text, index) {
    index = index === undefined ? 0 : Number(index) >>> 0;
    if (index > list.length) throw domError(`Failed to execute 'insertRule' on '${owner instanceof CSSStyleSheet ? 'CSSStyleSheet' : 'CSSRule'}': The index provided (${index}) is larger than the maximum index (${list.length}).`, 'IndexSizeError');
    const chunks = splitRules(String(text));
    if (chunks.length !== 1) throw domError(`Failed to execute 'insertRule': Failed to parse the rule '${text}'.`, 'SyntaxError');
    const sheet = owner instanceof CSSStyleSheet ? owner : owner[S].sheet;
    const rule = parseChunk(chunks[0], sheet, owner instanceof CSSStyleSheet ? null : owner, false);
    if (!rule) throw domError(`Failed to execute 'insertRule': Failed to parse the rule '${text}'.`, 'SyntaxError');
    if (owner instanceof CSSStyleSheet && rule instanceof CSSImportRule && owner[S].constructed) throw domError("Can't insert @import rules into a constructed stylesheet.", 'SyntaxError');
    list.splice(index, 0, rule);
    if (owner instanceof CSSStyleSheet) sheetChanged(owner); else touch(owner);
    return index;
  }

  function deleteFrom(owner, list, index) {
    index = Number(index) >>> 0;
    if (index >= list.length) throw domError(`Failed to execute 'deleteRule': The index provided (${index}) is larger than the maximum index (${list.length - 1}).`, 'IndexSizeError');
    list.splice(index, 1);
    if (owner instanceof CSSStyleSheet) sheetChanged(owner); else touch(owner);
  }

  // ----------------------------------------------------------- sheets

  class StyleSheet {
    constructor() { ILLEGAL(); }
    get type() { return 'text/css'; }
    get href() { return this[S].href; }
    get ownerNode() { return this[S].owner; }
    get parentStyleSheet() { return null; }
    get title() { const o = this[S].owner; return o && o.getAttribute ? o.getAttribute('title') : null; }
    get media() { return this[S].mediaList || (this[S].mediaList = makeMediaList(this[S].media, () => { this[S].media = this[S].mediaList.mediaText; })); }
    get disabled() { return this[S].disabled; }
    set disabled(v) {
      v = !!v;
      if (v === this[S].disabled) return;
      this[S].disabled = v;
      sheetChanged(this);
    }
  }
  tag(StyleSheet, 'StyleSheet');

  class CSSStyleSheet extends StyleSheet {
    constructor(options = {}) {
      const self = Object.create(new.target ? new.target.prototype : CSSStyleSheet.prototype);
      hidden(self, S, {
        rules: [], owner: null, href: null, constructed: true, disabled: !!(options && options.disabled),
        media: options && options.media ? (typeof options.media === 'string' ? options.media : options.media.mediaText) : '',
        baseURL: options && options.baseURL ? String(options.baseURL) : null,
        adopters: new Set(), loaded: true,
      });
      return self;
    }
    get ownerRule() { return null; }
    get cssRules() { ensureLoaded(this); return makeRuleList(() => this[S].rules); }
    get rules() { return this.cssRules; }
    insertRule(text, index = 0) { ensureLoaded(this); return insertInto(this, this[S].rules, text, index); }
    deleteRule(index) { ensureLoaded(this); deleteFrom(this, this[S].rules, index); }
    addRule(selector = 'undefined', style = 'undefined', index) {
      ensureLoaded(this);
      if (index === undefined) index = this[S].rules.length;
      insertInto(this, this[S].rules, `${selector} { ${style} }`, index);
      return -1;
    }
    removeRule(index = 0) { this.deleteRule(index); }
    replace(text) {
      try { this.replaceSync(text); } catch (e) { return Promise.reject(e); }
      return Promise.resolve(this);
    }
    replaceSync(text) {
      if (!this[S].constructed) throw domError("Failed to execute 'replaceSync' on 'CSSStyleSheet': Can't call replaceSync on non-constructed CSSStyleSheets.", 'NotAllowedError');
      // @import rules are ignored in constructed sheets.
      // Parsed lazily (cssRules / insertRule); the style engine gets the text.
      this[S].rules = [];
      this[S].source = String(text);
      this[S].loaded = false;
      sheetChanged(this);
    }
  }
  tag(CSSStyleSheet, 'CSSStyleSheet');

  const sheetText = (sheet) => (sheet[S].loaded ? sheet[S].rules.map((r) => r.cssText).join('\n') : sheet[S].source || '');

  function ensureLoaded(sheet) {
    const st = sheet[S];
    if (st.loaded) return;
    st.loaded = true;
    let text = st.source || '';
    if (st.owner && st.owner.localName === 'link') {
      text = st.href ? (N.fetchText(st.href) || '') : '';
    }
    st.rules = parseList(text, sheet, null);
    if (st.constructed) st.rules = st.rules.filter((r) => !(r instanceof CSSImportRule));
    st.source = null;
  }

  // Changes are pushed to the style engine once per microtask (CSS-in-JS
  // libraries insert many rules in a row), or right away before
  // getComputedStyle.
  const pendingSheets = new Set();
  let flushQueued = false;
  function flushSheets() {
    flushQueued = false;
    const list = Array.from(pendingSheets);
    pendingSheets.clear();
    const targets = new Set();
    for (const sheet of list) {
      const st = sheet[S];
      if (st.owner) {
        st.dirty = true;
        N.setSheetText(st.owner, st.disabled ? '' : sheetText(sheet));
      }
      for (const t of st.adopters) targets.add(t);
    }
    for (const t of targets) syncAdopted(t);
  }
  function sheetChanged(sheet) {
    pendingSheets.add(sheet);
    if (!flushQueued) { flushQueued = true; queueMicrotask(flushSheets); }
  }
  const gcs = g.getComputedStyle;
  if (gcs) hidden(g, 'getComputedStyle', function getComputedStyle(el, pseudo) { if (pendingSheets.size) flushSheets(); return gcs.call(this, el, pseudo); });
  // Geometry queries see pending CSSOM changes too.
  const geometry = [
    [g.Element, ['getBoundingClientRect', 'getClientRects'], ['clientTop', 'clientLeft', 'clientWidth', 'clientHeight', 'scrollWidth', 'scrollHeight']],
    [g.HTMLElement, [], ['offsetTop', 'offsetLeft', 'offsetWidth', 'offsetHeight', 'offsetParent']],
    [g.HTMLImageElement, [], ['width', 'height']],
  ];
  for (const [C, methods, getters] of geometry) {
    if (!C) continue;
    for (const m of methods) {
      const d = Object.getOwnPropertyDescriptor(C.prototype, m);
      if (d && typeof d.value === 'function') { const f = d.value; const w = { [m](...a) { if (pendingSheets.size) flushSheets(); return f.apply(this, a); } }[m]; Object.defineProperty(w, 'length', { value: f.length, configurable: true }); Object.defineProperty(C.prototype, m, { ...d, value: w }); }
    }
    for (const k of getters) {
      const d = Object.getOwnPropertyDescriptor(C.prototype, k);
      if (d && d.get) { const get = d.get; const w = { [k]() { if (pendingSheets.size) flushSheets(); return get.call(this); } }[k]; Object.defineProperty(w, 'name', { value: 'get ' + k, configurable: true }); Object.defineProperty(C.prototype, k, { ...d, get: w }); }
    }
  }

  // `<style>.sheet` / `<link rel=stylesheet>.sheet` (cached per element,
  // re-parsed when the element's text changes).
  const ownerSheets = new WeakMap();
  function sheetFor(el) {
    const isLink = el.localName === 'link';
    if (isLink) {
      const rel = (el.getAttribute('rel') || '').toLowerCase().split(/\s+/);
      if (!rel.includes('stylesheet') || !el.getAttribute('href')) return null;
    }
    if (!el.isConnected) return null;
    const source = isLink ? el.href : el.textContent;
    let rec = ownerSheets.get(el);
    if (rec && rec.source === source) return rec.sheet;
    const sheet = Object.create(CSSStyleSheet.prototype);
    hidden(sheet, S, {
      rules: [], owner: el, href: isLink ? el.href : null, constructed: false, disabled: false,
      media: el.getAttribute('media') || '', adopters: new Set(), loaded: false,
    });
    if (!isLink) sheet[S].source = source;
    if (rec && rec.sheet[S].dirty) N.setSheetText(el, null);
    ownerSheets.set(el, { source, sheet });
    return sheet;
  }
  for (const name of ['HTMLStyleElement', 'HTMLLinkElement']) {
    const C = g[name];
    if (C) Object.defineProperty(C.prototype, 'sheet', { get() { return sheetFor(this); }, configurable: true, enumerable: true });
  }
  // SVG <style> too.
  if (g.SVGElement) Object.defineProperty(g.SVGElement.prototype, 'sheet', { get() { return this.localName === 'style' ? sheetFor(this) : undefined; }, configurable: true });

  class StyleSheetList {
    constructor() { ILLEGAL(); }
    get length() { return this[S]().length; }
    item(i) { return this[S]()[i] ?? null; }
    [Symbol.iterator]() { return this[S]()[Symbol.iterator](); }
  }
  tag(StyleSheetList, 'StyleSheetList');
  function makeSheetList(getter) {
    const l = Object.create(StyleSheetList.prototype);
    hidden(l, S, getter);
    return new Proxy(l, {
      get(t, k) { return typeof k === 'string' && /^\d+$/.test(k) ? getter()[+k] : Reflect.get(t, k, t); },
      has(t, k) { return typeof k === 'string' && /^\d+$/.test(k) ? +k < getter().length : k in t; },
    });
  }
  const sheetsIn = (root) => {
    const out = [];
    for (const el of root.querySelectorAll('style, link')) {
      const s = sheetFor(el);
      if (s) out.push(s);
    }
    return out;
  };
  Object.defineProperty(g.Document.prototype, 'styleSheets', { get() { const d = this; return makeSheetList(() => sheetsIn(d)); }, configurable: true, enumerable: true });
  if (g.ShadowRoot) Object.defineProperty(g.ShadowRoot.prototype, 'styleSheets', { get() { const r = this; return makeSheetList(() => sheetsIn(r)); }, configurable: true, enumerable: true });

  // -------------------------------------------------- adoptedStyleSheets

  const adopted = new WeakMap();
  function syncAdopted(target) {
    const list = adopted.get(target) || [];
    N.setAdoptedStyles(target, list.filter((s) => !s[S].disabled).map(sheetText));
  }
  function adoptedArray(target) {
    let rec = adopted.get(target);
    if (!rec) { rec = []; adopted.set(target, rec); }
    // An observable array: index writes / push / splice resync the sheets.
    return new Proxy(rec, {
      set(t, k, v) {
        if (typeof k === 'string' && /^\d+$/.test(k) && !(v instanceof CSSStyleSheet)) throw new TypeError("Failed to set an indexed property on 'ObservableArray<CSSStyleSheet>': Failed to convert value to 'CSSStyleSheet'.");
        const old = t[k];
        t[k] = v;
        if (old instanceof CSSStyleSheet && !t.includes(old)) old[S].adopters.delete(target);
        if (v instanceof CSSStyleSheet) v[S].adopters.add(target);
        syncAdopted(target);
        return true;
      },
      deleteProperty(t, k) { delete t[k]; syncAdopted(target); return true; },
    });
  }
  function setAdopted(target, value) {
    if (value == null || typeof value[Symbol.iterator] !== 'function') throw new TypeError("Failed to set the 'adoptedStyleSheets' property: The provided value cannot be converted to a sequence.");
    const list = Array.from(value);
    for (const s of list) {
      if (!(s instanceof CSSStyleSheet)) throw new TypeError("Failed to set the 'adoptedStyleSheets' property: Failed to convert value to 'CSSStyleSheet'.");
      if (!s[S].constructed) throw domError("Failed to set the 'adoptedStyleSheets' property: Can't adopt non-constructed stylesheets.", 'NotAllowedError');
    }
    const old = adopted.get(target) || [];
    for (const s of old) s[S].adopters.delete(target);
    old.length = 0;
    for (const s of list) { old.push(s); s[S].adopters.add(target); }
    adopted.set(target, old);
    syncAdopted(target);
  }
  for (const C of [g.Document, g.ShadowRoot]) {
    if (!C) continue;
    Object.defineProperty(C.prototype, 'adoptedStyleSheets', {
      get() { return adoptedArray(this); },
      set(v) { setAdopted(this, v); },
      configurable: true, enumerable: true,
    });
  }

  // ------------------------------------------------------------- exports

  define('StyleSheet', StyleSheet);
  define('CSSStyleSheet', CSSStyleSheet);
  define('StyleSheetList', StyleSheetList);
  define('MediaList', MediaList);
  define('CSSRuleList', CSSRuleList);
  define('CSSRule', CSSRule);
  define('CSSStyleRule', CSSStyleRule);
  define('CSSGroupingRule', CSSGroupingRule);
  define('CSSConditionRule', CSSConditionRule);
  define('CSSMediaRule', CSSMediaRule);
  define('CSSSupportsRule', CSSSupportsRule);
  define('CSSContainerRule', CSSContainerRule);
  define('CSSLayerBlockRule', CSSLayerBlockRule);
  define('CSSLayerStatementRule', CSSLayerStatementRule);
  define('CSSScopeRule', CSSScopeRule);
  define('CSSStartingStyleRule', CSSStartingStyleRule);
  define('CSSPageRule', CSSPageRule);
  define('CSSMarginRule', CSSMarginRule);
  define('CSSFontFaceRule', CSSFontFaceRule);
  define('CSSImportRule', CSSImportRule);
  define('CSSNamespaceRule', CSSNamespaceRule);
  define('CSSKeyframeRule', CSSKeyframeRule);
  define('CSSKeyframesRule', CSSKeyframesRule);
  define('CSSCounterStyleRule', CSSCounterStyleRule);
  define('CSSFontFeatureValuesRule', CSSFontFeatureValuesRule);
  define('CSSPropertyRule', CSSPropertyRule);
  define('CSSNestedDeclarations', CSSNestedDeclarations);
  // Shared with other JS-implemented APIs.
  hidden(g, '__rhCssom', { parseDecls, makeDecl, kebab });
})();
