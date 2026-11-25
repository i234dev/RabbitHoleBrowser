#!/usr/bin/env python3
"""Generate crates/js/data/tzdb.bin, the embedded IANA time zone database.

The JavaScript engine reads the system zoneinfo directory when one exists
(Linux, macOS, BSD). This compact copy is the fallback for hosts without one
(Windows, minimal containers): every Zone and Link of the tz database plus the
CLDR mapping from Windows time zone key names to IANA identifiers.

Inputs:
  --tzdata   tzdata.zi of the release to embed (default: the system's)
  --windows  CLDR common/supplemental/windowsZones.xml
Requires `zic` (to compile the rules into slim TZif data).

Format (all integers are LEB128 varints, signed ones zigzag-encoded;
strings are a varint length followed by UTF-8 bytes):
  "RHTZ" u8(version=1) str(tzdata version)
  n str*n                                  abbreviation table
  n zone*n                                 zones, sorted by name
    zone = str(name) u8(ntypes) type*ntypes u8(initial type)
           n sint(first transition) uint(delta)*(n-1) u8(type index)*n
           str(POSIX TZ footer)
    type = sint(UTC offset seconds) uint(abbreviation index) u8(is DST)
  n (str(link name) uint(zone index))*n    links, sorted by name
  n (str(windows key) str(iana name))*n    Windows key names, sorted
"""
from __future__ import annotations

import argparse
import re
import struct
import subprocess
import sys
import tempfile
from pathlib import Path


def uvar(n: int) -> bytes:
    assert n >= 0
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        if n:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


def svar(n: int) -> bytes:
    return uvar((n << 1) if n >= 0 else ((-n << 1) - 1))


def s(text: str) -> bytes:
    b = text.encode()
    return uvar(len(b)) + b


def parse_tzif(data: bytes):
    """(types [(off, isdst, abbr)], times, idx, footer) from the v2+ block."""
    assert data[:4] == b"TZif" and data[4] >= ord("2"), "need TZif v2+"

    def counts(h):
        return struct.unpack(">6l", data[h + 20:h + 44])

    c1 = counts(0)
    isut, isstd, leap, timecnt, typecnt, charcnt = c1
    v1 = timecnt * 5 + typecnt * 6 + charcnt + leap * 8 + isstd + isut
    h = 44 + v1
    assert data[h:h + 4] == b"TZif"
    isut, isstd, leap, timecnt, typecnt, charcnt = counts(h)
    p = h + 44
    times = list(struct.unpack(">%dq" % timecnt, data[p:p + 8 * timecnt]))
    p += 8 * timecnt
    idx = list(data[p:p + timecnt])
    p += timecnt
    raw = []
    for i in range(typecnt):
        off, dst, ai = struct.unpack(">lBB", data[p + 6 * i:p + 6 * i + 6])
        raw.append((off, dst, ai))
    p += 6 * typecnt
    chars = data[p:p + charcnt]
    p += charcnt + leap * 12 + isstd + isut
    types = []
    for off, dst, ai in raw:
        end = chars.index(b"\0", ai)
        types.append((off, dst, chars[ai:end].decode()))
    footer = data[p:].decode().strip("\n\r")
    return types, times, idx, footer


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--tzdata", default="/usr/share/zoneinfo/tzdata.zi")
    ap.add_argument("--windows", required=True)
    ap.add_argument("--out", default=str(Path(__file__).resolve().parent.parent / "crates/js/data/tzdb.bin"))
    a = ap.parse_args()

    src = Path(a.tzdata).read_text()
    m = re.search(r"^# version (\S+)", src, re.M)
    version = m.group(1) if m else "unknown"
    zones, links = [], {}
    for line in src.splitlines():
        f = line.split()
        if f and f[0] == "Z":
            zones.append(f[1])
        elif f and f[0] == "L":
            links[f[2]] = f[1]
    zones.sort()

    with tempfile.TemporaryDirectory() as tmp:
        subprocess.run(["zic", "-b", "slim", "-d", tmp, a.tzdata], check=True)
        parsed = {z: parse_tzif((Path(tmp) / z).read_bytes()) for z in zones}

    abbrs: dict[str, int] = {}
    body = bytearray()
    for z in zones:
        types, times, idx, footer = parsed[z]
        initial = next((i for i, t in enumerate(types) if not t[1]), 0)
        body += s(z) + bytes([len(types)])
        for off, dst, ab in types:
            body += svar(off) + uvar(abbrs.setdefault(ab, len(abbrs))) + bytes([dst])
        body += bytes([initial]) + uvar(len(times))
        prev = None
        for t in times:
            body += svar(t) if prev is None else uvar(t - prev)
            prev = t
        body += bytes(idx) + s(footer)

    zindex = {z: i for i, z in enumerate(zones)}

    def resolve(name):
        seen = 0
        while name in links and seen < 8:
            name, seen = links[name], seen + 1
        return name

    out = bytearray(b"RHTZ\x01") + s(version)
    order = sorted(abbrs, key=abbrs.get)
    out += uvar(len(order))
    for ab in order:
        out += s(ab)
    out += uvar(len(zones)) + body
    link_rows = sorted((l, zindex[resolve(t)]) for l, t in links.items() if resolve(t) in zindex)
    out += uvar(len(link_rows))
    for l, i in link_rows:
        out += s(l) + uvar(i)

    xml = Path(a.windows).read_text()
    win = {}
    for other, typ in re.findall(r'<mapZone other="([^"]+)" territory="001" type="([^"]+)"', xml):
        win[other] = typ.split()[0]
    for k, v in win.items():
        if v not in zindex and v not in links:
            print(f"warning: {k} maps to unknown {v}", file=sys.stderr)
    out += uvar(len(win))
    for k in sorted(win):
        out += s(k) + s(win[k])

    Path(a.out).parent.mkdir(parents=True, exist_ok=True)
    Path(a.out).write_bytes(out)
    print(f"{a.out}: tzdata {version}, {len(zones)} zones, {len(link_rows)} links, "
          f"{len(win)} Windows names, {len(out)} bytes")
    return 0


if __name__ == "__main__":
    sys.exit(main())
