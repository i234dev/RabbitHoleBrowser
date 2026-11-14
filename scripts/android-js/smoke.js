// Portable, deterministic diagnostics. No browser, filesystem or clock APIs.
(function () {
  function assert(condition, message) {
    if (!condition) throw new Error(message);
  }
  const closures = [];
  for (let i = 0; i < 4; i++) closures.push(() => i);
  assert(closures.map(fn => fn()).join(',') === '0,1,2,3', 'closure bindings');
  const prototype = { get answer() { return this.value + 2; } };
  const object = Object.create(prototype);
  object.value = 40;
  assert(object.answer === 42, 'prototype getter receiver');
  assert(Object.is(-0, -1 / Infinity), 'negative zero');
  assert(Number.isNaN(0 / 0), 'NaN');
  assert(9007199254740991n + 2n === 9007199254740993n, 'BigInt precision');
  let finalized = false;
  try { throw new Error('expected'); } catch (error) {
    assert(error.message === 'expected', 'throw/catch');
  } finally { finalized = true; }
  assert(finalized, 'finally');
  print('PASS semantics answer=42');

  const unicode = 'A😀é漢';
  assert(unicode.length === 5, 'UTF-16 length');
  assert(Array.from(unicode).length === 4, 'Unicode iteration');
  assert(unicode.codePointAt(1) === 0x1f600, 'astral code point');
  assert(unicode.charCodeAt(2) === 0xde00, 'low surrogate');
  assert(JSON.parse(JSON.stringify(unicode)) === unicode, 'Unicode JSON');
  print('PASS unicode utf16=5 codepoints=4');

  const buffer = new ArrayBuffer(16);
  const view = new DataView(buffer);
  view.setUint32(0, 0x12345678, true);
  assert(view.getUint32(0, false) === 0x78563412, 'explicit byte order');
  const bytes = new Uint8Array(buffer);
  assert(Array.from(bytes.subarray(0, 4)).join(',') === '120,86,52,18', 'typed bytes');
  const alias = bytes.subarray(4, 8);
  alias[0] = 99;
  assert(bytes[4] === 99, 'shared subarray');
  const signed = new Int32Array([-1, 0x7fffffff, 0x80000000]);
  assert(signed[2] === -2147483648 && signed[0] === -1, 'signed conversion');
  const floats = new Float64Array([-0, NaN, 1.5]);
  assert(Object.is(floats[0], -0) && Number.isNaN(floats[1]) && floats[2] === 1.5, 'float storage');
  print('PASS typed-arrays endian=12345678 alias=99');

  const roots = [];
  const lookup = new Map();
  for (let i = 0; i < 128; i++) {
    const item = { index: i, values: Array(16).fill(i), label: 'root😀' + i };
    item.self = item;
    if (i % 16 === 0) { roots.push(item); lookup.set(i, item); }
  }
  let sum = 0;
  for (const item of roots) {
    assert(item.self === item && item.values[3] === item.index, 'retained cycle');
    assert(lookup.get(item.index) === item && item.label.indexOf('😀') === 4, 'retained map/string');
    sum += item.index;
  }
  assert(roots.length === 8 && sum === 448, 'retained values');
  print('PASS retained-objects roots=8 sum=448');

  Promise.resolve(6).then(value => {
    assert(value * 7 === 42, 'promise job');
    print('PASS jobs answer=42');
  });
})();
