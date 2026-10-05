// Find the native functions that use a string, including through a static FName built
// from it (UE3 code writes `static FName N(TEXT("..."))` or initialises a global FName in a
// static initializer, then reads the global). For research.
//
// -postScript StrUsers.java <out_file> <decompile 0|1> <string> [<string>...]
//
// For each string: its addresses (ASCII and UTF-16), the instructions referencing it, the
// global the surrounding code stores the FName into (the address loaded into ECX or pushed
// right before/after), and every function reading that global. With decompile=1 the user
// functions are decompiled into <out_file>. Output is derived from retail code: research
// only, never commit it.
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

public class StrUsers extends GhidraScript {
    Memory mem;

    List<Address> rawCodeRefs(Address target) throws Exception {
        List<Address> out = new ArrayList<>();
        long o = target.getOffset();
        byte[] le = { (byte) o, (byte) (o >> 8), (byte) (o >> 16), (byte) (o >> 24) };
        Address p = currentProgram.getMinAddress();
        while ((p = mem.findBytes(p, le, null, true, monitor)) != null) {
            MemoryBlock b = mem.getBlock(p);
            if (b != null && b.isExecute()) out.add(p);
            p = p.add(1);
        }
        return out;
    }

    Instruction insAt(Address a) {
        Instruction i = getInstructionContaining(a);
        return i;
    }

    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        mem = currentProgram.getMemory();
        boolean doDec = a[1].equals("1");
        DecompInterface dec = new DecompInterface();
        dec.openProgram(currentProgram);
        StringBuilder out = new StringBuilder();
        Set<Function> done = new HashSet<>();
        for (int k = 2; k < a.length; k++) {
            String s = a[k];
            for (byte[] pat : new byte[][] { (s + "\0").getBytes(StandardCharsets.US_ASCII), (s + "\0").getBytes(StandardCharsets.UTF_16LE) }) {
                Address sa = currentProgram.getMinAddress();
                while ((sa = mem.findBytes(sa, pat, null, true, monitor)) != null) {
                    // whole-string match: previous char must be a terminator
                    boolean wide = pat.length != s.length() + 1;
                    boolean ok = true;
                    try {
                        if (wide) ok = mem.getShort(sa.subtract(2)) == 0; else ok = mem.getByte(sa.subtract(1)) == 0;
                    } catch (Exception e) {
                    }
                    Address here = sa;
                    sa = sa.add(1);
                    if (!ok) continue;
                    println("STR " + s + " @ " + here + (wide ? " utf16" : " ascii"));
                    Set<Address> globals = new LinkedHashSet<>();
                    Set<Function> users = new LinkedHashSet<>();
                    for (Address r : rawCodeRefs(here)) {
                        Instruction ins = insAt(r);
                        if (ins == null) continue;
                        Function f = getFunctionContaining(ins.getAddress());
                        users.add(f);
                        // look at neighbouring instructions for a data global (FName storage)
                        Instruction c = ins;
                        for (int j = 0; j < 4 && c != null; j++) c = c.getPrevious();
                        for (int j = 0; j < 10 && c != null; j++, c = c.getNext()) {
                            for (int op = 0; op < c.getNumOperands(); op++) {
                                for (Object o : c.getOpObjects(op)) {
                                    if (o instanceof Scalar) {
                                        Address g = toAddr(((Scalar) o).getUnsignedValue());
                                        MemoryBlock b = mem.getBlock(g);
                                        if (b != null && !b.isExecute() && !g.equals(here) && b.isWrite()) globals.add(g);
                                    }
                                }
                            }
                        }
                    }
                    for (Function f : users) println("  direct user " + (f == null ? "?" : f.getName() + " @ " + f.getEntryPoint()));
                    for (Address g : globals) {
                        List<Address> gr = rawCodeRefs(g);
                        Set<Function> gu = new LinkedHashSet<>();
                        for (Address r : gr) {
                            Function f = getFunctionContaining(r);
                            if (f != null) gu.add(f);
                        }
                        if (gu.size() > 40) {
                            println("  global " + g + ": " + gu.size() + " users (skipped)");
                            continue;
                        }
                        for (Function f : gu) {
                            println("  global " + g + " user " + f.getName() + " @ " + f.getEntryPoint());
                            users.add(f);
                        }
                    }
                    if (doDec) {
                        for (Function f : users) {
                            if (f == null || !done.add(f)) continue;
                            if (f.getBody().getNumAddresses() > 15000) {
                                out.append("// skipped large ").append(f.getName()).append("\n");
                                continue;
                            }
                            DecompileResults r = dec.decompileFunction(f, 180, monitor);
                            out.append("\n// ===== [").append(s).append("] ").append(f.getName()).append(" @ ").append(f.getEntryPoint()).append("\n");
                            out.append(r != null && r.decompileCompleted() ? r.getDecompiledFunction().getC() : "// failed\n");
                        }
                    }
                }
            }
        }
        if (doDec) {
            try (Writer w = new OutputStreamWriter(new FileOutputStream(a[0]), StandardCharsets.UTF_8)) {
                w.write(out.toString());
            }
        }
    }
}
