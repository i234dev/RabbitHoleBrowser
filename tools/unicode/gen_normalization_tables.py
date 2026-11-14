#!/usr/bin/env python3
"""Generate the Unicode 17.0.0 normalization data used by the JS engine.

Usage: gen_normalization_tables.py <ucd-dir> > \
crates/js/src/builtins/string/normalization_data.rs

Inputs: https://www.unicode.org/Public/17.0.0/ucd/{UnicodeData,DerivedNormalizationProps}.txt
The pinned hashes make regeneration independent of the host Python Unicode version.
Unicode data is distributed under https://www.unicode.org/license.txt.
"""
import hashlib
import pathlib
import sys

EXPECTED = {
    'UnicodeData.txt': '2e1efc1dcb59c575eedf5ccae60f95229f706ee6d031835247d843c11d96470c',
    'DerivedNormalizationProps.txt': '71fd6a206a2c0cdd41feb6b7f656aa31091db45e9cedc926985d718397f9e488',
}
root = pathlib.Path(sys.argv[1])
inputs = {}
for name, expected in EXPECTED.items():
    data = (root / name).read_bytes()
    if hashlib.sha256(data).hexdigest() != expected:
        raise SystemExit(f'{name}: unexpected input hash (Unicode 17.0.0 required)')
    inputs[name] = data.decode('utf-8')

ccc = {}
decompositions = []
pool = []
for line in inputs['UnicodeData.txt'].splitlines():
    fields = line.split(';')
    cp = int(fields[0], 16)
    combining = int(fields[3])
    if combining:
        assert not fields[1].endswith((', First>', ', Last>'))
        ccc[cp] = combining
    mapping = fields[5].split()
    if mapping:
        compatibility = mapping[0].startswith('<')
        if compatibility:
            mapping = mapping[1:]
        values = [int(v, 16) for v in mapping]
        decompositions.append((cp, len(pool), len(values), compatibility))
        pool.extend(values)
assert len(pool) < 65536
assert all(length < 256 for _, _, length, _ in decompositions)

excluded = set()
for line in inputs['DerivedNormalizationProps.txt'].splitlines():
    fields = [s.strip() for s in line.split('#')[0].split(';')]
    if len(fields) < 2 or fields[1] != 'Full_Composition_Exclusion':
        continue
    bounds = fields[0].split('..')
    excluded.update(range(int(bounds[0], 16), int(bounds[-1], 16) + 1))
compositions = {}
for cp, offset, length, compatibility in decompositions:
    if compatibility or length != 2 or cp in excluded:
        continue
    a, b = pool[offset:offset + length]
    assert ccc.get(a, 0) == 0
    key = (a << 21) | b
    assert key not in compositions
    compositions[key] = cp

ranges = []
for cp, combining in sorted(ccc.items()):
    if ranges and cp == ranges[-1][1] + 1 and combining == ranges[-1][2]:
        ranges[-1] = (ranges[-1][0], cp, combining)
    else:
        ranges.append((cp, cp, combining))
print('//! Unicode 17.0.0 normalization data; generated, do not edit.')
print('//! Regenerate with tools/unicode/gen_normalization_tables.py.')
print('//! Source: https://www.unicode.org/Public/17.0.0/ucd/')
print('//! Data license: https://www.unicode.org/license.txt')
print('//! See tools/unicode/LICENSE-UNICODE.txt for the required notice.')
for name, digest in EXPECTED.items():
    print(f'//! {name} SHA-256: {digest}')
print('\npub(super) static COMBINING: &[(u32, u32, u8)] = &[')
for a, b, combining in ranges:
    print(f'    (0x{a:X}, 0x{b:X}, {combining}),')
print('];\npub(super) static DECOMPOSITIONS: &[(u32, u16, u8, bool)] = &[')
for cp, offset, length, compatibility in decompositions:
    print(f'    (0x{cp:X}, {offset}, {length}, {str(compatibility).lower()}),')
print('];\npub(super) static DECOMPOSED: &[u32] = &[')
for start in range(0, len(pool), 12):
    print('    ' + ', '.join(f'0x{cp:X}' for cp in pool[start:start + 12]) + ',')
print('];\npub(super) static COMPOSITIONS: &[(u64, u32)] = &[')
for key, cp in sorted(compositions.items()):
    print(f'    (0x{key:X}, 0x{cp:X}),')
print('];')
