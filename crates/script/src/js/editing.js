// HTML editing commands operate on the active selection in a single editing
// host. Each document keeps its own editing preferences and reentrancy flag.
(function () {
  'use strict';
  const N = globalThis.__rhNative;
  if (N.workerInfo()) return;

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
      const m = /^([+-]?)([0-9]+)/.exec(value.trimStart());
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
    const runs = selectedText(t);
    const first = formatValue(runs[0]?.node.parentElement || selectionElement(range), cmd);
    return mixed ? runs.some(r => formatValue(r.node.parentElement, cmd) !== first) : first;
  }

  function activeFormat(el, cmd) {
    if (!el) return false;
    const [, prop, on] = formats.get(cmd);
    const style = getComputedStyle(el).getPropertyValue(prop);
    if (cmd === 'bold') return style === 'bold' || Number(style) >= 600;
    if (style.split(' ').includes(on)) return true;
    if (['subscript','superscript','underline','strikethrough'].includes(cmd)) {
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
    const runs = selectedText(t);
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
      const semantic = el.localName === tag || (cmd === 'bold' && el.localName === 'strong') ||
        (cmd === 'italic' && el.localName === 'em') || (cmd === 'strikethrough' && el.localName === 'strike');
      const inline = el.style.getPropertyValue(prop) ||
        (prop === 'text-decoration-line' && el.style.getPropertyValue('text-decoration'));
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
  function wrapFormat(node, cmd, enable, css) {
    const [tag, prop, on, off] = formats.get(cmd);
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
      toggles.set(cmd, enable);
      state.typing = { node: range.startContainer, offset: range.startOffset, formats: toggles };
      return;
    }
    const runs = selectedText(t), backward = sel.direction === 'backward';
    for (const r of runs) {
      if ((valued ? formatValue(r.node.parentElement, cmd) : activeFormat(r.node.parentElement, cmd)) === enable) continue;
      if (r.to < r.node.length) r.node.splitText(r.to);
      if (r.from) r.node = r.node.splitText(r.from);
      r.to -= r.from;
      r.from = 0;
      if (valued || !enable) clearFormat(r.node, host, cmd);
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

  const alignments = new Map([
    ['justifyleft','left'], ['justifyright','right'], ['justifycenter','center'], ['justifyfull','justify']
  ]);
  const blockTags = new Set(['address','dd','div','dt','h1','h2','h3','h4','h5','h6','p','pre']);
  const blockCommand = cmd => alignments.has(cmd) || cmd === 'formatblock';
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
  function paragraph(host, node) {
    let top = node;
    while (top !== host && top.parentNode !== host && !isBlock(top)) top = top.parentNode;
    if (top !== host && isBlock(top) && !['li','td','th'].includes(top.localName))
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
  function paragraphs(t) {
    let nodes = t.range.collapsed ? [] : selectedText(t).map(run=>run.node);
    if (!nodes.length) {
      const point=t.range.startContainer;
      nodes=[point.nodeType===3 ? point : point.childNodes[t.range.startOffset] || point.lastChild || point];
    }
    const seen=new Set(), result=[];
    for (const node of nodes) {
      if (editingHost(node)!==t.host) continue;
      const run=paragraph(t.host,node), key=run.nodes[0] || run.container;
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

  const preferences = new Set(['stylewithcss', 'usecss', 'defaultparagraphseparator']);
  const textCommands = new Set(['inserttext', 'delete', 'forwarddelete']);
  const richCommands = new Set(['inserthtml', 'insertimage', 'inserthorizontalrule', 'insertlinebreak', 'insertparagraph']);
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
      return control.disabled || control.readOnly ? null : { control };
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
  function deleteSelection(t, cmd) {
    const { sel, host } = t;
    if (sel.isCollapsed) {
      const node = sel.anchorNode, offset = sel.anchorOffset;
      sel.modify('extend', cmd === 'delete' ? 'backward' : 'forward', 'character');
      const cell = n => (n.nodeType === 1 ? n : n.parentElement)?.closest('td,th');
      if (sel.isCollapsed || editingHost(sel.focusNode) !== host || cell(node) !== cell(sel.focusNode)) { sel.collapse(node, offset); return false; }
    }
    const range = sel.getRangeAt(0);
    range.deleteContents();
    sel.collapse(range.startContainer, range.startOffset);
    return true;
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
  function splitParagraph(doc, t, separator) {
    const { host, range, sel } = t;
    let block = range.startContainer.nodeType === 1 ? range.startContainer : range.startContainer.parentElement;
    while (block !== host && !['address','div','p','pre','li','blockquote','h1','h2','h3','h4','h5','h6'].includes(block.localName))
      block = block.parentElement;
    const tail = doc.createRange();
    tail.selectNodeContents(block);
    tail.setStart(range.startContainer, range.startOffset);
    const after = tail.extractContents();
    const next = doc.createElement(block === host ? separator : block.localName);
    next.appendChild(after);
    if (!next.firstChild) next.appendChild(doc.createElement('br'));
    if (block === host) block.appendChild(next);
    else {
      block.parentNode.insertBefore(next, block.nextSibling);
      if (!block.firstChild) block.appendChild(doc.createElement('br'));
    }
    sel.collapse(next, 0);
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
    if (cmd === 'selectall') {
      if (t?.control) t.control.select();
      else if (doc.body) doc.getSelection()?.selectAllChildren(t?.host || doc.body);
      return true;
    }
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
    if (cmd === 'inserttext' && !value) return true;
    const toggles = cmd === 'inserttext' && range.collapsed ? typingStyle(state, range) : null;
    state.typing = null;
    range.deleteContents();
    if (cmd === 'insertparagraph') {
      splitParagraph(doc, t, state.separator);
      input(host, 'insertParagraph');
    } else if (cmd === 'inserttext') {
      const lines = value.replace(/\r\n?/g, '\n').split('\n');
      for (let i = 0; i < lines.length; ++i) {
        if (i) splitParagraph(doc, {host, sel, range:sel.getRangeAt(0)}, state.separator);
        if (!lines[i]) continue;
        if (toggles?.size) {
          const node = doc.createTextNode(lines[i]);
          sel.getRangeAt(0).insertNode(node);
          for (const [name, enabled] of toggles) {
            if (valueFormats.has(name)) { wrapValue(node, name, enabled, state.css); continue; }
            if (!enabled) clearFormat(node, host, name);
            if (activeFormat(node.parentElement, name) !== enabled) wrapFormat(node, name, enabled, state.css);
          }
          sel.collapse(node, node.length);
          state.typing = { node, offset: node.length, formats: toggles };
        } else insertText(doc, sel.getRangeAt(0), sel, lines[i]);
      }
      input(host, 'insertText', value);
    } else {
      let node;
      if (cmd === 'inserthtml') node = range.createContextualFragment(value);
      else {
        node = doc.createElement(cmd === 'insertimage' ? 'img' : cmd === 'insertlinebreak' ? 'br' : 'hr');
        if (cmd === 'insertimage') node.setAttribute('src', value);
      }
      insertAt(range, node, sel);
      input(host, cmd === 'insertlinebreak' ? 'insertLineBreak' : cmd === 'inserthorizontalrule' ? 'insertHorizontalRule' : '');
    }
    return true;
  }

  N.setFactory('userEdit', function (host, cmd, value) {
    const doc = host.ownerDocument, state = context(doc), t = target(doc);
    if (state.executing || !t || t.host !== host) return false;
    state.userEdit = state.executing = true;
    state.changed = false;
    try {
      execute(doc, state, cmd, value);
      return state.changed;
    } finally { state.userEdit = state.executing = false; }
  });

  function command(value, count) {
    if (!count) throw new TypeError('A command is required');
    const cmd = `${value}`.toLowerCase();
    return cmd === 'hilitecolor' ? 'backcolor' : cmd;
  }
  const methods = {
    execCommand(name, showUI = false, value = '') {
      const state = context(this), cmd = command(name, arguments.length);
      const text = `${value}`;
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
      if (blockCommand(cmd)) return blockQuery(target(this),cmd,'state');
      return formats.has(cmd) ? formatState(state, target(this), cmd, false) : cmd === 'stylewithcss' && state.css;
    },
    queryCommandIndeterm(name) {
      const state = context(this), cmd = command(name, arguments.length);
      if (blockCommand(cmd)) return blockQuery(target(this),cmd,'mixed');
      return valueFormats.has(cmd) ? selectedValue(state, target(this), cmd, true) : formats.has(cmd) && formatState(state, target(this), cmd, true);
    },
    queryCommandValue(name) {
      const state = context(this);
      const cmd = command(name, arguments.length);
      if (blockCommand(cmd)) return blockQuery(target(this),cmd,'value');
      return valueFormats.has(cmd) ? selectedValue(state, target(this), cmd, false) : cmd === 'defaultparagraphseparator' ? state.separator : '';
    },
  };
  for (const [name, value] of Object.entries(methods))
    Object.defineProperty(Document.prototype, name, { value, enumerable: true, configurable: true, writable: true });
})();
