// Who uses a UE3 class natively: functions referencing its static-class record (directly, or
// through the GetPrivateStaticClass accessor that references it), decompiled. Finds IsA checks
// and spawn/iteration logic for classes without native overrides.
// -postScript ClassUsers.java <out_file> <Class> [max]   (research helper)
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

public class ClassUsers extends GhidraScript {
    Memory mem;

    List<Address> rawRefs(Address target) throws Exception {
        List<Address> out = new ArrayList<>();
        long o = target.getOffset();
        byte[] le = { (byte) o, (byte) (o >> 8), (byte) (o >> 16), (byte) (o >> 24) };
        Address p = currentProgram.getMinAddress();
        while ((p = mem.findBytes(p, le, null, true, monitor)) != null) {
            out.add(p);
            p = p.add(1);
        }
        return out;
    }

    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        mem = currentProgram.getMemory();
        int max = a.length > 2 ? Integer.parseInt(a[2]) : 40;
        byte[] pat = (a[1] + "\0").getBytes(StandardCharsets.UTF_16LE);
        Address rec = null;
        Address s = currentProgram.getMinAddress();
        while (rec == null && (s = mem.findBytes(s, pat, null, true, monitor)) != null) {
            for (Address r : rawRefs(s)) {
                if (mem.getBlock(r) != null && !mem.getBlock(r).isExecute()) {
                    rec = r;
                    break;
                }
            }
            s = s.add(2);
        }
        if (rec == null) {
            println("no record for " + a[1]);
            return;
        }
        // The static class pointer usually lives a few dwords around the record; code that
        // references any address in [rec-64, rec+64] is a class accessor or registration.
        Set<Function> accessors = new LinkedHashSet<>();
        for (int d = -64; d <= 64; d += 4) {
            for (Address r : rawRefs(rec.add(d))) {
                Function f = getFunctionContaining(r);
                if (f != null) accessors.add(f);
            }
        }
        Set<Function> users = new LinkedHashSet<>();
        for (Function f : accessors) {
            for (Reference ref : getReferencesTo(f.getEntryPoint())) {
                Function u = getFunctionContaining(ref.getFromAddress());
                if (u != null && !accessors.contains(u)) users.add(u);
            }
        }
        DecompInterface dec = new DecompInterface();
        dec.openProgram(currentProgram);
        try (Writer w = new OutputStreamWriter(new FileOutputStream(a[0]), StandardCharsets.UTF_8)) {
            w.write("// record " + rec + "; accessors:");
            for (Function f : accessors) w.write(" " + f.getEntryPoint());
            w.write("\n");
            int n = 0;
            for (Function u : users) {
                if (n++ >= max) break;
                DecompileResults r = dec.decompileFunction(u, 120, monitor);
                w.write("\n// ===== " + u.getName() + " @ " + u.getEntryPoint() + "\n");
                w.write(r != null && r.decompileCompleted() ? r.getDecompiledFunction().getC() : "// failed\n");
            }
        }
        println(a[1] + ": record " + rec + ", " + accessors.size() + " accessors, " + users.size() + " users");
    }
}
