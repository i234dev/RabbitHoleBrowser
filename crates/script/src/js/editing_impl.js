// HTML editing commands operate on the active selection in a single editing
// host. Each document keeps its own editing preferences and reentrancy flag.
// Compiled on first use by editing.js; evaluates to a factory receiving the
// natives and the spec algorithms (editing_spec.js), and returning the
// Document methods and the keyboard editing entry point.
(function (N, spec) {
  'use strict';

  const checkVisibility = Element.prototype.checkVisibility;
  const contexts = new WeakMap();
  function context(doc) {
    if (!(doc instanceof Document)) throw new TypeError('Expected Document');
    if (doc.contentType !== 'text/html' && doc.contentType !== 'application/xhtml+xml')
      throw new DOMException('Editing commands require an HTML document', 'InvalidStateError');
    let state = contexts.get(doc);
    if (!state) {
      state = { css: false, separator: 'div', executing: false };
      contexts.set(doc, state);
    }
    return state;
  }

  const formats = new Map([
    ['bold', ['b', 'font-weight', 'bold', 'normal', 'formatBold']],
    ['italic', ['i', 'font-style', 'italic', 'normal', 'formatItalic']],
    ['underline', ['u', 'text-decoration-line', 'underline', 'none', 'formatUnderline']],
    ['strikethrough', ['s', 'text-decoration-line', 'line-through', 'none', 'formatStrikeThrough']],
    ['subscript', ['sub', 'vertical-align', 'sub', 'baseline', 'formatSubscript']],
    ['superscript', ['sup', 'vertical-align', 'super', 'baseline', 'formatSuperscript']],
  ]);
  const valueFormats = new Map([
    ['forecolor', ['color', 'color', 'formatFontColor']],
    ['backcolor', ['background-color', '', 'formatBackColor']],
    ['fontsize', ['font-size', 'size', '']],
    ['fontname', ['font-family', 'face', 'formatFontName']],
  ]);
  const sizes = ['x-small','small','medium','large','x-large','xx-large','xxx-large'];
  const sizePixels = [10,13,16,18,24,32,48];
  function formatValue(el, cmd) {
    if (!el) return '';
    const prop = valueFormats.get(cmd)[0];
    let value = getComputedStyle(el).getPropertyValue(prop);
    if (cmd === 'backcolor') {
      for (let p = el.parentElement; p && value === 'rgba(0, 0, 0, 0)' && p.isContentEditable; p = p.parentElement)
        value = getComputedStyle(p).backgroundColor;
    }
    if (cmd === 'fontsize') {
      const px = parseFloat(value);
      let best = 0;
      for (let i = 1; i < sizePixels.length; ++i)
        if (Math.abs(px - sizePixels[i]) < Math.abs(px - sizePixels[best])) best = i;
      return String(best + 1);
    }
    return value;
  }
  function normalizeFormat(cmd, value) {
    if (cmd === 'fontsize') {
      const m = /^([+-]?)([0-9]+)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?$/.exec(value.replace(/^[\t\n\f\r ]+|[\t\n\f\r ]+$/g, ''));
      if (!m) return null;
      const n = m[1] ? 3 + Number(m[1] + m[2]) : Number(m[2]);
      return String(Math.min(7, Math.max(1, n)));
    }
    if (cmd === 'fontname') return value.trim() || null;
    return N.editingColor(value) ||
      (/^[0-9a-f]{3}(?:[0-9a-f]{3})?$/i.test(value) ? N.editingColor('#' + value) : null);
  }
  function wrapValue(node, cmd, value, css) {
    const [prop, attr] = valueFormats.get(cmd);
    const match = cmd === 'forecolor' && /^rgb\((\d+), (\d+), (\d+)\)$/.exec(value);
    const legacy = !css && attr && (cmd !== 'forecolor' || match);
    const wrapper = node.ownerDocument.createElement(legacy ? 'font' : 'span');
    if (legacy) {
      const attrValue = match ? '#' + match.slice(1).map(v => Number(v).toString(16).padStart(2,'0')).join('') : value;
      wrapper.setAttribute(attr, attrValue);
    } else wrapper.style.setProperty(prop, cmd === 'fontsize' ? sizes[Number(value) - 1] : value);
    node.parentNode.insertBefore(wrapper, node);
    wrapper.appendChild(node);
  }
  function selectedValue(state, t, cmd, mixed) {
    if (!t?.range) return mixed ? false : '';
    const {range} = t;
    if (range.collapsed) {
      if (mixed) return false;
      const pending = typingStyle(state, range);
      return pending?.get(cmd) ?? formatValue(selectionElement(range), cmd);
    }
    const runs = formattableRuns(t);
    const first = formatValue(runs[0]?.node.parentElement || selectionElement(range), cmd);
    return mixed ? runs.some(r => formatValue(r.node.parentElement, cmd) !== first) : first;
  }

  const isScriptFormat = cmd => cmd === 'subscript' || cmd === 'superscript';
  function scriptFormat(el) {
    let sub=false, sup=false;
    while (el && !isBlock(el)) {
      sub ||= el.localName === 'sub';
      sup ||= el.localName === 'sup';
      el = el.parentElement;
    }
    return sub && sup ? 'mixed' : sub ? 'subscript' : sup ? 'superscript' : '';
  }
  function activeFormat(el, cmd) {
    if (!el) return false;
    if (isScriptFormat(cmd)) return scriptFormat(el) === cmd;
    const [, prop, on] = formats.get(cmd);
    const style = getComputedStyle(el).getPropertyValue(prop);
    if (cmd === 'bold') return style === 'bold' || Number(style) >= 600;
    if (style.split(' ').includes(on)) return true;
    if (['underline','strikethrough'].includes(cmd)) {
      for (let p = el.parentElement; p; p = p.parentElement) {
        if (getComputedStyle(p).getPropertyValue(prop).split(' ').includes(on)) return true;
      }
    }
    return false;
  }
  // Boundary text nodes are split only when formatting actually changes.
  // Excluding indices at the open ends avoids styling adjacent siblings.
  function selectedText(t) {
    const { range, host } = t;
    const root = range.commonAncestorContainer, result = [], stack = [root];
    while (stack.length) {
      const node = stack.pop();
      if (!range.intersectsNode(node)) continue;
      if (node.nodeType === 3) {
        const from = node === range.startContainer ? range.startOffset : 0;
        const to = node === range.endContainer ? range.endOffset : node.length;
        if (from < to && editingHost(node) === host) result.push({ node, from, to });
      } else {
        for (let child = node.lastChild; child; child = child.previousSibling) stack.push(child);
      }
    }
    return result;
  }
  // Selected text runs that formatting applies to: collapsed whitespace
  // is invisible unless the selection holds nothing else.
  function formattableRuns(t) {
    const runs = selectedText(t), visibleRuns = runs.filter(r => !collapsedWhitespace(r.node));
    return visibleRuns.length ? visibleRuns : runs;
  }
  function selectionElement(range) {
    const n = range.startContainer;
    if (n.nodeType === 3) return n.parentElement;
    const child = n.childNodes[range.startOffset] || n.childNodes[range.startOffset - 1];
    return child?.nodeType === 1 ? child : n.nodeType === 1 ? n : null;
  }
  function typingStyle(state, range) {
    const previous = state.typing;
    if (previous && range.collapsed && previous.node === range.startContainer && previous.offset === range.startOffset) return previous.formats;
    state.typing = null;
    return null;
  }
  function formatState(state, t, cmd, mixed) {
    if (!t?.range) return false;
    if (t.range.collapsed) {
      if (mixed) return false;
      const pending = typingStyle(state, t.range);
      return pending?.has(cmd) ? pending.get(cmd) : activeFormat(selectionElement(t.range), cmd);
    }
    const runs = formattableRuns(t);
    if (mixed && isScriptFormat(cmd) && runs.some(r => scriptFormat(r.node.parentElement) === 'mixed')) return true;
    const values = runs.map(r => activeFormat(r.node.parentElement, cmd));
    if (!values.length) return !mixed && activeFormat(selectionElement(t.range), cmd);
    return mixed ? values.includes(true) && values.includes(false) : values.every(Boolean);
  }
  function splitAncestors(node, ancestor) {
    let child = node;
    for (;;) {
      const el = child.parentNode;
      if (child.previousSibling) {
        const left = el.cloneNode(false);
        left.removeAttribute('id');
        el.parentNode.insertBefore(left, el);
        while (el.firstChild !== child) left.appendChild(el.firstChild);
      }
      if (child.nextSibling) {
        const right = el.cloneNode(false);
        right.removeAttribute('id');
        el.parentNode.insertBefore(right, el.nextSibling);
        while (child.nextSibling) right.appendChild(child.nextSibling);
      }
      if (el === ancestor) break;
      child = el;
    }
  }
  function clearFormat(node, host, cmd) {
    const valued = valueFormats.get(cmd);
    const [tag, prop, on] = valued ? [null, valued[0], null] : formats.get(cmd);
    for (let el = node.parentElement; el && el !== host;) {
      const parent = el.parentElement;
      const scripts = isScriptFormat(cmd);
      const semantic = scripts && (el.localName === 'sup' || el.localName === 'sub') || el.localName === tag || (cmd === 'bold' && el.localName === 'strong') ||
        (cmd === 'italic' && el.localName === 'em') || (cmd === 'strikethrough' && el.localName === 'strike');
      const inline = !scripts && (el.style.getPropertyValue(prop) ||
        (prop === 'text-decoration-line' && el.style.getPropertyValue('text-decoration')));
      const attr = valued?.[1];
      const legacy = attr && el.localName === 'font' && el.hasAttribute(attr);
      if (semantic || inline || legacy) {
        // Do not split block containers to cancel inherited formatting.
        if (!['inline','contents'].includes(getComputedStyle(el).display)) break;
        splitAncestors(node, el);
        if (inline) {
          if (prop === 'text-decoration-line') {
            el.style.removeProperty('text-decoration');
            const rest = inline.split(' ').filter(v => v !== on).join(' ');
            el.style.setProperty(prop, rest);
          } else el.style.removeProperty(prop);
          if (!el.style.cssText) el.removeAttribute('style');
        }
        if (legacy) el.removeAttribute(attr);
        if (semantic || (legacy && !el.attributes.length) || (el.localName === 'span' && !el.attributes.length)) {
          let dest = parent;
          if (el.attributes.length) {
            dest = el.ownerDocument.createElement('span');
            for (const a of Array.from(el.attributes)) dest.setAttribute(a.name, a.value);
            parent.insertBefore(dest, el);
          }
          while (el.firstChild) {
            if (dest === parent) parent.insertBefore(el.firstChild, el);
            else dest.appendChild(el.firstChild);
          }
          el.remove();
        }
      }
      el = parent;
    }
  }
  const removableInline = new Set(['abbr','acronym','b','bdi','bdo','big','blink','cite',
    'code','dfn','em','font','i','ins','kbd','mark','nobr','q','s','samp','small','span',
    'strike','strong','sub','sup','tt','u','var']);
  function removable(element, host) {
    return element && element !== host && removableInline.has(element.localName) &&
      element.namespaceURI === 'http://www.w3.org/1999/xhtml' && element.isContentEditable;
  }
  function unwrap(element) {
    const parent = element.parentNode;
    while (element.firstChild) parent.insertBefore(element.firstChild, element);
    element.remove();
  }
  const formatProperties = ['vertical-align','font-weight','font-family','font-size','color',
    'background-color','font-style','text-decoration','text-decoration-line'];
  function removeFormatting(state, t) {
    const {host, range, sel} = t;
    state.typing = null;
    if (range.collapsed) return true;
    const runs = selectedText(t), backwards = sel.direction === 'backward';
    const root = range.commonAncestorContainer, whole = [], stack = [root];
    while (stack.length) {
      const node = stack.pop();
      if (node.nodeType !== 1 || !node.isContentEditable || !range.intersectsNode(node)) continue;
      if (removable(node,host) && range.comparePoint(node,0) === 0 && range.comparePoint(node,node.childNodes.length) === 0)
        whole.push(node);
      for (let child = node.lastElementChild; child; child = child.previousElementSibling) stack.push(child);
    }
    for (const el of whole) unwrap(el);
    let changed = whole.length !== 0;
    for (const run of runs) {
      let node = run.node;
      const ancestors = [];
      for (let el = node.parentElement; el && el !== host && !isBlock(el); el = el.parentElement) {
        if (removable(el,host)) ancestors.push({el,unwrap:true});
        else if (el.hasAttribute('style')) {
          const properties = formatProperties.filter(prop => el.style.getPropertyValue(prop));
          if (properties.length) ancestors.push({el,properties});
        }
      }
      if (!ancestors.length) continue;
      if (run.to < node.length) node.splitText(run.to);
      if (run.from) node = node.splitText(run.from);
      run.to -= run.from;
      run.from = 0;
      run.node = node;
      for (const item of ancestors) {
        splitAncestors(node,item.el);
        if (item.unwrap) unwrap(item.el);
        else {
          for (const prop of item.properties) item.el.style.removeProperty(prop);
          if (!item.el.style.cssText) item.el.removeAttribute('style');
        }
        changed = true;
      }
    }
    if (changed && runs.length) {
      const first = runs[0], last = runs[runs.length-1];
      if (backwards) sel.setBaseAndExtent(last.node,last.to,first.node,first.from);
      else sel.setBaseAndExtent(first.node,first.from,last.node,last.to);
    }
    if (changed) input(host,'formatRemove');
    return true;
  }
  function wrapFormat(node, cmd, enable, css) {
    const [tag, prop, on, off] = formats.get(cmd);
    if (isScriptFormat(cmd)) css = false;
    const wrapper = node.ownerDocument.createElement(css || !enable ? 'span' : tag);
    if (css || !enable) wrapper.style.setProperty(prop, enable ? on : off);
    node.parentNode.insertBefore(wrapper, node);
    wrapper.appendChild(node);
  }
  function applyFormat(state, t, cmd, value) {
    const valued = valueFormats.has(cmd);
    const enable = valued ? value : !formatState(state, t, cmd, false);
    const { sel, range, host } = t;
    if (range.collapsed) {
      const toggles = typingStyle(state, range) || new Map();
      if (isScriptFormat(cmd)) toggles.delete(cmd === 'subscript' ? 'superscript' : 'subscript');
      toggles.set(cmd, enable);
      state.typing = { node: range.startContainer, offset: range.startOffset, formats: toggles };
      return;
    }
    const runs = selectedText(t), backward = sel.direction === 'backward';
    for (const r of runs) {
      // Collapsed whitespace is invisible: formatting leaves it alone.
      if (collapsedWhitespace(r.node)) continue;
      if ((valued ? formatValue(r.node.parentElement, cmd) : activeFormat(r.node.parentElement, cmd)) === enable) continue;
      if (r.to < r.node.length) r.node.splitText(r.to);
      if (r.from) r.node = r.node.splitText(r.from);
      r.to -= r.from;
      r.from = 0;
      if (valued || !enable || isScriptFormat(cmd)) clearFormat(r.node, host, cmd);
      if (valued) wrapValue(r.node, cmd, value, state.css);
      else if (activeFormat(r.node.parentElement, cmd) !== enable) wrapFormat(r.node, cmd, enable, state.css);
    }
    if (runs.length) {
      const first = runs[0], last = runs[runs.length - 1];
      if (backward) sel.setBaseAndExtent(last.node,last.to,first.node,first.from);
      else sel.setBaseAndExtent(first.node,first.from,last.node,last.to);
      input(host, valued ? valueFormats.get(cmd)[2] : formats.get(cmd)[4], valued ? value : null);
    }
  }

  function putLink(node, href, host) {
    const parent = node.parentElement, current = parent?.closest('a');
    if (current && current !== host && editingHost(current) === host) {
      if (current.getAttribute('href') !== href) splitAncestors(node, current);
      current.setAttribute('href',href);
      return current;
    }
    const prev = node.previousSibling;
    const link = prev?.localName === 'a' && prev.attributes.length === 1 && prev.getAttribute('href') === href
      ? prev : node.ownerDocument.createElement('a');
    if (!link.parentNode) {
      link.setAttribute('href',href);
      node.parentNode.insertBefore(link,node);
    }
    link.appendChild(node);
    return link;
  }

  function editLink(t, state, href) {
    const {range,host,sel} = t, remove = href === null;
    if (href === '') return false;
    const backward = sel.direction === 'backward', links = new Set(), units = [];
    const ends = [bookmark(sel.anchorNode,sel.anchorOffset),bookmark(sel.focusNode,sel.focusOffset)];
    const replacements = new Map();
    const addLink = el => {
      const link = el?.closest('a[href]');
      if (link && editingHost(link) === host && link !== host) links.add(link);
    };
    if (range.collapsed) {
      const pending = typingStyle(state,range) || new Map();
      if (!remove) pending.set('createlink',href);
      else { pending.delete('createlink'); addLink(range.startContainer.nodeType === 1 ? range.startContainer : range.startContainer.parentElement); }
      state.typing = {node:range.startContainer,offset:range.startOffset,formats:pending};
    } else {
      state.typing = null;
      const stack = [range.commonAncestorContainer];
      while (stack.length) {
        const n = stack.pop();
        if (!range.intersectsNode(n) || editingHost(n) !== host) continue;
        if (n.nodeType === 3) {
          const from = n === range.startContainer ? range.startOffset : 0;
          const to = n === range.endContainer ? range.endOffset : n.length;
          if (to > from) { units.push({node:n,from,to}); addLink(n.parentElement); }
        } else if (n.nodeType === 1) {
          if (n.matches('a[href]')) addLink(n);
          if (['br','img'].includes(n.localName)) { units.push({node:n}); addLink(n); }
          else for (let c=n.lastChild;c;c=c.previousSibling) stack.push(c);
        }
      }
    }
    for (const link of links) {
      if (!remove) { link.setAttribute('href',href); continue; }
      link.removeAttribute('href');
      if (!link.attributes.length) {
        replacements.set(link,link.parentNode);
        unwrap(link);
      }
    }
    if (remove) {
      if (links.size) {
        sel.setBaseAndExtent(...restorePoint(ends[0],replacements),...restorePoint(ends[1],replacements));
        input(host,'');
      }
      return true;
    }
    for (const u of units) {
      if (u.node.parentElement?.closest('a[href]')) continue;
      if (u.node.nodeType === 3) {
        if (u.to < u.node.length) u.node.splitText(u.to);
        if (u.from) u.node = u.node.splitText(u.from);
        u.to -= u.from; u.from = 0;
      }
      putLink(u.node,href,host);
    }
    if (units.length) {
      const point = (u,end) => u.node.nodeType === 3 ? [u.node,end?u.to:u.from] :
        [u.node.parentNode,Array.prototype.indexOf.call(u.node.parentNode.childNodes,u.node) + Number(end)];
      const a=point(units[0],false), b=point(units[units.length-1],true);
      if (backward) sel.setBaseAndExtent(...b,...a); else sel.setBaseAndExtent(...a,...b);
      input(host,'insertLink',href);
    }
    return true;
  }

  const alignments = new Map([
    ['justifyleft','left'], ['justifyright','right'], ['justifycenter','center'], ['justifyfull','justify']
  ]);
  const blockTags = new Set(['address','dd','div','dt','h1','h2','h3','h4','h5','h6','p','pre']);
  const listCommands = new Map([['insertorderedlist','ol'],['insertunorderedlist','ul']]);
  const blockCommand = cmd => alignments.has(cmd) || cmd === 'formatblock' || listCommands.has(cmd);
  function isBlock(node) {
    return node?.nodeType === 1 && !['inline','contents','inline-block','inline-flex','inline-grid','none'].includes(getComputedStyle(node).display);
  }
  function alignOf(el) {
    const s=getComputedStyle(el), a=s.textAlign;
    if (a === 'start' || a === 'end') return (a === 'start') === (s.direction !== 'rtl') ? 'left' : 'right';
    return a;
  }
  // Identify the entire visual paragraph containing a selected leaf. When
  // inline children share an editing host or a table cell, limit the run
  // to adjacent inline content, stopping at block or line-break boundaries.
  function paragraph(host, node, byLine) {
    let top = node;
    while (top !== host && top.parentNode !== host && !isBlock(top)) top = top.parentNode;
    if (!byLine && top !== host && isBlock(top) && !['li','td','th'].includes(top.localName))
      return {container:top.parentNode, nodes:[top], block:top};
    const container = top === host || isBlock(top) ? top : top.parentNode;
    if (node===container) node=container.firstChild;
    if (!node) return {container,nodes:[],block:null};
    while (node.parentNode!==container) node=node.parentNode;
    let first=node, last=node;
    while (first.previousSibling && !isBlock(first.previousSibling) && first.previousSibling.localName!=='br') first=first.previousSibling;
    while (last.nextSibling && !isBlock(last.nextSibling) && last.localName!=='br') last=last.nextSibling;
    const nodes=[];
    for (let n=first;n;n=n.nextSibling) { nodes.push(n); if (n===last) break; }
    return {container,nodes,block:null};
  }
  function paragraphs(t, byLine = false) {
    let nodes = t.range.collapsed ? [] : selectedText(t).map(run=>run.node);
    if (!nodes.length) {
      const point=t.range.startContainer;
      nodes=[point.nodeType===3 ? point : point.childNodes[t.range.startOffset] || point.lastChild || point];
    }
    const seen=new Set(), result=[];
    for (const node of nodes) {
      if (editingHost(node)!==t.host) continue;
      const run=paragraph(t.host,node,byLine), key=run.nodes[0] || run.container;
      if (!seen.has(key)) { seen.add(key); result.push(run); }
    }
    return result;
  }
  function blockValue(run, cmd) {
    if (cmd==='formatblock') return blockTags.has(run.block?.localName) ? run.block.localName : '';
    return alignOf(run.block || run.container);
  }
  function blockQuery(t,cmd,kind) {
    if (!t?.range) return kind==='value' ? '' : false;
    if (listCommands.has(cmd)) {
      if (kind === 'value') return '';
      const items = listParagraphs(t), tag = listCommands.get(cmd);
      // A selection anchored on rows selects table cells as structural units.
      if (t.range.startContainer.localName === 'tr' && t.range.endContainer.localName === 'tr') return false;
      const state = selectionListState(items,tag);
      return kind === 'mixed' ? state === 'mixed' : state === 'active';
    }
    const values=paragraphs(t).map(run=>blockValue(run,cmd));
    if (kind==='value') return values[0] || '';
    if (kind==='mixed') return values.some(v=>v!==values[0]);
    return alignments.has(cmd) && values.length>0 && values.every(v=>v===alignments.get(cmd));
  }
  function bookmark(node, offset) {
    return {node,offset,next:node.childNodes[offset],prev:node.childNodes[offset-1]};
  }
  function restorePoint(p, replacements) {
    const adjacent=p.next || p.prev;
    if (adjacent?.parentNode && adjacent.parentNode!==p.node && adjacent.isConnected) {
      const index=Array.prototype.indexOf.call(adjacent.parentNode.childNodes,adjacent);
      return [adjacent.parentNode,index+(p.next?0:1)];
    }
    const n=replacements.get(p.node)||p.node;
    return [n, Math.min(p.offset,n.nodeType===3?n.length:n.childNodes.length)];
  }
  function listParagraphs(t) {
    const result = [], seen = new Set();
    const range = t.range, table = range.startContainer;
    if (table.localName === 'table' && table === range.endContainer && !range.collapsed &&
        range.startOffset === 0 && range.endOffset === table.childNodes.length && table !== t.host) {
      const parent = table.parentElement;
      const item = parent?.localName === 'li' && ['ol','ul'].includes(parent.parentElement?.localName) ? parent : null;
      return [{container:table.parentNode,nodes:[table],item}];
    }
    for (const run of paragraphs(t,true)) {
      const item = run.container.closest('li');
      if (item && item !== t.host && t.host.contains(item) && ['ol','ul'].includes(item.parentElement?.localName)) {
        if (!seen.has(item)) { seen.add(item); result.push({...run,item}); }
      } else result.push(run);
    }
    return result;
  }
  function selectionListState(runs,tag) {
    const other = tag === 'ol' ? 'ul' : 'ol';
    let present = false, pure = runs.length > 0;
    for (const {item} of runs) {
      const parent = item?.parentElement?.localName;
      if (parent === tag || item?.querySelector(tag)) present = true;
      if (parent !== tag || item.querySelector(other)) pure = false;
    }
    return pure ? 'active' : present ? 'mixed' : 'none';
  }
  function splitBeforeChild(parent, child) {
    if (!child.previousSibling) return;
    const before = parent.cloneNode(false);
    before.removeAttribute('id');
    parent.parentNode.insertBefore(before,parent);
    while (parent.firstChild !== child) before.appendChild(parent.firstChild);
  }
  function mergeAdjacentLists(list) {
    // Author-supplied starts, reversed numbering and styling delimit distinct lists.
    if (list.attributes.length) return;
    const left = list.previousSibling, right = list.nextSibling;
    if (left?.localName === list.localName && !left.attributes.length && left.isContentEditable) {
      while (left.lastChild) list.insertBefore(left.lastChild,list.firstChild);
      left.remove();
    }
    if (right?.localName === list.localName && !right.attributes.length && right.isContentEditable) {
      while (right.firstChild) list.appendChild(right.firstChild);
      right.remove();
    }
  }
  function liftListItem(item,old,host,separator,replacements) {
    const parent = old.parentElement;
    const outer = parent.localName === 'li' ? parent.parentElement : parent;
    if (['ol','ul'].includes(outer?.localName) && host.contains(outer)) {
      // Lift the selected item by one level. Its following siblings
      // stay at their original depth underneath it.
      outer.insertBefore(item,parent === outer ? old : parent.nextSibling);
      if (old.firstChild) item.appendChild(old);
    } else if (!item.attributes.length &&
        ((['td','th'].includes(parent.localName) && parent.childNodes.length === 1 && old.childNodes.length === 1) ||
         (item.childNodes.length === 1 && item.firstChild.localName === 'table'))) {
      while (item.firstChild) parent.insertBefore(item.firstChild,old);
      replacements.set(item,parent);
      item.remove();
    } else {
      const block = item.ownerDocument.createElement(separator);
      parent.insertBefore(block,old);
      for (const attr of Array.from(item.attributes)) block.setAttribute(attr.name,attr.value);
      while (item.firstChild) block.appendChild(item.firstChild);
      replacements.set(item,block);
      item.remove();
    }
  }
  function applyList(t,tag,separator) {
    const runs = listParagraphs(t), {host,sel} = t, doc = host.ownerDocument;
    if (!runs.length) return true;
    const remove = selectionListState(runs,tag) === 'active';
    const a = bookmark(sel.anchorNode,sel.anchorOffset), b = bookmark(sel.focusNode,sel.focusOffset);
    const replacements = new Map(), lists = [];
    for (const run of runs) {
      let item = run.item, list;
      if (item) {
        const old = item.parentElement;
        if (!remove && old.localName === tag) { lists.push(old); continue; }
        splitBeforeChild(old,item);
        if (remove) {
          liftListItem(item,old,host,separator,replacements);
        } else {
          list = doc.createElement(tag);
          old.parentNode.insertBefore(list,old);
          list.appendChild(item);
          lists.push(list);
        }
        if (!old.firstChild) old.remove();
        continue;
      }
      const container = run.nodes[0]?.parentNode || run.container;
      const first = run.nodes[0], last = run.nodes[run.nodes.length - 1];
      const replaceBlock = container !== host && first?.localName !== 'table' &&
          (container.localName === 'p' || container.localName === 'div');
      list = doc.createElement(tag);
      item = doc.createElement('li');
      list.appendChild(item);
      if (replaceBlock) {
        if (first) splitBeforeChild(container,first);
        container.parentNode.insertBefore(list,container);
        for (const attr of Array.from(container.attributes)) {
          if (attr.name !== 'id' || !last?.nextSibling) item.setAttribute(attr.name,attr.value);
        }
      } else container.insertBefore(list,first || null);
      const precedingBreak = list.previousSibling;
      if (precedingBreak?.localName === 'br' && precedingBreak.isContentEditable) precedingBreak.remove();
      for (const node of run.nodes) item.appendChild(node);
      if (item.childNodes.length > 1 && item.lastChild.localName === 'br') item.lastChild.remove();
      if (!item.firstChild) item.appendChild(caretBreak(doc));
      if (replaceBlock && !container.firstChild) { replacements.set(container,item); container.remove(); }
      lists.push(list);
    }
    for (const list of lists) if (list.parentNode) mergeAdjacentLists(list);
    sel.setBaseAndExtent(...restorePoint(a,replacements),...restorePoint(b,replacements));
    input(host,tag === 'ol' ? 'insertOrderedList' : 'insertUnorderedList');
    return true;
  }
  function applyIndent(t,indent,separator) {
    const {host,sel} = t, doc = host.ownerDocument;
    // A caret between two block boxes does not select either paragraph.
    if (t.range.collapsed) {
      const {startContainer,startOffset} = t.range;
      if (isBlock(startContainer.childNodes[startOffset - 1]) &&
          isBlock(startContainer.childNodes[startOffset])) return true;
    }
    const a = bookmark(sel.anchorNode,sel.anchorOffset), b = bookmark(sel.focusNode,sel.focusOffset);
    const replacements = new Map(), handled = new Set();
    let changed = false;
    for (const run of listParagraphs(t)) {
      let nodes = run.nodes, container = run.container;
      if (run.item) {
        const list = run.item.parentElement;
        if (!indent) {
          splitBeforeChild(list,run.item);
          liftListItem(run.item,list,host,separator,replacements);
          if (!list.firstChild) list.remove();
        } else {
          const sub = doc.createElement(list.localName);
          list.insertBefore(sub,run.item);
          sub.appendChild(run.item);
          mergeAdjacentLists(sub);
        }
        changed = true;
        continue;
      }
      if (container !== host && blockTags.has(container.localName)) {
        if (handled.has(container)) continue;
        handled.add(container);
        nodes = [container];
        container = container.parentNode;
      }
      if (indent) {
        const first = nodes[0], left = first?.previousSibling;
        const quote = left?.localName === 'blockquote' && !left.attributes.length && left.isContentEditable ? left : doc.createElement('blockquote');
        if (!quote.parentNode) container.insertBefore(quote, first || null);
        for (const node of nodes) quote.appendChild(node);
        if (!nodes.length) quote.appendChild(caretBreak(doc));
        changed = true;
      } else {
        const quote = container.closest('blockquote');
        if (!quote || quote === host || !host.contains(quote) || !nodes.length) continue;
        // Move just the selected children out, retaining quoted siblings
        // on either side and preserving their original order and style.
        const selected = new Set();
        for (let node of nodes) {
          while (node.parentNode && node.parentNode !== quote) node = node.parentNode;
          if (node.parentNode === quote) selected.add(node);
        }
        for (const node of selected) {
          splitBeforeChild(quote,node);
          quote.parentNode.insertBefore(node,quote);
          changed = true;
        }
        if (!quote.firstChild) { replacements.set(quote,quote.parentNode); quote.remove(); }
      }
    }
    if (changed) {
      sel.setBaseAndExtent(...restorePoint(a,replacements),...restorePoint(b,replacements));
      input(host,indent ? 'formatIndent' : 'formatOutdent');
    }
    return true;
  }
  function applyBlock(t,cmd,value) {
    const align=alignments.get(cmd);
    let tag=value.toLowerCase();
    if (cmd==='formatblock') {
      if (tag.startsWith('<') && tag.endsWith('>')) tag=tag.slice(1,-1);
      if (!blockTags.has(tag)) return false;
    }
    const {sel,host}=t, doc=host.ownerDocument;
    const anchor=bookmark(sel.anchorNode,sel.anchorOffset), focus=bookmark(sel.focusNode,sel.focusOffset);
    const replacements=new Map();
    let prev=null, changed=false;
    for (const run of paragraphs(t)) {
      if (blockValue(run,cmd)===(align || tag)) { prev=null; continue; }
      const {block,container,nodes}=run;
      changed=true;
      if (align && block?.localName==='div') {
        block.removeAttribute('align');
        block.style.textAlign=align;
        prev=block;
      } else if (!align && block && blockTags.has(block.localName)) {
        const el=doc.createElement(tag);
        for (const a of Array.from(block.attributes)) el.setAttribute(a.name,a.value);
        container.insertBefore(el,block);
        while (block.firstChild) el.appendChild(block.firstChild);
        replacements.set(block,el); block.remove(); prev=null;
      } else {
        const first=nodes[0];
        let wrapper;
        if (align && prev && first?.previousSibling===prev && prev.parentNode===container) wrapper=prev;
        else {
          wrapper=doc.createElement(align?'div':tag);
          if (align) wrapper.style.textAlign=align;
          container.insertBefore(wrapper,first||null);
        }
        if (align && block) { block.removeAttribute('align'); block.style.removeProperty('text-align'); }
        for (const node of nodes) wrapper.appendChild(node);
        if (!nodes.length) wrapper.appendChild(doc.createElement('br'));
        prev=wrapper;
      }
    }
    if (changed) {
      sel.setBaseAndExtent(...restorePoint(anchor,replacements),...restorePoint(focus,replacements));
      const types={left:'formatJustifyLeft',right:'formatJustifyRight',center:'formatJustifyCenter',justify:'formatJustifyFull'};
      input(host,types[align] || '');
    }
    return true;
  }


  // ---------------------------------------------------------------------
  // Justification (editing spec "justify the selection"): block-extend the
  // selection, clear existing alignment, then wrap runs of sibling nodes in
  // aligned divs, merging with aligned siblings.
  const notInDiv = new Set(['li','dt','dd','tbody','thead','tfoot','tr','td','th','caption','col','colgroup','html','head','body',
    'title','meta','base','link','style','option','optgroup','frame','frameset']);
  const isElement = (n, name) => n?.nodeType === 1 && (!name || n.localName === name);
  // Editable (not an editing host, inside one, not contenteditable=false).
  // Read from the contenteditable attributes (and designMode) so nodes
  // inserted during the command are classified without a style update.
  function inEditableContent(el) {
    const doc = el?.ownerDocument;
    for (; el?.nodeType === 1; el = el.parentElement) {
      const v = el.getAttribute('contenteditable');
      if (v === null) continue;
      const lower = v.toLowerCase();
      if (lower === '' || lower === 'true' || lower === 'plaintext-only') return true;
      if (lower === 'false') return false;
    }
    return doc?.designMode === 'on';
  }
  // Editable: in editable content, not contenteditable=false and not an
  // editing host (its parent is editable content too).
  const editable = (n) => {
    if (!n?.parentNode) return false;
    if (n.nodeType === 1) return !!n.parentElement && inEditableContent(n) && inEditableContent(n.parentElement);
    return inEditableContent(n.parentElement);
  };
  const blockNode = (n) => n && (n.nodeType === 9 || n.nodeType === 11 || isBlock(n));
  const inlineNode = (n) => n && !blockNode(n);
  const hiddenByStyle = (n) => {
    for (let el = n.nodeType === 1 ? n : n.parentElement; el; el = el.parentElement) if (getComputedStyle(el).display === 'none') return true;
    return false;
  };
  function collapsedWhitespace(node) {
    if (node.nodeType !== 3) return false;
    if (node.data === '') return true;
    if (!/^[ \t\n\r]*$/.test(node.data)) return false;
    const parent = node.parentElement;
    if (!parent) return true;
    const ws = getComputedStyle(parent).whiteSpace;
    if (ws === 'pre' || ws === 'pre-wrap' || ws === 'break-spaces') return false;
    if (getComputedStyle(parent).display === 'none') return true;
    let ancestor = node.parentNode;
    while (!blockNode(ancestor) && ancestor.parentNode) ancestor = ancestor.parentNode;
    const solid = (r) => (r.nodeType === 3 && !/^[ \t\n\r]*$/.test(r.data)) || isElement(r, 'img');
    for (let r = previousNode(node); r && ancestor.contains(r) && r !== ancestor; r = previousNode(r)) {
      if (blockNode(r) || isElement(r, 'br')) return true;
      if (solid(r)) break;
    }
    for (let r = nextNode(node); r && ancestor.contains(r); r = nextNode(r)) {
      if (blockNode(r) || isElement(r, 'br')) return true;
      if (solid(r)) break;
    }
    return false;
  }
  function previousNode(n) {
    if (n.previousSibling) { n = n.previousSibling; while (n.lastChild) n = n.lastChild; return n; }
    return n.parentNode;
  }
  function nextNode(n) {
    if (n.firstChild) return n.firstChild;
    while (n && !n.nextSibling) n = n.parentNode;
    return n?.nextSibling || null;
  }
  function nextNodeAfter(n) {
    while (n && !n.nextSibling) n = n.parentNode;
    return n?.nextSibling || null;
  }
  // A <br> with no visual effect: content precedes it on its line and
  // nothing visible follows it before the end of the block.
  function extraneousBreak(br) {
    if (!isElement(br, 'br')) return false;
    if (isElement(br.parentNode, 'li') && br.parentNode.childNodes.length === 1) return false;
    let block = br.parentNode;
    while (block && !blockNode(block)) block = block.parentNode;
    if (!block) return false;
    for (let r = nextNode(br); r && block.contains(r); r = nextNode(r)) {
      if (blockNode(r)) break;
      if (isElement(r, 'br') || isElement(r, 'img') || (r.nodeType === 3 && !collapsedWhitespace(r))) return false;
    }
    for (let r = previousNode(br); r && r !== block; r = previousNode(r)) {
      if (blockNode(r) || isElement(r, 'br')) return false;
      if (isElement(r, 'img') || (r.nodeType === 3 && !collapsedWhitespace(r))) return true;
    }
    return false;
  }
  function visible(n) {
    if (!n || hiddenByStyle(n)) return false;
    const walk = (x) => {
      if (x.nodeType === 3) return !collapsedWhitespace(x);
      if (x.nodeType !== 1) return false;
      if (isBlock(x) || x.localName === 'img') return true;
      if (x.localName === 'br') return !extraneousBreak(x);
      if (getComputedStyle(x).display === 'none') return false;
      for (let c = x.firstChild; c; c = c.nextSibling) if (walk(c)) return true;
      return false;
    };
    return walk(n);
  }
  const nodeLength = (n) => n.nodeType === 3 || n.nodeType === 8 || n.nodeType === 7 ? n.data.length : n.childNodes.length;
  const indexOf = (n) => { let i = 0; while ((n = n.previousSibling)) i++; return i; };
  function blockStartPoint(node, offset) {
    if (!node.parentNode && offset === 0) return true;
    const child = node.childNodes[offset - 1];
    return !!child && visible(child) && (blockNode(child) || isElement(child, 'br'));
  }
  function blockEndPoint(node, offset) {
    if (!node.parentNode && offset === nodeLength(node)) return true;
    const child = node.childNodes[offset];
    return !!child && visible(child) && blockNode(child);
  }
  function blockExtend(range) {
    let sn = range.startContainer, so = range.startOffset, en = range.endContainer, eo = range.endOffset;
    let li = null;
    for (let a = sn; a; a = a.parentNode) if (isElement(a, 'li')) li = a;
    if (li) { so = indexOf(li); sn = li.parentNode; }
    if (!blockStartPoint(sn, so)) {
      for (;;) {
        if (so === 0) { if (!sn.parentNode) break; so = indexOf(sn); sn = sn.parentNode; } else so--;
        if (blockStartPoint(sn, so) || blockEndPoint(sn, so)) break;
      }
    }
    while (so === 0 && sn.parentNode) { so = indexOf(sn); sn = sn.parentNode; }
    li = null;
    for (let a = en; a; a = a.parentNode) if (isElement(a, 'li')) li = a;
    if (li) { eo = indexOf(li) + 1; en = li.parentNode; }
    if (!blockEndPoint(en, eo)) {
      for (;;) {
        if (eo === nodeLength(en)) { if (!en.parentNode) break; eo = indexOf(en) + 1; en = en.parentNode; } else eo++;
        if (blockStartPoint(en, eo) || blockEndPoint(en, eo)) break;
      }
    }
    while (eo === nodeLength(en) && en.parentNode) { eo = indexOf(en) + 1; en = en.parentNode; }
    const r = en.ownerDocument ? en.ownerDocument.createRange() : en.createRange();
    r.setStart(sn, so); r.setEnd(en, eo);
    return r;
  }
  // Nodes contained in a range (DOM "contained"), in tree order.
  function containedNodes(range) {
    const out = [], root = range.commonAncestorContainer;
    for (let n = root.firstChild; n && root.contains(n) && n !== root; n = nextNode(n)) {
      try {
        const len = nodeLength(n);
        if (range.comparePoint(n, 0) >= 0 && !(range.startContainer === n && range.startOffset === 0) &&
            range.comparePoint(n, len) <= 0 && !(range.endContainer === n && range.endOffset === len)) out.push(n);
      } catch (e) { /* doctype */ }
    }
    return out;
  }
  // Move a node, preserving the selection (editing spec "preserving
  // ranges"): boundaries inside the node or right around it move with it.
  function moveNode(node, parent, ref, sel) {
    const oldParent = node.parentNode, oldIndex = oldParent ? indexOf(node) : -1;
    const points = sel && sel.rangeCount ? [[sel.anchorNode, sel.anchorOffset], [sel.focusNode, sel.focusOffset]] : null;
    parent.insertBefore(node, ref);
    if (!points) return;
    const newIndex = indexOf(node);
    const moved = points.map(([n, o]) => {
      if (n === node || node.contains(n)) return [n, o];
      if (n === oldParent && (o === oldIndex || o === oldIndex + 1)) return [parent, newIndex + o - oldIndex];
      let off = o;
      if (n === oldParent && o > oldIndex + 1) off--;
      if (n === parent && off > newIndex) off++;
      return [n, off];
    });
    try { sel.setBaseAndExtent(moved[0][0], moved[0][1], moved[1][0], moved[1][1]); } catch (e) { /* detached */ }
  }
  function removeExtraneousBreaks(node) {
    removeBreaksBefore(node);
    removeBreaksAtEnd(node);
  }
  function followsLineBreak(node) {
    let offset = 0;
    while (!(blockStartPoint(node, offset) || blockEndPoint(node, offset))) {
      const child = node.childNodes[offset - 1];
      if (child && visible(child)) return false;
      if (offset === 0 || !node.firstChild) { if (!node.parentNode) return true; offset = indexOf(node); node = node.parentNode; }
      else { node = node.childNodes[offset - 1]; offset = nodeLength(node); }
    }
    return true;
  }
  function precedesLineBreak(node) {
    let offset = nodeLength(node);
    while (!(blockStartPoint(node, offset) || blockEndPoint(node, offset))) {
      const child = node.childNodes[offset];
      if (child && visible(child)) return false;
      if (offset === nodeLength(node) || !node.firstChild) { if (!node.parentNode) return true; offset = indexOf(node) + 1; node = node.parentNode; }
      else { node = node.childNodes[offset]; offset = 0; }
    }
    return true;
  }
  function removeBreaksBefore(node) {
    let ref = node.previousSibling;
    if (!ref) return;
    while (ref.lastChild) ref = ref.lastChild;
    while (ref && !visible(ref) && !extraneousBreak(ref) && ref !== node.parentNode) ref = previousNode(ref);
    if (ref && editable(ref) && extraneousBreak(ref) && editable(ref.parentNode)) ref.remove();
  }
  function removeBreaksAtEnd(node) {
    let ref = node;
    while (ref.lastChild) ref = ref.lastChild;
    while (ref && ref !== node && !visible(ref) && !extraneousBreak(ref)) ref = previousNode(ref);
    if (ref && ref !== node && editable(ref) && extraneousBreak(ref)) {
      while (editable(ref.parentNode) && ref.parentNode !== node && !visible(ref.parentNode)) ref = ref.parentNode;
      ref.remove();
    }
  }
  // Editing spec "split the parent" of sibling nodes.
  function splitParent(list, sel) {
    const original = list[0].parentNode;
    if (!original || !editable(original) || !original.parentNode) return;
    const doc = original.ownerDocument;
    const first = list[0], last = list[list.length - 1];
    if (list.includes(original.firstChild)) removeBreaksBefore(original);
    const follows = list.includes(original.firstChild) && followsLineBreak(original);
    const precedes = list.includes(original.lastChild) && precedesLineBreak(original);
    if (!list.includes(original.firstChild) && list.includes(original.lastChild)) {
      for (let i = list.length - 1; i >= 0; i--) moveNode(list[i], original.parentNode, original.nextSibling, sel);
      if (precedes && !precedesLineBreak(last)) last.parentNode.insertBefore(doc.createElement('br'), last.nextSibling);
      removeBreaksAtEnd(original);
      return;
    }
    if (!list.includes(original.firstChild)) {
      const clone = original.cloneNode(false);
      original.removeAttribute('id');
      original.parentNode.insertBefore(clone, original);
      while (first.previousSibling) moveNode(original.firstChild, clone, null, sel);
    }
    for (const n of list) moveNode(n, original.parentNode, original, sel);
    if (follows && !followsLineBreak(first)) first.parentNode.insertBefore(doc.createElement('br'), first);
    if (inlineNode(last) && !isElement(last, 'br') && isElement(original.firstChild, 'br') && !inlineNode(original)) original.firstChild.remove();
    if (!original.firstChild) {
      original.remove();
      if (precedes && !precedesLineBreak(last)) last.parentNode.insertBefore(doc.createElement('br'), last.nextSibling);
    } else removeBreaksBefore(original);
    if (!last.nextSibling && last.parentNode) removeBreaksAtEnd(last.parentNode);
  }
  function removePreserving(el, sel) {
    if (el.firstChild) splitParent(Array.from(el.childNodes), sel);
    else el.remove();
  }
  function wrapNodes(list, criteria, create, sel) {
    if (list.every((n) => !visible(n) && !isElement(n, 'br'))) return null;
    if (!list[0].parentNode) return null;
    const last = list[list.length - 1];
    if (inlineNode(last) && !isElement(last, 'br') && isElement(last.nextSibling, 'br')) list.push(last.nextSibling);
    while (list[0].previousSibling && !visible(list[0].previousSibling)) list.unshift(list[0].previousSibling);
    while (list[list.length - 1].nextSibling && !visible(list[list.length - 1].nextSibling)) list.push(list[list.length - 1].nextSibling);
    let parent;
    const prev = list[0].previousSibling, next = list[list.length - 1].nextSibling;
    if (prev && editable(prev) && criteria(prev)) parent = prev;
    else if (next && editable(next) && criteria(next)) parent = next;
    else parent = create();
    if (!parent) return null;
    if (!parent.parentNode) {
      const container = list[0].parentNode, index = indexOf(list[0]);
      container.insertBefore(parent, list[0]);
      // Boundaries right before the new parent move past it.
      if (sel && sel.rangeCount) {
        const bump = (n, o) => n === container && o === index ? [n, o + 1] : [n, o];
        const a = bump(sel.anchorNode, sel.anchorOffset), f = bump(sel.focusNode, sel.focusOffset);
        try { sel.setBaseAndExtent(a[0], a[1], f[0], f[1]); } catch (e) { /* detached */ }
      }
    }
    const original = list[0].parentNode;
    const firstVisible = list.find(visible), lastVisible = [...list].reverse().find(visible);
    const doc = parent.ownerDocument;
    if (parent.compareDocumentPosition(list[0]) & Node.DOCUMENT_POSITION_FOLLOWING) {
      const lastChild = [...parent.childNodes].reverse().find(visible);
      if (!inlineNode(parent) && lastChild && firstVisible && inlineNode(lastChild) && inlineNode(firstVisible) && !isElement(parent.lastChild, 'br'))
        parent.appendChild(doc.createElement('br'));
      for (const n of list) moveNode(n, parent, null, sel);
    } else {
      const firstChild = [...parent.childNodes].find(visible);
      if (!inlineNode(parent) && firstChild && lastVisible && inlineNode(firstChild) && inlineNode(lastVisible) && !isElement(list[list.length - 1], 'br'))
        parent.insertBefore(doc.createElement('br'), parent.firstChild);
      for (let i = list.length - 1; i >= 0; i--) moveNode(list[i], parent, parent.firstChild, sel);
    }
    if (original && editable(original) && !original.firstChild) original.remove();
    const sibling = parent.nextSibling;
    if (sibling && editable(sibling) && criteria(sibling)) {
      if (!inlineNode(parent) && inlineNode(parent.lastChild) && inlineNode(sibling.firstChild) && !isElement(parent.lastChild, 'br'))
        parent.appendChild(doc.createElement('br'));
      while (sibling.firstChild) moveNode(sibling.firstChild, parent, null, sel);
      sibling.remove();
    }
    removeExtraneousBreaks(parent);
    return parent;
  }
  function alignmentValue(node) {
    while (node && (node.nodeType !== 1 || ['inline', 'none'].includes(getComputedStyle(node).display))) node = node.parentNode;
    if (!node || node.nodeType !== 1) return 'left';
    const value = alignOf(node).replace(/^-webkit-/, '');
    return ['center', 'left', 'right', 'justify'].includes(value) ? value : 'left';
  }
  function setsAlignment(el) {
    return el.hasAttribute('align') || !!el.style?.textAlign || el.localName === 'center';
  }
  function justify(t, state, align) {
    const { sel, host } = t, doc = host.ownerDocument;
    let range = blockExtend(sel.getRangeAt(0));
    const elements = containedNodes(range).filter((n) => n.nodeType === 1 && editable(n) && setsAlignment(n));
    for (const el of elements) {
      el.removeAttribute('align');
      if (el.style.textAlign) { el.style.removeProperty('text-align'); if (!el.getAttribute('style')) el.removeAttribute('style'); }
      if (['div', 'span', 'center'].includes(el.localName) && !el.attributes.length) removePreserving(el, sel);
      else if (el.localName === 'center') {
        const div = doc.createElement('div');
        for (const a of Array.from(el.attributes)) div.setAttribute(a.name, a.value);
        el.parentNode.insertBefore(div, el);
        while (el.firstChild) moveNode(el.firstChild, div, null, sel);
        el.remove();
      }
    }
    range = blockExtend(sel.getRangeAt(0));
    const list = [];
    for (const n of containedNodes(range)) {
      const lastNode = list[list.length - 1];
      if (lastNode && lastNode.contains(n)) continue;
      if (inEditableContent(n.parentElement) && !(n.nodeType === 1 && notInDiv.has(n.localName)) && alignmentValue(n) !== align) list.push(n);
    }
    // Like browsers, new wrappers use the text-align property; existing
    // aligned divs (attribute or property) absorb adjacent content.
    const criteria = (el) => isElement(el, 'div') && el.attributes.length === 1 &&
      (el.getAttribute('align') === align || (el.hasAttribute('style') && el.style.length === 1 && el.style.textAlign === align));
    const create = () => {
      const div = doc.createElement('div');
      div.style.textAlign = align;
      return div;
    };
    while (list.length) {
      const sub = [list.shift()];
      while (list.length && list[0] === sub[sub.length - 1].nextSibling) sub.push(list.shift());
      wrapNodes(sub, criteria, create, sel);
    }
    const types = { left: 'formatJustifyLeft', right: 'formatJustifyRight', center: 'formatJustifyCenter', justify: 'formatJustifyFull' };
    input(host, types[align]);
    return true;
  }
  function justifyQuery(t, align, kind) {
    if (!t?.range) return kind === 'value' ? 'left' : false;
    const range = blockExtend(t.range);
    const leaves = containedNodes(range).filter((n) => !n.firstChild && editable(n) && visible(n));
    if (kind === 'value') {
      const node = leaves[0] || range.startContainer;
      return alignmentValue(node);
    }
    const values = leaves.map(alignmentValue);
    if (kind === 'mixed') return values.some((v) => v === align) && values.some((v) => v !== align);
    return values.length > 0 && values.every((v) => v === align);
  }

  const preferences = new Set(['stylewithcss', 'usecss', 'defaultparagraphseparator']);
  const textCommands = new Set(['inserttext', 'delete', 'forwarddelete']);
  const richCommands = new Set(['createlink', 'unlink', 'indent', 'outdent', 'removeformat', 'inserthtml', 'insertimage', 'inserthorizontalrule', 'insertlinebreak', 'insertparagraph']);
  function supported(cmd) {
    return blockCommand(cmd) || valueFormats.has(cmd) || formats.has(cmd) || preferences.has(cmd) || textCommands.has(cmd) || richCommands.has(cmd) || cmd === 'selectall';
  }
  function editingHost(node) {
    let el = node?.nodeType === 1 ? node : node?.parentElement;
    if (!el?.isContentEditable || el.closest('select,textarea,input')) return null;
    while (el.parentElement?.isContentEditable) el = el.parentElement;
    return el;
  }
  function target(doc) {
    const control = doc.activeElement;
    if (control && (control.localName === 'textarea' ||
        (control.localName === 'input' && ['text','search','url','tel','password'].includes(control.type)))) {
      if (control.matches(':disabled') || control.readOnly || !checkVisibility.call(control, { visibilityProperty: true })) return null;
      return { control };
    }
    const sel = doc.getSelection();
    if (!sel?.rangeCount) return null;
    const range = sel.getRangeAt(0);
    const host = editingHost(range.startContainer);
    if (!host || host !== editingHost(range.endContainer)) return null;
    return { sel, range, host };
  }
  function enabled(cmd, t) {
    if (preferences.has(cmd) || cmd === 'selectall') return true;
    if (!t) return false;
    if (textCommands.has(cmd)) return true;
    return !t.control && (richCommands.has(cmd) || formats.has(cmd) || valueFormats.has(cmd) || blockCommand(cmd)) &&
      (t.host.contentEditable !== 'plaintext-only' || cmd === 'insertlinebreak' || cmd === 'insertparagraph');
  }

  function input(host, inputType, data = null) {
    const state = contexts.get(host.ownerDocument);
    if (state?.userEdit) { state.changed = true; return; }
    host.dispatchEvent(new InputEvent('input', { bubbles: true, composed: true, inputType, data }));
  }
  const inputTypeFor = cmd => cmd === 'delete' ? 'deleteContentBackward'
    : cmd === 'forwarddelete' ? 'deleteContentForward' : 'insertText';
  function editControl(control, cmd, text) {
    let start = control.selectionStart, end = control.selectionEnd;
    const old = control.value;
    if (cmd !== 'inserttext') {
      text = '';
      if (start === end) {
        if (cmd === 'delete' && start) {
          const before = Array.from(old.slice(0, start));
          start -= before[before.length - 1].length;
        } else if (cmd === 'forwarddelete' && end < old.length) {
          end += String.fromCodePoint(old.codePointAt(end)).length;
        }
      }
    }
    if (start === end && !text) return;
    control.value = old.slice(0, start) + text + old.slice(end);
    control.setSelectionRange(start + text.length, start + text.length);
    input(control, inputTypeFor(cmd), cmd === 'inserttext' ? text : null);
  }
  function adjacentNode(node, host, forward) {
    while (node && node !== host) {
      const sibling = forward ? node.nextSibling : node.previousSibling;
      if (sibling) return sibling;
      node = node.parentNode;
    }
    return null;
  }
  function boundaryLeaf(range, host, forward, whitespace) {
    const point = forward ? range.endContainer : range.startContainer;
    const offset = forward ? range.endOffset : range.startOffset;
    let n;
    if (point.nodeType === 3)
      n = (forward ? offset < point.length : offset > 0) ? point : adjacentNode(point, host, forward);
    else n = point.childNodes[forward ? offset : offset - 1] || adjacentNode(point, host, forward);
    while (n) {
      if (n.nodeType === 1 && (!n.isContentEditable || n.localName === 'br')) return null;
      if (n.hasChildNodes()) {
        n = forward ? n.firstChild : n.lastChild;
        continue;
      }
      if (n.nodeType === 3 && n.length) {
        // Inter-block whitespace collapses in normal flow. A space inside
        // an inline run, or a preserved newline, is itself an editing unit.
        if (!/^[\t\r\n ]+$/.test(n.data) ||
            !isBlock(n.parentElement) ||
            !(isBlock(n.previousSibling) || isBlock(n.nextSibling)) ||
            !['normal', 'nowrap'].includes(getComputedStyle(n.parentElement).whiteSpace)) return n;
        whitespace.push(n);
      } else if (n.nodeType === 1) return null;
      n = adjacentNode(n, host, forward);
    }
    return null;
  }
  function containingBlock(node, host) {
    let block = node.parentElement;
    while (block !== host && !isBlock(block)) block = block.parentElement;
    return block;
  }
  function branchIn(node, parent) {
    while (node.parentNode !== parent) node = node.parentNode;
    return node;
  }
  // Delete the paragraph separator as a unit. Preserve inline markup and
  // the source line's whitespace semantics when moving it into its neighbor.
  // With probe, only tell whether this is a join of lines with different
  // white-space, which must keep the white-space of the moved line (others
  // run the specification algorithm).
  function joinParagraphs(t, cmd, probe = false) {
    const { range, host, sel } = t;
    const point = range.startContainer, offset = range.startOffset;
    if (range.collapsed && point.nodeType === 3 &&
        (cmd === 'delete' ? offset !== 0 : offset !== point.length)) return false;
    if (!range.collapsed && /[^\t\n\r ]/.test(range.toString())) return false;
    const whitespace = [];
    const left = boundaryLeaf(range, host, false, whitespace);
    const right = boundaryLeaf(range, host, true, whitespace);
    if (!left || !right || left === right || editingHost(left) !== host || editingHost(right) !== host) return false;
    const dest = containingBlock(left, host), src = containingBlock(right, host);
    if (src === dest || left.parentElement.closest('td,th,li') !== right.parentElement.closest('td,th,li')) return false;
    if (probe) {
      const destinationSpace = getComputedStyle(dest).whiteSpace;
      // Definition list items join like paragraphs in other browsers.
      const list = e => e.closest('dt,dd')?.parentElement;
      if (list(src) && list(dest) && list(src) !== list(dest)) return true;
      return [src, left.parentElement, right.parentElement].some(e => getComputedStyle(e).whiteSpace !== destinationSpace);
    }
    // A range may select text or an atomic element between the two leaves.
    const doc = host.ownerDocument, gap = doc.createRange();
    gap.setStart(left, left.length);
    gap.collapse(true);
    if (boundaryLeaf(gap, host, true, whitespace) !== right) return false;
    if (range.collapsed && point.nodeType === 3 && (cmd === 'delete' ? right : left) !== point) return false;
    if (!range.collapsed && ((point === left && offset !== left.length) ||
        (range.endContainer === right && range.endOffset !== 0))) return false;

    const first = branchIn(right, src);
    const move = doc.createRange();
    move.setStartBefore(first);
    move.setEnd(src, src.childNodes.length);
    let n = first, br = null, newline = null;
    while (n && src.contains(n)) {
      if (n.nodeType === 1 && (isBlock(n) || n.localName === 'br' || !n.isContentEditable)) {
        move.setEndBefore(n);
        if (n.localName === 'br') br = n;
        break;
      }
      if (n.nodeType === 3) {
        const index = n.data.indexOf('\n');
        if (index !== -1 && ['pre', 'pre-wrap', 'pre-line', 'break-spaces'].includes(getComputedStyle(n.parentElement).whiteSpace)) {
          // The retained Text node starts at the separator after extraction.
          if (index) move.setEnd(n, index);
          else move.setEndBefore(n);
          newline = n;
          break;
        }
      }
      n = n.firstChild || adjacentNode(n, src, true);
    }
    if (move.collapsed && !newline) return false;
    const whiteSpace = getComputedStyle(src).whiteSpace;
    const destinationSpace = getComputedStyle(dest).whiteSpace;
    // Extraction can clone a partial inline ancestor. Save the formatting
    // of the source line while its elements still match their old selectors.
    const styles = [];
    const after = move.endContainer === src ? src.childNodes[move.endOffset] :
        branchIn(move.endContainer,src).nextSibling;
    for (let part = first; part && part !== after; part = part.nextSibling) {
      styles.push(part.nodeType === 1 ? getComputedStyle(part).whiteSpace : whiteSpace);
    }
    const before = branchIn(left, dest).nextSibling;
    const fragment = move.extractContents();
    if (newline) {
      newline.deleteData(0, 1);
      if (!newline.length) br = newline;
    }
    if (br) {
      let el = br.parentElement;
      br.remove();
      while (el && el !== src && !el.hasChildNodes()) {
        const parent = el.parentElement;
        el.remove();
        el = parent;
      }
    }
    let wrapper = null;
    for (const [i,part] of Array.from(fragment.childNodes).entries()) {
      const value = styles[i] || whiteSpace;
      if (part.nodeType === 1 && value !== whiteSpace) {
        // Carry a contextual class's computed whitespace across the block
        // boundary even if the class stops matching in the destination.
        if (part.style.whiteSpace !== value) part.style.whiteSpace = value;
        dest.insertBefore(part,before);
        wrapper = null;
      } else if (value !== destinationSpace) {
        if (!wrapper) {
          wrapper = doc.createElement('span');
          wrapper.style.whiteSpace = value;
          dest.insertBefore(wrapper,before);
        }
        wrapper.appendChild(part);
      } else {
        dest.insertBefore(part,before);
        wrapper = null;
      }
    }
    for (const space of whitespace) if (space.isConnected) space.remove();
    for (let el = src; el !== host && !el.hasChildNodes();) {
      const parent = el.parentElement;
      el.remove();
      el = parent;
    }
    sel.collapse(left, left.length);
    return true;
  }
  // Keep typed spaces visible in collapsing white-space modes, while leaving
  // ordinary wrapping opportunities in the run. Inline markup does not end
  // a run. A line edge requires a nonbreaking space at that edge.
  function rebalanceSpaces(node, offset, host, deletion = false) {
    if (node.nodeType !== 3) return;
    const space = c => c === ' ' || c === '\u00a0';
    if (!space(node.data[offset-1]) && !space(node.data[offset])) return;
    const collapses = n => ['normal','nowrap','pre-line'].includes(getComputedStyle(n.parentElement).whiteSpace);
    if (!collapses(node)) return;
    const block = containingBlock(node,host);
    function neighbor(text,forward) {
      let next = adjacentNode(text,block,forward);
      while (next) {
        if (next.nodeType === 3 && next.length) return next;
        if (next.nodeType === 1) {
          if (next.localName === 'br' || isBlock(next)) return null;
          if (!next.isContentEditable || ['img','input','canvas','svg','video','iframe'].includes(next.localName) ||
              !['inline','contents','none'].includes(getComputedStyle(next).display)) return next;
          if (getComputedStyle(next).display !== 'none' && next.firstChild) {
            next = forward ? next.firstChild : next.lastChild;
            continue;
          }
        }
        next = adjacentNode(next,block,forward);
      }
      return null;
    }
    // Deleting at an inline boundary should leave the run on the far side
    // of the edit intact, including author-supplied nonbreaking spaces.
    if (deletion && offset === node.length) {
      const right = neighbor(node,true);
      if (right) {
        if (right.nodeType === 3 && right.data[0] === ' ' && collapses(right))
          rebalanceSpaces(right,0,host,true);
        return;
      }
    }
    const parts = [];
    function scan(forward) {
      let n = node, at = offset;
      for (;;) {
        const start = at, text = n.data;
        while (forward ? space(text[at]) : space(text[at-1])) at += forward ? 1 : -1;
        if (start !== at) parts.push([n,Math.min(at,start),Math.max(at,start)]);
        if (forward ? at < text.length : at > 0) {
          return text[forward ? at : at-1] === '\n' && getComputedStyle(n.parentElement).whiteSpace === 'pre-line';
        }
        n = neighbor(n,forward);
        if (!n) return true;
        if (n.nodeType !== 3) return false;
        if (deletion) {
          // Across an inline boundary the adjoining node owns its spaces.
          // Keep an existing nonbreaking space at the edge of an adjoining
          // run or a span with a different whitespace mode.
          const adjacent = forward ? n.data[0] : n.data[n.length-1];
          const edge = forward ? text[text.length-1] : text[0];
          return adjacent === ' ' || adjacent === '\n' ||
            (edge === '\u00a0' && (adjacent === '\u00a0' || !collapses(n)));
        }
        if (!collapses(n)) return false;
        at = forward ? 0 : n.length;
      }
    }
    const atStart = scan(false);
    parts.reverse();
    const atEnd = scan(true);
    const count = parts.reduce((total,part) => total + part[2]-part[1],0);
    let index = 0;
    for (const [text,from,to] of parts) {
      let replacement = '';
      for (let i=from;i<to;++i,++index) {
        const protectedSpace = count === 1 ? atStart || atEnd : index % 2 === 0 || (atEnd && index === count-1);
        replacement += protectedSpace ? '\u00a0' : ' ';
      }
      if (replacement !== text.data.slice(from,to)) text.replaceData(from,to-from,replacement);
    }
  }
  function deleteSelection(t, cmd) {
    if (joinParagraphs(t, cmd)) return true;
    const { sel, host } = t;
    if (sel.isCollapsed) {
      const node = sel.anchorNode, offset = sel.anchorOffset;
      sel.modify('extend', cmd === 'delete' ? 'backward' : 'forward', 'character');
      const cell = n => (n.nodeType === 1 ? n : n.parentElement)?.closest('td,th');
      if (sel.isCollapsed || editingHost(sel.focusNode) !== host || cell(node) !== cell(sel.focusNode)) { sel.collapse(node, offset); return false; }
    }
    const range = sel.getRangeAt(0);
    const touchedSpace = /[ \u00a0]/.test(range.toString());
    range.deleteContents();
    const caret = range.startContainer, offset = range.startOffset;
    if (touchedSpace) rebalanceSpaces(caret,offset,host,true);
    sel.collapse(caret,offset);
    return true;
  }

  // Editing spec "allowed child" (HTML content model rules for editing).
  const inlineContents = new Set(['a','abbr','b','bdi','bdo','cite','code','dfn','em','h1','h2','h3','h4','h5','h6','i','kbd','mark',
    'p','pre','q','rp','rt','ruby','s','samp','small','span','strong','sub','sup','u','var','acronym','listing','strike','xmp','big',
    'blink','font','marquee','nobr','tt']);
  const paragraphProhibited = new Set(['address','article','aside','blockquote','caption','center','col','colgroup','dd','details',
    'dir','div','dl','dt','fieldset','figcaption','figure','footer','form','h1','h2','h3','h4','h5','h6','header','hgroup','hr','li',
    'listing','menu','nav','ol','p','plaintext','pre','section','summary','table','tbody','td','tfoot','th','thead','tr','ul','xmp']);
  const headings = new Set(['h1','h2','h3','h4','h5','h6']);
  const onlyChildren = { colgroup: ['col'], table: ['caption','col','colgroup','tbody','td','tfoot','th','thead','tr'],
    tbody: ['td','th','tr'], tfoot: ['td','th','tr'], thead: ['td','th','tr'], tr: ['td','th'], dl: ['dt','dd'],
    dir: ['dir','li','ol','ul'], ol: ['dir','li','ol','ul'], ul: ['dir','li','ol','ul'], hgroup: [...headings] };
  const isHtml = (n) => n?.nodeType === 1 && n.namespaceURI === 'http://www.w3.org/1999/xhtml';
  function allowedChild(child, parent) {
    const pname = isHtml(parent) ? parent.localName : parent;
    if (['colgroup','table','tbody','tfoot','thead','tr','script','style','plaintext','xmp'].includes(pname) &&
        child.nodeType === 3 && !/^[ \t\n\f\r]*$/.test(child.data)) return false;
    if (['script','style','plaintext','xmp'].includes(pname) && child.nodeType !== 3) return false;
    if (child.nodeType === 9 || child.nodeType === 11 || child.nodeType === 10) return false;
    if (!isHtml(child)) return true;
    const name = child.localName;
    if (isHtml(parent)) {
      for (let a = parent; a?.nodeType === 1; a = a.parentNode) {
        if (name === 'a' && a.localName === 'a') return false;
        if (paragraphProhibited.has(name) && inlineContents.has(a.localName)) return false;
        if (headings.has(name) && headings.has(a.localName)) return false;
      }
    } else if (parent?.nodeType === 1 || parent?.nodeType === 11) return true;
    if (typeof pname !== 'string') return false;
    if (onlyChildren[pname]) return onlyChildren[pname].includes(name);
    if (['body','caption','col','colgroup','frame','frameset','head','html','tbody','td','tfoot','th','thead','tr'].includes(name)) return false;
    if ((name === 'dd' || name === 'dt') && pname !== 'dl') return false;
    if (name === 'li' && pname !== 'ol' && pname !== 'ul') return false;
    if (pname === 'a' && name === 'a') return false;
    if ((pname === 'dd' || pname === 'dt') && (name === 'dd' || name === 'dt')) return false;
    if (headings.has(pname) && headings.has(name)) return false;
    if (pname === name && (name === 'li' || name === 'nobr')) return false;
    if (inlineContents.has(pname) && paragraphProhibited.has(name)) return false;
    if ((pname === 'td' || pname === 'th') && ['caption','col','colgroup','tbody','td','tfoot','th','thead','tr'].includes(name)) return false;
    return true;
  }
  // "Fix disallowed ancestors" for an inserted block-level node: split its
  // inline ancestors until it is an allowed child of its parent.
  function fixDisallowedAncestors(node, sel) {
    if (!editable(node)) return;
    while (node.parentNode && editable(node.parentNode) && !allowedChild(node, node.parentNode)) {
      const parent = node.parentNode;
      splitParent([node], sel);
      if (node.parentNode === parent) break;
    }
  }
  // insertHorizontalRule (editing spec).
  function insertHorizontalRule(doc, t) {
    const { sel, host } = t;
    const r = sel.getRangeAt(0);
    let node = r.startContainer, offset = r.startOffset;
    while (offset === 0 && node !== host && node.parentNode) { offset = indexOf(node); node = node.parentNode; }
    while (offset === nodeLength(node) && node !== host && node.parentNode) { offset = indexOf(node) + 1; node = node.parentNode; }
    sel.collapse(node, offset);
    const hr = doc.createElement('hr');
    sel.getRangeAt(0).insertNode(hr);
    fixDisallowedAncestors(hr, sel);
    sel.collapse(hr.parentNode, indexOf(hr) + 1);
  }
  function insertAt(range, node, sel) {
    const last = node.nodeType === 11 ? node.lastChild : node;
    if (!last) return;
    range.insertNode(node);
    range.setStartAfter(last);
    sel.collapse(range.startContainer, range.startOffset);
  }
  function insertText(doc, range, sel, text) {
    if (range.startContainer.nodeType === 3) {
      const node = range.startContainer, offset = range.startOffset;
      node.insertData(offset, text);
      sel.collapse(node, offset + text.length);
    } else {
      const node = doc.createTextNode(text);
      range.insertNode(node);
      sel.collapse(node, text.length);
    }
  }
  const caretBreaks = new WeakSet();
  function caretBreak(doc) {
    const br = doc.createElement('br');
    caretBreaks.add(br);
    return br;
  }
  function followingContent(range, container) {
    const point = range.startContainer, offset = range.startOffset;
    if (point.nodeType === 3 && offset < point.length) return point;
    let node = point.nodeType === 1 ? point.childNodes[offset] : null;
    node ||= adjacentNode(point, container, true);
    while (node) {
      if (node.nodeType === 3 && node.length || node.nodeType === 1 &&
          (node.localName === 'br' || isBlock(node) || !node.isContentEditable ||
           ['img','input','iframe','canvas','video','audio','hr'].includes(node.localName))) return node;
      node = node.firstChild || adjacentNode(node, container, true);
    }
    return null;
  }
  function insertLineBreak(doc, t) {
    const {host, range, sel} = t;
    const point = range.startContainer;
    const parent = point.nodeType === 3 ? point.parentElement : point;
    const block = parent === host || isBlock(parent) ? parent : containingBlock(parent, host);
    const atEnd = !followingContent(range, block);
    if (['pre','pre-line','pre-wrap','break-spaces'].includes(getComputedStyle(parent).whiteSpace)) {
      insertText(doc, range, sel, '\n');
    } else {
      insertAt(range, doc.createElement('br'), sel);
    }
    if (atEnd) {
      const caret = sel.getRangeAt(0).cloneRange();
      const node = caret.startContainer, offset = caret.startOffset;
      caret.insertNode(caretBreak(doc));
      sel.collapse(node, offset);
    }
  }
  function hasParagraphContent(block) {
    return !!block.textContent || !!block.querySelector('br,img,input,iframe,canvas,video,audio,hr,[contenteditable=false]');
  }
  function firstCaretContainer(block) {
    let node = block;
    while (node.firstChild && node.firstChild.nodeType === 1 && !isBlock(node.firstChild) &&
           node.firstChild.localName !== 'br' && node.firstChild.isContentEditable &&
           !['img','input','iframe','canvas','video','audio'].includes(node.firstChild.localName)) node = node.firstChild;
    return node;
  }
  function splitParagraph(doc, t, separator) {
    const { host, range, sel } = t;
    let block = range.startContainer.nodeType === 1 ? range.startContainer : range.startContainer.parentElement;
    while (block !== host && (!isBlock(block) || !['address','div','p','pre','li','blockquote','h1','h2','h3','h4','h5','h6'].includes(block.localName)))
      block = block.parentElement;
    if (!isBlock(block) || block.localName === 'pre' || host.contentEditable === 'plaintext-only') {
      insertLineBreak(doc, t);
      return;
    }
    const tail = doc.createRange();
    tail.selectNodeContents(block);
    tail.setStart(range.startContainer, range.startOffset);
    const after = tail.extractContents();
    const next = block === host ? doc.createElement(separator) : block.cloneNode(false);
    next.removeAttribute('id');
    next.appendChild(after);
    if (block === host) {
      if (!hasParagraphContent(block)) {
        const before = doc.createElement(separator);
        while (block.firstChild) before.appendChild(block.firstChild);
        block.appendChild(before);
        firstCaretContainer(before).appendChild(caretBreak(doc));
      }
      block.appendChild(next);
    } else {
      block.parentNode.insertBefore(next, block.nextSibling);
      if (!hasParagraphContent(block)) firstCaretContainer(block).appendChild(caretBreak(doc));
    }
    const caret = firstCaretContainer(next);
    if (!hasParagraphContent(next)) caret.appendChild(caretBreak(doc));
    if (caret.firstChild?.nodeType === 3) sel.collapse(caret.firstChild, 0);
    else sel.collapse(caret, 0);
  }
  function execute(doc, state, cmd, value) {
    if (cmd === 'stylewithcss' || cmd === 'usecss') {
      state.css = (value.toLowerCase() !== 'false') === (cmd === 'stylewithcss');
      return true;
    }
    if (cmd === 'defaultparagraphseparator') {
      const tag = value.toLowerCase();
      if (tag !== 'p' && tag !== 'div') return false;
      state.separator = tag;
      return true;
    }
    const t = target(doc);
    if (!enabled(cmd, t)) return false;
    // A link being typed (createLink at the caret) splits the surrounding
    // link in the implementation above.
    const typingLink = cmd === 'inserttext' && t && !t.control && typingStyle(state, t.range)?.has('createlink');
    if (specCommands.has(cmd) && !typingLink && specTarget(t, cmd, value)) {
      if ((cmd === 'delete' || cmd === 'forwarddelete') && joinParagraphs(t, cmd, true)) {
        state.typing = null;
        if (deleteSelection(t, cmd)) input(t.host, inputTypeFor(cmd));
        return true;
      }
      return specExecute(doc, state, t, cmd, value);
    }
    if (cmd === 'selectall') {
      if (t?.control) t.control.select();
      else if (doc.body) doc.getSelection()?.selectAllChildren(t?.host || doc.body);
      return true;
    }
    if (cmd === 'createlink' || cmd === 'unlink') return editLink(t,state,cmd === 'unlink' ? null : value);
    if (cmd === 'removeformat') return removeFormatting(state,t);
    if (listCommands.has(cmd)) return applyList(t,listCommands.get(cmd),state.separator);
    if (cmd === 'indent' || cmd === 'outdent') return applyIndent(t,cmd === 'indent',state.separator);
    if (alignments.has(cmd)) return justify(t, state, alignments.get(cmd));
    if (blockCommand(cmd)) return applyBlock(t,cmd,value);
    if (valueFormats.has(cmd)) {
      const normalized = normalizeFormat(cmd, value);
      if (normalized === null) return false;
      applyFormat(state, t, cmd, normalized);
      return true;
    }
    if (formats.has(cmd)) { applyFormat(state, t, cmd); return true; }
    if (t.control) { editControl(t.control, cmd, value); return true; }
    if (cmd === 'delete' || cmd === 'forwarddelete') {
      if (deleteSelection(t, cmd)) input(t.host, inputTypeFor(cmd));
      return true;
    }
    if (cmd === 'insertimage' && !value) return false;
    const { sel, range, host } = t;
    if (cmd === 'inserttext' && !value) {
      if (!range.collapsed && deleteSelection(t,'delete')) input(host,'insertText','');
      return true;
    }
    if (range.collapsed && !typingStyle(state, range) && host.contentEditable !== 'plaintext-only' &&
        ['inserttext','insertlinebreak','insertparagraph'].includes(cmd)) {
      const typing = overridesToTyping(spec.run(doc, state, () => spec.overrides()));
      if (typing.size) state.typing = { node: range.startContainer, offset: range.startOffset, formats: typing };
    }
    const toggles = range.collapsed && ['inserttext','insertlinebreak','insertparagraph'].includes(cmd) ? typingStyle(state, range) : null;
    state.typing = null;
    range.deleteContents();
    if (cmd === 'insertparagraph' || cmd === 'insertlinebreak') {
      if (cmd === 'insertparagraph') splitParagraph(doc, t, state.separator);
      else insertLineBreak(doc, t);
      if (toggles) state.typing = {node:sel.focusNode,offset:sel.focusOffset,formats:toggles};
      input(host, cmd === 'insertparagraph' ? 'insertParagraph' : 'insertLineBreak');
    } else if (cmd === 'inserttext') {
      const lines = value.replace(/\r\n?/g, '\n').split('\n');
      for (let i = 0; i < lines.length; ++i) {
        if (i) splitParagraph(doc, {host, sel, range:sel.getRangeAt(0)}, state.separator);
        if (!lines[i]) continue;
        const caret = sel.getRangeAt(0);
        const point = caret.startContainer, offset = caret.startOffset;
        const following = point.nodeType === 3 ? (offset === point.length ? point.nextSibling : null) : point.childNodes[offset];
        if (following && (caretBreaks.has(following) ||
            following.localName === 'br' && following.parentNode === host && host.childNodes.length === 1))
          following.remove();
        if (toggles?.size) {
          const node = doc.createTextNode(lines[i]);
          sel.getRangeAt(0).insertNode(node);
          for (const [name, enabled] of toggles) {
            if (name === 'createlink') { putLink(node,enabled,host); continue; }
            if (valueFormats.has(name)) { wrapValue(node, name, enabled, state.css); continue; }
            if (!enabled || isScriptFormat(name)) clearFormat(node, host, name);
            if (activeFormat(node.parentElement, name) !== enabled) wrapFormat(node, name, enabled, state.css);
          }
          sel.collapse(node, node.length);
          state.typing = { node, offset: node.length, formats: toggles };
        } else insertText(doc, sel.getRangeAt(0), sel, lines[i]);
        const inserted = sel.focusNode, end = sel.focusOffset;
        if (inserted.nodeType === 3) {
          const start = end - lines[i].length;
          if (start > 0 && inserted.data[start-1] === '\u00a0' && !/^[ \u00a0]/.test(lines[i]))
            rebalanceSpaces(inserted,start,host);
          for (const match of lines[i].matchAll(/ +/g)) rebalanceSpaces(inserted,start + match.index,host);
          sel.collapse(inserted,end);
        }
      }
      input(host, 'insertText', value);
    } else {
      if (cmd === 'inserthorizontalrule') {
        insertHorizontalRule(doc, t);
        input(host, 'insertHorizontalRule');
        return true;
      }
      let node;
      if (cmd === 'inserthtml') node = range.createContextualFragment(value);
      else {
        node = doc.createElement(cmd === 'insertimage' ? 'img' : 'hr');
        if (cmd === 'insertimage') node.setAttribute('src', value);
      }
      insertAt(range, node, sel);
      input(host, cmd === 'inserthorizontalrule' ? 'insertHorizontalRule' : '');
    }
    return true;
  }

  // In rich editing hosts the commands run the specification algorithms
  // (editing_spec.js); text controls and plaintext-only hosts use the
  // implementation above. Either way an edit that changed the document fires
  // input at the editing host.
  const Observer = MutationObserver;
  const specInputTypes = new Map([
    ['bold', 'formatBold'], ['italic', 'formatItalic'], ['underline', 'formatUnderline'],
    ['strikethrough', 'formatStrikeThrough'], ['subscript', 'formatSubscript'], ['superscript', 'formatSuperscript'],
    ['forecolor', 'formatFontColor'], ['backcolor', 'formatBackColor'], ['hilitecolor', 'formatBackColor'],
    ['fontname', 'formatFontName'], ['fontsize', ''], ['createlink', 'insertLink'], ['unlink', ''],
    ['removeformat', 'formatRemove'], ['insertorderedlist', 'insertOrderedList'],
    ['insertunorderedlist', 'insertUnorderedList'], ['indent', 'formatIndent'], ['outdent', 'formatOutdent'],
    ['justifyleft', 'formatJustifyLeft'], ['justifyright', 'formatJustifyRight'],
    ['justifycenter', 'formatJustifyCenter'], ['justifyfull', 'formatJustifyFull'], ['formatblock', ''],
    ['delete', 'deleteContentBackward'], ['forwarddelete', 'deleteContentForward'], ['inserttext', 'insertText'],
    ['insertparagraph', 'insertParagraph'], ['insertlinebreak', 'insertLineBreak'],
    ['inserthorizontalrule', 'insertHorizontalRule'], ['inserthtml', ''], ['insertimage', ''],
  ]);
  const specCommands = specInputTypes;
  function specTarget(t, cmd, value) {
    if (!t || t.control) return false;
    // Deletion in plaintext-only hosts works as in rich ones; nothing else
    // there may create markup.
    if (t.host.contentEditable === 'plaintext-only') return cmd === 'delete' || cmd === 'forwarddelete';
    // The specification assumes block editing hosts: an inline host gets
    // line breaks instead of paragraphs, as in other browsers.
    if (getComputedStyle(t.host).display.startsWith('inline')) return false;
    // Removing formatting keeps links and the selection, and a link typed
    // inside a link splits it; lists inside table cells and sublists keep
    // their boundaries: the implementation above does these as browsers do.
    if (cmd === 'removeformat' || (cmd === 'createlink' && t.range.collapsed)) return false;
    const el = n => n.nodeType === 1 ? n : n.parentElement;
    if (listCommands.has(cmd)) {
      const inCell = n => el(n)?.closest('td,th,caption');
      const nested = n => el(n)?.closest('li')?.parentElement?.closest('li') || el(n)?.closest('li')?.querySelector('ol,ul');
      const inTable = el(t.range.commonAncestorContainer)?.closest('table,thead,tbody,tfoot,tr');
      if (inCell(t.range.startContainer) || inCell(t.range.endContainer) || (inTable && t.host.contains(inTable)) ||
          nested(t.range.startContainer)) return false;
    }
    // Formatting the block of an xmp or listing element nests it in the new
    // block, and text directly in a list is indented with its item, as in
    // other browsers.
    if (cmd === 'formatblock' && value && (el(t.range.startContainer)?.closest('xmp,listing') || el(t.range.endContainer)?.closest('xmp,listing'))) return false;
    if (cmd === 'indent' && t.range.startContainer.nodeType === 3 && /^(ol|ul)$/.test(t.range.startContainer.parentElement?.localName)) return false;
    // Spaces typed before text that preserves white-space end in a
    // nonbreaking space, as the implementation above does.
    if (cmd === 'inserttext' && /^[ \u00a0]+$/.test(value) && t.range.collapsed) {
      const n = t.range.startContainer, o = t.range.startOffset;
      const next = n.nodeType === 3 ? (o === n.length ? n.nextSibling : null) : n.childNodes[o];
      if (next?.nodeType === 1 && getComputedStyle(next).whiteSpace !== 'normal' && getComputedStyle(next).whiteSpace !== 'nowrap') return false;
    }
    // A deletion from a line into a child block keeps the block's first line
    // in it (as in other browsers), which the implementation above does.
    if ((cmd === 'delete' || cmd === 'forwarddelete' || cmd === 'inserttext') && endsInChildBlock(t)) return false;
    // Where white-space is not normal, line breaks follow it (newlines where
    // it preserves them) as in other browsers.
    if (cmd === 'insertparagraph' || cmd === 'insertlinebreak' || (cmd === 'inserttext' && /[\r\n]/.test(value))) {
      const node = t.range.startContainer, el = node.nodeType === 1 ? node : node.parentElement;
      if (el && getComputedStyle(el).whiteSpace !== 'normal') return false;
    }
    return true;
  }
  // The typing style moves between the two implementations when the caret
  // stays put: ours keeps a map of formats, the spec algorithms overrides.
  function typingToOverrides(formats) {
    const list = [];
    for (const [name, value] of formats) {
      if (typeof value === 'boolean') list.push([name, value]);
      else if (name === 'fontsize') { if (sizes[Number(value) - 1]) list.push([name, sizes[Number(value) - 1]]); }
      else if (typeof value === 'string') list.push([name === 'backcolor' ? 'hilitecolor' : name, value]);
    }
    return list;
  }
  function overridesToTyping(list) {
    const formats = new Map();
    for (const [name, value] of list) {
      if (typeof value === 'boolean') { if (formats.has(name) || formatsKnown(name)) formats.set(name, value); }
      else if (name === 'fontsize') { const i = sizes.indexOf(value); if (i >= 0) formats.set(name, String(i + 1)); }
      else formats.set(name === 'hilitecolor' ? 'backcolor' : name, value);
    }
    return formats;
  }
  const formatsKnown = name => formats.has(name);
  // Like other browsers, forward deletion at the end of the editable content
  // (before at most the line break that ends it) changes nothing, also at
  // the end of a table cell or caption.
  function atEndOfEditableContent({ range, host }) {
    if (!range.collapsed) return false;
    const node = range.startContainer;
    const limit = (node.nodeType === 1 ? node : node.parentElement).closest('td,th,caption') || host;
    if (!host.contains(limit)) return false;
    const rest = host.ownerDocument.createRange();
    rest.setStart(node, range.startOffset);
    rest.setEnd(limit, limit.childNodes.length);
    const after = rest.cloneContents();
    if (after.textContent !== '') return false;
    // Only the (now empty) clones of the caret's ancestors and one br.
    let ancestors = 0;
    for (let n = node; n && n !== limit; n = n.parentNode) if (n.nodeType === 1) ancestors++;
    let breaks = 0, others = 0;
    for (const e of after.querySelectorAll('*')) {
      if (e.localName === 'br') breaks++;
      else others++;
    }
    return breaks <= 1 && others <= ancestors;
  }
  function endsInChildBlock({ range, host }) {
    if (range.collapsed) return false;
    const blockOf = node => {
      let el = node.nodeType === 1 ? node : node.parentElement;
      while (el && el !== host && !isBlock(el)) el = el.parentElement;
      return el;
    };
    const start = blockOf(range.startContainer), end = blockOf(range.endContainer);
    if (!start || !end || start === end || !start.contains(end)) return false;
    // Only when the range starts at the start of a line and selects whole
    // lines before the child block; otherwise the lines are joined.
    const doc = host.ownerDocument;
    let top = end;
    while (top.parentElement !== start) top = top.parentElement;
    const before = doc.createRange();
    before.setStart(start, 0);
    before.setEnd(range.startContainer, range.startOffset);
    let content = false;
    const newlines = ['pre', 'pre-wrap', 'pre-line', 'break-spaces'].includes(getComputedStyle(start).whiteSpace);
    const scan = (node) => {
      for (let c = node.firstChild; c; c = c.nextSibling) {
        if (c.nodeType === 3) {
          const tail = c.data.slice(c.data.lastIndexOf('\n') + 1);
          if (c.data.includes('\n') && newlines) content = /[^\t\r ]/.test(tail);
          else if (/[^\t\n\r ]/.test(c.data)) content = true;
        } else if (c.nodeType === 1) {
          if (c.localName === 'br') content = false;
          else if (c.localName === 'img') content = true;
          else scan(c);
        }
      }
    };
    scan(before.cloneContents());
    if (content) return false;
    const lines = doc.createRange();
    lines.setStart(range.startContainer, range.startOffset);
    lines.setEndBefore(top);
    const selected = lines.cloneContents();
    return /[^\t\r ]/.test(lines.toString()) || !!selected.querySelector('br');
  }
  function specInputData(cmd, value) {
    if (cmd === 'inserttext' || cmd === 'createlink') return value;
    if (valueFormats.has(cmd)) return normalizeFormat(cmd, value);
    return null;
  }
  // Like other browsers, a list command ignores the block where a selection
  // ends at its start.
  function excludeEndBlock(t) {
    const { range, host, sel } = t;
    if (range.collapsed) return;
    let block = range.endContainer.nodeType === 1 ? range.endContainer : range.endContainer.parentElement;
    while (block && block !== host && !isBlock(block)) block = block.parentElement;
    if (!block || block === host || !host.contains(block) || block.contains(range.startContainer)) return;
    const before = host.ownerDocument.createRange();
    before.setStart(block, 0);
    before.setEnd(range.endContainer, range.endOffset);
    if (before.toString() !== '' || before.cloneContents().querySelector('img,br,hr,input,iframe,video,canvas,svg')) return;
    const backward = sel.direction === 'backward';
    const start = [range.startContainer, range.startOffset];
    const end = [block.parentNode, Array.prototype.indexOf.call(block.parentNode.childNodes, block)];
    if (backward) sel.setBaseAndExtent(end[0], end[1], start[0], start[1]);
    else sel.setBaseAndExtent(start[0], start[1], end[0], end[1]);
    t.range = sel.getRangeAt(0);
  }
  function specExecute(doc, state, t, cmd, value) {
    if (listCommands.has(cmd)) excludeEndBlock(t);
    const name = cmd === 'backcolor' && state.name === 'hilitecolor' ? 'hilitecolor' : cmd;
    const observer = new Observer(() => {});
    observer.observe(doc, { childList: true, subtree: true, characterData: true, attributes: true });
    let result, changed = false;
    const typing = typingStyle(state, t.range);
    state.typing = null;
    if (cmd === 'forwarddelete' && atEndOfEditableContent(t)) return true;
    const backward = t.sel.direction === 'backward';
    if (typing?.size) spec.run(doc, state, () => spec.setOverrides(typingToOverrides(typing)));
    // A deletion from inside a link to outside it leaves the caret outside.
    const linkOf = n => (n.nodeType === 1 ? n : n.parentElement)?.closest('a[href]');
    const links = t.range.collapsed ? null : [linkOf(t.range.startContainer), linkOf(t.range.endContainer)];
    try {
      result = spec.run(doc, state, () => {
        if (cmd === 'inserttext' && value) {
          // Typing over a selection that leaves a link types outside it.
          if (links && links[0] !== links[1]) {
            spec.execCommand('delete', false, '');
            placeCaret(doc, 'delete', links);
          }
          removeCaretBreak(doc);
        }
        return spec.execCommand(name, false, value);
      }, name);
    } finally {
      changed = observer.takeRecords().length > 0;
      observer.disconnect();
    }
    if (result) spec.run(doc, state, () => placeCaret(doc, cmd, links));
    // The algorithms reset the selection while moving nodes; like other
    // browsers, keep its direction.
    const sel = doc.getSelection();
    if (backward && sel?.rangeCount && !sel.isCollapsed && sel.direction !== 'backward') {
      const r = sel.getRangeAt(0);
      sel.setBaseAndExtent(r.endContainer, r.endOffset, r.startContainer, r.startOffset);
    }
    if (result && changed) input(t.host, specInputTypes.get(cmd), specInputData(cmd, value));
    return result;
  }
  // Like other browsers, a caret left between elements by a deletion goes
  // to the end of the inline content before it, so typing and queries use
  // that formatting; one left in a list goes into the list item.
  const inlineContainers = new Set(['abbr','b','bdi','bdo','cite','code','del','dfn','em','font','i','ins','kbd','mark','q','s','samp','small','span','strike','strong','sub','sup','u','var']);
  // Typing replaces the line break that only held the caret on an empty line
  // created by insertParagraph or insertLineBreak above.
  function removeCaretBreak(doc) {
    const sel = doc.getSelection();
    if (!sel?.rangeCount || !sel.isCollapsed) return;
    const point = sel.focusNode, offset = sel.focusOffset;
    const following = point.nodeType === 3 ? (offset === point.length ? point.nextSibling : null) : point.childNodes[offset];
    if (following && caretBreaks.has(following)) following.remove();
  }
  function nextTextIn(root, text) {
    const walker = root.ownerDocument.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    walker.currentNode = text;
    for (let n = walker.nextNode(); n; n = walker.nextNode()) if (n.length) return n;
    return null;
  }
  function placeCaret(doc, cmd, links) {
    const sel = doc.getSelection();
    if (!sel?.rangeCount || !sel.isCollapsed) return;
    const node = sel.focusNode, offset = sel.focusOffset;
    if (links && links[0] !== links[1] && (cmd === 'delete' || cmd === 'forwarddelete')) {
      const link = (node.nodeType === 1 ? node : node.parentElement).closest('a[href]');
      if (link && link.isContentEditable) {
        const atEnd = link.textContent !== '' && ((node === link && offset === link.childNodes.length) ||
          (node.nodeType === 3 && offset === node.length && !nextTextIn(link, node)));
        if (atEnd) {
          spec.moveCaret(link.parentNode, Array.prototype.indexOf.call(link.parentNode.childNodes, link) + 1);
          // Nor does the link's own styling carry over.
          const style = getComputedStyle(link);
          for (const [name, value] of spec.overrides()) {
            if (name === 'createlink' || (name === 'forecolor' && N.editingColor(value) === style.color) ||
                (name === 'underline' && value === true && style.textDecorationLine.includes('underline')))
              spec.unsetOverrides(name);
          }
        }
      }
      return;
    }
    if (node.nodeType !== 1) return;
    if (listCommands.has(cmd) && (node.localName === 'ol' || node.localName === 'ul')) {
      const item = node.childNodes[offset] || node.childNodes[offset - 1];
      if (item?.localName === 'li') spec.moveCaret(item, item === node.childNodes[offset] ? 0 : item.childNodes.length);
      return;
    }
    // A new paragraph starting with inline formatting elements gets the
    // caret inside them, so that typing uses that formatting.
    if (cmd === 'insertparagraph' && offset === 0) {
      let first = node.firstChild;
      while (first?.nodeType === 1 && inlineContainers.has(first.localName) && first.isContentEditable && first.firstChild) first = first.firstChild;
      if (first !== node.firstChild && first?.nodeType === 3 && first.length) spec.moveCaret(first, 0);
      return;
    }
    if (cmd !== 'delete' && cmd !== 'forwarddelete') return;
    let before = node.childNodes[offset - 1];
    while (before?.nodeType === 1 && inlineContainers.has(before.localName) && before.isContentEditable && before.lastChild) before = before.lastChild;
    const after = node.childNodes[offset];
    if (before?.nodeType === 3 && before.length) spec.moveCaret(before, before.length);
    else if (!before && after?.nodeType === 3 && after.length) spec.moveCaret(after, 0);
  }
  function specQuery(doc, state, method, name) {
    const t = target(doc);
    if (!specTarget(t, name, '') || !spec.queryCommandSupported(name)) return undefined;
    return spec.run(doc, state, () => spec[method](name));
  }

  function userEdit(host, cmd, value) {
    const doc = host.ownerDocument, state = context(doc), t = target(doc);
    if (state.executing || !t || t.host !== host) return false;
    state.userEdit = state.executing = true;
    state.changed = false;
    try {
      execute(doc, state, cmd, value);
      return state.changed;
    } finally { state.userEdit = state.executing = false; }
  }


  function command(value, count) {
    if (!count) throw new TypeError('A command is required');
    const cmd = `${value}`.toLowerCase();
    return cmd === 'hilitecolor' ? 'backcolor' : cmd;
  }
  const methods = {
    execCommand(name, showUI = false, value = '') {
      const state = context(this), cmd = command(name, arguments.length);
      const text = `${value}`;
      state.name = `${name}`.toLowerCase();
      if (!supported(cmd) || state.executing) return false;
      state.executing = true;
      try { return execute(this, state, cmd, text); } finally { state.executing = false; }
    },
    queryCommandSupported(name) { context(this); return supported(command(name, arguments.length)); },
    queryCommandEnabled(name) {
      context(this);
      const cmd = command(name, arguments.length);
      return supported(cmd) && enabled(cmd, target(this));
    },
    queryCommandState(name) {
      const state = context(this);
      const cmd = command(name, arguments.length);
      const specResult = specQuery(this, state, 'queryCommandState', `${name}`.toLowerCase());
      if (specResult !== undefined) return specResult;
      if (alignments.has(cmd)) return justifyQuery(target(this), alignments.get(cmd), 'state');
      if (blockCommand(cmd)) return blockQuery(target(this),cmd,'state');
      return formats.has(cmd) ? formatState(state, target(this), cmd, false) : cmd === 'stylewithcss' && state.css;
    },
    queryCommandIndeterm(name) {
      const state = context(this), cmd = command(name, arguments.length);
      const specResult = specQuery(this, state, 'queryCommandIndeterm', `${name}`.toLowerCase());
      if (specResult !== undefined) return specResult;
      if (alignments.has(cmd)) return justifyQuery(target(this), alignments.get(cmd), 'mixed');
      if (blockCommand(cmd)) return blockQuery(target(this),cmd,'mixed');
      return valueFormats.has(cmd) ? selectedValue(state, target(this), cmd, true) : formats.has(cmd) && formatState(state, target(this), cmd, true);
    },
    queryCommandValue(name) {
      const state = context(this);
      const cmd = command(name, arguments.length);
      const specResult = specQuery(this, state, 'queryCommandValue', `${name}`.toLowerCase());
      if (specResult !== undefined) return specResult;
      if (alignments.has(cmd)) return justifyQuery(target(this), alignments.get(cmd), 'value');
      if (blockCommand(cmd)) return blockQuery(target(this),cmd,'value');
      return valueFormats.has(cmd) ? selectedValue(state, target(this), cmd, false) : cmd === 'defaultparagraphseparator' ? state.separator : '';
    },
  };
  return { userEdit, methods };
})
