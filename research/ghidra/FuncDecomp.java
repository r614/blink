// Decompile functions of the original executable, with their callees, for research.
//
// analyzeHeadless ... -postScript FuncDecomp.java <out_file> <depth> <addr hex | function name> ...
//
// Writes every requested function and its callees down to <depth> levels into <out_file>
// (skipping functions larger than 6000 bytes below the top level, and thunks). After each
// function, the data it reads is listed: UTF-16/ASCII strings and dwords shown as float and
// int, so constants (`_DAT_xxxx` in the decompiler output) can be read. Output is derived from
// retail code: research only, keep it out of the repository.
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

public class FuncDecomp extends GhidraScript {
    DecompInterface dec;
    Memory mem;

    Function func(Address a) {
        Function f = getFunctionContaining(a);
        if (f == null) {
            try {
                f = createFunction(a, null);
            } catch (Exception e) {
            }
        }
        return f;
    }

    boolean isCodePtr(Address a) {
        try {
            Address p = toAddr(mem.getInt(a) & 0xffffffffL);
            MemoryBlock b = mem.getBlock(p);
            return b != null && b.isExecute();
        } catch (Exception e) {
            return false;
        }
    }

    String str(Address a) {
        try {
            // UTF-16
            StringBuilder w = new StringBuilder();
            for (int i = 0; i < 80; i++) {
                int c = mem.getShort(a.add(i * 2)) & 0xffff;
                if (c == 0) break;
                if (c < 0x20 || c > 0x7e) { w.setLength(0); break; }
                w.append((char) c);
            }
            if (w.length() >= 3) return "L\"" + w + "\"";
            StringBuilder s = new StringBuilder();
            for (int i = 0; i < 120; i++) {
                int c = mem.getByte(a.add(i)) & 0xff;
                if (c == 0) break;
                if (c < 0x20 || c > 0x7e) { s.setLength(0); break; }
                s.append((char) c);
            }
            if (s.length() >= 4) return "\"" + s + "\"";
        } catch (Exception e) {
        }
        return null;
    }

    String dataNote(Address a) {
        String s = str(a);
        if (s != null) return s;
        try {
            int v = mem.getInt(a);
            float f = Float.intBitsToFloat(v);
            long q = mem.getLong(a);
            double d = Double.longBitsToDouble(q);
            String r = String.format("dword %08x int %d float %g", v, v, f);
            if (Math.abs(d) > 1e-6 && Math.abs(d) < 1e7) r += String.format(" double %g", d);
            return r;
        } catch (Exception e) {
            return null;
        }
    }

    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        mem = currentProgram.getMemory();
        dec = new DecompInterface();
        dec.openProgram(currentProgram);
        int depth = Integer.parseInt(args[1]);
        Deque<Object[]> q = new ArrayDeque<>();
        for (int i = 2; i < args.length; i++) {
            Function f = null;
            String a = args[i];
            if (a.startsWith("vt:")) {
                // Every slot of the vtable(s) holding a pointer to the function.
                long o = Long.parseLong(a.substring(3), 16);
                byte[] le = { (byte) o, (byte) (o >> 8), (byte) (o >> 16), (byte) (o >> 24) };
                Address p = currentProgram.getMinAddress();
                while ((p = mem.findBytes(p, le, null, true, monitor)) != null) {
                    MemoryBlock b = mem.getBlock(p);
                    if (b != null && !b.isExecute()) {
                        Address s = p;
                        while (isCodePtr(s.subtract(4)) && getReferencesTo(s).length == 0) s = s.subtract(4);
                        int slot = 0;
                        for (Address e = s; isCodePtr(e) && (e.equals(s) || getReferencesTo(e).length == 0); e = e.add(4), slot++) {
                            Function g = func(toAddr(mem.getInt(e) & 0xffffffffL));
                            if (g != null) { q.add(new Object[] { g, 0 }); println(a + " vtable " + s + " slot " + slot + " (+0x" + Integer.toHexString(slot * 4) + ") " + g.getEntryPoint()); }
                        }
                    }
                    p = p.add(1);
                }
                continue;
            }
            if (a.startsWith("ref:")) {
                // Every function whose code references the address (analysis references).
                Address t = toAddr(Long.parseLong(a.substring(4), 16));
                Set<Address> added = new HashSet<>();
                for (Reference r : getReferencesTo(t)) {
                    Function g = getFunctionContaining(r.getFromAddress());
                    if (g != null && added.add(g.getEntryPoint())) { q.add(new Object[] { g, 0 }); println(a + " used by " + g.getEntryPoint()); }
                }
                continue;
            }
            if (a.startsWith("str:")) {
                // Every function referencing the (UTF-16 or ASCII) string.
                String s = a.substring(4);
                for (byte[] pat : new byte[][] { (s + "\0").getBytes(StandardCharsets.UTF_16LE), (s + "\0").getBytes(StandardCharsets.US_ASCII) }) {
                    Address at = mem.findBytes(currentProgram.getMinAddress(), pat, null, true, monitor);
                    if (at == null) continue;
                    long o = at.getOffset();
                    byte[] le = { (byte) o, (byte) (o >> 8), (byte) (o >> 16), (byte) (o >> 24) };
                    Address p = currentProgram.getMinAddress();
                    while ((p = mem.findBytes(p, le, null, true, monitor)) != null) {
                        MemoryBlock b = mem.getBlock(p);
                        if (b != null && b.isExecute()) {
                            Function g = getFunctionContaining(p);
                            if (g != null) { q.add(new Object[] { g, 0 }); println(s + " used by " + g.getEntryPoint()); }
                        } else {
                            println(s + " referenced from data " + p);
                        }
                        p = p.add(1);
                    }
                }
                continue;
            }
            if (a.matches("(0x)?[0-9a-fA-F]{6,8}")) {
                f = func(toAddr(Long.parseLong(a.replace("0x", ""), 16)));
            } else {
                for (Function g : currentProgram.getFunctionManager().getFunctions(true)) {
                    if (g.getName().equals(a)) { f = g; break; }
                }
            }
            if (f == null) { println("not found: " + a); continue; }
            q.add(new Object[] { f, 0 });
        }
        Set<Address> seen = new HashSet<>();
        StringBuilder sb = new StringBuilder();
        int n = 0;
        while (!q.isEmpty() && n < 400) {
            Object[] e = q.poll();
            Function f = (Function) e[0];
            int d = (Integer) e[1];
            if (!seen.add(f.getEntryPoint())) continue;
            if (d > 0 && (f.isThunk() || f.getBody().getNumAddresses() > 6000)) continue;
            DecompileResults r = dec.decompileFunction(f, 180, monitor);
            sb.append("\n// ===== ").append(f.getName()).append(" @ ").append(f.getEntryPoint()).append(" (depth ").append(d).append(")\n");
            if (r == null || !r.decompileCompleted()) { sb.append("// decompile failed\n"); continue; }
            sb.append(r.getDecompiledFunction().getC());
            // Data read by the function.
            TreeMap<Address, String> notes = new TreeMap<>();
            InstructionIterator it = currentProgram.getListing().getInstructions(f.getBody(), true);
            while (it.hasNext()) {
                Instruction ins = it.next();
                for (Reference ref : ins.getReferencesFrom()) {
                    Address to = ref.getToAddress();
                    MemoryBlock b = mem.getBlock(to);
                    if (b == null || b.isExecute() || !ref.getReferenceType().isData()) continue;
                    String note = dataNote(to);
                    if (note != null) notes.put(to, note);
                }
            }
            for (Map.Entry<Address, String> m : notes.entrySet()) sb.append("//   data ").append(m.getKey()).append(": ").append(m.getValue()).append("\n");
            n++;
            if (d < depth) {
                for (Function c : f.getCalledFunctions(monitor)) q.add(new Object[] { c, d + 1 });
            }
        }
        try (Writer w = new OutputStreamWriter(new FileOutputStream(args[0]), StandardCharsets.UTF_8)) {
            w.write(sb.toString());
        }
        println("wrote " + n + " functions to " + args[0]);
    }
}
