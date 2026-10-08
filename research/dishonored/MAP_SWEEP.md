# Map load sweep: all 42 base maps

Run on 2026-10-08 with the prebuilt binaries in `.bin` (no cargo builds, game not launched), `DISHONORED_CACHE` = the shared cache.
Scope: load every base map through the port's own loader, run a 10 s headless smoke run of the original Kismet on each, and
queue the original-side collision capture. The rendered side is listed at the end, not run.

## Summary

The bullets, table and ranking below are the first sweep's; the current state per map (entries, unsupported ops, collision,
free roam) is in "State after the blocker pass".

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

## State after the blocker pass (2026-10-08, maps porter)

Re-run of the 42 smokes (`oracle scenario --system maps`, own build) and of the 42 cold loads after the fixes below.

- **Entry: 41 / 41 maps now reach their own entry** (the global map has none). Flooded_FIntro commits at `PlayerStart_FloodedIntroStart`
  (447 activations, was 33). The 10 redirected maps start through the right handler: the Pub maps were never story-flag
  redirects, the sweep had picked another chapter's entry event; Isl_HighChaos needs the chaos level, LightH_LowChaos and the
  three Out maps their handlers' flags (each scenario's description quotes the decoded branch).
- **Unsupported ops are visible**: each run's trace now lists, once per class, as `KismetNote`s (name `unsupported` / `not`):
  every event class of a loaded level that the port never activates (with the level's instance count), every op class the
  host does not support, and every op class run as a no-op (`PRESENTATION`, `NOT_MODELLED`). The per-map lists are in the
  table below.
- **Mover collision: 0 warnings** on the 42 cold loads (was 79: 48 DisRiverKrust, 24 DishonoredUsableObject, 5 DisWatchTower,
  2 DisAudioLogPlayer). Collision cache key `collision17`.
- **Spline actors**: already imported (cooked `SplineMeshComponent`s, deformed by `spline_mesh.rs`); they do not collide, as on
  the original. Now counted by name (`ImportStats::spline_loft_actors`), so the coverage work list stops listing them.
- **Free roam**: `dishonored_content::freeroam_def` and `harness freeroam` (below): 19 interior / exterior pairs, both ways,
  58 / 58 checks.

### Fixes

| blocker | cause (decoded) | fix |
|---|---|---|
| Flooded_FIntro entry stall | `fromHub_toFloodedDistrict` continues from `DisSeqAct_BackupAndClearInventory`'s output 1 "Finished (After Unequip)"; the op was unsupported, so only its latent "Out" fired. `Activated` 0x00ba91d0 fires "Out", asks each target to unequip hand slots 1 and 2 when they are not empty (0x00b4e070, 0x00b51790), else backs up / clears the inventory (0x00ba8980) and fires output 1; `Update` 0x00baf050 waits for empty hands | host op: "Out" and output 1 in `Activated` (the port has no hand-slot unequip, so the empty-hands path). NOT PORTED: the unequip and the inventory backup / clear (0x00ba8980) |
| ops skipped silently | the interpreter logged unsupported classes only in its own log; event classes never fired showed nothing; the host's no-op lists counted silently | `Kismet::notices` (drained into the host history), `Host::fires_event` + `kismet_host::FED_EVENTS` checked when a level loads, first activation of a no-op class noted |
| DisRiverKrust / DisWatchTower no collision | their skeletal component is a native member (`m_pSkeletalMeshCpnt`, `m_pTowerMeshCpnt`); only `CollisionComponent` names it among the serialized properties; the component carries `SkeletalMesh` + `PhysicsAsset` | `prop_collision::read_component_bodies`: the collision source's own component (else `SkeletalMeshComponent`, else a skeletal `CollisionComponent`), physics asset bodies at the reference pose |
| DishonoredUsableObject / DisAudioLogPlayer "mesh not found" | their `SkeletalMeshComponent0` has no mesh on the component nor its archetype: nothing to collide with in the original either; the visible / colliding part is the static `StaticUsablePart`, which the level importer already imports | mesh-less skeletal sources skipped |
| SetMatInstScalarParam (220) | 0x006d1d90: `MatInst.SetScalarParameterValue(ParamName, ScalarValue)`, or with `m_pMaterialOwnerPawn` on that pawn mesh's material `m_iMaterialIndex` | host records the values (`HostState::mat_scalars` / `pawn_mat_scalars`); the renderer does not read them yet |
| ToggleCinematicMode (184) | `PlayerController.OnToggleCinematicMode` -> `SetCinematicMode` (script) -> native 0x00aaf150: move / look input ignored while on, HUD / player hidden, the pawn's action permissions swapped (disable 0x7faf, enable 0x6010) | `HostState::cinematic`, `kismet_host::cinematic_inputs` (movement, view, buttons); the per-bit meaning of 0x7faf is not decoded (every button refused); HUD / hide not in the view |
| SetMaterial (109) | `OnSetMaterial` of `DynamicSMActor` / `SkeletalMeshActor` / `Pawn`: `SetMaterial(MaterialIndex, NewMaterial)` | recorded per actor / pawn (`HostState::materials`); not drawn yet |
| AIGuard (93) | `OnAIGuard` 0x00b63a80: guard stimulus on `m_pHomeActor` with `m_fGuardAwarenessRadius` (Start), ends it (Stop) | brain walks to the home actor and holds; the awareness radius is not modelled |
| IsSentinel (92) | 0x00b8f070: output 0 when `WorldInfo.Game.MyAutoTestManager` exists, else 1 | output 1 (no automated-test mode) |
| CompareBoolExtended (74) | 0x00b9b3e0: AND / OR over "Bool"; all -> 0, some -> 2, none -> 1 | ported |

Left open (decoded where each is fired could not be found in this pass, or no system in the port): `DisSeqEvent_PatrolPointReached`,
`Possessed`, `MovablePickedUp`, `Combat`, `BehaviorStarted`, `TaskStateChanged` (no direct reference to their classes in the static
index), `SetRatSwarmCustomBehavior`, `NPCTrackTarget`, `SetAudioOcclusion`, `SetDamageInstigator` (no such systems), `SetTutorialMessage`
(presentation). Not mine: `DisSeqEvent_Distracted` / `DisNPCDistractor`, the whale-oil batteries, `BreakableBroken`.

### Per map (run of 2026-10-08)

act = `Activated` script events in 10 s (the first sweep counted another way), ops = distinct ops activated, spawned = pawns spawned by
the Kismet. Events: class (without its `DisSeqEvent_` / `SeqEvent_` prefix) and the level's instance count. Ops: (p) presentation no-op,
(n) no such system in the port, (u) unsupported; a class is listed on the map where its first activation fell.

| map | entry commit (tag) | act / ops / spawned | drop uu | events never activated (classes: instances in the level) | ops run as no-ops or unsupported |
|---|---|---|---|---|---|
| ArtDealer | L_ArtDealer_P () | 56 / 55 / 4 | 2 | Distracted:23, Lock:2, MovablePickedUp:1, Possessed:4, RigidBodyCollision:1 |  |
| Boyle_Ext | L_Boyle_Ext_P (PlayerStart_SamuelDropOff) | 184 / 164 / 21 | 5 | BehaviorStarted:11, BreakableBroken:1, Combat:1, Corpse:1, Distracted:7, FleepointReached:1, Investigate:1, Possessed:9, PowerUsed:2, ParticleEvent:1 | DisSeqAct_SetCurrentChapter(p), SeqAct_StreamInTextures(p) |
| Boyle_Int | L_Boyle_Int_P () | 253 / 232 / 30 | 2 | BehaviorStarted:24, BreakableBroken:43, Combat:1, Distracted:58, KeyHoleUsed:2, MovablePickedUp:8, PatrolPointReached:20, Possessed:13, PowerUsed:1, Windblasted:1, DisSeqEvt_Choke:1, ParticleEvent:1, RigidBodyCollision:8 |  |
| Bridge_Part1a | L_Bridge_Part1a_P (PlayerStart_SamuelDropOff) | 64 / 63 / 0 | 0 | BreakableBroken:4, Distracted:15, MovablePickedUp:2, Possessed:3, TaskActivated:1, WhaleOilReceptacle:3, RigidBodyCollision:2 | DisSeqAct_SetCurrentChapter(p), SeqAct_StreamInTextures(p), ArkSeqAct_ChangePylonConnection(u) |
| Bridge_Part1b | L_Bridge_Part1b_P (PlayerStart_BridgePart1b) | 107 / 101 / 3 | 5 | BreakableBroken:1, Combat:1, Distracted:23, Lock:2, MovablePickedUp:1, PatrolPointReached:1, PlayerCombat:1, Possessed:3, TaskStateChanged:2, WhaleOilReceptacle:6, RigidBodyCollision:1 | ArkSeqAct_ChangePylonConnection(u) |
| Bridge_Part1c | L_Bridge_Part1c_P (PlayerStart_BridgePart1c) | 89 / 69 / 4 | 2 | BreakableBroken:2, Distracted:7, Possessed:1, PowerUsed:1, WhaleOilReceptacle:1, Destroyed:1 |  |
| Bridge_Part2 | L_Bridge_Part2_P (PlayerStart_BridgePart2) | 111 / 98 / 5 | 2 | BehaviorStarted:1, BreakableBroken:6, Corpse:2, Distracted:34, KeyHoleUsed:1, Lock:2, MovablePickedUp:2, PatrolPointReached:2, PlayerCombat:2, PlayerHeard:2, Possessed:7, TaskActivated:1, TaskStateChanged:5, WhaleOilBattery:1, WhaleOilReceptacle:8, DisSeqEvt_Choke:1 | DisSeqCond_CompareTweaks(u) |
| Brothel | L_Brothel_P () | 235 / 179 / 21 | 2 | BehaviorStarted:1, BreakableBroken:2, Combat:3, Corpse:2, Distracted:71, KeyHoleUsed:3, Lock:1, MovablePickedUp:6, PatrolPointReached:1, Possessed:18, Windblasted:2, Destroyed:1, RigidBodyCollision:6 | DisSeqAct_LimitPawnMinHealth(n), DisSeqAct_OverridePossess(n), DisSeqAct_Highlight(u) |
| DishonoredGameFull | (global map, no entry) | 0 / 0 / 0 | 2 |  |  |
| Distillery | L_Distillery_P (PlayerStart_DistilleryMainEntrance) | 99 / 90 / 13 | 5 | Distracted:34, PatrolPointReached:5, PawnSighted:1, PlayerCombat:1, PowerUsed:1, Tripwire:2, WhaleOilReceptacle:1 | DisSeqAct_NPCTrackTarget(n), DisSeqAct_SetRainEmitter(u), DisSeqAct_SpawnCameraLensEffect(u), DisSeqCond_IsPlayerInCombat(u) |
| Distillery2 | L_Distillery2_P (PlayerStart_DistilleryMainEntrance) | 92 / 89 / 4 | 5 | BehaviorStarted:1, Combat:26, Distracted:36, Possessed:1, WhaleOilReceptacle:1 | SeqAct_StreamInTextures(p), DisSeqCond_IsObjectiveComplete(u) |
| Flooded_FAssassins | L_Flooded_FAssassins_P (PlayerStart_FAssassinsStart) | 130 / 120 / 17 | 5 | Assassinated:1, AttackedByRats:1, Distracted:70, Investigate:1, PatrolPointReached:4, PlayerHolsterWeapon:1, Possessed:1, PowerUsed:6, TaskStateChanged:5 | DisSeqAct_SetCurrentChapter(p), DisSeqAct_ApplyPlayerLoadout(n), DisSeqAct_ToggleTutorial(p), DisSeqAct_LimitPawnMinHealth(n), DisSeqAct_Highlight(u), DisSeqAct_OverrideAwarenessDisplay(u), SeqAct_ModifyObjectList(u) |
| Flooded_FGate | L_Flooded_FGate_P (PlayerStart_StreetSewerEntrance) | 123 / 111 / 10 | 5 | BehaviorStarted:32, BreakableBroken:3, Combat:7, Distracted:67, Investigate:1, KeyHoleUsed:2, Lock:1, ObjectiveCompleted:1, PatrolPointReached:14, Possessed:9, TaskStateChanged:8, WhaleOilBattery:1, WhaleOilReceptacle:4, Destroyed:1, ParticleEvent:1 | DisSeqAct_SetCurrentChapter(p), DisSeqAct_ApplyPlayerLoadout(n), DisSeqAct_ToggleTutorial(p), DisSeqAct_EvalAchievement(p), DisSeqAct_AISetBrainFlags(u) |
| Flooded_FIntro | L_Flooded_FIntro_P (PlayerStart_FloodedIntroStart) | 447 / 55 / 2 | 2 | MovablePickedUp:2, WhaleOilBattery:2, WhaleOilReceptacle:2 | DisSeqAct_SetCurrentChapter(p), SeqAct_StreamInTextures(p) |
| Flooded_FRefinery | L_Flooded_FRefinery_P (PlayerStart_FRefineryAR) | 472 / 80 / 0 | 3 | BreakableBroken:6, Combat:3, Distracted:13, MovablePickedUp:2, Possessed:3, PowerUsed:1, TaskStateChanged:2, WhaleOilBattery:2, WhaleOilReceptacle:4 | DisSeqAct_SetCurrentChapter(p), DisSeqAct_ApplyPlayerLoadout(n), DisSeqAct_ToggleTutorial(p), DisSeqAct_Highlight(u) |
| Flooded_FStreets | L_Flooded_FStreets_P (PlayerStart_FStreetsStart) | 69 / 64 / 2 | 2 | BreakableBroken:11, Combat:7, Distracted:51, Lock:2, PlayerInventoryChanged:1, Possessed:2, PowerUsed:1, TaskStateChanged:2 | DisSeqAct_SetCurrentChapter(p), DisSeqAct_ApplyPlayerLoadout(n), DisSeqAct_ToggleTutorial(p), DisSeqAct_Highlight(u), SeqAct_ModifyObjectList(u) |
| Galvani1 | L_Galvani1_P () | 56 / 55 / 5 | 5 | BreakableBroken:1, Distracted:40, KeyHoleUsed:1, Lock:1, MovablePickedUp:2, PatrolPointReached:4, PlayerCombat:1, PowerUsed:1, RigidBodyCollision:2 | DisSeqCond_IsPlayerInCombat(u) |
| Galvani2 | L_Galvani2_P () | 101 / 95 / 4 | 5 | BreakableBroken:1, Distracted:47, KeyHoleUsed:1, Lock:1, MovablePickedUp:2, PatrolPointReached:3, PlayerCombat:1, Possessed:4, PowerUsed:1, RigidBodyCollision:2 | DisSeqCond_HasObjective(u), DisSeqCond_IsPlayerInCombat(u) |
| Isl_HighChaos | L_Isl_HighChaos_P (PlayerStart_Beach) | 144 / 137 / 1 | 2 | Assassinated:1, AttackedByRats:1, Combat:1, Distracted:76, PatrolPointReached:1, PawnSighted:2, Possessed:8, PowerUsed:1, TaskStateChanged:9, WatchTower:3, WhaleOilReceptacle:5, Windblasted:1, ParticleEvent:8 | SeqAct_StreamInTextures(p), DisSeqAct_OverridePossess(n), DisSeqAct_SetRainEmitter(u), DisSeqAct_SpawnCameraLensEffect(u), SeqAct_ModifyObjectList(u), DisSeqAct_SetPlayerVisSettings(u), SeqAct_AndGate(u), DisSeqAct_SetCurrentChapter(p) |
| Isl_LowChaos | L_Isl_LowChaos_P (PlayerStart_Beach) | 118 / 117 / 2 | 2 | Distracted:76, PatrolPointReached:1, PawnSighted:2, Possessed:6, TaskStateChanged:7, WatchTower:3, WhaleOilReceptacle:6 | SeqAct_StreamInTextures(p), DisSeqAct_OverridePossess(n), SeqAct_ModifyObjectList(u), DisSeqAct_SetPlayerVisSettings(u), SeqAct_AndGate(u), DisSeqAct_SetCurrentChapter(p) |
| LightH_HighChaos | L_LightH_HighChaos_P (PlayerStart_elevator) | 153 / 145 / 4 | 0 | Assassinated:2, Combat:1, Distracted:4, Possessed:1, PowerUsed:1, RatPossess:1, TaskStateChanged:5, WhaleOilReceptacle:2, Windblasted:1, DisSeqEvt_Choke:1, ParticleEvent:2 | DisSeqAct_SetRainEmitter(u), DisSeqAct_SpawnCameraLensEffect(u), DisSeqAct_SetPlayerVisSettings(u), DisSeqAct_AISetBrainFlags(u), DisSeqAct_OverrideAwarenessDisplay(u) |
| LightH_LowChaos | L_LightH_LowChaos_P (PlayerStart_elevator) | 105 / 105 / 4 | 0 | Combat:5, Distracted:8, Lock:2, Possessed:6, PowerUsed:1, TaskStateChanged:4, WhaleOilReceptacle:2 | DisSeqAct_SetPlayerVisSettings(u), DisSeqAct_OverrideAwarenessDisplay(u) |
| Out | L_Out_P (PlayerStart_Outro) | 62 / 61 / 1 | 8 |  | DisSeqAct_EvalAchievement(p), DisSeqAct_SetCurrentChapter(p), SeqAct_StreamInTextures(p), SeqAct_MultiLevelStreaming(u), DisSeqAct_BendTime(n) |
| Out_HighChaos_EmilyAlive | L_Out_HighChaos_EmilyAlive_P (PlayerStart_Outro) | 42 / 42 / 1 | 8 |  | DisSeqAct_EvalAchievement(p), DisSeqAct_SetCurrentChapter(p), SeqAct_StreamInTextures(p), DisSeqAct_BendTime(n) |
| Out_HighChaos_emilyDead | L_Out_HighChaos_emilyDead_P (PlayerStart_Outro) | 44 / 44 / 1 | 8 |  | DisSeqAct_EvalAchievement(p), DisSeqAct_SetCurrentChapter(p), SeqAct_StreamInTextures(p), DisSeqAct_BendTime(n) |
| OutsiderDream | L_OutsiderDream_P (PlayerStart_DreamBegin) | 87 / 70 / 0 | 79 | JournalViewed:1, KeyHoleUsed:1, MovablePickedUp:1, PlayerManaThreshold:1, PowerEquipped:2, PowerUsed:1, ParticleEvent:1, RigidBodyCollision:1 | DisSeqAct_SetCurrentChapter(p), DisSeqAct_LimitPawnMinHealth(n), DisSeqAct_ToggleHUDElement(p), DisSeqAct_EquipItemType(n), DisSeqAct_OutsiderConfig(n), SeqAct_StreamInTextures(p), DisSeqAct_SpawnCameraLensEffect(u) |
| Ovrsr | L_Ovrsr_P (PlayerStart_SquareEntrance) | 196 / 184 / 15 | 0 | BehaviorStarted:3, BreakableBroken:5, Combat:5, Corpse:4, Distracted:88, KeyHoleUsed:3, Lock:1, MovablePickedUp:1, PlayerCombat:2, PlayerCrouch:1, Possessed:20, TaskStateChanged:1, AnimNotify:1, RigidBodyCollision:1 | DisSeqAct_SetCurrentChapter(p), DisSeqAct_LimitPawnMinHealth(n), DisSeqAct_OverridePossess(n), DisSeqAct_SetRainEmitter(u), DisSeqAct_SpawnCameraLensEffect(u), DisSeqAct_RemoveInventoryItem(u), DisSeqAct_ModifyAmmo(u), DisSeqCond_HasObjective(u) |
| Ovrsr_Back | L_Ovrsr_Back_P (PlayerStart_AlleyEntrance) | 162 / 149 / 15 | 5 | BreakableBroken:9, Distracted:54, KeyHoleUsed:2, Lock:1, MovablePickedUp:1, PawnSighted:1, Possessed:6, Tripwire:1, AnimNotify:1, HitWall:1, RigidBodyCollision:1 | DisSeqAct_LimitPawnMinHealth(n), DisSeqAct_SetRainEmitter(u), DisSeqAct_SpawnCameraLensEffect(u), DisSeqAct_AISetBrainFlags(u) |
| Ovrsr_Kennel | L_Ovrsr_Kennel_P (PlayerStart_KennelSewer) | 83 / 82 / 4 | 5 | BreakableBroken:1, Combat:2, Distracted:15, PawnSighted:1, PlayerCombat:1, PlayerHeard:1 | DisSeqAct_LimitPawnMinHealth(n), DisSeqAct_SetAudioOcclusion(u) |
| Prison | L_Prison_P (PlayerStart_DraggedIN) | 105 / 104 / 0 | 84 | BreakableBroken:2, Combat:8, Distracted:9, Interact:4, KeyHoleUsed:3, ObjectiveCompleted:2, PlayerCombat:3, PlayerCrouch:1, TaskStateChanged:7, DisSeqEvt_Choke:1, ParticleEvent:11 | DisSeqAct_GetPlayerStat(p), DisSeqAct_SetCurrentChapter(p), SeqAct_StreamInTextures(p), DisSeqAct_TogglePowerWheel(p), DisSeqAct_ToggleHUDElement(p), DisSeqAct_ModifyAmmo(u), DisSeqAct_AdrenalineToggle(u), DisSeqAct_RemoveInventoryItem(u), DisSeqAct_SetTutorialMessage(p) |
| PrsnSewer | L_PrsnSewer_P (PlayerStart_SewerStart) | 49 / 49 / 3 | 2 | BreakableBroken:3, Climb:1, Corpse:3, Distracted:19, MovablePickedUp:1, PatrolPointReached:2, Tripwire:4, Destroyed:3, ParticleEvent:1, RigidBodyCollision:2 | DisSeqAct_SetCurrentChapter(p), DisSeqAct_EquipItemType(n), DisSeqAct_ToggleHUDElement(p), DisSeqAct_AdrenalineToggle(u) |
| Pub_Assault | L_Pub_Assault_P (PlayerStart_HoundPitsFromFlooded) | 205 / 171 / 20 | 3 | AttackedByRats:2, BehaviorStarted:4, BreakableBroken:3, Distracted:16, Investigate:2, KeyHoleUsed:5, Lock:2, MovablePickedUp:1, PatrolPointReached:11, Possessed:8, TaskStateChanged:19, WhaleOilBattery:1, WhaleOilReceptacle:4 | DisSeqAct_SetCurrentChapter(p), DisSeqAct_ApplyPlayerLoadout(n), DisSeqAct_ToggleTutorial(p), SeqAct_StreamInTextures(p), DisSeqAct_Highlight(u), SeqAct_GetDistance(u), DisSeqAct_SetAudioOcclusion(u) |
| Pub_Day | L_Pub_Day_P (PlayerStart_HubDefault) | 244 / 230 / 21 | 0 | BreakableBroken:3, Combat:1, Distracted:47, KeyHoleUsed:6, MovableDropped:1, MovablePickedUp:4, PatrolPointReached:16, TaskStateChanged:1, WhaleOilBattery:6, WhaleOilReceptacle:2, Windblasted:1, Destroyed:1 | DisSeqAct_SetCurrentChapter(p), SeqAct_StreamInTextures(p), DisSeqAct_ToggleHUDElement(p), DisSeqAct_LimitPawnMinHealth(n), DisSeqAct_OverridePossess(n), DisSeqAct_EquipItemType(n) |
| Pub_Dusk | L_Pub_Dusk_P (PlayerStart_HubDefault) | 295 / 272 / 26 | 0 | BreakableBroken:2, Distracted:64, MovablePickedUp:2, PatrolPointReached:22, Possessed:11, WhaleOilBattery:5, WhaleOilReceptacle:1, Destroyed:1 | DisSeqAct_ToggleHUDElement(p), DisSeqAct_TogglePowerWheel(p), SeqAct_ToggleHUD(p), DisSeqAct_ToggleJournal(p), DisSeqAct_SetCurrentChapter(p), DisSeqAct_ToggleTutorial(p), DisSeqAct_EquipItemType(n), SeqAct_StreamInTextures(p), DisSeqAct_RemoveInventoryItem(u), DisSeqAct_Highlight(u), SeqAct_CastToFloat(u), DisSeqCond_IsDLCUnlocked(u), DisSeqAct_NPCTrackTarget(n) |
| Pub_Morning | L_Pub_Morning_P (PlayerStart_HubDefault) | 194 / 189 / 14 | 5 | BreakableBroken:2, Distracted:36, KeyHoleUsed:5, Lock:2, MovablePickedUp:2, PatrolPointReached:1, WhaleOilBattery:5, WhaleOilReceptacle:1, Destroyed:4 | DisSeqAct_ToggleHUDElement(p), DisSeqAct_TogglePowerWheel(p), SeqAct_ToggleHUD(p), DisSeqAct_ToggleJournal(p), DisSeqAct_SetCurrentChapter(p), SeqAct_StreamInTextures(p), DisSeqAct_EquipItemType(n), DisSeqAct_SetAudioOcclusion(u), DisSeqCond_IsDLCUnlocked(u) |
| Pub_Night | L_Pub_Night_P (PlayerStart_HubDefault) | 138 / 138 / 2 | 3 | BreakableBroken:2, Distracted:6, KeyHoleUsed:1, MovablePickedUp:2, WhaleOilBattery:5, WhaleOilReceptacle:1, Windblasted:1, Destroyed:1 | DisSeqAct_ToggleHUDElement(p), DisSeqAct_TogglePowerWheel(p), SeqAct_ToggleHUD(p), DisSeqAct_ToggleJournal(p), DisSeqAct_SetCurrentChapter(p), SeqAct_StreamInTextures(p), DisSeqAct_EquipItemType(n), DisSeqCond_IsDLCUnlocked(u), DisSeqAct_NPCTrackTarget(n), DisSeqAct_LimitPawnMinHealth(n) |
| Streets1 | L_Streets1_P (PlayerStart_DockArrival) | 985 / 221 / 26 | 0 | BreakableBroken:5, Combat:1, Corpse:1, Distracted:46, FleepointReached:4, KeyHoleUsed:2, Lock:1, MovablePickedUp:1, PatrolPointReached:21, PlayerCombat:1, Possessed:3, PowerUsed:1, WhaleOilReceptacle:6, DisSeqEvt_Choke:1, Destroyed:3, RigidBodyCollision:1 | DisSeqAct_ToggleAchievementEval(p), DisSeqAct_GetPlayerStat(p), DisSeqAct_EquipItemType(n), DisSeqAct_SetCurrentChapter(p), SeqAct_StreamInTextures(p), DisSeqAct_OutsiderConfig(n), DisSeqAct_BendTime(n), DisSeqAct_SetRainEmitter(u), DisSeqAct_SpawnCameraLensEffect(u), DisSeqAct_FireProjectile(u), DisSeqAct_SetRatSwarmCustomBehavior(n) |
| Streets2 | L_Streets2_P (PlayerStart_DockArrival) | 252 / 199 / 18 | 2 | BreakableBroken:4, Combat:5, Distracted:85, Lock:4, MovablePickedUp:1, PatrolPointReached:9, PlayerCombat:3, Possessed:2, PowerUsed:1, WallOfLight:1, WatchTower:2, WhaleOilBattery:3, WhaleOilReceptacle:2, Windblasted:1, RigidBodyCollision:1 | DisSeqAct_EquipItemType(n), DisSeqAct_SetCurrentChapter(p), SeqAct_StreamInTextures(p), DisSeqAct_AISetBrainFlags(u) |
| Streetsewer | L_Streetsewer_P (PlayerStart_HoundPitsFromFlooded) | 169 / 144 / 11 | 5 | AttackedByRats:1, BreakableBroken:5, Corpse:1, Distracted:19, Lock:1, MovablePickedUp:1, PatrolPointReached:12, Possessed:2, PowerUsed:1, TeleportSpell:1, Windblasted:1, DisSeqEvt_Choke:1, Destroyed:2, RigidBodyCollision:1 | DisSeqAct_SetCurrentChapter(p), SeqAct_ModifyObjectList(u), DisSeqAct_NPCDoTeleportSpell(u), DisSeqAct_OverridePossess(n), DisSeqAct_LimitPawnMinHealth(n), DisSeqAct_SetAudioOcclusion(u) |
| Tower | l_tower_p (PlayerStart_NewGameEmpress) | 159 / 153 / 23 | 2 | BehaviorStarted:3, Combat:10, Distracted:4, PlayerCrouch:2, TaskStateChanged:2 | DisSeqAct_SetCurrentChapter(p), SeqAct_StreamInTextures(p), DisSeqAct_ToggleJournal(p), DisSeqAct_ToggleHUDElement(p), DisSeqAct_ToggleChoke(u), DisSeqAct_RemoveInventoryItem(u), SeqAct_CastToFloat(u), DisSeqCond_CompareTweaks(u), DisSeqAct_TogglePowerWheel(p) |
| TowerRtrn_Int | L_TowerRtrn_Int_P () | 115 / 113 / 20 | 2 | BehaviorStarted:2, BreakableBroken:1, Corpse:1, Distracted:86, MovablePickedUp:7, PatrolPointReached:4, PawnSighted:2, WhaleOilReceptacle:2, RigidBodyCollision:7 | DisSeqAct_ActivateRatSpawner(n), DisSeqAct_ApplyPlayerLoadout(n), DisSeqAct_LimitPawnMinHealth(n), DisSeqCond_IsObjectiveComplete(u), DisSeqAct_RBConstraint(u) |
| TowerRtrn_Yard | L_TowerRtrn_Yard_P (PlayerStart_SamuelDropOff) | 79 / 79 / 10 | 2 | BehaviorStarted:1, BreakableBroken:4, Corpse:1, Distracted:33, Possessed:3, WatchTower:1, WhaleOilReceptacle:2 | SeqAct_StreamInTextures(p), DisSeqAct_ActivateRatSpawner(n) |

Events never activated, by the number of maps (from the runs): Distracted 36, BreakableBroken 27, Possessed 26, MovablePickedUp 23,
WhaleOilReceptacle 22, PatrolPointReached 20, Combat 18, Lock 16, PowerUsed 16, TaskStateChanged 15, RigidBodyCollision 15, KeyHoleUsed 15,
BehaviorStarted 11, WhaleOilBattery 10, Destroyed 10, PlayerCombat 10, Corpse 9, ParticleEvent 8, Windblasted 8, PawnSighted 6, Choke 6, then
19 classes on 1-4 maps. Unsupported ops met (maps): DisSeqAct_SpawnCameraLensEffect 7, SetRainEmitter 6, Highlight 6, SeqAct_ModifyObjectList 5,
AISetBrainFlags 4, SetAudioOcclusion 4, SetPlayerVisSettings 4, RemoveInventoryItem 4, IsPlayerInCombat 3, IsDLCUnlocked 3,
OverrideAwarenessDisplay 3, then 13 classes on 1-2 maps (CastToFloat, AndGate, IsObjectiveComplete, HasObjective, ModifyAmmo,
ArkSeqAct_ChangePylonConnection, CompareTweaks, FireProjectile, ToggleChoke, RBConstraint, MultiLevelStreaming, NPCDoTeleportSpell, GetDistance).

### Collision against the original (the hook's captures, port rebuilt with this branch)

`oracle collision --map M --rebuild` (snapshots rebuilt in a scratch oracle root) on the 19 maps with river krusts, watch towers or the
mesh-less usables: agreement unchanged within a few queries (e.g. Streetsewer 99.91 %, FRefinery 99.76 %, Streets2 99.73 %, FStreets
99.66 % (-9 queries)). The krust bodies now answer where the original's do, within a few percent of the check length (the original poses
the creature by its idle animation, the port at the reference pose: Streetsewer `DisRiverKrust_30` 6 checks, port 0.3-1.3 % farther; FStreets
10 checks where a reference-pose body reaches past the animated one). The watch towers' own disagreements are on their static base
(`StaticMeshComponent0`, imported with the level's components: Streets2 113 + 88 checks), not on the skeletal tower (1 check each).

Coordinator follow-ups (top clusters, not fixed here):

- **L_Out_P 57.14 % (496 / 868)**: structural. 364 of the 372 disagreements are "port blocks where the original does not" on
  `modular_rocks` meshes of `L_Out_EnvLabPiero` (`StaticMeshActor_19610`, collection components 24, 25, 72). L_Out_P has ten story
  variant sublevels (`L_Out_EnvSamAlive / SamDead / CallistaCurnow / CallistaAlive / CallistaDead / LabEmpty / LabSokolov / LabPiero /
  LabPieroAndSokolov`) that its Kismet streams in by story state (`SeqAct_MultiLevelStreaming`, unsupported in the port); the port's
  importer loads every `WorldInfo.StreamingLevels` package (`dishonored_assets::map_packages`), the original only the streamed ones (none
  of the Lab variants at the capture). Fix: per-sublevel collision layers toggled by the Kismet's streaming ops (and `LevelStreaming`
  initial state), the same mechanism the other Kismet-streamed variants (Pub, Isl...) will need.
- **Ovrsr 98.51 %**: top 3 clusters (161 + 151 + 135) are `L_Ovrsr_Script:StaticMeshActor_2223` (`Env_Blockout.Cube_100`, scale 9.5 x 10
  x 0.5): the port blocks, the original misses; its `StaticMeshComponent` was cooked into the persistent `StaticMeshCollectionActor`
  (`StaticMeshActor_SMC_2223`), not referenced by any Kismet. **Tower 98.86 %**: the same pattern, `L_Tower_Script:StaticMeshActor_2`
  (`platform_small_01`, `bHidden`, 202 + 70 checks where the port hits nearer). Both look like the script-package blockers whose cooked
  component the original does not collide with; to decode (component / actor flags of the cooked collection entry).
- **Galvani2 97.90 %**: `DisDoor_4` / `DisDoor_20` (120 / 96, both directions: door state or pose at the capture) and
  `L_Galvani1_Scripts:InterpActor_3` (92). **LightH High / Low 98.4 %**: `InterpActor_4` / `_5` (41 + 38, port blocks: matinee movers at
  another position on the original) and `DisDoor_1` (55).
- DishonoredGameFull_P: not opened on the original (the console `open` stays on the previous level); no comparison.

### Free roam between a level's maps

`dishonored_content::freeroam_def(map, seed)` builds the session a free-roam launcher runs: the map, the other maps of its campaign level
(`level_group`), their original spawners, the original Kismet started by the map's campaign entry (`entry_event`: a global `ChangeLvl_*`
handler that prepares only this map, the mission's own entry preferred), `SF_Global_RealGameStarted` set (a game started from the menu; the
level doors test it before their `ChangeLvl_*`). The level's maps come from the travel data: `dishonored_assets::travel::map_exits` lists the
`ChangeLvl_*` events a map's gameplay events reach (not the developers' console skips) with every destination their global handler can
prepare and whether it crosses a mission boundary (`DisSeqAct_MissionStatsTracking` / `ShowMissionStats`); two maps belong together when
each can reach the other without a boundary. A level change out of the level (the boat) ends the session as it ends a mission.

`harness freeroam` (58 / 58): for each pair, a session on one map, then the agent uses the door (or enters the trigger) whose Kismet leads
to the other map's `ChangeLvl_*`, answering the confirmation with "go"; checked both ways: the other map, the tagged `PlayerStart` of the
travel (0 uu on every arrival), not falling, the inventory kept (coins carried across). Pairs (7 levels):
Boyle Ext / Int; Bridge 1a / 1b, 1b / 1c, 1c / 2; Flooded FAssassins / FStreets, FAssassins / FGate, FRefinery / FStreets, FGate / Streetsewer;
Prison / PrsnSewer; TowerRtrn Int / Yard; Streets1 / Distillery, Galvani1, Ovrsr; Ovrsr / Ovrsr_Back, Ovrsr_Kennel;
Streets2 / ArtDealer, Galvani2, Distillery2, Brothel. Notes: FStreets -> FAssassins is locked by the level's own state at that point (the
gate's `DisSeqAct_Lock`, "used while locked"), as in the original; on a first visit some destinations start their own intro (Streets1 /
Streets2 dock arrival, FStreets, TowerRtrn Yard: a matinee holding the player or cinematic mode) and the player is moved by it after
arriving; the free-roam session starts on maps the campaign enters later, so these first-visit intros play.

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
| 1 | Kismet ops the port does not implement (22 classes with 70+ instances, list below) | 3995 instances in the 22 listed classes alone; per map 6-29 % of op instances unimplemented (OutsiderDream 29 %) | 42 | now listed per run as KismetNotes (see the per-map table); before: not visible in the smoke: conversation / possession / distraction / pickup events never fire, so levels will not react to the player. Unverified how many sequences stall |
| 2 | Placed actors not imported: DisNPCDistractor 1401, DisSplineLoftActor 664, PathTargetPoint 350, DisFish 249, SplineLoftActor 170, DishonoredNote 165, DisHideoutAccessPoint 140 | 3139 | 37 / 18 / 28 / 23 / 14 / 24 / 12 (see below) | NPC distraction props, splines (rails, wires), AI path targets, fish, hideout access are absent |
| 3 | Entry not routed to the map (story state). FIXED: right entry events / flags / chaos in the scenarios | 10 maps | Isl_High, LightH_Low, Out x3, Pub x5 | the level starts without its own travel start / chapter flags; smoke covers the level's own sequences only |
| 4 | FIXED (0 warnings). Movers with no collision (mesh or skeletal mesh missing): DisRiverKrust 63 actors, DishonoredUsableObject 24, DisWatchTower 5, DisAudioLogPlayer 2 | 94 actors | 20 of the 39 maps with a cold run (`DisRiverKrust` 8 maps, `UsableObject` 12, `WatchTower` 4, `AudioLogPlayer` 1) | those objects have no collision at all in the port (usables such as levers and doors' props; river krust and watch towers are not solid). Worst: Streetsewer 18, Flooded_FRefinery 16, Flooded_FStreets 14 |
| 5 | FIXED (DisSeqAct_BackupAndClearInventory). Flooded_FIntro entry stalls after PrepareMapChange | 1 | Flooded_FIntro | no travel start, level Kismet mostly idle (33 activations) |
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

`fixtures/oracle/capture_queue.txt` has one block per map: `session open map=M` (fresh map on the hooked game), a comment with the exact collision command,
then the `maps/<name>` smoke scenario (original Kismet trace). The queue runner has no collide line (the `collide` session op takes inline queries), so the hook agent runs per map:

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