"""Print original functions' decompiled C for the audit, with who calls them and what they call.

    uv run scripts/audit/show.py ADDR [ADDR...] [--brief] [--callers]

Callers and callees are marked with where the port cites them (file:line), so a frontier entry of
port_audit.py reads in one step. The line index of decomp.c is cached under harness_out/audit/.
"""

import argparse
import pickle
import re
import sys
from collections import defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import port_audit as pa  # noqa: E402

CACHE = pa.REPO / "harness_out" / "audit" / "decomp_index.pkl"


def index():
    d = pa.cache_dir()
    src = d / "decomp.c"
    if CACHE.exists():
        with open(CACHE, "rb") as f:
            stamp, idx = pickle.load(f)
        if stamp == src.stat().st_mtime:
            return src, idx
    lines = src.read_text(encoding="utf-8", errors="replace").split("\n")
    spans = {}
    calls = defaultdict(set)
    i, n = 0, len(lines)
    while i < n:
        line = lines[i]
        m = pa.HEADER_RE.match(line) if line and not line.startswith((" ", "/", "{", "}")) else None
        if m and i + 2 < n and lines[i + 1] == "" and lines[i + 2] == "{":
            j = i + 3
            while j < n and lines[j] != "}":
                j += 1
            if m.group(1):
                a = int(m.group(1), 16)
                spans[a] = (i, j)
                for c in pa.FUN_RE.findall("\n".join(lines[i + 3:j])):
                    if int(c, 16) != a:
                        calls[int(c, 16)].add(a)
            i = j + 1
            continue
        i += 1
    idx = (spans, dict(calls))
    CACHE.parent.mkdir(parents=True, exist_ok=True)
    with open(CACHE, "wb") as f:
        pickle.dump((src.stat().st_mtime, idx), f)
    return src, idx


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("addr", nargs="+")
    ap.add_argument("--brief", action="store_true", help="signature, callers and callees only")
    ap.add_argument("--head", type=int, default=0, help="only the first N lines of each body (locals skipped)")
    a = ap.parse_args()
    src, (spans, callers) = index()
    cites = defaultdict(list)
    for p, ln, item, addr in pa.citations(pa.port_sources()):
        cites[addr].append(f"{p.split('/src/')[-1]}:{ln}")
    lines = None
    for s in a.addr:
        addr = int(s.lower().replace("0x", "").replace("fun_", ""), 16)
        if addr not in spans:
            print(f"{addr:08x}: not a function start")
            continue
        if lines is None:
            lines = src.read_text(encoding="utf-8", errors="replace").split("\n")
        i, j = spans[addr]
        body = lines[i:j + 1]
        tag = lambda x: f"{x:08x}" + (f" [{cites[x][0]}]" if x in cites else "")
        print(f"===== {addr:08x} ({j - i} lines)" + (f" cited at {', '.join(cites[addr][:3])}" if addr in cites else " UNCITED"))
        print("callers: " + ", ".join(tag(c) for c in sorted(callers.get(addr, ()))[:12]))
        callees = sorted({int(c, 16) for c in pa.FUN_RE.findall("\n".join(body))} - {addr})
        print("callees: " + ", ".join(tag(c) for c in callees[:20]))
        if a.head:
            # Skip the declarations: the first statement after the locals' blank line.
            k = next((t for t, l in enumerate(body) if t > 2 and l.strip() == ""), 2)
            print(body[0])
            print("\n".join(l for l in body[k + 1:k + 1 + a.head] if l.strip()))
        elif not a.brief:
            print("\n".join(body))
        print()


if __name__ == "__main__":
    main()
