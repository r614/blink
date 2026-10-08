param(
    [string]$Label,          # output name: effects/<Label>_timeline.json
    [string]$Hash,           # pixel shader 0x...
    [string]$Power,          # power to give (e.g. DarkVision)
    [int]$Level = 2,
    [string]$Component,      # power component class (select_power)
    [int]$HoldTicks = 3,     # RightMouseButton held this many ticks
    [int]$Samples = 300,     # psconst calls (one draw each) after the press
    [string]$Reads = ""      # extra JSON reads per sample: '"object":"PowerDarkVision","path":"m_fTimeRemaining"' entries separated by ';'
)
# Per-frame live constants of one pixel shader across a power's whole effect, with the power's
# state read after each sample, through one `session repl` (no process start per sample).
# The world runs freely (psconst leaves lockstep). Output under $DISHONORED_CACHE/oracle/shaders.
$pl = if ($env:PROBE_LAUNCH) { $env:PROBE_LAUNCH } else { "C:/Users/Roshan/DEV/dishonored-rs/target-agents-hook/i686-pc-windows-msvc/release/probe_launch.exe" }
$lines = @(
    '{"op":"setup"}',
    "{""op"":""power"",""name"":""$Power"",""level"":$Level}",
    "{""op"":""select_power"",""component"":""$Component""}",
    '{"op":"write","object":"pawn","path":"m_Mana","value":100}',
    '{"op":"resume"}',
    '{"op":"key","key":"RightMouseButton","event":"down"}'
)
$rd = @($Reads -split ';' | Where-Object { $_ })
for ($i = 0; $i -lt $Samples; $i++) {
    if ($i -eq $HoldTicks) { $lines += '{"op":"key","key":"RightMouseButton","event":"up"}' }
    $lines += "{""op"":""psconst"",""hash"":""$Hash"",""draws"":1}"
    $lines += '{"op":"read","object":"worldinfo","path":"TimeSeconds"}'
    foreach ($r in $rd) { $lines += "{""op"":""read"",$r}" }
}
$out = $lines | & $pl session repl
$per = 2 + $rd.Count
$res = @()
$j = $out | ForEach-Object { $_ | ConvertFrom-Json }
$k = 6
for ($i = 0; $i -lt $Samples; $i++) {
    if ($i -eq $HoldTicks) { $k++ }
    $pc = $j[$k]; $t = $j[$k + 1]
    $vals = @{}
    for ($q = 0; $q -lt $rd.Count; $q++) { $x = $j[$k + 2 + $q]; if ($x.ok) { $vals[$x.result.path] = $x.result.value } }
    $res += [ordered]@{ time = $t.result.value; draws = $pc.result.draws; state = $vals }
    $k += $per
}
$dir = Join-Path (Join-Path $env:LOCALAPPDATA "dishonored-rs\cache\oracle\shaders\70da96138fe2171c") "effects"
New-Item -ItemType Directory -Force $dir | Out-Null
$file = Join-Path $dir "$($Label)_timeline.json"
[ordered]@{ label = $Label; shader = $Hash; power = $Power; level = $Level; samples = $res } | ConvertTo-Json -Depth 10 -Compress | Set-Content -Encoding utf8 $file
"{0}: {1} samples, {2} with draws -> {3}" -f $Label, $res.Count, @($res | Where-Object { $_.draws.Count -gt 0 }).Count, $file
