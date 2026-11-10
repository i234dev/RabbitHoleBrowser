// Web API coverage checker. Defines `__rhWebApiCoverage(tsv, listMissing)`,
// which walks the global object / interface prototypes and compares them with
// tools/webapi/inventory.tsv (generated from the specs' WebIDL). Returns a
// JSON string: { total, present, specs: { spec: { tier, total, present,
// missing: [...] } } }.
//
// Run by the `webapi_coverage` unit test of crates/script, and against a real
// page with:  rabbithole js <URL> @tools/webapi/live_check.js
// (live_check.js is produced by tools/webapi/bundle.py).
function __rhWebApiCoverage(tsv, listMissing) {
  'use strict';
  const g = globalThis;
  let samples = null;
  function sample(iface) {
    if (!samples) {
      samples = {};
      const tryS = (k, f) => { try { const v = f(); if (v != null) samples[k] = v; } catch (e) {} };
      tryS('Window', () => g);
      tryS('Document', () => document);
      tryS('Location', () => location);
      tryS('Navigator', () => navigator);
      tryS('History', () => history);
      tryS('Screen', () => screen);
      tryS('Performance', () => performance);
      tryS('Storage', () => localStorage);
      tryS('Crypto', () => crypto);
      tryS('CSSStyleDeclaration', () => document.documentElement.style);
      tryS('CSSStyleProperties', () => document.documentElement.style);
    }
    return samples[iface];
  }
  function has(obj, name) {
    if (obj == null) return false;
    const key = name === '@@iterator' ? Symbol.iterator : name === '@@asyncIterator' ? Symbol.asyncIterator : name;
    try { return key in Object(obj); } catch (e) { return false; }
  }
  function check(iface, kind, member) {
    const I = g[iface];
    switch (kind) {
      case 'interface': return g[member] != null && (typeof g[member] === 'function' || typeof g[member] === 'object');
      case 'inherits': {
        const P = g[member];
        if (typeof I !== 'function' || typeof P !== 'function') return false;
        try { return Object.getPrototypeOf(I.prototype) === P.prototype; } catch (e) { return false; }
      }
      case 'const': case 'static': return has(I, member);
      case 'global': return has(g, member);
      case 'proto': {
        if (I != null && I.prototype != null && has(I.prototype, member)) return true;
        return has(sample(iface), member);
      }
    }
    return false;
  }
  const out = { total: 0, present: 0, specs: {} };
  for (const line of String(tsv).split('\n')) {
    if (!line || line[0] === '#') continue;
    const [spec, tier, iface, kind, member] = line.split('\t');
    const s = out.specs[spec] || (out.specs[spec] = { tier, total: 0, present: 0, missing: [] });
    s.total++; out.total++;
    let ok = false;
    try { ok = check(iface, kind, member); } catch (e) { ok = false; }
    if (ok) { s.present++; out.present++; }
    else if (listMissing) s.missing.push(kind === 'interface' ? member : kind === 'inherits' ? iface + ' : ' + member : iface + (kind === 'proto' ? '#' : kind === 'global' ? ' window.' : '.') + member);
  }
  return JSON.stringify(out);
}
