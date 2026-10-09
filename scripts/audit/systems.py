"""Which of the original's systems neither the port nor the research covers, by size.

Static: only the dumped decompile (the oracle index's `decomp.c`), the retail exe's bytes, the
port's source and the research docs are read; no program is launched.

Each function of the game module (from 0x009E0000) is covered by the port (cited in
crates/*/src), by the audit ledger, by the research (cited in research/*.md only), or by nothing.

`--by class` (default): every function is given to the native class that owns it. A class owns
its vtable's methods (the most basic class whose vtable holds the function; see exe_classes.py),
its natives (Ghidra's `A<Class>exec<Function>` thunks call them) and its constructor, and every
function only its own code reaches by direct calls (two classes reaching one function share it;
calls through vtables are not followed). Classes are ranked by the lines nothing covers.

`--by stretch`: the linker keeps one source file's functions together, so stretches of uncovered
functions (a cited function, or more than `--gap` covered lines, ends one) are ranked instead.

    uv run --no-project scripts/audit/systems.py [--by class|stretch] [--top N] [--fns N]
        [--class NAME]... [--at ADDR]... [--gap LINES] [--end ADDR] [--out FILE]

The report quotes the original's names and goes to harness_out/, not to git.
"""

import argparse
import pickle
import re
import sys
from collections import Counter, defaultdict, deque
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from exe_classes import VT_RE, load_classes  # noqa: E402
from port_audit import (  # noqa: E402
    ENGINE_END,
    REPO,
    UTILITY_CALLERS,
    cache_dir,
    citations,
    doc_citations,
    load_ledger,
    parse_decomp,
    port_sources,
)

OUT_DIR = REPO / "harness_out" / "audit"
NAME_RE = re.compile(r"^(?:m_|Dis|eDis|Twk_|_root|Attribute_|DishonoredGame\.)[A-Za-z0-9_.]{2,}$")
EXEC_RE = re.compile(r"^\S.*\b[AU]([A-Za-z0-9_]+?)exec([A-Za-z0-9_]+)\s*\(.*\)\s*$")
FUN_RE = re.compile(r"\bFUN_([0-9a-f]{8})\b")


def natives(lines):
    """{addr: ["Class.Function", ...]}: what a native's exec thunk (`A<Class>exec<Function>`) calls."""
    out = defaultdict(list)
    i, n = 0, len(lines)
    while i < n:
        m = EXEC_RE.match(lines[i]) if lines[i][:1].isalpha() else None
        if m and i + 2 < n and lines[i + 2] == "{":
            j = i + 3
            while j < n and lines[j] != "}":
                j += 1
            for a in set(FUN_RE.findall("\n".join(lines[i + 3 : j]))):
                out[int(a, 16)].append(f"{m.group(1)}.{m.group(2)}")
            i = j + 1
            continue
        i += 1
    return dict(out)


def load():
    """(funcs {addr: (callees, strings, lines)}, natives, classes), cached next to the report."""
    pkl = OUT_DIR / "systems_cache.pkl"
    src = cache_dir() / "decomp.c"
    if pkl.exists() and pkl.stat().st_mtime >= src.stat().st_mtime and pkl.stat().st_mtime >= Path(__file__).with_name("exe_classes.py").stat().st_mtime:
        with open(pkl, "rb") as f:
            return pickle.load(f)
    parsed = parse_decomp(src)
    funcs = {a: (c, s, n) for a, (c, s, _v, n, _t) in parsed.items()}
    vts = {}
    for a, (_c, _s, _v, _n, t) in parsed.items():
        # Every store in order: an inlined base constructor stores its own vtable first.
        m = VT_RE.findall(t)
        if m:
            vts[a] = [int(x, 16) for x in m]
    with open(src, encoding="utf-8", errors="replace") as f:
        nat = natives(f.read().split("\n"))
    data = (funcs, nat, load_classes(funcs, vts))
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    with open(pkl, "wb") as f:
        pickle.dump(data, f)
    return data


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--by", choices=("class", "stretch"), default="class")
    ap.add_argument("--top", type=int, default=50, help="classes / stretches listed")
    ap.add_argument("--fns", type=int, default=10, help="largest uncovered functions shown per entry")
    ap.add_argument("--class", dest="classes", action="append", default=[], help="only these classes")
    ap.add_argument("--at", action="append", default=[], help="only the stretch holding this address (hex)")
    ap.add_argument("--gap", type=int, default=40, help="covered lines a stretch may span")
    ap.add_argument("--end", default="00d20000", help="end of the game module (hex)")
    ap.add_argument("--out", default=None)
    args = ap.parse_args()
    end = int(args.end, 16)
    out_path = Path(args.out or OUT_DIR / f"systems_{args.by}.md")

    funcs, native, classes = load()
    callers = defaultdict(set)
    for a, (callees, _s, _n) in funcs.items():
        for c in callees:
            callers[c].add(a)
    cites = citations(port_sources())
    code = {a for (_p, _l, _i, a) in cites}
    cite_file = defaultdict(Counter)
    for p, _l, _i, a in cites:
        cite_file[a][p.split("/src/")[-1] if "/src/" in p else p] += 1
    docs = doc_citations()
    ledger = load_ledger()

    def cover(a):
        if a in code:
            return "port"
        st = ledger.get(a, ("", ""))[0]
        if st == "gap":
            return "gap"
        if st:
            return "ledger"
        if a in docs:
            return "docs"
        return "none"

    def in_game(a):
        return ENGINE_END <= a < end and a in funcs

    def is_util(a):
        return len(callers[a]) > UTILITY_CALLERS

    game = sorted(a for a in funcs if ENGINE_END <= a < end)
    groups = {}  # name -> [addr]

    if args.by == "class":
        holders = defaultdict(list)
        for name, c in classes.items():
            for s in c["slots"]:
                if in_game(s):
                    holders[s].append(name)
        seed_owner = {}
        for a, names in holders.items():
            best = min(len(classes[n]["slots"]) for n in names)
            base = sorted({n for n in names if len(classes[n]["slots"]) == best})
            seed_owner[a] = base[0] if len(base) == 1 else "(shared vtable)"
        for a, qs in native.items():
            if in_game(a):
                cls = {q.split(".")[0] for q in qs}
                if len(cls) == 1:
                    seed_owner.setdefault(a, next(iter(cls)))
        for name, c in classes.items():
            for a in (c["ctor"], c["ictor"]):
                if a and in_game(a):
                    seed_owner.setdefault(a, name)
        seeds = defaultdict(list)
        for a, n in seed_owner.items():
            seeds[n].append(a)
        reached = defaultdict(set)
        for name, ss in seeds.items():
            seen = set(ss)
            q = deque(ss)
            while q:
                a = q.popleft()
                for b in funcs[a][0]:
                    if b in seen or not in_game(b) or is_util(b) or b in seed_owner:
                        continue
                    seen.add(b)
                    q.append(b)
            for a in seen:
                reached[a].add(name)
        for a in game:
            if a in seed_owner:
                o = seed_owner[a]
            elif len(reached.get(a, ())) == 1:
                o = next(iter(reached[a]))
            elif reached.get(a):
                o = "(shared)"
            else:
                o = "(unreached)"
            groups.setdefault(o, []).append(a)
    else:
        cur, covered_run = [], 0
        for a in game:
            k = cover(a)
            if k in ("none", "gap"):
                cur.append(a)
                covered_run = 0
            else:
                covered_run += funcs[a][2]
                if k == "port" or covered_run > args.gap:
                    if cur:
                        groups[f"{cur[0]:08x}-{cur[-1]:08x}"] = cur
                    cur, covered_run = [], 0
        if cur:
            groups[f"{cur[0]:08x}-{cur[-1]:08x}"] = cur
        if args.at:
            want = [int(x, 16) for x in args.at]
            groups = {k: v for k, v in groups.items() if any(v[0] <= w <= v[-1] for w in want)}

    def stat(fs):
        s = Counter()
        for a in fs:
            n = funcs[a][2] if not is_util(a) else 0
            s[cover(a)] += n
            s["total"] += n
        return s

    def uncovered(fs):
        s = stat(fs)
        return s["none"] + s["gap"]

    order = sorted((g for g in groups if not g.startswith("(")), key=lambda g: -uncovered(groups[g]))
    if args.classes:
        wanted = {c.lower() for c in args.classes}
        order = [g for g in order if g.lower() in wanted]
    order = order[: args.top] if not args.classes else order

    tot = stat(game)
    lines = [
        f"# Game module by {args.by}, ranked by uncovered lines",
        "",
        f"0x{ENGINE_END:08X}-0x{end:08X}: {len(game)} functions ({len(classes)} native classes read from the exe). "
        f"Lines (utilities with more than {UTILITY_CALLERS} callers left out): port {tot['port']}, ledger {tot['ledger']}, "
        f"docs only {tot['docs']}, gaps {tot['gap']}, nothing {tot['none']}.",
    ]
    for g in ("(shared)", "(shared vtable)", "(unreached)"):
        if g in groups:
            s = stat(groups[g])
            lines.append(f"{g}: {len(groups[g])} functions, {s['total']} lines, uncovered {s['none'] + s['gap']}.")
    lines += ["", "| # | " + args.by + " | fns | lines | port | docs | ledger | uncovered | covered % | port files |", "|---|---|---|---|---|---|---|---|---|---|"]
    for i, g in enumerate(order, 1):
        fs = groups[g]
        s = stat(fs)
        files = Counter()
        for a in fs:
            files.update(cite_file.get(a, Counter()))
        pct = 100.0 * (s["total"] - s["none"] - s["gap"]) / max(1, s["total"])
        lines.append(f"| {i} | {g} | {len(fs)} | {s['total']} | {s['port']} | {s['docs']} | {s['ledger']} | {s['none'] + s['gap']} | {pct:.0f} | {', '.join(f for f, _ in files.most_common(3))} |")
    lines.append("")
    for i, g in enumerate(order, 1):
        fs = groups[g]
        inside = set(fs)
        head = f"## {i}. {g}"
        if g in classes:
            c = classes[g]
            head += f" (size 0x{c['size']:x}, {len(c['slots'])} vtable slots, vtable {c['vtable'] or 0:08x}, ctor {c['ctor'] or 0:08x})"
        lines.append(head)
        nat = sorted({q for a in fs for q in native.get(a, [])})
        if nat:
            lines.append("natives: " + ", ".join(nat[:30]) + (" ..." if len(nat) > 30 else ""))
        names = Counter(t for a in fs for t in funcs[a][1] if NAME_RE.match(t))
        if names:
            lines.append("names: " + ", ".join(n for n, _ in names.most_common(20)))
        entries = Counter(c for a in fs for c in callers[a] if c not in inside)
        if entries:
            ent = sorted(entries, key=lambda c: (cover(c) != "port", -entries[c]))[:8]
            lines.append("called from outside: " + ", ".join(f"{c:08x} [{cover(c)}]" for c in ent))
        for a in sorted((a for a in fs if cover(a) in ("none", "gap", "docs")), key=lambda a: -funcs[a][2])[: args.fns]:
            strs = sorted(t for t in funcs[a][1] if NAME_RE.match(t))[:6]
            note = ledger.get(a, ("", ""))[1]
            lines.append(f"- {a:08x} {funcs[a][2]} lines [{cover(a)}], {len(callers[a])} callers" + (f"; {', '.join(strs)}" if strs else "") + (f" -- {note}" if note else ""))
        lines.append("")
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print("\n".join(lines[: 8 + len(order)]))
    print(f"\nwritten {out_path}")


if __name__ == "__main__":
    main()
