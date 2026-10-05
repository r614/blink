// List instructions that write a bit mask into an object field: `AND [reg+off], mask` or
// `OR [reg+off], mask` (e.g. who clears SequenceOp.bAutoActivateOutputLinks, bit 0x10 of
// +0x8c), with the containing function.
// -postScript FindFieldWrites.java <out_file> <offset hex> <mask hex> [<start hex> <end hex>]
// (research helper; output is derived from retail code: keep it out of the repository)
//@category Dishonored

import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.scalar.Scalar;
import java.io.*;
import java.nio.charset.StandardCharsets;

public class FindFieldWrites extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        long off = Long.parseLong(a[1].replace("0x", ""), 16);
        long mask = Long.parseLong(a[2].replace("0x", ""), 16);
        long inv = (~mask) & 0xffffffffL;
        AddressSetView range = currentProgram.getMemory().getExecuteSet();
        if (a.length >= 5) {
            range = new AddressSet(toAddr(Long.parseLong(a[3], 16)), toAddr(Long.parseLong(a[4], 16)));
        }
        try (Writer w = new OutputStreamWriter(new FileOutputStream(a[0]), StandardCharsets.UTF_8)) {
            InstructionIterator it = currentProgram.getListing().getInstructions(range, true);
            while (it.hasNext() && !monitor.isCancelled()) {
                Instruction ins = it.next();
                String m = ins.getMnemonicString();
                if (!m.equals("AND") && !m.equals("OR") && !m.equals("XOR") && !m.equals("TEST")) continue;
                if (ins.getNumOperands() != 2) continue;
                String op0 = ins.getDefaultOperandRepresentation(0);
                if (!op0.contains("+ 0x" + Long.toHexString(off) + "]") && !op0.contains("+0x" + Long.toHexString(off) + "]")) continue;
                Object[] o1 = ins.getOpObjects(1);
                if (o1.length != 1 || !(o1[0] instanceof Scalar)) continue;
                long v = ((Scalar) o1[0]).getUnsignedValue() & 0xffffffffL;
                long vb = v & 0xff;
                boolean hit = v == mask || v == inv || (mask <= 0xff && (vb == mask || vb == ((~mask) & 0xff)));
                if (!hit) continue;
                Function f = getFunctionContaining(ins.getAddress());
                w.write(ins.getAddress() + "  " + ins + "   in " + (f == null ? "?" : f.getName() + " @ " + f.getEntryPoint()) + "\n");
            }
        }
    }
}
