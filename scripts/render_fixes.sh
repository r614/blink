#!/usr/bin/env bash
# Re-screenshot the start views of the maps whose rendering the light-beam / water-sheet / island
# sea / sky-horizon / door-material fixes change (research/dishonored/RENDERING.md "Light beams",
# "Water sheets", "Island sea", "Sky dome"), one `--inspect` process per map, 1280x720, sound off.
#
#   scripts/render_fixes.sh [OUT_DIR] [BIN]
#
# OUT_DIR defaults to harness_out/render_fixes, BIN to target/release/dishonored_runtime(.exe).
# Each map writes OUT_DIR/<map>/start.png (camera at the map's first PlayerStart + 64 uu, its yaw,
# pitch -10, as the map sweep's mapshots), plus OUT_DIR/<map>/log.txt. L_Bridge_Part2_P also runs
# with --profile (frame-time lines) and a longer limit.
set -u
OUT=${1:-harness_out/render_fixes}
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
# 1. Light beams (Volume_Light_PMAT): the Pub's window shafts, the Art Dealer's cone, the outro's god rays.
for m in L_Pub_Day_P L_Pub_Dusk_P L_Pub_Morning_P L_ArtDealer_P L_Out_P; do run "$m" 600; done
# 2. Water sheets (water_spillway_translu_PMAT), the island sea (water_Foam_MAT), the Outsider sky (H_Enable_Horizon).
for m in L_PrsnSewer_P L_Isl_HighChaos_P L_Isl_LowChaos_P L_Out_HighChaos_EmilyAlive_P L_Out_HighChaos_emilyDead_P; do run "$m" 600; done
# 3. The door material with its emissive slot off (E_Enable_Emissive): no "I'M OBSOLETE" text.
run L_TowerRtrn_Int_P 600
# 4. The reported hang: profile lines every 5 s show whether frames come at all.
run L_Bridge_Part2_P 900 --profile
# 5. The dark starts (no code change; for the record).
for m in L_Prison_P L_Ovrsr_P L_Flooded_FStreets_P; do run "$m" 600; done
