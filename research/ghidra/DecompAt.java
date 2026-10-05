// Decompile the functions at the given addresses (creating them if Ghidra has none there).
// -postScript DecompAt.java <out_file> <address hex> [<address hex>...]   (research helper;
// output is derived from retail code: keep it out of the repository)
//@category Dishonored

import ghidra.app.decompiler.*;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.*;
import java.io.*;
import java.nio.charset.StandardCharsets;

public class DecompAt extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        DecompInterface dec = new DecompInterface();
        dec.openProgram(currentProgram);
        try (Writer w = new OutputStreamWriter(new FileOutputStream(a[0]), StandardCharsets.UTF_8)) {
            for (int i = 1; i < a.length; i++) {
                Address at = toAddr(Long.parseLong(a[i].replace("0x", ""), 16));
                Function f = getFunctionAt(at);
                if (f == null) {
                    try {
                        f = createFunction(at, null);
                    } catch (Exception e) {
                        f = null;
                    }
                }
                if (f == null) {
                    f = getFunctionContaining(at);
                }
                w.write("// ===== " + a[i] + (f == null ? " (no function)" : " " + f.getName()) + "\n");
                if (f == null) continue;
                DecompileResults r = dec.decompileFunction(f, 120, monitor);
                if (r != null && r.getDecompiledFunction() != null) {
                    w.write(r.getDecompiledFunction().getC());
                } else {
                    w.write("// decompile failed\n");
                }
                w.write("\n");
            }
        }
    }
}
