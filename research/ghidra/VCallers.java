// Decompile every function in an address range that makes a virtual call through a given
// vtable offset (`CALL dword ptr [reg + off]`). Finds who calls a virtual like Evaluate().
// -postScript VCallers.java <out_file> <offset hex> <start hex> <end hex>   (research helper)
//@category Dishonored

import ghidra.app.decompiler.*;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.*;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.util.*;

public class VCallers extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        String off = "0x" + Long.toHexString(Long.parseLong(a[1].replace("0x", ""), 16));
        AddressSet range = new AddressSet(toAddr(Long.parseLong(a[2], 16)), toAddr(Long.parseLong(a[3], 16)));
        Set<Function> fs = new LinkedHashSet<>();
        InstructionIterator it = currentProgram.getListing().getInstructions(range, true);
        while (it.hasNext()) {
            Instruction ins = it.next();
            String mn = ins.getMnemonicString();
            if (!mn.equals("CALL") && !mn.equals("MOV")) continue;
            String op = ins.toString();
            if (op.contains("+ " + off + "]") && (mn.equals("CALL") || op.startsWith("MOV E") && (ins.getNext() != null && ins.getNext().getMnemonicString().equals("CALL")))) {
                Function f = getFunctionContaining(ins.getAddress());
                if (f != null) fs.add(f);
            }
        }
        DecompInterface dec = new DecompInterface();
        dec.openProgram(currentProgram);
        try (Writer w = new OutputStreamWriter(new FileOutputStream(a[0]), StandardCharsets.UTF_8)) {
            for (Function f : fs) {
                DecompileResults r = dec.decompileFunction(f, 120, monitor);
                w.write("\n// ===== " + f.getName() + " @ " + f.getEntryPoint() + "\n");
                w.write(r != null && r.decompileCompleted() ? r.getDecompiledFunction().getC() : "// failed\n");
            }
        }
        println(fs.size() + " functions call through +" + off);
    }
}
