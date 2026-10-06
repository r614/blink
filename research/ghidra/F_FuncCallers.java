// List the callers of functions given by name or address, decompile the functions themselves
// and (optionally) up to <max> of their callers. Finds who consumes or seeds a runtime helper
// such as the C runtime random generator. (research helper; output is derived from retail
// code: keep it out of the repository)
// -postScript F_FuncCallers.java <out_file> <max decompiled callers> <name | addr hex> ...
//@category Dishonored

import ghidra.app.decompiler.*;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import java.io.*;
import java.util.*;

public class F_FuncCallers extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        PrintWriter out = new PrintWriter(new FileWriter(a[0]));
        int max = Integer.parseInt(a[1]);
        DecompInterface di = new DecompInterface();
        di.openProgram(currentProgram);
        FunctionManager fm = currentProgram.getFunctionManager();
        for (int i = 2; i < a.length; i++) {
            List<Function> targets = new ArrayList<>();
            if (a[i].matches("[0-9a-fA-F]{6,8}")) {
                Function f = fm.getFunctionAt(toAddr(Long.parseLong(a[i], 16)));
                if (f != null) targets.add(f);
            } else {
                for (Function f : fm.getFunctions(true)) {
                    if (f.getName().equals(a[i])) targets.add(f);
                }
                for (Symbol s : currentProgram.getSymbolTable().getSymbols(a[i])) {
                    Function f = fm.getFunctionAt(s.getAddress());
                    if (f != null && !targets.contains(f)) targets.add(f);
                    if (f == null) out.println("// symbol " + a[i] + " at " + s.getAddress() + " (not a function)");
                }
            }
            for (Function t : targets) {
                out.println("// ===== " + a[i] + " = " + t.getName() + " @ " + t.getEntryPoint() + (t.isThunk() ? " (thunk -> " + t.getThunkedFunction(true).getEntryPoint() + ")" : ""));
                DecompileResults r = di.decompileFunction(t, 60, monitor);
                if (r.decompileCompleted()) out.println(r.getDecompiledFunction().getC());
                Set<Function> callers = new LinkedHashSet<>();
                List<Address> sites = new ArrayList<>();
                Set<Address> entries = new LinkedHashSet<>();
                entries.add(t.getEntryPoint());
                // Thunks pointing at this function count as the same target.
                for (Function f : fm.getFunctions(true)) {
                    if (f.isThunk() && f.getThunkedFunction(true) == t) entries.add(f.getEntryPoint());
                }
                for (Address e : entries) {
                    for (Reference ref : getReferencesTo(e)) {
                        Function c = fm.getFunctionContaining(ref.getFromAddress());
                        sites.add(ref.getFromAddress());
                        if (c != null) callers.add(c);
                    }
                }
                // Raw pointers to the entry (vtables, function tables).
                byte[] le = new byte[4];
                long eo = t.getEntryPoint().getOffset();
                for (int k = 0; k < 4; k++) le[k] = (byte) (eo >> (8 * k));
                Address p = currentProgram.getMinAddress();
                while ((p = currentProgram.getMemory().findBytes(p, le, null, true, monitor)) != null) {
                    out.println("// raw pointer at " + p);
                    p = p.add(1);
                }
                out.println("// " + sites.size() + " references from " + callers.size() + " functions:");
                for (Function c : callers) out.println("//   " + c.getName() + " @ " + c.getEntryPoint());
                int n = 0;
                for (Function c : callers) {
                    if (n++ >= max) break;
                    DecompileResults cr = di.decompileFunction(c, 60, monitor);
                    out.println("// ----- caller " + c.getName() + " @ " + c.getEntryPoint());
                    if (cr.decompileCompleted()) out.println(cr.getDecompiledFunction().getC());
                }
            }
        }
        out.close();
    }
}
