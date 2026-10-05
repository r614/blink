// Map the hard-coded FName globals of the native code to their strings.
// -postScript FNameMap.java <out_tsv> [<init function hex> ...]
//
// The game registers its native FNames in large static initializers: PUSH <UTF-16 string>,
// CALL FName::FName, then MOV [global],ECX (index) and MOV [global+4],EDX (number). This
// walks every function that calls the FName constructor (or the given ones) and writes
// "global<TAB>string" lines, so decompiled code reading DAT_<global> can be annotated.
//@category Dishonored

import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.mem.*;
import ghidra.program.model.scalar.Scalar;
import ghidra.program.model.symbol.*;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.util.*;

public class FNameMap extends GhidraScript {
    String wstr(Address p) {
        Memory m = currentProgram.getMemory();
        StringBuilder w = new StringBuilder();
        try {
            for (int i = 0; i < 128; i++) {
                short c = m.getShort(p.add(i * 2));
                if (c == 0) break;
                if (c < 32 || c > 126) return null;
                w.append((char) c);
            }
        } catch (Exception e) {
            return null;
        }
        return w.length() > 0 ? w.toString() : null;
    }

    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        Address ctor = toAddr(0x00454fc0L);
        Set<Function> funcs = new LinkedHashSet<>();
        if (a.length > 1) {
            for (int i = 1; i < a.length; i++) funcs.add(getFunctionAt(toAddr(Long.parseLong(a[i], 16))));
        } else {
            for (Reference r : getReferencesTo(ctor)) {
                Function f = getFunctionContaining(r.getFromAddress());
                if (f != null) funcs.add(f);
            }
        }
        Map<Long, String> map = new TreeMap<>();
        for (Function f : funcs) {
            if (f == null) continue;
            String pending = null;
            boolean called = false;
            InstructionIterator it = currentProgram.getListing().getInstructions(f.getBody(), true);
            while (it.hasNext()) {
                Instruction ins = it.next();
                String mn = ins.getMnemonicString();
                if (mn.equals("PUSH")) {
                    for (Object o : ins.getOpObjects(0)) {
                        if (o instanceof Scalar) {
                            long v = ((Scalar) o).getUnsignedValue();
                            Address p = toAddr(v);
                            if (currentProgram.getMemory().contains(p)) {
                                String s = wstr(p);
                                if (s != null) {
                                    pending = s;
                                    called = false;
                                }
                            }
                        }
                    }
                } else if (mn.equals("CALL")) {
                    Address t = ins.getAddress(0);
                    if (t != null && t.equals(ctor)) called = pending != null;
                } else if (mn.equals("MOV") && called && ins.getNumOperands() == 2) {
                    // MOV dword ptr [abs], reg
                    Object[] dst = ins.getOpObjects(0);
                    if (dst.length == 1 && dst[0] instanceof Scalar && ins.getOperandRefType(0).isWrite()) {
                        long g = ((Scalar) dst[0]).getUnsignedValue();
                        map.putIfAbsent(g, pending);
                        pending = null;
                        called = false;
                    } else if (dst.length == 1 && dst[0] instanceof Address) {
                        map.putIfAbsent(((Address) dst[0]).getOffset(), pending);
                        pending = null;
                        called = false;
                    }
                }
            }
        }
        try (Writer w = new OutputStreamWriter(new FileOutputStream(a[0]), StandardCharsets.UTF_8)) {
            for (Map.Entry<Long, String> e : map.entrySet()) w.write(String.format("%08x\t%s\n", e.getKey(), e.getValue()));
        }
        println("mapped " + map.size() + " FName globals from " + funcs.size() + " functions");
    }
}
