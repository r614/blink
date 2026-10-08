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
# 6. Depth prepass (water thickness / fake fog, volume-light and foam depth fades): Boyle exterior
# river and the flooded streets, on top of the sets above. DISHONORED_NODEPTH=1 gives the old look.
for m in L_Boyle_Ext_P L_Flooded_FStreets_P; do run "$m" 600; done
# 5. The dark starts (no code change; for the record).
for m in L_Prison_P L_Ovrsr_P L_Flooded_FStreets_P; do run "$m" 600; done
# 6. Planar reflections (SceneCaptureReflectActor, reflect.rs): the Boyle river (RealTime_refl), the Pub canal
#    is in step 1 (water_RTR), the Streets canal, the Bridge river, the Brothel
#    and Distillery water, the island sea (water_Foam_MAT), the Light House, the flooded Refinery.
#    The last run is the Boyle river with --no-reflections (the cube fallback), in OUT/no_reflections.
for m in L_Boyle_Ext_P L_Streets1_P L_Bridge_Part1a_P L_Brothel_P L_Distillery_P L_Flooded_FRefinery_P L_LightH_HighChaos_P L_Tower_P; do run "$m" 600; done
( OUT="$OUT/no_reflections"; run L_Boyle_Ext_P 600 --no-reflections )
# 7. Depth and reflection, round 2 (research/dishonored/systems/render_depth.md, render_reflect.md).
#    Soft (depth-biased alpha) sprite / mesh particles and light cones (FxMaterial, particle_soft.wgsl):
#    maps with steam, smoke, sparks and fire; the Distillery also runs with DISHONORED_NODEPTH=1 (no
#    depth fade: particles cut hard where they meet geometry) in OUT/nodepth for the comparison.
for m in L_Distillery_P L_Streets1_P L_Pub_Day_P; do run "$m" 600; done
( export DISHONORED_NODEPTH=1; OUT="$OUT/nodepth"; run L_Distillery_P 600 )
#    Capture fog (Boyle Ext, bEnableFog) and fog + post-process (Isl docks, both flags) in the
#    reflections: the water's reflection of the far scene fades into the DisFog colour.
for m in L_Boyle_Ext_P L_Isl_HighChaos_P L_Isl_LowChaos_P; do run "$m" 600; done
#    Kismet-toggled captures start disabled (PrsnSewer, Streetsewer, FStreets): --inspect has no mission
#    run, so these show the level state (water on the cube term); the toggled state needs a mission
#    run past the event that turns the capture on (SeqAct_Toggle -> SceneCaptureActor.OnToggle).
for m in L_PrsnSewer_P L_Streetsewer_P L_Flooded_FStreets_P; do run "$m" 600; done
#    The Tower Return lens TV (SceneCapture2DActor -> saili_FT_LensTV.screen_MAT, tv_screen.rs): the
#    Int map's start view; the TV is in the room the lens mission shows.
for m in L_TowerRtrn_Int_P L_TowerRtrn_Yard_P; do run "$m" 900; done
