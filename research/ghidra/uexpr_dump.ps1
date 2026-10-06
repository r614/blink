param([string]$Raw, [string]$Names, [int]$NameIndex = 288, [int]$Max = 12, [string]$Need = "")
# Research (output is derived from retail data; keep it out of the repo): decode UE3 FUniformExpressionSet arrays (pixel vectors, pixel scalars) around the
# shader maps that reference a scalar parameter name, in a decompressed shader cache.
# Usage: uexpr_dump.ps1 -Raw <cache decompressed by shader_dump with SHADER_DUMP_RAW> -Names <shader_dump name list> [-NameIndex 288] [-Max N] [-Need regex]
$d = [IO.File]::ReadAllBytes($Raw)
$ntab = @{}
foreach ($l in Get-Content $Names) { if ($l -match '^name\[(\d+)\] (\S+)') { $ntab[[int]$matches[1]] = $matches[2] } }
function U32($p) { return [BitConverter]::ToUInt32($d, $p) }
function F32($p) { return [BitConverter]::ToSingle($d, $p) }
$script:pos = 0
function Expr() {
  $t = U32 $script:pos; $n = U32 ($script:pos + 4)
  if ($n -ne 0 -or $t -lt 512 -or $t -gt 528) { throw "bad type $t at $($script:pos)" }
  $script:pos += 8
  switch ($t) {
    512 { return "Abs(" + (Expr) + ")" }
    513 { $a = Expr; $b = Expr; $k = U32 $script:pos; $script:pos += 4; return "Append($a, $b; $k)" }
    514 { $a = Expr; $b = Expr; $c = Expr; return "Clamp($a, $b, $c)" }
    515 { $v = "({0},{1},{2},{3})" -f (F32 $script:pos), (F32 ($script:pos+4)), (F32 ($script:pos+8)), (F32 ($script:pos+12)); $script:pos += 17; return "C$v" }
    517 { return "Floor(" + (Expr) + ")" }
    518 { $a = Expr; $b = Expr; return "Fmod($a, $b)" }
    519 { $a = Expr; $b = Expr; $op = $d[$script:pos]; $script:pos += 1; return "Math$op($a, $b)" }
    520 { return "Frac(" + (Expr) + ")" }
    521 { return "Periodic(" + (Expr) + ")" }
    522 { return "RealTime" }
    523 { $nm = $ntab[[int](U32 $script:pos)]; $v = F32 ($script:pos + 8); $script:pos += 12; return "S[$nm=$v]" }
    524 { $a = Expr; $c = U32 $script:pos; $script:pos += 4; return "Sine$c($a)" }
    525 { $i = U32 $script:pos; $script:pos += 4; return "Tex#$i" }
    526 { $nm = $ntab[[int](U32 $script:pos)]; $i = U32 ($script:pos + 8); $script:pos += 12; return "T[$nm #$i]" }
    527 { return "Time" }
    528 { $nm = $ntab[[int](U32 $script:pos)]; $v = "({0},{1},{2},{3})" -f (F32 ($script:pos+8)), (F32 ($script:pos+12)), (F32 ($script:pos+16)), (F32 ($script:pos+20)); $script:pos += 24; return "V[$nm=$v]" }
    default { throw "unhandled $t" }
  }
}
function Arr($p) {
  $script:pos = $p; $c = U32 $p
  if ($c -gt 40) { throw "count" }
  $script:pos += 4; $out = @()
  for ($i = 0; $i -lt $c; $i++) { $out += (Expr) }
  return ,$out
}
# Hits: ScalarParameter type (523) followed by the name.
$pat = [byte[]](0x0b,0x02,0,0,0,0,0,0) + [BitConverter]::GetBytes([uint32]$NameIndex) + [byte[]](0,0,0,0)
$hits = @(); $i = 0
while ($hits.Count -lt 4000) {
  $j = [Array]::IndexOf($d, [byte]0x0b, $i); if ($j -lt 0) { break }
  $ok = $true; for ($k = 0; $k -lt 16; $k++) { if ($d[$j + $k] -ne $pat[$k]) { $ok = $false; break } }
  if ($ok) { $hits += $j }
  $i = $j + 1
}
"hits $($hits.Count)"
$seen = @{}; $shown = 0
foreach ($h in $hits) {
  if ($shown -ge $Max) { break }
  # Find a vector array start s < h whose parse ends at the scalar array that contains h.
  for ($s = $h - 4; $s -gt $h - 6000; $s--) {
    $c = U32 $s; if ($c -lt 1 -or $c -gt 20) { continue }
    try { $vec = Arr $s; $e = $script:pos; $sc = Arr $e; $e2 = $script:pos } catch { continue }
    if (-not ($e -lt $h -and $e2 -gt $h)) { continue }
    try { $tex = Arr $e2 } catch { $tex = @("?") }
    $key = ($vec -join ';') + '|' + ($sc -join ';')
    if ($seen.ContainsKey($key)) { break }
    if ($Need -and -not ($key -match $Need)) { $seen[$key] = 1; break }
    $seen[$key] = 1; $shown++
    "=== at $s"
    for ($k = 0; $k -lt $vec.Count; $k++) { "  UniformPixelVector_$k = $($vec[$k])" }
    for ($k = 0; $k -lt $sc.Count; $k++) { "  scalar $k (UniformPixelScalars_$([math]::Floor($k/4)).$('xyzw'[$k%4])) = $($sc[$k])" }
    "  textures: $($tex -join ', ')"
    break
  }
}
