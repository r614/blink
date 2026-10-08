param([string]$File, [string]$Mode = "frames", [int]$Sprite = -1)
# Research helper: dump AS2 frame actions (DoAction in sprite timelines) and class init actions
# of an uncompressed GFx movie. Output goes to the console (scratch only).
$src = @"
using System;
using System.Collections.Generic;
using System.Text;
public static class SwfAct {
  static StringBuilder sb;
  static string CStr(byte[] b, ref int o, int end) { int s=o; while(o<end && b[o]!=0) o++; string r=Encoding.UTF8.GetString(b,s,o-s); o++; return r; }
  static string Op(byte op) { switch(op) {
    case 0x04: return "NextFrame"; case 0x05: return "PrevFrame"; case 0x06: return "Play"; case 0x07: return "Stop"; case 0x0A: return "Add"; case 0x0B: return "Subtract"; case 0x0C: return "Multiply"; case 0x0D: return "Divide";
    case 0x0E: return "Equals"; case 0x0F: return "Less"; case 0x10: return "And"; case 0x11: return "Or"; case 0x12: return "Not"; case 0x13: return "StringEquals";
    case 0x17: return "Pop"; case 0x18: return "ToInteger"; case 0x1C: return "GetVariable"; case 0x1D: return "SetVariable"; case 0x20: return "SetTarget2"; case 0x21: return "StringAdd"; case 0x22: return "GetProperty";
    case 0x23: return "SetProperty"; case 0x26: return "Trace"; case 0x2B: return "CastOp"; case 0x30: return "RandomNumber"; case 0x34: return "GetTime";
    case 0x3A: return "Delete"; case 0x3B: return "Delete2"; case 0x3C: return "DefineLocal"; case 0x3D: return "CallFunction"; case 0x3E: return "Return"; case 0x3F: return "Modulo"; case 0x40: return "NewObject";
    case 0x41: return "DefineLocal2"; case 0x42: return "InitArray"; case 0x43: return "InitObject"; case 0x44: return "TypeOf"; case 0x46: return "Enumerate"; case 0x47: return "Add2";
    case 0x48: return "Less2"; case 0x49: return "Equals2"; case 0x4A: return "ToNumber"; case 0x4B: return "ToString"; case 0x4C: return "PushDuplicate"; case 0x4D: return "StackSwap"; case 0x4E: return "GetMember";
    case 0x4F: return "SetMember"; case 0x50: return "Increment"; case 0x51: return "Decrement"; case 0x52: return "CallMethod"; case 0x53: return "NewMethod"; case 0x54: return "InstanceOf"; case 0x55: return "Enumerate2";
    case 0x60: return "BitAnd"; case 0x61: return "BitOr"; case 0x66: return "StrictEquals"; case 0x67: return "Greater"; case 0x69: return "Extends";
    case 0x81: return "GotoFrame"; case 0x83: return "GetURL"; case 0x87: return "StoreRegister"; case 0x88: return "ConstantPool"; case 0x8B: return "SetTarget"; case 0x8C: return "GotoLabel";
    case 0x8E: return "DefineFunction2"; case 0x94: return "With"; case 0x96: return "Push"; case 0x99: return "Jump"; case 0x9A: return "GetURL2"; case 0x9B: return "DefineFunction"; case 0x9D: return "If"; case 0x9F: return "GotoFrame2";
    default: return "op" + op.ToString("X2"); } }
  static string PushArgs(byte[] d, int s, int e, List<string> pool) {
    var v = new List<string>(); int o = s;
    while (o < e) { byte t = d[o++];
      switch (t) {
        case 0: v.Add("\"" + CStr(d, ref o, e) + "\""); break;
        case 1: v.Add(BitConverter.ToSingle(d, o).ToString("R")); o += 4; break;
        case 2: v.Add("null"); break; case 3: v.Add("undefined"); break;
        case 4: v.Add("r" + d[o]); o++; break;
        case 5: v.Add(d[o] != 0 ? "true" : "false"); o++; break;
        case 6: { byte[] x = new byte[8]; Array.Copy(d, o + 4, x, 0, 4); Array.Copy(d, o, x, 4, 4); v.Add(BitConverter.ToDouble(x, 0).ToString("R")); o += 8; break; }
        case 7: v.Add(BitConverter.ToInt32(d, o).ToString()); o += 4; break;
        case 8: { int i = d[o]; o++; v.Add("'" + (i < pool.Count ? pool[i] : "?") + "'"); break; }
        default: { int i = BitConverter.ToUInt16(d, o); o += 2; v.Add("'" + (i < pool.Count ? pool[i] : "?") + "'"); break; }
      } }
    return string.Join(", ", v);
  }
  static void Disasm(byte[] b, int s, int e, string indent) {
    var pool = new List<string>(); int i = s;
    while (i < e) { byte op = b[i]; int at = i - s; i++; int ds = i, de = i, next = i;
      if (op >= 0x80) { int l = BitConverter.ToUInt16(b, i); ds = i + 2; de = Math.Min(ds + l, e); next = ds + l; }
      string arg = "";
      if (op == 0x88) { int n = BitConverter.ToUInt16(b, ds); int o = ds + 2; pool.Clear(); for (int k = 0; k < n; k++) pool.Add(CStr(b, ref o, de)); arg = n + " strings"; }
      else if (op == 0x96) arg = PushArgs(b, ds, de, pool);
      else if (op == 0x87) arg = "r" + b[ds];
      else if (op == 0x99 || op == 0x9D) arg = "-> " + (next - s + BitConverter.ToInt16(b, ds));
      else if (op == 0x8C) { int o = ds; arg = "\"" + CStr(b, ref o, de) + "\""; }
      else if (op == 0x81) arg = BitConverter.ToUInt16(b, ds).ToString();
      else if (op == 0x8E) { int o = ds; string nm = CStr(b, ref o, de); int np = BitConverter.ToUInt16(b, o); o += 5; var ps = new List<string>(); for (int k = 0; k < np; k++) { byte r = b[o++]; ps.Add("r" + r + ":" + CStr(b, ref o, de)); } arg = nm + "(" + string.Join(", ", ps) + ") size " + BitConverter.ToUInt16(b, o); }
      else if (op == 0x9B) { int o = ds; string nm = CStr(b, ref o, de); int np = BitConverter.ToUInt16(b, o); o += 2; var ps = new List<string>(); for (int k = 0; k < np; k++) ps.Add(CStr(b, ref o, de)); arg = nm + "(" + string.Join(", ", ps) + ")"; }
      else if (op == 0x9F) arg = "flags " + b[ds];
      sb.AppendLine(indent + at.ToString().PadLeft(5) + ": " + (op == 0 ? "End" : Op(op)) + " " + arg);
      i = next; }
  }
  static void Tags(byte[] raw, int pos, int end, int sprite, string mode, int only) {
    int frame = 0;
    while (pos + 2 <= end) {
      int h = BitConverter.ToUInt16(raw, pos); pos += 2; int code = h >> 6; int len = h & 0x3f;
      if (len == 0x3f) { len = BitConverter.ToInt32(raw, pos); pos += 4; }
      int bs = pos, be = Math.Min(pos + len, raw.Length); pos += len;
      if (code == 0) break;
      if (code == 1) frame++;
      else if (code == 43 && mode == "frames" && (only < 0 || only == sprite)) { int o = bs; sb.AppendLine("sprite " + sprite + " f" + frame + " label " + CStr(raw, ref o, be)); }
      else if (code == 12 && mode == "frames" && (only < 0 || only == sprite)) { sb.AppendLine("sprite " + sprite + " f" + frame + " DoAction:"); Disasm(raw, bs, be, "    "); }
      else if (code == 39) { int id = BitConverter.ToUInt16(raw, bs); Tags(raw, bs + 4, be, id, mode, only); }
      else if (code == 59 && mode == "init" && (only < 0 || only == BitConverter.ToUInt16(raw, bs))) { sb.AppendLine("init " + BitConverter.ToUInt16(raw, bs) + ":"); Disasm(raw, bs + 2, be, "    "); }
    }
  }
  public static string Run(string path, string mode, int only) {
    sb = new StringBuilder(); byte[] raw = System.IO.File.ReadAllBytes(path);
    int nbits = raw[8] >> 3; int pos = 8 + (5 + 4 * nbits + 7) / 8 + 4;
    Tags(raw, pos, raw.Length, 0, mode, only);
    return sb.ToString();
  }
}
"@
if (-not ("SwfAct" -as [type])) { Add-Type -TypeDefinition $src -Language CSharp }
[SwfAct]::Run($File, $Mode, $Sprite)
