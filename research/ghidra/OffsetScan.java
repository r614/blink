// Find functions that use every given structure offset as a memory displacement.
//
// analyzeHeadless ... -postScript OffsetScan.java <start hex> <end hex> <min hits> <off hex>...
//
// Lists the functions in [start, end) whose instructions use at least <min hits> of the
// offsets as `[reg + off]` displacements: a way to find the code that reads a given tweak
// class (its property offsets come from `class_layout`). Research helper.
//@category Dishonored

import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.scalar.Scalar;
import java.util.*;

public class OffsetScan extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        Address start = toAddr(Long.parseLong(a[0], 16));
        Address end = toAddr(Long.parseLong(a[1], 16));
        int min = Integer.parseInt(a[2]);
        Set<Long> offs = new HashSet<>();
        for (int i = 3; i < a.length; i++) offs.add(Long.parseLong(a[i], 16));
        FunctionIterator it = currentProgram.getFunctionManager().getFunctions(start, true);
        while (it.hasNext()) {
            Function f = it.next();
            if (f.getEntryPoint().compareTo(end) >= 0) break;
            Set<Long> hit = new TreeSet<>();
            InstructionIterator ii = currentProgram.getListing().getInstructions(f.getBody(), true);
            while (ii.hasNext()) {
                Instruction ins = ii.next();
                for (int o = 0; o < ins.getNumOperands(); o++) {
                    if ((ins.getOperandType(o) & ghidra.program.model.lang.OperandType.DYNAMIC) == 0) continue;
                    for (Object x : ins.getOpObjects(o)) {
                        if (x instanceof Scalar) {
                            long v = ((Scalar) x).getUnsignedValue();
                            if (offs.contains(v)) hit.add(v);
                        }
                    }
                }
            }
            if (hit.size() >= min) {
                StringBuilder sb = new StringBuilder();
                for (Long h : hit) sb.append(Long.toHexString(h)).append(' ');
                println(f.getEntryPoint() + " " + f.getName() + " hits " + hit.size() + ": " + sb);
            }
        }
    }
}
