// Find functions that use groups of structure offsets as memory displacements, and report
// at which slot of the given vtables each found function sits.
//
// -postScript G_OffsetGroups.java <out_file> <start hex> <end hex> vt@<addr hex>/<slots> ...
//     <name>@<min hits>/<off hex>+<off hex>+...  ...
//
// A research helper (locating engine physics routines such as the walking / falling / swimming
// physics of the pawn by the fields they touch). Output is derived from retail code: keep it
// out of the repository.
//@category Dishonored

import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.scalar.Scalar;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.util.*;

public class G_OffsetGroups extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        Address start = toAddr(Long.parseLong(a[1], 16));
        Address end = toAddr(Long.parseLong(a[2], 16));
        List<String> names = new ArrayList<>();
        List<Integer> mins = new ArrayList<>();
        List<Set<Long>> groups = new ArrayList<>();
        Map<Long, List<String>> slotOf = new HashMap<>();
        ghidra.program.model.mem.Memory mem = currentProgram.getMemory();
        for (int i = 3; i < a.length; i++) {
            String s = a[i];
            if (s.startsWith("vt@")) {
                String[] p = s.substring(3).split("/");
                long base = Long.parseLong(p[0], 16);
                int n = Integer.parseInt(p[1]);
                for (int k = 0; k < n; k++) {
                    long f = mem.getInt(toAddr(base + 4L * k)) & 0xffffffffL;
                    slotOf.computeIfAbsent(f, x -> new ArrayList<>()).add(p[0] + "[" + k + "]");
                }
                continue;
            }
            String[] kv = s.split("@");
            String[] p = kv[1].split("/");
            Set<Long> offs = new HashSet<>();
            for (String o : p[1].split("[+]")) offs.add(Long.parseLong(o, 16));
            names.add(kv[0]);
            mins.add(Integer.parseInt(p[0]));
            groups.add(offs);
        }
        try (Writer w = new OutputStreamWriter(new FileOutputStream(a[0]), StandardCharsets.UTF_8)) {
            FunctionIterator it = currentProgram.getFunctionManager().getFunctions(start, true);
            while (it.hasNext()) {
                Function f = it.next();
                if (f.getEntryPoint().compareTo(end) >= 0) break;
                Set<Long> used = new HashSet<>();
                InstructionIterator ii = currentProgram.getListing().getInstructions(f.getBody(), true);
                while (ii.hasNext()) {
                    Instruction ins = ii.next();
                    for (int o = 0; o < ins.getNumOperands(); o++) {
                        if ((ins.getOperandType(o) & ghidra.program.model.lang.OperandType.DYNAMIC) == 0) continue;
                        for (Object x : ins.getOpObjects(o)) {
                            if (x instanceof Scalar) used.add(((Scalar) x).getUnsignedValue());
                        }
                    }
                }
                for (int g = 0; g < groups.size(); g++) {
                    Set<Long> hit = new TreeSet<>();
                    for (Long o : groups.get(g)) if (used.contains(o)) hit.add(o);
                    if (hit.size() >= mins.get(g)) {
                        StringBuilder sb = new StringBuilder();
                        for (Long h : hit) sb.append(Long.toHexString(h)).append(' ');
                        long ep = f.getEntryPoint().getOffset();
                        List<String> sl = slotOf.getOrDefault(ep, Collections.emptyList());
                        w.write(names.get(g) + " " + f.getEntryPoint() + " size " + f.getBody().getNumAddresses() + " hits " + hit.size() + ": " + sb + " slots " + sl + "\n");
                    }
                }
            }
        }
    }
}
