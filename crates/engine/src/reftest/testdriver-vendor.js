// Harness-only mouse backend. Served only by wpt_reftest --mouse-actions.
(() => {
  'use strict';
  if (!window.test_driver_internal) throw new Error('testdriver.js must load before its vendor');
  const MAX_PENDING = 4, MAX_TICKS = 128, MAX_QUEUED_TICKS = 256;
  const queue = [];
  const noneIds = new Set();
  let nextId = 1, queuedTicks = 0, mouseId = null, plannedPressed = false;
  let pressed = false, pointerX = 0, pointerY = 0, fatal = null, pageRoot = null;
  test_driver_internal.in_automation = true;
  const own = (object, keys) => {
    if (!object || typeof object !== 'object' || Object.keys(object).some(key => !keys.includes(key))) {
      throw new Error('Unsupported action fields');
    }
  };
  const integer = (value, name, min, max) => {
    if (!Number.isSafeInteger(value) || value < min || value > max) throw new Error('Invalid ' + name);
    return value;
  };
  const duration = value => value === undefined ? 0 : integer(value, 'pause duration', 0, 1000);
  const rects = Element.prototype.getClientRects;
  function cancel(message) {
    if (!fatal) fatal = String(message);
    while (queue.length) queue.shift().reject(new Error(fatal));
    queuedTicks = 0;
    plannedPressed = pressed;
    return 'cancelled';
  }
  function normalize(actions, context) {
    if (context !== null && context !== undefined && context !== window) throw new Error('Unsupported browsing context');
    if (!Array.isArray(actions) || actions.length > 2) throw new Error('Only one mouse and one none source are supported');
    let pointer = null, general = null, id = mouseId, noneId = null, count = 0;
    const ids = new Set();
    for (const source of actions) {
      own(source, ['type', 'id', 'parameters', 'actions']);
      const sourceId = source.id, sourceType = source.type;
      const list = source.actions, parameters = source.parameters;
      if (typeof sourceId !== 'string' || !sourceId.length || sourceId.length > 64 || ids.has(sourceId)) throw new Error('Invalid source id');
      ids.add(sourceId);
      if (!Array.isArray(list) || list.length > MAX_TICKS) throw new Error('Too many action ticks');
      count = Math.max(count, list.length);
      if (sourceType === 'pointer' && !pointer) {
        if (parameters !== undefined) {
          own(parameters, ['pointerType']);
          const kind = parameters.pointerType;
          if (kind !== undefined && kind !== 'mouse') throw new Error('Only mouse pointers are supported');
        }
        if (noneIds.has(sourceId)) throw new Error('Input source changed type');
        if (id !== null && id !== sourceId) throw new Error('A second mouse source id is not supported');
        id = sourceId;
        pointer = list;
      } else if (sourceType === 'none' && !general) {
        if (parameters !== undefined) throw new Error('None source parameters are unsupported');
        if (sourceId === mouseId) throw new Error('Input source changed type');
        noneId = sourceId;
        general = list;
      } else {
        throw new Error('Unsupported or duplicate input source');
      }
    }
    count = Math.max(1, count);
    if (queuedTicks + count > MAX_QUEUED_TICKS) throw new Error('Action queue tick limit exceeded');
    const ticks = [];
    let projected = plannedPressed, total = 0;
    for (let index = 0; index < count; index++) {
      const mouse = pointer && pointer[index] !== undefined ? pointer[index] : {type: 'pause'};
      const none = general && general[index] !== undefined ? general[index] : {type: 'pause'};
      own(none, ['type', 'duration']);
      if (none.type !== 'pause') throw new Error('None source supports pause only');
      let wait = duration(none.duration), tick;
      const type = mouse.type;
      if (type === 'pause') {
        own(mouse, ['type', 'duration']);
        wait = Math.max(wait, duration(mouse.duration));
        tick = {verb: 'P'};
      } else if (type === 'pointerMove') {
        own(mouse, ['type', 'x', 'y', 'origin', 'duration']);
        if (duration(mouse.duration) !== 0) throw new Error('Timed pointer movement is unsupported');
        const x = integer(mouse.x, 'x', -1000000, 1000000);
        const y = integer(mouse.y, 'y', -1000000, 1000000);
        const origin = mouse.origin === undefined ? 'viewport' : mouse.origin;
        if (origin !== 'viewport' && origin !== 'pointer' && !(origin instanceof Element && origin.ownerDocument === document)) throw new Error('Unsupported pointer origin');
        if (origin === 'viewport' && (x < 0 || y < 0 || x > innerWidth || y > innerHeight)) throw new Error('Pointer coordinates out of bounds');
        tick = {verb: 'M', x, y, origin};
      } else if (type === 'pointerDown' || type === 'pointerUp') {
        own(mouse, ['type', 'button']);
        if (mouse.button !== 0) throw new Error('Only the primary button is supported');
        const down = type === 'pointerDown';
        // WebDriver repeats are no-ops, with no extra event dispatch.
        tick = {verb: projected === down ? 'P' : down ? 'D' : 'U'};
        projected = down;
      } else {
        throw new Error('Unsupported pointer action');
      }
      total += wait;
      if (total > 4000) throw new Error('Action sequence exceeds pause budget');
      tick.wait = wait;
      ticks.push(tick);
    }
    return {ticks, id, projected, noneId};
  }
  test_driver_internal.action_sequence = function(actions, context = null) {
    return new Promise((resolve, reject) => {
      try {
        if (fatal) throw new Error(fatal);
        if (queue.length >= MAX_PENDING || nextId > 65535) throw new Error('Action queue request limit exceeded');
        const request = normalize(actions, context);
        // Publish only after validating and snapshotting the whole sequence.
        if (pageRoot === null) pageRoot = document.documentElement;
        if (pageRoot !== document.documentElement) throw new Error('Document root changed during actions');
        mouseId = request.id;
        if (request.noneId !== null) noneIds.add(request.noneId);
        plannedPressed = request.projected;
        queuedTicks += request.ticks.length;
        queue.push({id: nextId++, ticks: request.ticks, index: 0, pending: false,
                    root: document.documentElement, resolve, reject});
      } catch (error) {
        reject(error);
        // Unsupported automation must remain an explicit harness ERROR, even
        // when a reftest happens to clear its screenshot wait on rejection.
        cancel(error.message || 'Invalid action sequence');
      }
    });
  };
  const bridge = {
    poll() {
      if (fatal) return 'E ' + encodeURIComponent(fatal);
      if (pageRoot !== null && pageRoot !== document.documentElement) { cancel('Document root changed during actions'); return 'E ' + encodeURIComponent(fatal); }
      if (!queue.length) return 'I';
      const request = queue[0];
      if (request.root !== document.documentElement) { cancel('Document root changed during actions'); return 'E ' + encodeURIComponent(fatal); }
      if (request.pending) return 'E Missing_action_acknowledgement';
      const tick = request.ticks[request.index];
      let x = pointerX, y = pointerY;
      if (tick.verb === 'M') {
        if (tick.origin === 'viewport') { x = tick.x; y = tick.y; }
        else if (tick.origin === 'pointer') { x += tick.x; y += tick.y; }
        else {
          const origin = tick.origin;
          if (!origin.isConnected || origin.ownerDocument !== document) { cancel('Pointer origin is detached'); return 'E ' + encodeURIComponent(fatal); }
          const list = rects.call(origin);
          if (!list.length) { cancel('Pointer origin has no client rect'); return 'E ' + encodeURIComponent(fatal); }
          const rect = list[0];
          const left = Math.max(0, Math.min(rect.x, rect.x + rect.width));
          const right = Math.min(innerWidth, Math.max(rect.x, rect.x + rect.width));
          const top = Math.max(0, Math.min(rect.y, rect.y + rect.height));
          const bottom = Math.min(innerHeight, Math.max(rect.y, rect.y + rect.height));
          if (!(right > left && bottom > top)) { cancel('Pointer origin is outside the viewport'); return 'E ' + encodeURIComponent(fatal); }
          x = Math.floor((left + right) / 2) + tick.x;
          y = Math.floor((top + bottom) / 2) + tick.y;
        }
      }
      if (!Number.isFinite(x) || !Number.isFinite(y) || x < 0 || y < 0 || x > innerWidth || y > innerHeight) {
        cancel('Pointer coordinates out of bounds'); return 'E ' + encodeURIComponent(fatal);
      }
      request.pending = {x, y};
      return ['S', request.id, request.index, tick.verb, x, y, tick.wait].join(' ');
    },
    ack(id, index) {
      if (fatal) return 'E ' + encodeURIComponent(fatal);
      const request = queue[0];
      if (!request || request.id !== id || request.index !== index || !request.pending) { cancel('Stale action acknowledgement'); return 'E ' + encodeURIComponent(fatal); }
      if (request.root !== document.documentElement) { cancel('Document root changed during actions'); return 'E ' + encodeURIComponent(fatal); }
      const tick = request.ticks[index];
      if (tick.verb === 'M') { pointerX = request.pending.x; pointerY = request.pending.y; }
      if (tick.verb === 'D') pressed = true;
      if (tick.verb === 'U') pressed = false;
      request.pending = false;
      request.index++;
      if (request.index === request.ticks.length) {
        queue.shift();
        queuedTicks -= request.ticks.length;
        if (!queue.length) plannedPressed = pressed;
        request.resolve();
      }
      return 'A';
    },
    cancel,
  };
  Object.defineProperty(window, '__rhWptMouseActions', {value: Object.freeze(bridge)});
})();
