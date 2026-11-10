#!/usr/bin/env python3
"""Download the parsed WebIDL (webref "idlparsed" JSON) of the specs that
make up RabbitHole's Web API inventory.

Usage: python3 tools/webapi/fetch_idl.py <out_dir>

The spec list lives in specs.txt next to this script. Run gen_inventory.py
afterwards to regenerate inventory.tsv.
"""
import os
import sys
import urllib.request

BASE = "https://raw.githubusercontent.com/w3c/webref/curated/ed/idlparsed/"


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else "idl"
    os.makedirs(out, exist_ok=True)
    here = os.path.dirname(os.path.abspath(__file__))
    with open(os.path.join(here, "specs.txt")) as f:
        specs = [l.split("#")[0].split()[0] for l in f if l.split("#")[0].strip()]
    for s in specs:
        path = os.path.join(out, s + ".json")
        if os.path.exists(path):
            continue
        try:
            with urllib.request.urlopen(BASE + s + ".json") as r:
                data = r.read()
        except Exception as e:
            print("FAILED", s, e)
            continue
        with open(path, "wb") as f:
            f.write(data)
        print("fetched", s, len(data))


if __name__ == "__main__":
    main()
