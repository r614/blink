param(
    [string]$Label,            # output name: effects/<Label>.json and <Label>_timeline.json
    [string]$Effect,           # eEffectPp name (Epp_Knocked, Epp_Weepers, Epp_MaskOn, Epp_Adrenaline, Epp_ZoomLens, Epp_Blinded, Epp_Dream, Epp_Underwater, Epp_FakeBlink ...)
    [string]$Writes = "",      # extra writes applied with the effect: JSON array of {"object":..,"path":..,"value":..} ("MGR" in object = the PP manager)
    [string]$Reads = "",       # extra per-sample reads: JSON object {name: "object:path"} ("MGR:" = the PP manager, "CTL_BLIND:" / "CTL_OPACITY:" = the live controllers)
    [int]$Frames = 12,         # census frames while active
    [int]$Settle = 40,         # ticks between forcing the effect and the census
    [int]$Samples = 120,       # timeline samples per shader (one psconst draw each)
    [int]$MaxShaders = 6       # timelines for this many drawn-only-while-active shaders
)
# Capture the pixel shaders, constants over time and bound textures of a post-process effect in
# the running original, by forcing `DisPostProcessManager.m_RequiredEffects[e]` (what the setters
# FUN_00be7dc0 / the Kismet ops do) and diffing the pixel-shader census against the baseline.
# Output: $DISHONORED_CACHE/oracle/shaders/<exe16>/effects/<Label>.json (census, drawn-only-while-active
# shaders, constants + textures of 2 draws each) and <Label>_timeline.json (constants per frame,
# the effect forced at sample 5). Retail-derived: never committed.
$d = if ($env:PROBE_SCRATCH) { $env:PROBE_SCRATCH } else { Join-Path $env:TEMP "probe_launch_scripts" }; New-Item -ItemType Directory -Force $d | Out-Null
$o = Join-Path $PSScriptRoot "op.ps1"
function J($op, $json) { $r = & $o $op $json | ConvertFrom-Json; if (-not $r.ok) { throw "$op failed: $($r.error)" }; $r.result }
function Mgr { $f = J find '{"class":"DisPostProcessManager","limit":20}'; ($f.objects | Where-Object { $_.name -match 'L_.*PersistentLevel' } | Select-Object -First 1).addr }
function Ctl($cls) { $f = J find "{""class"":""$cls"",""limit"":40}"; $best = $null; $bn = -1; foreach ($x in $f.objects) { if ($x.name -match 'Transient\.PostProcessChain_(\d+)') { $n = [int]$Matches[1]; if ($n -gt $bn) { $bn = $n; $best = $x.addr } } }; $best }
$idx = [array]::IndexOf((J enum '{"name":"eEffectPp"}').names, $Effect)
if ($idx -lt 0) { throw "unknown effect $Effect" }
function Force($mgr, $extra) {
    J write "{""object"":""$mgr"",""path"":""m_RequiredEffects[$idx]"",""value"":1}" | Out-Null
    J write "{""object"":""$mgr"",""path"":""m_EffectStates[$idx]"",""value"":2}" | Out-Null
    foreach ($w in $extra) { $ob = if ($w.object -eq "MGR") { $mgr } else { $w.object }; J write ((@{ object = $ob; path = $w.path; value = $w.value } | ConvertTo-Json -Compress)) | Out-Null }
}
$extra = @(); if ($Writes) { $extra = @($Writes | ConvertFrom-Json) }
$readSpec = @{}; if ($Reads) { ($Reads | ConvertFrom-Json).PSObject.Properties | ForEach-Object { $readSpec[$_.Name] = $_.Value } }
function Fresh {
    J reload '{}' | Out-Null
    J setup '{}' | Out-Null
    J step '{"n":30}' | Out-Null
    J resume '{}' | Out-Null
}
function Resolve($spec, $mgr, $cb, $co) { $spec.Replace("MGR:", "${mgr}:").Replace("CTL_BLIND:", "${cb}:").Replace("CTL_OPACITY:", "${co}:") }
function Sample($mgr, $cb, $co) {
    $v = [ordered]@{ time = (J read '{"object":"worldinfo","path":"TimeSeconds"}').value }
    foreach ($k in $readSpec.Keys) { $p = (Resolve $readSpec[$k] $mgr $cb $co).Split(":", 2); try { $v[$k] = (J read ((@{ object = $p[0]; path = $p[1] } | ConvertTo-Json -Compress))).value } catch { $v[$k] = $null } }
    $v
}
Fresh
$mgr = Mgr; $cb = Ctl "DisBlindedPpController"; $co = Ctl "DisOpacityParameterPpController"
$next = (J shaders '{"next_only":true}').next
$base = J census '{"frames":20}'
Force $mgr $extra
J step ("{""n"":$Settle}") | Out-Null
J resume '{}' | Out-Null
$state_after = Sample $mgr $cb $co
$active = J census "{""frames"":$Frames}"
$new = J shaders "{""since"":$next}"
$baseSet = @{}; foreach ($s in $base.shaders) { $baseSet[$s.hash] = 1 }
$only = @($active.shaders | Where-Object { -not $baseSet.ContainsKey($_.hash) })
$consts = @()
foreach ($s in $only | Select-Object -First 12) {
    try { $consts += J psconst "{""hash"":""$($s.hash)"",""draws"":2}" } catch { $consts += @{ hash = $s.hash; error = "$_" } }
}
$dir = Join-Path ((J shaders '{"since":999999999}').dir) "effects"
New-Item -ItemType Directory -Force $dir | Out-Null
$res = [ordered]@{ label = $Label; effect = $Effect; effect_index = $idx; baseline_census = $base.shaders; active_census = $active.shaders; drawn_only_while_active = $only; new_shaders = $new.shaders; constants_and_textures = $consts; state_after_settle = $state_after; writes = $extra }
$res | ConvertTo-Json -Depth 12 -Compress | Set-Content -Encoding utf8 (Join-Path $dir "$Label.json")
"{0}: effect {1} ({2}); {3} shaders created, {4} drawn only while active, {5} constant sets -> {6}" -f $Label, $Effect, $idx, $new.shaders.Count, $only.Count, $consts.Count, (Join-Path $dir "$Label.json")
$only | ForEach-Object { "  $($_.hash) draws $($_.draws) $($_.ctab)" }
# Timelines: a fresh map per shader, the effect forced at sample 5, one draw's constants (and textures) per sample.
$tl = @()
foreach ($s in ($only | Sort-Object { - [int]$_.draws } | Select-Object -First $MaxShaders)) {
    Fresh
    $mgr = Mgr; $cb = Ctl "DisBlindedPpController"; $co = Ctl "DisOpacityParameterPpController"
    $rows = @()
    for ($i = 0; $i -lt $Samples; $i++) {
        if ($i -eq 5) { Force $mgr $extra }
        $pc = try { J psconst "{""hash"":""$($s.hash)"",""draws"":1}" } catch { $null }
        $st = Sample $mgr $cb $co
        $rows += [ordered]@{ time = $st.time; state = $st; draws = if ($pc) { $pc.draws } else { @() } }
    }
    $tl += [ordered]@{ shader = $s.hash; ctab = $s.ctab; samples = $rows }
}
[ordered]@{ label = $Label; effect = $Effect; timelines = $tl } | ConvertTo-Json -Depth 12 -Compress | Set-Content -Encoding utf8 (Join-Path $dir "${Label}_timeline.json")
"{0}: {1} timelines -> {2}" -f $Label, $tl.Count, (Join-Path $dir "${Label}_timeline.json")
