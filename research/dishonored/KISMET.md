# Kismet: how Dishonored runs its level scripting, and how the port runs it

The original missions are UE3 Kismet graphs (plus Arkane's `DisSeq*` ops) cooked into the level
packages, with one always-loaded global game sequence in `DishonoredGameFull_P`. The port loads
them as data and runs them with an interpreter that follows the engine code of `Dishonored.exe`
(GOG build, SHA-256 `70da9613…c6fb`), decoded with Ghidra. Nothing from the game is copied
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
