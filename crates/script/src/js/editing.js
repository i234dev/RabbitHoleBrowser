// HTML editing commands (execCommand and queryCommand*) and keyboard editing.
// This file only defines the entry points; the implementation
// (editing_impl.js with the spec algorithms of editing_spec.js) is compiled
// the first time a document runs or queries a command or the user edits.
(function () {
  'use strict';
  const N = globalThis.__rhNative;
  if (N.workerInfo()) return;
  let impl = null;
  const api = () => impl || (impl = N.editingImpl(false)(N, N.editingImpl(true)(N)));
  const apply = Reflect.apply;
  N.setFactory('userEdit', function (host, cmd, value) { return api().userEdit(host, cmd, value); });
  const methods = {
    execCommand(name, showUI = false, value = '') { return apply(api().methods.execCommand, this, arguments); },
    queryCommandSupported(name) { return apply(api().methods.queryCommandSupported, this, arguments); },
    queryCommandEnabled(name) { return apply(api().methods.queryCommandEnabled, this, arguments); },
    queryCommandState(name) { return apply(api().methods.queryCommandState, this, arguments); },
    queryCommandIndeterm(name) { return apply(api().methods.queryCommandIndeterm, this, arguments); },
    queryCommandValue(name) { return apply(api().methods.queryCommandValue, this, arguments); },
  };
  for (const [name, value] of Object.entries(methods))
    Object.defineProperty(Document.prototype, name, { value, enumerable: true, configurable: true, writable: true });
})();
