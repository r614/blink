# Kismet: how Dishonored runs its level scripting, and how the port runs it

The original missions are UE3 Kismet graphs (plus Arkane's `DisSeq*` ops) cooked into the level
packages, with one always-loaded global game sequence in `DishonoredGameFull_P`. The port loads
them as data and runs them with an interpreter that follows the engine code of `Dishonored.exe`
(GOG build, SHA-256 `70da9613â€¦c6fb`), decoded with Ghidra. Nothing from the game is copied
here: addresses, field offsets and behaviour are described in my own words; decompiled output
stays in scratch folders.

Confidence tags follow `research/LOG.md` (**CONFIRMED** = read from data or code,
**LIKELY** = inferred, not verified in the game, **UNKNOWN**).

Code:
- `crates/dishonored_kismet`: `model` (typed database), `load` (reader, feature `load`),
  `runtime` (interpreter), `host` (the trait the game implements); CLI `kismet`
  (`stats`, `op`, `find`, `fwd`, `bwd`, `actors`, `obj`, `fields`, `run`).
- `crates/dishonored_multiplayer/src/kismet_host.rs`: the host on a mission match.
- `missions/boyle_original.ron`, `harness kismet`.
- Ghidra helpers in `research/ghidra/`: `ClassDecomp.java` (now compares vtables slot by
  slot), `DecompAt.java` (functions by address), `KismetSlots.java` (UpdateOp / Activated /
  DeActivated of each Kismet class), `FindFieldWrites.java` (who writes a bit of a field).

## 1. Data

### What is loaded (CONFIRMED)

- **Ops**: every export under `TheWorld` whose class derives from `SequenceObject` (class
  chains read from `Core`, `Engine`, `GameFramework`, `DishonoredGame`). Boyle Ext + Int + the
  global sequence: 18 packages, 8,580 Kismet objects of 170 classes (153 op classes besides
  variables, frames and matinee data), 972 referenced level actors, 4 objectives.
- **Class defaults**: cooked objects omit properties equal to their archetype, and a class
  default object stores only what differs from its parent class's. The loader merges the
  `Default__*` chain from the root class down, then the object's own properties. This matters
  for links: a gate whose four inputs equal the class defaults has no `InputLinks` in the level
  data at all, and `bAutoActivateOutputLinks` is a class default almost everywhere (true from
  `SequenceOp`; false for events, conditions, `SeqAct_Gate`, `SeqAct_CameraFade`,
  `DisSeqAct_DialogScriptedChoice`, ...).
- **Links**: `InputLinks` (name, `LinkedOp` for a sequence's `SeqEvent_SequenceActivated`,
  `bDisabled`, `ActivateDelay`), `OutputLinks` (targets as op + input index, `LinkedOp` for a
  sequence's `SeqAct_FinishSequence`), `VariableLinks` (linked variables, `PropertyName`,
  `bWriteable`, expected class), `EventLinks`.
- **Typed properties**: ints, floats, bools, names, strings, enums, object references resolved
  to ops / level actors / object paths, structs, arrays (struct arrays decoded generically,
  4-byte arrays as object references unless known to be ints like `Indices`).
- **Named variables**: `SeqVar_Named` is resolved like the engine does when a level starts
  (`SequenceOp` slot 89 rewrites the variable links that point at the named variable): the
  first variable whose `VarName` matches, searching the global sequence, then the map's levels
  in streaming order, depth first in `SequenceObjects` order. Boyle: 779/779 resolve.
- **Level actors** referenced by Kismet: location, rotation, scale, tag, scalar properties and,
  for triggers, the touch shape: a `CylinderComponent` (radius, half height; template values
  when the instance omits them) or the boxes of a brush volume's
  `BrushComponent.BrushAggGeom.ConvexElems[].ElemBox` (local space, `PrePivot` applied).
- **Matinees**: Dishonored's `InterpData` is a stub whose `m_Data` points at a `MatineeData`
  object (cooked into the level package); its `m_RunData` holds `InterpLength` and
  `InterpGroups`. Event keys come from `InterpTrackEvent.EventTrack` (time, name), and each
  key name is also an output of the `SeqAct_Interp`. Example: Samuel's boat intro is 29.81 s
  with keys at 5.26 s and 23.90 s.
- **Story flags**: `m_StoryFlag` GUIDs are named from `Startup.upk` `DisStoryFlagSet`s.
- **Objectives**: `DishonoredObjective` / `DishonoredTask_Custom` with their localized text
  (`Localization/INT/<Package>.int`, read at runtime). Kismet strings (`m_Message`,
  `m_TargetName`, ...) are replaced by their localized values when the level's `.int` has them.

### Dishonored variables (CONFIRMED from data and class layouts)

- `DisSeqVar_SpawnerPawn` (a `SeqVar_Object` whose value is a `DishonoredSpawner`): reads give
  the pawns the spawner spawned (`m_pSpawnedPawns`), nothing before it spawns.
- `DisSeqVar_PlayerPawn`: the player's pawn. `SeqVar_Player`: all players or `PlayerIdx`.
- `DisSeqVar_Task`: `m_pObjective` + `m_TaskName`.

## 2. Execution model (decoded)

Addresses are function entry points in the GOG build.

### Sequences and the op stack (CONFIRMED)

- `Sequence::UpdateOp` (vtable slot 92, 0x00701550): register pending events; if the sequence
  and its parents are enabled, run `ExecuteActiveOps` on its own op stack, then `UpdateOp` of
  every nested sequence in order. Streaming levels' root sequences are nested under the
  persistent level's root (here the global game sequence), so each tick runs the global
  sequence, its subsequences, then the map's levels in streaming order.
- `ExecuteActiveOps` (0x00700740):
  1. Delayed activations (`ActivateDelay`): count down (by time-dilated dt unless the op has
     `m_bAlwaysOutOfBendTime`), and when due give the input an impulse and queue the op.
  2. Until the stack is empty (at most 1000 ops per call): pop the top op; copy linked
     variable values into its properties (`PopulateLinkedVariableValues`, slot 91); if it is
     not active, set `bActive`, increment `ActivateCount` and call `Activated` (slot 93);
     if it is (still) active call `UpdateOp` (slot 92): finished ops call `DeActivated`
     (slot 94) and `PublishLinkedVariableValues` (slot 90); a still-active op with
     `bLatentExecution` is kept for the next tick and its outputs propagate now; any other
     still-active op neither propagates nor stays.
  3. Outputs with an impulse: for each link, if output + input `ActivateDelay` > 0 the
     target goes to the delayed list (an existing entry for the same op and input gets the new
     delay), otherwise (op, input) is collected. Inputs of the op are then cleared unless they
     have queued activations (non-latent ops only), in which case one is consumed and the op is
     queued again; output impulses are cleared. The collected targets get their impulses in
     reverse order and are pushed on top, so the first link of the first output runs next
     (depth first). A second impulse on an input that already has one counts a queued
     activation.
  4. When the stack is empty, one event activation that arrived while the event was still
     active is replayed.
- `QueueSequenceOp` (slot 101, 0x006e5050): push on top, or insert at the bottom, unless the op
  is already on the stack. Linked ops go on top; events triggered by gameplay go to the bottom.
- `Sequence::Activated` (slot 93): for the first input with an impulse, `CheckActivate` on its
  `SeqEvent_SequenceActivated`; then the sequence op clears its own `bActive`, so it never
  updates or deactivates: its outputs come only from `SeqAct_FinishSequence`.
- `SeqAct_FinishSequence::Activated` (0x006e5920): the enclosing sequence's output whose
  `LinkedOp` is this op fires its links into the stack of the sequence that contains that
  sequence (already updated this tick, so they run on the next tick).
- `SequenceOp` flag bits at +0x8c (from the field declaration order in `Engine.upk`, checked
  against the code): bit 2 `bActive`, bit 3 `bLatentExecution`, bit 4
  `bAutoActivateOutputLinks`, bit 10 `m_bAlwaysOutOfBendTime`. Link records: input 0x30
  bytes (impulse bit at +0xC, queued count +0x10, disabled +0x14, delay +0x24), output 0x38
  (targets array, impulse/disabled bits at +0x18, `LinkedOp` +0x1C, delay +0x20), variable 0x48.

### Events (CONFIRMED)

- `CheckActivate` (slot 99, 0x006cf3a0) passes when: the game is running, the event is not
  being destroyed, its sequence is enabled, there is an originator, `bPlayerOnly` events have a
  player-controlled instigator, `MaxTriggerCount` is 0 or not reached, and `ReTriggerDelay` is
  0, or it never fired, or more than the delay has passed since the last activation. A disabled
  event passes the check but does not activate.
- `ActivateEvent` (slot 102, 0x006f5dd0): store originator and instigator, record the
  activation time and count the trigger; then, if the event is not active: set it active, call
  `Activated`, write the `Instigator` variables, publish, set the output impulses (all of them,
  or the given indices) and queue it in its sequence. An event that is still active is queued
  in its sequence's replay list instead.
- `SeqEvent_Touch` (slots 99, 103-106): class filters (`ClassProximityTypes`,
  `IgnoredClassProximityTypes`), Arkane's possession filters, then `CheckActivate` as a test;
  a new toucher activates output 0 ("Touched") and joins the touched list. Leaving runs the
  check with `bPlayerOnly` and the retrigger delay ignored (the trigger count still applies),
  then activates "UnTouched", plus "Empty" when the list becomes empty.
- `SeqEvent_LevelLoaded` outputs (slot 81): "Loaded and Visible", "Beginning of Level",
  "Level Reset", "State Restored". Each output has its own engine call, and each call skips
  events whose output has no links (so an unlinked output counts no trigger):
  - "Loaded and Visible": `Sequence` slot 106 (0x00700120), recursively over nested
    sequences; also called by `NotifyMatchStarted` when its third flag is set.
  - "Beginning of Level": `NotifyMatchStarted` (slot 107, 0x007004b0, second flag; its first
    flag activates every output of the level startup events).
  - "State Restored": Dishonored's level-state restore (0x00a02760, recursive), called at the
    end of loading a level's saved state (0x00a13070 <- 0x00a13db0, from the map-load hook
    0x00a152f0 and the save-game load 0x00a162d0). It first sets the event's `TriggerCount`
    to 0, so a one-shot event fires again.
  - Whether "Loaded and Visible" / "Beginning of Level" also run for a restored level is not
    settled (U-080); with the default one-shot trigger count they would be blocked by the
    restored count anyway.

### Engine ops (CONFIRMED unless marked)

| Class | Behaviour |
|---|---|
| `SeqAct_Gate` (0x00705bc0) | Open sets open and the close counter to `AutoCloseCount + ActivateCount` (minus one when In arrives at the same time); Close closes; Toggle flips (and resets the counter when it opens). In passes while open and closes the gate once `ActivateCount` reaches the counter. Level data stores `CurrentCloseCount = AutoCloseCount`. |
| `SeqAct_Switch` (0x00706480) | Fires output `index - 1` for each `Indices` entry (disabling it with `bAutoDisableLinks`), then adds `IncrementAmount` (wrapping over the outputs with `bLooping`). The "Index" variable feeds `Indices` and is written back. |
| `SeqAct_RandomSwitch` (0x006dc630) | Reset re-enables the auto-disabled links. In picks uniformly among enabled outputs (`rand() % n`; when looping with auto-disable and nothing is left, everything is re-enabled first), disables the pick with `bAutoDisableLinks`, and writes it to `Indices`. |
| `SeqAct_Delay` (0x006cf6c0 / 0x006dba50) | Activated arms `Duration`; Update: Start (re)arms (with `bStartWillRestart`), Stop finishes with no output, Pause holds; it counts down from the tick after the one it started in, and fires "Finished" when it runs out. |
| `SeqAct_Toggle` (0x006f6370) | Bool variables on/off/flip; linked events (and their attached duplicates) enabled/disabled/flipped; actor targets toggled through their handlers. |
| `SeqAct_ActivateRemoteEvent` (0x00701ec0) | Every enabled `SeqEvent_RemoteEvent` of that name anywhere under the root sequence (all loaded levels), checked with the WorldInfo as originator and this op's `Instigator`. |
| `SeqAct_AttachToEvent` (0x006fb270) | For each attachee (pawn or controller) and linked event: unless the attachee is already the event's originator, duplicate the event (`<name>_AttachDup`) into this op's sequence with the attachee as originator. Every activation duplicates again. |
| `SeqCond_CompareBool` (0x006fa8f0) | AND of the "Bool" variables: output 0 when true, 1 when false. |
| `SeqCond_CompareInt` / `CompareFloat` | Outputs A<=B, A>B, A==B, A<B, A>=B, every one that holds. |
| `SeqCond_CompareObject` | A == B (controller and pawn count as the same) -> output 0, else 1. |
| `SeqAct_SetBool` | AND of "Value" variables (or `DefaultValue` when none) into "Target". |
| `SeqAct_SetInt` / `SetFloat` / `SetObject` | Target = sum of the Value array / Value (or `DefaultValue`), through the property links. |
| `SeqAct_AddInt` etc. | Result properties, output fired directly. |
| `SeqAct_Interp` (Activated 0x00638f70, Update 0x00639330, DeActivated 0x0062f780) | Play/Reverse/Change Dir start it and fire "Started" (a matinee that cannot initialize fires output 2); Update handles Pause/Play/Reverse/Stop/Change Dir, finishes once not playing, otherwise steps (event keys fire their outputs as playback crosses them). DeActivated: Completed (0) at the end, Reversed (1) at the start, Interrupted (2) stopped part way, Stopped (3) stopped part way by the Stop input. Outputs in the data: Completed, Reversed, Interrupted, Stopped, Started, OnSkipWhole, then event names. |
| `SeqAct_PrepareMapChange` | Requests the level load in Activated; its Update returns finished at once; then `SeqAct_Latent`'s DeActivated ("Finished"). |
| `SeqAct_CommitMapChange` | Commits the prepared map change. |
| `SeqAct_Latent` (Activated 0x006eb060, Update 0x006e5250, DeActivated 0x006da810) | Handlers on the targets register latent actors; finished when none remain; "Finished" or, when aborted, "Aborted". |
| `SeqAct_CameraFade` (Activated 0x006e6960, Update 0x006e6de0) | Activated sets the time left to `FadeTime`, sends the fade (colour, alpha, time) to every target controller (all player controllers when there is no target), remembers them and fires "Out". Update subtracts the step's time (the first update runs in the activation step); at 0 or less it clears the fade on the remembered controllers unless `bPersistFade` with a non-zero target alpha, fires "Finished" and is done. |
| `SeqAct_ActorFactory` (Activated 0x006dae60, Update 0x006daed0, slot 101 0x006f90f0) | One spawn per update while the delay is out: the next spawn point in order (shuffled with `rand() % (n - i) + i` swaps when `PointSelection` is random, reversed when it is 2); each spawn is written to "Spawned" and fires output 0; then `SpawnDelay` is armed; done when `SpawnCount` actors exist. DeActivated only clears a flag (no outputs). |

### `DeActivated` per class (vtable slot 94, CONFIRMED with `KismetSlots.java`)

`DeActivated` decides the automatic outputs, so it matters as much as `Activated`:

- the engine default (`SequenceOp`, 0x006d9680: every non-disabled output when
  `bAutoActivateOutputLinks`): most actions, all conditions and events;
- an empty override (no automatic outputs): `SeqAct_Switch`, `SeqAct_RandomSwitch`,
  `SeqAct_Delay`, `SeqAct_StreamInTextures`, `DisSeqAct_StartSpawn`, `DisSeqAct_AIDoFollow`,
  `DisSeqAct_AIDoSimpleBehaviors`, `DisSeqAct_NPCMarkForVanish`. (A switch's
  `bAutoActivateOutputLinks` is true by class default, so without this every switch would fire
  all its links.)
- `SeqAct_Latent`'s (Finished / Aborted): `SeqAct_Latent`, `DisSeqAct_Latent`,
  `SeqAct_PrepareMapChange` and `DisSeqAct_AutoSave` (from their own), `DisSeqAct_ActivateUsable`,
  `DisSeqAct_BackupAndClearInventory`, `SeqAct_WaitForLevelsVisible`;
- own: `SeqAct_Interp`, `DisSeqAct_DialogScriptedChoice` (output 0 "Player Busy" only when
  aborted), `DisSeqAct_AIGoToActor` / `AIShoot`, `SeqAct_ActorFactory`.

The earlier `ClassDecomp.java` missed the empty overrides: it compared a class's vtable with
the set of its parent's functions, and the shared empty function sits in other parent slots.

### Dishonored ops (CONFIRMED from code unless marked)

- `DisSeqCond_CheckStoryFlag`: output 0 when set (LOG 2026-10-05).
- `DisSeqAct_SetStoryFlag`: inputs Set True / Set False / Toggle.
- `DisSeqCond_TaskState` (0x00ba78c0): fires the output of the task's current state
  (`EDisTaskState` order Inactive, Active, Completed, Failed).
- `DisSeqAct_DialogInputs` (0x00b9bf00): each input with an impulse is sent by name to the
  one-shot's `DDH_KISMET_ACTIVATED` hook (conversation start), then the action's handlers.
- `DisSeqAct_DialogScriptedChoice` (Activated 0x00b8b250, Update 0x00b90bc0): opens the choice
  UI (failing that, marks itself failed); Update waits for a pick and fires output pick + 1, or
  output 0 when the player is busy or the UI failed.
- `DisSeqAct_AIGoToActor` (Update 0x00b90530; Activated is empty): Start (re)starts the move
  and fires "Out"; Abort finishes; otherwise `SeqAct_Latent` update. Outputs Out, Success,
  Failed, Interrupted (enum order). DeActivated (0x00ba1ba0) only clears the latent list, so
  Abort fires nothing. The results come from each pawn's `DisBehaviorGoTo` through the pawn
  (0x00b4abb0: the pawn's current Kismet action at +0x398, then the action's slot 101,
  0x00ba1eb0, which removes the pawn from the latent list and fires the result):
  - Failed (2): the behaviour's path request fails (0x00ae8600);
  - Success (1): the behaviour reached `DGTS_GoToCompleted` (arrived, then turned to the
    destination with `m_bAlignRotation`; 0x00aedc00 sets finished + success);
  - Interrupted (3): the behaviour ended without success for any other reason (0x00ae4d60).
  Failed and Interrupted fire per pawn; Success fires only when it is the last pawn's report.
- `DisSeqAct_NPCMarkForVanish` (Update 0x00b90f90, empty Activated / DeActivated): Mark and
  Cancel go to the targets' handler (`OnNPCMarkForVanish`, 0x00b4acb0: Mark stores the action
  and `m_fOffscreenTimeRequired` on the pawn at +0xc78/+0xc7c; Cancel clears it and removes
  the pawn from the latent list without an output) and Mark fires "Out". The pawn's tick
  (0x00b64480) counts the time down while the pawn is not seen and resets it while it is;
  once it is below 0 and the vanish test passes (0x00b5f7d0 with `m_bRespectSearchAndCombat`
  and `m_fRequiredDistance`), the pawn is destroyed (0.01 s life span) and reports
  "Vanished" (0x00ba2190 with 1). The action finishes when no pawn is left.
- `DisSeqAct_Door::Activated` (0x00b91d70): only the first input with an impulse counts
  (Open CW, Open CCW, Close, Lock, Unlock); for each `DisDoor` target: open with the direction
  (0x00a1a450), close (0x00a1a4b0), lock / unlock (door virtuals +0x3b8 / +0x3bc); then
  `SequenceAction::Activated` (handlers) and the automatic outputs.
- `DisSeqAct_ActivateUsable` (Activated 0x00b9aae0, Update 0x00b9ac90, `SeqAct_Latent`'s
  DeActivated): goal stage = `m_TargetStageIndex`, or the next stage (wrapping) when it is -1;
  an index out of range sets the abort flag; a usable already at the goal is skipped unless
  `m_bForceAdvance`; usable targets are used toward the goal by `m_pUsingPawn` (else the
  player); Update finishes once the usable's stage is the goal.
- `DisSeqEvent_Used` (0x00a57640, a usable's transition end): every enabled `DisSeqEvent_Used`
  of the usable is checked with the usable as originator, its using pawn (`m_pInstigator`) as
  instigator and the reached stage as the only output index (`m_Stage` set to it): the stage
  outputs come first, the special outputs (used while locked, hold-to-use, missing item)
  after them.
- `DisConv_KismetOutput` -> `DisSeqEvent_DialogOutputs` (0x00cb1080): events whose originator
  is the one-shot (or whose tree is the conversation's) and whose output has the node's name;
  the instigator is the pawn of the node's speaker index (else the WorldInfo).
- `DisSeqAct_StartSpawn` (Activated 0x00b9bcc0, Update 0x00b90050): counts the spawners,
  requests the spawns, fires "Out"; each update fires "Spawned" per spawned pawn and "Failed"
  per failure, and "Finished" once nothing is pending.
- `DisSeqAct_AutoSave` (Activated 0x00b8f240, Update 0x00b8f2e0): output 1 ("Accepted") when
  saving is not possible or the save was written, 2 ("Denied") when the pending autosave was
  dropped, then "Finished" (0) from the latent DeActivated. The engine side (pending
  autosave, mission-start saves, combat rule) is in `SAVES.md`; the port runs it in
  `mission::save`.
- `DisSeqAct_SetDisposition` (0x00b90a50): the input index (0..2) overrides `m_Disposition`,
  then the handlers.
- `DisSeqAct_GotoPlayerTravelDestination`, `SetPlayerTravelDestination`, `SaveLevelState`:
  LOG 2026-10-05.
- Many Dis actions keep `SequenceAction::Activated` (0x006eac90): it calls the targets' script
  handlers (`OnXxx`) - the behaviour is in the pawn/actor classes, not in the op.

### Random numbers (CONFIRMED, U-082)

- Every Kismet draw is the C runtime `rand()` imported from the CRT DLL: `SeqAct_RandomSwitch`
  (0x006dc630: `rand() % valid`), `SeqVar_RandomFloat` (slot 96, 0x006ee230:
  `Min + (Max - Min) * rand() * (1/32767)`, the float constant at 0x00fb9318),
  `SeqAct_ActorFactory` (random spawn-point order). The MSVC generator is a per-thread linear
  congruential one: `holdrand = holdrand * 214013 + 2531011`, result bits 16..30.
- Seeding: `FEngineLoop::PreInit` (0x005fe910) calls `srand` with the low half of
  `QueryPerformanceCounter`, or `srand(0)` with the `-FIXEDSEED` switch (also when a global
  at 0x014129c4 is set); `appInit` (0x0048bf10) seeds with `time()` (or 0) earlier, and a
  third site (0x005762c0) reseeds from the performance counter before a random pick unless
  `-FIXEDSEED`.
- The stream is shared: `rand` has 258 call sites in 181 functions (AI decisions, particles,
  accessories, barks, sounds...), all on the game thread. Even with `-FIXEDSEED` the value a
  Kismet switch draws depends on every call made before it since start-up (frame timing,
  particle counts, menus), so the original's outcome for a given run cannot be reproduced.
  The port runs the same generator (`runtime::CrtRand`) as a stream of its own seeded with
  the match seed: the distribution, the `% n` bias and the float formula are the original's.

## 3. The port

### Interpreter (`dishonored_kismet::runtime`)

Per-sequence stacks, delayed activations and event replay queues as above; ops pushed/popped
in the engine's order; `ActivateCount`, input queues, latent ops, populate/publish of property
links, `CheckActivate` / `ActivateEvent`, touch lists, remote events over the loaded levels,
attached duplicates, the engine ops of the table, and the `DeActivated` table. Deterministic:
`SeqAct_RandomSwitch` / `SeqVar_RandomFloat` use the CRT generator seeded per match (the original uses
the C runtime `rand()`). Levels load and unload with their maps; a level's Kismet state is kept
while the player is elsewhere, and a return visit fires "State Restored" instead of "Loaded and
Visible" + "Beginning of Level" (LIKELY, U-080). Unsupported classes are logged once and
counted; they finish with their automatic outputs.

### Host (`kismet_host.rs`)

| Original | Port |
|---|---|
| Story flags | the mission's flags by `SF_*` name, shared with the conversation runner |
| `DishonoredObjective` / tasks | mission objectives built from the original data (hidden until `AddObjective`), `SetTaskState` / `SetTaskHidden` / `UpdateTaskTarget` (HUD marker) / `DisSeqCond_TaskState` / `ShowHUDMessage` / `ShowLocationDiscovery` / `ShowTargetNotification` |
| Spawners | begin-play pawns and `StartSpawn` through the match's original spawners; `DisSeqEvent_Spawned`; `DisSeqVar_SpawnerPawn` |
| AI | `AIGoToActor` (walk to the destination, Success / Failed / Interrupted from the brain), `AISetPatrol` (back to routine) |
| Looks | `NPCSetMaterials` Set / Clear on the pawn's look |
| World | `Teleport` (pawns and level actors), `Destroy` |
| Travel | `SetPlayerTravelDestination`, `PrepareMapChange` + `CommitMapChange` -> the match's level travel (level state kept), `GotoPlayerTravelDestination`; a map outside the mission ends it |
| Gameplay events | `DisSeqEvent_Used` and `DisSeqEvent_PickupPickedUp` actors as usables (localized use prompts); touch shapes vs pawns each tick (`SeqEvent_Touch`); deaths -> `SeqEvent_Death` / `DisSeqEvent_NPCIncapacitated` (on the pawn and its spawner) |
| Conversations | `DialogInputs` -> the conversation runner (`Package:OneShot/<hook>`); its Kismet outputs -> `DisSeqEvent_DialogOutputs` of that one-shot; a soiree plays its matinee by GUID; original NPCs with a use hook are talkable |
| `DialogScriptedChoice` | the shared choice view (options localized), frozen input while choosing |
| `SeqAct_Interp` | not played: its timing, event keys and outputs run; start and end are recorded |
| `AkPostEvent` | mission sound |
| Presentation | camera fades (timed), cinematic mode, HUD toggles, tutorials, chapters, statistics, achievements, materials on actors, ambient sounds: no-ops, counted apart |

### Boyle (`harness kismet`)

The original mission runs from its first event to its last:

1. `ChangeLvl_fromHub_toBoyle` in the global sequence: PrepareMapChange `L_Boyle_Ext_P`,
   travel destination `PlayerStart_SamuelDropOff`, CommitMapChange.
2. Samuel's spawner -> `DialogInputs` "Boyle_Start" -> `Samuel_Intro` -> its soiree plays the
   29.8 s boat matinee -> "Add Party Objective" -> the objectives appear with their text
   ("Assassinate Lady Boyle": "Find a way inside the Boyle Estate").
3. The estate entrance trigger completes T12 and activates T1.
4. The garden door: `DisSeqEvent_Used` -> the original choice -> travel destination ->
   remote events -> `ChangeLvl_fromBoyleExterior_toBoyleInterior` -> global PrepareMapChange /
   SaveLevelState / CommitMapChange -> the ballroom.
5. The sisters spawn; the counter switch opens the "3 Pronged Gate"; the random switches place
   them, dress them (NPCSetMaterials) and pick the guilty one; her note is teleported to her
   bedroom.
6. Picking up that note shows her dress task (T13..T21), completes T1 and T22.
7. Killing her with the sword fires the death event attached by `Create Death Events`: all
   elimination tasks complete, T6 "Escape back to Samuel's boat".
8. The ballroom door back to the garden (state restored), the canal trigger (T10), Samuel's
   `Lets_Go` choice, the final exit: PN tasks resolve, camera fade,
   `BoyleCanal_PlayerIsReadyTogo` -> `ChangeLvl_fromBoyle_toHub` -> global PrepareMapChange
   `L_Pub_Morning_P` -> CommitMapChange: the mission ends.


The agent is placed next to what it uses (it does not path across the maps). It ends the
mission at 58.7 s of play (seed 1; the matinee and conversation timings are the original's).
Coverage on that run: 78 op classes executed of the 153 op classes in the data, 893
activations of 764 distinct ops, 53 presentation-only activations, no unsupported class. The
same seed gives the same Kismet state hash; over seeds 1-6 all three sisters come out guilty.
The full activation tables are in the harness report.

### The other routes (`harness kismet`, `--route brisby|white|duel`)

Each runs on a fresh match; every stage is a check.

- **Nonlethal (Lord Brisby), end to end.** Brisby's approach hook starts
  `Lord_Brisby_Quest_Start`; `Join_Brisby_in_Private` -> AIGoToActor [8331] (Success) -> his
  route and window matinee, whose Started output arms the window touch; the player stepping in
  sets `Boyle_BrisbyInPosition`. His use hook plays `Brisby_Boyle_Invite`, then the guilty
  sister's KO conversation (its soiree starts `GetBrisbyQuest`: `Boyle_BrisbyTalked`, her KO
  task). She is choked unconscious (`DisSeqEvent_KnockedOut` retargets the task to the cellar
  boat) and carried down; the player-only touches by the boat put Brisby there ("BRISBY
  TELEPORTED" -> "reveal love nest"); the body test (`DisTrigger_23`, sisters' tweaks only),
  her body on the boat and the player on the boat play `Thanks`; its soiree closes the gate ->
  `CompleteMercyPath` (BoyleMercyPathComplete, SF_Global_PlyrMercyBoyle, chaos -20, T6). The
  way out as on the lethal route ends the mission at 136.9 s.
- **Miss White's drink.** `Hits_On_Player` -> "Quest For the Drink" (T25); the drinks usable
  (`DisWaterSource_7`) runs `DisSeqAct_GivePickup` of the party drink; her use hook's Kismet
  output `HasDrinkLogicCheck` -> `Takes_Drink`, `RemoveAbstractItem`, T25 completed;
  `MissWhiteRevealsColors` plays Waverly's then Lydia's colour; `End W Conversation` marks the
  three colours known and sends her away (AIGoToActor to TargetPoint_100). Which sister is
  guilty still needs a suspect (`Boyle_Suspect*`: her note or Brisby's reveal), as in the
  original. Her walk does not arrive headless (U-220).
- **Lord Shaw's duel (part of `L_Boyle_Ext_Script`, "Pistols in the Yard").** His radial
  approach hook plays `Begin_Duel_Conv` (Boyle_PlayerFoundShaw); standing in front of him
  (DisTrigger_3) and using him plays `Player_Initiates_Choice` / `Player_Initiates` with
  Pendleton's note (`SoireeShawReadsNote`): PN T0 completed, PN T2 "Follow Lord Shaw". He walks
  to the duel ground, the elite offers the weapons (`Choose_Weapon` -> `PlayerTakesGun` ->
  "Player Pistol Setup" moves and enables the box's use trigger), and the player takes the
  pistol (`Duel_Explained`). The go-tos `AIGoToActor_0` / `_1` / `_2` send `Shaw_Elite`
  (`DishonoredSpawner_11`) to TargetPoint_5, `Lord Shaw` (`_45`) to TargetPoint_9 inside his
  mark trigger TriggerVolume_2, and `Shaw_Elite_Pistols` (`_46`) to TargetPoint_55. When the
  elite arrives, `enableshawtrig` arms Shaw's touch (`SeqEvent_Touch_39`, "Shaw On Mark"). With
  the player on TriggerVolume_3 the loop plays `Go_to_Mark` and the countdown `Three` / `Two` /
  `One`; "turn off mark trigger" completes PN T2.
  - **The shot (2026-10-06).** With the player looking away (`SeqEvent_LOS_2` on
    `Trigger_LOS_2` sets "Player Looking Away"), `CompareBool_21` starts `One` and, in the same
    step, "Shaw Shoots": `DisSeqAct_AIShoot_0` makes Lord Shaw shoot at the player from `One`
    on (`m_iMaxNumShots` 0 = forever, accuracy override 100 %): he turns early. `One`'s output
    runs `SetIgnoreDeath_0` and "Go Time" (Duel_Started, "Licence To Kill", "OK To Fire", PN T2
    completed). "Go Time" also links to the `Turn_and_Fire` input of `DisSeqAct_DialogInputs_6`,
    but that input link is **disabled** (`bDisabled`): `Turn_and_Fire` never plays in the
    original.
  - **Outcome.** Shaw's `DisSeqEvent_NPCIncapacitated_0` (culprit: the player) with "Player
    Failed Duel" false and `DeliveredNoteToShaw` set: "Player Won Duel", PN T1 shown and active,
    `PistolDuelWon`, story flags `PistolWinHonor` and `boyle_pistol_win`, the seconds become
    allies of Corvo and go to check the body. With "Player Failed Duel" set: `PistolWinMess`.
  - **View tracking.** Touching the mark ("Mark Touched") toggles cinematic mode and
    `DisSeqAct_PlayerTrackTarget_0` ("Start Tracking") turns the camera toward `Trigger_LOS_2`
    over 3 s with 20 degrees of freedom; `_11` ("Stop Tracking") on "UnfreezeForDuel". Decoded
    from the camera code (`kismet_host::ViewTrack`, 0x00b8f9c0 / 0x00aaf4c0 /
    0x00ab4e60 / 0x00aa0f10 / 0x00aaf3a0 / 0x00aa0e60): without `m_bHoldUntilStopped` the
    channel ends by itself once blended in.
  - Port run (`kismet --route duel`, 8/8): the countdown at 93.4 / 96.4 / 98.4 s, Shaw shoots
    from 98.4 s, the player kills him at 101.4 s, `PistolWinHonor` and PN T1 at 102.4 s (the
    player survives with 2 health). Open: the pistol second (2008) is killed at 101.1 s by the
    other second (2004) (U-480).
  - **Publishing array properties.** An op's array property (`Targets`) is written back to its
    variables element by element (`runtime::publish_values`, UE3 `PublishValue`). Several duel
    ops link `Targets` to all three of Shaw's variables. Until 2026-10-06 the whole array went
    into each variable, so all three held Lord Shaw: he walked to the elite's mark and never
    touched his own.
  - **Conversation refusals.** `Go_to_Mark` (High) is refused while `Stop_stand_there` (High)
    plays. That is the decoded rule: only a strictly higher priority interrupts
    (CONVERSATIONS.md section 2). The duel loop asks again on its next pass, and it plays one
    second later.

## 4. Open points

See `research/UNKNOWNS.md` U-080..U-087 and U-220..U-224.

Update 2026-10-06 (AI): `DisSeqAct_SetDisposition` (faction targets through
`Crowd::set_disposition`, pawn targets through `Crowd::set_pawn_disposition`; inputs Set /
Clear / ClearAll per `EDisSetDispositionKismetInput`, decoded 0x00c79000) and
`DisSeqAct_AISetSenses` (brain `deaf` / `blind` / `numb`) are now handled by the host
(`[ai]` hooks in `kismet_host.rs`); see `dishonored/AI.md` "Factions".

### Host additions (2026-10-06)

- Doors: `DisSeqAct_Door` keeps each door's state and lock; a lock change rebuilds the AI's
  navigation with the door's path-object edges blocked or opened (`KismetRun::door_nav`,
  `NavOverlay`); state changes fire `DisSeqEvent_Door` Opened / Closed (at once: the door's
  swing time is not modelled, U-222).
- Usables have stages: a use moves to the next stage and fires the `DisSeqEvent_Used` output
  of that stage (0x00a57640); `DisSeqAct_ActivateUsable` moves to its goal (no transition
  time, U-223). The engine's `SeqEvent_Used` (triggers) is a usable too.
- `SeqAct_AttachToActor`: attached level actors follow their base (touch shapes move; a pawn
  does not touch what is attached to it). `SeqAct_ActorFactory`: decoded spawning of the
  factory's movable ambient sounds (presentation).
- `SeqEvent_Touch` toucher filter (0x006da210): `ClassProximityTypes`, `ProximityTweaks`
  (the sisters' boat test), ignored classes. `SeqEvent_LOS`: looking at the originator within
  `TriggerDistance` and `ScreenCenterDistance` (U-224). `SeqEvent_TakeDamage`: accumulated
  hits from `SimEvent::Damage` with the attacker as instigator.
- Abstract items: `AddAbstractItem` / `RemoveAbstractItem` (`m_pItemToAdd` /
  `m_pItemToRemove`, quantities), abstract pickups and `DisSeqAct_GivePickup` (its tweak
  resolved at load, `KismetSetup::gives`) go to `MissionState::items`, which the
  conversations' `DisConv_HasAbstractItem` reads.
- AI: `AIDoSearch`, `AISetSuspicionLevel`, `AIClearAttention`, `AIPsychicAttention`,
  `AIRingAlarm`, `AIShoot` (latent, shots counted from `SimEvent::Fired`; drives the brain's
  Shoot behaviour, AI.md "Scripted shoot"), `PlayerTrackTarget` (forced view), `AIDoFollow`,
  `AIDoSimpleBehaviors`, `AIStartDistraction`, `AINoise` (radius from
  `m_LoudnessDistanceArray`), `SetIgnoreDeath`, `ForbiddenZoneOverride`, `AlarmBell`.
- Other: `GameOver` (mission failure after `m_fDelay`), `AttachPickup`, `ModifyHealth`, chaos
  (`AddDarkness`). `kismet_host::NOT_MODELLED` lists the 14 decoded classes whose systems the
  port lacks; they are counted apart from unsupported classes.
- A hook output outside a conversation (a use hook's Kismet output) finds its one-shot by name
  in the loaded levels.

### Op coverage pass (2026-10-08)

Decoded from `Dishonored.exe` (`oracle index fn / uscript / vcall`), checked against the
oracle's coverage report (`oracle coverage`). Interpreter (`runtime.rs`):

- `SeqAct_AndGate` 0x006dcd60 (Activated), 0x006dcc40 (slot 96, `OnReceivedImpulse`), 0x006ec1b0
  (slot 88, rebuilds `LinkedOutputFiredStatus`): `LinkedOutputs` is every output link of a
  sibling in the gate's sequence that leads to the gate; an impulse from an op marks all of
  that op's outputs leading here (while `bOpen`); once all are marked Activated arms output 0
  and clears `bOpen` for good. Delayed links deliver without a source, so the gate misses them.
- `SeqAct_CastToInt` 0x00706b20 (`bTruncate`, else `(int)ROUND(v * 2 + 0.5) >> 1`),
  `SeqAct_CastToFloat` 0x00706e10, `SeqCond_Increment` 0x00704f20 / `IncrementFloat` 0x00705080
  (`ValueA += IncrementAmount`, outputs `<=`, `>`, `==`, `<`, `>=` against `ValueB`; the
  interpreter used to hand `SeqCond_Increment` to the host, which never ran it).
- Object lists (`SeqVar_ObjectList.ObjList`): `SeqAct_ModifyObjectList` 0x00704300 / 0x00701ba0 /
  0x006f8460 (Add = `AddUniqueItem`, Remove = every occurrence, Empty; then `ListEntriesCount`),
  `SeqAct_AccessObjectList` 0x006f8840 (random = `ROUND(appFrand() * (n - 1))`, first, last,
  `ObjectIndex`), `SeqAct_IsInObjectList` 0x006f8b90 / 0x006f8d50: the decoded loop keeps only
  the result of the LAST test object (the inner loop resets it), `runtime::in_object_list`.
  ModifyObjectList / AccessObjectList `DeActivated` (0x006dab90) arms every output whatever
  `bAutoActivateOutputLinks` says (`DeAct::AllOutputs`).
- `SeqVar_External` (a variable inside a sequence standing for the sequence's own link
  `VariableLabel`): ops read what the sequence is linked to (the engine's Externals have no
  value of their own). `SeqVar_Vector` (`VectValue`) reads and writes.

Host (`kismet_host.rs`): `SeqAct_GetDistance` 0x006fb9f0, `SeqCond_IsAlive` 0x006fad70,
`SeqAct_SetLocation` 0x006fb0b0, `SeqAct_GetLocationAndRotation` 0x006fbf70 (rotation as Euler
degrees roll / pitch / yaw), `SeqAct_SetVelocity` (script `Actor.OnSetVelocity`),
`DisSeqCond_IsPlayerInCombat` 0x00b8efc0 (reads `DishonoredPlayerPawn.m_bIsInCombat`, 0x00aa2350;
its writer is not decoded: the port answers with `MissionMatch::engaged`),
`DisSeqCond_IsDLCUnlocked` 0x00b8ee30 (`KismetSetup::dlc_unlocked`), `DisSeqAct_AddPower` /
`RemovePower` (0x00aa38a0 / 0x00aa3920 -> `SetPowerLevel` 0x00ace270: the level index, -1 removes),
`DisSeqAct_ModifyAmmo` 0x00b51cc0 (0x00c052e0 add, 0x00c05380 set, 0x00c05260 remove),
`DisSeqAct_AddKey` / `RemoveKey` (0x00aa1530 / 0x00aa1560, key ring), `AdrenalineToggle` 0x00aaa100,
`ToggleChoke` 0x00ab1bd0 (`m_InputEnableMask[7]` bit 0x20000), `AISetBrainFlags` 0x00b90690,
`SetAudioOcclusion` 0x00ba1780 (on `DishonoredAudioPortal` actors). The keys, adrenaline, choke,
brain flags, occlusion and actor velocities are recorded in `HostState` only: nothing in the
port reads them yet.

Event firing sites (the engine fires them through 0x00bc7b30 `FireGeneratedEvents(actor, class,
originator, instigator, exact, outputs)`; found by resolving each caller's class getter to its
class descriptor name). Not fed by the port yet (the host feeds the classes in `FED_EVENTS`):

| event class | firing functions |
|---|---|
| `DisSeqEvent_Possessed` | 0x00a52130, 0x00a5a990 (possessable pawn states; output 0 Started, 1 Finished), 0x00b7d110, 0x00b7d2f0 (DLC06) |
| `DisSeqEvent_PatrolPointReached` | 0x00af2300, 0x00afd350 |
| `DisSeqEvent_BehaviorStarted` / `Combat` | 0x00af7670 / 0x00af7d00 |
| `DisSeqEvent_MovablePickedUp` / `MovableDropped` | 0x00a45fd0 / 0x00a46380 |
| `DisSeqEvent_Corpse` (0 picked up, 1 dropped, 2 discovered) | 0x00a998a0, 0x00a90b50, 0x00b40740 |
| `DisSeqEvent_PlayerCrouch` (0 stand, 1 sneak, 2 crouch; `m_OldCrouchState`) | 0x00ab0960 |
| `DisSeqEvent_PlayerHolsterWeapon` | 0x00ab3d80 |
| `DisSeqEvent_PlayerManaThreshold` | 0x00ab1a30 |
| `DisSeqEvent_Windblasted` | 0x00c4de90, 0x00c53790 |
| `DisSeqEvent_KeyHoleUsed` / `Lock` | 0x00a2e4e0 / 0x00a4e870, 0x00a4e910 |
| `DisSeqEvent_Assassinated` | 0x00a78270, 0x00a7beb0 |
| `DisSeqEvt_Choke` | 0x00a6d380, 0x00a74720, 0x00a7c270, 0x00a7c4d0 |
| `DisSeqEvent_Investigate` | 0x00ae9fe0, 0x00b2e050 |
| `DisSeqEvent_TeleportSpell` | 0x00c2ba40, 0x00c35280, 0x00cd7090, 0x00ce0aa0, 0x00cf12f0 |
| `DisSeqEvent_WatchTower` / `DefenceTower` / `WallOfLight` | 0x00a2a770 / 0x00a258f0, 0x00a2fce0..0x00a2fff0 / 0x00a28f80, 0x00a29170, 0x00a3c010, 0x00a3eb10, 0x00a3eca0 |
| `DisSeqEvent_Interact` / `AttackedByRats` / `RatPossess` | 0x00a3df60, 0x00a3e0b0 / 0x00a70450 / 0x00c7fbc0 |

### Player and world events (2026-10-08)

The firing function of each class was read (`oracle index fn`); every engine firing goes through
`FireGeneratedEvents(actor, class, originator, instigator, exact, outputs)` 0x00BC7B30: each
enabled event of `actor.GeneratedEvents` of that class runs `CheckActivate(originator, instigator,
.., outputs)`. `DAT_0145F628` is the local player pawn: the `DisSeqEvent_PlayerEvent` family
(registration 0x00B8A140) gives an event without an `Originator` the player, so the host finds
those events as "class, no originator" (`player_events.rs`, `player_events()`). The host code is
`crates/dishonored_multiplayer/src/kismet_host/player_events.rs`; the sim emits the new
`SimEvent::PowerUsed` / `Windblasted` and the mission runtime a `TaskNote` log.

| event | engine firing (output) | port hook | now firing |
|---|---|---|---|
| `DisSeqEvent_Possessed` (154) | NPC pawn possess start 0x00B7D110 (0), end 0x00B7D2F0 (1): originator the pawn, instigator the player, exact; the `DisDLC06NPCPawn` label of the index is the first vtable with the slot, the functions are `DishonoredNPCPawn`'s own (Tallboy 0x00B7EB00 calls the first). 0x00A52130 / 0x00A5A990 are the exit / enter of the river-krust-style creature state machine (state 7), no such pawn in the port | `SimEvent::PossessStart` / `PossessEnd` of a non-rat host, events found through pawn and spawner | all 154 (NPC hosts) |
| `DisSeqEvent_PlayerCrouch` (4) | every tick of the player pawn (0x00ABB1E0 -> 0x00AB0960): 2 if `bIsCrouched`, else 1 if sneak is toggled (0x00AC8280), else 0; fires `[state]` when it differs from `m_OldCrouchState` (+0xB6C) | `PlayerState::crouched` each step | outputs 0 and 2; output 1 (toggled, not yet crouched) never: the port applies the toggle to the crouch at once |
| `DisSeqEvent_PlayerCombat` (15) | every tick (0x00ABBDE0 -> 0x00AB9FA0): `m_bIsInCombat` (+0x8A0 bit 3) becomes the check "a hostile on the player's list (0x00AB6960: relation 2, attention >= 4 by 0x00B0B780) has awareness `InCombat` (0x00B0C1E0 == 5)"; false->true fires output 0, true->false output 1. Also read by `DisSeqCond_IsPlayerInCombat`, which now reads this flag instead of the autosave's `engaged` set | `in_combat_now`: a brain with `awareness() == IN_COMBAT` and attention >= 4 on the player | all 15. Not modelled: the registry lookup 0x00C6CE00 and the DLC check 0x00CF7B40 of the same function |
| `DisSeqEvent_TaskStateChanged` (79) / `TaskActivated` (2) | objective manager dispatcher 0x00AD4F60: kind 5 0x00AD1320 (output = the new `EDisTaskState`), kind 3 0x00AD1080 (all outputs); raised by 0x00AC17B0 (`SetState`: Active -> kind 3 then 5, other change -> 5), 0x00AC63C0 (hidden flag cleared on an Active task -> kind 3) and the task's registration 0x00AC6300 (both, at load). The event names the task: 0x00AD0F80 compares task and objective names. Originator the player | `MissionState::task_log` (`TaskNote`) filled by `set_task` / `ShowTask`, drained each step | all 81. Not raised: the registration pair at load, and the second `TaskActivated` of a `SetTaskState` + `SetTaskHidden(false)` pair (the port reveals a task when it activates) |
| `DisSeqEvt_Choke` (6) | the `StateNPCMasterBeingChoked` state: enter 0x00A7C270 (0), the win 0x00A6D380 (1, choke stage 2), the loss 0x00A74720 (2, stage 4), the exit 0x00A7C4D0 (2 unless a result was given, or 1 when the next state is the knocked-out one); originator the NPC, instigator the player | `SimEvent::ChokeStart` / `ChokeEnd` (Win 1, Lose and Cancel 2) | all 6 |
| `DisSeqEvent_Windblasted` (9) | the NPC windblast reaction: 0x00C53790 fires output 0 "Pushed" for an NPC that resists (0x00B4B4C0), else 0x00C4DE90 starts the reaction and fires 0/1 by whether the resulting state is the thrown one (class `DAT_0145F184`); instigator the caster | `SimEvent::Windblasted` where the throw starts (`step_windblasts`): output 1 | all 9 as "Thrown"; "Pushed" needs the armour state |
| `DisSeqEvent_PowerUsed` (21) | the power component's slot 94 (0x00BF32F0 -> 0x00BF30C0): the player's events of exactly that class, the first output named for the power plus `m_CurrentLevel - 1` ("Blink 2"); called by the activation of Windblast (0x00BFB950), Blink (0x00BFC990), Dark Vision (0x00BE89E0), Devouring Swarm (0x00BFAE60) | `SimEvent::PowerUsed` at the port's matching moments (`left_hand`, `blink`, `powers`) | 4 powers. Bend Time (0x00BFA180) and Possess (0x00BE7730) activations were traced and do not call slot 94: left open. `PowerEquipped` (a subclass, 0x00C44FE0 / 0x00C4BA70, from the equip contexts) is not fed |
| `DisSeqEvent_Lock` (25) | `Lock` 0x00A4E870 (0, always) and `Unlock` 0x00A4E910 (1, only if locked) of `DishonoredUsableObject` (doors via 0x00A32490 / 0x00A324B0); originator the usable, instigator its last user (+0x2C8) | `DisSeqAct_Lock` queues the output; fired next step | all 25 (the Kismet's own locks) |
| `DisSeqEvent_Climb` (1) | the climb state: enter 0x00A72B00 (0), exit 0x00A6E2C0 (1), originator the chain (pawn +0xAA0), instigator the pawn | `SimEvent::Climb`, chain by name in the active map | 1 |

Decoded but not hooked (no system in the port to hook them to; counts are level instances):
`MovablePickedUp` (127) 0x00A45FD0 (output none = all; originator the grabbing player, instigator the
movable; from the grab state 0x00C3D030) and `MovableDropped` (1) 0x00A46380 (0x00C45340, the carried-object
release): the port cannot grab physics props. `KeyHoleUsed` (37) 0x00A38580 (output 0, the key used)
and 0x00A2E4E0 (output 1): no keyholes and the key ring is recorded only. `Interact` (4) 0x00A3DF60
(0) / `AltInteract` 0x00A3E0B0 (1): fired from the Use handler 0x00AD5BA0 for focused interactables that
are not pickups, event on the user with the interactable as instigator, filtered by
`m_pAssociatedTweak` (slot 102 0x00B8E830); the tutorial notes of the Prison. `Tripwire` (7) 0x00A21A30
(from 0x00A28B40 `Trip(instigator)`, once; originator the tripwire, instigator the tripper): no
tripwires. `SeqEvent_RigidBodyCollision` (73) 0x0078F7D0, `SeqEvent_ParticleEvent` (26) 0x0089FC00,
`SeqEvent_AnimNotify` (2) 0x0070F4F0, `SeqEvent_HitWall` (1, `Actor.HitWall` script): no rigid bodies,
particles or per-actor notifies. `SeqEvent_Destroyed` (18) is `UWorld::DestroyActor` 0x00652CF0, which
runs `CheckActivate(actor, actor, 0, 0, 0)` on the actor's Destroyed events (the instigator is the
destroyed actor itself, so a `bPlayerOnly` one cannot pass). `DisSeqEvent_PlayerManaThreshold` (1)
0x00AB1A30 (output 1 when the mana threshold level falls, 0 when it rises; the vslot-456 caller is
not decoded), `PlayerHolsterWeapon` (1) 0x00AB3D80 (no holster in the port),
`PlayerInventoryChanged` (1) 0x00BA1A30 (from 0x00C17F20 / 0x00C18C80 / 0x00C19380, the item class
test), `JournalViewed` (1) 0x00BB9960 / 0x00BD11E0, `ObjectiveCompleted` (3) 0x00AD0E00 (the
port has no objective completion state: `CompleteObjective` is unsupported), `TeleportSpell` (1)
0x00C2BA40 and siblings (DLC witches), `WallOfLight` (1), `RatPossess` (1) 0x00C7FBC0, `FishPossess`
0x00A04820 / 0x00A06710 (output 0 / 1).

Map smoke (`oracle scenario --system kismet`, 42 maps, 600 ticks, no input): the activation counts changed
on 3 maps only, as the player does nothing and nobody possesses, chokes or blasts: Galvani2 +2
(`DisSeqAct_Lock` of `DishonoredUsableObject_19` fires its `DisSeqEvent_Lock`), LightH_HighChaos +4 and
LightH_LowChaos +4 (`Lighthouse_Obj.DishonoredTask_Custom_12` -> Active fires two
`DisSeqEvent_TaskStateChanged`). The fed classes of the table no longer appear among the "never
activated" notes of the load, and `Distracted`, `BreakableBroken`, `WhaleOilBattery` and
`WhaleOilReceptacle` (fed all along) were added to `FED_EVENTS` so they stop being reported as unsupported.
