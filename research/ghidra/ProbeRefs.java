// Print where an ASCII/UTF-16 string is referenced from and the dwords around each reference.
// -postScript ProbeRefs.java <string> [<string>...]   (research helper)
//@category Dishonored

import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.mem.*;
import ghidra.program.model.symbol.*;
import java.nio.charset.StandardCharsets;
import java.util.*;

public class ProbeRefs extends GhidraScript {
    @Override
    public void run() throws Exception {
        Memory mem = currentProgram.getMemory();
        for (String s : getScriptArgs()) {
            for (byte[] pat : new byte[][] { (s + "\0").getBytes(StandardCharsets.US_ASCII), (s + "\0").getBytes(StandardCharsets.UTF_16LE) }) {
                Address a = mem.findBytes(currentProgram.getMinAddress(), pat, null, true, monitor);
                if (a == null) continue;
                println(s + " @ " + a + " (" + (pat.length == s.length() + 1 ? "ascii" : "utf16") + ")");
                // Raw dword search (works even where analysis made no reference).
                byte[] le = new byte[] { (byte) a.getOffset(), (byte) (a.getOffset() >> 8), (byte) (a.getOffset() >> 16), (byte) (a.getOffset() >> 24) };
                Address p = currentProgram.getMinAddress();
                int n = 0;
                while ((p = mem.findBytes(p, le, null, true, monitor)) != null && n < 6) {
                    StringBuilder sb = new StringBuilder("  dword at " + p + " [" + (mem.getBlock(p) == null ? "?" : mem.getBlock(p).getName()) + "]:");
                    for (int i = -4; i <= 6; i++) {
                        try {
                            sb.append(String.format(" %08x", mem.getInt(p.add(i * 4))));
                        } catch (Exception e) {
                        }
                    }
                    println(sb.toString());
                    p = p.add(1);
                    n++;
                }
            }
        }
    }
}
