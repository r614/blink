# Decompile UnrealScript classes of an original package with UE Explorer's UELib.
#
#   powershell -ExecutionPolicy Bypass -File research/ghidra/uc_decompile.ps1 `
#       -Package <inflated.upk> -Out <dir> -Classes DisTravelSpawner,DishonoredSpawner
#   (-Classes * decompiles every class in the package)
#
# UELib cannot read LZO-compressed packages: inflate first with
#   target/release/package_inflate <CookedPCConsole/X.upk> <scratch>/X.upk
# Output is derived from retail code: write it to scratch space, never commit it.
param(
    [Parameter(Mandatory = $true)][string]$Package,
    [Parameter(Mandatory = $true)][string]$Out,
    [Parameter(Mandatory = $true)][string[]]$Classes,
    [string]$UELib = "C:\Users\Roshan\DEV\tools\ue-explorer\ue-explorer\Eliot.UELib.dll"
)
Add-Type -Path $UELib
New-Item -ItemType Directory -Force $Out | Out-Null
# The loader prints the summary and one recoverable header warning to the console.
$pkg = [UELib.UnrealLoader]::LoadFullPackage($Package, [System.IO.FileAccess]::Read) 2>$null
$all = $pkg.Objects | Where-Object { $_.GetType().Name -eq 'UClass' }
$want = if ($Classes -contains '*') { $all } else { $all | Where-Object { $Classes -contains "$($_.Name)" } }
$n = 0
foreach ($c in $want) {
    try {
        $c.Decompile() | Out-File -Encoding utf8 (Join-Path $Out "$($c.Name).uc")
        $n++
    } catch {
        Write-Output "$($c.Name): $($_.Exception.Message)"
    }
}
Write-Output "decompiled $n classes to $Out"
