// Find the functions that reference any address inside given data ranges (writers included),
// and decompile them: for runtime-initialised tables (static initialisers) whose values are not in
// the image. Research helper; output is derived from retail code, keep it out of the repository.
//
// -postScript W_RangeWriters.java <out_file> <start hex>-<end hex> [...]
//@category Dishonored

import ghidra.app.decompiler.*;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.util.*;

public class W_RangeWriters extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        PrintWriter out = new PrintWriter(new OutputStreamWriter(new FileOutputStream(a[0]), StandardCharsets.UTF_8));
        DecompInterface dec = new DecompInterface();
        dec.openProgram(currentProgram);
        ReferenceManager rm = currentProgram.getReferenceManager();
        FunctionManager fm = currentProgram.getFunctionManager();
        Set<Function> done = new LinkedHashSet<>();
        for (int i = 1; i < a.length; i++) {
            String[] r = a[i].split("-");
            long s = Long.parseLong(r[0], 16), e = Long.parseLong(r[1], 16);
            out.println("== range " + r[0] + "-" + r[1]);
            for (long p = s; p < e; p++) {
                Address ad = toAddr(p);
                for (Reference ref : rm.getReferencesTo(ad)) {
                    Function f = fm.getFunctionContaining(ref.getFromAddress());
                    out.println("  " + ad + " <- " + ref.getFromAddress() + " " + ref.getReferenceType() + " in " + (f == null ? "?" : f.getName() + " @ " + f.getEntryPoint()));
                    if (f != null && ref.getReferenceType().isWrite()) done.add(f);
                }
            }
        }
        for (Function f : done) {
            DecompileResults res = dec.decompileFunction(f, 120, monitor);
            out.println("\n// ===== " + f.getName() + " @ " + f.getEntryPoint());
            out.println(res.getDecompiledFunction() == null ? "(failed)" : res.getDecompiledFunction().getC());
        }
        out.close();
    }
}
