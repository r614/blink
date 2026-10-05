// Print the dword at each address as hex, int, float, and the qword as double (research helper).
// -postScript Consts.java <out_file> <addr hex> ...
//@category Dishonored

import ghidra.app.script.GhidraScript;
import ghidra.program.model.mem.*;
import java.io.*;
import java.nio.charset.StandardCharsets;

public class Consts extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        Memory m = currentProgram.getMemory();
        try (Writer w = new OutputStreamWriter(new FileOutputStream(a[0]), StandardCharsets.UTF_8)) {
            for (int i = 1; i < a.length; i++) {
                long addr = Long.parseLong(a[i].replace("0x", ""), 16);
                try {
                    int v = m.getInt(toAddr(addr));
                    long q = m.getLong(toAddr(addr));
                    w.write(String.format("%08x  0x%08x  %d  %g  %g%n", addr, v, v, Float.intBitsToFloat(v), Double.longBitsToDouble(q)));
                } catch (Exception e) {
                    w.write(String.format("%08x  ?%n", addr));
                }
            }
        }
        println("done");
    }
}
