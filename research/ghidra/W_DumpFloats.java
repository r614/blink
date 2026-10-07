// Dump dwords of static data as hex and float: -postScript W_DumpFloats.java <out_file> <addr hex>:<count> ...
// Research helper; output is derived from retail data, keep it out of the repository.
//@category Dishonored

import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.mem.*;
import java.io.*;
import java.nio.charset.StandardCharsets;

public class W_DumpFloats extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        PrintWriter out = new PrintWriter(new OutputStreamWriter(new FileOutputStream(a[0]), StandardCharsets.UTF_8));
        Memory m = currentProgram.getMemory();
        for (int i = 1; i < a.length; i++) {
            String[] r = a[i].split(":");
            Address s = toAddr(Long.parseLong(r[0], 16));
            int n = Integer.parseInt(r[1]);
            out.println("== " + s);
            for (int k = 0; k < n; k++) {
                Address p = s.add(4L * k);
                try {
                    int v = m.getInt(p);
                    out.println(p + " " + String.format("%08x %.9g", v, Float.intBitsToFloat(v)));
                } catch (Exception e) {
                    out.println(p + " ?");
                }
            }
        }
        out.close();
    }
}
