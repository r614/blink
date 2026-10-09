"""Port vs original, statically: where the port's code stops following the original.

The port cites the original's functions by address next to the code that reimplements them
(`0x00AD5BA0`, `FUN_00ad5ba0`). This script joins those citations with Ghidra's decompiled C of the
whole exe (the oracle index's `decomp.c`; nothing is run) and reports, per port source file:

- frontier: functions the cited ones call directly that no port code cites: logic next to code we
  ported that nobody has read (UNREAD) or that a research doc decoded but the code never mentions
  (DOC-ONLY). Utilities called from everywhere are left out by their caller count.
- strings: identifier-like literals the cited functions use (GFx invoke names and paths, property
  and class names, img:// paths, ini keys) that appear nowhere in the port's source.
- virtual calls: vtable offsets the cited functions call through (the frontier only follows
  direct calls).

and over the whole port: citations that name no function start (typos or mid-function labels) and
functions only the research docs cite (decoded, never ported).

    uv run scripts/audit/port_audit.py [--file SUBSTR]... [--min-lines N] [--limit N] [--out FILE]

decomp.c is derived from the retail exe: it stays in the cache, and the reports (which quote its
names and strings) go to harness_out/, not to git.
"""

import argparse
import bisect
import os
import re
import sys
from collections import defaultdict
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
UTILITY_CALLERS = 120

ADDR_RE = re.compile(r"(?i)\b(?:0x|FUN_|LAB_)(00[0-9a-f]{6})\b")
FUN_RE = re.compile(r"\bFUN_([0-9a-f]{8})\b")
HEADER_RE = re.compile(r"^[A-Za-z_].*\b(?:FUN_([0-9a-f]{8})|([A-Za-z_][\w:~<>]*))\s*\(.*\)\s*$")
STR_RE = re.compile(r'L?"((?:[^"\\\n]|\\.){3,120})"')
VCALL_RE = re.compile(r"\(\*\*\(code \*\*\)\(\*[^)]*?\+ (0x[0-9a-f]+)\)\)")
ITEM_RE = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:fn\s+(\w+)|(?:impl(?:<[^>]*>)?\s+(?:[\w:<>]+\s+for\s+)?|struct\s+|enum\s+)(\w+))")
IDENT_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_.:/]{3,79}$")


def cache_dir():
    base = Path(os.environ.get("DISHONORED_CACHE", Path(os.environ["LOCALAPPDATA"]) / "dishonored-rs" / "cache"))
    idx = base / "oracle" / "index"
    dirs = sorted(d for d in idx.iterdir() if (d / "decomp.c").exists())
    if not dirs:
        sys.exit(f"no decomp.c under {idx} (run `oracle index build`)")
    return dirs[0]


def load_ledger():
    """scripts/audit/ledger.tsv: `addr<TAB>status<TAB>note` per frontier function already looked at.
    status: helper (engine or library utility), ported (reimplemented without a citation: add one),
    irrelevant (editor, network, platform), gap (real logic the port lacks: shown, flagged)."""
    out = {}
    p = REPO / "scripts" / "audit" / "ledger.tsv"
    if p.exists():
        for line in p.read_text(encoding="utf-8").split("\n"):
            parts = line.split("\t")
            if len(parts) >= 2 and not line.startswith("#"):
                try:
                    out[int(parts[0], 16)] = (parts[1].strip(), parts[2].strip() if len(parts) > 2 else "")
                except ValueError:
                    pass
    return out


def parse_decomp(path):
    """{addr: (callees, strings, vslots, lines, text)} for every FUN_ function of decomp.c."""
    funcs = {}
    cur = None
    body = []
    with open(path, encoding="utf-8", errors="replace") as f:
        lines = f.read().split("\n")
    i = 0
    n = len(lines)
    while i < n:
        line = lines[i]
        m = HEADER_RE.match(line) if line and not line.startswith((" ", "/", "{", "}")) else None
        if m and i + 2 < n and lines[i + 1] == "" and lines[i + 2] == "{":
            addr = int(m.group(1), 16) if m.group(1) else None
            j = i + 3
            while j < n and lines[j] != "}":
                j += 1
            if addr is not None:
                text = "\n".join(lines[i + 3 : j])
                callees = {int(a, 16) for a in FUN_RE.findall(text)} - {addr}
                strings = set(s for s in STR_RE.findall(text))
                vslots = VCALL_RE.findall(text)
                funcs[addr] = (callees, strings, vslots, j - i, text)
            i = j + 1
            continue
        i += 1
    return funcs


def port_sources(only_crates=None):
    skip = lambda n: n in ("ue3_package", "dishonored_oracle") or n.startswith("dishonored_probe")
    out = []
    for c in sorted((REPO / "crates").iterdir()):
        if skip(c.name) or not (c / "src").is_dir():
            continue
        for p in sorted((c / "src").rglob("*.rs")):
            t = p.read_text(encoding="utf-8", errors="replace")
            k = t.find("#[cfg(test)]")
            if k >= 0:
                t = t[:k]
            out.append((p.relative_to(REPO).as_posix(), t))
    return out


def citations(files):
    out = []
    for path, text in files:
        item = ""
        for ln, line in enumerate(text.split("\n"), 1):
            m = ITEM_RE.match(line)
            if m:
                item = m.group(1) or m.group(2) or item
            for a in ADDR_RE.findall(line):
                out.append((path, ln, item, int(a, 16)))
    return out


def doc_citations():
    out = defaultdict(list)
    for p in sorted((REPO / "research").rglob("*.md")):
        rel = p.relative_to(REPO).as_posix()
        for ln, line in enumerate(p.read_text(encoding="utf-8", errors="replace").split("\n"), 1):
            for a in ADDR_RE.findall(line):
                out[int(a, 16)].append(f"{rel}:{ln}")
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--file", action="append", default=[], help="only port files whose path contains this")
    ap.add_argument("--min-lines", type=int, default=6, help="frontier functions shorter than this (decompiled lines) are not listed")
    ap.add_argument("--limit", type=int, default=25)
    ap.add_argument("--out", default=str(REPO / "harness_out" / "audit" / "port_audit.md"))
    a = ap.parse_args()

    d = cache_dir()
    funcs = parse_decomp(d / "decomp.c")
    starts = sorted(funcs)

    def containing(addr):
        if addr in funcs:
            return addr, True
        k = bisect.bisect_right(starts, addr) - 1
        return (starts[k], False) if k >= 0 else (None, False)

    ledger = load_ledger()
    # Mechanical helpers, not logic: short bodies without identifier strings, and the template
    # array operations (their bounds assert) of moderate length.
    auto_helper = set()
    for f, (cs, ss, vs, size, text) in funcs.items():
        named = any(IDENT_RE.match(s) for s in ss if "ArrayNum" not in s)
        if (size <= 12 and not named) or ("ArrayNum" in text and size <= 40 and not named):
            auto_helper.add(f)
    for f in auto_helper:
        ledger.setdefault(f, ("helper", "auto"))
    callers = defaultdict(int)
    for f, (cs, *_rest) in funcs.items():
        for c in cs:
            callers[c] += 1

    files = port_sources()
    all_text = "\n".join(t for _, t in files)
    cites = citations(files)
    by_func = defaultdict(list)
    mid = []
    for c in cites:
        f, exact = containing(c[3])
        if f is None:
            mid.append(c)
            continue
        if not exact:
            mid.append(c)
        by_func[f].append(c)
    code_cited = set(by_func)
    docs = doc_citations()
    doc_cited = defaultdict(list)
    for addr, places in docs.items():
        f, _ = containing(addr)
        if f is not None:
            doc_cited[f] += places

    per_file = defaultdict(set)
    for f, cs in by_func.items():
        for c in cs:
            per_file[c[0]].add(f)

    in_port = {}
    def present(s):
        if s not in in_port:
            in_port[s] = s in all_text
        return in_port[s]

    out = []
    tot_unread = tot_doc = tot_str = 0
    sections = []
    for path in sorted(per_file):
        if a.file and not any(w in path for w in a.file):
            continue
        fs = per_file[path]
        frontier = defaultdict(set)
        vslots = defaultdict(int)
        for f in fs:
            cs, _, vs, _, _ = funcs[f]
            for t in cs:
                if t in code_cited or t not in funcs or callers[t] > UTILITY_CALLERS:
                    continue
                if ledger.get(t, ("",))[0] in ("helper", "ported", "irrelevant"):
                    continue
                frontier[t].add(f)
            for v in vs:
                vslots[v] += 1
        rows = [(t, funcs[t][3], t in doc_cited, sorted(fr)) for t, fr in frontier.items() if funcs[t][3] >= a.min_lines]
        rows.sort(key=lambda r: -r[1])
        unread = sum(1 for r in rows if not r[2])
        tot_unread += unread
        tot_doc += len(rows) - unread
        missing = defaultdict(set)
        for f in fs:
            for s in funcs[f][1]:
                if IDENT_RE.match(s) and not present(s):
                    missing[s].add(f)
        tot_str += len(missing)
        item_of = {}
        for f in fs:
            for c in by_func[f]:
                if c[0] == path:
                    item_of.setdefault(f, c[2])
        lines = [f"## {path}", "", f"{len(fs)} cited functions; frontier: {unread} unread, {len(rows) - unread} decoded in the docs only; {len(missing)} strings not in the port; {len(vslots)} virtual offsets called", ""]
        for t, size, doc, fr in rows[: a.limit]:
            tag = "GAP" if ledger.get(t, ("",))[0] == "gap" else "doc-only" if doc else "UNREAD"
            src = ", ".join(f"{f:08x} ({item_of.get(f, '')})" for f in fr[:3])
            extra = f" [{', '.join(doc_cited[t][:2])}]" if doc else ""
            note = f" -- {ledger[t][1]}" if t in ledger and ledger[t][1] else ""
            lines.append(f"- {tag} {t:08x} ({size} lines), called from {src}{extra}{note}")
            # What the cited caller does with it, and what it touches.
            site = next((l.strip() for l in funcs[fr[0]][4].split("\n") if f"FUN_{t:08x}" in l), "")
            if site:
                lines.append(f"    call: `{site[:160]}`")
            strs = sorted(s for s in funcs[t][1] if len(s) < 60)[:4]
            sub = sorted(c for c in funcs[t][0] if c in code_cited)[:4]
            info = []
            if strs:
                info.append("strings " + ", ".join(f'"{s}"' for s in strs))
            if sub:
                info.append("calls cited " + ", ".join(f"{c:08x}" for c in sub))
            if info:
                lines.append("    " + "; ".join(info))
        if len(rows) > a.limit:
            lines.append(f"- ... {len(rows) - a.limit} more")
        if missing:
            lines += ["", "strings the cited functions use, absent from the port:"]
            for s, fr in sorted(missing.items())[: a.limit]:
                lines.append(f'- "{s}" in {", ".join(f"{f:08x}" for f in sorted(fr)[:3])}')
            if len(missing) > a.limit:
                lines.append(f"- ... {len(missing) - a.limit} more")
        lines.append("")
        sections.append("\n".join(lines))

    # Frontier functions shared by many port files: the first ledger candidates (helpers).
    shared = defaultdict(set)
    for path, fs in per_file.items():
        for f in fs:
            for t in funcs[f][0]:
                if t not in code_cited and t in funcs and callers[t] <= UTILITY_CALLERS and t not in ledger:
                    shared[t].add(path)
    shared_rows = sorted(((t, len(p), funcs[t][3]) for t, p in shared.items() if len(p) >= 3), key=lambda r: (-r[1], -r[2]))

    head = ["# Port vs original: static audit", "",
            f"{len(funcs)} decompiled functions ({len(auto_helper)} mechanical helpers left out); {len(files)} port source files with {len(cites)} citations of {len(code_cited)} functions; research docs cite {len(doc_cited)} functions.",
            f"Frontier over the listed files: {tot_unread} unread, {tot_doc} decoded in the docs only (direct callees of >= {a.min_lines} lines; utilities with > {UTILITY_CALLERS} callers left out); {tot_str} strings absent from the port.", ""]
    if not a.file:
        head += [f"## Citations that are not a function start ({len(mid)})", ""]
        for p, ln, item, addr in mid[:200]:
            f, _ = containing(addr)
            head.append(f"- {p}:{ln} ({item}) cites {addr:08x}" + (f", inside {f:08x}" if f is not None else ", no function"))
        head.append("")
        doc_only = sorted(((f, funcs[f][3], pl) for f, pl in doc_cited.items() if f not in code_cited and f in funcs and funcs[f][3] >= a.min_lines), key=lambda r: -r[1])
        head += [f"## Decoded in the research docs, cited nowhere in the code ({len(doc_only)})", ""]
        for f, size, pl in doc_only[: a.limit * 6]:
            head.append(f"- {f:08x} ({size} lines) [{', '.join(pl[:2])}]")
        if len(doc_only) > a.limit * 6:
            head.append(f"- ... {len(doc_only) - a.limit * 6} more")
        head.append("")
        head += [f"## Frontier functions shared by 3+ port files ({len(shared_rows)}): classify these first in ledger.tsv", ""]
        for t, nfiles, size in shared_rows[: a.limit * 4]:
            strs = sorted(s for s in funcs[t][1] if len(s) < 50)[:3]
            head.append(f"- {t:08x} ({size} lines, {callers[t]} callers) in {nfiles} files" + (f"; strings {', '.join(repr(s) for s in strs)}" if strs else ""))
        head.append("")
    report = "\n".join(head) + "\n" + "\n".join(sections)
    Path(a.out).parent.mkdir(parents=True, exist_ok=True)
    Path(a.out).write_text(report, encoding="utf-8")
    print("\n".join(head[:4]))
    print(f"wrote {a.out}")


if __name__ == "__main__":
    main()
