"""Name the class behind a `StaticClass()` getter or a class global, statically.

The game reaches a class through a getter (`if (DAT_x == 0) DAT_x = FUN_00479370(&STRUCT)`) or by
reading the global it fills; the static class struct's name is in the exe (exe_classes.py).

    uv run --no-project scripts/audit/static_class.py ADDR...   (a getter, a DAT_ global or a struct)
"""

import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from exe_classes import Exe, static_classes  # noqa: E402
from port_audit import cache_dir  # noqa: E402

INIT_RE = re.compile(r"(DAT_[0-9a-f]{8}) = FUN_00479370\(&DAT_([0-9a-f]{8})\);")


def main():
    exe = Exe()
    by_struct = {s: (n, p, size) for s, n, p, size, _i in static_classes(exe)}
    glob_to_struct = {}
    getter_to_struct = {}
    cur = None
    with open(cache_dir() / "decomp.c", encoding="utf-8", errors="replace") as f:
        for line in f:
            m = re.match(r"^\S.*\bFUN_([0-9a-f]{8})\s*\(", line)
            if m:
                cur = int(m.group(1), 16)
                continue
            m = INIT_RE.search(line)
            if m:
                s = int(m.group(2), 16)
                glob_to_struct[int(m.group(1)[4:], 16)] = s
                if cur is not None:
                    getter_to_struct.setdefault(cur, s)
    for x in sys.argv[1:]:
        a = int(x.replace("DAT_", "").replace("FUN_", ""), 16)
        s = by_struct.get(a) and a or glob_to_struct.get(a) or getter_to_struct.get(a)
        if s and s in by_struct:
            n, p, size = by_struct[s]
            print(f"{x}: {p}.{n} (size 0x{size:x}, struct {s:08x})")
        else:
            print(f"{x}: not a known class getter / global / struct")


if __name__ == "__main__":
    main()
