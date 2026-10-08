param([string]$Op, [string]$Json = "{}", [string]$Out = "", [switch]$Changes, [double]$Eps = 1e-4)
# One session op with a JSON argument (written to a file: no shell quoting), output to a file
# under P2_out when -Out is given (then -Changes prints the change points).
$d = if ($env:PROBE_SCRATCH) { $env:PROBE_SCRATCH } else { Join-Path $env:TEMP "probe_launch_scripts" }; New-Item -ItemType Directory -Force $d | Out-Null
$pl = if ($env:PROBE_LAUNCH) { $env:PROBE_LAUNCH } else { "C:/Users/Roshan/DEV/dishonored-rs/target-agents/i686-pc-windows-msvc/release/probe_launch.exe" }
[IO.File]::WriteAllText("$d\req.json", $Json, [Text.UTF8Encoding]::new($false))
$r = & $pl session $Op "@$d\req.json"
if ($Out) {
    [IO.File]::WriteAllText("$d\$Out", ($r -join "`n"), [Text.UTF8Encoding]::new($false))
    if ($Changes) { & (Join-Path $PSScriptRoot "changes.ps1") -File "$d\$Out" -Eps $Eps } else { "wrote $Out" }
} else { $r }
