"""Who posts which AI stimulus (`EAIStimID`), statically from decomp.c.

A stimulus is built from a template whose constructor stores its type in the byte at +4
(`*(undefined1 *)(param_1 + 1) = 0xNN;`) next to its vtable; the template's callers are the places
that post it. Each poster is shown with its coverage (port / ledger / docs / nothing) and the
port files citing it. The names come from `DishonoredAIBehavior.EAIStimID` (enum_dump).

    uv run --no-project scripts/audit/stimuli.py [--id N]... [--out FILE]

Report to harness_out/ (it quotes the original's names).
"""

import argparse
import re
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from port_audit import REPO, cache_dir, citations, doc_citations, load_ledger, port_sources  # noqa: E402
from systems import load  # noqa: E402

TYPE_RE = re.compile(r"\*\(undefined1 \*\)\(param_1 \+ 1\) = (0x[0-9a-f]+|\d+);")
VT_RE = re.compile(r"\*param_1 = &PTR_FUN_(011[0-9a-f]{5});")


def stim_names():
    pkg = Path(r"C:\Program Files\GOG Galaxy\Games\Dishonored\DishonoredGame\CookedPCConsole\DishonoredGame.upk")
    exe = REPO / "target" / "release" / "enum_dump.exe"
    names = {}
    if exe.exists() and pkg.exists():
        out = subprocess.run([str(exe), str(pkg), "EAIStimID"], capture_output=True, text=True).stdout
        for line in out.split("\n"):
            m = re.match(r"\s+(\d+) EAIStimID_(\w+)", line)
            if m:
                names[int(m.group(1))] = m.group(2)
    return names


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--id", type=int, action="append", default=[])
    ap.add_argument("--out", default=str(REPO / "harness_out" / "audit" / "stimuli.md"))
    args = ap.parse_args()
    funcs, _native, _classes = load()
    callers = defaultdict(set)
    for a, (callees, _s, _n) in funcs.items():
        for c in callees:
            callers[c].add(a)
    # Template constructors: scan decomp.c once for the type store in a short function.
    ctors = defaultdict(list)
    cur, body = None, []
    with open(cache_dir() / "decomp.c", encoding="utf-8", errors="replace") as f:
        for line in f:
            m = re.match(r"^\S.*\bFUN_([0-9a-f]{8})\s*\(", line)
            if m:
                cur, body = int(m.group(1), 16), []
                continue
            if cur is None:
                continue
            if line.startswith("}"):
                text = "".join(body)
                t, v = TYPE_RE.search(text), VT_RE.search(text)
                if t and v and len(body) < 120:
                    ctors[int(t.group(1), 0)].append(cur)
                cur = None
                continue
            body.append(line)
    code = defaultdict(set)
    for p, ln, _i, a in citations(port_sources()):
        code[a].add(f"{p.split('/src/')[-1]}:{ln}")
    docs = doc_citations()
    ledger = load_ledger()
    names = stim_names()

    def cov(a):
        if a in code:
            return "port " + ", ".join(sorted(code[a])[:2])
        if a in ledger:
            return f"ledger {ledger[a][0]}"
        if a in docs:
            return "docs " + docs[a][0]
        return "-"

    lines = ["# AI stimuli and their posters", ""]
    for sid in sorted(set(ctors) | set(names)):
        if args.id and sid not in args.id:
            continue
        cs = ctors.get(sid, [])
        posters = sorted({c for k in cs for c in callers[k]})
        lines.append(f"## {sid} {names.get(sid, '?')}: template {', '.join(f'{c:08x}' for c in cs) or 'not found'}")
        for p in posters:
            lines.append(f"- {p:08x} ({funcs[p][2] if p in funcs else 0} lines) {cov(p)}")
        lines.append("")
    Path(args.out).parent.mkdir(parents=True, exist_ok=True)
    Path(args.out).write_text("\n".join(lines) + "\n", encoding="utf-8")
    print("\n".join(lines))


if __name__ == "__main__":
    main()
