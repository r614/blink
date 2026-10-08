#!/usr/bin/env bash
# Re-screenshot the start views of the maps whose post-process the post work changes
# (research/dishonored/systems/render_post.md): light shafts (a `bRenderLightShafts` directional
# light in the level's light package), one `--inspect` process per map, 1280x720, sound off.
#
#   scripts/render_post.sh [OUT_DIR] [BIN]
#
# OUT_DIR defaults to harness_out/render_post, BIN to target/release/dishonored_runtime(.exe).
# Each map writes OUT_DIR/<map>/start.png and log.txt. The Kismet screen grades
# (DisSeqAct_UberPostProcess), the possession field, the arm glow and the menu blur only show in
# play (a mission's Kismet, a cast, the pause menu): see the audit's "What to look at".
set -u
OUT=${1:-harness_out/render_post}
BIN=${2:-target/release/dishonored_runtime}
[ -x "$BIN" ] || BIN="$BIN.exe"
export DISHONORED_MUTE=1
SCRIPT=fixtures/scripts/map_start.json
run() {
  local map=$1 limit=$2; shift 2
  mkdir -p "$OUT/$map"
  local t0=$(date +%s)
  "$BIN" --inspect --map "$map" --script "$SCRIPT" --out "$OUT/$map" --size 1280x720 --exit-after "$limit" "$@" >"$OUT/$map/log.txt" 2>&1
  echo "$map: exit $? in $(( $(date +%s) - t0 )) s, $( [ -f "$OUT/$map/start.png" ] && echo 'start.png written' || echo 'NO start.png')"
}
# Light shafts: the Pub's four times of day and assault, the Lighthouse chaos variants, the Tower
# yard, the Boyle interior, the Outsider's dream.
for m in L_Pub_Day_P L_Pub_Dusk_P L_Pub_Morning_P L_Pub_Night_P L_Pub_Assault_P L_LightH_HighChaos_P L_LightH_LowChaos_P L_TowerRtrn_Yard_P L_Boyle_Int_P L_OutsiderDream_P; do run "$m" 600; done
