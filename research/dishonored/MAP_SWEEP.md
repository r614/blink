# Map load sweep: all 42 base maps

Run on 2026-10-08 with the prebuilt binaries in `.bin` (no cargo builds, game not launched), `DISHONORED_CACHE` = the shared cache.
Scope: load every base map through the port's own loader, run a 10 s headless smoke run of the original Kismet on each, and
queue the original-side collision capture. The rendered side is listed at the end, not run.

## Summary

- **Load: 42 / 42 maps load** through `dishonored_content::load_match` (`oracle snapshot --map M`), exit 0, no panic. Cold 2-15 s (the
  importer builds the collision, nav, NPC and AI caches), warm about 1.1-2.3 s per process (about 0.2 s of that is `load_match`, the rest
  is the process start and the `.snap` write). Total 35.6 M collision triangles over the 42 maps (largest: TowerRtrn_Int 1.81 M,
  Isl_Low/High 1.78 M, Streetsewer 1.72 M).
- **Smoke: 42 / 42 scenarios run to 600 ticks without a panic or error** (`fixtures/oracle/scenarios/maps/*.ron`, 2-7 s each, 50 s for all 42 with 4 jobs).
  The player pawn never falls through the world: on every map it settles 9-17 uu after the spawn point (the spawn hover) and then holds its
  height (the three larger drops are scripted movement, see the table). Original Kismet runs on 41 maps (activations 20-933 in 10 s).
- **Entry event reaches the map on 30 / 42** (the remote `ChangeLvl_*` event of the global sequence does PrepareMapChange and
  CommitMapChange to the scenario's map, with the travel start). 10 maps are reached only with a story state the scenario does not set
  (the event leaves for a sibling map), 1 map stalls (Flooded_FIntro), 1 is the global map.
- Only Boyle Ext / Int have an original-side collision comparison. For the other 40 the query sets are generated and the capture is queued.
- Not measured here: NPC positions (the scenarios place no named NPCs, so NPC fall-through is not covered), render, audio, AI behaviour.

## Method

1. `oracle snapshot --map M` for each map, twice, timed (cold = no cache files for the map; Boyle Ext / Int were already cached; Streets1 was
   cold on the first probe run, 9.7 s). stderr of the cold run holds the importer's warnings. Warm runs print none (the cache replays),
   so Boyle Ext / Int / Streets1 have no warning data (none was seen on the one cold Streets1 run).
2. One scenario per map, `maps/<name>`: `setup: (kismet: true, kismet_start: Some(<entry event>), original_npcs: true)`, no `player` (the default
   spawn of the match, then the Kismet's travel start moves it), no input, 600 ticks, record Pawn / Kismet / SimEvents. Entry events were taken
   from the `ChangeLvl_*` remote events of DishonoredGameFull_P (`kismet find SeqEvent_RemoteEvent`); where several events lead into a map one was chosen
   (listed in each `.ron`). The checks are `Unmeasured` (they never fail); the verdict comes from the trace (pawn z, KismetNote log, `prepare / commit map change` notes).
   Run: `oracle scenario maps/<name>... --jobs 4 --dir <worktree>/fixtures/oracle/scenarios --out <scratch>`.
3. Columns: tris = collision triangles; movers = moving collision actors; nav = navmesh polygons; starts = PlayerStart count (`spawns.json`);
   NPC spw = placed NPC spawners (`npcs*.json`); smoke = Kismet activations / distinct ops that fired / pawns spawned by Kismet in 10 s; drop = pawn z lost after
   tick 6 (the settle); cov = `oracle coverage` static coverage (placed actors / Kismet ops / overall).

## Table

| map | load cold s | warm s | tris | movers | nav | starts | NPC spw | smoke act / ops / spawned | drop uu | cov actors | cov Kismet ops | cov overall | notes |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| DishonoredGameFull_P | 2.2 | 1.1 | 12 | 0 | 0 | 1 | 0 | 0 / 0 / 0 | 14 | 100.0 % | 93.6 % | 92.7 % | global map: no level start event, Kismet not started; no navmesh (no pylons) |
| L_ArtDealer_P | 4.2 | 1.2 | 230497 | 8 | 540 | 4 | 15 | 96 / 51 / 4 | 13 | 91.3 % | 88.9 % | 83.1 % | ok |
| L_Boyle_Ext_P | cached | 1.1 | 876645 | 13 | 984 | 5 | 68 | 354 / 164 / 21 | 14 | 90.1 % | 94.0 % | 85.0 % | ok |
| L_Boyle_Int_P | cached | 1.2 | 927711 | 24 | 684 | 4 | 54 | 490 / 232 / 30 | 13 | 89.3 % | 91.3 % | 83.6 % | ok |
| L_Bridge_Part1a_P | 11.3 | 1.2 | 637350 | 12 | 650 | 3 | 20 | 121 / 63 / 0 | 9 | 91.6 % | 92.2 % | 83.7 % | ok |
| L_Bridge_Part1b_P | 11.3 | 1.2 | 981639 | 82 | 1323 | 2 | 50 | 207 / 101 / 3 | 14 | 90.4 % | 89.6 % | 81.7 % | 5 mover-collision warnings |
| L_Bridge_Part1c_P | 5.2 | 1.2 | 492345 | 5 | 777 | 2 | 17 | 171 / 69 / 4 | 10 | 90.4 % | 93.6 % | 84.0 % | 5 mover-collision warnings |
| L_Bridge_Part2_P | 11.3 | 1.1 | 982239 | 49 | 1165 | 1 | 48 | 213 / 98 / 5 | 11 | 91.1 % | 89.3 % | 81.6 % | ok |
| L_Brothel_P | 9.2 | 1.2 | 1239145 | 34 | 1395 | 4 | 106 | 359 / 141 / 18 | 14 | 90.4 % | 89.2 % | 82.3 % | 3 mover-collision warnings |
| L_Distillery2_P | 6.5 | 1.1 | 667439 | 7 | 697 | 1 | 42 | 154 / 75 / 4 | 13 | 93.0 % | 87.7 % | 82.3 % | ok |
| L_Distillery_P | 7.2 | 1.2 | 689471 | 6 | 774 | 2 | 21 | 194 / 90 / 13 | 13 | 89.9 % | 89.5 % | 81.7 % | ok |
| L_Flooded_FAssassins_P | 9.6 | 1.1 | 1265875 | 7 | 979 | 2 | 32 | 254 / 119 / 17 | 13 | 90.5 % | 89.7 % | 81.8 % | ok |
| L_Flooded_FGate_P | 8.2 | 1.1 | 1595877 | 25 | 2537 | 2 | 39 | 241 / 110 / 10 | 14 | 85.8 % | 89.1 % | 83.0 % | ok |
| L_Flooded_FIntro_P | 5.2 | 1 | 1037288 | 4 | 8 | 1 | 11 | 33 / 17 / 0 | 14 | 86.2 % | 87.0 % | 79.9 % | ENTRY STALL: no commit after PrepareMapChange; navmesh 8 polys |
| L_Flooded_FRefinery_P | 6.1 | 1.1 | 1060379 | 29 | 381 | 2 | 32 | 933 / 79 / 0 | 11 | 88.0 % | 88.0 % | 80.8 % | 16 mover-collision warnings |
| L_Flooded_FStreets_P | 6.2 | 1.1 | 990164 | 1 | 726 | 4 | 58 | 133 / 63 / 2 | 11 | 89.9 % | 85.1 % | 81.2 % | 14 mover-collision warnings |
| L_Galvani1_P | 9.2 | 1.4 | 544774 | 17 | 498 | 3 | 8 | 107 / 54 / 5 | 17 | 94.1 % | 89.5 % | 83.8 % | ok |
| L_Galvani2_P | 12.4 | 1.3 | 543112 | 17 | 498 | 3 | 13 | 181 / 88 / 4 | 17 | 93.5 % | 88.6 % | 83.6 % | ok |
| L_Isl_HighChaos_P | 14.5 | 2.3 | 1778682 | 12 | 1405 | 1 | 66 | 44 / 23 / 0 | 13 | 92.3 % | 91.7 % | 83.7 % | ENTRY REDIRECT: start event goes to L_Isl_LowChaos_P (story state); level's own Kismet only; 1 mover-collision warnings |
| L_Isl_LowChaos_P | 12.4 | 1.2 | 1779525 | 12 | 1320 | 2 | 67 | 230 / 117 / 2 | 10 | 91.4 % | 90.9 % | 82.6 % | 1 mover-collision warnings |
| L_LightH_HighChaos_P | 4.2 | 1.1 | 585584 | 15 | 249 | 1 | 7 | 300 / 145 / 4 | 0 | 97.4 % | 92.5 % | 86.1 % | ok |
| L_LightH_LowChaos_P | 4.2 | 1.2 | 588710 | 15 | 249 | 1 | 8 | 26 / 14 / 0 | 14 | 97.0 % | 90.7 % | 85.0 % | ENTRY REDIRECT: start event goes to L_LightH_HighChaos_P (story state); level's own Kismet only |
| L_Out_HighChaos_EmilyAlive_P | 2.1 | 1.1 | 26684 | 0 | 1 | 1 | 1 | 20 / 11 / 0 | 13 | 84.4 % | 92.7 % | 81.0 % | ENTRY REDIRECT: start event goes to L_Outro_P (story state); level's own Kismet only; navmesh 1 polys |
| L_Out_HighChaos_emilyDead_P | 2.1 | 1.1 | 3838 | 0 | 1 | 1 | 1 | 20 / 11 / 0 | 13 | 82.5 % | 91.9 % | 80.0 % | ENTRY REDIRECT: start event goes to L_Outro_P (story state); level's own Kismet only; navmesh 1 polys |
| L_Out_P | 3.2 | 1.1 | 110189 | 0 | 15 | 1 | 1 | 20 / 11 / 0 | 13 | 88.2 % | 93.0 % | 81.1 % | ENTRY REDIRECT: start event goes to L_Outro_P (story state); level's own Kismet only; navmesh 15 polys |
| L_OutsiderDream_P | 3.2 | 1.1 | 516090 | 2 | 6 | 9 | 1 | 171 / 70 / 0 | 87 | 93.8 % | 70.8 % | 74.9 % | scripted move 288 uu; navmesh 6 polys |
| L_Ovrsr_Back_P | 6.2 | 1.1 | 864931 | 7 | 588 | 2 | 56 | 314 / 149 / 15 | 13 | 90.2 % | 89.2 % | 81.6 % | ok |
| L_Ovrsr_Kennel_P | 3.1 | 1.1 | 174806 | 11 | 245 | 2 | 21 | 163 / 82 / 4 | 13 | 93.3 % | 93.5 % | 86.5 % | travel-start teleport |
| L_Ovrsr_P | 8.2 | 1.1 | 1204348 | 25 | 1144 | 6 | 86 | 381 / 184 / 15 | 0 | 88.9 % | 92.4 % | 83.1 % | travel-start teleport; 3 mover-collision warnings |
| L_Prison_P | 6.2 | 1.1 | 732667 | 23 | 894 | 2 | 45 | 205 / 104 / 0 | 92 | 95.5 % | 91.2 % | 85.5 % | scripted move 493 uu; 1 mover-collision warnings |
| L_PrsnSewer_P | 6.2 | 1.1 | 1029877 | 11 | 732 | 1 | 26 | 96 / 49 / 3 | 10 | 90.8 % | 89.3 % | 82.0 % | 1 mover-collision warnings |
| L_Pub_Assault_P | 7.2 | 1.2 | 711228 | 29 | 846 | 1 | 45 | 68 / 35 / 0 | 14 | 93.5 % | 89.1 % | 84.0 % | ENTRY REDIRECT: start event goes to L_Pub_Morning_P (story state); level's own Kismet only; 2 mover-collision warnings |
| L_Pub_Day_P | 8.2 | 1.1 | 843878 | 32 | 978 | 2 | 29 | 66 / 34 / 0 | 17 | 92.0 % | 90.6 % | 83.6 % | ENTRY REDIRECT: start event goes to L_Pub_Night_P (story state); level's own Kismet only; 5 mover-collision warnings |
| L_Pub_Dusk_P | 8.2 | 1.1 | 854587 | 33 | 992 | 2 | 35 | 70 / 36 / 0 | 17 | 92.8 % | 90.5 % | 83.6 % | ENTRY REDIRECT: start event goes to L_Pub_Morning_P (story state); level's own Kismet only; 5 mover-collision warnings |
| L_Pub_Morning_P | 7.2 | 1.1 | 846154 | 33 | 977 | 3 | 19 | 82 / 42 / 0 | 17 | 92.4 % | 90.7 % | 82.8 % | ENTRY REDIRECT: start event goes to L_Pub_Dusk_P (story state); level's own Kismet only; 7 mover-collision warnings |
| L_Pub_Night_P | 6.2 | 1.1 | 840605 | 27 | 978 | 3 | 10 | 34 / 18 / 0 | 17 | 95.3 % | 92.7 % | 83.9 % | ENTRY REDIRECT: start event goes to L_Pub_Day_P (story state); level's own Kismet only; 1 mover-collision warnings |
| L_Streets1_P | 9.7 | 1.1 | 1295826 | 8 | 1168 | 10 | 54 | 405 / 191 / 26 | 10 | 87.6 % | 89.9 % | 80.9 % | travel-start teleport |
| L_Streets2_P | 9.2 | 1.1 | 1226502 | 6 | 1303 | 12 | 116 | 441 / 173 / 17 | 11 | 89.4 % | 88.3 % | 81.1 % | travel-start teleport; 2 mover-collision warnings |
| L_Streetsewer_P | 9.2 | 1.2 | 1716671 | 6 | 1302 | 2 | 29 | 329 / 144 / 11 | 14 | 93.9 % | 90.2 % | 82.2 % | travel-start teleport; 18 mover-collision warnings |
| L_Tower_P | 4.2 | 1.2 | 320290 | 7 | 668 | 2 | 41 | 215 / 107 / 21 | 10 | 96.0 % | 92.1 % | 86.4 % | ok |
| L_TowerRtrn_Int_P | 11.3 | 2.2 | 1810922 | 34 | 1175 | 5 | 75 | 227 / 113 / 20 | 13 | 83.1 % | 90.4 % | 79.4 % | 2 mover-collision warnings |
| L_TowerRtrn_Yard_P | 11.3 | 1.2 | 961724 | 21 | 1004 | 6 | 84 | 155 / 79 / 10 | 10 | 87.1 % | 93.3 % | 82.9 % | 2 mover-collision warnings |


Cold times are rounded (about 1 s resolution of the timer). Load result is "yes" for all 42; none printed an error.

Notes on the flags:

- **ENTRY REDIRECT**: the global sequence picked another map from story state (Pub variants by time of day / chapter flags, Isl and LightH by chaos,
  Out maps go to `L_Outro_P`). The level's own Kismet still ran (20-82 activations), the match did not switch map, and the pawn settled. To smoke-test the
  global routing on these, the scenario needs the right `story_flags` (read what the chosen handler tests with `kismet find`; not done here).
- **ENTRY STALL**: Flooded_FIntro prints "prepare map change L_Flooded_FIntro_P" and stops (no travel destination or commit, 33 activations total). The remaining chain of
  `fromHub_toFloodedDistrict` is not running: needs a porter to find the op that does not continue (a latent op or a condition on a variable the port never sets).
- **scripted move**: Prison (DraggedIN, 493 uu in 10 s) and OutsiderDream (288 uu, rise and fall) move with no input; this is the level's cutscene / matinee, not a fall.
- **travel-start teleport**: the pawn jumps from the default spawn to the Kismet's `PlayerStart_*` in the first ticks (Streets1 by 601 uu, Streets2 2940, Streetsewer 939, Ovrsr 211, Kennel 120), then holds.
  The default spawn of those maps is not their real start, so a scenario that places the player itself must use the travel start.
- Maps with `cov` Kismet ops below 75 % are the ones with the most unimplemented ops (OutsiderDream 70.8 %, Bridge_Part2, Isl_High, Brothel...).

## What breaks, ranked

Counts are from the load sweep, the smoke traces and `oracle coverage` (all 42 maps; coverage counts are placed instances or op instances).

| # | cause | count | maps | effect observed here |
|---|---|---|---|---|
| 1 | Kismet ops the port does not implement (22 classes with 70+ instances, list below) | 3995 instances in the 22 listed classes alone; per map 6-29 % of op instances unimplemented (OutsiderDream 29 %) | 42 | not visible in the smoke (activations run; the missing ops are skipped silently, no note is logged): conversation / possession / distraction / pickup events never fire, so levels will not react to the player. Unverified how many sequences stall |
| 2 | Placed actors not imported: DisNPCDistractor 1401, DisSplineLoftActor 664, PathTargetPoint 350, DisFish 249, SplineLoftActor 170, DishonoredNote 165, DisHideoutAccessPoint 140 | 3139 | 37 / 18 / 28 / 23 / 14 / 24 / 12 (see below) | NPC distraction props, splines (rails, wires), AI path targets, fish, hideout access are absent |
| 3 | Entry not routed to the map (story state) | 10 maps | Isl_High, LightH_Low, Out x3, Pub x5 | the level starts without its own travel start / chapter flags; smoke covers the level's own sequences only |
| 4 | Movers with no collision (mesh or skeletal mesh missing): DisRiverKrust 63 actors, DishonoredUsableObject 24, DisWatchTower 5, DisAudioLogPlayer 2 | 94 actors | 20 of the 39 maps with a cold run (`DisRiverKrust` 8 maps, `UsableObject` 12, `WatchTower` 4, `AudioLogPlayer` 1) | those objects have no collision at all in the port (usables such as levers and doors' props; river krust and watch towers are not solid). Worst: Streetsewer 18, Flooded_FRefinery 16, Flooded_FStreets 14 |
| 5 | Flooded_FIntro entry stalls after PrepareMapChange | 1 | Flooded_FIntro | no travel start, level Kismet mostly idle (33 activations) |
| 6 | Navmesh essentially absent: Flooded_FIntro 8 polys (11 spawners), OutsiderDream 6, Out 15 / 1 / 1; DishonoredGameFull no pylons | 6 maps | FIntro, Dream, Out x3, Full | no AI path on those maps; plausible for the Out and Dream maps (one spawner each), suspicious for FIntro (11 spawners) |
| 7 | Not covered by this sweep | - | all | NPC fall-through, collision correctness (only Boyle compared with the original), AI, render |

Maps by actor class for #2 (maps with at least one instance, from the worklist): DisNPCDistractor 37, PathTargetPoint 28, DishonoredNote 24, DisFish 23, DisSplineLoftActor 18, SplineLoftActor 14, DisHideoutAccessPoint 12.

Unimplemented Kismet op classes (`oracle coverage` work list, instances over all maps, maps affected):

| op | instances | maps |
|---|---|---|
| `DisSeqEvent_Distracted` | 1387 | 36 |
| `DisSeqEvent_BreakableBroken` | 234 | 27 |
| `DisSeqAct_RefillWhaleOilBattery` | 224 | 10 |
| `SeqAct_SetMatInstScalarParam` | 220 | 14 |
| `SeqAct_ToggleCinematicMode` | 184 | 31 |
| `DisSeqEvent_PatrolPointReached` | 154 | 20 |
| `DisSeqEvent_Possessed` | 154 | 26 |
| `DisSeqEvent_MovablePickedUp` | 134 | 25 |
| `DisSeqAct_SetRatSwarmCustomBehavior` | 129 | 24 |
| `DisSeqEvent_WhaleOilReceptacle` | 125 | 22 |
| `SeqAct_SetMaterial` | 109 | 22 |
| `DisSeqAct_NPCTrackTarget` | 103 | 16 |
| `DisSeqAct_SetAudioOcclusion` | 100 | 20 |
| `DisSeqAct_AIGuard` | 93 | 13 |
| `DisSeqCond_IsSentinel` | 92 | 26 |
| `DisSeqEvent_Combat` | 88 | 18 |
| `DisSeqEvent_BehaviorStarted` | 83 | 11 |
| `SeqAct_SetDamageInstigator` | 81 | 15 |
| `DisSeqEvent_TaskStateChanged` | 79 | 15 |
| `SeqEvent_RigidBodyCollision` | 77 | 17 |
| `DisSeqCond_CompareBoolExtended` | 74 | 8 |
| `DisSeqAct_SetTutorialMessage` | 71 | 13 |

Unimported actor classes (work list):

| class | instances | maps |
|---|---|---|
| `DisNPCDistractor` actors | 1401 | 37 |
| `DisSplineLoftActor` actors | 664 | 18 |
| `PathTargetPoint` actors | 350 | 28 |
| `DisFish` actors | 249 | 23 |
| `SplineLoftActor` actors | 170 | 14 |
| `DishonoredNote` actors | 165 | 24 |
| `DisHideoutAccessPoint` actors | 140 | 12 |

Renderer-side property reads dominate the rest of the coverage work list (StaticMeshComponent.VisibilityId, Texture2D.SizeX, particle LODValidity...), which is the rendered pass's concern, not load.

## Top fixes (most maps unblocked)

1. **Kismet ops, in instance order** (porter, Opus): `DisSeqEvent_Distracted` (1387 instances, 36 maps) together with importing `DisNPCDistractor` (1401, 37 maps): the
   distraction events are the largest single gap and the same maps need both. Then `DisSeqEvent_BreakableBroken` (234), `DisSeqAct_RefillWhaleOilBattery` / `DisSeqEvent_WhaleOilReceptacle`
   (224 + 125, Bridge / Pub maps), `SeqAct_SetMatInstScalarParam` (220), `SeqAct_ToggleCinematicMode` (184, 31 maps), `DisSeqEvent_PatrolPointReached` (154), `DisSeqEvent_Possessed` (154),
   `DisSeqEvent_MovablePickedUp` (134), `DisSeqAct_SetRatSwarmCustomBehavior` (129). Also make the Kismet runtime log an unsupported op once per class (a `KismetNote`)
   so a smoke run lists them instead of skipping silently.
2. **Collision for movers without a mesh**: DisRiverKrust and DisWatchTower (skeletal mesh component missing from the actor's archetype: the importer should read the class default
   component), DishonoredUsableObject / DisAudioLogPlayer (mesh not found: probably the mesh comes from the usable's tweak or an archetype, not the placed actor). 94 actors, 20 maps.
3. **Flooded_FIntro entry**: find the op after PrepareMapChange in `fromHub_toFloodedDistrict` that never continues; check why FIntro has 8 navmesh polys.
4. **Story-state setup for the 10 redirected entries** (scenario-side, Sonnet): add `story_flags` to the Pub / Isl / LightH / Out scenarios from the flags their global handlers test.
5. Import `DisSplineLoftActor` / `SplineLoftActor` (834, 24+ maps) and `PathTargetPoint` (350).
6. Extend the smoke with named NPCs per map (spawner names, tracked by `npc.<id>.z`) to catch NPC fall-through; not possible from the current spec without per-map spawner names (`original_npcs` spawns them but untracked).

Needs an Opus porter: 1 (the ops carry game logic), 2 (importer / archetype resolution), 3 (Kismet latent flow), 5 (spline actors).

## Original-side collision capture (queued)

`oracle gen collision --map M --out <cache>/oracle/queries/M.txt` was run for 41 maps (default sampling: navmesh grid + primitive-directed, `harness-collision-v1`), 192 MB in
`%LOCALAPPDATA%\dishonored-rs\cache\oracle\queries\` (retail-derived: not committed; regenerate in 0.5 s per map). `DishonoredGameFull_P` has no navmesh, so its set is the 20 primitive-directed queries
(`--sampling prim`). Boyle Ext / Int regenerate to the 77283 / 45475 queries of the existing captures (counts match; hashes not compared). Galvani1 / 2 and Pub_Day / Pub_Night generate identical sets (same hash) because
their geometry is the same; both are still queued since their door and mover states differ.

`fixtures/oracle/capture_queue.txt` has one block per map: `session open map=M` (fresh map on the hooked game), a `collide map=M` line (run the map's query set in the session and import it into the oracle cache, `queue run`'s collide line, 2026-10-08),
then the `maps/<name>` smoke scenario (original Kismet trace). `probe_launch queue run fixtures/oracle/capture_queue.txt --only ...` or `--sessions a,b` runs the blocks unattended; by hand, per map:

`
P=target-agents/i686-pc-windows-msvc/release/probe_launch.exe
$P session open map=M
$P collide  <cache>/oracle/queries/M.txt  <scratch>/M.results.txt
.bin/oracle.exe import collision --map M --queries <cache>/oracle/queries/M.txt --results <scratch>/M.results.txt --generator harness-collision-v1
.bin/oracle.exe collision --map M
`

For DishonoredGameFull_P use `--generator "primitive-directed-v1(reach 64)"`. Cost: 4096 checks per pipe request at about 7 k/s (ORACLE.md): the larger sets (Flooded_FGate 165 k, Isl 105-108 k, Streets2 101 k) take 15-25 s each, all 41 about 8 minutes of capture plus a map open each.
The hook's map switch has only been exercised for Boyle; a failed `session open` on another map is itself a result (record it).

## Rendered side: what to check in your pass (not run)

- Missing collision and probably missing meshes: DisRiverKrust (Bridge 1b / 1c, FRefinery, FStreets, Streetsewer, Pub Assault / Dusk / Morning), DisWatchTower (Isl x2, Streets2, TowerRtrn_Yard), DishonoredUsableObject (12 maps above).
- Absent placed actors: splines (Bridge_Part1b, FGate, Isl_Low, Boyle_Ext, FIntro, FRefinery), DisFish (FStreets, Bridge_Part1b, Boyle_Ext...), notes.
- Maps whose Kismet drives much of what is on screen: Flooded_FRefinery (933 activations, 47 matinee binds in 10 s), OutsiderDream (cutscene movement), Prison (DraggedIN), Tower (NewGameEmpress start).
- Near-empty geometry: Out_HighChaos_emilyDead 3838 tris, Out_HighChaos_EmilyAlive 26684, DishonoredGameFull 12 (global map, expected), Out_P 110 k.
- Entry-redirected maps (Pub x5, Isl_High, LightH_Low, Out x3) will look right only if loaded directly; the rendered pass should load them by map name.

### Original-side collision capture: results (2026-10-08)

One hooked session, `probe_launch queue run` over the 42 blocks (`session open`, `collide`, `maps/<name>`): 125 / 126 lines ok in 1263 s. Agreement from `oracle collision --map M` (answered queries that agree; clusters = disagreement clusters). Switch = the `open` line (load s / control s). `DishonoredGameFull_P`: the `open` returns but the session stays on the previous level (the front-end map is not entered by the console `open`), so its collide line fails; its smoke scenario ran on the level it was on.

| map | open | switch s | load / control s | collide | queries | doors | agree (rate), clusters | smoke scenario |
|---|---|---|---|---|---|---|---|---|
| DishonoredGameFull_P | ok | 12.3 | 0.3 / 12.0 | failed |  |  |  | maps/DishonoredGameFull ok |
| L_ArtDealer_P | ok | 2.7 | 0.8 / 1.9 | ok | 31884 | 5 | 31695 / 31698 (99.99 %), 3 clusters | maps/ArtDealer ok |
| L_Boyle_Ext_P | ok | 7.6 | 0.8 / 6.8 | ok | 77283 | 8 | 76800 / 76842 (99.95 %), 18 clusters | maps/Boyle_Ext ok |
| L_Boyle_Int_P | ok | 4.9 | 1.0 / 3.8 | ok | 45475 | 18 | 44680 / 44872 (99.57 %), 35 clusters | maps/Boyle_Int ok |
| L_Bridge_Part1a_P | ok | 8.1 | 0.5 / 7.5 | ok | 44562 | 4 | 44379 / 44562 (99.59 %), 49 clusters | maps/Bridge_Part1a ok |
| L_Bridge_Part1b_P | ok | 6.7 | 0.5 / 6.1 | ok | 94507 | 11 | 93523 / 94364 (99.11 %), 144 clusters | maps/Bridge_Part1b ok |
| L_Bridge_Part1c_P | ok | 3.3 | 0.5 / 2.7 | ok | 50312 | 2 | 50005 / 50172 (99.67 %), 39 clusters | maps/Bridge_Part1c ok |
| L_Bridge_Part2_P | ok | 7.8 | 0.5 / 7.2 | ok | 80330 | 11 | 79749 / 80070 (99.60 %), 83 clusters | maps/Bridge_Part2 ok |
| L_Brothel_P | ok | 5.9 | 1.3 / 4.6 | ok | 94807 | 26 | 93957 / 94142 (99.80 %), 26 clusters | maps/Brothel ok |
| L_Distillery2_P | ok | 6.5 | 0.8 / 5.6 | ok | 44293 | 6 | 43884 / 44000 (99.74 %), 20 clusters | maps/Distillery2 ok |
| L_Distillery_P | ok | 5.5 | 0.8 / 4.7 | ok | 48568 | 6 | 47996 / 48127 (99.73 %), 19 clusters | maps/Distillery ok |
| L_Flooded_FAssassins_P | ok | 5.7 | 0.3 / 5.4 | ok | 71454 | 5 | 70767 / 71093 (99.54 %), 118 clusters | maps/Flooded_FAssassins ok |
| L_Flooded_FGate_P | ok | 7.4 | 0.8 / 6.5 | ok | 164952 | 17 | 163909 / 164426 (99.69 %), 60 clusters | maps/Flooded_FGate ok |
| L_Flooded_FIntro_P | ok | 4.6 | 0.5 / 4.0 | ok | 1143 | 0 | 1131 / 1131 (100.00 %), 0 clusters | maps/Flooded_FIntro ok |
| L_Flooded_FRefinery_P | ok | 4.0 | 0.5 / 3.5 | ok | 28872 | 2 | 28563 / 28632 (99.76 %), 19 clusters | maps/Flooded_FRefinery ok |
| L_Flooded_FStreets_P | ok | 9.0 | 0.5 / 8.4 | ok | 56281 | 0 | 55726 / 55907 (99.68 %), 49 clusters | maps/Flooded_FStreets ok |
| L_Galvani1_P | ok | 3.6 | 0.5 / 3.0 | ok | 30090 | 16 | 29732 / 29876 (99.52 %), 11 clusters | maps/Galvani1 ok |
| L_Galvani2_P | ok | 2.9 | 0.3 / 2.6 | ok | 30090 | 16 | 29295 / 29923 (97.90 %), 33 clusters | maps/Galvani2 ok |
| L_Isl_HighChaos_P | ok | 6.1 | 0.5 / 5.5 | ok | 108145 | 8 | 107854 / 108145 (99.73 %), 106 clusters | maps/Isl_HighChaos ok |
| L_Isl_LowChaos_P | ok | 3.8 | 1.0 / 2.7 | ok | 104256 | 8 | 103948 / 104256 (99.70 %), 111 clusters | maps/Isl_LowChaos ok |
| L_LightH_HighChaos_P | ok | 2.7 | 1.0 / 1.7 | ok | 16875 | 5 | 16598 / 16875 (98.36 %), 49 clusters | maps/LightH_HighChaos ok |
| L_LightH_LowChaos_P | ok | 4.0 | 0.5 / 3.4 | ok | 16875 | 5 | 16612 / 16875 (98.44 %), 51 clusters | maps/LightH_LowChaos ok |
| L_Out_HighChaos_EmilyAlive_P | ok | 4.8 | 0.5 / 4.2 | ok | 111 | 0 | 111 / 111 (100.00 %), 0 clusters | maps/Out_HighChaos_EmilyAlive ok |
| L_Out_HighChaos_emilyDead_P | ok | 3.7 | 0.5 / 3.1 | ok | 120 | 0 | 120 / 120 (100.00 %), 0 clusters | maps/Out_HighChaos_emilyDead ok |
| L_Out_P | ok | 1.6 | 0.5 / 1.0 | ok | 868 | 0 | 496 / 868 (57.14 %), 8 clusters | maps/Out ok |
| L_OutsiderDream_P | ok | 4.0 | 0.5 / 3.4 | ok | 779 | 1 | 779 / 779 (100.00 %), 0 clusters | maps/OutsiderDream ok |
| L_Ovrsr_Back_P | ok | 8.1 | 0.8 / 7.2 | ok | 45616 | 6 | 45173 / 45349 (99.61 %), 27 clusters | maps/Ovrsr_Back ok |
| L_Ovrsr_Kennel_P | ok | 5.5 | 1.1 / 4.4 | ok | 15045 | 7 | 14801 / 14819 (99.88 %), 3 clusters | maps/Ovrsr_Kennel ok |
| L_Ovrsr_P | ok | 11.8 | 0.8 / 10.9 | ok | 88043 | 19 | 86306 / 87608 (98.51 %), 120 clusters | maps/Ovrsr ok |
| L_Prison_P | ok | 5.2 | 1.0 / 4.1 | ok | 59984 | 13 | 59461 / 59984 (99.13 %), 92 clusters | maps/Prison ok |
| L_PrsnSewer_P | ok | 7.6 | 0.8 / 6.8 | ok | 47437 | 7 | 47331 / 47437 (99.78 %), 21 clusters | maps/PrsnSewer ok |
| L_Pub_Assault_P | ok | 6.6 | 0.5 / 6.0 | ok | 59278 | 18 | 58160 / 58800 (98.91 %), 54 clusters | maps/Pub_Assault ok |
| L_Pub_Day_P | ok | 4.7 | 1.1 / 3.6 | ok | 68055 | 22 | 67513 / 68055 (99.20 %), 81 clusters | maps/Pub_Day ok |
| L_Pub_Dusk_P | ok | 5.1 | 0.8 / 4.3 | ok | 68746 | 22 | 68376 / 68746 (99.46 %), 101 clusters | maps/Pub_Dusk ok |
| L_Pub_Morning_P | ok | 4.0 | 0.8 / 3.1 | ok | 68027 | 21 | 67661 / 68027 (99.46 %), 95 clusters | maps/Pub_Morning ok |
| L_Pub_Night_P | ok | 6.8 | 0.5 / 6.2 | ok | 68055 | 21 | 67768 / 68055 (99.58 %), 76 clusters | maps/Pub_Night ok |
| L_Streets1_P | ok | 6.6 | 1.0 / 5.6 | ok | 92279 | 3 | 91679 / 91796 (99.87 %), 49 clusters | maps/Streets1 ok |
| L_Streets2_P | ok | 4.9 | 1.0 / 3.8 | ok | 100818 | 1 | 99878 / 100153 (99.73 %), 36 clusters | maps/Streets2 ok |
| L_Streetsewer_P | ok | 5.4 | 0.8 / 4.6 | ok | 82177 | 3 | 81681 / 81752 (99.91 %), 39 clusters | maps/Streetsewer ok |
| L_Tower_P | ok | 5.6 | 0.8 / 4.7 | ok | 52929 | 0 | 51824 / 52422 (98.86 %), 57 clusters | maps/Tower ok |
| L_TowerRtrn_Int_P | ok | 5.7 | 0.5 / 5.2 | ok | 80622 | 32 | 79846 / 80038 (99.76 %), 48 clusters | maps/TowerRtrn_Int ok |
| L_TowerRtrn_Yard_P | ok | 5.6 | 0.8 / 4.8 | ok | 78093 | 8 | 77576 / 77840 (99.66 %), 65 clusters | maps/TowerRtrn_Yard ok |
