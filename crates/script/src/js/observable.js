// Observable (WICG Observable, integrated into DOM): Observable,
// Subscriber, the operators, the Promise-returning methods and
// EventTarget.prototype.when().
(function () {
  'use strict';
  const g = globalThis;
  if (typeof g.Observable === 'function') return;
  const S = Symbol('rhObservable');
  const reportException = (e) => {
    try {
      if (typeof g.reportError === 'function') g.reportError(e);
      else setTimeout(() => { throw e; }, 0);
    } catch (_) { /* reported */ }
  };
  const define = (name, value) => Object.defineProperty(g, name, { value, writable: true, configurable: true, enumerable: false });
  const tag = (C, name) => Object.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });

  // ------------------------------------------------------------ Subscriber
  let constructing = false;
  class Subscriber {
    constructor() {
      if (!constructing) throw new TypeError("Failed to construct 'Subscriber': Illegal constructor");
    }
    next(value) {
      const s = state(this, 'next');
      if (arguments.length < 1) throw new TypeError("Failed to execute 'next' on 'Subscriber': 1 argument required, but only 0 present.");
      if (!s.active) return;
      s.observer.next(value);
    }
    error(error) {
      const s = state(this, 'error');
      if (arguments.length < 1) throw new TypeError("Failed to execute 'error' on 'Subscriber': 1 argument required, but only 0 present.");
      if (!s.active) { reportException(error); return; }
      closeSubscription(this, error);
      s.observer.error(error);
    }
    complete() {
      const s = state(this, 'complete');
      if (!s.active) return;
      closeSubscription(this, undefined);
      s.observer.complete();
    }
    addTeardown(teardown) {
      const s = state(this, 'addTeardown');
      if (typeof teardown !== 'function') throw new TypeError("Failed to execute 'addTeardown' on 'Subscriber': parameter 1 is not of type 'Function'.");
      if (s.active) s.teardowns.push(teardown);
      else { try { teardown.call(undefined); } catch (e) { reportException(e); } }
    }
    get active() { return state(this, 'active').active; }
    get signal() { return state(this, 'signal').controller.signal; }
  }
  tag(Subscriber, 'Subscriber');
  function state(sub, what) {
    const s = sub !== null && typeof sub === 'object' ? sub[S] : undefined;
    if (!s) throw new TypeError(`Failed to execute '${what}' on 'Subscriber': Illegal invocation`);
    return s;
  }
  function newSubscriber(observer) {
    constructing = true;
    let sub;
    try { sub = new Subscriber(); } finally { constructing = false; }
    Object.defineProperty(sub, S, { value: { active: true, observer, teardowns: [], controller: new AbortController() } });
    return sub;
  }
  function closeSubscription(sub, reason) {
    const s = sub[S];
    if (!s.active) return;
    s.active = false;
    if (reason === undefined) s.controller.abort(); else s.controller.abort(reason);
    const t = s.teardowns;
    s.teardowns = [];
    for (let i = t.length - 1; i >= 0; i--) {
      try { t[i].call(undefined); } catch (e) { reportException(e); }
    }
  }

  // Internal observers from the subscribe() argument.
  function makeObserver(observer) {
    if (typeof observer === 'function') {
      return {
        next: (v) => { try { observer.call(undefined, v); } catch (e) { reportException(e); } },
        error: (e) => reportException(e),
        complete: () => {},
      };
    }
    if (observer !== undefined && observer !== null && typeof observer !== 'object') throw new TypeError("Failed to execute 'subscribe' on 'Observable': The provided value is not of type 'ObserverUnion'.");
    const o = observer || {};
    const next = o.next, error = o.error, complete = o.complete;
    const check = (f, n) => { if (f !== undefined && typeof f !== 'function') throw new TypeError(`Failed to read the '${n}' property from 'Observer': The provided value is not a function.`); };
    check(next, 'next'); check(error, 'error'); check(complete, 'complete');
    return {
      next: (v) => { if (next) { try { next.call(undefined, v); } catch (e) { reportException(e); } } },
      error: (e) => { if (error) { try { error.call(undefined, e); } catch (e2) { reportException(e2); } } else reportException(e); },
      complete: () => { if (complete) { try { complete.call(undefined); } catch (e) { reportException(e); } } },
    };
  }

  function readSignal(options, where) {
    if (options === undefined || options === null) return null;
    if (typeof options !== 'object') throw new TypeError(`Failed to execute '${where}': The provided value is not of type 'SubscribeOptions'.`);
    const signal = options.signal;
    if (signal === undefined) return null;
    if (!(signal instanceof AbortSignal)) throw new TypeError(`Failed to read the 'signal' property from 'SubscribeOptions': Failed to convert value to 'AbortSignal'.`);
    return signal;
  }

  // Subscribe with an internal observer (operators).
  function subscribeInternal(observable, observer, signal) {
    const sub = newSubscriber(observer);
    if (signal) {
      if (signal.aborted) closeSubscription(sub, signal.reason);
      else signal.addEventListener('abort', () => closeSubscription(sub, signal.reason), { once: true });
    }
    const st = observable[S];
    if (st.native) st.native(sub);
    else {
      try { st.callback.call(undefined, sub); } catch (e) { sub.error(e); }
    }
    return sub;
  }

  // ------------------------------------------------------------ Observable
  class Observable {
    constructor(callback) {
      if (typeof callback !== 'function') throw new TypeError("Failed to construct 'Observable': parameter 1 is not of type 'Function'.");
      Object.defineProperty(this, S, { value: { callback, native: null } });
    }
    subscribe(observer = {}, options = {}) {
      obs(this, 'subscribe');
      const internal = makeObserver(observer);
      subscribeInternal(this, internal, readSignal(options, 'subscribe'));
    }
    static from(value) {
      if (value instanceof Observable) return value;
      if (value !== null && value !== undefined) {
        const asyncIt = value[Symbol.asyncIterator];
        if (asyncIt !== undefined && asyncIt !== null) {
          if (typeof asyncIt !== 'function') throw new TypeError("Failed to execute 'from' on 'Observable': @@asyncIterator must be a callable.");
          return nativeObservable((sub) => fromAsyncIterable(value, asyncIt, sub));
        }
        const it = value[Symbol.iterator];
        if (it !== undefined && it !== null && typeof value !== 'string') {
          if (typeof it !== 'function') throw new TypeError("Failed to execute 'from' on 'Observable': @@iterator must be a callable.");
          return nativeObservable((sub) => fromIterable(value, it, sub));
        }
        if (typeof value === 'string') return nativeObservable((sub) => fromIterable(value, it, sub));
        if (value instanceof Promise) {
          return nativeObservable((sub) => {
            value.then((v) => { sub.next(v); sub.complete(); }, (e) => sub.error(e));
          });
        }
      }
      throw new TypeError("Failed to execute 'from' on 'Observable': Cannot convert value to an Observable. Input value must be an Observable, async iterable, iterable, or Promise.");
    }

    // ---------------------------------------------------- operators
    takeUntil(notifier) {
      const source = obs(this, 'takeUntil');
      const notifierObservable = Observable.from(notifier);
      return nativeObservable((sub) => {
        subscribeInternal(notifierObservable, {
          next: () => sub.complete(),
          error: (e) => sub.error(e),
          complete: () => {},
        }, sub.signal);
        if (!sub.active) return;
        subscribeInternal(source, passthrough(sub), sub.signal);
      });
    }
    map(mapper) {
      const source = obs(this, 'map');
      fn(mapper, 'map');
      return nativeObservable((sub) => {
        let idx = 0;
        subscribeInternal(source, Object.assign(passthrough(sub), {
          next: (v) => {
            let r;
            try { r = mapper.call(undefined, v, idx++); } catch (e) { sub.error(e); return; }
            sub.next(r);
          },
        }), sub.signal);
      });
    }
    filter(predicate) {
      const source = obs(this, 'filter');
      fn(predicate, 'filter');
      return nativeObservable((sub) => {
        let idx = 0;
        subscribeInternal(source, Object.assign(passthrough(sub), {
          next: (v) => {
            let keep;
            try { keep = predicate.call(undefined, v, idx++); } catch (e) { sub.error(e); return; }
            if (keep) sub.next(v);
          },
        }), sub.signal);
      });
    }
    take(amount) {
      const source = obs(this, 'take');
      let remaining = toCount(amount);
      return nativeObservable((sub) => {
        let left = remaining;
        if (left === 0) { sub.complete(); return; }
        subscribeInternal(source, Object.assign(passthrough(sub), {
          next: (v) => {
            sub.next(v);
            if (--left === 0) sub.complete();
          },
        }), sub.signal);
      });
    }
    drop(amount) {
      const source = obs(this, 'drop');
      const n = toCount(amount);
      return nativeObservable((sub) => {
        let left = n;
        subscribeInternal(source, Object.assign(passthrough(sub), {
          next: (v) => {
            if (left > 0) { left--; return; }
            sub.next(v);
          },
        }), sub.signal);
      });
    }
    flatMap(mapper) {
      const source = obs(this, 'flatMap');
      fn(mapper, 'flatMap');
      return nativeObservable((sub) => {
        let idx = 0;
        let innerActive = false;
        let outerDone = false;
        const queue = [];
        const processNext = (v) => {
          let inner;
          try { inner = Observable.from(mapper.call(undefined, v, idx++)); } catch (e) { sub.error(e); return; }
          innerActive = true;
          subscribeInternal(inner, {
            next: (x) => sub.next(x),
            error: (e) => sub.error(e),
            complete: () => {
              if (queue.length) processNext(queue.shift());
              else {
                innerActive = false;
                if (outerDone) sub.complete();
              }
            },
          }, sub.signal);
        };
        subscribeInternal(source, {
          next: (v) => { if (innerActive) queue.push(v); else processNext(v); },
          error: (e) => sub.error(e),
          complete: () => {
            outerDone = true;
            if (!innerActive && queue.length === 0) sub.complete();
          },
        }, sub.signal);
      });
    }
    switchMap(mapper) {
      const source = obs(this, 'switchMap');
      fn(mapper, 'switchMap');
      return nativeObservable((sub) => {
        let idx = 0;
        let innerController = null;
        let outerDone = false;
        subscribeInternal(source, {
          next: (v) => {
            if (innerController) innerController.abort();
            const controller = new AbortController();
            innerController = controller;
            let inner;
            try { inner = Observable.from(mapper.call(undefined, v, idx++)); } catch (e) { sub.error(e); return; }
            const signal = AbortSignal.any([sub.signal, controller.signal]);
            subscribeInternal(inner, {
              next: (x) => sub.next(x),
              error: (e) => sub.error(e),
              complete: () => {
                if (innerController === controller) innerController = null;
                if (outerDone && innerController === null) sub.complete();
              },
            }, signal);
          },
          error: (e) => sub.error(e),
          complete: () => {
            outerDone = true;
            if (innerController === null) sub.complete();
          },
        }, sub.signal);
      });
    }
    inspect(inspector = {}) {
      const source = obs(this, 'inspect');
      let hooks;
      if (typeof inspector === 'function') hooks = { next: inspector };
      else if (inspector !== null && typeof inspector === 'object') {
        hooks = {};
        for (const k of ['next', 'error', 'complete', 'subscribe', 'abort']) {
          const f = inspector[k];
          if (f !== undefined && typeof f !== 'function') throw new TypeError(`Failed to read the '${k}' property from 'ObservableInspector': The provided value is not a function.`);
          hooks[k] = f;
        }
      } else throw new TypeError("Failed to execute 'inspect' on 'Observable': The provided value is not of type 'ObservableInspectorUnion'.");
      return nativeObservable((sub) => {
        if (hooks.subscribe) {
          try { hooks.subscribe.call(undefined); } catch (e) { sub.error(e); return; }
        }
        let onAbort = null;
        if (hooks.abort) {
          onAbort = () => { try { hooks.abort.call(undefined, sub.signal.reason); } catch (e) { reportException(e); } };
          sub.signal.addEventListener('abort', onAbort, { once: true });
        }
        const dropAbort = () => { if (onAbort) { sub.signal.removeEventListener('abort', onAbort); onAbort = null; } };
        subscribeInternal(source, {
          next: (v) => {
            if (hooks.next) { try { hooks.next.call(undefined, v); } catch (e) { dropAbort(); sub.error(e); return; } }
            sub.next(v);
          },
          error: (e) => {
            dropAbort();
            if (hooks.error) { try { hooks.error.call(undefined, e); } catch (e2) { sub.error(e2); return; } }
            sub.error(e);
          },
          complete: () => {
            dropAbort();
            if (hooks.complete) { try { hooks.complete.call(undefined); } catch (e) { sub.error(e); return; } }
            sub.complete();
          },
        }, sub.signal);
      });
    }
    catch(callback) {
      const source = obs(this, 'catch');
      fn(callback, 'catch');
      return nativeObservable((sub) => {
        subscribeInternal(source, {
          next: (v) => sub.next(v),
          error: (e) => {
            let next;
            try { next = Observable.from(callback.call(undefined, e)); } catch (e2) { sub.error(e2); return; }
            subscribeInternal(next, passthrough(sub), sub.signal);
          },
          complete: () => sub.complete(),
        }, sub.signal);
      });
    }
    finally(callback) {
      const source = obs(this, 'finally');
      fn(callback, 'finally');
      return nativeObservable((sub) => {
        sub.addTeardown(callback);
        subscribeInternal(source, passthrough(sub), sub.signal);
      });
    }

    // ---------------------------------------------------- promises
    toArray(options) {
      const source = obs(this, 'toArray');
      const values = [];
      return consume(source, options, 'toArray', (resolve) => ({
        next: (v) => values.push(v),
        complete: () => resolve(values),
      }));
    }
    forEach(callback, options) {
      const source = obs(this, 'forEach');
      fn(callback, 'forEach');
      let idx = 0;
      return consume(source, options, 'forEach', (resolve, reject, stop) => ({
        next: (v) => { try { callback.call(undefined, v, idx++); } catch (e) { reject(e); stop(e); } },
        complete: () => resolve(undefined),
      }));
    }
    every(predicate, options) {
      const source = obs(this, 'every');
      fn(predicate, 'every');
      let idx = 0;
      return consume(source, options, 'every', (resolve, reject, stop) => ({
        next: (v) => {
          let ok;
          try { ok = predicate.call(undefined, v, idx++); } catch (e) { reject(e); stop(e); return; }
          if (!ok) { resolve(false); stop(); }
        },
        complete: () => resolve(true),
      }));
    }
    some(predicate, options) {
      const source = obs(this, 'some');
      fn(predicate, 'some');
      let idx = 0;
      return consume(source, options, 'some', (resolve, reject, stop) => ({
        next: (v) => {
          let ok;
          try { ok = predicate.call(undefined, v, idx++); } catch (e) { reject(e); stop(e); return; }
          if (ok) { resolve(true); stop(); }
        },
        complete: () => resolve(false),
      }));
    }
    find(predicate, options) {
      const source = obs(this, 'find');
      fn(predicate, 'find');
      let idx = 0;
      return consume(source, options, 'find', (resolve, reject, stop) => ({
        next: (v) => {
          let ok;
          try { ok = predicate.call(undefined, v, idx++); } catch (e) { reject(e); stop(e); return; }
          if (ok) { resolve(v); stop(); }
        },
        complete: () => resolve(undefined),
      }));
    }
    first(options) {
      const source = obs(this, 'first');
      return consume(source, options, 'first', (resolve, reject, stop) => ({
        next: (v) => { resolve(v); stop(); },
        complete: () => reject(new RangeError('No values in Observable')),
      }));
    }
    last(options) {
      const source = obs(this, 'last');
      let has = false, last;
      return consume(source, options, 'last', (resolve, reject) => ({
        next: (v) => { has = true; last = v; },
        complete: () => { if (has) resolve(last); else reject(new RangeError('No values in Observable')); },
      }));
    }
    reduce(reducer, ...rest) {
      const source = obs(this, 'reduce');
      fn(reducer, 'reduce');
      let hasAcc = rest.length > 0;
      let acc = rest[0];
      let idx = 0;
      const options = rest[1];
      return consume(source, options, 'reduce', (resolve, reject, stop) => ({
        next: (v) => {
          if (!hasAcc) { hasAcc = true; acc = v; idx++; return; }
          try { acc = reducer.call(undefined, acc, v, idx++); } catch (e) { reject(e); stop(e); }
        },
        complete: () => { if (hasAcc) resolve(acc); else reject(new TypeError('Reduce of empty Observable with no initial value')); },
      }));
    }
  }
  tag(Observable, 'Observable');

  function obs(o, what) {
    if (o === null || typeof o !== 'object' || !o[S] || !(S in o)) throw new TypeError(`Failed to execute '${what}' on 'Observable': Illegal invocation`);
    return o;
  }
  function fn(f, what) {
    if (typeof f !== 'function') throw new TypeError(`Failed to execute '${what}' on 'Observable': parameter 1 is not of type 'Function'.`);
  }
  function toCount(n) {
    const v = Number(n);
    if (!Number.isFinite(v)) return Number.isNaN(v) ? 0 : (v > 0 ? Infinity : 0);
    return Math.max(0, Math.trunc(v));
  }
  function passthrough(sub) {
    return { next: (v) => sub.next(v), error: (e) => sub.error(e), complete: () => sub.complete() };
  }
  function nativeObservable(native) {
    const o = Object.create(Observable.prototype);
    Object.defineProperty(o, S, { value: { callback: null, native } });
    return o;
  }
  // A Promise settled by a subscription with an internal controller (so a
  // decided result can stop the source) and the caller's signal.
  function consume(source, options, what, makeObserver) {
    return new Promise((resolve, reject) => {
      const signal = readSignal(options, what);
      if (signal && signal.aborted) { reject(signal.reason); return; }
      const controller = new AbortController();
      const stop = (reason) => controller.abort(reason);
      if (signal) signal.addEventListener('abort', () => { reject(signal.reason); controller.abort(signal.reason); }, { once: true });
      const o = makeObserver(resolve, reject, stop);
      subscribeInternal(source, {
        next: o.next,
        error: (e) => reject(e),
        complete: o.complete,
      }, controller.signal);
    });
  }
  function fromIterable(value, iteratorMethod, sub) {
    let it;
    try { it = iteratorMethod.call(value); } catch (e) { sub.error(e); return; }
    sub.addTeardown(() => {
      if (typeof it.return === 'function') { try { it.return(); } catch (e) { reportException(e); } }
    });
    try {
      while (sub.active) {
        const r = it.next();
        if (r.done) { sub[S].teardowns = sub[S].teardowns.filter(() => false); sub.complete(); return; }
        sub.next(r.value);
      }
    } catch (e) {
      sub[S].teardowns = [];
      sub.error(e);
    }
  }
  function fromAsyncIterable(value, iteratorMethod, sub) {
    let it;
    try { it = iteratorMethod.call(value); } catch (e) { sub.error(e); return; }
    let finished = false;
    sub.addTeardown(() => {
      if (!finished && typeof it.return === 'function') { try { Promise.resolve(it.return()).catch(() => {}); } catch (e) { reportException(e); } }
    });
    const step = () => {
      if (!sub.active) return;
      let p;
      try { p = Promise.resolve(it.next()); } catch (e) { finished = true; sub.error(e); return; }
      p.then((r) => {
        if (!sub.active) return;
        if (r.done) { finished = true; sub.complete(); return; }
        sub.next(r.value);
        step();
      }, (e) => { finished = true; sub.error(e); });
    };
    step();
  }

  define('Observable', Observable);
  define('Subscriber', Subscriber);

  // EventTarget.prototype.when(type, options)
  if (g.EventTarget) {
    Object.defineProperty(g.EventTarget.prototype, 'when', {
      value: function when(type, options) {
        if (this === null || this === undefined) throw new TypeError("Failed to execute 'when' on 'EventTarget': Illegal invocation");
        const target = this;
        type = String(type);
        const capture = !!(options && options.capture);
        const passive = options && options.passive !== undefined ? !!options.passive : undefined;
        return nativeObservable((sub) => {
          if (sub.signal.aborted) return;
          const opts = { capture, signal: sub.signal };
          if (passive !== undefined) opts.passive = passive;
          target.addEventListener(type, (e) => sub.next(e), opts);
        });
      },
      writable: true, configurable: true, enumerable: false,
    });
  }
})();
