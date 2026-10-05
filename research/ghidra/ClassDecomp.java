// Decompile the native code of UE3 classes of the original executable, for research.
//
// analyzeHeadless <proj> dishonored -process Dishonored.exe -noanalysis -readOnly
//     -scriptPath research/ghidra -postScript ClassDecomp.java <out_dir> <Class>[:<Parent>] ...
//
// UE3 registers each native class with a static record holding its UTF-16 name followed by
// pointers (package name, flags, InternalConstructor, StaticConstructor...). The constructor
// stores the class vtable into the new object. For each class this writes <out_dir>/<Class>.c
// with the record's functions, and every vtable slot whose function differs from the parent
// class's vtable (the class's own virtual overrides: Activated, Tick, ...), decompiled; with
// the native `exec*` functions labelled by NativeTable.java. Output is derived from retail
// code: research only, never commit it.
//@category Dishonored

import ghidra.app.decompiler.*;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.mem.*;
import ghidra.program.model.scalar.Scalar;
import ghidra.program.model.symbol.*;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.util.*;

public class ClassDecomp extends GhidraScript {
    DecompInterface dec;
    Memory mem;

    Address ptr(Address a) {
        try {
            Address p = toAddr(mem.getInt(a) & 0xffffffffL);
            return mem.contains(p) ? p : null;
        } catch (Exception e) {
            return null;
        }
    }

    boolean isCode(Address p) {
        return p != null && mem.getBlock(p) != null && mem.getBlock(p).isExecute();
    }

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

    Function func(Address a) {
        Function f = getFunctionAt(a);
        if (f == null) {
            try {
                f = createFunction(a, null);
            } catch (Exception e) {
            }
        }
        return f;
    }

    String decompile(Function f) {
        DecompileResults r = dec.decompileFunction(f, 120, monitor);
        if (r == null || !r.decompileCompleted()) return "// decompile failed: " + f.getName() + "\n";
        return r.getDecompiledFunction().getC();
    }

    /// Static class record: where the class's UTF-16 name is referenced from data.
    Address record(String cls) throws Exception {
        // Every occurrence of the name: the one the record references (not a suffix of a longer name).
        byte[] pat = (cls + "\0").getBytes(StandardCharsets.UTF_16LE);
        Address s = currentProgram.getMinAddress();
        while ((s = mem.findBytes(s, pat, null, true, monitor)) != null) {
            // (Records point exactly at the name, so suffix matches have no references.)
            for (Address r : rawRefs(s)) {
                MemoryBlock b = mem.getBlock(r);
                if (b != null && !b.isExecute()) return r;
            }
            s = s.add(2);
        }
        return null;
    }

    /// The vtable a constructor stores: a data address whose first slots are code pointers,
    /// written as an immediate by the constructor (or a function it calls).
    Address vtableOf(Function ctor, int depth) {
        if (ctor == null || depth > 2) return null;
        Address best = null;
        InstructionIterator it = currentProgram.getListing().getInstructions(ctor.getBody(), true);
        while (it.hasNext()) {
            Instruction ins = it.next();
            if (!ins.getMnemonicString().equals("MOV")) continue;
            for (int i = 0; i < ins.getNumOperands(); i++) {
                for (Object o : ins.getOpObjects(i)) {
                    if (o instanceof Scalar) {
                        Address v = toAddr(((Scalar) o).getUnsignedValue());
                        // Multiple inheritance stores several vtables: keep the longest (the UObject one).
                        if (mem.contains(v) && !isCode(v) && isCode(ptr(v)) && isCode(ptr(v.add(4))) && isCode(ptr(v.add(8)))
                                && (best == null || slots(v).size() > slots(best).size())) best = v;
                    }
                }
            }
        }
        if (best != null) return best;
        for (Function c : ctor.getCalledFunctions(monitor)) {
            Address v = vtableOf(c, depth + 1);
            if (v != null) return v;
        }
        return null;
    }

    List<Address> slots(Address vt) {
        List<Address> out = new ArrayList<>();
        for (int i = 0; i < 400; i++) {
            Address p = ptr(vt.add(i * 4));
            if (!isCode(p)) break;
            // The next class's vtable starts where code references the slot address.
            if (i > 0 && getReferencesTo(vt.add(i * 4)).length > 0) break;
            out.add(p);
        }
        return out;
    }

    Function ctorOf(Address rec) {
        // The record's code pointers; the InternalConstructor is the one that stores a vtable.
        for (int i = 1; i < 16; i++) {
            Address p = ptr(rec.add(i * 4));
            if (isCode(p)) {
                Function f = func(p);
                if (vtableOf(f, 0) != null) return f;
            }
        }
        return null;
    }

    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        File outDir = new File(args[0]);
        outDir.mkdirs();
        mem = currentProgram.getMemory();
        dec = new DecompInterface();
        dec.openProgram(currentProgram);
        for (int i = 1; i < args.length; i++) {
            String[] cp = args[i].split(":");
            String cls = cp[0];
            StringBuilder sb = new StringBuilder();
            Address rec = record(cls);
            if (rec == null) {
                println(cls + ": no class record");
                continue;
            }
            sb.append("// class record of ").append(cls).append(" at ").append(rec).append("\n");
            Function ctor = ctorOf(rec);
            Address vt = vtableOf(ctor, 0);
            List<Address> mine = vt == null ? new ArrayList<>() : slots(vt);
            Set<Address> parent = new HashSet<>();
            if (cp.length > 1) {
                Address prec = record(cp[1]);
                Address pvt = prec == null ? null : vtableOf(ctorOf(prec), 0);
                if (pvt != null) parent.addAll(slots(pvt));
            }
            sb.append("// constructor ").append(ctor == null ? "?" : ctor.getEntryPoint()).append(", vtable ").append(vt).append(" (").append(mine.size()).append(" slots)\n");
            int n = 0;
            for (int s = 0; s < mine.size(); s++) {
                if (parent.contains(mine.get(s))) continue;
                if (cp.length == 1 && s < 0) continue;
                Function f = func(mine.get(s));
                if (f == null) continue;
                sb.append("\n// ===== vtable slot ").append(s).append(" (+0x").append(Integer.toHexString(s * 4)).append(") ").append(f.getName()).append(" @ ").append(f.getEntryPoint()).append("\n");
                sb.append(decompile(f));
                n++;
            }
            try (Writer w = new OutputStreamWriter(new FileOutputStream(new File(outDir, cls + ".c")), StandardCharsets.UTF_8)) {
                w.write(sb.toString());
            }
            println(cls + ": vtable " + vt + ", " + mine.size() + " slots, " + n + " overridden decompiled");
        }
    }
}
