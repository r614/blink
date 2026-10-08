# Build environment for Blink checkouts and git worktrees (bash / Git Bash).
#   source scripts/dev-env.sh
# - sccache as the rustc wrapper (when installed) shares compiled dependencies between all
#   checkouts and worktrees; each worktree keeps its own `target/` so builds never wait on another
#   checkout's cargo lock.
# - DISHONORED_CACHE points every checkout at one shared directory of derived game data
#   (collision BVHs, navmeshes, gameplay config, NPC / travel / dialog caches), so a new worktree
#   does not re-extract them. The files are versioned by name; bump a file's name when changing
#   its format.
if command -v sccache >/dev/null 2>&1; then
  export RUSTC_WRAPPER=sccache
  # Incremental compilation for the workspace crates (cargo's dev default; fast edit-rebuild
  # loops). sccache refuses to run when CARGO_INCREMENTAL is set at all, so leave it unset:
  # cargo builds only path crates incrementally, and those sccache passes through uncached, while
  # dependencies stay cached.
  unset CARGO_INCREMENTAL
  export SCCACHE_CACHE_SIZE="${SCCACHE_CACHE_SIZE:-40G}"
else
  echo "dev-env: sccache not found (cargo install sccache --locked); building without it" >&2
fi
# Per-worktree target dir (cargo's default `target/` next to Cargo.toml).
unset CARGO_TARGET_DIR
# Per-checkout cfg flags (an old workaround for a shared target dir) would defeat the cache.
unset RUSTFLAGS
if [ -n "${LOCALAPPDATA:-}" ]; then
  export DISHONORED_CACHE="${DISHONORED_CACHE:-$(cygpath -m "$LOCALAPPDATA" 2>/dev/null || echo "$LOCALAPPDATA")/dishonored-rs/cache}"
else
  export DISHONORED_CACHE="${DISHONORED_CACHE:-$HOME/.cache/dishonored-rs}"
fi
mkdir -p "$DISHONORED_CACHE"
echo "dev-env: RUSTC_WRAPPER=${RUSTC_WRAPPER:-none} DISHONORED_CACHE=$DISHONORED_CACHE" >&2
