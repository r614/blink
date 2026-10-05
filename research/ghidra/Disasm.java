// Print the instructions of the functions at the given addresses (research helper).
// -postScript Disasm.java <out_file> <addr hex> ...
//@category Dishonored

import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.*;
import java.io.*;
import java.nio.charset.StandardCharsets;

public class Disasm extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        try (Writer w = new OutputStreamWriter(new FileOutputStream(a[0]), StandardCharsets.UTF_8)) {
            for (int i = 1; i < a.length; i++) {
                Function f = getFunctionContaining(toAddr(Long.parseLong(a[i].replace("0x", ""), 16)));
                if (f == null) {
                    w.write("// no function at " + a[i] + "\n");
                    continue;
                }
                w.write("\n// ===== " + f.getName() + " @ " + f.getEntryPoint() + "\n");
                InstructionIterator it = currentProgram.getListing().getInstructions(f.getBody(), true);
                while (it.hasNext()) {
                    Instruction ins = it.next();
                    w.write(ins.getAddress() + "  " + ins + "\n");
                }
            }
        }
        println("done");
    }
}
