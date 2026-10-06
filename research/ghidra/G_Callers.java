// List the functions that call (or otherwise reference) the given functions, with the call
// site, and decompile those callers.
//
// -postScript G_Callers.java <out_file> <depth 0|1> <addr hex> ...
//
// depth 1 also decompiles each caller. Research helper; output is derived from retail code:
// keep it out of the repository.
//@category Dishonored

import ghidra.app.decompiler.*;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.util.*;

public class G_Callers extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        int depth = Integer.parseInt(a[1]);
        DecompInterface dec = new DecompInterface();
        dec.openProgram(currentProgram);
        Set<Address> done = new HashSet<>();
        try (Writer w = new OutputStreamWriter(new FileOutputStream(a[0]), StandardCharsets.UTF_8)) {
            for (int i = 2; i < a.length; i++) {
                Address t = toAddr(Long.parseLong(a[i], 16));
                w.write("== callers of " + t + "\n");
                for (Reference r : getReferencesTo(t)) {
                    Function f = getFunctionContaining(r.getFromAddress());
                    String fn = f == null ? "?" : f.getName() + " @ " + f.getEntryPoint();
                    w.write("  " + r.getFromAddress() + " " + r.getReferenceType() + " in " + fn + "\n");
                    if (depth > 0 && f != null && done.add(f.getEntryPoint()) && f.getBody().getNumAddresses() < 12000) {
                        DecompileResults res = dec.decompileFunction(f, 120, monitor);
                        if (res.decompileCompleted()) {
                            w.write("\n// ===== caller " + f.getName() + " @ " + f.getEntryPoint() + "\n");
                            w.write(res.getDecompiledFunction().getC());
                        }
                    }
                }
            }
        }
    }
}
