// Event interfaces (DOM, UI Events, Pointer Events, Touch Events, Input
// Events, CSS animations/transitions, Clipboard, HTML): prototype
// attributes, legacy init* methods, constants and the event classes not
// provided natively. Natively created events carry their values as own
// properties; the prototype accessors provide the spec defaults.
(function () {
  'use strict';
  const g = globalThis;
  const N = g.__rhNative;
  const define = (name, value) => Object.defineProperty(g, name, { value, writable: true, configurable: true });
  const hidden = (o, k, v) => Object.defineProperty(o, k, { value: v, writable: true, configurable: true });
  const tag = (C, name) => Object.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  const ILLEGAL = () => { throw new TypeError('Illegal constructor'); };

  // Prototype accessors returning the IDL default (instances shadow them
  // with their own values).
  function defaults(C, map) {
    if (!C) return;
    for (const [k, v] of Object.entries(map)) {
      if (Object.prototype.hasOwnProperty.call(C.prototype, k)) continue;
      Object.defineProperty(C.prototype, k, { get() { return typeof v === 'function' ? v.call(this) : v; }, configurable: true, enumerable: true });
    }
  }
  const consts = (C, map) => {
    if (!C) return;
    for (const [k, v] of Object.entries(map)) {
      if (!Object.prototype.hasOwnProperty.call(C, k)) Object.defineProperty(C, k, { value: v, enumerable: true });
      if (!Object.prototype.hasOwnProperty.call(C.prototype, k)) Object.defineProperty(C.prototype, k, { value: v, enumerable: true });
    }
  };
  const method = (C, name, fn) => { if (C && !Object.prototype.hasOwnProperty.call(C.prototype, name)) hidden(C.prototype, name, fn); };
  // Copy dictionary members onto a freshly constructed event.
  const assign = (ev, init, map) => {
    for (const [k, d] of Object.entries(map)) {
      const v = init && init[k] !== undefined ? init[k] : d;
      Object.defineProperty(ev, k, { value: v, enumerable: true, configurable: true, writable: false });
    }
  };
  // Subclass a native Event class with a JS dictionary.
  function eventClass(name, Parent, members) {
    const C = {
      [name]: class extends Parent {
        constructor(type, init) {
          if (arguments.length < 1) throw new TypeError(`Failed to construct '${name}': 1 argument required, but only 0 present.`);
          super(type, init);
          assign(this, init || {}, members);
        }
      },
    }[name];
    tag(C, name);
    defaults(C, members);
    return C;
  }

  consts(g.Event, { NONE: 0, CAPTURING_PHASE: 1, AT_TARGET: 2, BUBBLING_PHASE: 3 });

  // ------------------------------------------------------------ UI Events
  defaults(g.UIEvent, { detail: 0, view: null, sourceCapabilities: null, which: function () { return this.keyCode !== undefined ? this.keyCode : this.button !== undefined ? this.button + 1 : 0; } });
  method(g.UIEvent, 'initUIEvent', function (type, bubbles, cancelable, view, detail) { this.initEvent(type, bubbles, cancelable); Object.defineProperty(this, 'view', { value: view ?? null, configurable: true }); Object.defineProperty(this, 'detail', { value: detail | 0, configurable: true }); });
  const MOUSE = { screenX: 0, screenY: 0, clientX: 0, clientY: 0, layerX: 0, layerY: 0, ctrlKey: false, shiftKey: false, altKey: false, metaKey: false, button: 0, buttons: 0, relatedTarget: null, pageX: 0, pageY: 0, x: 0, y: 0, offsetX: 0, offsetY: 0, movementX: 0, movementY: 0 };
  defaults(g.MouseEvent, MOUSE);
  const modifierState = function (key) {
    switch (String(key)) {
      case 'Control': return !!this.ctrlKey;
      case 'Shift': return !!this.shiftKey;
      case 'Alt': return !!this.altKey;
      case 'Meta': return !!this.metaKey;
      default: return !!(this.__modifiers && this.__modifiers[key]);
    }
  };
  method(g.MouseEvent, 'getModifierState', modifierState);
  defaults(g.PointerEvent, { pointerId: 0, width: 1, height: 1, pressure: 0, tangentialPressure: 0, tiltX: 0, tiltY: 0, twist: 0, altitudeAngle: Math.PI / 2, azimuthAngle: 0, pointerType: '', isPrimary: false, persistentDeviceId: 0 });
  method(g.PointerEvent, 'getCoalescedEvents', function () { return this.isTrusted ? [this] : []; });
  method(g.PointerEvent, 'getPredictedEvents', function () { return []; });
  consts(g.WheelEvent, { DOM_DELTA_PIXEL: 0, DOM_DELTA_LINE: 1, DOM_DELTA_PAGE: 2 });
  defaults(g.WheelEvent, { deltaX: 0, deltaY: 0, deltaZ: 0, deltaMode: 0, momentum: 0, wheelDelta: function () { return -Math.round(this.deltaY * 1.2); }, wheelDeltaX: function () { return -Math.round(this.deltaX * 1.2); }, wheelDeltaY: function () { return -Math.round(this.deltaY * 1.2); } });
  consts(g.KeyboardEvent, { DOM_KEY_LOCATION_STANDARD: 0, DOM_KEY_LOCATION_LEFT: 1, DOM_KEY_LOCATION_RIGHT: 2, DOM_KEY_LOCATION_NUMPAD: 3 });
  defaults(g.KeyboardEvent, { key: '', code: '', location: 0, ctrlKey: false, shiftKey: false, altKey: false, metaKey: false, repeat: false, isComposing: false, charCode: 0, keyCode: 0 });
  method(g.KeyboardEvent, 'getModifierState', modifierState);
  method(g.KeyboardEvent, 'initKeyboardEvent', function (type, bubbles, cancelable, view, key, location, ctrl, alt, shift, meta) {
    this.initEvent(type, bubbles, cancelable);
    const set = (k, v) => Object.defineProperty(this, k, { value: v, configurable: true, enumerable: true });
    set('view', view ?? null); set('key', String(key ?? '')); set('location', location | 0);
    set('ctrlKey', !!ctrl); set('altKey', !!alt); set('shiftKey', !!shift); set('metaKey', !!meta);
  });
  defaults(g.FocusEvent, { relatedTarget: null });
  defaults(g.InputEvent, { data: null, isComposing: false, inputType: '', dataTransfer: null });
  method(g.InputEvent, 'getTargetRanges', function () { return []; });
  defaults(g.CompositionEvent, { data: '' });
  method(g.CompositionEvent, 'initCompositionEvent', function (type, bubbles, cancelable, view, data) { this.initEvent(type, bubbles, cancelable); Object.defineProperty(this, 'data', { value: String(data ?? ''), configurable: true }); });
  class TextEvent extends g.UIEvent {
    constructor() { super(''); ILLEGAL(); }
    initTextEvent(type, bubbles, cancelable, view, data) { this.initEvent(type, bubbles, cancelable); Object.defineProperty(this, 'data', { value: String(data ?? ''), configurable: true }); }
  }
  defaults(TextEvent, { data: '' });
  tag(TextEvent, 'TextEvent');
  define('TextEvent', TextEvent);

  // ---------------------------------------------------------------- Touch
  const TS = Symbol('touch');
  class Touch {
    constructor(init) {
      if (!init || !(init.target instanceof g.EventTarget)) throw new TypeError("Failed to construct 'Touch': required member target is undefined.");
      if (init.identifier === undefined) throw new TypeError("Failed to construct 'Touch': required member identifier is undefined.");
      hidden(this, TS, Object.assign({ clientX: 0, clientY: 0, screenX: 0, screenY: 0, pageX: 0, pageY: 0, radiusX: 0, radiusY: 0, rotationAngle: 0, force: 0, altitudeAngle: Math.PI / 2, azimuthAngle: 0, touchType: 'direct' }, init));
    }
  }
  for (const k of ['identifier', 'target', 'screenX', 'screenY', 'clientX', 'clientY', 'pageX', 'pageY', 'radiusX', 'radiusY', 'rotationAngle', 'force', 'altitudeAngle', 'azimuthAngle', 'touchType']) {
    Object.defineProperty(Touch.prototype, k, { get() { return this[TS][k]; }, configurable: true, enumerable: true });
  }
  tag(Touch, 'Touch');
  class TouchList {
    constructor() { ILLEGAL(); }
    get length() { return this[TS].length; }
    item(i) { return this[TS][i] ?? null; }
    [Symbol.iterator]() { return this[TS][Symbol.iterator](); }
  }
  tag(TouchList, 'TouchList');
  const touchList = (arr) => { const l = Object.create(TouchList.prototype); hidden(l, TS, Array.from(arr || [])); l[TS].forEach((t, i) => Object.defineProperty(l, i, { value: t, enumerable: true })); return l; };
  define('Touch', Touch);
  define('TouchList', TouchList);
  if (g.TouchEvent) {
    const Native = g.TouchEvent;
    const TouchEvent = class TouchEvent extends g.UIEvent {
      constructor(type, init = {}) {
        super(type, init);
        assign(this, init, { altKey: false, metaKey: false, ctrlKey: false, shiftKey: false });
        for (const k of ['touches', 'targetTouches', 'changedTouches']) Object.defineProperty(this, k, { value: touchList(init[k]), enumerable: true, configurable: true });
      }
    };
    tag(TouchEvent, 'TouchEvent');
    defaults(TouchEvent, { touches: () => touchList([]), targetTouches: () => touchList([]), changedTouches: () => touchList([]), altKey: false, metaKey: false, ctrlKey: false, shiftKey: false });
    hidden(TouchEvent.prototype, 'getModifierState', modifierState);
    N.registerProto('TouchEvent', TouchEvent.prototype);
    define('TouchEvent', TouchEvent);
    void Native;
  }

  // ------------------------------------------------- other event classes
  const EVENTS = [
    ['AnimationEvent', g.Event, { animationName: '', elapsedTime: 0, pseudoElement: '' }],
    ['TransitionEvent', g.Event, { propertyName: '', elapsedTime: 0, pseudoElement: '' }],
    ['ErrorEvent', g.Event, { message: '', filename: '', lineno: 0, colno: 0, error: undefined }],
    ['MessageEvent', g.Event, { data: null, origin: '', lastEventId: '', source: null, ports: () => Object.freeze([]) }],
    ['StorageEvent', g.Event, { key: null, oldValue: null, newValue: null, url: '', storageArea: null }],
    ['PopStateEvent', g.Event, { state: null, hasUAVisualTransition: false }],
    ['HashChangeEvent', g.Event, { oldURL: '', newURL: '' }],
    ['PageTransitionEvent', g.Event, { persisted: false }],
    ['ProgressEvent', g.Event, { lengthComputable: false, loaded: 0, total: 0 }],
    ['PromiseRejectionEvent', g.Event, { promise: undefined, reason: undefined }],
    ['SubmitEvent', g.Event, { submitter: null }],
    ['ToggleEvent', g.Event, { oldState: '', newState: '', source: null }],
    ['ClipboardEvent', g.Event, { clipboardData: null }],
    ['SecurityPolicyViolationEvent', g.Event, { documentURI: '', referrer: '', blockedURI: '', violatedDirective: '', effectiveDirective: '', originalPolicy: '', sourceFile: '', sample: '', disposition: 'enforce', statusCode: 0, lineNumber: 0, columnNumber: 0 }],
    ['BeforeUnloadEvent', g.Event, {}],
    ['DragEvent', g.MouseEvent, { dataTransfer: null }],
  ];
  for (const [name, , members] of EVENTS) defaults(g[name], members);
  const NEW_EVENTS = [
    ['CommandEvent', g.Event, { source: null, command: '' }],
    ['FormDataEvent', g.Event, { formData: null }],
    ['PageRevealEvent', g.Event, { viewTransition: null }],
    ['PageSwapEvent', g.Event, { activation: null, viewTransition: null }],
    ['TrackEvent', g.Event, { track: null }],
    ['CloseEvent', g.Event, { wasClean: false, code: 0, reason: '' }],
    ['ClipboardChangeEvent', g.Event, { types: () => Object.freeze([]), changeId: 0 }],
    ['DeviceOrientationEvent', g.Event, { alpha: null, beta: null, gamma: null, absolute: false }],
    ['DeviceMotionEvent', g.Event, { acceleration: null, accelerationIncludingGravity: null, rotationRate: null, interval: 0 }],
    ['GamepadEvent', g.Event, { gamepad: null }],
    ['MediaQueryListEvent', g.Event, { media: '', matches: false }],
    ['FontFaceSetLoadEvent', g.Event, { fontfaces: () => Object.freeze([]) }],
    ['IDBVersionChangeEvent', g.Event, { oldVersion: 0, newVersion: null }],
    ['TaskPriorityChangeEvent', g.Event, { previousPriority: 'user-visible' }],
    ['ContentVisibilityAutoStateChangeEvent', g.Event, { skipped: false }],
  ];
  for (const [name, Parent, members] of NEW_EVENTS) {
    if (g[name]) { defaults(g[name], members); continue; }
    define(name, eventClass(name, Parent, members));
  }
  method(g.MessageEvent, 'initMessageEvent', function (type, bubbles, cancelable, data, origin, lastEventId, source, ports) {
    this.initEvent(type, bubbles, cancelable);
    const set = (k, v) => Object.defineProperty(this, k, { value: v, configurable: true, enumerable: true });
    set('data', data ?? null); set('origin', String(origin ?? '')); set('lastEventId', String(lastEventId ?? '')); set('source', source ?? null); set('ports', Object.freeze(Array.from(ports || [])));
  });
  method(g.StorageEvent, 'initStorageEvent', function (type, bubbles, cancelable, key, oldValue, newValue, url, storageArea) {
    this.initEvent(type, bubbles, cancelable);
    const set = (k, v) => Object.defineProperty(this, k, { value: v, configurable: true, enumerable: true });
    set('key', key ?? null); set('oldValue', oldValue ?? null); set('newValue', newValue ?? null); set('url', String(url ?? '')); set('storageArea', storageArea ?? null);
  });
  // Native constructors that copy the whole dictionary keep the class's
  // own members; the generic subclass constructors above handle the rest.
})();
