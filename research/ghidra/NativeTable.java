// UE3 native function tables of the original executable: `FNameNativePtrPair { const ANSICHAR*
// Name; Native Func; }` arrays ({"execFoo", &UClass::execFoo}, ..., {NULL, NULL}), one per class.
// Labels each native `exec*` function in the program and writes "table / name / address" lines.
//
// analyzeHeadless <proj> dishonored -process Dishonored.exe -noanalysis
//     -scriptPath research/ghidra -postScript NativeTable.java <out_file>
// Output is derived from retail code: research only, never commit it.
//@category Dishonored

import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.mem.*;
import ghidra.program.model.symbol.*;
import java.io.*;
import java.nio.charset.StandardCharsets;

public class NativeTable extends GhidraScript {
    String ascii(Address a) {
        try {
            StringBuilder sb = new StringBuilder();
            Memory m = currentProgram.getMemory();
            for (int i = 0; i < 128; i++) {
                byte b = m.getByte(a.add(i));
                if (b == 0) return sb.toString();
                if (b < 32 || b > 126) return null;
                sb.append((char) b);
            }
        } catch (Exception e) {
        }
        return null;
    }

    Address ptr(Address a) {
        try {
            long v = currentProgram.getMemory().getInt(a) & 0xffffffffL;
            Address p = toAddr(v);
            return currentProgram.getMemory().contains(p) ? p : null;
        } catch (Exception e) {
            return null;
        }
    }

    @Override
    public void run() throws Exception {
        File out = new File(getScriptArgs()[0]);
        Memory mem = currentProgram.getMemory();
        int tables = 0, natives = 0;
        try (PrintWriter w = new PrintWriter(new OutputStreamWriter(new FileOutputStream(out), StandardCharsets.UTF_8))) {
            for (MemoryBlock b : mem.getBlocks()) {
                if (!b.isInitialized() || b.isExecute()) continue;
                Address a = b.getStart();
                Address end = b.getEnd().subtract(8);
                while (a.compareTo(end) < 0) {
                    // A table starts where a pair {name -> "exec...", func -> code} follows a non-pair.
                    Address n = ptr(a);
                    String s = n == null ? null : ascii(n);
                    Address f = ptr(a.add(4));
                    boolean pair = s != null && s.startsWith("exec") && f != null && mem.getBlock(f) != null && mem.getBlock(f).isExecute();
                    if (!pair) {
                        a = a.add(4);
                        continue;
                    }
                    tables++;
                    Address t = a;
                    while (true) {
                        Address nn = ptr(t);
                        String ss = nn == null ? null : ascii(nn);
                        Address ff = ptr(t.add(4));
                        if (ss == null || ff == null || !ss.startsWith("exec")) break;
                        w.println(a + "\t" + ss + "\t" + ff);
                        try {
                            if (getFunctionAt(ff) == null) createFunction(ff, null);
                            getSymbolTable().createLabel(ff, ss, SourceType.ANALYSIS);
                        } catch (Exception e) {
                        }
                        natives++;
                        t = t.add(8);
                    }
                    a = t.add(4);
                }
            }
        }
        println("native tables: " + tables + ", natives: " + natives);
    }
}
