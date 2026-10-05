// For each Kismet class, print the entry points of the SequenceOp virtuals that decide its
// runtime behaviour: slot 92 UpdateOp (+0x170), 93 Activated (+0x174), 94 DeActivated (+0x178),
// so classes can be grouped by whether they keep the engine defaults (e.g. a no-op DeActivated
// means no automatic output activation). Finds the vtable like ClassDecomp.java.
// -postScript KismetSlots.java <out_file> <Class> [<Class>...]   (research helper)
//@category Dishonored

import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.mem.*;
import ghidra.program.model.scalar.Scalar;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.util.*;

public class KismetSlots extends GhidraScript {
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

    Address record(String cls) throws Exception {
        byte[] pat = (cls + "\0").getBytes(StandardCharsets.UTF_16LE);
        Address s = currentProgram.getMinAddress();
        while ((s = mem.findBytes(s, pat, null, true, monitor)) != null) {
            for (Address r : rawRefs(s)) {
                MemoryBlock b = mem.getBlock(r);
                if (b != null && !b.isExecute()) return r;
            }
            s = s.add(2);
        }
        return null;
    }

    int nslots(Address vt) {
        int i = 0;
        for (; i < 400; i++) {
            Address p = ptr(vt.add(i * 4));
            if (!isCode(p)) break;
            if (i > 0 && getReferencesTo(vt.add(i * 4)).length > 0) break;
        }
        return i;
    }

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
                        if (mem.contains(v) && !isCode(v) && isCode(ptr(v)) && isCode(ptr(v.add(4))) && isCode(ptr(v.add(8))) && (best == null || nslots(v) > nslots(best))) best = v;
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

    Function ctorOf(Address rec) {
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
        mem = currentProgram.getMemory();
        try (Writer w = new OutputStreamWriter(new FileOutputStream(args[0]), StandardCharsets.UTF_8)) {
            for (int i = 1; i < args.length; i++) {
                String cls = args[i];
                Address rec = record(cls);
                Address vt = rec == null ? null : vtableOf(ctorOf(rec), 0);
                if (vt == null) {
                    w.write(cls + "\t?\n");
                    continue;
                }
                StringBuilder sb = new StringBuilder(cls + "\t" + vt);
                for (int s : new int[] { 92, 93, 94 }) {
                    Address p = ptr(vt.add(s * 4));
                    sb.append("\t").append(s).append("=").append(p);
                }
                w.write(sb + "\n");
            }
        }
    }
}
