// Decompile functions by address (and, optionally, the functions they call), for research.
//
// -postScript Decomp.java <out_file> <depth> <addr hex>[@label] | refs:<value hex>[@label] ...
//
// Writes every function reached within <depth> call levels (0 = only the given ones) to
// <out_file>, each once, skipping thunks and functions larger than 12 KB. Output is derived
// from retail code: research only, never commit it.
//@category Dishonored

import ghidra.app.decompiler.*;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.*;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.util.*;

public class Decomp extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        int depth = Integer.parseInt(a[1]);
        DecompInterface dec = new DecompInterface();
        dec.openProgram(currentProgram);
        Set<Address> seen = new HashSet<>();
        ArrayDeque<Object[]> q = new ArrayDeque<>();
        ghidra.program.model.mem.Memory mem = currentProgram.getMemory();
        for (int i = 2; i < a.length; i++) {
            if (a[i].startsWith("mem:")) {
                // initial value of a global (writable data keeps its initializer in the image)
                Address da = toAddr(Long.parseLong(a[i].substring(4), 16));
                try {
                    int bits = mem.getInt(da);
                    println("mem " + da + " = " + Float.intBitsToFloat(bits) + "f / " + bits);
                } catch (Exception e) {
                    println("mem " + da + " unreadable");
                }
                continue;
            }
            if (a[i].startsWith("refs:")) {
                // every function whose code contains the value (a global's address)
                String[] kv = a[i].substring(5).split("@");
                long o = Long.parseLong(kv[0], 16);
                byte[] le = { (byte) o, (byte) (o >> 8), (byte) (o >> 16), (byte) (o >> 24) };
                Address p = currentProgram.getMinAddress();
                while ((p = mem.findBytes(p, le, null, true, monitor)) != null) {
                    ghidra.program.model.mem.MemoryBlock b = mem.getBlock(p);
                    Function f = getFunctionContaining(p);
                    if (b != null && b.isExecute() && f != null) {
                        q.add(new Object[] { f.getEntryPoint(), 0, "uses " + (kv.length > 1 ? kv[1] : kv[0]) + " at " + p });
                    }
                    p = p.add(1);
                }
                continue;
            }
            String[] kv = a[i].split("@");
            q.add(new Object[] { toAddr(Long.parseLong(kv[0], 16)), 0, kv.length > 1 ? kv[1] : "" });
        }
        StringBuilder sb = new StringBuilder();
        int n = 0;
        while (!q.isEmpty() && n < 400) {
            Object[] it = q.poll();
            Address ad = (Address) it[0];
            int d = (Integer) it[1];
            if (!seen.add(ad)) continue;
            Function f = getFunctionAt(ad);
            if (f == null) {
                try {
                    f = createFunction(ad, null);
                } catch (Exception e) {
                }
            }
            if (f == null) {
                sb.append("// no function at ").append(ad).append("\n");
                continue;
            }
            if (f.isThunk()) f = f.getThunkedFunction(true);
            if (f.getBody().getNumAddresses() > 12000) {
                sb.append("// skipped large ").append(f.getName()).append(" @ ").append(f.getEntryPoint()).append("\n");
                continue;
            }
            DecompileResults r = dec.decompileFunction(f, 180, monitor);
            sb.append("\n// ===== ").append(it[2]).append(" ").append(f.getName()).append(" @ ").append(f.getEntryPoint()).append(" (depth ").append(d).append(")\n");
            String c = r != null && r.decompileCompleted() ? r.getDecompiledFunction().getC() : "// decompile failed\n";
            sb.append(c);
            // values of the read-only data the function reads (float constants, mostly)
            java.util.regex.Matcher m = java.util.regex.Pattern.compile("_?DAT_([0-9a-f]{8})").matcher(c);
            TreeSet<String> consts = new TreeSet<>();
            while (m.find()) {
                Address da = toAddr(Long.parseLong(m.group(1), 16));
                ghidra.program.model.mem.MemoryBlock b = mem.getBlock(da);
                if (b == null || b.isWrite() || b.isExecute()) continue;
                try {
                    int bits = mem.getInt(da);
                    consts.add(m.group(1) + "=" + Float.intBitsToFloat(bits) + "f/" + bits + (da.getOffset() % 8 == 0 ? "/d" + Double.longBitsToDouble(mem.getLong(da)) : ""));
                } catch (Exception e) {
                }
            }
            if (!consts.isEmpty()) sb.append("// consts: ").append(String.join("  ", consts)).append("\n");
            n++;
            if (d < depth) {
                for (Function cf : f.getCalledFunctions(monitor)) {
                    q.add(new Object[] { cf.getEntryPoint(), d + 1, "callee of " + f.getEntryPoint() });
                }
            }
        }
        try (Writer w = new OutputStreamWriter(new FileOutputStream(a[0]), StandardCharsets.UTF_8)) {
            w.write(sb.toString());
        }
        println("wrote " + n + " functions to " + a[0]);
    }
}
