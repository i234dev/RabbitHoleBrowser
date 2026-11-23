// HTML editing commands operate on the active selection in a single editing
// host. Each document keeps its own editing preferences and reentrancy flag.
(function () {
  'use strict';
  if (globalThis.__rhNative.workerInfo()) return;

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

  const preferences = new Set(['stylewithcss', 'usecss', 'defaultparagraphseparator']);
  const textCommands = new Set(['inserttext', 'delete', 'forwarddelete']);
  const richCommands = new Set(['inserthtml', 'insertimage', 'inserthorizontalrule', 'insertlinebreak', 'insertparagraph']);
  function supported(cmd) {
    return preferences.has(cmd) || textCommands.has(cmd) || richCommands.has(cmd) || cmd === 'selectall';
  }
  function editingHost(node) {
    let el = node?.nodeType === 1 ? node : node?.parentElement;
    if (!el?.isContentEditable) return null;
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
    return !t.control && richCommands.has(cmd) &&
      (t.host.contentEditable !== 'plaintext-only' || cmd === 'insertlinebreak' || cmd === 'insertparagraph');
  }

  function input(host, inputType, data = null) {
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
      if (sel.isCollapsed || editingHost(sel.focusNode) !== host) { sel.collapse(node, offset); return false; }
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
    if (t.control) { editControl(t.control, cmd, value); return true; }
    if (cmd === 'delete' || cmd === 'forwarddelete') {
      if (deleteSelection(t, cmd)) input(t.host, inputTypeFor(cmd));
      return true;
    }
    if (cmd === 'insertimage' && !value) return false;
    const { sel, range, host } = t;
    if (cmd === 'inserttext' && !value) return true;
    range.deleteContents();
    if (cmd === 'insertparagraph') {
      splitParagraph(doc, t, state.separator);
      input(host, 'insertParagraph');
    } else if (cmd === 'inserttext') {
      const lines = value.replace(/\r\n?/g, '\n').split('\n');
      for (let i = 0; i < lines.length; ++i) {
        if (i) splitParagraph(doc, {host, sel, range:sel.getRangeAt(0)}, state.separator);
        if (lines[i]) insertText(doc, sel.getRangeAt(0), sel, lines[i]);
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

  function command(value, count) {
    if (!count) throw new TypeError('A command is required');
    return `${value}`.toLowerCase();
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
      return command(name, arguments.length) === 'stylewithcss' && state.css;
    },
    queryCommandIndeterm(name) { context(this); command(name, arguments.length); return false; },
    queryCommandValue(name) {
      const state = context(this);
      return command(name, arguments.length) === 'defaultparagraphseparator' ? state.separator : '';
    },
  };
  for (const [name, value] of Object.entries(methods))
    Object.defineProperty(Document.prototype, name, { value, enumerable: true, configurable: true, writable: true });
})();
