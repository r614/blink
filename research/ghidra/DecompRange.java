// Decompile every function in one or more address ranges (or single functions) into one file,
// each preceded by its entry address and its direct callers, for offline reading.
//
// -postScript DecompRange.java <out_file> <start hex>[-<end hex>] ...
//
// A single address decompiles the function containing it. Output is derived from retail code:
// research only, never commit it.
//@category Dishonored

import ghidra.app.decompiler.*;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.util.*;

public class DecompRange extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        Set<Function> fs = new LinkedHashSet<>();
        for (int i = 1; i < a.length; i++) {
            String[] r = a[i].split("-");
            Address s = toAddr(Long.parseLong(r[0].replace("0x", ""), 16));
            if (r.length == 1) {
                Function f = getFunctionContaining(s);
                if (f == null) {
                    try {
                        f = createFunction(s, null);
                    } catch (Exception e) {
                    }
                }
                if (f != null) fs.add(f);
                continue;
            }
            Address e = toAddr(Long.parseLong(r[1].replace("0x", ""), 16));
            FunctionIterator it = currentProgram.getFunctionManager().getFunctions(new AddressSet(s, e), true);
            while (it.hasNext()) fs.add(it.next());
        }
        DecompInterface dec = new DecompInterface();
        dec.openProgram(currentProgram);
        try (Writer w = new OutputStreamWriter(new FileOutputStream(a[0]), StandardCharsets.UTF_8)) {
            for (Function f : fs) {
                StringBuilder callers = new StringBuilder();
                Set<String> seen = new TreeSet<>();
                for (Reference ref : getReferencesTo(f.getEntryPoint())) {
                    Function c = getFunctionContaining(ref.getFromAddress());
                    seen.add(c == null ? ("data@" + ref.getFromAddress()) : c.getEntryPoint().toString());
                }
                for (String c : seen) callers.append(' ').append(c);
                w.write("\n// ===== " + f.getName() + " @ " + f.getEntryPoint() + " refs:" + callers + "\n");
                DecompileResults r = dec.decompileFunction(f, 60, monitor);
                String c = r != null && r.decompileCompleted() ? r.getDecompiledFunction().getC() : "// failed\n";
                // Drop the array bounds-check noise.
                StringBuilder out = new StringBuilder();
                for (String line : c.split("\n")) {
                    if (line.contains("Array.h") || line.contains("i>=0 && (i<ArrayNum")) continue;
                    out.append(line).append('\n');
                }
                w.write(out.toString());
            }
        }
        println(fs.size() + " functions decompiled");
    }
}
