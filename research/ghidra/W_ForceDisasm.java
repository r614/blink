// Disassemble code that the analysis left undefined (e.g. static initialisers) and print it,
// with the dwords each instruction references shown as float / int. Research helper (the
// read-only project's changes are discarded); output is derived from retail code.
//
// -postScript W_ForceDisasm.java <out_file> <start hex>:<length> ...
//@category Dishonored

import ghidra.app.cmd.disassemble.DisassembleCommand;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.mem.*;
import ghidra.program.model.scalar.Scalar;
import java.io.*;
import java.nio.charset.StandardCharsets;

public class W_ForceDisasm extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        PrintWriter out = new PrintWriter(new OutputStreamWriter(new FileOutputStream(a[0]), StandardCharsets.UTF_8));
        Memory mem = currentProgram.getMemory();
        for (int i = 1; i < a.length; i++) {
            String[] r = a[i].split(":");
            Address s = toAddr(Long.parseLong(r[0], 16));
            int len = Integer.parseInt(r[1]);
            AddressSet set = new AddressSet(s, s.add(len - 1));
            new DisassembleCommand(set, null, false).applyTo(currentProgram, monitor);
            out.println("== " + s);
            Instruction ins = getInstructionAt(s);
            while (ins != null && ins.getAddress().compareTo(s.add(len)) < 0) {
                StringBuilder b = new StringBuilder(ins.getAddress() + ": " + ins);
                for (int op = 0; op < ins.getNumOperands(); op++) {
                    for (Object o : ins.getOpObjects(op)) {
                        if (o instanceof Address || o instanceof Scalar) {
                            long v = o instanceof Address ? ((Address) o).getOffset() : ((Scalar) o).getUnsignedValue();
                            Address d = toAddr(v);
                            if (mem.contains(d)) {
                                try {
                                    int x = mem.getInt(d);
                                    b.append(String.format("   [%08x]=%08x f=%g", v, x, Float.intBitsToFloat(x)));
                                } catch (Exception e) {
                                }
                            }
                        }
                    }
                }
                out.println(b);
                ins = ins.getNext();
            }
        }
        out.close();
    }
}
