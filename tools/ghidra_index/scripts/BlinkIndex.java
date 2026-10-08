// Static index of the original executable for the differential oracle (research/dishonored/ORACLE.md,
// phase 3). One headless pass over an analysed program writes plain tables that
// `oracle index build` turns into the queryable index:
//
//   functions.tsv  entry, size, name, symbol source, thunk target
//   calls.tsv      caller entry, kind (d direct / v vtable offset / i import / r register), call site, target
//   strings.tsv    function entry, site, string address, kind (a ascii / u utf-16), text (escaped)
//   natives.tsv    FNativeFunctionLookup tables: table address, exec name, function
//   register.tsv   native registration calls (PUSH func; PUSH index; CALL) and inlined stores
//   native_arrays.tsv  exec function pointers outside the lookup tables (address, function)
//   classes.tsv    class name, record, constructor, primary vtable, slot count, slots (comma separated)
//   ivtables.tsv   class name, every other vtable its constructor stores, slots
//   traces.tsv     trace call sites: callee, caller, site, flags, extent, flags expr, extent expr, statement
//   decomp.c + decomp.idx   every function decompiled (entry, byte offset, byte length)
//   summary.txt    counts and timings
//
// analyzeHeadless <proj_dir> <proj_name> -process Dishonored.exe -noanalysis -readOnly
//     -scriptPath tools/ghidra_index/scripts -postScript BlinkIndex.java <out_dir>
//     [classes:<file with one class name per line>] [traces:<addr>/<kind>+<addr>/<kind>...]
//     [nodecomp] [decomp_timeout:<seconds>]
// (`:`, `/` and `+` because cmd.exe splits arguments at `=` and `,`.)
// Trace kinds: `single` (UWorld::SingleLineCheck: this, Hit, Source, End, Start, TraceFlags,
// Extent, Light) and `multi` (UWorld::MultiLineCheck: this, Mem, End, Start, Extent, TraceFlags,
// Source, Light).
//
// Everything written is derived from retail code: it goes to the oracle cache, never into git.
//@category Dishonored

import ghidra.app.cmd.function.ApplyFunctionSignatureCmd;
import ghidra.app.decompiler.*;
import ghidra.app.decompiler.parallel.*;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.data.*;
import ghidra.program.model.lang.Register;
import ghidra.program.model.listing.*;
import ghidra.program.model.mem.*;
import ghidra.program.model.pcode.*;
import ghidra.program.model.scalar.Scalar;
import ghidra.program.model.symbol.*;
import ghidra.util.task.TaskMonitor;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.util.*;
import java.util.concurrent.atomic.AtomicLong;

public class BlinkIndex extends GhidraScript {
    Memory mem;
    Listing listing;
    FunctionManager fm;
    File outDir;
    StringBuilder summary = new StringBuilder();
    Set<Long> execFuncs = new HashSet<>();

    // ---------------------------------------------------------------- helpers

    Address ptr(Address a) {
        try {
            Address p = toAddr(mem.getInt(a) & 0xffffffffL);
            return mem.contains(p) ? p : null;
        } catch (Exception e) {
            return null;
        }
    }

    boolean isCode(Address p) {
        if (p == null) return false;
        MemoryBlock b = mem.getBlock(p);
        return b != null && b.isExecute();
    }

    boolean isData(Address p) {
        if (p == null) return false;
        MemoryBlock b = mem.getBlock(p);
        return b != null && !b.isExecute() && b.isInitialized();
    }

    Function func(Address a) {
        Function f = fm.getFunctionAt(a);
        if (f == null) {
            try {
                f = createFunction(a, null);
            } catch (Exception e) {
            }
        }
        return f;
    }

    PrintWriter open(String name) throws IOException {
        return new PrintWriter(new BufferedWriter(new OutputStreamWriter(new FileOutputStream(new File(outDir, name)), StandardCharsets.UTF_8), 1 << 20));
    }

    static String hex(long v) {
        return String.format("%08x", v);
    }

    static String hex(Address a) {
        return a == null ? "-" : hex(a.getOffset());
    }

    static String esc(String s) {
        StringBuilder b = new StringBuilder(s.length());
        for (char c : s.toCharArray()) {
            if (c == '\\') b.append("\\\\");
            else if (c == '\t') b.append("\\t");
            else if (c == '\n') b.append("\\n");
            else if (c == '\r') b.append("\\r");
            else if (c < 32) b.append(String.format("\\x%02x", (int) c));
            else b.append(c);
        }
        return b.toString();
    }

    String ascii(Address a, int min) {
        try {
            StringBuilder sb = new StringBuilder();
            for (int i = 0; i < 512; i++) {
                byte b = mem.getByte(a.add(i));
                if (b == 0) return sb.length() >= min ? sb.toString() : null;
                int c = b & 0xff;
                if ((c < 32 && c != 9 && c != 10 && c != 13) || c > 126) return null;
                sb.append((char) c);
            }
        } catch (Exception e) {
        }
        return null;
    }

    String utf16(Address a, int min) {
        try {
            StringBuilder sb = new StringBuilder();
            for (int i = 0; i < 512; i++) {
                int c = (mem.getShort(a.add(2 * i)) & 0xffff);
                if (c == 0) return sb.length() >= min ? sb.toString() : null;
                if ((c < 32 && c != 9 && c != 10 && c != 13) || c > 126) return null;
                sb.append((char) c);
            }
        } catch (Exception e) {
        }
        return null;
    }

    long t0;

    void lap(String what) {
        long t = System.currentTimeMillis();
        String line = String.format("%-12s %8.1f s", what, (t - t0) / 1000.0);
        println(line);
        summary.append(line).append('\n');
        t0 = t;
    }

    // ---------------------------------------------------------------- native lookup tables

    static final java.util.regex.Pattern EXEC = java.util.regex.Pattern.compile("[UA][A-Za-z0-9_]*exec[A-Za-z0-9_]+");

    void nativeTables() throws Exception {
        int tables = 0, natives = 0;
        try (PrintWriter w = open("natives.tsv")) {
            for (MemoryBlock b : mem.getBlocks()) {
                if (!b.isInitialized() || b.isExecute()) continue;
                Address a = b.getStart();
                Address end = b.getEnd().subtract(8);
                while (a.compareTo(end) < 0) {
                    Address n = ptr(a);
                    String s = n == null ? null : ascii(n, 1);
                    Address f = ptr(a.add(4));
                    boolean pair = s != null && EXEC.matcher(s).matches() && isCode(f);
                    if (!pair) {
                        a = a.add(4);
                        continue;
                    }
                    tables++;
                    Address t = a;
                    while (true) {
                        Address nn = ptr(t);
                        String ss = nn == null ? null : ascii(nn, 1);
                        Address ff = ptr(t.add(4));
                        if (ss == null || !isCode(ff) || !EXEC.matcher(ss).matches()) break;
                        w.println(hex(a) + "\t" + ss + "\t" + hex(ff));
                        execFuncs.add(ff.getOffset());
                        try {
                            if (func(ff) != null) currentProgram.getSymbolTable().createLabel(ff, ss, SourceType.ANALYSIS).setPrimary();
                        } catch (Exception e) {
                        }
                        natives++;
                        t = t.add(8);
                    }
                    a = t.add(4);
                }
            }
        }
        summary.append("native tables ").append(tables).append(", natives ").append(natives).append('\n');
    }

    /// Data words that point at an exec function outside the lookup tables: the statically
    /// initialized `GNatives` array (index = (address - base) / 4; the base is voted from the
    /// script's native indices by `oracle index build`).
    void nativeArrays() throws Exception {
        int n = 0;
        try (PrintWriter w = open("native_arrays.tsv")) {
            for (MemoryBlock b : mem.getBlocks()) {
                if (!b.isInitialized() || b.isExecute()) continue;
                long start = b.getStart().getOffset();
                byte[] buf = new byte[(int) b.getSize()];
                b.getBytes(b.getStart(), buf);
                for (int i = 4; i + 4 <= buf.length; i += 4) {
                    long v = (buf[i] & 0xffL) | ((buf[i + 1] & 0xffL) << 8) | ((buf[i + 2] & 0xffL) << 16) | ((buf[i + 3] & 0xffL) << 24);
                    if (!execFuncs.contains(v)) continue;
                    // Not the function half of a {name, func} lookup pair.
                    Address nm = ptr(toAddr(start + i - 4));
                    String s = nm == null ? null : ascii(nm, 1);
                    if (s != null && EXEC.matcher(s).matches()) continue;
                    w.println(hex(start + i) + "\t" + hex(v));
                    n++;
                }
            }
        }
        summary.append("exec pointers outside lookup tables ").append(n).append('\n');
    }

    // ---------------------------------------------------------------- one pass over every function

    Map<Long, String[]> strCache = new HashMap<>();

    /// A string at a data address: {kind, text} or null.
    String[] stringAt(Address a) {
        long k = a.getOffset();
        if (strCache.containsKey(k)) return strCache.get(k);
        String[] r = null;
        if (isData(a)) {
            String u = (k & 1) == 0 ? utf16(a, 2) : null;
            String s = ascii(a, 3);
            if (u != null && (s == null || u.length() >= s.length())) r = new String[] { "u", u };
            else if (s != null) r = new String[] { "a", s };
        }
        strCache.put(k, r);
        return r;
    }

    void functionPass() throws Exception {
        long nf = 0, ncalls = 0, nstr = 0, nreg = 0;
        ReferenceManager rm = currentProgram.getReferenceManager();
        try (PrintWriter wf = open("functions.tsv"); PrintWriter wc = open("calls.tsv"); PrintWriter ws = open("strings.tsv"); PrintWriter wr = open("register.tsv")) {
            FunctionIterator it = fm.getFunctions(true);
            while (it.hasNext()) {
                Function f = it.next();
                if (monitor.isCancelled()) break;
                nf++;
                Function thunk = f.isThunk() ? f.getThunkedFunction(true) : null;
                wf.println(hex(f.getEntryPoint()) + "\t" + f.getBody().getNumAddresses() + "\t" + f.getName(true) + "\t" + f.getSymbol().getSource() + "\t" + (thunk == null ? "-" : hex(thunk.getEntryPoint()) + ":" + thunk.getName(true)));
                if (f.isExternal()) continue;
                String fe = hex(f.getEntryPoint());
                Instruction prev1 = null, prev2 = null;
                ArrayDeque<Instruction> recent = new ArrayDeque<>();
                InstructionIterator ii = listing.getInstructions(f.getBody(), true);
                while (ii.hasNext()) {
                    Instruction ins = ii.next();
                    String mn = ins.getMnemonicString();
                    if (recent.size() > 8) recent.removeLast();
                    // Calls.
                    if (ins.getFlowType().isCall()) {
                        Address[] flows = ins.getFlows();
                        if (flows.length > 0) {
                            for (Address t : flows) {
                                wc.println(fe + "\td\t" + hex(ins.getAddress()) + "\t" + hex(t));
                                ncalls++;
                            }
                        } else {
                            Object[] ops = ins.getOpObjects(0);
                            String kind = "r", target = ins.getDefaultOperandRepresentation(0);
                            Register reg = null;
                            Scalar sc = null;
                            for (Object o : ops) {
                                if (o instanceof Register) reg = (Register) o;
                                if (o instanceof Scalar) sc = (Scalar) o;
                            }
                            boolean memOp = target.contains("[");
                            if (reg != null && memOp) {
                                kind = "v";
                                target = Long.toHexString(sc == null ? 0 : sc.getUnsignedValue());
                            } else if (reg != null) {
                                // CALL reg after MOV reg, [obj_vtable + slot]: a virtual call.
                                for (Instruction p : recent) {
                                    if (p.getNumOperands() < 1 || !reg.equals(p.getRegister(0))) continue;
                                    if (p.getMnemonicString().equals("MOV") && p.getNumOperands() == 2) {
                                        String src = p.getDefaultOperandRepresentation(1);
                                        Register base = null;
                                        Scalar disp = null;
                                        for (Object o : p.getOpObjects(1)) {
                                            if (o instanceof Register) base = (Register) o;
                                            if (o instanceof Scalar) disp = (Scalar) o;
                                        }
                                        if (src.contains("[") && !src.contains("*") && base != null && (disp == null || disp.getUnsignedValue() < 0x4000)) {
                                            kind = "v";
                                            target = Long.toHexString(disp == null ? 0 : disp.getUnsignedValue());
                                        }
                                    }
                                    break;
                                }
                            } else if (reg == null && sc != null) {
                                Address slot = toAddr(sc.getUnsignedValue());
                                Reference[] refs = rm.getReferencesFrom(ins.getAddress());
                                String nm = null;
                                for (Reference r : refs) {
                                    Symbol s = currentProgram.getSymbolTable().getPrimarySymbol(r.getToAddress());
                                    if (s != null && r.getToAddress().isExternalAddress()) nm = s.getName(true);
                                    else if (s != null && nm == null) nm = s.getName(true);
                                }
                                Address p = ptr(slot);
                                if (nm != null) {
                                    kind = "i";
                                    target = nm;
                                } else if (isCode(p)) {
                                    wc.println(fe + "\td\t" + hex(ins.getAddress()) + "\t" + hex(p));
                                    ncalls++;
                                    prev2 = prev1;
                                    prev1 = ins;
                                    recent.addFirst(ins);
                                    continue;
                                } else {
                                    kind = "i";
                                    target = hex(slot);
                                }
                            }
                            wc.println(fe + "\t" + kind + "\t" + hex(ins.getAddress()) + "\t" + target);
                            ncalls++;
                        }
                        // Native registration: PUSH func; PUSH index; CALL GRegisterNative.
                        if (flows.length == 1 && prev1 != null && prev2 != null && prev1.getMnemonicString().equals("PUSH") && prev2.getMnemonicString().equals("PUSH")) {
                            Scalar idx = prev1.getScalar(0);
                            Scalar fn = prev2.getScalar(0);
                            if (idx != null && fn != null && idx.getUnsignedValue() < 0x10000 && execFuncs.contains(fn.getUnsignedValue())) {
                                wr.println("call\t" + hex(ins.getAddress()) + "\t" + hex(fn.getUnsignedValue()) + "\t" + idx.getUnsignedValue() + "\t" + hex(flows[0]));
                                nreg++;
                            }
                        }
                    }
                    // Inlined registration: MOV [GNatives + 4 * index], func.
                    if (mn.equals("MOV") && ins.getNumOperands() == 2) {
                        Scalar v = ins.getScalar(1);
                        if (v != null && execFuncs.contains(v.getUnsignedValue())) {
                            Object[] d = ins.getOpObjects(0);
                            if (d.length == 1 && d[0] instanceof Scalar) {
                                wr.println("mov\t" + hex(ins.getAddress()) + "\t" + hex(((Scalar) d[0]).getUnsignedValue()) + "\t" + hex(v.getUnsignedValue()));
                                nreg++;
                            } else if (d.length == 1 && d[0] instanceof Address) {
                                wr.println("mov\t" + hex(ins.getAddress()) + "\t" + hex((Address) d[0]) + "\t" + hex(v.getUnsignedValue()));
                                nreg++;
                            }
                        }
                    }
                    // String references: references and immediates that point at text.
                    Set<Long> seen = null;
                    for (Reference r : rm.getReferencesFrom(ins.getAddress())) {
                        Address to = r.getToAddress();
                        if (!to.isMemoryAddress()) continue;
                        String[] s = stringAt(to);
                        if (s == null) continue;
                        if (seen == null) seen = new HashSet<>();
                        if (!seen.add(to.getOffset())) continue;
                        ws.println(fe + "\t" + hex(ins.getAddress()) + "\t" + hex(to) + "\t" + s[0] + "\t" + esc(s[1]));
                        nstr++;
                    }
                    for (int oi = 0; oi < ins.getNumOperands(); oi++) {
                        for (Object o : ins.getOpObjects(oi)) {
                            if (!(o instanceof Scalar)) continue;
                            long v = ((Scalar) o).getUnsignedValue();
                            if (v < 0x400000L || v > 0xffffffffL) continue;
                            Address to;
                            try {
                                to = toAddr(v);
                            } catch (Exception e) {
                                continue;
                            }
                            String[] s = stringAt(to);
                            if (s == null) continue;
                            if (seen == null) seen = new HashSet<>();
                            if (!seen.add(v)) continue;
                            ws.println(fe + "\t" + hex(ins.getAddress()) + "\t" + hex(to) + "\t" + s[0] + "\t" + esc(s[1]));
                            nstr++;
                        }
                    }
                    prev2 = prev1;
                    prev1 = ins;
                    recent.addFirst(ins);
                }
            }
        }
        summary.append("functions ").append(nf).append(", calls ").append(ncalls).append(", string refs ").append(nstr).append(", registration candidates ").append(nreg).append('\n');
    }

    // ---------------------------------------------------------------- class records and vtables

    Map<Long, List<Address>> slotCache = new HashMap<>();

    List<Address> slots(Address vt) {
        List<Address> c = slotCache.get(vt.getOffset());
        if (c != null) return c;
        List<Address> out = new ArrayList<>();
        for (int i = 0; i < 1024; i++) {
            Address p = ptr(vt.add(i * 4));
            if (!isCode(p)) break;
            // The next vtable starts where code references the slot address.
            if (i > 0 && currentProgram.getReferenceManager().getReferenceCountTo(vt.add(i * 4)) > 0) break;
            out.add(p);
        }
        slotCache.put(vt.getOffset(), out);
        return out;
    }

    void vtablesIn(Function f, int depth, Set<Address> out, Set<Function> seen) {
        if (f == null || depth > 1 || !seen.add(f)) return;
        InstructionIterator it = listing.getInstructions(f.getBody(), true);
        while (it.hasNext()) {
            Instruction ins = it.next();
            if (!ins.getMnemonicString().equals("MOV")) continue;
            for (int i = 0; i < ins.getNumOperands(); i++) {
                for (Object o : ins.getOpObjects(i)) {
                    if (o instanceof Scalar) {
                        long v = ((Scalar) o).getUnsignedValue();
                        if (v < 0x400000L) continue;
                        Address a;
                        try {
                            a = toAddr(v);
                        } catch (Exception e) {
                            continue;
                        }
                        if (isData(a) && isCode(ptr(a)) && isCode(ptr(a.add(4))) && isCode(ptr(a.add(8)))) out.add(a);
                    }
                }
            }
        }
        if (depth == 0) {
            for (Function c : f.getCalledFunctions(monitor)) vtablesIn(c, depth + 1, out, seen);
        }
    }

    void classes(String classFile) throws Exception {
        // Every UTF-16 identifier in initialized data, keyed by address.
        Map<Long, String> names = new HashMap<>();
        Set<String> want = null;
        if (classFile != null) {
            want = new HashSet<>();
            for (String l : java.nio.file.Files.readAllLines(new File(classFile).toPath(), StandardCharsets.UTF_8)) {
                l = l.trim();
                if (!l.isEmpty()) want.add(l);
            }
        }
        for (MemoryBlock b : mem.getBlocks()) {
            if (!b.isInitialized() || b.isExecute()) continue;
            long start = b.getStart().getOffset(), end = b.getEnd().getOffset();
            byte[] buf = new byte[(int) (end - start + 1)];
            b.getBytes(b.getStart(), buf);
            int i = 0;
            while (i + 4 <= buf.length) {
                // A run of [A-Za-z0-9_] UTF-16 code units ending in 0, starting after a 0 unit.
                if (i >= 2 && (buf[i - 2] != 0 || buf[i - 1] != 0)) {
                    i += 2;
                    continue;
                }
                int j = i;
                StringBuilder sb = new StringBuilder();
                while (j + 2 <= buf.length && buf[j + 1] == 0 && (Character.isLetterOrDigit(buf[j]) || buf[j] == '_') && buf[j] > 0) {
                    sb.append((char) buf[j]);
                    j += 2;
                }
                if (sb.length() >= 2 && j + 2 <= buf.length && buf[j] == 0 && buf[j + 1] == 0 && Character.isLetter(sb.charAt(0))) {
                    String s = sb.toString();
                    if (want == null || want.contains(s)) names.put(start + i, s);
                    // Class records point past the C++ prefix: TEXT("AActor") + 1.
                    if (s.length() >= 3 && (s.charAt(0) == 'U' || s.charAt(0) == 'A') && (want == null || want.contains(s.substring(1)))) names.put(start + i + 2, s.substring(1));
                    i = j + 2;
                } else {
                    i += 2;
                }
            }
        }
        // Data words that point at one of them: the class records.
        Map<String, List<Address>> recs = new HashMap<>();
        for (MemoryBlock b : mem.getBlocks()) {
            if (!b.isInitialized() || b.isExecute()) continue;
            long start = b.getStart().getOffset(), end = b.getEnd().getOffset();
            byte[] buf = new byte[(int) (end - start + 1)];
            b.getBytes(b.getStart(), buf);
            for (int i = 0; i + 4 <= buf.length; i += 4) {
                long v = (buf[i] & 0xffL) | ((buf[i + 1] & 0xffL) << 8) | ((buf[i + 2] & 0xffL) << 16) | ((buf[i + 3] & 0xffL) << 24);
                String s = names.get(v);
                if (s != null) recs.computeIfAbsent(s, k -> new ArrayList<>()).add(toAddr(start + i));
            }
        }
        int nc = 0;
        try (PrintWriter w = open("classes.tsv"); PrintWriter wi = open("ivtables.tsv")) {
            List<String> sorted = new ArrayList<>(recs.keySet());
            Collections.sort(sorted);
            for (String cls : sorted) {
                for (Address rec : recs.get(cls)) {
                    Function ctor = null;
                    Set<Address> vts = null;
                    for (int i = 1; i < 16 && ctor == null; i++) {
                        Address p = ptr(rec.add(i * 4));
                        if (!isCode(p)) continue;
                        Set<Address> v = new LinkedHashSet<>();
                        Function f = func(p);
                        vtablesIn(f, 0, v, new HashSet<>());
                        if (!v.isEmpty()) {
                            ctor = f;
                            vts = v;
                        }
                    }
                    if (ctor == null) continue;
                    Address best = null;
                    for (Address v : vts) {
                        if (best == null || slots(v).size() > slots(best).size()) best = v;
                    }
                    List<Address> sl = slots(best);
                    StringBuilder sb = new StringBuilder();
                    for (Address s : sl) {
                        if (sb.length() > 0) sb.append(',');
                        sb.append(hex(s));
                        func(s);
                    }
                    w.println(cls + "\t" + hex(rec) + "\t" + hex(ctor.getEntryPoint()) + "\t" + hex(best) + "\t" + sl.size() + "\t" + sb);
                    for (Address v : vts) {
                        if (v.equals(best)) continue;
                        StringBuilder si = new StringBuilder();
                        for (Address s : slots(v)) {
                            if (si.length() > 0) si.append(',');
                            si.append(hex(s));
                        }
                        wi.println(cls + "\t" + hex(v) + "\t" + slots(v).size() + "\t" + si);
                    }
                    nc++;
                    break;
                }
            }
        }
        summary.append("utf-16 identifiers ").append(names.size()).append(", referenced ").append(recs.size()).append(", classes with a vtable ").append(nc).append('\n');
    }

    // ---------------------------------------------------------------- trace call sites

    static final class Val {
        Set<Long> consts = new TreeSet<>();
        boolean dynamic;
        String note = "";

        String show() {
            StringBuilder b = new StringBuilder();
            for (long c : consts) {
                if (b.length() > 0) b.append('|');
                b.append("0x").append(Long.toHexString(c));
            }
            if (dynamic) b.append(b.length() > 0 ? "|dyn" : "dyn");
            return b.toString();
        }
    }

    DecompInterface traceDec;
    Map<Long, HighFunction> highCache = new HashMap<>();
    Map<Long, DecompileResults> resCache = new HashMap<>();

    DecompileResults decomp(Function f) {
        DecompileResults r = resCache.get(f.getEntryPoint().getOffset());
        if (r == null) {
            r = traceDec.decompileFunction(f, 90, monitor);
            resCache.put(f.getEntryPoint().getOffset(), r);
        }
        return r;
    }

    /// The constant values a varnode can take (through copies, casts, ORs, phis and, up to
    /// `depth` levels, the callers' arguments when it is a parameter).
    void resolve(Varnode v, Val out, int depth, Function f, Set<Varnode> seen) {
        if (v == null) {
            out.dynamic = true;
            return;
        }
        if (v.isConstant()) {
            out.consts.add(v.getOffset() & 0xffffffffL);
            return;
        }
        if (!seen.add(v)) return;
        PcodeOp def = v.getDef();
        if (def == null) {
            HighVariable h = v.getHigh();
            if (h instanceof HighParam && depth > 0) {
                int slot = ((HighParam) h).getSlot();
                out.note += "param" + slot + ";";
                resolveCallers(f, slot, out, depth - 1);
                return;
            }
            out.dynamic = true;
            return;
        }
        switch (def.getOpcode()) {
            case PcodeOp.COPY:
            case PcodeOp.CAST:
            case PcodeOp.INT_ZEXT:
            case PcodeOp.INT_SEXT:
                resolve(def.getInput(0), out, depth, f, seen);
                return;
            case PcodeOp.MULTIEQUAL:
                for (int i = 0; i < def.getNumInputs(); i++) resolve(def.getInput(i), out, depth, f, seen);
                return;
            case PcodeOp.INT_OR:
            case PcodeOp.INT_AND:
            case PcodeOp.INT_XOR: {
                Val a = new Val(), b = new Val();
                resolve(def.getInput(0), a, depth, f, seen);
                resolve(def.getInput(1), b, depth, f, seen);
                if (a.dynamic || b.dynamic || a.consts.size() * b.consts.size() > 64) {
                    out.dynamic = true;
                    out.consts.addAll(a.consts);
                    out.consts.addAll(b.consts);
                    return;
                }
                for (long x : a.consts)
                    for (long y : b.consts) {
                        long r = def.getOpcode() == PcodeOp.INT_OR ? (x | y) : def.getOpcode() == PcodeOp.INT_AND ? (x & y) : (x ^ y);
                        out.consts.add(r & 0xffffffffL);
                    }
                return;
            }
            default:
                out.dynamic = true;
        }
    }

    /// The argument `slot` (0-based, as the decompiler counts parameters) at every call of `f`.
    void resolveCallers(Function f, int slot, Val out, int depth) {
        int n = 0;
        for (Reference r : getReferencesTo(f.getEntryPoint())) {
            if (!r.getReferenceType().isCall()) continue;
            Function c = fm.getFunctionContaining(r.getFromAddress());
            if (c == null || ++n > 24) continue;
            DecompileResults res = decomp(c);
            if (res == null || res.getHighFunction() == null) {
                out.dynamic = true;
                continue;
            }
            Iterator<PcodeOpAST> ops = res.getHighFunction().getPcodeOps(r.getFromAddress());
            while (ops.hasNext()) {
                PcodeOpAST op = ops.next();
                if (op.getOpcode() != PcodeOp.CALL || op.getNumInputs() <= slot + 1) continue;
                resolve(op.getInput(slot + 1), out, depth, c, new HashSet<>());
            }
        }
    }

    /// Float bits written to a stack location (stack offset), for extent vectors built in place.
    Map<Long, Long> stackConstWrites(HighFunction hf, Address before) {
        Map<Long, Long> out = new HashMap<>();
        Iterator<PcodeOpAST> it = hf.getPcodeOps();
        while (it.hasNext()) {
            PcodeOpAST op = it.next();
            if (op.getSeqnum().getTarget().compareTo(before) > 0) continue;
            if (op.getOpcode() == PcodeOp.COPY) {
                Varnode o = op.getOutput();
                if (o != null && o.getAddress().isStackAddress() && o.getSize() == 4) out.put(o.getOffset(), op.getInput(0).isConstant() ? op.getInput(0).getOffset() & 0xffffffffL : -1L);
            } else if (op.getOpcode() == PcodeOp.STORE) {
                Varnode p = op.getInput(1), val = op.getInput(2);
                Long off = stackOffset(p);
                if (off != null) out.put(off, val.isConstant() && val.getSize() == 4 ? val.getOffset() & 0xffffffffL : -1L);
            }
        }
        return out;
    }

    /// Stack offset of a pointer varnode built as PTRSUB(sp, c) / PTRADD / INT_ADD.
    Long stackOffset(Varnode p) {
        PcodeOp d = p.getDef();
        if (d == null) return null;
        int oc = d.getOpcode();
        if ((oc == PcodeOp.PTRSUB || oc == PcodeOp.INT_ADD) && d.getInput(1).isConstant()) {
            Varnode base = d.getInput(0);
            long c = d.getInput(1).getOffset();
            if (base.isRegister() || (base.getDef() == null && !base.isConstant())) {
                // PTRSUB(sp, off): sp is the stack pointer at entry.
                return (long) (int) c;
            }
            Long inner = stackOffset(base);
            return inner == null ? null : inner + (int) c;
        }
        if (oc == PcodeOp.CAST || oc == PcodeOp.COPY) return stackOffset(d.getInput(0));
        return null;
    }

    String extentOf(Varnode v, HighFunction hf, Address site) {
        if (v == null) return "?";
        Val c = new Val();
        resolve(v, c, 0, hf.getFunction(), new HashSet<>());
        if (!c.dynamic && c.consts.size() == 1) {
            long a = c.consts.iterator().next();
            if (a == 0) return "null";
            try {
                Address addr = toAddr(a);
                MemoryBlock b = mem.getBlock(addr);
                if (b != null && !b.isExecute()) {
                    if (!b.isInitialized()) return "global:" + hex(a) + ":uninit";
                    return "global:" + hex(a) + ":" + Float.intBitsToFloat(mem.getInt(addr)) + "," + Float.intBitsToFloat(mem.getInt(addr.add(4))) + "," + Float.intBitsToFloat(mem.getInt(addr.add(8)));
                }
            } catch (Exception e) {
            }
            return "const:" + hex(a);
        }
        Long off = stackOffset(v);
        if (off != null) {
            Map<Long, Long> w = stackConstWrites(hf, site);
            Long x = w.get(off), y = w.get(off + 4), z = w.get(off + 8);
            if (x != null && y != null && z != null && x >= 0 && y >= 0 && z >= 0) return "stack:" + Float.intBitsToFloat((int) (long) x) + "," + Float.intBitsToFloat((int) (long) y) + "," + Float.intBitsToFloat((int) (long) z);
            return "stack:" + off + ":dyn";
        }
        return "dyn";
    }

    String statementOf(DecompileResults r, PcodeOp call) {
        ClangTokenGroup g = r.getCCodeMarkup();
        if (g == null) return "";
        Deque<ClangNode> st = new ArrayDeque<>();
        st.push(g);
        while (!st.isEmpty()) {
            ClangNode n = st.pop();
            if (n instanceof ClangStatement) {
                PcodeOp op = ((ClangStatement) n).getPcodeOp();
                if (op != null && op.getSeqnum().equals(call.getSeqnum())) return n.toString().trim().replaceAll("\\s+", " ");
            }
            for (int i = n.numChildren() - 1; i >= 0; i--) st.push(n.Child(i));
        }
        return "";
    }

    void applySignature(Address a, String kind) {
        try {
            Function f = func(a);
            if (f == null) return;
            DataTypeManager dtm = currentProgram.getDataTypeManager();
            DataType vp = new PointerDataType(VoidDataType.dataType, dtm);
            DataType u = UnsignedIntegerDataType.dataType;
            String[] names = kind.equals("single")
                    ? new String[] { "Hit", "Source", "End", "Start", "TraceFlags", "Extent", "Light" }
                    : new String[] { "Mem", "End", "Start", "Extent", "TraceFlags", "Source", "Light" };
            FunctionDefinitionDataType sig = new FunctionDefinitionDataType(kind.equals("single") ? "SingleLineCheck" : "MultiLineCheck", dtm);
            ParameterDefinition[] ps = new ParameterDefinition[names.length];
            for (int i = 0; i < names.length; i++) ps[i] = new ParameterDefinitionImpl(names[i], names[i].equals("TraceFlags") ? u : vp, null);
            sig.setArguments(ps);
            sig.setReturnType(u);
            sig.setCallingConvention("__thiscall");
            new ApplyFunctionSignatureCmd(a, sig, SourceType.USER_DEFINED).applyTo(currentProgram, monitor);
        } catch (Exception e) {
            println("signature " + a + ": " + e);
        }
    }

    void traces(String spec) throws Exception {
        traceDec = new DecompInterface();
        traceDec.openProgram(currentProgram);
        Map<Address, String> targets = new LinkedHashMap<>();
        for (String t : spec.split("\\+")) {
            String[] p = t.split("/");
            Address a = toAddr(Long.parseLong(p[0].replace("0x", ""), 16));
            targets.put(a, p[1]);
            applySignature(a, p[1]);
        }
        int n = 0;
        try (PrintWriter w = open("traces.tsv")) {
            for (Map.Entry<Address, String> e : targets.entrySet()) {
                // TraceFlags and Extent are inputs 5 and 6 (single) or 5 and 4 (multi) after `this`.
                int flagsIn = e.getValue().equals("single") ? 6 : 6, extIn = e.getValue().equals("single") ? 7 : 5;
                for (Reference r : getReferencesTo(e.getKey())) {
                    if (!r.getReferenceType().isCall()) continue;
                    Function c = fm.getFunctionContaining(r.getFromAddress());
                    if (c == null) continue;
                    DecompileResults res = decomp(c);
                    HighFunction hf = res == null ? null : res.getHighFunction();
                    if (hf == null) {
                        w.println(e.getValue() + "\t" + hex(c.getEntryPoint()) + "\t" + hex(r.getFromAddress()) + "\t?\t?\t\t\tdecompile failed");
                        continue;
                    }
                    Iterator<PcodeOpAST> ops = hf.getPcodeOps(r.getFromAddress());
                    while (ops.hasNext()) {
                        PcodeOpAST op = ops.next();
                        if (op.getOpcode() != PcodeOp.CALL) continue;
                        Val fl = new Val();
                        resolve(op.getNumInputs() > flagsIn ? op.getInput(flagsIn) : null, fl, 2, c, new HashSet<>());
                        String ext = extentOf(op.getNumInputs() > extIn ? op.getInput(extIn) : null, hf, r.getFromAddress());
                        w.println(e.getValue() + "\t" + hex(c.getEntryPoint()) + "\t" + hex(r.getFromAddress()) + "\t" + fl.show() + "\t" + ext + "\t" + fl.note + "\t\t" + esc(statementOf(res, op)));
                        n++;
                    }
                }
            }
        }
        traceDec.dispose();
        resCache.clear();
        summary.append("trace call sites ").append(n).append('\n');
    }

    // ---------------------------------------------------------------- decompile everything

    void decompileAll(int timeout) throws Exception {
        File cf = new File(outDir, "decomp.c.tmp"), xf = new File(outDir, "decomp.idx.tmp");
        AtomicLong failed = new AtomicLong(), done = new AtomicLong();
        long total = fm.getFunctionCount();
        try (RandomAccessFile out = new RandomAccessFile(cf, "rw"); PrintWriter idx = new PrintWriter(new BufferedWriter(new OutputStreamWriter(new FileOutputStream(xf), StandardCharsets.UTF_8)))) {
            out.setLength(0);
            DecompileConfigurer cfg = d -> {
                DecompileOptions o = new DecompileOptions();
                o.grabFromProgram(currentProgram);
                d.setOptions(o);
                d.toggleCCode(true);
                d.toggleSyntaxTree(false);
                d.setSimplificationStyle("decompile");
            };
            DecompilerCallback<String[]> cb = new DecompilerCallback<String[]>(currentProgram, cfg) {
                @Override
                public String[] process(DecompileResults r, TaskMonitor m) {
                    Function f = r.getFunction();
                    String c;
                    if (r.decompileCompleted() && r.getDecompiledFunction() != null) c = r.getDecompiledFunction().getC();
                    else {
                        failed.incrementAndGet();
                        c = "// decompile failed: " + r.getErrorMessage() + "\n";
                    }
                    return new String[] { hex(f.getEntryPoint()), c };
                }
            };
            cb.setTimeout(timeout);
            Iterator<Function> fit = new Iterator<Function>() {
                FunctionIterator it = fm.getFunctions(true);
                Function nxt = advance();

                Function advance() {
                    while (it.hasNext()) {
                        Function f = it.next();
                        if (!f.isExternal() && !f.isThunk()) return f;
                    }
                    return null;
                }

                public boolean hasNext() {
                    return nxt != null;
                }

                public Function next() {
                    Function f = nxt;
                    nxt = advance();
                    return f;
                }
            };
            long[] t = { System.currentTimeMillis() };
            ParallelDecompiler.decompileFunctions(cb, currentProgram, fit, (String[] res) -> {
                if (res == null) return;
                synchronized (out) {
                    try {
                        byte[] b = res[1].getBytes(StandardCharsets.UTF_8);
                        long off = out.getFilePointer();
                        out.write(b);
                        idx.println(res[0] + "\t" + off + "\t" + b.length);
                    } catch (IOException e) {
                        throw new RuntimeException(e);
                    }
                    long d = done.incrementAndGet();
                    if (System.currentTimeMillis() - t[0] > 30000) {
                        t[0] = System.currentTimeMillis();
                        println("decompiled " + d + " / " + total);
                    }
                }
            }, monitor);
            cb.dispose();
        }
        new File(outDir, "decomp.c").delete();
        new File(outDir, "decomp.idx").delete();
        cf.renameTo(new File(outDir, "decomp.c"));
        xf.renameTo(new File(outDir, "decomp.idx"));
        summary.append("decompiled ").append(done.get()).append(", failed ").append(failed.get()).append('\n');
    }

    // ---------------------------------------------------------------- main

    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        outDir = new File(args[0]);
        outDir.mkdirs();
        String classFile = null, traceSpec = null;
        boolean decomp = true;
        int timeout = 60;
        for (int i = 1; i < args.length; i++) {
            String a = args[i];
            if (a.startsWith("classes:")) classFile = a.substring(8);
            else if (a.startsWith("traces:")) traceSpec = a.substring(7);
            else if (a.equals("nodecomp")) decomp = false;
            else if (a.startsWith("decomp_timeout:")) timeout = Integer.parseInt(a.substring(15));
        }
        mem = currentProgram.getMemory();
        listing = currentProgram.getListing();
        fm = currentProgram.getFunctionManager();
        summary.append("program ").append(currentProgram.getName()).append(" sha256 ").append(currentProgram.getExecutableSHA256()).append('\n');
        t0 = System.currentTimeMillis();
        long start = t0;
        nativeTables();
        nativeArrays();
        lap("natives");
        classes(classFile);
        lap("classes");
        functionPass();
        lap("functions");
        if (traceSpec != null) {
            traces(traceSpec);
            lap("traces");
        }
        if (decomp) {
            decompileAll(timeout);
            lap("decompile");
        }
        summary.append(String.format("total %.1f s%n", (System.currentTimeMillis() - start) / 1000.0));
        try (PrintWriter w = open("summary.txt")) {
            w.print(summary);
        }
        println(summary.toString());
    }
}
