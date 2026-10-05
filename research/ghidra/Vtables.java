// Every vtable a UE3 class's constructor stores (primary and interface vtables), with the
// listed slots of each decompiled. Finds interface implementations (e.g. a pawn's
// DisConvSpeakerInterface methods).
// -postScript Vtables.java <out_file> <Class> <slot>[+<slot>...] [max vtable slots]   (research helper)
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

public class Vtables extends GhidraScript {
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

    List<Address> slots(Address vt) {
        List<Address> out = new ArrayList<>();
        for (int i = 0; i < 400; i++) {
            Address p = ptr(vt.add(i * 4));
            if (!isCode(p)) break;
            if (i > 0 && getReferencesTo(vt.add(i * 4)).length > 0) break;
            out.add(p);
        }
        return out;
    }

    void vtables(Function f, int depth, Set<Address> out, Set<Function> seen) {
        if (f == null || depth > 2 || !seen.add(f)) return;
        InstructionIterator it = currentProgram.getListing().getInstructions(f.getBody(), true);
        while (it.hasNext()) {
            Instruction ins = it.next();
            if (!ins.getMnemonicString().equals("MOV")) continue;
            for (int i = 0; i < ins.getNumOperands(); i++) {
                for (Object o : ins.getOpObjects(i)) {
                    if (o instanceof Scalar) {
                        Address v = toAddr(((Scalar) o).getUnsignedValue());
                        if (mem.contains(v) && !isCode(v) && isCode(ptr(v)) && isCode(ptr(v.add(4))) && isCode(ptr(v.add(8)))) out.add(v);
                    }
                }
            }
        }
        if (depth == 0) {
            for (Function c : f.getCalledFunctions(monitor)) vtables(c, depth + 1, out, seen);
        }
    }

    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        mem = currentProgram.getMemory();
        int maxSlots = a.length > 3 ? Integer.parseInt(a[3]) : 100;
        List<Integer> want = new ArrayList<>();
        for (String s : a[2].split("[,+]")) want.add(Integer.parseInt(s));
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
        DecompInterface dec = new DecompInterface();
        dec.openProgram(currentProgram);
        try (Writer w = new OutputStreamWriter(new FileOutputStream(a[0]), StandardCharsets.UTF_8)) {
            if (rec == null) {
                w.write("no record\n");
                return;
            }
            Set<Address> vts = new LinkedHashSet<>();
            for (int i = 1; i < 16; i++) {
                Address p = ptr(rec.add(i * 4));
                if (isCode(p)) vtables(func(p), 0, vts, new HashSet<>());
            }
            for (Address vt : vts) {
                List<Address> sl = slots(vt);
                w.write("\n// ##### vtable " + vt + " (" + sl.size() + " slots)\n");
                if (sl.size() > maxSlots) continue;
                StringBuilder all = new StringBuilder("// slots:");
                for (Address x : sl) all.append(' ').append(x);
                w.write(all.append('\n').toString());
                for (int k : want) {
                    if (k >= sl.size()) continue;
                    Function f = func(sl.get(k));
                    if (f == null) f = getFunctionContaining(sl.get(k));
                    if (f == null) {
                        w.write("// slot " + k + " -> " + sl.get(k) + " (no function)\n");
                        continue;
                    }
                    w.write("\n// ===== slot " + k + " (+0x" + Integer.toHexString(k * 4) + ") " + f.getName() + " @ " + f.getEntryPoint() + "\n");
                    DecompileResults r = dec.decompileFunction(f, 60, monitor);
                    w.write(r != null && r.decompileCompleted() ? r.getDecompiledFunction().getC() : "// failed\n");
                }
            }
        }
        println("done");
    }
}
