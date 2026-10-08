param([string]$File, [double]$Eps = 1e-4, [string[]]$Ignore = @("world.time"))
# Print the frames of a `session run` answer where any channel (except ignored ones) changes
# by more than Eps from the previous printed frame... compared to previous frame.
$j = Get-Content $File -Raw | ConvertFrom-Json
if (-not $j.ok) { $j.error; return }
$r = $j.result
$ch = $r.channels
"tick " + ($ch -join " ")
$prev = $null
foreach ($f in $r.frames) {
    $show = $false
    if ($null -eq $prev) { $show = $true } else {
        for ($i = 0; $i -lt $ch.Count; $i++) {
            if ($Ignore -contains $ch[$i]) { continue }
            $a = $f[$i + 1]; $b = $prev[$i + 1]
            if ($null -eq $a -or $null -eq $b) { if ($a -ne $b) { $show = $true }; continue }
            # timers: show only when the sign of the change pattern changes is too complex; show jumps
            if ([math]::Abs($a - $b) -gt $Eps) { $show = $true }
        }
    }
    if ($show) { ($f | ForEach-Object { if ($_ -is [double]) { "{0:G6}" -f $_ } else { "$_" } }) -join " " }
    $prev = $f
}
if ($r.events) { "events:"; $r.events | Select-Object -First 60 | ForEach-Object { "$($_.tick) $($_.object) $($_.function)" } }
if ($r.spawns) { "spawns:"; $r.spawns | Select-Object -First 60 | ForEach-Object { "$($_.tick) $($_.object)" } }
