# Lady Boyle's Last Party: how the original mission works, and a port plan

These findings come from the GOG Definitive Edition install, read with the tools listed below.
Text the game shows (subtitles, choices, objective and note text) is not copied here. Every
item is referenced by its package/object path, so the port can load it at runtime. Task labels
such as "T13 Esma/red" are my own shorthand for `Boyle_Objectives.BoyleAssassination.DishonoredTask_Custom_13`.
Node numbers in `[brackets]` are export indices in the named package. They are stable for this
install and are what `kismet_dump` and `task_flow` print.

Confidence tags follow `research/LOG.md`. **CONFIRMED** means read directly from data.
**LIKELY** means inferred from consistent data, not verified in the game. **UNKNOWN** means not
resolved.

## 0. Tools and evidence

Two research-only binaries were added. Both compile in the workspace, and their output stays in
`harness_out/boyle/`.

- **`tools/package_dump/src/bin/conv_dump.rs`**:
  - `props <upk> <path-suffix>...` prints exact exports and decodes name, string and object
    arrays.
  - `convs <upk>` prints every `DisDialogOneShot`/`DisDialogTree`/`DisConversation` with its
    node graph, and for each blurb its VO event, bank and GUID.
  - `table <upk>` prints one TSV row per conversation.
  - `vo <upk>` prints the blurb GUID -> event -> bank table.
  - `stats <upk>` prints counts.
  - `vocheck <upk>` checks that every VO event exists in its `Bk_*.INT` bank (FNV-1 over the
    HIRC event ids).
- **`tools/map_dump/src/bin/task_flow.rs`** follows remote events, dialog outputs and
  sub-sequences, and resolves `SeqVar_Named` names and `SF_*` story-flag GUIDs to names (read
  from `Startup.upk` `DisStoryFlagSet`):
  - `--roots` lists every task-control node and the root events that reach it.
  - `--story` lists every event with the effects it reaches and the branch path to each.
  - `--from N` walks backwards from a node; `--fwd N` walks forwards.

Raw dumps:
- `harness_out/boyle/k_*.txt` (`kismet_dump`)
- `convs_*.txt` and `ctable_*.tsv` (conversations)
- `story_*.txt` and `story_int_compact.txt` (event -> effect)
- `tr_*.txt` (task -> root events)
- `spawners.txt`, `doors.txt`, `travel.txt`
- `sf_ladyboyle.txt` (81 flag names)
- `vo/*.wav` (decoded VO samples)

## 1. Map flow

### Persistent maps and streaming levels (CONFIRMED)

| Persistent map | `LevelStreamingAlwaysLoaded` sublevels | Contents |
|---|---|---|
| `L_Boyle_Ext_P` | `L_Boyle_Ext_Audio`, `_Env`, `_Fx`, `_Light`, `_Nav`, `L_Boyle_Ext_Script`, `L_BoyleStreet_Ext_Script` | The street (Samuel drop-off, art dealer, doorman gate) and the estate grounds (garden, courtyard, Lord Shaw's duel yard, roof balcony, river grate) |
| `L_Boyle_Int_P` | `L_Boyle_Int_Audio`, `L_Boyle_Int_Basement`, `L_Boyle_Int_Stealth`, `_Env`, `_Fx`, `_Light`, `_Nav`, `L_Boyle_Int_Script` | The mansion: ballroom, foyer, upstairs bedrooms, cellar and boat tunnel. Brisby's boat is in the cellar |

Sublevel details:
- All sublevels are always loaded, and none are streamed by trigger. A level change swaps the
  persistent map.
- `L_Boyle_Int_Basement` holds geometry only (518 `StaticMeshComponent`s, no Kismet).
- `L_Boyle_Int_Stealth` holds two `DisStealthVolume`s. Each points to a
  `Boyle_Twk.Pvs_BoyleInteriorMainHall`/`Pvs_BoyleInteriorUpstairs` `DisTweaks_PlayerVisSettings`
  (light-value thresholds 0.35 and 0.2).
- Doors (`DisDoor`) live in the `*_Audio` sublevels: 8 in Ext, 18 in Int.

**The two maps share one coordinate frame (CONFIRMED).** Matching travel doors sit a few hundred
units apart:
- Ext `DishonoredUsableObject_7` is at (-2509,-1890,10). Int `DishonoredUsableObject_10` is at
  (-2163,-1855,10).
- Ext `PlayerStart_Garden` is at (-2663,-1879,98). Int `PlayerStart_Ballroom` is at
  (-2013,-1848,113).

This resolves the offset question in U-006: the interior can be rendered at its native
coordinates beside the exterior. Example: the interior party music emitters at (-226,-2049,469)
lie inside the exterior mansion shell.

### Travel mechanism (CONFIRMED)

1. A `DishonoredUsableObject` with `m_pUsableObjTweaks = Boyle_Twk.Door_Transitions.LTD_*` fires
   a `DisSeqEvent_Used`.
2. That opens a `DisSeqAct_DialogScriptedChoice`: a two-option prompt (go / cancel). Its option
   text is in the package and in `Localization/INT/L_Boyle_*_P.int`.
3. The "go" output posts `Snd_FixInt_AP_Door.Door_Metal_Level_Transition` and runs
   `DisSeqAct_SetPlayerTravelDestination(m_Tag = PlayerStart_*)`.
4. Ext_P: remote events `Turn off the Fireworks` -> `bulletproofkillinvite` ->
   `DisSeqCond_CheckStoryFlag(SF_Global_RealGameStarted)`.
5. True: `SeqAct_ActivateRemoteEvent("ChangeLvl_fromBoyleExterior_toBoyleInterior")` (Ext_P
   [5455]); Int_P [3410] -> [5158] is the reverse. False: a `SeqAct_ConsoleCommand` (a
   developer-only standalone path).
6. The `ChangeLvl_*` events are handled by the always-loaded global Kismet in
   `DishonoredGameFull_P`:
   - `SeqEvent_RemoteEvent` [1063]/[1064] start sequence
     `Main_Sequence.fromBoyleExterior_toBoyleInterior_0` [1228] or its reverse [1229].
   - That sequence runs `Changelevel_FadeOut` -> `DisSeqAct_SetStoryFlag(SF_Global …)` ->
     `SeqAct_PrepareMapChange(MainLevelName = L_Boyle_Int_P)` [842] ->
     **`DisSeqAct_SaveLevelState`** [131] -> `SeqAct_CommitMapChange` [751] ->
     `Changelevel_FadeIn`.
7. On arrival, `SeqEvent_LevelLoaded` outputs *Loaded and Visible* and *State Restored* ->
   `DisSeqAct_GotoPlayerTravelDestination` (Ext_P [5497] -> [3859], Int_P [3405]) put the
   player at the tagged `PlayerStart`.
8. Level state is saved per map and restored on return. This includes bodies, pickups taken and
   Kismet variables; several level-loaded events branch on *State Restored*. The port must
   persist per-map state across travel.

| From (use object -> tweak) | Ext_P choice node | Destination tag | Arrives at |
|---|---|---|---|
| Ext `DishonoredUsableObject_6` `LTD_Courtyard_to_Foyer` (1518,1420,10) | [3856] | `PlayerStart_Foyer` | Int `PlayerStart_4` (1715,1295,113) |
| Ext `_7` `LTD_Garden_to_Ballroom` (-2509,-1890,10) | [3855] | `PlayerStart_Ballroom` | Int `PlayerStart_3` (-2013,-1848,113) |
| Ext `_5` `LTD_Balcony_to_Bedroom` (-1620,-2020,1030) | [3857] (forbidden upstairs) | `PlayerStart_Bedroom` | Int `PlayerStart_5` (-1385,-1895,1113) |
| Ext `_8` `LTD_Sewer_to_Cellar` (-301,1070,-1633) | [3854] | `PlayerStart_BoatTunnel` | Int `PlayerStart_1` (-3511,-1760,-1290) |
| Ext Script front-door guard conversation (see section 5) | Ext Script [4242] -> remote `Get_The_Door` -> Ext_P [5503] | `PlayerStart_Foyer` | as above |
| Int `_13` `LTD_Foyer_to_Courtyard` (1472,1415,10) | Int_P [3403] | `PlayerStart_FrontDoors` | Ext `PlayerStart_3` (1355,1445,93) |
| Int `_10` `LTD_Ballroom_to_Garden` (-2163,-1855,10) | [3402] | `PlayerStart_Garden` | Ext `PlayerStart_1` (-2663,-1879,98) |
| Int `_12` `LTD_Bedroom_to_Balcony` (-1510,-1975,1010) | [3404] | `PlayerStart_RoofPatio` | Ext `PlayerStart_4` (-1775,-1940,1113) |
| Int `_11` `LTD_Cellar_to_Sewer` (-3580,-1840,-1390) | [3401] -> remote `check brisby` -> [3408] | `PlayerStart_RiverGrate` | Ext `PlayerStart_5` (-921,1160,-1835) |

Other entry and exit points:
- **Mission start:** Ext `PlayerStart_0`, tag `PlayerStart_SamuelDropOff`, at
  (-10092,11378,-2222), rotation yaw -20480.
- **Mission exit (CONFIRMED):** L_BoyleStreet_Ext_Script [13251]
  `BoyleCanal_PlayerIsReadyTogo` -> Ext_P [5499] -> `ChangeLvl_fromBoyle_toHub`. The next map
  is `L_Pub_Morning_P` with `L_Pub_FromBoyle_Script` (from `m_MissionsGame` MissionNumber 5 in
  config).

`DisTravelSpawner` (16 in Int_P, 16 in Ext Script) has `m_PlayerTravelLocationTags`
(`PlayerStart_Ballroom`/`_Foyer`), `m_bAwareOfPlayerUponStartup` and
`m_SpawnAtSuspicionLevel = DAISL_Suspecting`, with elite-guard tweaks. LIKELY: these spawn
pursuing guards at the arrival point when the player travels while hunted. They can be skipped
in a first port.

## 2. Objective flow

There are two `DishonoredObjective`s:
- `Boyle_Objectives.BoyleAssassination` (BA), 27 `DishonoredTask_Custom`.
- `Boyle_Objectives.PendeltonsNote` (PN), 3 tasks.

Both are exported in every Boyle script package.
- Text: `m_Description`/`m_Status` in the objects, localized in `Localization/<LANG>/Boyle_Objectives.<lang>`.
- HUD target names: `DisSeqAct_UpdateTaskTarget.m_TargetName`, localized in
  `L_Boyle_*_Script.int`.

Semantics (CONFIRMED from `DishonoredGame.upk`):
- The `EDisTaskState` enum order is `DTS_Inactive, DTS_Active, DTS_Completed, DTS_Failed`.
- `DisSeqAct_SetTaskState` without an `m_TaskState` property means **DTS_Inactive**. It is used
  to retire tasks.
- `DisSeqAct_SetTaskHidden` defaults to `m_bHidden = true`.

Node counts in L_Boyle_Int_Script: 4,228 nodes in 129 sequences, including 74 `SetTaskState`,
55 `SetTaskHidden`, 25 `UpdateTaskTarget`, 5 `DisSeqCond_TaskState`, 53 `SetStoryFlag` and 35
`CheckStoryFlag`.

### Tasks: setter nodes and triggering events

Unless prefixed, nodes are in L_Boyle_Int_Script `Main_Sequence.Boyle_Objectives`.
BS = L_BoyleStreet_Ext_Script, ES = L_Boyle_Ext_Script, E_P = L_Boyle_Ext_P.

| Task | Activate / show | Complete | Fail / retire | Trigger events |
|---|---|---|---|---|
| BA T0 ride with Samuel | set in the hub (previous mission) | — | hidden E_P [3869] on `LevelLoaded` [5498]; BS [7751] at exit | — |
| BA T12 get inside the estate | BS [7739] show + [7754] Active, with PN T0 [7740]/[7753] and `AddObjective` [7591]/[7592] | ES [4282] | BS [7751] | Activated by remote `Add Party Objective`, fired by the BS `DisDialogOneShot_7` (Samuel) conversation `Samuel_Intro` (`DisConv_KismetActivateRemoteEvent`; its soiree matinee is [13384]). Completed by ES `SeqEvent_Touch` [7032] `TriggerVolume_14` or [7033] `TriggerVolume_0` (estate entrance). Targets: ES [4299] `TargetPoint_1`, [4301] `TargetPoint_56`; BS [7799]/[7801] |
| BA T24 invitation (optional) | BS [7750]/[7765] or [7752]/[7767], target "Doorman" [7795]/[7796] or "Party Invitation" [7798] | BS [7763] on Touch [13568] `TriggerVolume_0` if flag `GaveInvitationtoGuard` | failed BS [7762] when the doorman pawn (`DishonoredSpawner_2`) is incapacitated [7865]; retired [7764] | Invitation pickup: BS `DisAbstractItemPickup_5` `PickupPickedUp` [7871] / `Used` `DisTrigger_9` [13589] (art dealer's invitation), or the wind-blown invitation `DisAbstractItemPickup_67` (Touch [13583], `Trigger_LOS_1` [13487]) |
| BA T1 identify the costume | ES [4276] show + [4283] Active on the estate-entrance touch; target [4298] `TargetPoint_15` | [8682] (remote `Remove Kill All 3 Objective`, when a sister's identity becomes known), [8689] (clues complete), also [8679]/[8688]/[8702]/[8714] | Inactive [8708]/[8724] + hidden [8648]/[8670] | — |
| BA T23 talk with guests (optional) | [8646] show + [8723] Active | [8721] (remote `Complete Talk to Guests Objective`) | hidden [8645]/[8647] | Activated on `LevelLoaded` [16533]. Completed by the guest gossip conversations' outputs `Upstairs Objective` (`DisSeqEvent_DialogOutputs` [8929] `DisDialogOneShot_27`, [8931] `_18`, [8932] `_16`, [8933] `_2`, [8934] `_7`) and by Miss White [8945] |
| BA T22 search the bedrooms (optional) | [8642] show + [8715] Active, targets [8795] `TargetPoint_22` "Upstairs", clue markers [8790] `TargetPoint_55` (Esma), [8789] `TargetPoint_60` (Waverly), [8801] `TargetPoint_58` (Lydia) | [8695] (remote `Find a clue Objective Complete`) | [8644]/[8716] | Activated by the same gossip outputs. Completed by any of the 9 clue notes being picked up (section 3) |
| BA T25 Miss White's drink (optional) | [8654] show + [8729] Active, targets [8794] `TargetPoint_108` (cider fountain), [8793] `DishonoredSpawner_20` (Miss White) | [8730] | [8728]/[8653] on alarm (`DisSeqEvent_AttentionIncreasedTo DAL_Busted` [8857]), Miss White's death (`NPCIncapacitated` [9034]) or the target's death | Activated by `DialogOutputs` [8918] (`DisDialogOneShot_9`, output `Quest For the Drink`). Completed by [8920] / remote `Complete Miss White Drink Task` after `DisSeqAct_RemoveAbstractItem(Boyle_AbstractInv_MS.boylepartydrink_AbsItm)` [8551] |
| BA T26 talk to Lord Brisby (optional) | [8650] show + [8725] Active, target [8808]/[8792] `DishonoredSpawner_9` | — | [8727]/[8651] (remote `Turn Off Brisby Quest`) | `DialogOutputs` [8941] (`DisDialogOneShot_6`, output `Join_Brisby_in_Private`) -> `AIGoToActor` [8331] to `TargetPoint_120` |
| BA T7/T8/T9 eliminate Esma/Lydia/Waverly (generic) | show [8667]/[8621]/[8665]; Active [8743]/[8720]/[8741] | [8694]/[8702]/[8711]/[8714] | retired when the dress is known: hidden [8632]/[8659]/[8664] + Inactive [8699]/[8735]/[8739] (also [8705]/[8706]/[8704]) | target [8807] Esma, [8806] Lydia, [8809] Waverly |
| BA T13–T15 Esma red/white/black | red [8628]+[8696] or [8624]+[8687]; white [8656]+[8732] or [8623]+[8684]; black [8629]+[8697] or [8671]+[8686] | as T7 (one multi-task node) | hidden [8639]/[8649] | Remote `Set New Objective Based on Clues` [16585] (dispatcher), or a direct clue-note pickup |
| BA T16–T18 Lydia red/white/black | [8658]+[8734] / [8657]+[8733] / [8640]+[8712] (and [8626]+[8693] / [8625]+[8692] / [8652]+[8683]) | as above | as above | as above |
| BA T19–T21 Waverly red/white/black | [8661]+[8737] / [8662]+[8738] / [8660]+[8736] (and [8633]+[8700] / [8635]+[8703] / [8634]+[8701]) | as above | as above | as above |
| BA T2/T3/T4 knock out Esma/Lydia/Waverly and deliver (nonlethal) | show [8668]/[8631]/[8666] + Active [8744]/[8698]/[8742] | [8680] (remote `CompleteMercyPath`) | failed [8677]/[8678]/[8713] (target killed), [8719]/[8718]/[8717] (wrong body delivered), [8745]/[8746]/[8681] (Brisby killed or panics), [8740]/[8690]/[8731] (Brisby leaves); retired [8710]/[8724]/[8727] | Path `GetBrisbyQuest` -> `Has KO Objective` -> `Set New Objective Based on Clues` -> branch `BrisbyKOobjective` (needs the guilty sister identified). Target [8791] `TargetPoint_164` "Boat in the Cellar" on `DisSeqEvent_KnockedOut` [9010–9012] / `PutToSleep` [9098–9100] |
| BA T11 process of elimination | initial `DTS_Active` hidden; [8643]/[8672] show | [8709]/[8691] | — | All three sisters killed without identification (third-kill output of `SeqAct_Switch` [16220]) |
| BA T6 escape to Samuel's boat | [8630] show + [8748] Active, target [8810] `TargetPoint_30` | — | BS [7766] Inactive + [7746] hidden | Remote `Boyle Objective Complete` (target dead and identified, or mercy path complete) |
| BA T10 Samuel has moved | BS [7745] + [7761], target [7800] | BS [7758] | — | Touch BS [13567] `TriggerVolume_23`. Completed by Touch [13555]/[13556]/[13570] (`DisTrigger_35`/`_0`/`_34`) |
| BA T5 talk to Pendleton | BS [7743] + [7760] (same touches as T10 completing) | in `L_Pub_FromBoyle_Script` | — | — |
| PN T0 deliver the note to Lord Shaw (optional) | BS [7740]/[7753]; targets ES [4300] `DishonoredSpawner_45` (Touch `DisTrigger_15`/`_30`), Int [8796] `TargetPoint_157` (gossip outputs `Shaw Target`) | ES [4289] when duel matinee [6773] completes | failed ES [4288]/[4292] (Shaw dies before or outside the duel), BS [7755]/[7759] at exit or after the canal jump | — |
| PN T2 follow Lord Shaw (duel) | ES [4278] + [4287] after [6773] | ES [4285] (`SeqEvent_LOS` [6944] / `Touch` [7027] "on mark" / gate [6762]) | ES [4291] (cheating with powers: `DisSeqEvent_PowerUsed` [4335]/[4336], leaving the area [7028], killing early [4329]); BS [7756]/[7757] | — |
| PN T1 return to the pub | ES [4279]+[4290], [4280]+[4286], [4281]+[4284] (after the duel's outcome) | `L_Pub_FromBoyle_Script` | — | — |

### Main lethal path (CONFIRMED except where marked)

1. **Arrival.** Ext `PlayerStart_SamuelDropOff`.
   - BS `DisSeqEvent_Spawned` [7889] (Samuel, `DishonoredSpawner_15`) -> DialogInputs "Boyle_Start"
     [7697] on `DisDialogOneShot_7`.
   - The `Samuel_Intro` conversation's soiree plays matinee [13384]. It fires `Add Party
     Objective` -> `AddObjective` BA and PN, T12 + PN T0 Active, and
     `ChapterNotes_twk.BOYLE.ChapterNotes_BOYLE` added.
2. **Street.** Get an invitation: the art dealer's (BS `DisAbstractItemPickup_5`) or the
   wind-blown one (`DisAbstractItemPickup_67`). Either sets flag `Boyle_Has_Invitation` and
   activates T24.
   - The doorman (BS `DisDialogOneShot_4`, Kismet name `BoyleGuard1`): his `DialogOutputs` [7846]
     -> `CheckStoryFlag(Boyle_Has_Invitation)`.
   - With the invitation: `DisSeqAct_DialogScriptedChoice` [7705] (hand over / not) ->
     `SetStoryFlag GaveInvitationtoGuard` [7736], conversation "Come In" [7699],
     `RemoveAbstractItem(Boyle_AbstractInv_MS.boyleinvitation_AbsItm)` [7713] and
     `StartSpawn` [7775].
   - Without it: "CantLetYouIn" / "NeedToSee" / "DontComeBack" (`SeqAct_Switch`).
   - Alternatives: the gate-house key (BS `DisKey_Base_0`/`_4` "Gate House Key"), the sewer gate,
     climbing.
3. **Grounds.** Touch ES `TriggerVolume_14`/`_0`: T12 Completed and T1 Active.
   - Optional: Lord Shaw duel (section 5).
   - Enter through the front-door elite (ES `DisDialogOneShot_5`, use -> `Get_The_Door` ->
     choice [4242]) or through any travel door (section 1).
4. **Interior setup** (on load; section 3): random positions, dress colours and guilty sister.
   T23 Active.
5. **Gather clues** (section 3). Every clue path ends in remote `Set New Objective Based on
   Clues` [16585], or a direct note-pickup branch. That shows the one dress task for the guilty
   sister (T13..T21), retires her generic T7/T8/T9 task, sets `Player Knows <Sister>` and the
   flag `PlayerKnows<Sister>`, and posts `DisSeqAct_ShowTargetNotification`
   (`Twk_ChapterInfoList.ChapterTargets.Twk_ChapterTarget_LadyBoyle`, portrait
   `UI_portraits_objectives.target_Boyle{Red,White,Black}`).
6. **Kill.** `SeqEvent_Death` [16523] (Esma), [16525] (Lydia) or [16526] (Waverly) on the
   sister's pawn. These are attached at runtime: remote `Create Death Events` ->
   `SeqAct_AttachToEvent` [15681]–[15690]. Section 3 has the decision logic. If the dead sister
   is guilty **and identified**:
   - Flags `Boyle_Return_To_Boat` and `SF_Global_PlyrKilledBoyle`.
   - All elimination tasks Completed ([8711]).
   - Remote `Boyle Objective Complete`: T6 Active (target `TargetPoint_30`), T22/T25 retired,
     `TargetNeutralized` notification with the dress portrait, autosave.
7. **Leave the mansion** by any door and travel to Ext.
8. **Street exit.** Touch BS `TriggerVolume_23` (T6 retired, T10 Active), then touch
   `DisTrigger_35`/`_0`/`_34` (T10 Completed, T5 Active, Samuel conversation "Lets_Go").
   - Its outputs reach remote `return to exit`: PN T0 failed if the note was not delivered,
     achievement check, `SeqAct_CameraFade` [13290], then `BoyleCanal_PlayerIsReadyTogo` ->
     `ChangeLvl_fromBoyle_toHub`.

### Nonlethal path, Lord Brisby (CONFIRMED except where marked)

1. **Meet Brisby.** Brisby is `DishonoredSpawner_9`, tweak `Twk_Pawn_Boyle_Party.Twk_Pawn_LordBrisby_NoCorpseReaction`,
   placed near (4697,-1627,15).
   - His conversation tree is `DisDialogOneShot_6`. `Lord_Brisby_Quest_Start` (`DisConversation_0`)
     is started by tree hooks (use/approach), not by Kismet (LIKELY: no `DialogInputs` targets
     it).
   - It fires `Join_Brisby_in_Private`. `DialogOutputs` [8941] -> Brisby walks to
     `TargetPoint_120` [8331] -> T26 Active -> patrol from `DishonoredNavPoint_208` -> idle
     matinee [15999] ("Brisby Window Trigger Active").
2. **The private talk.**
   - Conversation `Brisby_Boyle_Invite` (`DisConversation_47`, one soiree). LIKELY started by the
     tree, since hook name index 0 has no output.
   - Then one of `Esma_KO`/`Lydia_KO`/`Waverly KO` (`DisConversation_1495/1496/1497`), chosen by
     the `<Sister> Guilty` booleans. These are DialogInputs [8498] inputs 1–3 on hook
     `DisConv_Hook_KismetActivated_0` [5612].
   - Each KO conversation's `DisConv_Soiree` plays matinee [15980]/[15981]/[15982]
     ("GoToNearBoat"). Their *Started* output fires remote `GoToGetBrisbyQuest` -> `GetBrisbyQuest`
     [16553]:
     - `Has KO Objective` = true, flag `Boyle_BrisbyTalked`, autosave.
     - Sister KO events attached ([15686]/[15689]/[15690]).
     - `Brisby Go To Boat` (matinee [16000]): Brisby goes to the cellar boat.
   - If the player already knows the guilty sister's colour (`WhiteToldAllColors` or a known
     note), `Set New Objective Based on Clues` runs. Its `BrisbyKOobjective` branch shows T2, T3
     or T4 for the guilty sister.
3. **Knock out the guilty sister.** `DisSeqEvent_KnockedOut` [9010–9012] or `PutToSleep`
   [9098–9100] retarget T2–T4 to the cellar boat ([8791], `TargetPoint_164`).
4. **Carry her to the boat.** The Int map cellar holds the `L_Boyle_Int_Basement` geometry.
   - `SeqEvent_Touch` [16693] on `DisTrigger_23` (`bAllowDeadPawns`, `ProximityTweaks` 3) runs
     sub-sequence `Boyle_Objectives.TestBoyleBody_Touch` [16753]. It compares the touching body
     with the Named `Esma`/`Waverly`/`Lydia` objects ([16506]…), checks that she is identified,
     and checks dead or alive. Its result is *FinishedTouch_Test* -> `Boyle_TryToStarBrisbyConvo`
     [16608]:
     - Correct body, alive, Brisby at the boat: `Boyle_Plry&BodyRHere`.
     - Correct body, dead: `Player Killed Her` -> conversation "Killed_Right_Girl"; T2–T4 failed,
       elimination tasks completed.
     - Wrong body: `Wrong Girl` -> "Wrong_girl_one", then the T2–T4 failure branch.
   - `SeqEvent_Touch` [16708] on `TriggerVolume_10` ("Lady On Boat?", `bAllowDeadPawns`): with
     the lady on board, alive and Brisby alive, DialogInputs "Thanks" [8506]. `Touch` [16709]
     `TriggerVolume_8` ("Player On Boat?") drives "Get_off" and "Thanks".
5. **Exit cutscene.**
   - The `Thanks` conversation (`DisConversation_6`) has `DisConv_Soiree_7`
     (`m_SoireeGUID F6D22F90426A978BB95B31ACB2F4D0E5`). That GUID equals matinee [15985]'s
     `m_MatineeGUID` ("SoireeBrisbyThanks"; actors Brisby, `LADYINTHEBOAT` =
     `DishonoredSpawner_1` = `Twk_Pawn_LadyLydiaBoyle_INVINC`, boat mesh `InterpActor_7`).
   - Its event key `Close_gate` -> remote `CloseGate` -> gate matinee [16004] reversed ->
     `gateclosing` -> delay -> **`CompleteMercyPath`** [16614]:
     - T2–T4 Completed ([8680]) and hidden ([8641]); all elimination tasks Completed ([8702]).
     - Flags `BoyleMercyPathComplete` and `SF_Global_PlyrMercyBoyle`.
     - Remote `Boyle Objective Complete`: T6 Active.
   - Sub-sequence `RemoveCarriedBody` [16752] destroys the carried guilty sister's pawn. The
     scripted boat lady (`DishonoredSpawner_1`) stands in for her; `Spawned` [9104] starts
     DialogInputs "Excited".
6. **Failure cases.**
   - Brisby dies (`NPCIncapacitated` [9035]) or panics (`BehaviorStarted` [8859]–[8878]): T2–T4
     failed.
   - The alarm bell (`DisAlarmBell_0` `Used` [9170]) with Brisby not yet at the boat:
     `handle_brisby_disappear` -> `RemoveBrisbyQuests`.
   - `DisSeqEvent_NPCIncapacitated` [9036] on `DishonoredSpawner_9` -> `DisSeqAct_GameOver`
     [8522]. This node is disabled (`bEnabled=false`).
7. **Exit** as in the lethal path, steps 7–8.

## 3. The three sisters

**Pawns (L_Boyle_Int_Script, CONFIRMED):**

| Sister | Spawner | Tweak | `DisDialogOneShot` | VO bank |
|---|---|---|---|---|
| Esma | `DishonoredSpawner_2` | `Twk_Pawn_LadyEsmaBoyle.Twk_Pawn_LadyEsmaBoyle` | `_11` (nick "Esma") | `Bk_CA35DF0743EBCCD03ED835AAD4445477` |
| Lydia | `DishonoredSpawner_11` | `Twk_Pawn_LadyLydiaBoyle.Twk_Pawn_LadyLydiaBoyle` | `_10` | `Bk_1366FA854BF2DF62F875AB924F039C1C` |
| Waverly | `DishonoredSpawner_4` | `Twk_Pawn_LadyWaverlyBoyle.Twk_Pawn_LadyWaverlyBoyle` | `_12` | `Bk_056870EF43902F85B2CAFD900B591E67` |

- All three spawners sit upstairs near (1375,-730,1015) with `m_bPatrolUponStartup`.
- Shared barks come from `m_GenericDialogData = Dlg_Boyles.VoiceData.Voice_Boyles`.
- Kismet addresses them as `SeqVar_Named` `Esma`/`Lydia`/`Waverly`.

**Randomization (CONFIRMED)** runs once all three have spawned:
- Spawned [9103]/[9111]/[9112] -> counter `SeqAct_Switch` [16223] link 3 -> gate [15948]
  ("3 Pronged Gate Fired!").
- **Positions:** `SeqAct_RandomSwitch` [16099] (3 links), then a 2-link switch
  ([16106]/[16107]/[16101]). Six `SeqAct_Teleport`s put the sisters on a permutation of
  `Position 0/1/2` = `TargetPoint_10`, `TargetPoint_82`, `TargetPoint_140`.
- **Dresses:** `SeqAct_RandomSwitch` [16104] gives red to one sister. A 2-link switch
  ([16105]/[16102]/[16100]) gives white to one of the other two, and a `CompareBool` gives black
  to the third. Each step is a `DisSeqAct_NPCSetMaterials` with body
  `Npc_Boyle.Materials.Boyle_{red_inst, inst (white), black_inst}` and mask
  `Npc_Masks.BOYLE.BoyleMask_{red,white,black}_inst` (e.g. [8535] red, [8538] white, [8544]
  black). Each sets `SeqVar_Bool` `<Sister> is <Colour>`. Two story flags record Waverly's colour
  for the conversations: `Boyle_Waverly_Red` [8615] and `Boyle_Waverly_White` [8612].
- **Guilty sister:** independent of the dress.
  - `SeqAct_RandomSwitch` [16103] (`bAutoDisableLinks`, so no link repeats) fires twice through
    `Switch` [16217]. Each firing destroys one per-sister pair: `InterpActor_21`+`DisAbstractItemPickup_69`
    (W), `InterpActor_6`+`_67` (E) or `InterpActor_20`+`_68` (L). The pickups are paintings,
    e.g. `Money_twk.Paintings.Painting_Asymmetry_twk`. Each firing sets `WRing/ERing/LRing
    Destroyed`.
  - The surviving pair names the guilty sister: `Esma Guilty` [16115], `Waverly Guilty` [16114]
    or `Lydia Guilty` [16145].
  - Each branch also destroys two of three `DisKey_Base_7/_11/_12` (all named with a placeholder
    "skeleton key" string):
    - Esma guilty keeps `_11`.
    - Waverly guilty keeps `_12`.
    - Lydia guilty keeps `_7`.
  - Each branch fires remote `add fake paitings` and `Set <Sister> Proximity Trigger`.
  - Console events `esma_guilty`, `lydia_guilty` and `waverly_guilty` force a result.
  - No story flag from earlier missions affects the choice.

**Clue placement (CONFIRMED).** The guilty sister's note for her actual dress colour is teleported
into her bedroom: Esma -> `TargetPoint_55`, Waverly -> `TargetPoint_60`, Lydia ->
`TargetPoint_58`. This sets `Guilty Girl Is <Colour>`. Examples: [16234] `EsmaRedNote`,
[16238] `WaverlyWhiteNote`, [16240] `LydiaRedNote`. The notes are `DisAbstractItemPickupNote`
actors in L_Boyle_Int_Script with `m_pAbstractItem = WrittenNotes_twk.BoyleNotes.Boyle_<Sister>_<Colour>`:

| | Red | White | Black |
|---|---|---|---|
| Esma | `_14` | `_13` | `_10` |
| Lydia | `_17` | `_12` | `_15` |
| Waverly | `_0` | `_4` | `_3` |

**How the player learns the dress:**
1. **Bedroom note** (`PickupPickedUp` [9058]–[9081]). T22 completes, a flag is set, and the
   matching dress task is shown directly, e.g. [9080] "Esma Note Red" -> T13 Active and `Player
   Knows Esma`. Doors involved:
   - Esma's door `DisDoor_14` and Waverly's door `DisDoor_21` (L_Boyle_Int_Audio) list matching
     keys "Esma's Bedroom Key" and "Waverly's Bedroom Key". Waverly's key exists twice (Int `DisKey_Base_0`, `_10`); no key
     named for Esma exists in the Int packages (UNKNOWN: Kismet may lock or unlock it). Neither door is `m_bLocked` in data.
   - Lydia's bedroom door: no keyed door was identified for it (UNKNOWN).
2. **Miss White** (`DisDialogOneShot_9`, `DishonoredSpawner_20`, with Jack Ramsey `DishonoredSpawner_21`):
   - Conversation `Hits_On_Player` -> `Quest For the Drink` (T25).
   - The drink is `Boyle_AbstractInv_MS.boylepartydrink_AbsItm`, from the cider fountain at
     `TargetPoint_108`.
   - Handing it over runs sub-sequence `MissWhiteRevealsColors`. She names Waverly's colour
     (conversation `WaverlyRed`/`White`/`Black`, plus `ChapterNotes_twk.BOYLE.Wavery*`), then
     Lydia's (`GoCheckLydiaColor` -> `Lydia*`, plus `ChapterNotes_twk.BOYLE.Lydia*`).
   - Esma's colour follows by elimination. Flag `WhiteToldAllColors` makes `GetBrisbyQuest` and
     the `End W Conversation` path call `Set New Objective Based on Clues`.
3. **Guest gossip** (`DisDialogOneShot_2/7/16/18/27`) points upstairs (T22) and at Lord Shaw. It
   does not name the colour.
4. **The sisters' own conversations** (`Hunting <Sister>`, 6–8 `DisConv_PlayerChoice` each) send
   them to the bedroom, garden or distillery through Kismet outputs such as `Esma_To_Bedroom`,
   `Lydia_moves_to_garden` and `Waverly_Follows_Player`. LIKELY: they are for isolation, not
   identification.

**Kill detection (CONFIRMED).** `SeqEvent_Death` [16523]/[16525]/[16526] are attached to the
pawns at runtime through `SeqAct_AttachToEvent`. Each one:
- sets `<Sister> Dead`;
- if `<Sister> Guilty`: `Turn Off Brisby Quest`, `SF_Global_PlyrKilledBoyle`, `Guilty Girl Dead`
  (`GuiltyGirlisDead`), and the mercy tasks cleared. **Only if `Player Knows <Sister>`:**
  `Boyle_Return_To_Boat`, all elimination tasks Completed, and `Boyle Objective Complete`. If she
  is guilty but unidentified, the objective does not complete;
- increments a kill counter, `SeqAct_Switch` [16220]: link 2 sets `KilledatleastTwoBoyles`;
  link 3 sets `KilledAllThreeBoyleWomen` and, if not yet done, completes everything and T11
  "process of elimination";
- `test flee <sister>` -> the surviving sisters flee (`ChapterNotes_twk.BOYLE.HidingBoyles`,
  `DisTetherVolume_1`, `LockWaverlyDoor`);
- `resolve<Sister>KO Objective` -> fails that sister's KO task if it was active.

## 4. Conversations

### Data model (CONFIRMED)

**`DisDialogOneShot`** is a level actor:
- `m_pDialogTree` -> `DisDialogTree_OneShot`.
- `m_MeSpeaker` (`DisOneOffSpeaker`: `m_SpeakerNickName`, `m_pAttachedActor` = a
  `DishonoredSpawner`, `m_pVoiceData` -> `DisDialogVoiceData_OneShot`), plus extra speakers in
  the same form.
- Shared generic trees (`Dlg_AristocraticFemale`, `Dlg_EliteGuard`, …: `DisDialogTree` with
  `DisDialogVoiceData` variants `_A/_B/_C`) are picked through the pawn tweak's
  `m_GenericDialogData` (e.g. `Twk_Pawn_Boyle_AristoF1` -> `Voice_AristocraticFemale_A`).

**Tree-level nodes** (owned by the tree):
- `DisConv_DialogHook` with `m_HookEnum` `DDH_*`: `DDH_PLAYER_USED` (the player presses use on
  the NPC), `DDH_COMBAT_*`, `DDH_ALARM_*`, `DDH_NOTICE_*`, `DDH_PATROL_*`, …
- `DisConv_Hook_Distance` (`DDH_PLAYER_APPROACHES`, `DDH_NEUTRAL_APPROACHES`; three outputs,
  LIKELY distance bands).
- `DisConv_Hook_SuspicionLevel`/`_SuspicionDist`/`_Notice`/`_Investigate`/`_Distraction`/`_DeathMode`/`_WitnessedMagic`/`_WitnessedInteraction`.
- `DisConv_Hook_KismetActivated` (`DDH_KISMET_ACTIVATED`, `m_KismetDialogHooks` = a name list).
- `DisConv_ConversationRef` -> `DisConversation`.

**`DisConversation`:**
- `m_Speakers[]` with `m_Type` `ME_SPEAKER`/`INITIATOR_SPEAKER`/`PLAYER_SPEAKER`.
- `m_bIsBlocking`, `m_bFireConvOnlyOnce`, `m_Priority`.
- `m_Label`, used by `DisConv_SeenDialogLabel`.
- `m_Description` (a designer note).
- `m_OutputLinks` `Start`/`Interrupted`.

**Nodes:**
- `DisConv_Blurb` / `DisConv_NonWord` hold the line. Fields: `m_iBlurbGUID`, `m_iSpeaker` (index
  into the speakers), **`m_Text` (the English subtitle)**, `m_SpokenTo`, `m_LookType`,
  `m_fVoiceRadius` (4500 on shared barks), `m_bMainSubtitle`, `m_Label`.
- Branching: `DisConv_RandomBranch`, `SequentialBranch` (`m_bLooping`, `m_bApplyGlobally`),
  `RandomSequentialBranch`, `TimeLimit`.
- Conditions: `CheckStoryFlag`, `SetStoryFlag`, `IsPossessed`, `SpeakerInStoryGroup`,
  `SpeakerHasTweaks`, `CheckSpeakerSuspicionLevel`, `HasInventoryItem(Equipped)`,
  `SeenDialogLabel`, `IsSpeakerAvailable`, `TryFallbackTree`.
- Control and staging: `MeSpeaker`, `OneShotSpeaker` (`m_bNotWhileInCombat`,
  `m_bNotWhileFearful`, `m_bMustHaveLOS`), `InitiatorSpeaker`, `PlayerLookAtSpeaker` (camera
  constraints), `PlayerStopLookAt`.
- **`DisConv_PlayerChoice`**: `m_Choices_Static[].m_ChoiceText` (text in the package), one output
  per choice. All 26 in Int have two options.
- **`DisConv_KismetActivateRemoteEvent`** (`m_EventName`): fires the same-named output of the
  `DisSeqEvent_DialogOutputs` whose `Originator` is the owning one-shot (verified, e.g. `Set
  Senses`, `Get_The_Door`).
- **`DisConv_Soiree`** (`m_SoireeGUID`): plays the Kismet `SeqAct_Interp` whose `m_MatineeGUID`
  is equal. Verified for `Thanks` -> [15985] and `Esma_KO` -> [15980]. Matinees can carry
  `InterpTrackDialog` keys (`InterpTrackDialogKeyProperties.m_BlurbGUID`) and
  `InterpTrackSoireeControl` keys (loop pins).

**Kismet side:**
- `DisSeqAct_DialogInputs` has `m_pDialogOneShot` and named input links. Input *i* fires
  `DisConv_Hook_KismetActivated` output *i*, i.e. the conversation behind that hook name
  (verified on `DisDialogOneShot_6`: input 1 "Esma_KO" -> output 1 -> `ConversationRef_6` ->
  `DisConversation_1495`).
- `DisSeqAct_DialogScriptedChoice` is a two-option prompt outside any conversation (travel
  doors, the doorman, the guest book). Its text is overridden in `L_Boyle_*.int`.

**Starting a conversation (CONFIRMED):**
- Use: hook `DDH_PLAYER_USED`.
- Proximity: `DDH_*_APPROACHES`.
- AI state: bark hooks.
- Kismet: `DialogInputs`.

There is no separate "talk" volume.

### Text and VO location (CONFIRMED)

**Subtitles** are `DisConv_Blurb.m_Text` inside the cooked level packages (English). Player
choices are `DisConv_PlayerChoice.m_Choices_Static.m_ChoiceText`. No blurb text exists in
`Localization/<LANG>/*.int`; a scan of all 658 INT files found none. What the INT files do
localize:
- objectives and tasks (`Boyle_Objectives.int`);
- HUD messages and task target names (`L_Boyle_*_Script.int`);
- scripted choices;
- items (`Boyle_AbstractInv_*.int`);
- notes: `WrittenNotes_twk.int`, UTF-16, sections `[BoyleNotes.Boyle_* DisAbstractItem]`, field
  `m_Description`; `ChapterNotes_twk.int`;
- doors.

The note body is also in the package (`DisAbstractItem.m_Description`). The port reads text from
the packages. For non-English subtitles the source is UNKNOWN: no other-language blurb text was
found on this install.

**VO chain:** blurb `m_iBlurbGUID` -> `DisDialogVoiceData[_OneShot].m_VoiceData[].m_Blurbs[]`
`{m_GUID, m_AkEventName = "Play_<32 hex>", m_bIsNonWord, m_TextHash}` -> bank
`m_pAkBank` (`<Tree>_DlgData.Bk_<GUID>`).
- Each bank is a per-voice AKPK file **`CookedPCConsole/Bk_<GUID>.INT`**, with `.DEU/.ESN/.FRA/.ITA/.RUS`
  variants.
- There are 670 INT files, 758 MB. They are not `.pck` files, so `AUDIO_FORMAT.md`'s survey and
  `SoundLibrary::find_pck` miss them. The format is the same AKPK, language "english(us)", bank
  v65.
- Event ids are FNV-1 of the lowercased `Play_…` name, as for every other event.
- `L_Boyle_*_Audio` and `Bank_*Boyle*.pck` hold ambience, music and doors, not VO.

### Counts

Counts come from `conv_dump stats` / `table`. "Conv blurbs" are blurbs inside `DisConversation`s.
"All blurb nodes" also counts tree-level barks and the shared `Dlg_*` trees, which are duplicated
in each package.

| Package | Conversations (one-shot) | Conv blurbs | Player choices | Soirees | All blurb nodes (non-word) | Voice entries | VO events found in bank | VO banks (MB) |
|---|---|---|---|---|---|---|---|---|
| L_Boyle_Int_Script | 116 (112) | 415 | 26 | 13 | 1,003 (91) | 1,582 | 1,574 | 48 (71.5) |
| L_Boyle_Ext_Script | 31 (29) | 87 | 1 | 6 | 432 (46) | 693 | 686 | 12 (31.0) |
| L_BoyleStreet_Ext_Script | 35 (30) | 163 | 1 | 8 | 769 (98) | 1,418 | 1,344 (1 bank file missing) | 31 (59.1) |
| L_Boyle_Ext_P | 5 (4) | 19 | 0 | 0 | 264 (28) | 525 | 523 | 4 (22.5) |
| L_Boyle_Int_P | 1 (0) | 16 | 0 | 0 | 261 (28) | 261 | 260 | 1 (10.6) |

Notes on the counts:
- Every blurb node has a voice-table entry (0 without).
- About 99% of voice entries resolve to an event in their bank.
- What does not resolve:
  - 8 mission lines in Int. Examples: Brisby `DisConv_Blurb_15` [5099], Lydia [4805], Waverly
    [4892], Esma [4854].
  - 2 × 32 `Dlg_Weeper` lines in the street.
  - Bank `Bk_8431BF9C…` is absent.
- These are LIKELY cut lines. The port must time such lines from text length.
- `DisDialogOneShot` totals: 28 in Int, 7 in Ext Script, 13 in the street, 2 in Ext_P.

**Decode check (CONFIRMED).** `audio_dump event` was pointed at a scratch install
(`DISHONORED_DIR`) holding copies of the `Bk_*.INT` files renamed to `.pck`. 8/8 sample lines
resolved and decoded, from both embedded and streamed media, all with attenuation 3,000 uu:

| Speaker | Event | Length |
|---|---|---|
| Brisby | `Play_AE78FCA3…` | 7.09 s, streamed |
| Lydia | `Play_DEE1387D…` | 4.13 s |
| Miss White | `Play_813A92A8…` | 7.37 s, streamed |
| Esma | `Play_331CA3CA…` | 3.22 s |
| Waverly | `Play_DDD41286…` | 2.71 s |
| Shared aristocrat | `Play_887B2A5D…` | 5.62 s |
| Doorman | `Play_BA0A0078…` | 1.10 s |
| Shared female bark | `Play_90D9900C…` | 0.81 s |

WAVs are in `harness_out/boyle/vo/`.

Lip sync: the voice data carry `FaceFxGeneratedFromEnglish`, so FaceFX data exists. Its format is
not investigated (UNKNOWN). Approximate it with jaw or none.

## 5. Interactables

- **Travel doors:** 8 `DishonoredUsableObject`s with `Boyle_Twk.Door_Transitions.LTD_*` (section 1).
- **Doors.** `DisDoor` (`m_pDoorTweaks = doors_twk.*`, `m_bLocked`, `m_MatchingKeys` = an
  FString array matched against `DisKey_Base.m_Name`; decoded by `conv_dump`).
  - Int (`L_Boyle_Int_Audio`), 18 doors:

    | Door | Key | Locked |
    |---|---|---|
    | `DisDoor_0` | "Switch" | yes |
    | `_1` | Whale Oil Closet Key (key `DisKey_Base_2`) | yes |
    | `_11` | Basement Vault Key (`DisKey_Base_5`) | yes |
    | `_14` | Esma's bedroom (no matching key actor found) | no |
    | `_21` | Waverly's bedroom (`DisKey_Base_0`/`_10`) | no |
    | the other 13 | none | no |

  - Ext (`L_Boyle_Ext_Audio`), 8 doors:

    | Door | Key | Locked |
    |---|---|---|
    | `_0`, `_4` | Gate House Key (BS `DisKey_Base_0`/`_4`) | yes |
    | `_2` | Guard Quarters Key (ES `DisKey_Base_0`/`_2`) | yes |
    | `_8` | Sewer Gate Key (Ext_Audio `DisKey_Base_0`) | yes |

  - `DisDoorBreakSteps` make some doors breakable.
- **Pickups.**
  - Int Script: 59 `DisAbstractItemPickup` (valuables, paintings), 17 notes, 28 `DisStatPickup`
    (runes, bone charms, coins; optional), elixirs.
  - Ext Script: 20 pickups, 2 notes.
  - Street: 22 pickups, 2 notes.
  - Optional items can be skipped.
- **Mission items:**
  - Invitation: `Boyle_AbstractInv_Assets.BoyleInvitation_twk` (given by
    `DisSeqAct_GivePickup`) / `Boyle_AbstractInv_MS.boyleinvitation_AbsItm`.
  - Party drink: `boylepartydrink_AbsItm`.
  - Pendleton's note to Shaw: `WrittenNotes_twk.BoyleNotes.Pendleton_Shaw_Note`.
  - Guest book: `Boyle_Guest_Book*`. Choice `DisSeqAct_DialogScriptedChoice_0` [8517], flag
    `boyle_signed_guest_book`.
- **Doorman and invitation check:** BS `DisDialogOneShot_4`. Conversations: `QuiteACostume`,
  `InvitationPlease`, `Come In`, `CantLetYouIn`, `NeedToSee`, `DontComeBack`, `tresspasser`,
  `You thief`, plus a `windo guard used` use-hook. Choice [7705]; section 2 has the flow.
- **Front door of the mansion:** ES `DisDialogOneShot_5` (`DishonoredSpawner_1`).
  `Front_Door_Salutation` on approach; use runs `Front_Door_Guard` -> `Get_The_Door`.
- **Lord Brisby:** section 2. The choice to accept is implicit: the player hears the KO
  conversation and then either delivers the body or not.
- **Basement delivery:** `DisTrigger_23`, `TriggerVolume_10` and `TriggerVolume_8` around the
  boat at `TargetPoint_164`; gate `InterpActor_0` (matinee [16004]).
- **Samuel's boat (exit):** BS `DisDialogOneShot_7` (`DishonoredSpawner_15`); conversations
  `Samuel_Intro`, `SamUsed`, `Get_in_the_Boat`, `Lets_Go`.
  - Flag `Boyle_Return_To_Boat` starts the moved-boat logic on street load: BS `LevelLoaded`
    [13481] teleports the boat `SkeletalMeshActorMAT_1` and Samuel to `TargetPoint_22`/`_31` and
    sets T6's target.
  - Exit trigger chain: section 2, step 8.
- **Pistol duel** (ES sub-sequence `Pistols_in_the_Yard`; Lord Shaw `DishonoredSpawner_45`,
  seconds `_46` and `_11`; one-shots `_7`, `_2`, `_0`):
  1. Using `DisTrigger_2` (the case) [7035] gives `Pickups_twk.Weapons.Gun_Elite_twk` and
     bullets, equips the pistol, sets `player_took_the_pistol` and starts conversation
     `Duel_Explained`.
  2. The countdown conversations are `Three`/`Two`/`One`/`Turn_and_Fire`.
  3. "On mark" uses `TriggerVolume_3` and `Trigger_LOS_2`.
  4. Fail conditions: powers used, leaving `DisTrigger_29`, killing before the count.
  5. The outcome is set by flags `PistolWinHonor`/`PistolWinMess`/`PistolFailObj` and chapter
     notes `PistolDuelWon`/`PistolDuelCheat`.
- **Forbidden zones:** `DisForbiddenZone_1` (whole upstairs) and `_2` (staircase). Touch
  [16692]/[16694] -> `DisSeqAct_ShowLocationDiscovery` (trespassing on/off, text in
  `L_Boyle_Int_Script.int`); `DisSeqAct_ForbiddenZoneOverride` ×2.
- **Alarm bell:** Int `DisAlarmBell_0` -> flag `Boyle_Alarm_Rung`, which affects Brisby.
- **The Outsider:** ES `DisDialogOneShot_1` (shrine). Optional.

## 6. Party crowd behaviour

**Spawners (CONFIRMED, `harness_out/boyle/spawners.txt`).** Int Script has 38 `DishonoredSpawner`s:
- Guests use `Twk_Pawn_Boyle_Party.Pwn_AristoParty_MTall_*` and `Twk_Pawn_Boyle_AristoF1..4`.
- Named NPCs: Miss White, Ramsey, Brisby and the sisters.
- Party elite guards: `Pwn_EliteGuard_MTall_*_Party`. Four of them have `m_bSpawnOnBeginPlay=false`
  and `DAISL_Suspecting` (reinforcements).
- Servants: `Pwn_Servant_F_*`. Music overseers: `Pwn_MusicOverseer_Party`.
- Upstairs guards: plain `Pwn_EliteGuard_MTall_*`.

Ext Script has 9 spawners and the street 42.

**Placement and idles:**
- `m_bPatrolUponStartup` plus 69 `DisSeqAct_AISetPatrol` in Int (11 Ext, 28 street), starting at
  `DishonoredNavPoint_*` on `DishonoredRoute`s, which Kismet toggles.
- `Main_Sequence.Soiree_Directory` has 53 `DisNPCDistractor` spots, each with sub-sequence
  `DisNPCDistractor_N_seq`. `DisSeqEvent_Distracted` *Start/End* -> a looping `SeqAct_Interp` at
  `m_AIBehaviorPriority = ESAP_Distractor` from `DistractionSoiree.*` (Cigarette, DrinkGlass,
  LookAroundWindow, Painting2, …). `InterpTrackSoireeControl` sets the loop pins and break mode.
- These are ambient "activity stations" that patrolling guests stop at.

**Gossip.** NPC-to-NPC conversations are one-shots with 2–3 speakers, e.g. `_18` Jane Blair and
two guests, `_24` Miss Triss, Jerval and Stephen, `_23` Bernard and Burke. Shared trees add
`GuardsPassingEachOther`, `DDH_FRIEND_APPROACHES` and other barks.

**Cutscenes.** 115 `SeqAct_Interp` in Int Script, 14 in Ext Script and 28 in the street. Many are
soirees: multi-actor scenes with `InterpGroupAI` tracks, look-at, stretch-anim, dialog and AkEvent
keys.

**Reactions.** The panic chain: guests' `DisBehaviorPanic` -> `party_is_panicked` fails Brisby's
quest. `revamp_downstairs_behaviors` runs after a death. Guards use `SetDisposition` and
`AIRingAlarm`.

**Social stealth.**
- The ground floor accepts the masked player. `DisStealthVolume_1` main-hall settings apply.
- Upstairs is forbidden (`DisForbiddenZone_1`/`_2`, `DisStealthVolume_0`).
- Guards warn before engaging, e.g. `Boyle_UpstairsWarning` and `Boyle_BackStairsGuardWarning`
  matinees, and `DDH_PATROL_PERSONAL_SPACE_WARNING` with the sword equipped.

## 7. Port plan

### What exists already

`crates/dishonored_mission`:
- objectives and tasks, flags, counters, markers and NPCs;
- triggers including `Use(marker)` and `Talk(npc)`;
- actions including `Usable`.

The runtime has:
- rendering of `L_Boyle_Ext_P`, collision, NPC spawning from spawners and bots;
- Wwise playback by event name, and HUD objectives.

### Engine features to add (minimal set, in dependency order)

1. **Two-map world and travel.**
   - Load `L_Boyle_Int_P` (all sublevels). Because of the shared frame, either render both maps
     at once or swap them.
   - Implement `SetPlayerTravelDestination` + `GotoPlayerTravelDestination` with `PlayerStart`
     tags, the fade, and **per-map state save and restore**: NPC alive/dead/KO and position,
     bodies, pickups taken, door states, mission flags.
   - All of this can be read from the original data: start tags, `LTD_*` use objects and their
     choice nodes.
2. **Use/interaction system.**
   - Interactables: `DishonoredUsableObject` (travel), `DisDoor` (open, locked, `m_MatchingKeys`
     to inventory key names), `DisKey_Base`, `DisAbstractItemPickup[Note]`, `DisStatPickup`
     (optional), `DisAlarmBell`.
   - Prompt text comes from the tweaks (`DisTweaks_InteractableInterface`/`_PickupBase`) and the
     INT files.
3. **Inventory and abstract items.**
   - Abstract items: invitation, drink, notes, keys. Chapter notes go to a journal list.
   - The notes reader shows `DisAbstractItem.m_Description` (or `WrittenNotes_twk.int`).
4. **Conversation runtime**, interpreting the original `DisDialogTree`/`DisConversation` graph
   loaded at runtime:
   - Nodes: blurbs (subtitle `m_Text` + VO event + bank), branches (random, sequential,
     random-sequential), `CheckStoryFlag`/`SetStoryFlag`, `PlayerChoice` (choice UI),
     `KismetActivateRemoteEvent` (to mission `Remote` events), `Soiree` (to a named scripted
     scene), `SeenDialogLabel`, `HasInventoryItem`, `TimeLimit`, `OneShotSpeaker` gating.
   - Hooks: `DDH_PLAYER_USED`, `*_APPROACHES` and `DDH_KISMET_ACTIVATED`, plus a small set of AI
     barks.
   - VO: index `Bk_*.INT` (a small extension of `SoundLibrary::find_pck`, or an alias table) and
     play the line 3D on the speaker's pawn; advance on the clip length, or on text length when
     the event is missing.
   - Subtitles in the HUD font. Speaker names come from `m_SpeakerNickName` or the pawn.
5. **Carry and knockout.**
   - Choke/sleep-dart KO producing an unconscious state.
   - Pick up and carry a body (KO or dead; `DisTweaks_StateNPCMasterDead_BeingCarried`), drop it,
     and touch triggers that accept carried or dead pawns (`bAllowDeadPawns`).
   - Mission events: `NpcKnockedOut`, `BodyInVolume(npc, marker)`.
6. **NPC scripting hooks for the mission.**
   - Per-NPC material override (dress colours from `Npc_Boyle.Materials.*` and
     `Npc_Masks.BOYLE.*`).
   - Teleport to an actor; `MoveNpc`/`Patrol` with `DishonoredNavPoint` routes.
   - Ambient "distractor" idle loops (one looping anim per spot is enough at first).
   - Dispositions (neutral party versus forbidden zone).
7. **Scripted scenes (soirees).** Do not port the matinees in general. Hand-author the 4–5 that
   gate progress (Samuel intro, Brisby walk-to-boat, the `Thanks`/gate-close exit, Miss White's
   drink) as timed sequences: move, anim, line. Fade-to-black is acceptable.
8. **Random choice in missions.** A `Random([..branches])` / `Shuffle` action that sets flags.
   It is needed for the sisters' positions, dresses and guilt.

### Data-driven versus hand-authored

**Read at runtime from the original packages:**
- Objective and task text (`Boyle_Objectives.*`, INT).
- All conversation graphs, subtitles, choices and VO (by one-shot path, e.g.
  `L_Boyle_Int_Script:DisDialogOneShot_6`).
- Notes and items.
- Door locks and keys.
- Spawners and their tweaks: pawns, voices, patrol flags.
- PlayerStarts and use objects.
- Dress materials.
- Target points.
- Story-flag names (`Startup:SF_LadyBoyle`).

**Hand-authored in `missions/boyle_original.ron`:**
- The mission rules, i.e. a re-expression of the ~200 relevant Kismet nodes listed above. The
  Kismet graph is not executed. It depends on matinees, `AttachToEvent` and native behaviours.
- The random setup.
- The clue dispatcher.
- Kill and KO resolution.
- The Brisby delivery checks.
- The exit chain.
- The simplified soirees.

**Mission file references** (`Package:Object`; the port resolves the text):
- Tasks: `Boyle_Objectives.BoyleAssassination.DishonoredTask_Custom_{0..26}` and
  `Boyle_Objectives.PendeltonsNote.DishonoredTask_Custom_{0..2}`. Text is `m_Description`, or the
  `Boyle_Objectives.int` section `[BoyleAssassination:DishonoredTask_Custom_N DishonoredTask_Custom]`.
- Target names: `L_Boyle_Int_Script:Main_Sequence.Boyle_Objectives.DisSeqAct_UpdateTaskTarget_*`
  (INT sections of the same path).
- Conversations, as `<package>:<one-shot>` + label:
  - **Int:**
    - `DisDialogOneShot_6`: Brisby `Lord_Brisby_Quest_Start`, `Brisby_Boyle_Invite`, `Esma_KO`,
      `Lydia_KO`, `Waverly KO`, `Thanks`, `Excited_Success`, `Get_off`, `Wrong_girl_one`/`_two`,
      `Killed_Right_Girl`, `Where_is_the_girl`, `Hurry_On_Board`, `Put_her_back`.
    - `_9`: Miss White `Hits_On_Player`, `Takes_Drink`, `WaverlyRed/White/Black`,
      `LydiaRed/White/Black`, `Idle_With_Jack`, `Wight Over`.
    - `_10`/`_11`/`_12`: sisters `<Name>_w_Player`, `Hunting <Name>`, …
    - `_2`, `_7`, `_16`, `_18`, `_27`: gossip.
  - **Ext Script:** `_5` front door; `_7`, `_2`, `_0` duel.
  - **Street:** `_4` doorman; `_7` Samuel; `_3`, `_9` invitation scenes.
  - Each line's VO is the blurb GUID -> event -> bank (`conv_dump vo`); store none of it.
- Notes: `WrittenNotes_twk.BoyleNotes.Boyle_{Esma,Lydia,Waverly}_{Red,White,Black}` and
  pickups `L_Boyle_Int_Script:DisAbstractItemPickupNote_{0,3,4,10,12,13,14,15,17}`.
- Chapter notes: `ChapterNotes_twk.BOYLE.*`.
- Items: `Boyle_AbstractInv_MS.boyleinvitation_AbsItm`, `boylepartydrink_AbsItm`.
- NPCs: Int `DishonoredSpawner_{2,11,4}` (sisters), `_9` (Brisby), `_20` (Miss White), `_21`
  (Ramsey), `_1` (boat lady); Ext Script `_45` (Shaw), `_1` (front door); street `_15`
  (Samuel), `_2` (doorman).
- Markers:
  - Int `TargetPoint_10`/`_82`/`_140` (sister positions), `_55`/`_58`/`_60` (bedroom clues),
    `_108` (cider), `_120` (Brisby), `_164` (boat), `_30` (exit), `_22` (upstairs).
  - Ext Script `TriggerVolume_14`/`_0` (estate entry).
  - Street `TriggerVolume_23`, `DisTrigger_35`/`_0`/`_34` (exit), `TriggerVolume_0` (gate).
  - Names repeat across sublevels (e.g. `TriggerVolume_0`, `DisAbstractItemPickup_67`,
    `DishonoredSpawner_1`), so always qualify them with the package.

### Milestones

**(a) Complete lethal playthrough**
1. Load and render `L_Boyle_Int_P` (all 8 sublevels) and its collision. Verify the shared frame
   with Ext.
2. Travel between Ext and Int through the 8 `LTD_*` doors, with fade, `PlayerStart` tags and
   per-map state persistence. Harness: walk through each door and back.
3. Use system, doors and keys, pickups, and abstract inventory with the notes reader (data from
   tweaks and INT).
4. Spawn the Int cast from `DishonoredSpawner`s with the sisters' materials. Add the
   random-setup action and port the setup (positions, dresses, guilt, note teleport).
5. Conversation runtime v1: use and approach hooks, blurbs with subtitles and VO from
   `Bk_*.INT`, branches, flag nodes, Kismet-event outputs, `DialogInputs`-by-name. Gate it on
   Miss White, the doorman and the front-door guard.
6. Player choices (two-option UI), and scripted choices for doors, the doorman and the guest
   book.
7. Mission file `boyle_original.ron`, lethal part:
   - T12/T1/T24/T23/T22/T25 activation;
   - the clue dispatcher (9 notes and Miss White -> dress tasks);
   - death of a specific NPC -> guilty/known resolution, T11 elimination, T6;
   - the street exit chain (T10, T5, end).
   - Harness: scripted agent identifies by note, kills, exits; three seeds cover all three
     guilty sisters.
8. Polish: forbidden-zone warnings and trespass messages, alarm and reinforcement spawners,
   the panic chain.

**(b) Nonlethal playthrough** (needs (a) steps 1–7)
1. Knockout (choke and sleep dart) and the unconscious state. Carry and drop bodies through
   travel doors. The body must travel with the player to the Int cellar; the boat is in the same
   map, so only intra-map carry is needed.
2. Brisby's conversation chain and walk to the boat, as a simplified soiree. KO-task activation
   when the guilty sister is known.
3. Body-in-volume checks at the boat (`DisTrigger_23`, `TriggerVolume_10`): right or wrong body,
   alive or dead. Brisby reactions.
4. Exit scene: the `Thanks` line, despawn the carried body and show the boat lady, gate close,
   `CompleteMercyPath`, T6. Harness: identify via Miss White, KO, carry, deliver, exit.

Optional afterwards: the Lord Shaw duel (PN T0/T2), the art-dealer invitation scene, the
Outsider shrine, runes and bone charms, lip sync.

## 8. Summary

**Key numbers:**
- Two persistent maps (Ext: 7 sublevels; Int: 8, including Basement and Stealth) in **one shared
  coordinate frame**. 8 travel doors, 9 `PlayerStart`s, travel by remote `ChangeLvl_*` handled
  in `DishonoredGameFull_P`, with level state saved.
- Objectives: BA has 27 tasks and PN 3. 159 task-control nodes in L_Boyle_Int_Script alone
  (Int: 74 `SetTaskState`, 55 `SetTaskHidden`, 25 `UpdateTaskTarget`). 81 `SF_LadyBoyle` flags.
- Conversations in mission packages:
  - 188 total; 175 are one-shot conversations on 50 `DisDialogOneShot` actors.
  - 700 conversation blurbs; 2,729 blurb nodes including barks. 28 player-choice nodes, 27
    soirees.
  - Every blurb has a voice entry; about 99% of entries have an event in their bank.
  - 8/8 sampled lines decode.
- VO is in 670 per-voice AKPK files `Bk_<GUID>.INT` (758 MB). Subtitles are `m_Text` in the
  cooked packages, not in the INT localization.
- The sisters: random permutation of 3 positions, random red/white/black, guilty sister chosen by
  a 2-of-3 destroy random switch (independent of dress). 9 clue notes, 3 placed per run (one per
  guilty colour). A kill counts only if the sister is guilty **and** identified, or after all
  three are dead (T11).

**Top risks:**
1. **Level travel with persistent state.** No such system exists yet. Carrying a body across
   travel (Ext to Int) is plausible in the original. The minimal nonlethal path can stay inside
   Int.
2. **Conversation runtime breadth.** About 40 node and hook classes. Shared bark trees depend on
   AI states that the port's AI may not expose; start with use, approach and Kismet hooks only.
3. **Soirees and matinees.** Brisby's walk, the exit cutscene, the Samuel intro and Miss White
   gate progression through `DisConv_Soiree` -> `SeqAct_Interp` (GUID link). They must be
   hand-authored approximations.
4. **Missing pieces:**
   - VO for about 1% of lines;
   - FaceFX lip sync not decoded;
   - non-English subtitles not found;
   - `Brisby_Boyle_Invite`'s start condition not fully traced (hook output 0 is empty);
   - `DisTravelSpawner` semantics are LIKELY only.
5. **Name collisions across sublevels** (`TriggerVolume_0`, `DisAbstractItemPickup_67`,
   `DishonoredSpawner_1/_2`, …). The mission file must use package-qualified references, and
   `mission check` must enforce it.
