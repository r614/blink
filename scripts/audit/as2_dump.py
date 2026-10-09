"""Decompile the ActionScript 2 (AVM1) of a Scaleform movie into readable pseudo-code.

The HUD and menu widgets are hand-rewritten in Rust from the movies' ActionScript classes; this
dump is the original side of that comparison: every class method (`DoInitAction` blocks, named
by the movie's exports), frame script and button handler, with constant-pool names resolved,
registers named after their parameters, and branches as labelled gotos. It also lists the
movie's exports and the external images it references (GFx `DefineExternalImage`).

    target/release/swf_dump <Package> <movie filter> <out.gfx>      # the movie's raw bytes
    uv run scripts/audit/as2_dump.py <movie.gfx> [--grep REGEX] [--out FILE]

The output quotes retail code: keep it under harness_out/, never in git.
"""

import argparse
import re
import struct
import sys
from pathlib import Path

PROPS = ["_x", "_y", "_xscale", "_yscale", "_currentframe", "_totalframes", "_alpha", "_visible", "_width", "_height",
         "_rotation", "_target", "_framesloaded", "_name", "_droptarget", "_url", "_highquality", "_focusrect",
         "_soundbuftime", "_quality", "_xmouse", "_ymouse"]

SIMPLE = {0x04: "nextFrame()", 0x05: "prevFrame()", 0x06: "play()", 0x07: "stop()", 0x08: "toggleQuality()", 0x09: "stopSounds()"}
BINOP = {0x0A: "+", 0x0B: "-", 0x0C: "*", 0x0D: "/", 0x0E: "==", 0x0F: "<", 0x10: "&&", 0x11: "||", 0x13: "eq",
         0x21: "add", 0x29: "lt", 0x47: "+", 0x48: "<", 0x49: "==", 0x3F: "%", 0x60: "&", 0x61: "|", 0x62: "^",
         0x63: "<<", 0x64: ">>", 0x65: ">>>", 0x66: "===", 0x67: ">", 0x68: "gt", 0x54: "instanceof"}


class Reader:
    def __init__(self, b, o=0):
        self.b, self.o = b, o

    def u8(self):
        v = self.b[self.o]; self.o += 1; return v

    def u16(self):
        v = struct.unpack_from("<H", self.b, self.o)[0]; self.o += 2; return v

    def s16(self):
        v = struct.unpack_from("<h", self.b, self.o)[0]; self.o += 2; return v

    def u32(self):
        v = struct.unpack_from("<I", self.b, self.o)[0]; self.o += 4; return v

    def cstr(self):
        e = self.b.index(0, self.o)
        s = self.b[self.o:e].decode("latin-1"); self.o = e + 1; return s


def q(s):
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n") + '"'


class Decompiler:
    """A stack-machine walk that prints statements: enough to read what a method does."""

    def __init__(self, out):
        self.out = out

    def block(self, code, pool, regs, indent):
        r = Reader(code)
        # Branch targets get labels.
        targets = set()
        ops = []
        while r.o < len(code):
            at = r.o
            op = r.u8()
            if op == 0:
                ops.append((at, op, b"", r.o))
                break
            data = b""
            if op >= 0x80:
                n = r.u16()
                data = code[r.o:r.o + n]
                r.o += n
            ops.append((at, op, data, r.o))
            if op in (0x99, 0x9D):
                targets.add(r.o + struct.unpack("<h", data[:2])[0])
        st = []
        pad = "  " * indent
        emit = lambda s: self.out.append(pad + s)

        def pop():
            return st.pop() if st else "<?>"

        i = 0
        while i < len(ops):
            at, op, data, end = ops[i]
            i += 1
            if at in targets:
                self.out.append(f"{pad[:-2] if pad else ''}L{at}:")
            d = Reader(data)
            if op == 0:
                break
            elif op == 0x88:
                n = d.u16()
                pool[:] = [d.cstr() for _ in range(n)]
            elif op == 0x96:
                while d.o < len(data):
                    t = d.u8()
                    if t == 0: st.append(q(d.cstr()))
                    elif t == 1: st.append(repr(round(struct.unpack("<f", data[d.o:d.o + 4])[0], 6))); d.o += 4
                    elif t == 2: st.append("null")
                    elif t == 3: st.append("undefined")
                    elif t == 4: st.append(regs.get(d.u8(), "r?"))
                    elif t == 5: st.append("true" if d.u8() else "false")
                    elif t == 6:
                        raw = data[d.o:d.o + 8]; d.o += 8
                        v = struct.unpack("<d", raw[4:8] + raw[0:4])[0]
                        st.append(repr(int(v)) if v == int(v) else repr(v))
                    elif t == 7: st.append(str(struct.unpack("<i", data[d.o:d.o + 4])[0])); d.o += 4
                    elif t == 8: k = d.u8(); st.append(q(pool[k]) if k < len(pool) else f"pool{k}")
                    elif t == 9: k = d.u16(); st.append(q(pool[k]) if k < len(pool) else f"pool{k}")
                    else: st.append(f"<push type {t}>"); break
            elif op in (0x9B, 0x8E):
                name = d.cstr()
                np = d.u16()
                fregs = {}
                params = []
                if op == 0x8E:
                    nreg = d.u8(); flags = d.u16()
                    k = 1
                    for bit, nm in ((0x0001, "this"), (0x0004, "arguments"), (0x0010, "super"), (0x0040, "_root"), (0x0080, "_parent"), (0x0100, "_global")):
                        if flags & bit:
                            fregs[k] = nm; k += 1
                    for _ in range(np):
                        reg = d.u8(); pn = d.cstr(); params.append(pn)
                        if reg:
                            fregs[reg] = pn
                else:
                    params = [d.cstr() for _ in range(np)]
                size = d.u16()
                body = code[end:end + size]
                # Skip the body's ops in this walk.
                while i < len(ops) and ops[i][0] < end + size:
                    i += 1
                sub = []
                Decompiler(sub).block(body, list(pool), dict(fregs) if op == 0x8E else {}, 1)
                text = f"function {name}({', '.join(params)}) {{\n" + "\n".join(pad + s for s in sub) + f"\n{pad}}}"
                if name:
                    emit(text)
                else:
                    st.append(text)
            elif op in SIMPLE:
                emit(SIMPLE[op] + ";")
            elif op in BINOP:
                b2 = pop(); a2 = pop(); st.append(f"({a2} {BINOP[op]} {b2})")
            elif op == 0x12:
                st.append(f"!{pop()}")
            elif op == 0x17:
                v = pop()
                emit(f"{v};")
            elif op == 0x1C:
                st.append(pop().strip('"'))
            elif op == 0x1D:
                v = pop(); n = pop().strip('"'); emit(f"{n} = {v};")
            elif op == 0x3C:
                v = pop(); n = pop().strip('"'); emit(f"var {n} = {v};")
            elif op == 0x41:
                n = pop().strip('"'); emit(f"var {n};")
            elif op == 0x4E:
                m = pop(); o = pop(); st.append(f"{o}.{m.strip(chr(34))}" if m.startswith('"') else f"{o}[{m}]")
            elif op == 0x4F:
                v = pop(); m = pop(); o = pop(); emit(f"{o}.{m.strip(chr(34))} = {v};" if m.startswith('"') else f"{o}[{m}] = {v};")
            elif op == 0x22:
                p = pop(); t = pop()
                try: p = PROPS[int(float(p))]
                except (ValueError, IndexError): pass
                st.append(f"getProperty({t}, {p})")
            elif op == 0x23:
                v = pop(); p = pop(); t = pop()
                try: p = PROPS[int(float(p))]
                except (ValueError, IndexError): pass
                emit(f"setProperty({t}, {p}, {v});")
            elif op in (0x3D, 0x52, 0x40, 0x53):
                if op in (0x3D, 0x40):
                    n = pop().strip('"'); obj = None
                else:
                    n = pop().strip('"'); obj = pop()
                try: argc = int(float(pop()))
                except ValueError: argc = 0
                args = [pop() for _ in range(argc)]
                callee = f"{obj}.{n}" if obj and n not in ("", "undefined") else (obj or n)
                pre = "new " if op in (0x40, 0x53) else ""
                st.append(f"{pre}{callee}({', '.join(args)})")
            elif op == 0x3E:
                emit(f"return {pop()};")
            elif op == 0x87:
                v = st[-1] if st else "<?>"
                reg = d.u8()
                emit(f"{regs.get(reg, f'r{reg}')} = {v};")
                if reg not in regs:
                    regs[reg] = f"r{reg}"
                if st:
                    st[-1] = regs[reg]
            elif op == 0x4C:
                st.append(st[-1] if st else "<?>")
            elif op == 0x4D:
                if len(st) >= 2: st[-1], st[-2] = st[-2], st[-1]
            elif op == 0x43:
                try: n = int(float(pop()))
                except ValueError: n = 0
                kv = []
                for _ in range(n):
                    v = pop(); k = pop(); kv.append(f"{k.strip(chr(34))}: {v}")
                st.append("{" + ", ".join(kv) + "}")
            elif op == 0x42:
                try: n = int(float(pop()))
                except ValueError: n = 0
                st.append("[" + ", ".join(pop() for _ in range(n)) + "]")
            elif op in (0x50, 0x51):
                st.append(f"({pop()} {'+' if op == 0x50 else '-'} 1)")
            elif op == 0x44:
                st.append(f"typeof {pop()}")
            elif op in (0x4A, 0x4B):
                st.append(f"{'Number' if op == 0x4A else 'String'}({pop()})")
            elif op == 0x3A:
                m = pop(); o = pop(); emit(f"delete {o}.{m.strip(chr(34))};")
            elif op == 0x3B:
                emit(f"delete {pop()};")
            elif op == 0x69:
                sup = pop(); sub = pop(); emit(f"{sub} extends {sup};")
            elif op == 0x2C:
                try: n = int(float(pop()))
                except ValueError: n = 0
                c = pop(); ifs = [pop() for _ in range(n)]; emit(f"{c} implements {', '.join(ifs)};")
            elif op == 0x99:
                emit(f"goto L{end + struct.unpack('<h', data[:2])[0]};")
            elif op == 0x9D:
                emit(f"if {pop()} goto L{end + struct.unpack('<h', data[:2])[0]};")
            elif op == 0x8B:
                emit(f"setTarget({q(d.cstr())});")
            elif op == 0x8C:
                emit(f"gotoAndStop({q(d.cstr())});")
            elif op == 0x81:
                emit(f"gotoFrame({d.u16()});")
            elif op == 0x9F:
                emit(f"gotoAndPlay({pop()});")
            elif op == 0x26:
                emit(f"trace({pop()});")
            elif op == 0x2B:
                c = pop(); o = pop(); st.append(f"cast({o}, {c})")
            elif op == 0x46:
                o = pop(); emit(f"for (k in {o}) ...;"); st.append("<enum>")
            elif op == 0x55:
                o = pop(); emit(f"for (k in {o}) ...;"); st.append("<enum>")
            elif op == 0x94:
                emit(f"with ({pop()}) {{ ... }}")
            else:
                emit(f"/* op 0x{op:02x} */")
        for x in st:
            emit(f"/* stack */ {x}")


def tags(b, o, end):
    r = Reader(b, o)
    while r.o < end:
        h = r.u16()
        code, n = h >> 6, h & 0x3F
        if n == 0x3F:
            n = r.u32()
        yield code, r.o, n
        r.o += n
        if code == 0:
            break


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("movie")
    ap.add_argument("--grep", help="only blocks whose text matches")
    ap.add_argument("--out")
    ap.add_argument("--audit", action="store_true", help="list each class's methods the port's runtime source never names")
    a = ap.parse_args()
    b = Path(a.movie).read_bytes()
    if b[:3] not in (b"FWS", b"GFX"):
        sys.exit(f"{a.movie}: not an uncompressed SWF / GFX ({b[:3]!r})")
    nbits = b[8] >> 3
    o = 8 + (5 + 4 * nbits + 7) // 8 + 4
    exports = {}
    images = []
    blocks = []

    def walk(start, end, where):
        for code, at, n in tags(b, start, end):
            if code == 56:  # ExportAssets
                r = Reader(b, at)
                for _ in range(r.u16()):
                    cid = r.u16(); exports[cid] = r.cstr()
            elif code in (12, 59):
                r = Reader(b, at)
                sid = r.u16() if code == 59 else None
                blocks.append((where, sid, b[r.o:at + n]))
            elif code == 39:
                r = Reader(b, at)
                sid = r.u16(); r.u16()
                walk(r.o, at + n, f"sprite {sid}")
            elif code in (1001, 1008):  # GFx DefineExternalImage(2)
                r = Reader(b, at)
                try:
                    if code == 1001:
                        cid = r.u16(); fmt = r.u16(); w = r.u16(); h = r.u16(); exp = r.b[r.o + 1:r.o + 1 + r.b[r.o]].decode("latin-1"); r.o += 1 + len(exp)
                        fn = r.b[r.o + 1:r.o + 1 + r.b[r.o]].decode("latin-1")
                    else:
                        cid = r.u32() & 0xFFFF; fmt = r.u16(); w = r.u16(); h = r.u16(); exp = r.b[r.o + 1:r.o + 1 + r.b[r.o]].decode("latin-1"); r.o += 1 + len(exp)
                        fn = r.b[r.o + 1:r.o + 1 + r.b[r.o]].decode("latin-1")
                    images.append((cid, exp, fn, w, h))
                except Exception:
                    images.append((None, "?", f"<tag {code} at {at}>", 0, 0))

    walk(o, len(b), "root")
    if a.audit:
        # Methods (`proto.Name = function`) per class, against the runtime's source text.
        repo = Path(__file__).resolve().parents[2]
        src = "\n".join(p.read_text(encoding="utf-8", errors="replace") for p in (repo / "crates" / "dishonored_runtime" / "src").rglob("*.rs"))
        meth = re.compile(r"^\s*r\d+\.(\w+) = function")
        rows = []
        for where, sid, code in blocks:
            if sid is None or not exports.get(sid, "").startswith("__Packages."):
                continue
            cls = exports[sid][len("__Packages."):]
            lines = []
            Decompiler(lines).block(code, [], {}, 1)
            names = [m.group(1) for l in lines if (m := meth.match(l))]
            if not names:
                continue
            cls_named = cls.split(".")[-1] in src
            missing = [n for n in names if n not in src]
            rows.append((cls, cls_named, len(names), missing))
        print(f"# {Path(a.movie).name}: methods the runtime never names")
        for cls, named, n, missing in sorted(rows, key=lambda r: (not r[1], -len(r[3]))):
            if missing:
                print(f"- {cls}{'' if named else ' (class not named in the port)'}: {len(missing)}/{n} missing: {', '.join(missing)}")
        return
    out = [f"# {Path(a.movie).name}: {len(blocks)} script blocks, {len(exports)} exports, {len(images)} external images", ""]
    if images and not a.grep:
        out.append("## External images")
        out += [f"- id {cid} export {exp!r} file {fn!r} {w}x{h}" for cid, exp, fn, w, h in images]
        out.append("")
    for where, sid, code in blocks:
        name = exports.get(sid, f"sprite {sid}") if sid is not None else where
        lines = []
        Decompiler(lines).block(code, [], {}, 1)
        text = "\n".join(lines)
        if a.grep and not re.search(a.grep, name + "\n" + text):
            continue
        out.append(f"## {'init ' if sid is not None else 'frame '}{name}")
        out.append(text)
        out.append("")
    s = "\n".join(out)
    if a.out:
        Path(a.out).write_text(s, encoding="utf-8")
        print(f"wrote {a.out}: {len(blocks)} blocks")
    else:
        sys.stdout.reconfigure(encoding="utf-8")
        print(s)


if __name__ == "__main__":
    main()
