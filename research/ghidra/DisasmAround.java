// Print the instructions around addresses, or the instructions referencing a value.
// -postScript DisasmAround.java <addr hex>[:<before>:<after>] ...      (research helper)
// -postScript DisasmAround.java refs <value hex> ...  (each code site whose bytes contain the value)
//@category Dishonored

import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.mem.*;

public class DisasmAround extends GhidraScript {
    void around(Address at, int before, int after) {
        Instruction i = getInstructionContaining(at);
        if (i == null) {
            println(at + ": no instruction");
            return;
        }
        Function f = getFunctionContaining(at);
        println("-- " + at + " in " + (f == null ? "?" : f.getName() + " @ " + f.getEntryPoint()));
        Instruction c = i;
        for (int k = 0; k < before && c.getPrevious() != null; k++) c = c.getPrevious();
        for (int k = 0; k < before + after + 1 && c != null; k++, c = c.getNext()) {
            println((c.getAddress().equals(i.getAddress()) ? "> " : "  ") + c.getAddress() + "  " + c);
        }
    }

    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        Memory mem = currentProgram.getMemory();
        if (a[0].equals("refs")) {
            for (int k = 1; k < a.length; k++) {
                long o = Long.parseLong(a[k], 16);
                byte[] le = { (byte) o, (byte) (o >> 8), (byte) (o >> 16), (byte) (o >> 24) };
                Address p = currentProgram.getMinAddress();
                int n = 0;
                while ((p = mem.findBytes(p, le, null, true, monitor)) != null && n < 60) {
                    MemoryBlock b = mem.getBlock(p);
                    if (b != null && b.isExecute()) {
                        around(p, 2, 2);
                        n++;
                    }
                    p = p.add(1);
                }
            }
            return;
        }
        for (String s : a) {
            String[] parts = s.split(":");
            around(toAddr(Long.parseLong(parts[0], 16)), parts.length > 1 ? Integer.parseInt(parts[1]) : 6, parts.length > 2 ? Integer.parseInt(parts[2]) : 6);
        }
    }
}
