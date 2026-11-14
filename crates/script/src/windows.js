// Proxies for real top-level contexts owned by the embedder. Every
// origin-sensitive operation rechecks the live registry in the native.
(function (info, clone) {
  'use strict';
  const proxies = new Map();
  const selfId = info(-1, 'self');
  const security = () => { throw new DOMException('Cross-origin WindowProxy access is denied.', 'SecurityError'); };
  const same = (id) => info(id, 'same') === true;
  const keys = ['window', 'self', 'top', 'parent', 'location', 'closed', 'close', 'focus', 'opener', 'postMessage'];
  function windowFor(id) {
    if (id === null || id === undefined) return null;
    if (id === selfId) return globalThis;
    if (proxies.has(id)) return proxies.get(id);
    const location = new Proxy(Object.create(null), {
      get(_, key) {
        if (key === Symbol.toStringTag) return 'Location';
        if (key === 'href') return info(id, 'href');
        if (key === 'replace') return replace;
        if (!same(id)) security();
        if (key === 'assign') return assign;
        if (key === 'toString' || key === Symbol.toPrimitive) return () => info(id, 'href');
        return undefined;
      },
      set(_, key, value) {
        if (key === 'href') { info(id, 'navigate', String(value)); return true; }
        if (!same(id)) security();
        throw new DOMException('This Location setter is not implemented.', 'NotSupportedError');
      },
      getPrototypeOf() { return null; },
      setPrototypeOf() { return false; },
      defineProperty() { if (!same(id)) security(); throw new DOMException('Cannot redefine a remote Location.', 'NotSupportedError'); },
      preventExtensions() { return false; },
    });
    const close = () => info(id, 'close');
    const focus = () => info(id, 'focus');
    const replace = (url) => info(id, 'replace', String(url));
    const assign = (url) => info(id, 'assign', String(url));
    const postMessage = (message, targetOrigin, transfer) => {
      const options = targetOrigin && typeof targetOrigin === 'object' ? targetOrigin : null;
      const list = transfer === undefined && options ? options.transfer : transfer;
      if (list !== undefined && Array.from(list).length) throw new DOMException('Window message transfer lists are not implemented.', 'NotSupportedError');
      const origin = options ? (options.targetOrigin === undefined ? '/' : String(options.targetOrigin)) : (targetOrigin === undefined ? '/' : String(targetOrigin));
      info(id, 'post', clone.serialize(message), origin);
    };
    let proxy;
    function get(key) {
      if (key === Symbol.toStringTag) return 'Window';
      if (key === 'then') return undefined;
      if (key === 'window' || key === 'self' || key === 'top' || key === 'parent') return proxy;
      if (key === 'closed') return info(id, 'closed');
      if (key === 'location') return location;
      if (key === 'close') return close;
      if (key === 'focus') return focus;
      if (key === 'postMessage') return postMessage;
      if (key === 'opener') return windowFor(info(id, 'opener'));
      if (key === 'document') return info(id, 'document');
      if (key === 'name') return info(id, 'name');
      info(id, 'unknown');
      return undefined;
    }
    proxy = new Proxy(Object.create(null), {
      get(_, key) { return get(key); },
      set(_, key, value) {
        if (key === 'location') { info(id, 'navigate', String(value)); return true; }
        if (key === 'name') { info(id, 'setName', String(value)); return true; }
        info(id, 'unknown');
        throw new DOMException('Remote Window property assignment is not implemented.', 'NotSupportedError');
      },
      getOwnPropertyDescriptor(_, key) {
        const value = get(key);
        return value === undefined ? undefined : { value, configurable: true, enumerable: true, writable: false };
      },
      ownKeys() { return keys.slice(); },
      has(_, key) { return get(key) !== undefined; },
      getPrototypeOf() { return null; },
      setPrototypeOf() { return false; },
      defineProperty() { info(id, 'unknown'); throw new DOMException('Cannot redefine a remote Window.', 'NotSupportedError'); },
      deleteProperty() { info(id, 'unknown'); return false; },
      preventExtensions() { return false; },
    });
    clone.registerWindow(proxy);
    proxies.set(id, proxy);
    return proxy;
  }
  return { windowFor, deserialize: clone.deserialize };
})
