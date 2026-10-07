// Find functions that use a structure offset as a memory displacement and also an immediate
// operand from a given set (e.g. the code that tests trace-flag bits against a primitive's
// `m_CollisionTraceTypes` at +0x140).
//
// -postScript CA_ImmScan.java <start hex> <end hex> <disp hex> <imm hex>...   (research helper)
//@category Dishonored

import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.scalar.Scalar;
import java.util.*;

public class CA_ImmScan extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        Address start = toAddr(Long.parseLong(a[0], 16));
        Address end = toAddr(Long.parseLong(a[1], 16));
        long disp = Long.parseLong(a[2], 16);
        Set<Long> imms = new HashSet<>();
        for (int i = 3; i < a.length; i++) imms.add(Long.parseLong(a[i], 16));
        FunctionIterator it = currentProgram.getFunctionManager().getFunctions(start, true);
        while (it.hasNext()) {
            Function f = it.next();
            if (f.getEntryPoint().compareTo(end) >= 0) break;
            boolean hasDisp = false;
            Set<Long> hit = new TreeSet<>();
            InstructionIterator ii = currentProgram.getListing().getInstructions(f.getBody(), true);
            while (ii.hasNext()) {
                Instruction ins = ii.next();
                for (int o = 0; o < ins.getNumOperands(); o++) {
                    boolean dyn = (ins.getOperandType(o) & ghidra.program.model.lang.OperandType.DYNAMIC) != 0;
                    for (Object x : ins.getOpObjects(o)) {
                        if (x instanceof Scalar) {
                            long v = ((Scalar) x).getUnsignedValue();
                            if (dyn && v == disp) hasDisp = true;
                            if (!dyn && imms.contains(v)) hit.add(v);
                        }
                    }
                }
            }
            // disp 0: no displacement required, at least three of the immediates.
            if (disp == 0 ? hit.size() >= 3 : (hasDisp && !hit.isEmpty())) {
                StringBuilder sb = new StringBuilder();
                for (Long h : hit) sb.append(Long.toHexString(h)).append(' ');
                println(f.getEntryPoint() + " " + f.getName() + " imms: " + sb);
            }
        }
    }
}
