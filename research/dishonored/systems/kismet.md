# Kismet and mission scripting: the core rules, checked against the original

Scope: the Kismet interpreter (`crates/dishonored_kismet/src/runtime.rs`) and its host
(`crates/dishonored_multiplayer/src/kismet_host*`). This is the 2026-10-08 verification pass. It
re-reads the engine side of the original (GOG build, SHA-256 `70da9613...c6fb`) with the index
(`oracle index fn / class / xref / vcall`) and `exe_probe dis`, and compares each rule with the port.
`research/dishonored/KISMET.md` stays the description of the data and of the individual ops; this
file is the rule book and the list of divergences.

Confidence tags: **CONFIRMED** = read from the decompiled or disassembled code in this pass;
**LIKELY** = inferred from code plus data; **UNVERIFIED** = needs an original capture (listed in
section 6). Every address is a function entry (or an instruction where it says so).

Method notes that matter for the next agent:
- The decompiler drops stack arguments of virtual calls. Where an argument decides the result
  (`UpdateOp(dt)`, `QueueSequenceOp(op, bPushTop)`), the call was checked in the disassembly
  (`exe_probe dis <va> <count>`; `exe_probe func` can start in the wrong function).
- `oracle index xref --json` lists **every** caller; the text form stops at 12 or 40. Virtual
  call sites are not in the JSON.
- Native code finds the events of an actor by walking `Actor.GeneratedEvents` (`+0x230`) and
  testing each event against a cached class pointer (`DAT_...`, a lazy getter with the pattern
  `mov eax,[DAT]; test; jne; push <ClassInfo>; call 0x00479370`). Finding the references to that
  global (`exe_probe find-imm 0x<DAT>`) finds every native fire site of an event class (appendix A).

---

## 1. The model, as rules

### K-1. Where Kismet runs and what the trace sees (CONFIRMED / one part UNVERIFIED)

- Once per world tick the engine calls `Sequence::UpdateOp(dt)` (vtable slot 92, `0x00701550`)
  on the game sequence. For each sequence it: registers pending events (`UnregisteredEvents`,
  slot 101 of the event, `0x006e50b0`); if the sequence's `bEnabled` (`+0x12c` bit 0) and all
  its parents are enabled (`0x006cf320`), calls `ExecuteActiveOps(dt, 0)` (`0x00700740`) on its
  own stack; then calls `UpdateOp(dt)` of each entry of `NestedSequences` (`+0xfc`), in array order.
  A disabled sequence skips itself and everything below it. Streaming levels' roots hang under
  the persistent level's root, so one tick runs the global sequence, its sub-sequences, then
  each level in streaming order (KISMET.md section 2).
- The hook (`ProcessEvent` on `SequenceOp`) sees the script events `Activated` (raised right after
  the native `Activated`, slot 93) and `Deactivated` (right after `DeActivated`, slot 94) of every
  op including events (`ActivateEvent` raises `Activated` too, `0x006f5dd0`). A Kismet trace is
  therefore the sequence `(tick, op, Activated | Deactivated)`, in exact order.
- Frame convention (ORACLE.md "Trace alignment"): the Kismet of world tick k is recorded with tick
  k; it runs inside the world tick after the boundary-k input, so a remote event given at
  boundary k is processed in tick k + 1 on the original and in the port's step that produces
  frame k + 1 (`kismet_host::step` calls `k.remote_event` then `k.tick` in the same step): same.
- UNVERIFIED: where in the world tick `UpdateOp` sits relative to the actors' ticks (pawn physics,
  touch/overlap callbacks, AI). The port runs gameplay events (`feed`) and then `k.tick` after
  `MissionState::step`. If the original ticks Kismet before the actors, every gameplay-triggered
  event lands one tick later than in the port. Check: F-13 / section 6.

### K-2. Time base (CONFIRMED, new)

`ExecuteActiveOps` does not use `dt` as given.

```
ratio   = GameInfo.vtable[244](0)      // DishonoredGameInfo 0x009e9e90
        = m_fCurrentWorldTimeDilation (+0x3a8) / m_fCurrentPlayerTimeDilation (+0x3ac)
dt_scaled = dt * ratio                 // 1.0 when there is no game info
```

- Delayed activations count down by `dt` when the op's `m_bAlwaysOutOfBendTime` (op flags `+0x8c`
  bit 10) is set, else by `dt_scaled` (`0x00700740`, first loop).
- `UpdateOp` of each op is called with the same choice (disassembly `0x00700a67..0x00700a90`:
  `test eax,400h; movss xmm0,[ebp+8]` (raw) or `[ebp-1Ch]` (scaled)).
- The world tick's own `dt` is already scaled by `WorldInfo.TimeDilation`, which Bend Time sets to
  the **player** dilation (HOOK_TRIAGE: 1 -> 0.85, level 2: 0.9). So an unflagged op effectively
  runs at the **world** dilation (0.15 at level 1, 0.0 at level 2: all unflagged Kismet stops
  while Bend Time level 2 is active); flagged ops run at the player's rate. LIKELY for the first
  step (that the `dt` passed in is `TimeDilation`-scaled); the formula itself is CONFIRMED.
- Which ops are flagged is data: `m_bAlwaysOutOfBendTime` is a per-op property (a `SequenceOp`
  field). In Boyle it is set on `SeqAct_CameraFade` (35), `DisSeqCond_CheckStoryFlag` (31),
  `SeqEvent_Touch` (20), `DisSeqAct_SetTaskState` (17), `CompareBool`, `Gate`, `TakeDamage`,
  `ActivateRemoteEvent`, `SetBool`, `SeqAct_Interp` (4)... i.e. the gameplay plumbing and the
  fades; delays and matinees are mostly **not** flagged.
- Two clocks for events: `ActivateEvent` stores `ActivationTime` and `CheckActivate` compares
  `ReTriggerDelay` against `BendTimeSeconds` (`WorldInfo +0x398`, advances at the world
  dilation) for unflagged events and `TimeSeconds` (`+0x38c`) for flagged ones (`0x006f5dd0`,
  `0x006c8350`, `0x00781200/20`). `SeqAct_Delay` stamps `TimeSeconds` when armed and counts down
  only when `TimeSeconds` differs from the stamp, i.e. from the next tick (`0x006cf6c0`,
  `0x006dba50`), by the `dt` it is given (so under K-2, in world time).

Port: `Kismet::tick(host, dt)` passes the fixed 1/60 to everything; one clock `time += dt`.
No flag is read. **F-3, F-4.**

### K-3. The impulse model (CONFIRMED)

An op has input links (record 0x30 bytes: impulse bit `+0xc`, `QueuedActivations` `+0x10`,
`bDisabled` `+0x14`, `ActivateDelay` `+0x24`) and output links (0x38 bytes: targets, impulse bit
`+0x18`, `bDisabled`, `LinkedOp` `+0x1c`, `ActivateDelay` `+0x20`).

- Giving an input an impulse (`ExecuteActiveOps`, targets loop, and the delayed loop): ignored
  when the input is `bDisabled`; if the input already has an impulse, `QueuedActivations++`;
  set the impulse bit; copy `PlayerIndex`/`GamepadID` from the activator (slot 96, `0x006cf2c0`);
  `QueueSequenceOp` the op.
- An op is run when popped (K-4). Per pop, in this order:
  1. `PopulateLinkedVariableValues` (slot 91) every time;
  2. if `bActive` is false: set it, `ActivateCount++`, `Activated` (slot 93) + script `Activated`;
  3. `UpdateOp` (slot 92) -> returns non-zero when finished. `bActive = !finished`.
  4. Finished: `DeActivated` (slot 94) + script `Deactivated` (outputs are armed here), then
     `PublishLinkedVariableValues` (slot 90).
  5. Output propagation happens only for ops that finished **or** are still active and
     `bLatentExecution` (op flags bit 3). A still-active non-latent op neither propagates nor
     stays on the stack; its next impulse pops it straight into `UpdateOp` (no second
     `Activated`).
  6. Inputs are cleared (impulse bit and queued count) unless the input has queued activations
     and the op is **not** latent: then one queued activation is consumed and the op is queued
     again (K-4, bottom). Latent ops always clear.
  7. All output impulse bits are cleared.
  8. Slot 95 (`0x17c`, no-op in the base class) is called for a finished op.
- Output propagation: for each output with an impulse, each link `(op, input)`: if
  `output.ActivateDelay + input.ActivateDelay > 0` the pair goes to the sequence's delayed list
  (`DelayedActivatedOps`, +0x120; an existing entry for the same op+input gets the new delay,
  `0x006d9ec0`), else it is collected. Collected pairs get their impulses in **reverse**
  collection order and are pushed on top: the first link of the first output ends on top and
  runs next (depth first).

Port: `run_op` / `impulse` / `fire` follow all of this. **Verified equal** except the queue
positions of F-1 and F-2.

### K-4. The stack: who goes where (CONFIRMED, two divergences found)

`QueueSequenceOp(op, bPushTop)` (slot 101, `0x006e5050`): skipped when the op is already on the
stack; `bPushTop` true appends (top, popped next), false inserts at index 0 (bottom, popped last).
Pops take the last element. The disassembly of `ExecuteActiveOps` fixes each caller's argument:

| Who queues | position | evidence | port |
|---|---|---|---|
| a target that got an impulse from an output (collected, in reverse) | top | `0x00701112 push 1` | top: same |
| a delayed activation that came due (start of `ExecuteActiveOps`) | **bottom** | `0x00700942..0x0070094a`, `edi` = 0 | **top**: F-1 |
| a non-latent op with `QueuedActivations` left (one consumed) | **bottom** | `0x00700f0d push 0` | **top**: F-1 |
| a latent op that stayed active, after the pass (see K-5) | top, reverse order | `0x00701452..0x007014f1` | top, encounter order: F-2 |
| an event activated by gameplay (`CheckActivate`, default `bPushTop` 0) | bottom | `0x006f5dd0` | bottom: same |
| an event activated by `ActivateRemoteEvent` | bottom (`push 0`) | `0x00702031..` | bottom: same |
| an event activated while already active | not queued: stored in `Sequence.QueuedActivations` (+0x130) | `0x006f5dd0` | same |

- Per call: at most 999 pops (the loop breaks when `iterations + 1 > 999`; replays count). The port
  breaks at the 1000th pop and re-queues it: same.
- Replay: when the stack is empty and `QueuedActivations` is non-empty, the first entry is
  activated again (`event.slot 102`, with the stored originator, instigator, indices, bPushTop),
  which refills the stack; the loop goes on. Port: same (`queued_events`).
- There is **no event priority**. `SequenceEvent.Priority` (`+0x108`, byte) is read by none of
  `CheckActivate`, `ActivateEvent`, `UpdateOp`, `ExecuteActiveOps` (checked); it is editor
  metadata. Simultaneous events run in the order the stack gives. Gameplay events of one tick
  are each inserted at index 0, so the earliest ends nearest the top of the bottom group and is popped
  first (FIFO among them), but everything already on the stack (kept latent ops, ops queued by
  an earlier event of the pass) is above them and runs first.

### K-5. Latent ops (CONFIRMED)

A latent op (`bLatentExecution`) that is still active after `UpdateOp` is kept: it is added to a
local list in pop order; after the pass is over (stack empty or cap) the list is emptied from the
**end**, and every op in it that is still `bActive` is queued on the **top**. So the op that was
encountered **first** in the pass is on top next tick and updates first (F-2). Latent ops propagate
the outputs that already carry an impulse every tick they are popped (so `Started`-style outputs
set in `Activated` reach their targets on the activation tick) and clear all their inputs every
pop, so an impulse on a running latent op (Delay `Start`/`Stop`/`Pause`, Interp `Play`/`Stop`) is
seen once by `UpdateOp` and then gone.

`SeqAct_Delay` (verified again): `Stop` (input 1) finishes with no output, `Pause` (input 2)
clears the running flag, `Start` (input 0) arms (and re-arms when `bStartWillRestart`); while
running and `TimeSeconds != stamp`, `remaining -= dt`; `<= 0` fires "Finished" (output 0) and
finishes.

### K-6. Variables (CONFIRMED; the port is a subset)

- `Populate` (slot 91, `0x006ef6c0`): for each variable link that has a `PropertyName` and at
  least one variable and is not `bSequenceNeverReadsOnlyWritesToThisVar`: take the **first
  non-null** linked variable and call its `PopulateValue(op, property, link)` (slot 93); the loop
  stops after that call when the global `DAT_0141b21c` is 0 (LIKELY the shipping value; the
  alternative is that every linked variable populates in turn, last wins). The variable class decides
  the semantics over the **whole link**: `SeqVar_Int` (`0x006f6da0`) sets a scalar int property to the
  **sum** of all linked int variables' `IntValue`; an int array property gets one element per
  variable. Objects append all (empty ones skipped).
- `Publish` (slot 90, `0x006d9710`): for the first non-null variable call `PublishValue` (slot 94)
  then `break`; `SeqVar_Int::PublishValue` (`0x006f6f90`) writes the scalar to **every** int variable
  of the link and, for array properties, element i into variable i (stops at the shorter).
  Then slot 95 of every variable is called (no-op for the stock variables).
- Evaluation order inside one op: populate on every pop, before `Activated` and `UpdateOp`;
  publish only when the op finishes (and for events, at activation, after `Activated`). A
  variable written by an op is visible to the next op that is popped, not before.
- Events write their `Instigator` (and other writeable links) at activation: `ActivateEvent` runs
  `Activated`, slot 97 (`0x006fef20`: the "Player Index" and "Gamepad Id" variables get
  `PlayerIndex` / the pad of the player), then publish.

Port: `populate` takes `vars[0]` for scalars, all for arrays (`Indices`, `Targets`, and the
`SetInt`/`SetFloat` `Value` special case); `publish` writes a scalar to every variable
(`publish_values`). Close enough for the data in Boyle; the sum rule matters wherever two ints/floats
are linked to one input (rare: `SeqCond_CompareInt` `A` once in the five maps scanned). F-5.

### K-7. Events: when an activation is accepted (CONFIRMED)

`CheckActivate(originator, instigator, bTest, indices, bPushTop)` (slot 99, `0x006cf3a0`) passes
when all of these hold:
1. net mode: not a client (`WorldInfo.NetMode != 3`), or for `bClientSideOnly` events not a
   dedicated server (`!= 1`);
2. the world has begun play (`WorldInfo.bBegunPlay`, `0x00784580`) and the event is not being
   destroyed (slot 13);
3. its sequence and all parents are enabled;
4. an originator was given;
5. `bPlayerOnly` -> the instigator is player controlled (slot 87 of the instigator);
6. `MaxTriggerCount == 0` or `TriggerCount < MaxTriggerCount`;
7. `ReTriggerDelay == 0` or `TriggerCount == 0` or `ReTriggerDelay < now - ActivationTime`, with
   `now` on the clock of K-2 (BendTimeSeconds, or TimeSeconds for a flagged event).

If it passes and `!bTest` and the event's own `bEnabled` is set, `ActivateEvent` (slot 102) runs. A
disabled event still returns true. `ActivateEvent` stores originator and instigator, and **only
when called without an explicit activation time** increments `TriggerCount` and stamps
`ActivationTime`; then, if the event is not active (or has no parent): `bActive`, `Activated`,
slot 97, publish, set the output impulses (all, or the given indices unless the output is
`bDisabled`), `QueueSequenceOp(parent, event, bPushTop)`. If it is active it is queued for replay.

Subclass hooks: `DisSeqEvent_Attention` adds a level gate (section 3.3); `DisSeqEvent_BehaviorStarted`,
`WitnessedInteraction`, `Used` etc. test their own state before calling the base; the base
function is the only place that touches counts and delays.

Port: `check_activate` / `activate_event` match every point except the net-mode/begun-play gates
(always true in the port) and the two clocks (F-4).

### K-8. Native event sources (CONFIRMED mechanism)

Gameplay code does not search the sequences. Each actor keeps its events in
`Actor.GeneratedEvents` (`+0x230`); code walks the array, tests the class, sets the event's own
fields, and calls `CheckActivate(owner, instigator, 0, &indices, 0)`. There are two shapes:
the inline loop (`0x00b79b80`, `0x00a57640`, `0x00b363d0`) and a thin helper
`0x00bc7b30(actor, classPtr, originator, instigator, flag, indices)` with 87 call sites. Appendix A
lists the fire sites of every event class. Consequence for the port: the *originator* of a native
event is the actor that owns it in the level (`Originator` property), not a global; spawned pawns get
the events of their spawner (`SeqAct_AttachToEvent` duplicates, and Dishonored's spawner copy).

### K-9. Randomness, `SeqAct_Gate`, `Switch`, `Interp`, ...

As KISMET.md sections 2 and "Random numbers"; no change.

---

## 2. What the port does, rule by rule

| Rule | Port (file : function) | Status |
|---|---|---|
| K-1 nesting, enabled chain | `runtime.rs : update_seq` | match |
| K-1 UpdateOp per tick, remote event on the next tick | `kismet_host.rs : step` | match (UNVERIFIED order vs actors, F-13) |
| K-2 dt scaling, flag | `runtime.rs : tick_dilated`, `execute_active_ops`; `kismet_host.rs : step` | match (F-3 ported 2026-10-08) |
| K-2 event clocks | `runtime.rs : check_activate`, `activate_event` (`event_clock`: `time` / `bend_time`) | match (F-4 ported) |
| K-3 impulses, queued counts, disabled, delays | `runtime.rs : impulse`, `run_op`, `execute_active_ops` | match |
| K-3 PlayerIndex / GamepadID | none | missing, harmless in single player (F-5) |
| K-4 target push order | `run_op` (`targets.into_iter().rev()`) | match |
| K-4 delayed-due / repeat queue position | `execute_active_ops`, `run_op` | match (F-1 ported) |
| K-4 cap, replay | `execute_active_ops` | match |
| K-5 latent keep | `execute_active_ops` (`latent_keep`) | match (F-2 ported) |
| K-6 populate / publish | `populate`, `populate_scalar`, `publish`, `publish_values` | match for Int / Float / Bool scalars (F-5 ported); PlayerIndex / GamepadID not kept |
| K-7 CheckActivate / ActivateEvent | `check_activate`, `activate_event` | match except gates and clocks |
| K-8 native event sources | `kismet_host.rs : feed` and friends | partial: section 5 |
| Trace | `runtime.rs : activations`, `trace_name`; `tools/oracle/src/runner.rs` | comparable (F-6 ported): per-op `Activated` / `Deactivated` |

---

## 3. The top unhandled ops

Counts are objects over all 470 packages (`harness_out/oracle/coverage/classes.tsv`). Class
layouts from `oracle index class`; fire sites from appendix A.

### 3.1 `DisSeqEvent_Distracted` (1350 objects; 162 in Boyle Ext/Int + 3 other maps; 58 in Boyle Int alone)

What it is. The glue between an ambient NPC's *distraction* and the sequence that dresses it. A
`DisNPCDistractor` actor (placed 1364 times; component `DisNPCDistractionComponent`) offers a point
where nearby NPCs stop for a short animation (`eDistractionType`: servant dusting, lord at the
fire place, guard arms crossed / looking around / peeing / piano / railing / scratch belly / warming
hands / looking under a table / looking out of a window / scavenging). The event is owned by the
distractor (`Originator` = the distractor actor) and has three outputs: 0 "Distracted Start", 1
"Distracted End", 2 "Distracted Looked At start"; its `Instigator` variable ("DistractedPawn") is the
NPC. In the data 145 of 162 follow one template: Start -> `SeqAct_Interp` input 0 (Play the
matinee that animates the prop / second actor) + `SeqAct_FinishSequence`; End -> `SeqAct_Interp`
input 2 (Stop). The other seven chain to `DisSeqAct_AIStartDistraction`, `Delay`, `ActivateUsable`,
`SetBool`, `CompareBool`, `ActivateRemoteEvent`: those are the ones that affect mission logic.

Fire sites (CONFIRMED): `FireDistracted(component, pawn, output)` = `0x00b79b80`:
1. `component.m_fCooldownTimer (+0x2a0) = m_fCooldownTime (+0x204)` (side effect, always);
2. for every enabled event in the owner's `GeneratedEvents` that is a `DisSeqEvent_Distracted`:
   `CheckActivate(owner, pawn, 0, [output], 0)`.
Callers: output 1 from `0x00b844f0` (the NPC's anim state reports the end), `0x00b84810` (the
distraction is cleared: abort, state exit `StateNPCMasterDistracted` slot 74 `0x00a61940`); output 0
from `0x00b84670` (`m_AnimDistractionState = 1` WeaponSwitching) and `0x00b84750` (state 2
Animating, after the soiree variant `0x00b79cf0`); output 2 from `0x00b7da20`, called by the AI
sub-process `DisAISubProcessDistractions` (`0x00b28550`) when it takes a look-at target. Output 0
therefore fires at the first moment the NPC enters the distraction proper; outputs 1 and 2 at its end
(or interruption) and at the glance. The exact split between the two output-0 call sites is
UNVERIFIED (a trace shows it).

Selection (the AI part; CONFIRMED structure, constants from the class defaults): the component
ticks (`0x00b84e90`, slot 79): `m_fCooldownTimer -= dt`, then `0x00b842f0`: a distraction is *ready*
when `m_bDistractionEnabled` (+0x1cc bit 0), cooldown <= 0 and neither `m_pLinkedGoToPawn` nor
`m_pLinkedAnimPawn` is set. With `m_fDistractionPulseTime < 0` (default -1) the brains in
`m_BrainsToCheckOnTick` (up to `DISTRACTION_MAX_BRAINS` = 5) are evaluated every tick; with a pulse
time >= 0 the list `m_NearbyBrains` is evaluated on the tick the pulse timer expires (the timer is
reloaded from `m_fDistractionPulseTime`) and `m_BrainsToCheckOnTick` on the other ticks. Each
candidate goes to `0x00b83bb0` or `0x00b80f30`, chosen by `0x00b4b3c0(brain)` (LIKELY the
LOS/range-checked path, `m_bRequireLOS`, `m_fDistractionLOSCheckPeriod` = 1.0 s, against the
unconditional one; UNVERIFIED).
Defaults: range `m_fDistractionRange` 1000, cooldown 45 s, `m_bLowerAIAlertness` true, categories
8 names, `m_DistractionTime` a distribution, `m_SuspicionLvl1_Chances` / `m_SuspicionLvl234_Chances`
(chance to look / to animate) and `m_DistractablePawnTweaks` / `m_NonDistractablePawnTweaks` /
`m_AllowedRoutes` filters. States: `EDisAnimDistractionState` GoingTo -> WeaponSwitching ->
Animating (`0x00b84c20` leaves WeaponSwitching for Animating once the weapon change is done).
The matinee/soiree: `m_bUseSoireeInsteadAnim`, `m_pDistractionMatineeData`,
`m_lDistractionSoireeActor/Loops`, else animations `Distraction_<type>_In/Loop/Out`.

Port: nothing (no `DisNPCDistractor`, no AI sub-process). The event is "unsupported", so the linked
`Interp` never plays. For Boyle Int that is 58 events (the soiree guests).

Needs: (a) load `DisNPCDistractor` + component data; (b) the AI sub-process (selection, GoTo,
weapon switch, animation, cooldown) from the addresses above; (c) fire through
`events_for("DisSeqEvent_Distracted", [distractor])` + `check_activate(..., Some(vec![out]), false)`
with the instigator the NPC; (d) do the side effect (cooldown reset) in the component, not in the
event. Priority: low for mission logic (only the seven non-template uses), medium for Boyle Int
fidelity. Do (c) last; without (b) it never fires.

### 3.2 `DisSeqAct_Lock` (319): done by the world porter; checked

`Activated` (`0x00b8f730`): the first input with an impulse (0 = Lock, 1 = Unlock; none leaves it
unchanged); for each `Targets` entry that is a `DishonoredUsableObject` (class cached in
`DAT_0145ef10`): Lock -> virtual `+0x3b8` (slot 238), Unlock -> `+0x3bc` (slot 239); then
`SequenceAction::Activated` (handlers) and the automatic output. `kismet_host.rs` `"DisSeqAct_Lock"`
(line ~1606) does exactly this for doors (`doors`, nav rebuild) and other usables (`usable_locked`),
and `DisSeqEvent_Used` fires its "used while locked" output. Match. (The matching
`DisSeqEvent_Lock`, 25 objects, fires from `0x00a4e870` / `0x00a4e910` and from `0x00a42240`;
port: not triggered, appendix A.)

### 3.3 `DisSeqEvent_AttentionIncreasedTo` (284) and `AttentionDecreasedTo` (65)

What it is. Fired when an NPC's attention level toward a target changes. `EDisAttentionLevel`:
Unaware 0, HeadTrack 1, TurnToFace 2, Investigate 3, Busted 4 (AI.md); the event stores the level
it wants in `m_eAttentionLevel` (+0x110) and has one output "Out" and an "AttentionTarget"
variable (the `Instigator`). Typical use: `MaxTriggerCount` 1, `bPlayerOnly`, one of HeadTrack /
TurnToFace / Investigate / Busted, Out -> a gate, a matinee input, `SetBool`, `ActivateRemoteEvent`
(Boyle Int: Busted -> remote event).

Fire site (CONFIRMED): `0x00b363d0(attention component, target, oldLevel, newLevel)`, called from
`0x00b3b350` for the records the observer's attention tick computes (`0x00b45910` -> `0x00b60c10`):
if `old != new`, for each enabled event in the **observer pawn's** `GeneratedEvents` (originator =
the observer, instigator = the target):
- `old < new`: the event cast to `DisSeqEvent_AttentionIncreasedTo` (`0x00b2cca0`; the DLC06 class
  `0x00b291f0` is fired with `ActivateIndices = [new]`);
- `old > new`: the same with `AttentionDecreasedTo` (`0x00b2ccf0`, DLC06 `0x00b29240`);
- the event's `m_eActivatedAttentionLevel` (+0x111) is set to `new`, then `CheckActivate`. The
  override `DisSeqEvent_Attention::CheckActivate` (`0x00b8a2a0`) only calls the base when
  `m_eActivatedAttentionLevel == m_eAttentionLevel` **and**, if `bPlayerOnly`, the instigator is of the
  player pawn class (`DAT_0145f620`, `0x00aa36c0`). So it fires once per change *to* the configured
  level, not for every level passed through unless the level steps one at a time.
Then, if the target is the player (`DAT_0145f628`): `new == 4` posts a message (id 0x2f); falling from
>= 4 to below 4 posts 0x3c (not Kismet).

Port: not triggered. The port's brain has the data (per-target attention accumulators, level
thresholds: `npc.<n>.player_attn.level` is recorded on both sides, AI.md). Needs: at the point where the
brain's level toward a target changes, call `k.trigger("DisSeqEvent_AttentionIncreasedTo" or
"...DecreasedTo", &[observer pawn, its spawner], instigator = target)` with the level gate above
(the event class must carry `m_eAttentionLevel`; read from the op's property) and the
`bPlayerOnly` instigator test. UNVERIFIED: whether the level in `0x00b3b350`'s records steps one level
per tick or jumps (read the records' construction `0x00b60c10`).

### 3.4 `DisSeqEvent_BreakableBroken` (233)

What it is. A `DishonoredBreakable` (barrels, crates, windows, doors with `DisTweaks_StaticBreakable`
steps) reports its damage steps. Outputs are named "Broken 100%", "Broken 66%", "Broken 33%" (one
to three, by the number of steps), one `Instigator` variable that stays empty. Data: 52 events in the
six maps scanned, `MaxTriggerCount` 0, no `bPlayerOnly`; Broken 100% -> `ActivateRemoteEvent` (20),
`CompareBool` (9), `Toggle` (8), `DisSeqAct_AINoise` (6), `SetBool` (5), `SetStoryFlag`...

Fire sites (CONFIRMED): the break routine `0x00a34460` (and the sibling `0x00a3a5f0`) calls
`0x00bc7b30(breakable, class, breakable, 0, 0, [index])` with
`index = tweak.m_Steps.Num() - stepReached - 1` (disassembly `0x00a345b4..0x00a3462e`, vtable `+0x140` of
the tweak interface = the step count). So the final step is output 0 ("100%") and an intermediate
step fires the output with the remaining steps (LIKELY mapping to the names; formula CONFIRMED).
Originator = the breakable itself, instigator none.

Port: not triggered; breakables are not modelled as stepped (the port has `DishonoredBreakable` data
for loot/props only: `DisTweaks_StaticBreakable.m_Steps`, `m_Health` +0xb0, `m_DamageThreshold`
+0xb4). Needs: health/step state on breakable actors (`m_CurHealth`, `m_NextStep`), damage from
the port's `SimEvent::Damage`/explosions/collision speed (`m_fCollisionSpeedToBreakWhenFallenOn`,
`m_fMinSpeedToCauseDamage`), and the fire above. The one thing the Kismet layer needs from it: the
step index and a stable owner id for `events_for`.

### 3.5 `DisSeqAct_RefillWhaleOilBattery` (224)

What it is. Refills `DisWhaleOilBattery` actors (146 placed; the whale oil batteries of the
Flooded District / Overseer / Refinery maps). Properties: `m_PercentageCharge` (float, usually a
variable) and `m_RefillSound` (AkEvent); Targets link `Battery`; `bCallHandler` false.

`Activated` (`0x00c82190`): for each target that is a `DisWhaleOilBattery`:
`SetCharges(int(m_PercentageCharge * 0.01 * tweak.m_NumberOfCharges (+0x11c)), notify = 0)`
(`0x00c81d70`: `m_CurrentCharges (+0x428) = n`; when it was < 1 and n > 0 the battery's life counter
`+0x388` = `tweak.m_Health (+0xb0)`, and when n < 1 it is set to -1; material swaps by
`m_MaterialChangeOnChargeUse` thresholds), then the refill sound at the battery. Then
`SequenceAction::Activated` and the automatic output. Note `int()` truncates.

Port: unsupported, finishes with the automatic output (fine for logic). Needs the battery actor
(`m_CurrentCharges`, `m_pPlug`, `m_bRecharging`), the charge state in the save, and the visual state;
the Kismet part is ten lines once the actor exists.

### 3.6 `SeqAct_SetPhysics` (218)

Engine op, `bCallHandler` true, one property `newPhysics` (`EPhysics`: None 0, Walking 1, Falling 2,
Swimming 3, Flying 4, Rotating 5, Projectile 6, Interpolating 7, Spider 8, Ladder 9, RigidBody 10,
SoftBody 11, NavMeshWalking 12, Custom 14). The base `SequenceAction::Activated` calls
`Target.OnSetPhysics(action)` on every target: `Actor.OnSetPhysics` is `SetPhysics(Action.newPhysics)`
(script), `Controller.OnSetPhysics` forwards to its pawn. Data: PHYS_RigidBody on props (the value
is stored only when non-default, so None is the unmarked case) with Targets from a variable or a named
variable.

Port: unsupported, outputs fire. Needs: an actor "physics mode" for level actors/pawns the Kismet can
address (`PHYS_RigidBody` on a movable/breakable = it starts to simulate and fall; `PHYS_None` freezes
it; for pawns `PHYS_Walking/Falling/None` map onto the sim's movement mode). Without rigid bodies in
the port the minimum is to record the mode and apply `None` (freeze) and `RigidBody` (gravity fall
with collision against the static world) to the actors that can move; do not touch pawns that are
walking. UNVERIFIED which actors are the targets in Boyle (none of the 218 is in the first five
seconds).

### 3.7 Next tier (trivial or already decoded fire sites), by object count

| Class | Objects | Fire / handler (appendix A) | Note |
|---|---|---|---|
| `DisSeqEvent_Possessed` | 154 | `0x00a52130`, `0x00a5a990`, `0x00b7d110`, `0x00b7d2f0` | possession start/end of a pawn/rat/fish |
| `DisSeqEvent_PatrolPointReached` | 151 | `0x00af2300`, `0x00afd350` | AI patrol node reached; port's brain has patrol nodes |
| `DisSeqEvent_MovablePickedUp` / `Dropped` | 127 / - | `0x00a45fd0` / `0x00a46380` | fed by the movables port (movables.md): originator the prop, instigator the pawn |
| `DisSeqEvent_WhaleOilReceptacle` / `WhaleOilBattery` | 125 / 35 | `0x00c7b490`, `0x00c7b6b0` / `0x00c7d660`, `0x00c81d70` | with 3.5 |
| `DisSeqEvent_Combat` / `BehaviorStarted` | 88 / 83 | `0x00af7d00` / `0x00af7670` | brain state changes |
| `DisSeqEvent_TaskStateChanged` | 79 | `0x00b9a8f0`, `0x00ad1320` | objective plumbing: the port already has tasks |
| `DisSeqAct_CompleteObjective` / `FailObjective` / `DisSeqCond_IsObjectiveComplete` / `HasObjective` | 61 / 22 / 45 / 40 | - | mission logic, small |
| `DisSeqCond_IsSentinel`, `IsPlayerInCombat`, `CompareBoolExtended` | 91 / 53 / 74 | - | conditions |
| `SeqEvent_RigidBodyCollision`, `SeqAct_GetDistance` | 73 / 66 | `0x0078f7d0` | |
| `DisSeqAct_SetAudioOcclusion` | 98 | - | presentation: add to `PRESENTATION` |

Status of the AI-owned rows (2026-10-08, AI events pass; the per-event decode is in KISMET.md "AI events pass"):
`PatrolPointReached` output 0, `BehaviorStarted`, `Combat`, `Investigate`, `FleepointReached`, `Corpse` (all three
outputs) with `CorpseDetected`, `PawnSighted` and `Assassinated` output 0 are ported (`kismet_host::ai_events`,
`FED_EVENTS`). Open: `PatrolPointReached` output 1 (guard post point actors), `Assassinated` output 1 (kill notify),
`PlayerHeard` (writer of the brain's pending-name field not found), `AttackedByRats` (NPC master state entry),
`WatchTower` (no tower in the port), `BehaviorStarted` for classes the port's behaviours lack.

---

## 4. Findings (each with the fix)

| Id | Kind | Severity | One line |
|---|---|---|---|
| F-1 | port bug | medium | delayed-due ops and repeat activations are queued on top; the original queues them at the bottom |
| F-2 | port bug | medium | kept latent ops come back in encounter order; the original reverses, and only if still active |
| F-3 | port missing | medium | Kismet time is not scaled by world/player dilation; `m_bAlwaysOutOfBendTime` ignored |
| F-4 | port missing | low | retrigger/activation time on one clock; the original has two |
| F-5 | known difference | low | populate/publish generalisation; PlayerIndex/GamepadID |
| F-6 | oracle limitation | high for testing | the port's Kismet trace is host notes; per-op activations are not recorded |
| F-7 | port missing | medium | `DisSeqEvent_Distracted` + `DisNPCDistractor` AI |
| F-8 | port missing | medium | `AttentionIncreasedTo/DecreasedTo` never fire |
| F-9 | port missing | low-medium | `BreakableBroken` never fires |
| F-10 | port missing | low | `RefillWhaleOilBattery` |
| F-11 | port missing | low | `SetPhysics` |
| F-12 | verified equal | - | list below |
| F-13 | unverified | high if different | Kismet vs actor order inside the world tick |
| F-14 | doc correction | - | "39 activations" in the brief and ORACLE.md are the port's host notes |

**F-1. Queue position of delayed-due and repeated ops.**
Evidence: `ExecuteActiveOps` calls slot 101 with `bPushTop` = 0 at `0x00700948` (delayed ops that
came due; `edi` is the null that ended the previous search loop) and at `0x00700f0d` (`push 0` for an op
whose input had queued activations); `FUN_006e5050` inserts at index 0 when the flag is 0. The
target loop's call (`0x00701112`) pushes 1.
Port: `runtime.rs : execute_active_ops` (`self.queue_op(seq, op, true)` after `self.impulse` in the
delayed block) and `run_op` (`if again { self.queue_op(seq, op, true); }`).
Fix: both calls become `queue_op(seq, op, false)`. Effect: an op that comes due runs after the gameplay
events and kept latent ops already on the stack instead of before them; a repeated activation runs
after everything else queued. Only the order of independent chains in one tick changes (visible in the
Kismet trace, and in races such as a Gate closed by an event the same tick).

**F-2. Re-queue of kept latent ops.**
Evidence: `0x00700b1d` adds the op to a local array; after the loop (`0x00701452..0x007014f1`) the array
is emptied from the end, and each op that is still `bActive` (`test byte ptr [edi+8Ch],4`) is queued with
`bPushTop` = 1. So A, B, C (encounter order) end up with A on top.
Port: `execute_active_ops` appends `latent_keep` in encounter order and does not test `active`.
Fix: `for op in latent_keep.into_iter().rev() { if self.ops[op].active { self.queue_op(seq, op, true); } }`.
(An op can be popped twice in one pass if impulses were queued; `queue_op` already ignores duplicates.)
Effect: with two latent ops finishing in the same tick, the one met first in the previous tick finishes first.

**F-3. Time scaling.**
Evidence: K-2. Port: `Kismet::tick(host, dt)`, `execute_active_ops`, `update(host, op, dt)`,
`delay_update`, `interp_update`.
Fix: add `Host::kismet_time_ratio() -> f32` (default 1.0; the mission host returns
`world_dilation / player_dilation` of the active Bend Time state, the same pair the sim uses for NPCs;
guard player dilation 0). In `execute_active_ops` compute `scaled = dt * ratio` and use
`if prop_bool(op, "m_bAlwaysOutOfBendTime") { dt } else { scaled }` for the delayed countdown
(per delayed entry: the flag of the **target op**) and for `update`. Matinee playback
(`interp_update`) uses the same dt, so an unflagged matinee slows with the world and stops at level 2.
Flagged ops in Boyle are listed in K-2. Check with section 6 (F-3 scenario) before relying on the
`TimeDilation`-scaled-dt assumption.

**F-4. Two event clocks.**
Fix: keep `time_world` (advances by `dt_scaled` per tick, the original's `BendTimeSeconds`) and
`time_player` (by `dt`, `TimeSeconds`); `activate_event` stores, and `check_activate` compares, the one
selected by the event's `m_bAlwaysOutOfBendTime`. Equal to today when no Bend Time is active.

**F-5. Variables.** Implement the link-level semantics of K-6 in `populate`/`publish`: scalar int/float
= sum over the link, bool = AND (SetBool; check `SeqVar_Bool` slot 93 `0x006f6740` for the rule before
generalising), objects = first-wins or append per property type; publish scalars to all variables.
Only needed where a data link has several variables of one type on a scalar property. Keep the existing
`Targets` handling. `PlayerIndex`/`GamepadID`: set to 0, no change in single player.

**F-6. The Kismet trace is not an activation trace.**
`tools/oracle/src/runner.rs:558-562` pushes the host's `note()` strings as `Kismet` events
(`object` empty, `name` = `[<seconds>]`). The 39 "activations" of `kismet/boyle_start` are 39 log lines
(spawn notes, "matinee bound", ...). The hook records `ProcessEvent` `Activated`/`Deactivated` on every
`SequenceOp`. As the two never share a name, the comparison reports nothing.
Fix (oracle, not the port crate): give `Kismet` an activation log `Vec<(tick, OpId, Activated | Deactivated)>`
(`run_op`: before `activated` and after `deactivated`; `activate_event`), have the runner emit one
`TraceEvent { kind: "Kismet", object: <export name>, name: "Activated" | "Deactivated" }` per entry with the
step's tick, and keep the notes as kind `KismetNote` (not compared). `object` must use the original's
object name; the first captured trace of `kismet/boyle_start` fixes the exact string (expected: the
export name such as `SeqAct_PrepareMapChange_0`, without package; `db.describe(op)` is
`Package:Name[export]`). Same-tick order is part of the comparison (K-4), so the log must be in call
order.

**F-12. Verified equal to the original (no action).**
Depth-first order of output targets; input/output clearing; queued activations and the `bDisabled`
input; 999-pop cap and replay of queued event activations; `CheckActivate` conditions 3-7 and
`ActivateEvent` counts/time; remote events populate the event first and queue at the bottom; the
`SeqAct_Delay` timing (armed on the tick before it counts); nested sequence update order; the
`DisSeqAct_Lock` op.

**F-13. UNVERIFIED: Kismet inside the world tick.** See K-1 and section 6.

**F-14.** The brief and ORACLE.md ("Kismet traces: ChangeLvl_fromHub_toBoyle fired 39 activations
in 5 s") describe the port's note count. No original Kismet trace exists in the cache yet (checked
`.../cache/oracle/70da96138fe2171c/`: no `kismet/` directory); the scenarios are queued in
section 6.

---

## 5. What the port triggers from gameplay (against appendix A)

Triggered: `DisSeqEvent_Used`, `DisSeqEvent_PickupPickedUp`, `DisSeqEvent_Spawned`,
`DisSeqEvent_Door`, `SeqEvent_Used`, `SeqEvent_Touch`, `SeqEvent_LOS`, `SeqEvent_TakeDamage`,
`SeqEvent_Death`, `DisSeqEvent_NPCIncapacitated`, `DisSeqEvent_KnockedOut`, `DisSeqEvent_PutToSleep`
(comments in `kismet_host.rs`), `DisSeqEvent_DialogOutputs`, `SeqEvent_RemoteEvent`,
`SeqEvent_LevelLoaded`/`LevelStartup`/`SequenceActivated`, `SeqEvent_Console`.

Not triggered (the native fire sites to port, appendix A): Distracted, AttentionIncreased/DecreasedTo,
BreakableBroken, Possessed, PatrolPointReached, Combat, BehaviorStarted, Investigate, PawnSighted,
PlayerHeard, CorpseDetected, Corpse, Assassinated, Choke, Windblasted, TeleportSpell, Tripwire,
MovablePickedUp/Dropped, WhaleOilBattery/Receptacle, KeyHoleUsed, Climb, Lock, DefenceTower,
WatchTower, WallOfLight, AlarmEnded, ActorTouched, WitnessedInteraction, Interact, PlayerEvent
family, TaskEvent family, PowerUsed, `SeqEvent_Destroyed`, `RigidBodyCollision`, `AnimNotify`,
`ParticleEvent`, `Mover`. Several (Investigate, PawnSighted, PlayerHeard, Combat, BehaviorStarted)
fire from the brain code the AI porters already hold: the thin helper `0x00bc7b30` at
`0x00b04100` (PlayerHeard), `0x00b0be40/0x00b0bef0/0x00b0bf30` (PawnSighted), `0x00b040c0/0x00b04140/
0x00b0bf70` (ActorTouched), `0x00b04190` (WitnessedInteraction), `0x00b2e050` (Investigate).

---

## 6. Scenarios, captures, unverified checks

Scenarios (`fixtures/oracle/scenarios/kismet/`, queued in `fixtures/oracle/capture_queue.txt`):

| Scenario | What one capture settles |
|---|---|
| `kismet/boyle_start` (exists) | the first 5 s of `ChangeLvl_fromHub_toBoyle` activation by activation: depth-first order, the remote-event tick, spawner events |
| `kismet/boyle_intro_20s` (new, 1200 ticks, port runs it in 10 s) | the same for 20 s: Samuel's intro conversation, objectives, delayed links, latent op finishing order (F-2), delayed-due position (F-1), and the activation tick of everything relative to the world clock (F-13: the first activation tick) |

F-6 is ported (section 7): the port records the same per-op events, so a capture compares directly.

Unverified checks and what decides them:
- **F-13 (Kismet vs actors).** From the two traces: in the original, find an op that depends on a
  gameplay fact of the same tick (a `DisSeqEvent_Spawned` after the spawner's `StartSpawn`, or the first
  `SeqEvent_Touch` of the player standing in a trigger at start) and read the tick of its `Activated`
  next to the tick of the cause. A cause at tick k and an activation at tick k means Kismet runs after
  the actors; at k + 1 means before. The port's convention is "same tick".
- **F-3.** Needs a Bend Time capture with a running unflagged Kismet delay; scenario to write once
  a Boyle mission step with a `SeqAct_Delay` is reachable after the intro (the intro locks control).
  Cheap partial check queued as `session` reads (the dilation fields) in the queue file: confirms
  the field names and values only.
- **3.3.** whether attention levels step one per tick or jump (read `0x00b60c10`).
- **3.1.** which of `0x00b84670` / `0x00b84750` fires output 0 for a given distraction type (a trace in
  Boyle Int with `Soiree_Directory`).

---

## 7. Port status (2026-10-08)

Each cited function was re-read (`oracle index fn`, `exe_probe dis`) before porting.

| Id | Status | Where | Notes |
|---|---|---|---|
| F-1 | **ported** | `runtime.rs : execute_active_ops` (due delayed ops), `run_op` (repeat) | `queue_op(.., false)`; `edi` = 0 at 0x00700948 checked (set at 0x007008ac); `push 0` at 0x00700f0d. Test `delayed_activation_that_comes_due_runs_after_the_ops_already_queued`, `repeated_activation_goes_to_the_bottom_of_the_stack`. |
| F-2 | **ported** | `execute_active_ops` | reverse, `bActive` test (0x007014db), push top; the local list is AddUnique (0x00514c60). Test `kept_latent_ops_come_back_in_reverse_so_the_first_met_updates_first`. |
| F-3 | **ported** | `runtime.rs : tick_dilated`, `execute_active_ops`; `kismet_host.rs : step`; `dishonored_sim::powers::player_time_dilation` | The host passes `dt x player dilation` (the world tick's TimeDilation-scaled dt) and `world / player` from the powers' Bend Time / Blink channels (`time_bends`, `blink_layers`: Blink's channel has player rate 1). Flag of the target op for delayed entries. `dt * ratio` is computed in double (the x87 product) and stored as f32. LIKELY part unchanged: the dt handed to `UpdateOp` is TimeDilation-scaled (consistent with U-109). Test `bend_time_slows_unflagged_ops_and_not_flagged_ones`. |
| F-4 | **ported** | `runtime.rs : event_clock` (`check_activate`, `activate_event`) | `Kismet.time` = TimeSeconds (+= dt), `Kismet.bend_time` = BendTimeSeconds (+= dt x ratio = world rate, as U-109 measured). `SeqAct_Delay` keeps stamping TimeSeconds. Test `retrigger_delay_runs_on_the_bend_time_clock_unless_flagged`. |
| F-5 | **ported** (scalars) | `runtime.rs : populate_scalar` | The first variable's class decides: `SeqVar_Int` sum (0x006f6da0), `SeqVar_Float` float sum from 0 in link order (0x006f69b0), `SeqVar_Bool` AND (0x006f6740), over the link's variables of that class. Publish already wrote a scalar to every variable. Array properties unchanged. PlayerIndex / GamepadID not kept (single player: 0). Test `scalar_int_and_float_properties_take_the_sum_of_the_link`. |
| F-6 | **ported** | `runtime.rs : record`, `activations`, `trace_name`, `activation_detail`; `tools/oracle/src/runner.rs` | Log entries: an op's `Activated` after its native `Activated` (with the inputs carrying an impulse), `Deactivated` after its native `DeActivated` (outputs), an event's `Activated` in `ActivateEvent` (outputs). Trace: kind `Kismet`, object = `Package:Outer.Name` without `TheWorld.PersistentLevel.` (the hook's `short_path`), name `Activated` / `Deactivated`, detail `class=.. in=..` / `out=..`; host notes go out as `KismetNote`. Attached duplicates are named `<Event>_AttachDup` in the attaching sequence (UNVERIFIED). `kismet/boyle_start`: 301 `Kismet` events in 5 s (and the 39 notes). Test `activation_log_lists_activated_and_deactivated_in_call_order`. |
| F-7 | **ported** | `dishonored_ai::distraction`; `kismet_host.rs : feed` (`AiEvent::Distracted`), `"DisSeqAct_AIStartDistraction"`, `toggle_actors` | `FireDistracted` 0x00b79b80: every `DisSeqEvent_Distracted` of the distractor checks with the pawn as instigator and `[output]`; the cooldown reset is the component's (the AI). Output 0 from the follow-up on arrival (0x00b84d60 -> 0x00b84750 soiree / 0x00b84670 animated: the two call sites are the two variants, which settles section 6's question), 1 from the soiree's end (0x00b844f0) or the distracted state's exit (0x00b84810), 2 from the look (0x00b7da20). `DisSeqAct_AIStartDistraction` now posts the animate message (0x00b63ed0: ready ignoring the cooldown, positive animate chance) instead of a walk; `SeqAct_Toggle` on a `DisNPCDistractor` switches `m_bDistractionEnabled` (0x00a4c350 -> 0x00b83b60). Scenario `kismet/boyle_distractor_railing`: Start at 4.07 s, End 8.93 s later (the Kismet's `SeqAct_Interp` plays the soiree with the pawn bound). Rule details and limits: world.md 10.1 F-13. |
| F-8 | **ported** | `kismet_host.rs : attention_events` (from `feed`) | 0x00b3b350 calls 0x00b363d0 once per target batch with the first record's old level and the last record's new level (a jump; this settles the section 6 question for the fire site). The port diffs each brain's per-target levels once per step (the brains update attention once per tick): originator the observer pawn (events found through the pawn and its spawner), instigator the target, `m_eActivatedAttentionLevel` set, the level gate and the `bPlayerOnly` player test, then `CheckActivate`. The noise pseudo-target is skipped; the DLC06 variants (indices `[level]`) are not in the port's maps. |
| F-9 | **ported** | `dishonored_sim::world_actors`; `kismet_host.rs : world_events` | The step routine is 0x00a3a5f0 (from the tick 0x00a3b0d0; 0x00a34460 is a second break path, called by other code, not ported): `0x00bc7b30(breakable, class, breakable, 0, 0, [Num(m_Steps) - step - 1])`. Damage comes from grenade explosions only (world.md 10.1 F-12). Scenario `kismet/boyle_int_bottles_grenade` (8 BreakableBroken output 0 in the blast tick). |
| F-10 | **ported** | `kismet_host.rs` `"DisSeqAct_RefillWhaleOilBattery"`, `"DisSeqAct_PlugWhaleOilBattery"`, `world_events` | Refill 0x00c82190 (truncated `m_PercentageCharge * 0.01 * m_NumberOfCharges`, `SetCharges(n, 0)`), Plug 0x00c7d7c0 (Battery = `Targets`, Receptacle link; counts must match; both inputs: nothing), `DisSeqEvent_WhaleOilBattery` (0x00c81d70, outputs 0 ChargeUsed / 1 AllChargesDepleted; 2 / 3 from the battery's damage path, not ported) and `DisSeqEvent_WhaleOilReceptacle` (0x00c7b490 OnPlug, 0x00c7b6b0 OnUnplug) fire inside the op as the original's do. world.md 10.1 F-14. |
| F-11 | partial | `kismet_host.rs` `"SeqAct_SetPhysics"` | the class is handled: the `EPhysics` per target is recorded (`HostState.physics`); nothing moves (no rigid bodies, no per-actor physics mode in the sim). |
| F-13 | open | - | needs the captures of section 6. |
| F-14 | **done** | ORACLE.md | the 39 were the host's notes; corrected. |

The trace of the port's `Sequence` op (`fromHub_toBoyle`) shows an `Activated` with no `Deactivated` (the port's `Sequence::Activated` clears `bActive`); the first capture of `kismet/boyle_start` decides whether the original does the same.
## Appendix A. Native fire sites per event class

Method: class pointer cache `DAT_xxx` (from the lazy getter) -> `exe_probe find-imm` ->
containing function (`oracle index fn <site> --no-c`). The list includes the class cast helpers and
the registration getters (functions at `0x00b8a2xx`-`0x00b8e8xx`, `0x00b89xxx`, `0x00b8dfxx` are
casts/getters, not fire sites). Functions that only read the global to test a class (`is_a`) are also
listed; open each one. Event classes the port already triggers are marked `*`.

```
SeqEvent_AnimNotify 0070f4f0 | SeqEvent_Console* 007ec5a0 | SeqEvent_ConstraintBroken 007cf350
SeqEvent_Destroyed 00652cf0 | SeqEvent_LevelLoaded* 006fe400,00700120,007004b0,00701d10,00a02760
SeqEvent_LevelStartup* 007004b0 | SeqEvent_ParticleEvent 0089fc00
SeqEvent_RemoteEvent* 006fd8a0,006fe450,00701ec0,007ec5a0,00cbafc0
SeqEvent_RigidBodyCollision 0078f7d0 | SeqEvent_SequenceActivated* 006f36f0,00700120,007016b0
SeqEvent_TakeDamage* 0056a080,005753f0,00b72b50,00b741c0
SeqEvent_Touch* 00569f40,00579c70,0057a8c0,0057ed60,00a06af0 | SeqEvent_Used* 00a1f1f0,00a218c0
SeqEvent_LOS* 0057f090 | SeqEvent_CrowdAgentReachedDestination 0095a230,009678f0
DisSeqEvent_DefenceTower 00a18120,00a258f0,00a2fce0,00a2feb0,00a2fff0
DisSeqEvent_WallOfLight 00a19230,00a28f80,00a29170,00a3c010,00a3eb10,00a3eca0
DisSeqEvent_WatchTower 00a19580,00a2a770,00a31560,00a31740 | DisSeqEvent_AlarmEnded 00a19df0,00a371f0
DisSeqEvent_KeyHoleUsed 00a1a3a0,00a2e4e0,00a38580 | DisSeqEvent_Lock 00a42240,00a4e870,00a4e910
DisSeqEvent_RiverKrustLooted 00a445e0,00a52840 | DisSeqEvent_Climb 00a5fee0,00a6e2c0,00a72b00
DisSeqEvent_PutToSleep* 00a7cfb0,00a994c0 | DisSeqEvent_Corpse 00a7d080,00a90b50,00a998a0,00b40740
DisSeqEvent_Assassinated 00a78270,00a7beb0 | DisSeqEvt_Choke 00a6d380,00a74720,00a7c270,00a7c4d0
DisSeqEvent_ActorTouched 00b040c0,00b04140,00b0bf70 | DisSeqEvent_BehaviorStarted 00af7670
DisSeqEvent_Combat 00af7d00 | DisSeqEvent_Door* 00a2d1f0 | DisSeqEvent_FleepointReached 00a47340
DisSeqEvent_Investigate 00ae9fe0,00b2e050 | DisSeqEvent_KnockedOut* 00b7bf50
DisSeqEvent_NPCIncapacitated* 00a53670,00b7bf50 | DisSeqEvent_PatrolPointReached 00af2300,00afd350
DisSeqEvent_PawnSighted 00a316a0,00a4d5d0,00b0be40,00b0bef0,00b0bf30 | DisSeqEvent_PlayerHeard 00b04100
DisSeqEvent_Spawned* 00a55bd0,00a58d30,00a590e0,00cf1260
DisSeqEvent_TeleportSpell 00c2ba40,00c35280,00cd7090,00ce0aa0 | DisSeqEvent_Tripwire 00a21a30
DisSeqEvent_Used* 00a53ec4,00a57260,00a57640,00a57cc0 | DisSeqEvent_CorpseDetected 00b40740
DisSeqEvent_Possessed 00a52130,00a5a990,00b7d110,00b7d2f0 | DisSeqEvent_Windblasted 00b72ba0,00b72bf0,00c4de90,00c53790
DisSeqEvent_AttackedByRats 00a70450 | DisSeqEvent_DLC05_DropKilled 00a7beb0
DisSeqEvent_PickupPickedUp* 00a1f2c0,00a495b0 | DisSeqEvent_BreakableBroken 00a34460,00a3a5f0
DisSeqEvent_DialogOutputs* 00cb1080,00cb1350 | DisSeqEvent_Distracted 00b79b80,00b79cf0
DisSeqEvent_Interact 00a3df60,00a3e0b0 | DisSeqEvent_MovableDropped 00a46380
DisSeqEvent_MovablePickedUp 00a45fd0 | DisSeqEvent_WitnessedInteraction 00b04190
DisSeqEvent_PlayerTutorialEvent 00ab1f60,00c15060,00c2a250 | DisSeqEvent_PowerUsed 00bf32f0,00cf64b0
DisSeqEvent_RatPossess 00c66340,00c7fbc0 | DisSeqEvent_FishPossess 00a04820,00a06710
DisSeqEvent_AttentionIncreasedTo 00b2cca0 (cast; fire 00b363d0)
DisSeqEvent_AttentionDecreasedTo 00b2ccf0 (cast; fire 00b363d0)
DisSeqEvent_PlayerInventoryChanged 00ba1a30 | DisSeqEvent_PlayerManaThreshold 00ab1a30
DisSeqEvent_ObjectiveCompleted 00ad0e00 | DisSeqEvent_PlayerCombat 00ab9fa0
DisSeqEvent_PlayerCrouch 00ab0960 | DisSeqEvent_PlayerHolsterWeapon 00ab3d80
DisSeqEvent_PowerEquipped 00c44fe0,00c4ba70 | DisSeqEvent_TaskActivated 00ad1080,00b9a8d0
DisSeqEvent_TaskStateChanged 00ad1320,00b9a8f0 | DisSeqEvent_JournalViewed 00bb9960,00bd11e0
DisSeqEvent_WhaleOilBattery 00c65760,00c7d660,00c81d70 | DisSeqEvent_WhaleOilReceptacle 00c65c50,00c7b490,00c7b6b0
DisSeqEvent_DLC05_Challenge 00cbb240,00cbdd50 | DisSeqEvent_DLC05_PlayerDeath 00cbe2b0
```

## Appendix B. Evidence index

- Pop loop and clocks: `0x00700740` (+ `exe_probe dis 0x00700a50 40`, `0x00700e60`, `0x00701452`);
  `Sequence::UpdateOp` `0x00701550`; `QueueSequenceOp` `0x006e5050`; delayed add `0x006d9ec0`;
  InheritPlayerIndex `0x006cf2c0`; Player Index / Gamepad Id `0x006fef20`.
- Events: `CheckActivate` `0x006cf3a0`, `ActivateEvent` `0x006f5dd0`, event register `0x006e50b0`, clock getters
  `0x00781200` (TimeSeconds `+0x38c`) / `0x00781220` (BendTimeSeconds `+0x398`), retrigger clock `0x006c8350`.
- Dilation: `DishonoredGameInfo` vslot 244 `0x009e9e90`; fields `+0x3a8/+0x3ac`.
- Variables: Populate `0x006ef6c0`, Publish `0x006d9710`, `SeqVar_Int` `0x006f6da0` / `0x006f6f90`.
- Delay: `0x006cf6c0` (Activated), `0x006dba50` (Update).
- Distracted: `0x00b79b80`, `0x00b79cf0`, `0x00b844f0`, `0x00b84580`, `0x00b84670`, `0x00b84750`, `0x00b84810`,
  `0x00b84970`, `0x00b84c20`, `0x00b842f0`, `0x00b84e90`, `0x00b7da20`, `0x00b28550`, state `0x00a61940`.
- Attention: `0x00b363d0`, `0x00b3b350`, `0x00b45910`, `0x00b8a2a0`, casts `0x00b2cca0` / `0x00b2ccf0`.
- Breakable: `0x00a34460`, `0x00a3a5f0`. Battery: `0x00c82190`, `0x00c81d70`. Lock: `0x00b8f730`.
