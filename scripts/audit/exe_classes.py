"""The game's native classes, their vtables and methods, read from the retail exe and decomp.c.

Static: the exe's bytes and the dumped decompile are read; nothing is launched.

Every native class has a static class struct in the exe's data (UE3 `IMPLEMENT_CLASS`): +0x00 the
instance size, +0x10 the class name, +0x14 the package name, +0x18 the config name (UTF-16), +0x24
`InternalConstructor<T>`, which calls the C++ constructor; the constructor stores the class's
vtable in the object's first dword (`*param_1 = &PTR_..._<vtable>` in the decompile). The vtable's
slots are read from the exe until a dword that is not a function start or the next class's vtable.

    from exe_classes import load_classes
    classes = load_classes(funcs, texts)   # {name: {"size", "package", "vtable", "slots", "ctor"}}
"""

import os
import re
import struct
from pathlib import Path

VT_RE = re.compile(r"\*(?:\(undefined \*\*\))?param_1 = &PTR_[A-Za-z]+_([0-9a-f]{8});")
EXE = Path(os.environ.get("DISHONORED_EXE_DIR", r"C:\Program Files\GOG Galaxy\Games\Dishonored")) / "Binaries" / "Win32" / "Dishonored.exe"


class Exe:
    def __init__(self, path=EXE):
        self.d = path.read_bytes()
        pe = struct.unpack_from("<I", self.d, 0x3C)[0]
        n = struct.unpack_from("<H", self.d, pe + 6)[0]
        opt = struct.unpack_from("<H", self.d, pe + 20)[0]
        self.base = struct.unpack_from("<I", self.d, pe + 52)[0]
        self.secs = []
        o = pe + 24 + opt
        for _ in range(n):
            name = self.d[o : o + 8].rstrip(b"\0").decode("ascii", "replace")
            vsize, va, rsize, raw = struct.unpack_from("<IIII", self.d, o + 8)
            chars = struct.unpack_from("<I", self.d, o + 36)[0]
            self.secs.append((name, self.base + va, vsize, raw, rsize, chars))
            o += 40

    def off(self, va):
        for _n, sva, vsize, raw, rsize, _c in self.secs:
            if sva <= va < sva + min(vsize, rsize):
                return raw + va - sva
        return None

    def dword(self, va):
        f = self.off(va)
        return struct.unpack_from("<I", self.d, f)[0] if f is not None else None

    def wstr(self, va, limit=128):
        f = self.off(va)
        if f is None:
            return None
        end = self.d.find(b"\0\0", f, f + limit * 2)
        while end != -1 and (end - f) % 2:
            end = self.d.find(b"\0\0", end + 1, f + limit * 2)
        if end == -1:
            return None
        try:
            s = self.d[f:end].decode("utf-16-le")
        except UnicodeDecodeError:
            return None
        return s if s and s.isprintable() else None

    def code(self, va):
        return any(c & 0x20000000 and sva <= va < sva + vs for _n, sva, vs, _r, _rs, c in self.secs)

    def data_sections(self):
        return [(sva, raw, min(vs, rs)) for _n, sva, vs, raw, rs, c in self.secs if not c & 0x20000000]


PACKAGES = ("DishonoredGame", "Engine", "GameFramework", "Core", "GFxUI", "IpDrv", "AkAudio", "WinDrv", "OnlineSubsystemSteamworks")


def va_of(exe, f):
    for _n, sva, vsize, raw, rsize, _c in exe.secs:
        if raw <= f < raw + min(vsize, rsize):
            return sva + f - raw
    return None


def static_classes(exe):
    """[(struct VA, name, package, size, internal constructor)] for every static class struct: a
    dword at +0x14 pointing at a package name, a class name at +0x10, code at +0x24."""
    out = []
    data = exe.data_sections()
    for pkg in PACKAGES:
        pat = (pkg + "\0").encode("utf-16-le")
        f = exe.d.find(pat)
        while f != -1:
            pva = va_of(exe, f)
            # Any occurrence: the linker shares a string with the tail of a longer one.
            if pva is not None and pva % 2 == 0:
                ref = struct.pack("<I", pva)
                for sva, raw, size in data:
                    k = exe.d.find(ref, raw, raw + size)
                    while k != -1:
                        s = k - 0x14
                        if (s - raw) % 4 == 0 and s >= raw:
                            name = exe.wstr(struct.unpack_from("<I", exe.d, s + 0x10)[0])
                            ictor = struct.unpack_from("<I", exe.d, s + 0x24)[0]
                            size_ = struct.unpack_from("<I", exe.d, s)[0]
                            if name and exe.code(ictor) and 0 < size_ < 0x100000:
                                out.append((sva + s - raw, name, pkg, size_, ictor))
                        k = exe.d.find(ref, k + 4, raw + size)
            f = exe.d.find(pat, f + 2)
    return out


def load_classes(funcs, texts):
    """funcs: {addr: (callees, ...)} of decomp.c; texts: {addr: vtable VA stored as *param_1}."""
    exe = Exe()
    found = static_classes(exe)
    vt_starts = set(texts.values())
    classes = {}
    for sva, name, pkg, size, ictor in found:
        ctor, vt = None, None
        # InternalConstructor<T> (T::T inlined) or the callee storing a vtable, at most two calls deep.
        frontier = [ictor]
        for _ in range(3):
            nxt = []
            for f in frontier:
                if f in texts:
                    ctor, vt = f, texts[f]
                    break
                nxt += sorted(funcs.get(f, (set(),))[0])
            if vt:
                break
            frontier = nxt
        slots = []
        if vt:
            va = vt
            while True:
                x = exe.dword(va)
                if x is None or not exe.code(x) or (va != vt and va in vt_starts):
                    break
                slots.append(x)
                va += 4
        classes[name] = {"package": pkg, "size": size, "struct": sva, "ictor": ictor, "ctor": ctor, "vtable": vt, "slots": slots}
    return classes
