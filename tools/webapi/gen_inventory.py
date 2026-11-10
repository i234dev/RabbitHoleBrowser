#!/usr/bin/env python3
"""Generate tools/webapi/inventory.tsv: every interface and member exposed to
Window by the specs listed in specs.txt, from webref's parsed WebIDL.

Usage:
    python3 tools/webapi/fetch_idl.py /tmp/idl
    python3 tools/webapi/gen_inventory.py /tmp/idl > tools/webapi/inventory.tsv

Row format (tab separated):  spec  tier  interface  kind  member
kinds:
    interface  the interface object exists as a global (member = name)
    inherits   Interface.prototype's [[Prototype]] is Parent.prototype
    proto      member on Interface.prototype (or on a sample instance)
    static     member on the interface / namespace object
    const      constant on the interface object
    global     member of the Window global object
"""
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))


def ext(attrs, name):
    for a in attrs or []:
        if a["name"] == name:
            return a
    return None


def rhs_values(a):
    if a is None or a.get("rhs") is None:
        return None
    if a["rhs"].get("type") == "*":
        return ["*"]
    v = a["rhs"]["value"]
    if isinstance(v, list):
        return [x["value"] if isinstance(x, dict) else x for x in v]
    return [v]


def exposed_on_window(attrs, default):
    a = ext(attrs, "Exposed")
    if a is None:
        return default
    vals = rhs_values(a) or []
    return "Window" in vals or "*" in vals


def load(dirname):
    specs = []
    with open(os.path.join(HERE, "specs.txt")) as f:
        for line in f:
            line = line.split("#")[0].strip()
            if line:
                name, tier = line.split()[:2]
                specs.append((name, tier))
    defs = {}  # name -> list of (spec, tier, node)
    includes = []  # (target, mixin)
    for spec, tier in specs:
        path = os.path.join(dirname, spec + ".json")
        if not os.path.exists(path):
            print("missing " + path, file=sys.stderr)
            continue
        ip = json.load(open(path))["idlparsed"]
        for name, node in ip.get("idlNames", {}).items():
            defs.setdefault(name, []).append((spec, tier, node))
        for name, nodes in ip.get("idlExtendedNames", {}).items():
            for node in nodes:
                if node["type"] == "includes":
                    includes.append((node["target"], node["includes"]))
                else:
                    defs.setdefault(name, []).append((spec, tier, node))
    return defs, includes


def members_of(node):
    """Yield (kind, name) for an interface/mixin/namespace member list."""
    for m in node.get("members", []):
        t = m["type"]
        if t == "const":
            yield ("const", m["name"], m)
        elif t in ("attribute", "operation"):
            name = m.get("name")
            if not name:
                if m.get("special") == "stringifier":
                    yield ("proto", "toString", m)
                continue
            static = m.get("special") == "static"
            yield ("static" if static else "proto", name, m)
            if t == "attribute" and m.get("special") == "stringifier":
                yield ("proto", "toString", m)
        elif t == "iterable":
            if m.get("async"):
                yield ("proto", "@@asyncIterator", m)
                yield ("proto", "values", m)
            else:
                for n in ("entries", "keys", "values", "forEach", "@@iterator"):
                    yield ("proto", n, m)
        elif t in ("maplike", "setlike"):
            names = ["size", "entries", "keys", "values", "forEach", "has", "@@iterator"]
            if t == "maplike":
                names.append("get")
            if not m.get("readonly"):
                names += ["delete", "clear"] + (["set"] if t == "maplike" else ["add"])
            for n in names:
                yield ("proto", n, m)


def main():
    dirname = sys.argv[1] if len(sys.argv) > 1 else "idl"
    defs, includes = load(dirname)
    rows = []
    seen = set()

    def add(spec, tier, iface, kind, member):
        key = (iface, kind, member)
        if key in seen:
            return
        seen.add(key)
        rows.append((spec, tier, iface, kind, member))

    mixins = {}
    for name, lst in defs.items():
        for spec, tier, node in lst:
            if node["type"] == "interface mixin":
                mixins.setdefault(name, []).append((spec, tier, node))

    for name in sorted(defs):
        lst = defs[name]
        main = [x for x in lst if not x[2].get("partial") and x[2]["type"] in ("interface", "namespace", "callback interface")]
        if not main:
            continue
        mspec, mtier, mnode = main[0]
        typ = mnode["type"]
        if typ == "callback interface":
            # Only callback interfaces with constants get an interface object.
            if not any(m["type"] == "const" for m in mnode.get("members", [])):
                continue
        if not exposed_on_window(mnode.get("extAttrs"), False):
            continue
        is_global = ext(mnode.get("extAttrs"), "Global") is not None
        if is_global and name != "Window":
            continue
        if not is_global and not ext(mnode.get("extAttrs"), "LegacyNoInterfaceObject"):
            add(mspec, mtier, name, "interface", name)
            if mnode.get("inheritance"):
                add(mspec, mtier, name, "inherits", mnode["inheritance"])
        for a in mnode.get("extAttrs", []):
            if a["name"] == "LegacyFactoryFunction":
                add(mspec, mtier, name, "interface", a["rhs"]["value"])
            if a["name"] == "LegacyWindowAlias":
                for v in rhs_values(a):
                    add(mspec, mtier, name, "interface", v)
        parts = [x for x in lst if x[2]["type"] == typ]
        # Mixins included into this interface.
        for target, mixin in includes:
            if target == name:
                for x in mixins.get(mixin, []):
                    parts.append(x)
        for spec, tier, node in parts:
            node_default = True
            if node.get("partial") or node["type"] == "interface mixin":
                node_default = True
            if not exposed_on_window(node.get("extAttrs"), node_default):
                continue
            for kind, member, m in members_of(node):
                if not exposed_on_window(m.get("extAttrs"), True):
                    continue
                if typ == "namespace":
                    kind = "static"
                elif is_global:
                    kind = "global" if kind != "const" else "const"
                add(spec, tier, name, kind, member)
    rows.sort(key=lambda r: (r[0], r[2], ["interface", "inherits", "const", "static", "proto", "global"].index(r[3]), r[4]))
    print("# Generated by tools/webapi/gen_inventory.py from webref parsed WebIDL. Do not edit.")
    print("# spec\ttier\tinterface\tkind\tmember")
    for r in rows:
        print("\t".join(r))


if __name__ == "__main__":
    main()
