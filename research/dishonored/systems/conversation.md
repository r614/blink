# Conversations, barks and cutscenes: core rules and port audit

Scope: the dialog runtime (`DisConversationComponent`, `DisConvGlobalMan`, `DisConv_*` graphs), the
bark ("dialog hook") pipeline, subtitles and VO timing, and matinee / soiree control.
Port: `crates/dishonored_conversation` (`hooks.rs`, `runner.rs`, `load.rs`, `model.rs`),
`dishonored_mission::runtime` (`fire_ai_hooks`, `approach_hooks`), `dishonored_ai` (`transition_barks`),
`dishonored_runtime` (`hud_dialogue.rs`, `dialogue_vo.rs`, `cutscene.rs`), `dishonored_kismet::interp`.
Earlier decodes: `CONVERSATIONS.md` (graph evaluation, which this document does not repeat), `CUTSCENES.md`,
`AI.md` section "Barks". Everything below was read from the decompiled exe (`oracle index fn`, GOG exe
`70da9613...`) and from cooked data (`Default__DisTweaks_ConvSettings` via `obj_dump`); nothing is guessed.
Addresses are virtual addresses. Evidence classes: **Verified** = read from code or data and checked against
the port's source; **Unverified** = needs an original capture (listed in section 6).

Status of the original side: no original trace of this system exists yet. The scenarios in section 5 are
queued (`fixtures/oracle/capture_queue.txt`); until they land, port-vs-original statements below are
code-to-code comparisons, not trace diffs.

---------------------------------------------------------------------------------------------------

## 1. The core rules (original)

### R1. Requesting a hook: `FireHook` (0x00ca6bd0)

A behaviour or pawn asks its `DisConversationComponent` to fire hook `h` with an optional initiator `t`
(`FUN_00ca7360` queued form; `FUN_00ca73c0` is the immediate form that skips the queue and calls the fire
step directly). The steps, in order:

1. The hook must be *supported*: bit `h` of `m_IsDialogHookSupported_Cache` (+0x17c, 4 ints) is set (some tree
   of the NPC has the hook). Otherwise nothing happens and nothing is recorded.
2. The NPC's health interface says dead: stop, except hooks 57-59 (`DDH_COMBAT_DYING`, `_BACKSTABBED`,
   `_STEALTHKILL_PROJECTILE`), which still fire on a dead owner.
3. A trial walk (`FUN_00ca4690`, tree order below) finds a tree whose hook slot `h` exists, asks the hook node
   for its output index (virtual +0x158, see R5; -1 = the hook cannot start) and runs (queued and immediate
   requests alike: `FUN_00ca6bd0` always passes 0 for the skip flag)
   the availability test `FUN_00c9aec0(speaker, priority 4, hook, flags 4)` (R4). Failure: stop.
4. **Per-NPC refire gate** (the NPC's `m_DialogHookInfo_InGame[h]`, +0x98, holds `next_h`):

       pass  iff  next_h < 1e-8                      (never fired)
              or  min_h < (now - next_h) + 1e-8      i.e.  now > next_h + min_h

   `min_h`, `max_h`, `global_h` are the hook row's fields at +0x38 / +0x3c / +0x40 (R9). On fail the request
   is recorded in the NPC's failed list with reason 0 (`RejectedByRefireDelay`).
5. **Global refire gate** (`DisConvGlobalMan.m_DialogHookInfo_InGame[h].m_fLastFireTime`, +0x558): pass iff
   `last < 1e-8` or `global_h < (now - last) + 1e-8`. All `global_h` are 0 (rows and tweak overrides), so this
   gate never rejects. Fail: reason 1.
6. Immediate form: fire now (R4). Queued form: `FUN_00c9cc80` enqueues (R3).

The fire step (`FUN_00ca5670`) on success stamps both tables:

    next_h        = now + rand() * (max_h - min_h) / 32768
    last_global_h = now
    comp.m_FireHookTag += 1

Combining 4 and the stamp: **the effective refire delay on one NPC is `min_h + u * (max_h - min_h)`**,
`u = rand() / 32768` in [0, 1), counted from the moment the hook actually starts. (The earlier reading
"the minimum is not added" in `CONVERSATIONS.md` section 4 and `hooks.rs` is wrong: the gate adds it.)

Test vector: `DDH_SEARCH` (override min 8, max 12). Fires on NPC A at t = 10.0 with rand() = 16384
(u = 0.5): `next = 12.0`. A second `DDH_SEARCH` request on A is refused until `now > 12.0 + 8.0 = 20.0`
(10 s after the first). With `DDH_PLAYER_USED` (min = max = 0): always allowed. A default AI hook (min = max = 1):
refire 1.0 s exactly.

### R2. Tree order and the fallback chain (0x00ca5670, 0x00ca5150, 0x00ca49e0)

The hook is tried in: the one-shot trees bound to the NPC (`m_OneShotInfo`, +0x80), the additional generic
trees (+0x5c), then its generic tree (+0x54). Within each tree the chain `m_pFallbackTree` (+0x148) is
followed **only** while the current tree lacks a hook of that type (trial walk `FUN_00ca4690`) or its walk ends
in a `DisConv_TryFallbackTree` node. That node's `Evaluate` (0x00ca7b20) sets a "continue to the fallback
tree" flag on the walk state (`FUN_00c8b290`) and returns -1; `FUN_00ca5150` then moves on to
`m_pFallbackTree`. A walk that ends any other way stops the search (success when it started something).
`FUN_00ca49e0` binds the speakers for every hook walk (`FUN_00c9e2b0`: ME = owner, INITIATOR = initiator; this
runs the availability rule R4) and then runs the normal walker (`CONVERSATIONS.md` section 1).

### R3. The queue, selection and caps (`DisConvGlobalMan` tick: 0x00ca71c0 -> 0x00ca5b20)

`DisConvGlobalMan.m_QueuedHookGroups[13]` (indexed by fire group), each with two lists, **Loud (0)** and
**Quiet (1)** (`DisQueuedFireDialogHookGroup`: `m_PendingHooks`, `m_FiringHooks`, per-group cap, per-initiator
cap, ignore-global-cap, min spacing, `m_bIsLocked`, `m_fGroupLockoutTimer`; entry 0x4c bytes). The settings
object is `DisTweaks_ConvSettings` (`m_pConvSettingTweaks`, +0x7f8); the cooked class defaults are the only
data (R9).

**Enqueue** (`FUN_00c9cc80`, from R1 step 6):
- list = `[group of the hook row (+0x34)] x [Quiet if the speaker's audio path distance >= 2000 else Loud]`.
  The audio path distance (`FUN_00c96920`, via `FUN_00c9a300`) is the minimum, over the speaker's audio
  emitters, of the Wwise-style obstructed path length to the listener (`FUN_00bb52e0`), cached per emitter
  until the emitter or listener moves by 100 uu; 2000 is `m_fDialogDistanceConsideredLoud`.
- if the list is locked (`m_bIsLocked`): reject, reason 3 (`FiringGroupLocked`).
- per-initiator cap: if the list's per-initiator cap > 0 and there is an initiator that is not the owner and
  the number of that initiator's hooks already firing in this list type (`m_InitiatorCountMap`, +0x754, one map
  per Loud/Quiet) is `>=` the cap, reject (reason 4) unless the owner's current running hook already has that
  initiator.
- otherwise append `{hook, sub-index, fire tag, component, initiator, copy of m_HookParams}` to
  `m_PendingHooks`.

**Tick** (every frame, `FUN_00ca5b20`):
1. For each pending entry whose speaker exists, recompute Loud/Quiet and move it to the other list if the
   class changed since the enqueue.
2. `slots[Loud] = m_MaxDialogHooksAtOnce` (5), `slots[Quiet] = m_MaxDialogHooksAtOnce_Quiet` (5).
3. For group g = 12 down to 0 (so **GameplayImportant, CombatImportant, SoundAlarm, WitnessedMagic, Threat,
   Combat, CombatCries, Agitated, Exertions, Reaction, Attention, CasualStateChanges, Casual** in that order),
   for list t in {Loud, Quiet}, with S = the group's list:
   - if S is locked: `lockoutTimer -= dt`; at <= 0 unlock.
   - `room_group = cap < 1 (unlimited)  or  cap - firing.count > 0`   (cap is `m_MaxPlaying_PerGroup`;
     -1 and 0 both mean unlimited).
   - unless the group ignores the global cap: `slots[t] -= firing.count` (every hook of every non-ignoring
     group that is still playing, whatever its flags, uses a slot).
   - `room_global = slots[t] >= 1  or  group ignores the global cap`.
   - if `room_group and room_global and pending non-empty`: weigh every pending entry (below), sort
     **descending** by weight (`FUN_00c92850`, selection/quick sort, unstable), then for each entry in order:
     stop when `room_group` is exhausted or the weight is < 0.0001; otherwise copy its `m_HookParams` into the
     component (+0x124) and run the fire step (R1). Each *attempt* uses one unit of group room, success or not.
     A success appends the entry to `m_FiringHooks`, counts its initiator, and (non-ignoring groups) takes one
     global slot; when the global slots reach 0 the rest of the list stops.
   - **every entry not fired is dropped** (recorded with reason 5) and `m_PendingHooks` is emptied. A request
     lives for exactly one manager tick; there is no waiting.
4. Housekeeping over all 26 lists: a firing entry is *still playing* only while the component's running
   state (+0x8c) has that hook (byte +0x1d), sub-index (+0x20) and `m_FireHookTag` (+0x15c). When it is not,
   remove it, release its initiator count, and **if the group's `m_fMinTimeBetweenFiring` > 0.0001, lock the
   list and set `m_fGroupLockoutTimer = m_fMinTimeBetweenFiring`**. The lock therefore starts when the group's
   hook *ends*, and every ending resets the timer.

**Weight** (`FUN_00ca2580`), for a pending entry with owner pawn `o`:

    d = 0                      if o is the local player pawn (its own barks)
        |o.Location - player.Location|   otherwise
    w = (1 - clamp(d, 0, 5000) / 5000) + rand() * 0.2 / 32768     if d < 5000 or hook == 10 (KISMET_ACTIVATED)
    w = 0                                                              otherwise (never fires: weight < 0.0001)

(5000 = `m_fMaxDialogDistance`, 0.2 = `m_fHookWeightRandRange`.) A per-initiator cap > 0 sets `w = 0` for an
entry whose initiator already has `count + in-flight >= cap`, unless the owner's running hook has that same
initiator. Consequences: speakers farther than 5000 uu (straight line, not audio path) never bark except
Kismet-activated hooks; two simultaneous requests whose distances differ by >= 1000 uu are always ordered
nearest first; at 600 uu apart the nearer wins with probability 0.92 (triangular noise on +-0.2).

Test vector (same tick, group Agitated, cap 1 per list, both Loud): A at 600 uu (w = 0.88 + 0.2 u_A),
B at 1800 uu (w = 0.64 + 0.2 u_B): A is fired, B is dropped and records reason 5; no later retry. After A's
bark ends, Agitated is locked for 2.5 s.

### R4. Availability, priority, interruption (0x00c9d1f0, 0x00c9aec0, 0x00c9c9c0)

`Available(newPriority, flags)` on a speaker's component:
- no conversation component or the "interface" bit +0x4c clear: result 1 (cannot);
- the speaker's current state came from a hook whose flags include 0x4 (not interruptible): **5**;
- else `0` (available) if the speaker's current priority is **lower than** `newPriority`, or if `flags & 4 == 0`
  (the request is not a fresh start); else **4** (`DDA_IN_HIGHER_PRIORITY_CONVO`).
`FUN_00c9aec0` then turns any result except 5 into 0 when the *requesting hook* has flag 0x2 (interrupts).
Priorities: NOT_IN_A_CONV 0 (idle), LOW 1, MEDIUM 2, HIGH 3, NOT_INTERRUPTABLE 4. The priority of a running
state is its conversation's `m_Priority`, or, before a conversation node ran, the hook node's `m_Priority`
(default LOW).
- The **hook-time test** (R1 step 3) passes priority 4, flags 4: a speaker is available unless it is in a
  priority-4 conversation or a flag-0x4 hook (or the combination returns 5).
- The **bind-time test** (`FUN_00c9e2b0` -> `FUN_00c9c6c0`, every hook walk and every conversation start) passes
  the hook's/conversation's own priority and flag 4 when the state has no conversation yet. So a fresh bark (LOW)
  can start on a speaker only if the speaker is idle, or the bark's hook has flag 0x2, and a bark on a speaker
  already speaking (any LOW state) is refused (4). A conversation (HIGH default) takes a speaker from a bark
  (the bark's state ends, reason 6).
- Hooks 57-59 additionally end the previous state with reason 3 (death).

### R5. Hook node output selection (virtual +0x158 of each `DisConv_Hook_*`)

The walk starts at the hook node's output chosen by the node class; -1 = the hook cannot start (R1 step 3):

| Node class (Boyle Ext count) | Output index | Source |
|---|---|---|
| `DisConv_DialogHook` (97 in Boyle Ext, base) | 0 | |
| `Hook_SuspicionLevel` (23) | the owner's AI suspicion level byte (`brain+0x41`), 2 outputs | 0x00cac8b0 |
| `Hook_DeathMode` (4) | 0 = `DDMHO_Dead`, 1 = `DDMHO_KnockedOut` (from the dying pawn's state, vcall +0x18) | 0x00ca9070 |
| `Hook_Notice` (2) | 0 `DHN_Visual`; 1 `DHN_Aural_Anomaly`, 2 `DHN_Aural_Danger` (aural loudness class above 3 = danger); -1 for anything else | 0x00cac450 |
| `Hook_Investigate` (2) | the byte at `state+0x1c` (sub-state chosen by the behaviour), 3 outputs | 0x00ca9170 |
| `Hook_SuspicionDist` (2) | like Distance: nearest speaker-to-initiator distance `d`, fires `SuspicionLevel` output when `d <= m_fRadius` (500) and the previous distance was beyond | 0x00cb5b50 |
| `Hook_Distance` | close / medium / far bands (`CONVERSATIONS.md` section 4) | 0x00cb4ff0 |
| `Hook_PlayerLoiter` | radius 600, loiter time 5-7 s (`m_fLoiterTime_Min/Max`) | 0x00cb53c0 |
| `Hook_PlayerLookAt` | `m_LookAtTargets[]` (actor, angle, time), radius 300 | 0x00cb5680 |
| `Hook_KismetActivated` | the index of the Kismet hook name | 0x00ca9200 |
| `Hook_SplitByTypeAndActor` family (`HeartTargeted`, `NoticeBroken`, `WitnessedInteraction`) | by `m_AssociatedTypes` / `m_AssociatedActors` of the initiator | 0x00ca92e0 |
| `Hook_Distraction` | by `m_DistractionLinks[]` (actor, category) | 0x00cb0e80 |
| `Hook_WitnessedMagic` | filter `m_MagicTypes[]` (power component classes) | |
| `Hook_PlayAudioLog`, `Hook_Heart*` | | |

### R6. Who asks for which hook (bark selection)

Barks are asked from behaviours at specific sites, not from state transitions as such. The central caller is
`FUN_00af00f0(hook, initiator)` (88 call sites in 45 functions; it ends in R1). Hooks passed as constants
(decimal enum index): 12 PATROL_MISC_GROUP, 16, 17, 19, 21, 22, 23, 26, 27, 29, 30, 33, 34, 35, 36, 40, 41, 42,
47, 49, 50, 51, 64, 65, 66, 68, 69, 70, 71, 85, 86, 90, 94, 97, 99, 102, 104, 105, 107, 108, 109, 110, 111, 116;
passed from fields or computed (to be resolved per site by the AI porter): 0x00ad2460, 0x00af2ec0 (cVar4),
0x00af41b0 (field +0xa5 of the behaviour tweak), 0x00af4d20 (+0xb4), 0x00af7670, 0x00b3d5e0 (R6a), 0x00b42160
(R6b), 0x00bd0320, 0x00bd03a0, 0x00cf4840 (player-side). Others bypass it and call R1 directly:
- **Ambient barks** (`DisAISubProcessAmbientBarks` tick 0x00b3d5e0): while `m_fBarkInhibitionTimer > 0` count it down;
  otherwise (not LOD-disabled) look at nearby pawns (`FUN_00b14c00`), and when one is in AI state 1 and
  `FUN_00b2f210` (hook 12, the group variant) succeeds stop; else fire **hook 11 `DDH_PATROL_MISC`** every
  tick it is allowed (the refire override, 5-12 s per NPC, is what spaces it). The route-switch flag fires
  hook 15 `DDH_PATROL_SWITCH_ROUTE`.
- **Generic barks** (`DisAISubProcessGenericBark` tick 0x00b42160): timer `m_fBarkTimer`; at 0 fire
  `m_eBarkToPlay` (or `m_eBarkToPlayWhenGrouped` when a grouped ally exists, `FUN_00c47680`), then re-roll
  `min + rand * (max - min) / 32768`, or finish when `m_bIsOneOff`.
- **Pain** (pawn damage handlers): `FUN_00b733f0` fires **60 `DDH_COMBAT_OUCH_SMALL`**, `FUN_00b73630` and
  `FUN_00b736e0` fire **61 `DDH_COMBAT_OUCH_BIG`**; if the damage type's tweak (`FUN_00b72c40`, byte +0x84) names
  a hook other than 126 it replaces the default (this is where immolation, falling, etc. come from). The
  initiator is the damage causer's pawn. `FUN_00b73f60` fires 0x3d directly.
- Dying / backstabbed / stealth-kill (57-59) are the only hooks allowed on a dead owner (R1.2).
- Wolfhound/heart, audio logs, Kismet `DisSeqAct_DialogInputs`, player use and approach/loiter/look-at
  node polling feed hooks 1-10 and 118-121 (flags 0x4000 / 0x2000 in R9).

### R7. Subtitles, dominance and ducking (`DisConvGlobalMan.m_SubtitlesInfo`, 0x00c9e660, 0x00ca71c0, 0x00c99f80)

Per manager tick:
- **Dominant hook**: among all firing hooks (26 lists) pick the one with the highest *dominance* above 1e-8
  (first strictly greater wins). Dominance of a hook (0x00c99f80): the speaker interface flag word +0xbc: bit
  0x10 -> 0; bit 0x2 -> 1.0; bit 0x8 -> max over its audible speakers of
  `1 - pathDistance / (0.4 * voiceRadius)` (0.4 = `m_fDominantMaxDistance_Percent`), 0 beyond. The dominant
  index is stored in `m_CurDominantHook` and handed to `FUN_00c96cb0` (volume scaling of the rest:
  `m_fNonDominantVolumeScale` 0.3, blended at `m_fAudioDuckBlendSpeed` 5 per second) and `FUN_00c9e660`
  (subtitle owner).
- **Subtitle lists**: `m_MainSubtitles` and `m_SecondarySubtitles` (`DisSubtitle`: owner component, speakers
  `{AkComponent, playingID}`, text, `m_fVoiceRadius`, shown bit, `m_TextPartsRatio[]`, shown part). Every tick
  each entry is dropped when its owner's health is <= 0 (+0x100 of the owner actor), or none of its playing IDs
  plays any more. The main subtitle shown is the one with the highest `1 - pathDistance / voiceRadius` (> 0;
  entries beyond their voice radius are not shown), 99 for the one belonging to the dominant hook; the secondary
  list picks the same way independently. A subtitle with several text parts advances its part when
  `elapsed / duration` passes the next ratio. A blurb's `m_bMainSubtitle` (default true; barks are authored
  false) selects the list. `DisConv_Blurb` adds a subtitle only for a playing ID with non-empty text
  (`CONVERSATIONS.md` section 3).

### R8. Conversation graph, conditions, lines (summary; details in `CONVERSATIONS.md`)

Node `Evaluate` returns the output index or -1 (wait). Conditions: output 0 false / 1 true. Line length is
the VO clip plus `m_fBlurbEndDelay`; no clip, no subtitle and no wait. Choices have no timeout.
**`DisConv_CompareDarknessLevel` (0x00ca8fe0)**: with `L` = the chaos level (`FUN_00c487d0`: the highest index
`i` with `threshold[i] <= score`, else -1) and `m_LevelToCompare` = T: output 0 if `L == T`, else
`(T <= L) + 1` i.e. 1 when the level is lower, 2 when higher. **Verified identical** in
`runner.rs` (`CompareDarknessLevel`) and `dishonored_sim::darkness::level_of`.

### R9. The hook table and the tweak object

126 rows of 0x50 bytes at `DAT_01471d0c`, built by `FUN_00c8d820`: group +0x34, min +0x38, max +0x3c,
global +0x40, bust time +0x44, flags +0x48 (0x2 interrupts, 0x4 not interruptible, 0x8 battle sound, 0x20 search
sound, 0x1000 notice sound, 0x2000 radial, 0x4000 player used, 0x80 AI hook). `Default__DisTweaks_ConvSettings`
(the only tweak data; there is no `Twk_ConvSettings` object in the packages): overrides DDH_SEARCH and
SEARCH_GROUP 8-12 s group Agitated; FLEEING 2-5 s Combat; COMBAT_THREAT and _GROUP 5-13 s Threat;
PLAY_AUDIO_LOG -1..0 GameplayImportant; PATROL_MISC 5-12 s and PATROL_MISC_GROUP 20-60 s Casual; fire group caps
(per group / per initiator, Loud = Quiet in all 13): Casual 3/1 3 s, CasualStateChanges 3/1 12 s, Attention 2/2
0.2 s, Exertions 2/-1, Combat 5/2, Threat 1/1, Agitated 1/1 2.5 s, WitnessedMagic 3/3 3 s, GameplayImportant -1/-1
ignore-global, Reaction 1/1, SoundAlarm 2/1, CombatCries 2/2 2 s, CombatImportant 2/2 1.5 s.

---------------------------------------------------------------------------------------------------

## 2. Cutscene (matinee / soiree) rules, as far as decoded

The data model, track evaluation and group runtime are in `CUTSCENES.md` and are not repeated. Rules that matter
for control:

- **C1. Player lock.** `InterpGroupInstPlayer` init saves the start transform and enters
  `StatePlayerMasterSoiree` (enter 0x00a66ff0): input masked by `FUN_00ab1160(0x8002, 0, 9)` (skipped when the
  state flags have bit 0x20), both hands' item weights blended to 1, `PHYS_Interpolating` (7) with
  `m_bDisablePlayerCollision`, the first-person rig follows the `camera_jnt` of the group's full-body animation.
  Exit restores everything (0x00a628f0). Looking is limited by `DisInterpTrackAllowPlayerLook` keys (yaw +-170,
  pitch -15..60 in Boyle). **Unverified**: which input actions the mask `0x8002` covers (movement and jump are
  expected; look is known free) - capture C4.
- **C2. AI lock.** `LockRequests` (default true) locks the AI request channels `Gesture`, `Posture`,
  `Distraction`, `Face`; the pawn's group instance registers on the pawn; "in soiree" =
  `FUN_00b4b4c0` (pawn `+0xc6c > 0` and `+0x344 > 0` and flag `+0x964 & 0x8000` clear), tested at 23 sites in
  AI and pawn code (hit reactions `0x00a77d10`, perception, behaviours).
- **C3. Dialog keys.** Blurb start, random pick among `m_AltBlurbsGUID`, `WaitForEndOfBlurb` hold, as in
  `CUTSCENES.md`; the matinee's blurbs use the same line machinery (R7 subtitles, R8 timing).
- **C4. Soiree node.** `DisConv_Soiree` plays the matinee found by GUID and waits for it (output 0 after);
  `m_UsePreviewBlurbsOnly` plays the preview chain from output 1 instead; skipped in conversations that are not
  `m_bIsBlocking`.
- **C5. Skipping.** The only skip-related reference in the exe is the HUD movie clip name
  `_root.skipCutScene_mc` registered at HUD init (0x00b95650); no native, script function or matinee flag reads a
  skip input. **Unverified** that skipping does not exist (capture C4 presses every key).

---------------------------------------------------------------------------------------------------

## 3. What the port does, rule by rule

| Rule | Port | Verdict |
|---|---|---|
| R1.1 supported cache | `fire_hook` tries every tree of the request and ends `NoContent` | equivalent result |
| R1.2 dead owner | not checked in `fire_hook`; AI crate does not emit hooks 57-59 | **missing** (F9) |
| R1.4 refire gate | `HookClock::allowed`: `now < next_allowed`, `next = now + u*(max-min)` (`hooks.rs:425,442`) | **bug F1** (min dropped) |
| R1.5 global refire | `refire_global > 0` test, always false | equivalent |
| R1.5 stamp on success only | `fired()` called when the start produced a line or conversation | equivalent |
| R2 tree order | `HookRequest.trees` in order one-shot, then voice trees | order matches; fallback chain missing (F6) |
| R3 queue / weights / Loud-Quiet / drop | none: first asker fires if group cap and flag-0x80 count allow | **missing** (F3, F4) |
| R3 group lock | `last_group` set at fire time (`hooks.rs:447`) | **bug F2** (lock from start, not end) |
| R3 caps | `fire_hook` counts running instances per group (`runner.rs:418`) and flag-0x80 hooks against 5 | partly (F3) |
| R4 availability for barks | `enter_conversation` only; conversation-less hook walks never check speakers | **bug/missing F7** |
| R5 output selection | `Hook{..}` always entered at `req.output` (0); `load.rs:502` flattens every `DisConv_Hook_*` | **missing F5** |
| R6 call sites | transition barks (`dishonored_ai::transition_barks`) | partial (F9) |
| R7 subtitles | `hud_dialogue::current_line`: newest main line, else newest line | **bug F10** |
| R7 dominance / ducking | none | **missing F11** |
| R8 graph | `runner.rs` `eval` | matches `CONVERSATIONS.md`; CompareDarknessLevel verified |
| R9 table | 124 of the 125 defined rows identical; row 110 flags differ (F8) | verified by script |
| Player hooks 4-9 | only `DDH_PLAYER_APPROACHES(_RADIAL)` (`approach_hooks`), "aware" ignored | **missing F12** |
| C1-C5 | `kismet_host/cutscene.rs`, `interp.rs`, `runtime/cutscene.rs` | lock modelled; flags unused (F16) |

The hook table check was mechanical: `FUN_00c8d820` was parsed (97 rows set by field stores, 28 by the helper
`FUN_00c8bce0`) and compared field by field (group, min, max, global, flags, bust time) with `hooks.rs::TABLE`.

---------------------------------------------------------------------------------------------------

## 4. Findings (for the porting agents)

Class: PB port bug, PM port missing, OL oracle limitation, KD known difference.

**F1 (PB). The refire gate adds the minimum.**
Evidence: gate in `FUN_00ca6bd0` (R1.4), stamp in `FUN_00ca5670`. Port: `hooks.rs:425-448`, `fired()` stores
`now + unit*(max-min)`, `allowed()` compares `now < t`. Effect: every default AI hook (min = max = 1) refires with
**no** delay in the port (original 1.0 s); the overrides are 0..7 s instead of 5..12 s (PATROL_MISC), 0..4 instead
of 8..12 (SEARCH), 0..8 instead of 5..13 (COMBAT_THREAT), 0..3 instead of 2..5 (FLEEING).
Fix: `allowed(npc, hook, now)`: `let t = next_allowed.get(..)`; reject iff `t` exists, `t >= 1e-8` and
`!(info.refire_min < (now - t) + 1e-8)`. Keep `fired()` as is, with `unit = rand()/32768` (not `/32767`).
The unit test at `tests/runner.rs:544-550` encodes the wrong rule and must be rewritten with the R1 test vector.

**F2 (PB). The fire group's spacing lock starts when the group's hook ends.**
Evidence: R3 tick step 4. Port: `HookClock.last_group[group] = now` at fire time, rejected while
`now - last < min_time_between`. Effect: for a 3 s Casual bark the next Casual bark may start at 3 s after the
start in the port, 3 s after the bark *ended* (about 6 s in) in the original; Agitated 2.5 s, CombatCries 2 s,
CombatImportant 1.5 s, CasualStateChanges 12 s, WitnessedMagic 3 s.
Fix: store `lock_until[group] = None` while a hook of the group is running; when the last running hook of that
group (per Loud/Quiet list, F4) ends, set `lock_until = now + min_time_between` (reset on every ending, only if the
value > 0.0001); `allowed` rejects while `now < lock_until`.

**F3 (PM). No request queue, no weight selection, no group priority order.**
Evidence: R3. Port: `Runner::fire_hook` fires on the spot in the order the AI asked. Fix, in
`dishonored_conversation`: add `HookQueue` with 13 x 2 lists mirroring R3 (pending, firing, locked, timer). The
mission host calls `request_hook(...)` (R1 steps 1-5, enqueue) from `fire_ai_hooks`; `Runner::update` (or a new
`tick_hooks(dt, env)`) runs the R3 tick once per step: re-classify Loud/Quiet, slots 5/5, groups 12 -> 0, weights
`1 - clamp(d,0,5000)/5000 + rand/32768*0.2` (d from `ConvEnv`, 0 for the player), sort descending, fire until
group room or weight < 0.0001, count attempts for group room and successes for global slots, drop the rest,
empty pending. `ConvEnv` needs `distance_to_listener(npc)` and `audio_path_distance(npc)` (F4). The cap counts
every running hook of a non-ignoring group, not only flag-0x80 ones (port today: `runner.rs:422-424`).

**F4 (PM). Loud / Quiet split and distance cull.**
Evidence: R3 (`FUN_00c9a300`, `FUN_00c96920`). A speaker whose audio path distance to the listener is >= 2000 uu
uses its own list and its own 5 global slots; a speaker farther than 5000 uu (straight line) never barks.
Fix: `ConvEnv::audio_path_distance` first as the straight line with the port's audio occlusion
(`dishonored_audio` portal/cell distance if available; the straight line is an APPROXIMATION to mark).

**F5 (PM). Hook output selection by node class.**
Evidence: R5. Port: `load.rs:502` maps every `DisConv_Hook_*` to `NodeKind::Hook` (only `Distance` keeps its
edges) and `fire_ai_hooks` passes `output: 0`. Boyle Ext alone has 23 `Hook_SuspicionLevel`, 4 `DeathMode`,
2 `Notice`, 2 `Investigate`, 2 `SuspicionDist` nodes, so those barks always take output 0. Fix: keep the class on
the node (`HookClass` enum with the fields of R5), add `ConvEnv::hook_output(npc, class, initiator)`
(suspicion byte from the AI brain, dying mode dead/knocked out, noise class visual/anomaly/danger,
investigate sub-state), return -1 = no start. `HookRequest.output` becomes an `Option` filled by the env.

**F6 (PM). `m_pFallbackTree` chain and `DisConv_TryFallbackTree`.**
Evidence: R2; `DisDialogTree.m_pFallbackTree` +0x148 is never read (`grep Fallback` finds only the
`TryFallbackTree` node, which ends the walk as finished at `runner.rs:1007`). Fix: load `m_pFallbackTree` into
`Tree`, extend `HookRequest.trees` by following the chain when a tree lacks the hook, and make the node set a
`continue_fallback` flag that makes `fire_hook` try the next tree instead of ending.

**F7 (PB/PM, LIKELY). Bark walks do not check or take their speaker.**
Evidence: R2/R4: every hook walk binds ME through `FUN_00c9e2b0` and `Available`. Port: only
`enter_conversation` does, and it ignores other instances without a `conversation` (`runner.rs:529`,
`o.conversation.is_some()`). Effects: one NPC can run two barks at once; a conversation does not end a bark in
progress; the not-interruptible flag (0x4) never protects a hook-only state (dying 0x8e, 0x86 stealth kills,
0x300 audio log/heart hooks). Fix: give hook-only instances a speaker key (the bound NPC) and apply the R4 rule
at start: reject (HookRejection::Busy) when the speaker is in an instance whose hook has flag 0x4, or when the
new priority (hook node priority, default LOW) is not strictly greater than the speaker's current instance
priority and the new hook lacks flag 0x2; end the displaced instance with `EndReason::NewConvo`
(`DeathReason` for hooks 57-59). Verify with capture C1/C2 before relying on it.

**F8 (PB, minor). Hook 110 `DDH_ATTENTION_INVESTIGATE` flags are 0xa0.**
`hooks.rs::TABLE` has `(110, 2, 1.0, 1.0, 0x80)`; the row (`FUN_00c8bce0` call right after the `< 0x6f` check)
has 0xa0 (search sound: the blurb start emits an AI noise with the search loudness). Fix: set 0xa0 and make the
host act on 0x20/0x1000/0x8 (noise at blurb start with `m_SearchLoudness` 2 / `m_NoticeLoudness` 1 /
`m_BattleLoudness` 3, `FUN_00c9bb50`) if it does not already.

**F9 (PM). Bark call sites the AI does not have.**
Evidence: R6. The port emits barks from `transition_barks` (state changes) plus a few action sites. Missing:
ambient `DDH_PATROL_MISC` (11) / `_GROUP` (12) loop and `DDH_PATROL_SWITCH_ROUTE` (15) (ambient sub-process),
`GenericBark` timer barks, `DDH_PATROL_PROXIMITY_WARNING` (17), `_PERSONAL_SPACE(_WARNING)` (18, 19),
`DDH_COMBAT_HELP` (47), `_GOING_HOME` (65), `DDH_ROBBED` (71), `DDH_PANIC_NOWHERE_TO_HIDE` (86),
`DDH_UNREACH_*` (90 and its group), `DDH_WEEP_*` (94, 97, ...), the Overseer `DDH_COMBAT_OVS_*` (99, 102, 104,
105), `DDH_ATTENTION_HEADTRACK` (107) and `_TRACKTARGET_BEGIN` (111), `DDH_ESCAPE_IMPENDING_EXPLOSION` (116),
`DDH_COMBAT_OUCH_BIG` (61) and the damage-type hook override of pain (R6), the dying family 57-59 (and the
dead-owner exemption R1.2), `DDH_NOTICE_*` witnessed-activity hooks, `DDH_FALLING`. The port also fires
`DDH_COMBAT_OUCH_SMALL` and `DDH_PATROL_ATTACKED_BY_ENEMY` as alternatives; the original fires 60 from the
damage handler and 16 from `FUN_00afc3e0` (both can fire). The AI porter should resolve the field-driven sites
listed in R6 and add them behind `AiEvent::Bark`. `DDH_MAGIC_WITNESSED` (69): matches (0x00af2800).

**F10 (PB/PM). Subtitles ignore voice radius, owner death, the secondary slot and text parts.**
Evidence: R7. `Line.voice_radius` is loaded (`load.rs:452`) and never used; `hud_dialogue::current_line` shows
the newest main line anywhere, falling back to non-main lines. Fix: `MissionView.dialogue` carries per line
speaker position, voice radius, owner alive and (for multi-part text) the part ratios; the HUD computes
`1 - dist/radius` (use the straight line until audio path distance exists), shows the best positive one, with
the dominant hook's line preferred (weight 99), a separate secondary field for non-main lines, drops lines whose
owner died or whose voice stopped. The multi-part text split needs the `m_TextPartsRatio` source (set by the
blurb text splitter; Unverified).

**F11 (PM). Dominant hook and dialogue ducking.**
Evidence: R7, `m_fNonDominantVolumeScale` 0.3, `m_fDominantMaxDistance_Percent` 0.4, `m_fAudioDuckBlendSpeed` 5.
Port: none (`dialogue_vo.rs` plays every clip at its attenuation). Fix: compute dominance per R7, scale the
other dialogue voices by 0.3, blended at 5 per second (presentation only).

**F12 (PM). Player-proximity hooks.**
`DDH_PLAYER_LOITERS(_RADIAL)` (4, 5: radius 600, 5-7 s), `DDH_PLAYER_LOOKAT(_RADIAL)` (6, 7: radius 300 and the
target list), `DDH_FRIEND_APPROACHES` (8), `DDH_NEUTRAL_APPROACHES` (9) are never polled; the "aware" approach
variants (2) ignore the NPC's attention rule (`runtime.rs:499` comment). Fix: poll them like `approach_hooks`
with the node fields of R5; "aware" = the NPC's attention target is the initiator (`ConvEnv`/AI state).

**F13 (KD, low). `DisConv_NonWord` main-subtitle default.** `load.rs:451` defaults `m_bMainSubtitle` to false for
NonWord; `Default__DisConv_NonWord` carries no override, so it inherits true. No visible effect while NonWord lines
have no text.

**F14 (KD). Random draws.** The original uses the C runtime `rand()/32768` everywhere (branches use
`rand()*100/32767`, per `CONVERSATIONS.md`); the port's `crand()` (`runner.rs:324`) and `fired()`'s
`crand/32767.0` give [0, 1] instead of [0, 1). Match the divisor per call site (hook refire and weights: 32768).
Original values are not reproducible (seeded by the process); compare distributions.

**F15 (verified OK). `DisConv_CompareDarknessLevel`, the 124 matching hook rows, the override/fire-group tables
(group enum order checked with `enum_dump`), the tree search order, line timing/VO rule, priority enum order.**

**F16 (PM, data). Unused matinee group flags.** `can_interrupt` (`CanInterruptMatinee`), `bDisableHitReact`,
`bShouldFakeDeath`, `bDisableJiggleBones`, `LookatRequestsPriority`, `camera_to_dialog`, `lock_requests` are loaded
(`interp.rs:1407-1417`) but only `lock_requests`/`snap` are acted on. What `CanInterruptMatinee` does in the original
is not decoded (its only consumers are not found by name; the "in soiree" check C2 is the likely reader):
**Unverified**; do not implement from the name.

**F17 (Unverified). Player input lock set and release timing.** See C1; capture `cutscene/boyle_intro_lock`.

**F18 (OL). The oracle's port runner records no conversation events.** `tools/oracle/src/runner.rs` runs the
mission (`MissionState.conv_log` holds `ConvEvent::Line/Started/Ended/Output`), but no event reaches the trace,
so no checkpoint on barks or lines can be judged on the port side. Fix for the oracle porter: after each step
drain `conv_log` into `TraceEvent`s `Sound:<vo.event>@<npc id>` (event name `Play_<32 hex>`, the same shape as the
hook's AkEvent post; detail `line=<id> dur=<s>`), `Conv:Started`, `Conv:Ended`, and `Bark:<hook>@<npc>` with
detail `result=Fired|Rejected:<reason>` for every `fire_ai_hooks` request, behind `Record::Sounds` /
`Record::SimEvents`. `Record::ProcessEvent(["AkComponent"...])` is the original-side counterpart.

---------------------------------------------------------------------------------------------------

## 5. Scenarios

Files under `fixtures/oracle/scenarios/` (they run headless on the port; checkpoints are `Unmeasured` /
`Unverified` until captures and F18 exist):

| Scenario | Rule | Observable |
|---|---|---|
| `conversation/bark_notice_detect600` | R1, R3, R6 (notice, search, refire) | AkEvent post ticks on the party guard next to the attention level channel (the `stealth/detect_600` setup, 480 ticks); then the session reads of the NPC's per-hook table |
| `conversation/bark_group_arbitration` | R3 (group caps, Loud/Quiet, weights, lock) | six guards at 600-3600 uu hear one shot: order, count and overlap of posted barks |
| `cutscene/boyle_intro_lock` | C1, C3, C5 | pawn path and camera under scripted input during Samuel's intro; the dialog-key AkEvents at 5.68 / 16.90 / 22.65 s; release near 29.64 s |

Parameter sweeps (the formulas are functions of distance/time, so one capture each covers the range):
- weight vs distance: the arbitration scenario already puts the speakers at 600 .. 3600 uu in 600 uu steps; the
  ordering test is "nearer wins when the gap is >= 1000 uu" (guards 600 vs 1800, 1200 vs 2400, 1800 vs 3000, ...).
- refire vs hook: read `m_DialogHookInfo_InGame[h]` of every NPC after the runs (queue lines) and compare
  `next_h - fire_time` against `[0, max-min]` per hook, and the observed gap between two barks of one hook on one
  NPC against `min + u*(max-min)` (the detect600 run produces repeated SEARCH/NOTICE barks when extended to
  1200 ticks; extend it if the first capture shows fewer than three same-hook barks).
- Port-only unit tests the porter should add from the test vectors in R1 and R3 (no original needed).

## 6. Capture requests (all in `fixtures/oracle/capture_queue.txt`, section "conversations, barks, cutscenes")

- C1 `conversation/bark_notice_detect600` + `session dump class=DisConversationComponent filter=m_DialogHookInfo_InGame`,
  `session dump class=DisConvGlobalMan filter=m_DialogHookInfo_InGame`, world time: validates R1.4 (the stamp and,
  with two barks of the same hook, the added minimum), the Notice/Search bark ticks and the per-hook tables.
- C2 `conversation/bark_group_arbitration` + `session dump class=DisConvGlobalMan filter=m_QueuedHookGroups` and
  `m_CurDominantHook`: validates R3 (order, caps, lock) and the dominant hook.
- C3 `session find/dump class=DisTweaks_ConvSettings` and `m_pConvSettingTweaks`: whether the running manager
  really uses the cooked class defaults of R9 (the port assumes it).
- C4 `cutscene/boyle_intro_lock` + `session dump class=StatePlayerMasterSoiree`: R-C1 lock, release time, camera
  limits, and (because the script presses every common key) whether any skip input exists.
- Asked of the hook agent: AkEvent post tracing with object, event name and tick (`ProcessEvent` on `AkComponent` or
  the native post wrapper), recorded as `Sound:<event>@<object>`.

## 7. Known gaps in this audit

- The audio path distance (`FUN_00c96920`) and its occlusion model were read only at the interface level; the port
  needs the straight-line stand-in until it is decoded (U-number to be assigned).
- Hook sites with computed hook ids (R6 list) and the `DisConv_Hook_SplitByTypeAndActor` / `Distraction` selectors
  were identified but not decoded in detail.
- The speaker-bind path `FUN_00c9e2b0` / `FUN_00c9c6c0` for hook-only walks (F7) is read from call structure, not
  line by line: marked LIKELY until C2 shows overlapping/interrupted barks on one NPC.
- Subtitle part splitting (`m_TextPartsRatio`) and `FUN_00c96cb0` (the exact ducking maths) are not decoded.

---------------------------------------------------------------------------------------------------

## 8. Port status (2026-10-08, conversation port pass)

Corrections found while re-reading the cited functions with the static index:
- **Random divisor.** The refire stamp (`FUN_00ca5670`) and the bark weight (`FUN_00ca2580`) both multiply
  `rand()` by `3.051851e-05` = 1/32767, not 1/32768 (R1, R3, F1, F14 say 32768). The port keeps 32767.
- **R3 tick step 1** re-classifies the *firing* entries (the lists at +0x84, the ones the housekeeping checks),
  moving them between Loud and Quiet with their initiator counts; pending entries are classified at enqueue only.
- **R3 per-initiator weight:** the first pending entry of an initiator in a tick is never zeroed (the tally map
  gets it with count 1 unchecked); later ones are zeroed when `count + in-flight >= cap` unless the owner's
  running hook has that initiator.
- **R1 trial walk** (`FUN_00ca4690`): the first tree whose fallback chain holds the hook decides; the output must
  also be below the node's output count, else the hook cannot start.
- **R6:** `FUN_00a77d10` fires hook 62 (`DDH_COMBAT_IMMOLATED`) or the damage type's override, not a dying hook;
  `FUN_00b73630` fires 61. The sites of hooks 57-59 were not found among the direct `FireHook` callers.
- **Ambient barks:** activation (vslot 75, 0x00b47020) clears the flags and sets the inhibition timer to the tweak
  (+0x8c, `m_fBarkInhibitionDuration` 12 s); the tick (0x00b3d5e0) counts down, then asks hook 11 every tick (or
  hook 12 through `FUN_00b2f210` when a nearby pawn in AI state 1 qualifies, hook 15 when a route switch is
  flagged). The sub-process exists only in the Patrol and Guard behaviour tweaks.
- **Player hooks:** `Hook_PlayerLoiter` (0x00cb53c0) and `Hook_PlayerLookAt` (0x00cb5680) decoded (see
  `hooks.rs::LoiterHook` / `LookAtHook`): 2D distances, a gap above 0.5 s between polls resets the loiter, the
  loiter timer is `min + rand * (max - min) / 32767`, a look-at target fires its output once after its time.
- **Hook_Notice** reads the attention change reason of the owner's stimulus for the initiator: 19 (seen) -> 0,
  7 (heard) -> 1 + (context > 3).

| Finding | Status | Port |
|---|---|---|
| F1 refire gate | ported | `hooks.rs::HookClock::allowed` (`next < 1e-8 \|\| min < now - next + 1e-8`), `fired` unchanged (1/32767); test `hook_table_and_refire` rewritten with the R1 vector |
| F2 group lock at the end | ported | `Runner::tick_hooks` housekeeping locks the list for `m_fMinTimeBetweenFiring` when one of its hooks stops playing; enqueue rejects `GroupLocked` |
| F3 queue | ported | `Runner::request_hook` (R1 gates + enqueue), `Runner::tick_hooks` (groups 12 -> 0, Loud / Quiet, slots 5 + 5, weights, sort, room per attempt, drop the rest); `fire_hook` stays as the immediate form (`FUN_00ca73c0`); the mission requests the AI's barks and ticks the manager every step |
| F4 Loud / Quiet, cull | ported | `ConvEnv::audio_distance` (>= 2000 = Quiet) and `listener_distance` (weight 0 beyond 5000 but hook 10). APPROXIMATION: the audio path distance is the straight line to the nearest living player |
| F5 output by node class | ported | `HookClass` on hook nodes (loader), `Runner::hook_output`, `ConvEnv::hook_facts` (suspicion level, knocked out, notice kind; investigate sub-state 0: not decoded). `SplitByTypeAndActor`, `Distraction`, `WitnessedMagic`, `Heart*`: output 0 (not decoded) |
| F6 fallback trees | ported | `Tree.fallback` (`m_pFallbackTree`), the walk follows it when a tree lacks the hook or ends in `DisConv_TryFallbackTree`; the one-shot and generic categories end the search when a tree had the hook but started nothing |
| F7 bark speakers | ported (LIKELY rule) | hook-time test (priority 4) in `gate`, bind-time test with the hook node's priority in `fire_now`; a displaced state ends `NewConvo` (`Death` for 57-59); conversations now see and end hook-only states |
| F8 hook 110 flags | ported | 0xa0 (re-read in `FUN_00c8d820`). The blurb-start AI noise of flags 0x8 / 0x20 / 0x1000 is still not emitted (left open: the noise call `FUN_00c9bb50` and its position were not decoded) |
| F9 call sites | partly | ambient `DDH_PATROL_MISC` (patrol, guard behaviours, `m_fBarkInhibitionDuration` from the tweak), pain asks both 60 and 16; `Hook_Notice` facts. Left open: 12 / 15 (nearby-pawn query and route switch not decoded), GenericBark timers, the dying family 57-59 (sites not found), the damage-type hook override (no damage-type tweak in the AI), 17-19, 47, 61, 65, 71, 86, 90, 94-105, 107, 111, 116 (field-driven sites not resolved) |
| F10 subtitles | ported (runtime, not built here) | `DialogueLineView.voice_radius / instance / hook`; dead speakers' lines dropped by the server; `hud_dialogue::current_line` scores `1 - d / radius` (> 0), 99 for the dominant hook's line, main list first then secondary. Text parts left open (`m_TextPartsRatio` not decoded) |
| F11 dominance, ducking | ported (runtime, not built here) | `hud_dialogue::dominant_instance` (0.4 x voice radius), `dialogue_vo` scales the other hooks' voices to 0.3, blended at 5 / s (linear blend: APPROXIMATION, `FUN_00c96cb0` not decoded); NPC speakers taken as flag 0x8 |
| F12 player hooks | partly | loiter (4, 5) polled every step for one-shots whose me-speaker is a mission NPC; look-at (6, 7) state machine and data ported but `ConvEnv::look_angles` returns no angles (the view direction and target actors are not in the mission step). 8 / 9 and the "aware" rule of 2 / 4 / 6 left open (not decoded) |
| F18 oracle events | ported | `Bark:<hook>@<npc>` (result Queued / Fired / Rejected:<why>, per-tick refire rejections left out), `Conv:Started` / `Conv:Ended`, `Sound:Play_<hex>@<speaker>`; conversation scenarios load the map's dialog packages and give scenario NPCs the generic voice of their tweak |

Scenarios: `conversation/bark_notice_detect600` runs with dialog data but the port's party guard never leaves
Idle in 8 s (no bark; the brain finding of stealth/detect_600); with a hostile guard (scratch run) it posts
`DDH_ATTENTION_NOTICE_BEGIN` (Notice output 0) at tick 30 and `DDH_ALARM_GO_TO_RING_ALARM` at tick 73, each a
`Sound:Play_*@guard`. `conversation/bark_group_arbitration` runs on the Playground, which has no cooked dialog
packages and no generic voice for the Boyle tweak: no barks until the Playground's AI level lists voices.