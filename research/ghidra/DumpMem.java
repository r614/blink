// Dump dwords at an address, following each pointer to an ASCII/UTF-16 string when there is one.
// -postScript DumpMem.java <address hex> [count]   (research helper)
//@category Dishonored

import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.mem.*;

public class DumpMem extends GhidraScript {
    String str(Address p) {
        Memory m = currentProgram.getMemory();
        try {
            StringBuilder a = new StringBuilder(), w = new StringBuilder();
            for (int i = 0; i < 64; i++) {
                byte b = m.getByte(p.add(i));
                if (b == 0) break;
                if (b < 32 || b > 126) { a = null; break; }
                a.append((char) b);
            }
            for (int i = 0; i < 64; i++) {
                short c = m.getShort(p.add(i * 2));
                if (c == 0) break;
                if (c < 32 || c > 126) { w = null; break; }
                w.append((char) c);
            }
            if (w != null && w.length() > 1) return "L\"" + w + "\"";
            if (a != null && a.length() > 1) return "\"" + a + "\"";
        } catch (Exception e) {
        }
        return "";
    }

    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        Address base = toAddr(Long.parseLong(a[0], 16));
        int n = a.length > 1 ? Integer.parseInt(a[1]) : 16;
        Memory m = currentProgram.getMemory();
        println("at " + base + ": " + str(base));
        for (int i = 0; i < n; i++) {
            Address at = base.add(i * 4);
            long v = m.getInt(at) & 0xffffffffL;
            Address p = toAddr(v);
            println(at + ": " + String.format("%08x", v) + " " + (m.contains(p) ? str(p) : ""));
        }
    }
}
