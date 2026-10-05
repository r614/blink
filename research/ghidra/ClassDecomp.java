// Decompile the code around UE3 classes/strings of the original executable, for research.
//
// analyzeHeadless <proj> dishonored -process Dishonored.exe -noanalysis -readOnly
//     -scriptPath research/ghidra -postScript ClassDecomp.java <out_dir> <name> [<name>...]
//
// For each name: finds it as a UTF-16 and ASCII string, every function referencing it, and
// writes those functions plus their direct callees (and, for functions that store a vtable,
// the vtable's virtual functions) decompiled into <out_dir>/<name>.c. Output is derived from
// retail code: research only, never commit it.
//@category Dishonored

import ghidra.app.decompiler.*;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.mem.*;
import ghidra.program.model.symbol.*;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.util.*;

public class ClassDecomp extends GhidraScript {
    DecompInterface dec;

    List<Address> findAll(byte[] pat) throws Exception {
        List<Address> out = new ArrayList<>();
        Memory mem = currentProgram.getMemory();
        for (MemoryBlock b : mem.getBlocks()) {
            if (!b.isInitialized()) continue;
            Address a = b.getStart();
            while (a != null && a.compareTo(b.getEnd()) <= 0) {
                Address f = mem.findBytes(a, b.getEnd(), pat, null, true, monitor);
                if (f == null) break;
                out.add(f);
                a = f.add(1);
            }
        }
        return out;
    }

    byte[] terminated(byte[] s, int zeros) {
        byte[] r = Arrays.copyOf(s, s.length + zeros);
        return r;
    }

    String decompile(Function f) {
        DecompileResults r = dec.decompileFunction(f, 120, monitor);
        if (r == null || !r.decompileCompleted()) return "// decompile failed: " + f.getName() + "\n";
        return r.getDecompiledFunction().getC();
    }

    Set<Function> referencing(Address target) {
        Set<Function> fs = new LinkedHashSet<>();
        for (Reference ref : getReferencesTo(target)) {
            Function f = getFunctionContaining(ref.getFromAddress());
            if (f != null) fs.add(f);
        }
        // Pointer tables (data referencing the string, then code referencing that slot).
        for (Reference ref : getReferencesTo(target)) {
            if (getFunctionContaining(ref.getFromAddress()) == null) {
                for (Reference r2 : getReferencesTo(ref.getFromAddress())) {
                    Function f = getFunctionContaining(r2.getFromAddress());
                    if (f != null) fs.add(f);
                }
            }
        }
        return fs;
    }

    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        File outDir = new File(args[0]);
        outDir.mkdirs();
        dec = new DecompInterface();
        dec.openProgram(currentProgram);
        Listing listing = currentProgram.getListing();
        for (int i = 1; i < args.length; i++) {
            String name = args[i];
            StringBuilder sb = new StringBuilder();
            Set<Function> roots = new LinkedHashSet<>();
            for (Address a : findAll(terminated(name.getBytes(StandardCharsets.UTF_16LE), 2))) {
                sb.append("// UTF-16 string at ").append(a).append("\n");
                roots.addAll(referencing(a));
            }
            for (Address a : findAll(terminated(name.getBytes(StandardCharsets.US_ASCII), 1))) {
                sb.append("// ASCII string at ").append(a).append("\n");
                roots.addAll(referencing(a));
            }
            Set<Function> done = new LinkedHashSet<>();
            for (Function f : roots) {
                if (!done.add(f)) continue;
                sb.append("\n// ===== ").append(f.getName()).append(" @ ").append(f.getEntryPoint()).append(" (references '").append(name).append("')\n");
                sb.append(decompile(f));
                for (Function c : f.getCalledFunctions(monitor)) {
                    if (done.add(c) && c.getBody().getNumAddresses() < 20000) {
                        sb.append("\n// ----- callee ").append(c.getName()).append(" @ ").append(c.getEntryPoint()).append("\n");
                        sb.append(decompile(c));
                    }
                }
            }
            try (Writer w = new OutputStreamWriter(new FileOutputStream(new File(outDir, name + ".c")), StandardCharsets.UTF_8)) {
                w.write(sb.toString());
            }
            println(name + ": " + roots.size() + " referencing functions, " + done.size() + " decompiled");
        }
    }
}
