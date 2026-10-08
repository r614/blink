# Build environment for Blink checkouts and git worktrees (PowerShell).
#   . .\scripts\dev-env.ps1
# See scripts/dev-env.sh: sccache shares compiled dependencies between checkouts and worktrees,
# each worktree keeps its own target/, and DISHONORED_CACHE is one shared directory of derived
# game data (collision, navmeshes, gameplay config, NPC / travel / dialog caches).
if (Get-Command sccache -ErrorAction SilentlyContinue) {
    $env:RUSTC_WRAPPER = "sccache"
    Remove-Item Env:CARGO_INCREMENTAL -ErrorAction SilentlyContinue  # see dev-env.sh
    if (-not $env:SCCACHE_CACHE_SIZE) { $env:SCCACHE_CACHE_SIZE = "40G" }
} else {
    Write-Warning "dev-env: sccache not found (cargo install sccache --locked); building without it"
}
Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
Remove-Item Env:RUSTFLAGS -ErrorAction SilentlyContinue
if (-not $env:DISHONORED_CACHE) { $env:DISHONORED_CACHE = Join-Path $env:LOCALAPPDATA "dishonored-rs\cache" }
New-Item -ItemType Directory -Force $env:DISHONORED_CACHE | Out-Null
Write-Host "dev-env: RUSTC_WRAPPER=$($env:RUSTC_WRAPPER) DISHONORED_CACHE=$($env:DISHONORED_CACHE)"
