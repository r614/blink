# How Dishonored builds missions

Read with `kismet_dump <package>` (objectives, tasks and the Kismet graph) and
`mission original <map>` (summary per package). Code: `crates/dishonored_assets/src/kismet.rs`.
All findings are from the GOG Definitive Edition, Boyle Ext packages (`L_Boyle_Ext_Script`,
`L_Boyle_Int_Script`, `L_BoyleStreet_Ext_Script`, `L_Boyle_Ext_P`).

## Layers

| Layer | Original classes | Role |
|---|---|---|
| Objectives | `DishonoredObjective` (`m_Description`, `m_ChaptersShownIn`, `m_Tasks[]`) | journal entries ("Assassinate Lady Boyle") |
| Tasks | `DishonoredTask_Custom` (`m_Description`, `m_InitialState` = `DTS_Inactive/Active/...`, `m_bInitiallyHidden`) | the steps under an objective. Boyle has 27, including lethal and non-lethal variants per sister, per-dress variants and optional side tasks |
| Task control | `DisSeqAct_SetTaskState`, `DisSeqAct_SetTaskHidden`, `DisSeqAct_UpdateTaskTarget`, `DisSeqAct_AddObjective`, `DisSeqAct_RemoveObjective`, `DisSeqAct_SetObjectiveHidden`, `DisSeqCond_TaskState`, `DisSeqVar_Task` (`m_pObjective` + `m_TaskName`) | Kismet nodes that move tasks between states and set the HUD target |
| Story flags | `SF_*` flag sets (GUID per flag); `DisSeqAct_SetStoryFlag` (Set True/False), `DisSeqCond_CheckStoryFlag`, `DisConv_SetStoryFlag/CheckStoryFlag` | world state shared by Kismet and conversations |
| Spawners | `DishonoredSpawner` / `DisTravelSpawner` (`m_pPawnTweaks`), `DisSeqVar_SpawnerPawn`, `DisSeqEvent_Spawned`, `DisSeqAct_StartSpawn` | NPCs placed by archetype tweak and addressed by spawner |
| AI direction | `DisSeqAct_AIGoToActor` (walk/run, outputs Success, Interrupted, Instant), `AISetPatrol`, `AIDoSearch`, `AIDoFollow`, `AINoise`, `AISetSuspicionLevel`, `AIShoot`, `AIRingAlarm`, `SetDisposition`, `AISetSenses` | scripted behaviour on top of the generic AI |
| Gameplay events | `SeqEvent_Touch` (Touched/UnTouched, with an `Originator` trigger), `SeqEvent_TakeDamage`, `DisSeqEvent_Distracted`, `PickupPickedUp`, `Used`, `Possessed`, `NPCIncapacitated`, `PatrolPointReached`, `BehaviorStarted`, `BreakableBroken` | what the level reacts to |
| Flow | `SeqEvent_RemoteEvent` / `SeqAct_ActivateRemoteEvent` (311 / 257 in Boyle), `SeqAct_Delay`, `SeqAct_Gate`, `SeqAct_Switch`, `SeqAct_Toggle`, `SeqCond_CompareBool`, `SeqVar_*`, nested `Sequence`s | wiring |
| Presentation | `DisSeqAct_ShowHUDMessage` ("New Mission Clue Added"), `DisSeqAct_ShowTargetNotification`, `DisSeqAct_ShowLocationDiscovery`, `SeqAct_AkPostEvent`, `SeqAct_CameraFade`, `SeqAct_Interp` (matinee) | feedback |
| Progress | `DisSeqAct_AutoSave`, `DisSeqAct_IncrementPlayerStat`, `DisSeqAct_SetPlayerTravelDestination` / `GotoPlayerTravelDestination` | saves, stats, level travel |

Boyle Ext's main script package has 976 Kismet nodes in 13 sequences. The mission logic is
spread over nested sequences named after beats ("Pistols_in_the_Yard"). Nodes reference level
actors by name (`TargetPoint_5`, `DishonoredSpawner_46`, `DisTrigger_32`). Example
(paraphrased): a touch on `DisTrigger_3` sets a story flag; a story-flag check gates a
`DisSeqAct_AIGoToActor` that sends a guard to `TargetPoint_55`; its *Success* output fires a
remote event that other sequences listen for.

## Mapping to the custom framework (`crates/dishonored_mission`)

| Original | Custom mission (RON) |
|---|---|
| `DishonoredObjective` / `DishonoredTask_Custom` | `ObjectiveDef` / `TaskDef` (`initial`, `hidden`, `optional`, `target`) |
| `SetTaskState`, `SetTaskHidden`, `UpdateTaskTarget` | `SetTask`, `ShowTask`/`HideTask`, `TaskDef.target` (the HUD marker follows the first active visible task) |
| `SF_*` story flags | `flags` + `SetFlag`/`ClearFlag`, `Flag`/`NotFlag` conditions, `FlagSet` trigger |
| `SeqEvent_Touch` on a trigger | `MarkerDef` with a radius + `Enter`/`Leave` |
| Level actors by name | `Place::Actor("TargetPoint_5")` or `"Package:Name"`; `mission markers <map>` lists them |
| `DishonoredSpawner` + `StartSpawn` | `NpcDef` (`spawn_at_start`) + `Spawn`/`Despawn` |
| `AIGoToActor`, `AISetPatrol`, `SetDisposition` | `MoveNpc`/`RunNpc`, `Patrol`, `Alert`/`Calm` |
| `SeqAct_Delay`, remote events, gates | `After(seconds, [...])`, `Fire`/`Remote`, `EnableRule`/`DisableRule`, `once` |
| `ShowHUDMessage`, `ShowTargetNotification`, `AkPostEvent` | `Message`, `Notify(title, text)`, `Sound(event)` |
| Mission stats screen | `MissionStats` (hostiles/civilians killed, detected, alarms, time, chaos, Ghost, Clean Hands) |

Deliberate differences:
- **One flat rule list instead of a node graph.** Rules cascade within a step the way Kismet
  does within a frame.
- **Named markers and NPCs instead of object variables.**
- **Server-authoritative.** The interpreter is deterministic and runs on dedicated servers,
  in replays and in the harness.
- **Chaos is approximated** by the kill ratio (`high_chaos_kill_ratio`). The original's exact
  formula is not reverse-engineered.
- **Conversations are data:** the original `DisConversation` graphs run as-is in
  `dishonored_conversation` (`Conversation(..)` action = `DisSeqAct_DialogInputs`,
  `ConversationOutput(..)` trigger = `DisSeqEvent_DialogOutputs`, story flags shared by name).
- **Not yet covered:** pickups and abstract items, possession, distraction events, location
  discovery, matinee cutscenes (soirees are recorded, not played).

## Importing original data

`mission original <map>` prints the original objectives and tasks for reference, and
`mission markers <map>` lists the original actors a mission can anchor to. The original
Kismet graph itself is not executed. It references engine systems (conversations,
matinees, pickups) that this port doesn't implement.
