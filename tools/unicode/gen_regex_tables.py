#!/usr/bin/env python3
"""Generate crates/js/src/regex_unicode.rs from the Unicode Character Database.

usage: gen_regex_tables.py <ucd-dir> > crates/js/src/regex_unicode.rs

<ucd-dir> holds (flat) UnicodeData.txt Scripts.txt ScriptExtensions.txt
PropList.txt DerivedCoreProperties.txt DerivedBinaryProperties.txt
DerivedNormalizationProps.txt emoji-data.txt PropertyValueAliases.txt
CaseFolding.txt SpecialCasing.txt emoji-sequences.txt emoji-zwj-sequences.txt
from https://www.unicode.org/Public/<version>/ucd/ (+ emoji/).

Every set is emitted as sorted, non-overlapping inclusive ranges encoded as
a byte string: per range, LEB128(lo - previous hi) then LEB128(hi - lo).
"""
import os
import sys

UCD = sys.argv[1]
VERSION = sys.argv[2] if len(sys.argv) > 2 else "17.0.0"
MAXCP = 0x10FFFF


def lines(name):
    with open(os.path.join(UCD, name), encoding="utf-8") as f:
        for line in f:
            line = line.split("#", 1)[0].strip()
            if line:
                yield [x.strip() for x in line.split(";")]


def parse_range(s):
    if ".." in s:
        a, b = s.split("..")
        return int(a, 16), int(b, 16)
    return int(s, 16), int(s, 16)


def to_ranges(cps):
    out = []
    for c in sorted(cps):
        if out and out[-1][1] + 1 == c:
            out[-1][1] = c
        else:
            out.append([c, c])
    return out


# ---- General_Category
gc = {}
first = None
for f in lines("UnicodeData.txt"):
    cp = int(f[0], 16)
    name, cat = f[1], f[2]
    if name.endswith(", First>"):
        first = cp
        continue
    if name.endswith(", Last>"):
        for c in range(first, cp + 1):
            gc[c] = cat
        continue
    gc[cp] = cat
gc_sets = {}
for c in range(MAXCP + 1):
    gc_sets.setdefault(gc.get(c, "Cn"), set()).add(c)

# ---- Script / Script_Extensions
sc_long = {}  # short -> long
sc_alias = {}  # every accepted name -> short
gc_alias = {}  # every accepted name -> short
for f in lines("PropertyValueAliases.txt"):
    if f[0] == "sc":
        sc_long[f[1]] = f[2]
        for n in f[1:]:
            sc_alias[n] = f[1]
    elif f[0] == "gc":
        for n in f[1:]:
            gc_alias[n] = f[1]
long_to_short = {v: k for k, v in sc_long.items()}
script = {}
for f in lines("Scripts.txt"):
    a, b = parse_range(f[0])
    s = long_to_short[f[1]]
    for c in range(a, b + 1):
        script[c] = s
sc_sets = {}
for c in range(MAXCP + 1):
    sc_sets.setdefault(script.get(c, "Zzzz"), set()).add(c)
scx = {c: {s} for c, s in script.items()}
for f in lines("ScriptExtensions.txt"):
    a, b = parse_range(f[0])
    for c in range(a, b + 1):
        scx[c] = set(f[1].split())
scx_sets = {}
for c in range(MAXCP + 1):
    for s in scx.get(c, {"Zzzz"}):
        scx_sets.setdefault(s, set()).add(c)

# ---- binary properties (ECMA-262 table "Binary Unicode property aliases")
BINARY = """ASCII ASCII_Hex_Digit:AHex Alphabetic:Alpha Any Assigned Bidi_Control:Bidi_C
Bidi_Mirrored:Bidi_M Case_Ignorable:CI Cased Changes_When_Casefolded:CWCF
Changes_When_Casemapped:CWCM Changes_When_Lowercased:CWL Changes_When_NFKC_Casefolded:CWKCF
Changes_When_Titlecased:CWT Changes_When_Uppercased:CWU Dash Default_Ignorable_Code_Point:DI
Deprecated:Dep Diacritic:Dia Emoji Emoji_Component:EComp Emoji_Modifier:EMod
Emoji_Modifier_Base:EBase Emoji_Presentation:EPres Extended_Pictographic:ExtPict Extender:Ext
Grapheme_Base:Gr_Base Grapheme_Extend:Gr_Ext Hex_Digit:Hex IDS_Binary_Operator:IDSB
IDS_Trinary_Operator:IDST ID_Continue:IDC ID_Start:IDS Ideographic:Ideo Join_Control:Join_C
Logical_Order_Exception:LOE Lowercase:Lower Math Noncharacter_Code_Point:NChar
Pattern_Syntax:Pat_Syn Pattern_White_Space:Pat_WS Quotation_Mark:QMark Radical
Regional_Indicator:RI Sentence_Terminal:STerm Soft_Dotted:SD Terminal_Punctuation:Term
Unified_Ideograph:UIdeo Uppercase:Upper Variation_Selector:VS White_Space:space
XID_Continue:XIDC XID_Start:XIDS""".split()
bin_sets = {}
for fname in ["PropList.txt", "DerivedCoreProperties.txt", "DerivedBinaryProperties.txt",
              "DerivedNormalizationProps.txt", "emoji-data.txt"]:
    for f in lines(fname):
        if len(f) != 2:
            continue
        a, b = parse_range(f[0])
        bin_sets.setdefault(f[1], set()).update(range(a, b + 1))
bin_sets["ASCII"] = set(range(0x80))
bin_sets["Any"] = set(range(MAXCP + 1))
bin_sets["Assigned"] = set(range(MAXCP + 1)) - gc_sets["Cn"]
binary = []
for item in BINARY:
    parts = item.split(":")
    binary.append((parts[0], parts[1:]))
    assert parts[0] in bin_sets, parts[0]

# ---- case mapping
scf = {}
for f in lines("CaseFolding.txt"):
    if f[1] in ("C", "S"):
        scf[int(f[0], 16)] = int(f[2], 16)
upper_simple = {}
for f in lines("UnicodeData.txt"):
    if f[12]:
        upper_simple[int(f[0], 16)] = int(f[12], 16)
upper_full = {}
for f in lines("SpecialCasing.txt"):
    if len(f) >= 5 and f[4]:
        continue  # conditional mappings
    upper_full[int(f[0], 16)] = [int(x, 16) for x in f[3].split()]
# Canonicalize for non-unicode regexps: toUppercase if it is a single code
# unit, and never map non-ASCII to ASCII.
canon_u16 = {}
for c in range(0x10000):
    u = upper_full.get(c, [upper_simple.get(c, c)])
    if len(u) != 1 or u[0] > 0xFFFF:
        continue
    u = u[0]
    if c >= 128 and u < 128:
        continue
    if u != c:
        canon_u16[c] = u

# ---- properties of strings
seqs = {}
basic_single = set()
for fname in ["emoji-sequences.txt", "emoji-zwj-sequences.txt"]:
    for f in lines(fname):
        prop = f[1]
        if ".." in f[0]:
            a, b = parse_range(f[0])
            assert prop == "Basic_Emoji"
            basic_single.update(range(a, b + 1))
            continue
        cps = [int(x, 16) for x in f[0].split()]
        if len(cps) == 1:
            assert prop == "Basic_Emoji"
            basic_single.add(cps[0])
        else:
            seqs.setdefault(prop, []).append(cps)


# ---- output
def enc(ranges):
    out = bytearray()
    prev = 0
    for a, b in ranges:
        for v in (a - prev, b - a):
            while True:
                byte = v & 0x7F
                v >>= 7
                if v:
                    out.append(byte | 0x80)
                else:
                    out.append(byte)
                    break
        prev = b
    s = []
    for byte in out:
        if byte == 0x5C:
            s.append("\\\\")
        elif byte == 0x22:
            s.append('\\"')
        elif 0x20 <= byte < 0x7F:
            s.append(chr(byte))
        else:
            s.append("\\x%02x" % byte)
    return 'b"' + "".join(s) + '"', len(out)


total = 0
w = sys.stdout.write
w("//! Unicode %s data for RegExp property escapes and case folding.\n" % VERSION)
w("//! GENERATED by tools/unicode/gen_regex_tables.py from the UCD; do not edit.\n")
w("//! Each set is a byte string of LEB128 pairs (lo - previous hi, hi - lo).\n\n")
w("#![allow(clippy::all)]\n#![cfg_attr(rustfmt, rustfmt_skip)]\n\n")


def set_list(name, doc, keys, sets):
    global total
    w("/// %s\n" % doc)
    w("pub static %s: &[(&str, &[u8])] = &[\n" % name)
    for k in keys:
        lit, n = enc(to_ranges(sets.get(k, set())))
        total += n
        w('    ("%s", %s),\n' % (k, lit))
    w("];\n\n")


def alias_list(name, doc, pairs):
    w("/// %s\n" % doc)
    w("pub static %s: &[(&str, &str)] = &[\n" % name)
    for a, b in sorted(pairs):
        w('    ("%s", "%s"),\n' % (a, b))
    w("];\n\n")


set_list("GC", "Leaf General_Category values (short name, set), sorted by name.", sorted(gc_sets), gc_sets)
alias_list("GC_ALIASES", "General_Category value names -> short name (groups such as L are expanded by the caller).",
           gc_alias.items())
alias_list("SC_ALIASES", "Script value names -> short name.", sc_alias.items())
set_list("SC", "Script (short name, set), sorted by name.", sorted(sc_long), sc_sets)
set_list("SCX", "Script_Extensions (short name, set), sorted by name.", sorted(sc_long), scx_sets)
names = sorted(n for n, _ in binary)
set_list("BINARY", "Binary properties (canonical name, set), sorted by name.", names, bin_sets)
alias_list("BINARY_ALIASES", "Binary property names (canonical and short) -> canonical name.",
           [(n, n) for n, _ in binary] + [(a, n) for n, al in binary for a in al])


def pair_list(name, doc, ty, items):
    w("/// %s\n" % doc)
    w("pub static %s: &[(%s, %s)] = &[\n" % (name, ty, ty))
    for i in range(0, len(items), 8):
        w("    " + " ".join("(0x%X, 0x%X)," % kv for kv in items[i:i + 8]) + "\n")
    w("];\n\n")


pair_list("SCF", "Simple case folding (code point, folded), sorted.", "u32", sorted(scf.items()))
pair_list("UPPER16", "Non-unicode Canonicalize: single code unit toUppercase (unit, upper), sorted.", "u16",
          sorted(canon_u16.items()))
w("/// Basic_Emoji single code points.\n")
lit, n = enc(to_ranges(basic_single))
total += n
w("pub static BASIC_EMOJI_SINGLE: &[u8] = %s;\n\n" % lit)
w("/// Properties of strings: (name, sequences separated by ' ', each as hex code points separated by ',').\n")
w("pub static STRING_PROPS: &[(&str, &str)] = &[\n")
for prop in ["Basic_Emoji", "Emoji_Keycap_Sequence", "RGI_Emoji_Flag_Sequence", "RGI_Emoji_Modifier_Sequence",
             "RGI_Emoji_Tag_Sequence", "RGI_Emoji_ZWJ_Sequence"]:
    lst = seqs.get(prop, [])
    s = " ".join(",".join("%X" % c for c in q) for q in lst)
    total += len(s)
    w('    ("%s", "%s"),\n' % (prop, s))
w("];\n")
sys.stderr.write("table bytes: %d\n" % total)
