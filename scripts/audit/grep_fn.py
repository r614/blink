"""Grep the decompiled C and name the function each match is in.

    uv run scripts/audit/grep_fn.py REGEX [--from ADDR] [--to ADDR] [--context N]
"""

import argparse
import bisect
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import show  # noqa: E402


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("regex")
    ap.add_argument("--from", dest="lo", default="0")
    ap.add_argument("--to", dest="hi", default="ffffffff")
    ap.add_argument("--context", type=int, default=0)
    a = ap.parse_args()
    src, (spans, _) = show.index()
    lo, hi = int(a.lo, 16), int(a.hi, 16)
    order = sorted((s, e, addr) for addr, (s, e) in spans.items())
    starts = [o[0] for o in order]
    rx = re.compile(a.regex)
    lines = src.read_text(encoding="utf-8", errors="replace").split("\n")
    for k, line in enumerate(lines):
        if not rx.search(line):
            continue
        i = bisect.bisect_right(starts, k) - 1
        if i < 0:
            continue
        s, e, addr = order[i]
        if not (s <= k <= e) or not (lo <= addr <= hi):
            continue
        print(f"{addr:08x}:{k - s}: {line.strip()}")
        for c in range(1, a.context + 1):
            if k + c <= e:
                print(f"           {lines[k + c].strip()}")


if __name__ == "__main__":
    main()
