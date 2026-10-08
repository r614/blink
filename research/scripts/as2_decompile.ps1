param([string]$File, [int]$Sprite = -1, [string]$Out = "")
# Research helper: rough AS2 decompiler (stack ops -> statements) for DoInitAction class sprites
# of an uncompressed GFx movie. Output stays in the scratchpad.
$src = @"
using System;
using System.Collections.Generic;
using System.Text;
using System.Linq;
public static class As2Dec {
  static string CStr(byte[] b, ref int o, int end) { int s=o; while(o<end && b[o]!=0) o++; string r=Encoding.UTF8.GetString(b,s,o-s); o++; return r; }
  static string Q(string s) { return "'" + s.Replace("\n","\\n") + "'"; }
  static bool IsQ(string s) { return s.Length >= 2 && s[0]=='\'' && s[s.Length-1]=='\''; }
  static string UnQ(string s) { return IsQ(s) ? s.Substring(1, s.Length-2) : s; }
  static string Pop(List<string> st) { if (st.Count==0) return "?"; var v = st[st.Count-1]; st.RemoveAt(st.Count-1); return v; }
  static int PopInt(List<string> st) { var v = Pop(st); int n; if (int.TryParse(v, out n)) return n; double d; if (double.TryParse(v, System.Globalization.NumberStyles.Float, System.Globalization.CultureInfo.InvariantCulture, out d)) return (int)d; return 0; }
  static string Mem(string o, string m) { if (IsQ(m)) { var u = UnQ(m); if (u.Length>0 && (char.IsLetter(u[0])||u[0]=='_'||u[0]=='$')) return o + "." + u; } return o + "[" + m + "]"; }
  static void Dec(byte[] b, int s, int e, string ind, List<string> pool, Dictionary<int,string> regs, StringBuilder sb) {
    var st = new List<string>();
    var targets = new HashSet<int>();
    // pre-pass for jump targets
    { int i = s; while (i < e) { byte op = b[i]; i++; if (op >= 0x80) { int l = BitConverter.ToUInt16(b, i); int next = i + 2 + l;
        if (op == 0x99 || op == 0x9D) targets.Add(next + BitConverter.ToInt16(b, i + 2));
        if (op == 0x8E || op == 0x9B) { int o = i + 2; CStr(b, ref o, e); int np = BitConverter.ToUInt16(b, o); int size;
          if (op == 0x8E) { o += 5; for (int k = 0; k < np; k++) { o++; CStr(b, ref o, e); } } else { o += 2; for (int k = 0; k < np; k++) CStr(b, ref o, e); }
          size = BitConverter.ToUInt16(b, o); next += size; }
        i = next; } } }
    int p = s;
    while (p < e) {
      if (targets.Contains(p)) sb.AppendLine(ind + "L" + (p - s) + ":");
      byte op = b[p]; int at = p; p++; int ds = p, de = p, nx = p;
      if (op >= 0x80) { int l = BitConverter.ToUInt16(b, p); ds = p + 2; de = Math.Min(ds + l, e); nx = ds + l; }
      Func<string> P = () => Pop(st);
      switch (op) {
        case 0x00: break;
        case 0x88: { int n = BitConverter.ToUInt16(b, ds); int o = ds + 2; pool.Clear(); for (int k = 0; k < n; k++) pool.Add(CStr(b, ref o, de)); break; }
        case 0x96: { int o = ds; while (o < de) { byte t = b[o++]; switch (t) {
            case 0: st.Add(Q(CStr(b, ref o, de))); break;
            case 1: st.Add(BitConverter.ToSingle(b, o).ToString("R", System.Globalization.CultureInfo.InvariantCulture)); o += 4; break;
            case 2: st.Add("null"); break; case 3: st.Add("undefined"); break;
            case 4: { int r = b[o++]; string nm; st.Add(regs.TryGetValue(r, out nm) ? nm : "r" + r); break; }
            case 5: st.Add(b[o++] != 0 ? "true" : "false"); break;
            case 6: { byte[] x = new byte[8]; Array.Copy(b, o + 4, x, 0, 4); Array.Copy(b, o, x, 4, 4); st.Add(BitConverter.ToDouble(x, 0).ToString("R", System.Globalization.CultureInfo.InvariantCulture)); o += 8; break; }
            case 7: st.Add(BitConverter.ToInt32(b, o).ToString()); o += 4; break;
            case 8: { int i = b[o++]; st.Add(Q(i < pool.Count ? pool[i] : "?")); break; }
            default: { int i = BitConverter.ToUInt16(b, o); o += 2; st.Add(Q(i < pool.Count ? pool[i] : "?")); break; } } } break; }
        case 0x17: { var v = P(); if (!(v.StartsWith("r") && v.Length < 4) && !regs.ContainsValue(v)) sb.AppendLine(ind + v + ";"); break; }
        case 0x1C: st.Add(UnQ(P())); break;
        case 0x1D: { var v = P(); var n = P(); sb.AppendLine(ind + UnQ(n) + " = " + v + ";"); break; }
        case 0x4E: { var m = P(); var o = P(); st.Add(Mem(o, m)); break; }
        case 0x4F: { var v = P(); var m = P(); var o = P(); sb.AppendLine(ind + Mem(o, m) + " = " + v + ";"); break; }
        case 0x52: { var m = P(); var o = P(); int n = PopInt(st); var a = new List<string>(); for (int k = 0; k < n; k++) a.Add(P()); st.Add((m == "undefined" || m == "''" ? o : Mem(o, m)) + "(" + string.Join(", ", a) + ")"); break; }
        case 0x3D: { var m = UnQ(P()); int n = PopInt(st); var a = new List<string>(); for (int k = 0; k < n; k++) a.Add(P()); st.Add(m + "(" + string.Join(", ", a) + ")"); break; }
        case 0x53: { var m = P(); var o = P(); int n = PopInt(st); var a = new List<string>(); for (int k = 0; k < n; k++) a.Add(P()); st.Add("new " + (m == "undefined" ? o : Mem(o, m)) + "(" + string.Join(", ", a) + ")"); break; }
        case 0x40: { var m = UnQ(P()); int n = PopInt(st); var a = new List<string>(); for (int k = 0; k < n; k++) a.Add(P()); st.Add("new " + m + "(" + string.Join(", ", a) + ")"); break; }
        case 0x43: { int n = PopInt(st); var a = new List<string>(); for (int k = 0; k < n; k++) { var v = P(); var nm = UnQ(P()); a.Insert(0, nm + ": " + v); } st.Add("{" + string.Join(", ", a) + "}"); break; }
        case 0x42: { int n = PopInt(st); var a = new List<string>(); for (int k = 0; k < n; k++) a.Add(P()); st.Add("[" + string.Join(", ", a) + "]"); break; }
        case 0x47: case 0x0A: case 0x21: { var y = P(); var x = P(); st.Add("(" + x + " + " + y + ")"); break; }
        case 0x0B: { var y = P(); var x = P(); st.Add("(" + x + " - " + y + ")"); break; }
        case 0x0C: { var y = P(); var x = P(); st.Add("(" + x + " * " + y + ")"); break; }
        case 0x0D: { var y = P(); var x = P(); st.Add("(" + x + " / " + y + ")"); break; }
        case 0x3F: { var y = P(); var x = P(); st.Add("(" + x + " % " + y + ")"); break; }
        case 0x48: case 0x0F: { var y = P(); var x = P(); st.Add("(" + x + " < " + y + ")"); break; }
        case 0x67: { var y = P(); var x = P(); st.Add("(" + x + " > " + y + ")"); break; }
        case 0x49: case 0x0E: case 0x13: { var y = P(); var x = P(); st.Add("(" + x + " == " + y + ")"); break; }
        case 0x66: { var y = P(); var x = P(); st.Add("(" + x + " === " + y + ")"); break; }
        case 0x10: { var y = P(); var x = P(); st.Add("(" + x + " && " + y + ")"); break; }
        case 0x11: { var y = P(); var x = P(); st.Add("(" + x + " || " + y + ")"); break; }
        case 0x60: { var y = P(); var x = P(); st.Add("(" + x + " & " + y + ")"); break; }
        case 0x61: { var y = P(); var x = P(); st.Add("(" + x + " | " + y + ")"); break; }
        case 0x12: st.Add("!" + P()); break;
        case 0x50: st.Add("(" + P() + " + 1)"); break;
        case 0x51: st.Add("(" + P() + " - 1)"); break;
        case 0x44: st.Add("typeof(" + P() + ")"); break;
        case 0x4A: st.Add("Number(" + P() + ")"); break;
        case 0x4B: st.Add("String(" + P() + ")"); break;
        case 0x18: st.Add("int(" + P() + ")"); break;
        case 0x2B: { var o = P(); var c = P(); st.Add("(" + c + ")" + o); break; }
        case 0x54: { var c = P(); var o = P(); st.Add("(" + o + " instanceof " + c + ")"); break; }
        case 0x69: { var sup = P(); var sub = P(); sb.AppendLine(ind + sub + " extends " + sup + ";"); break; }
        case 0x4C: { var v = P(); st.Add(v); st.Add(v); break; }
        case 0x4D: { var y = P(); var x = P(); st.Add(y); st.Add(x); break; }
        case 0x87: { int r = b[ds]; string nm; var v = st.Count > 0 ? st[st.Count-1] : "?"; var rn = regs.TryGetValue(r, out nm) ? nm : "r" + r; sb.AppendLine(ind + rn + " = " + v + ";"); if (st.Count > 0) st[st.Count-1] = rn; break; }
        case 0x3C: { var v = P(); var n = UnQ(P()); sb.AppendLine(ind + "var " + n + " = " + v + ";"); break; }
        case 0x41: { var n = UnQ(P()); sb.AppendLine(ind + "var " + n + ";"); break; }
        case 0x3E: sb.AppendLine(ind + "return " + P() + ";"); break;
        case 0x26: sb.AppendLine(ind + "trace(" + P() + ");"); break;
        case 0x3A: { var m = P(); var o = P(); sb.AppendLine(ind + "delete " + Mem(o, m) + ";"); break; }
        case 0x3B: sb.AppendLine(ind + "delete " + UnQ(P()) + ";"); break;
        case 0x55: case 0x46: st.Add("enumerate(" + P() + ")"); break;
        case 0x34: st.Add("getTimer()"); break;
        case 0x30: st.Add("random(" + P() + ")"); break;
        case 0x22: { var i = P(); var t = P(); st.Add(t + ".prop" + i); break; }
        case 0x23: { var v = P(); var i = P(); var t = P(); sb.AppendLine(ind + t + ".prop" + i + " = " + v + ";"); break; }
        case 0x99: sb.AppendLine(ind + "goto L" + (nx + BitConverter.ToInt16(b, ds) - s) + ";"); break;
        case 0x9D: sb.AppendLine(ind + "if (" + P() + ") goto L" + (nx + BitConverter.ToInt16(b, ds) - s) + ";"); break;
        case 0x07: sb.AppendLine(ind + "stop();"); break;
        case 0x06: sb.AppendLine(ind + "play();"); break;
        case 0x8C: { int o = ds; sb.AppendLine(ind + "gotoLabel(" + Q(CStr(b, ref o, de)) + ");"); break; }
        case 0x81: sb.AppendLine(ind + "gotoFrame(" + BitConverter.ToUInt16(b, ds) + ");"); break;
        case 0x94: { P(); sb.AppendLine(ind + "with(...)"); break; }
        case 0x8E: case 0x9B: {
          int o = ds; string nm = CStr(b, ref o, de); int np = BitConverter.ToUInt16(b, o); var ps = new List<string>(); var nregs = new Dictionary<int,string>();
          if (op == 0x8E) { o += 2; o += 1; int flags = BitConverter.ToUInt16(b, o); o += 2;
            for (int k = 0; k < np; k++) { int r = b[o++]; string pn = CStr(b, ref o, de); ps.Add(pn); if (r > 0) nregs[r] = pn; }
            int rr = 1; string[] pre = { "this", "arguments", "super", "_root", "_parent", "_global" }; int[] bits = { 0x01, 0x04, 0x10, 0x40, 0x80, 0x100 };
            for (int k = 0; k < 6; k++) if ((flags & bits[k]) != 0) nregs[rr++] = pre[k];
          } else { o += 2; for (int k = 0; k < np; k++) ps.Add(CStr(b, ref o, de)); }
          int size = BitConverter.ToUInt16(b, o); int bs = nx, be = nx + size; nx = be;
          var body = new StringBuilder(); Dec(b, bs, be, ind + "    ", new List<string>(pool), nregs, body);
          var head = "function " + nm + "(" + string.Join(", ", ps) + ") {";
          if (nm.Length > 0) { sb.AppendLine(ind + head); sb.Append(body); sb.AppendLine(ind + "}"); }
          else st.Add(head + "\n" + body.ToString() + ind + "}");
          break; }
        default: sb.AppendLine(ind + "/* op " + op.ToString("X2") + " */"); break;
      }
      p = nx;
    }
    foreach (var v in st) if (v != "?") sb.AppendLine(ind + "/*stack*/ " + v);
  }
  public static string Run(string path, int only) {
    var sb = new StringBuilder(); byte[] raw = System.IO.File.ReadAllBytes(path);
    int nbits = raw[8] >> 3; int pos = 8 + (5 + 4 * nbits + 7) / 8 + 4;
    while (pos + 2 <= raw.Length) {
      int h = BitConverter.ToUInt16(raw, pos); pos += 2; int code = h >> 6; int len = h & 0x3f;
      if (len == 0x3f) { len = BitConverter.ToInt32(raw, pos); pos += 4; }
      int bs = pos, be = Math.Min(pos + len, raw.Length); pos += len;
      if (code == 0) break;
      if (code == 59) { int id = BitConverter.ToUInt16(raw, bs); if (only < 0 || only == id) { sb.AppendLine("// ===== class sprite " + id); Dec(raw, bs + 2, be, "", new List<string>(), new Dictionary<int,string>(), sb); } }
    }
    return sb.ToString();
  }
}
"@
if (-not ("As2Dec" -as [type])) { Add-Type -TypeDefinition $src -Language CSharp }
$r = [As2Dec]::Run($File, $Sprite)
if ($Out) { [IO.File]::WriteAllText($Out, $r) } else { $r }
