param([string]$Label, [string]$Power, [int]$Level, [string]$Component, [int]$Hold = 3, [int]$ActiveAfter = 20, [string]$Setup = "")
# Capture the shaders and parameters of a power's screen effect in the running original:
# baseline census, cast, the PP manager's parameters per tick, census while active, shaders
# created since the baseline, live constants of the shaders drawn only while active.
# Output: $DISHONORED_CACHE/oracle/shaders/<exe16>/effects/<Label>.json (retail-derived).
$d = if ($env:PROBE_SCRATCH) { $env:PROBE_SCRATCH } else { Join-Path $env:TEMP "probe_launch_scripts" }; New-Item -ItemType Directory -Force $d | Out-Null
$pl = if ($env:PROBE_LAUNCH) { $env:PROBE_LAUNCH } else { "C:/Users/Roshan/DEV/dishonored-rs/target-agents/i686-pc-windows-msvc/release/probe_launch.exe" }
$o = Join-Path $PSScriptRoot "op.ps1"
function J($op, $json) { $r = & $o $op $json | ConvertFrom-Json; if (-not $r.ok) { throw "$op failed: $($r.error)" }; $r.result }
$next = (J shaders '{"since":0}').next
$base = J census '{"frames":20}'
J power "{""name"":""$Power"",""level"":$Level}" | Out-Null
J select_power "{""component"":""$Component""}" | Out-Null
if ($Setup) { Invoke-Expression $Setup }
J write '{"object":"pawn","path":"m_Mana","value":100}' | Out-Null
$ch = '"bt":"pPpManager:m_BendTimeIntensity","blink":"pPpManager:m_BlinkParams.R","blinkg":"pPpManager:m_BlinkParams.G","blinkb":"pPpManager:m_BlinkParams.B","blinka":"pPpManager:m_BlinkParams.A","pos":"pPpManager:m_PossessionParams.R","posg":"pPpManager:m_PossessionParams.G","posb":"pPpManager:m_PossessionParams.B","posa":"pPpManager:m_PossessionParams.A","post":"pPpManager:m_PossessionStateTime","dil":"worldinfo:TimeDilation"'
$run = J run ("{""ticks"":$ActiveAfter,""channels"":{$ch},""keys"":[[2,""RightMouseButton"",true],[$(2+$Hold),""RightMouseButton"",false]]}")
$active = J census '{"frames":10}'
$after = J run ("{""ticks"":600,""every"":5,""channels"":{$ch}}")
$new = J shaders "{""since"":$next}"
$baseSet = @{}; foreach ($s in $base.shaders) { $baseSet[$s.hash] = 1 }
$only = @($active.shaders | Where-Object { -not $baseSet.ContainsKey($_.hash) })
$consts = @()
foreach ($s in $only | Select-Object -First 12) {
    try { $consts += J psconst "{""hash"":""$($s.hash)"",""draws"":2}" } catch { $consts += @{ hash = $s.hash; error = "$_" } }
}
$outDir = Join-Path ((J shaders "{""since"":999999999}").dir) "effects"
New-Item -ItemType Directory -Force $outDir | Out-Null
$res = [ordered]@{ label = $Label; power = $Power; level = $Level; baseline_census = $base.shaders; active_census = $active.shaders; drawn_only_while_active = $only; new_shaders = $new.shaders; constants = $consts; pp_cast = $run; pp_after = $after }
$res | ConvertTo-Json -Depth 12 -Compress | Set-Content -Encoding utf8 (Join-Path $outDir "$Label.json")
"{0}: {1} shaders created, {2} drawn only while active, {3} constant sets -> {4}" -f $Label, $new.shaders.Count, $only.Count, $consts.Count, (Join-Path $outDir "$Label.json")
$only | ForEach-Object { "  $($_.hash) draws $($_.draws) $($_.ctab)" }
