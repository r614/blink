"""A native class's vtable from the exe: each slot's function, the class that introduced it, its
size and whether the port, the ledger or the research cites it. Static (exe bytes + decomp.c).

    uv run --no-project scripts/audit/vtable.py CLASS [--all] [--find ADDR]

Without --all only the slots the class itself defines (differing from its parent's) are listed.
The parent comes from the constructor (exe_classes.py); failing that, the class with fewer slots
sharing the most of this class's slot functions.
--find lists the classes whose vtable holds ADDR (and at which slot).
"""

import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from port_audit import citations, doc_citations, load_ledger, port_sources  # noqa: E402
from systems import load  # noqa: E402


def parent_of(classes, name):
    me = classes[name]["slots"]
    best, score = None, -1
    for n, c in classes.items():
        s = c["slots"]
        if n == name or not s or len(s) > len(me):
            continue
        same = sum(1 for a, b in zip(s, me) if a == b)
        if same > score or (same == score and best and len(s) > len(classes[best]["slots"])):
            best, score = n, same
    return best


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("cls", nargs="?")
    ap.add_argument("--all", action="store_true")
    ap.add_argument("--find")
    args = ap.parse_args()
    funcs, native, classes = load()
    if args.find:
        a = int(args.find, 16)
        for n, c in sorted(classes.items(), key=lambda kv: len(kv[1]["slots"])):
            for i, s in enumerate(c["slots"]):
                if s == a:
                    print(f"{n}: slot {i} (+0x{i * 4:x}) of {len(c['slots'])}")
        return
    name = next((n for n in classes if n.lower() == args.cls.lower()), None)
    if not name:
        sys.exit(f"no class {args.cls}")
    c = classes[name]
    code = {x for (_p, _l, _i, x) in citations(port_sources())}
    cite_at = {}
    for p, ln, _i, x in citations(port_sources()):
        cite_at.setdefault(x, f"{p.split('/src/')[-1]}:{ln}")
    docs = doc_citations()
    ledger = load_ledger()
    par = c.get("parent") or parent_of(classes, name)
    ps = classes[par]["slots"] if par else []
    print(f"{name} ({c['package']}): size 0x{c['size']:x}, vtable {c['vtable'] or 0:08x}, {len(c['slots'])} slots, ctor {c['ctor'] or 0:08x}; parent {par} ({len(ps)} slots)")
    for i, a in enumerate(c["slots"]):
        own = i >= len(ps) or ps[i] != a
        if not own and not args.all:
            continue
        f = funcs.get(a)
        n = f[2] if f else 0
        if a in code:
            cov = "port " + cite_at[a]
        elif a in ledger:
            cov = "ledger " + ledger[a][0]
        elif a in docs:
            cov = "docs " + docs[a][0]
        else:
            cov = "-"
        kind = "new" if i >= len(ps) else ("override" if own else "inherited")
        strs = sorted(s for s in (f[1] if f else ()) if s.startswith(("m_", "Dis", "eDis")))[:4]
        nat = native.get(a, [])
        print(f"  {i:3} +0x{i * 4:03x} {a:08x} {kind:9} {n:4} lines  {cov}" + (f"  native {','.join(nat)}" if nat else "") + (f"  {' '.join(strs)}" if strs else ""))


if __name__ == "__main__":
    main()
