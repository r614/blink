# Saving and loading

How Dishonored saves, autosaves and restores a game, and how the port does it. Sources:
the UnrealScript class layouts (`DishonoredEngine`, `DisSeqAct_AutoSave`,
`DisSeqAct_SaveLevelState`, `DisSeqAct_DiscardLevelState`, `DisSeqAct_DiscardAllLevelStates`,
`DisGFxMoviePlayerMenuBase`, `DisTweaks_GFxMoviePlayerMenuBase`), `DefaultInput.ini`,
`DefaultEngine.ini`, `DefaultDebugMenu.ini`, and Ghidra on `Dishonored.exe` (addresses below).
The user's own save files were not read.

## 1. Slots and files

`DishonoredEngine.ESaveGameSlot`:

| Slot | Name | Use |
|---|---|---|
| 0 | `SGS_LastSave` | load only: the newest save (first entry of the save list) |
| 1 | `SGS_DishonoredOptions` | options (`OPTIONS.SAV`) |
| 2, 3 | `SGS_DishonoredDLC06To07_LowChaos` / `_HighChaos` | DLC transition saves |
| 4..12 | `SGS_DisMission0..8` | mission-start saves (`DisSeqAct_AutoSave` with `m_bMissionStart`) |
| 13, 14 | `SGS_DisAutoSave0` / `1` | autosaves, used in turn |
| 15 | `SGS_DisQuickSave` | quicksave |
| 16.. | `SGS_Dishonored0` .. | manual saves (up to slot 55) |

- File names (the cloud file list, 0x00b75ed0): slots 4..15 are `<EnumName>.SAV`
  (`DisMission0.SAV` .. `DisMission8.SAV`, `DisAutoSave0.SAV`, `DisAutoSave1.SAV`,
  `DisQuickSave.SAV`); slots 16..55 are `DISHONORED<slot - 16>.SAV`, i.e. 40 manual slots.
  DLC copies are `DLC0<n>_<name>.SAV` for n = 2..9. Also `OPTIONS.SAV`, `PROFILE.SAV`,
  `PROFILE.BIN`, `DLC02_TO_DLC03_LOW.SAV`, `DLC02_TO_DLC03_HIGH.SAV`. The folder string is
  `SaveData\`.
- Bindings (`DefaultInput.ini`): `GBA_QuickSave` = `Dis_Save 15` on F5, `GBA_QuickLoad` =
  `Dis_Load 15` on F9.
- `DefaultDebugMenu.ini` still lists an older numbering (quicksave 12, slots 13..22): a
  development leftover; the shipped binding and enum use 15.
- Save list and menus: `UI_LoadGame.LoadGame` (imported by the main and pause menus as
  `loadGame_mc` / `saveGame_mc`). Prefixes are localized `DisTweaks_GFxMoviePlayerMenuBase`
  strings (`m_MissionSavePrefix`, `m_AutosavePrefix`, `m_QuicksavePrefix`, `m_CorruptedSave`,
  `m_DateFormat`), with map pictures from `UI_Mission_Large` / `UI_Mission_Small` and the
  map's `m_MapConfig.m_FriendlyName` ("The Boyle Estate"). Pause menu entries `t_SaveGame`,
  `t_LoadGamePauseMenu`; main menu `OnLoadLastSaveClicked` ("Continue"), `Req_SaveSlotInfos`.

## 2. Saving and loading commands (`DishonoredEngine`, vtable 0x010db390)

- `Dis_Save(slot)` (exec 0x009ecd10 -> vtable +0x200, 0x00a0b680):
  - refused when the player controller says no (its vtable +0x444);
  - slots 2 and 3 always go through; otherwise only slots above 14 (quicksave and manual:
    autosave and mission slots are never written by the player);
  - refused when saving is disallowed (0x009e41c0: any `m_DisallowSavingRefCount[]` non-zero,
    `m_fDisallowSavingTimer` > 0, or the controller check) or a mission-start save is pending;
    the refusal prints the reasons (`EDisDisableSaveType` names, "Mission Start Pending (%s)",
    "Wait Time: %.2f");
  - otherwise, with no command running, 0x00a095a0 clamps the slot to [2, 55], stores
    `m_SavegameSlot` and sets `m_SaveLoadCmd = SLC_Save`.
- `Dis_Load(slot)` (vtable +0x204, 0x00a0c750): with no command running, clamps the slot to
  [0, 55]; slot 0 becomes the first entry of the save list (the newest); sets
  `m_SaveLoadCmd = SLC_Load` (19).
- `PushDisableSave(type)` / `PopDisableSave(type, wait)` (+0x208 / +0x20c): reference counts per
  `EDisDisableSaveType` (`KillCam`, `Blink`, `GameOverPending`, `CinematicMode`,
  `PlayerInCombat`, `ItemContext_Assassinate`, ..., `Soiree`, the player master states
  `Assassinate`, `Choice_Base`, `Choke`, `Dead`, `Minigame`, `PrePossess`, `Stunned`, `Versus`,
  `HolePeeking`, `Kismet`, `Pull`). Pushing cancels a save that has not started yet (its
  actions are denied); popping sets `m_fDisallowSavingTimer = max(timer, wait)`.
- `PushIgnoreAutosave(type)` / `PopIgnoreAutosave(type)` (+0x210 / +0x214): reference counts for
  `DIAT_PlayerInCombat` and `DIAT_GameOverPending`.
- No native code calls these four through the engine vtable (VCallers on +0x208 / +0x210
  found none; no direct callers): who pushes them is open (U-340, U-342).
- The save/load state machine (`ESaveLoadCmd`, run from the engine tick 0x00a177e0 ->
  0x00a162d0) also handles device and corruption errors (`SLS_Corrupted`, `SLS_BadVersion`:
  "Failed to read savegame! File version ...").

## 3. Autosaves

`DisSeqAct_AutoSave` (Kismet; `m_bMissionStart`, `m_bEvenInCombat`, `m_bSkipAutosaveDelay`,
`m_MissionNumber`; outputs Out, Accepted, Denied; status `EDisAutoSaveActionStatus` Waiting,
Saving, Accepted, Denied at +0x110):

- Activated (0x00b8f240): status = Accepted. If the controller check (+0x440) fails, nothing
  else happens (so "Accepted" fires without a save). With `m_bMissionStart` and
  0 <= `m_MissionNumber` < 9: status Waiting and a pending mission-start save in slot
  `4 + m_MissionNumber` (0x00a09700, replacing and denying an older one). Otherwise status
  Waiting and the action joins the pending autosave (0x00a0d7f0): the first requester starts
  it with `m_fPendingAutosaveTimer = m_fAutosaveWaitTime` and the slot from 0x00a0b050;
  `m_bSkipAutosaveDelay` zeroes the timer.
- Update (0x00b8f2e0): Accepted fires output 1, Denied output 2, then the latent action
  finishes (output 0 "Out"); Waiting / Saving keep it running.
- Engine, every tick with no command running (0x00a0c530): once the timer is out, if the
  autosave was queued by code it is taken; otherwise, unless some requester has
  `m_bEvenInCombat`, `DIAT_PlayerInCombat` drops it, and `DIAT_GameOverPending` drops it in
  any case (all requesters Denied, 0x00a070b0). A kept autosave waits until saving is allowed
  and some world flags are clear, then its requesters become the actions to notify and the
  save starts. When the save finishes the actions go Saving -> Accepted (vtable slot 71,
  0x00b85f30; a still-Waiting action becomes Denied).
- The pending mission-start save (0x00a09640) is taken as soon as saving is allowed (no
  combat check).
- Autosave slot (0x00a0b050): `m_NextAutoSaveSlot` if set; else 13, unless the save list's
  first autosave is 13, then 14: the newer autosave is never overwritten.
- Every map load that is not a reload queues an autosave from code (vtable +0x194, 0x00a152f0,
  `FUN_00a0d7f0(0)`; menu maps excluded): Dishonored autosaves on each level transition.
- `m_fAutosaveWaitTime` is a config value that no shipped ini sets: 0 (U-341).
- Boyle: the mission-start save is `L_Boyle_Ext_P` op 3852 (`m_bMissionStart`, number 4 =
  `DisMission4.SAV`, skip delay); its "Accepted" output starts Samuel's intro matinee. The
  script maps hold 2 (Ext), 13 (Int) and 5 (street) more autosaves after story beats, two
  with `m_bEvenInCombat` ("Game Saved" in the street script).

## 4. Level states

- `DisSeqAct_SaveLevelState` (Activated 0x00b8e0e0) only sets engine flags (0x009e43c0:
  `m_bSaveLevelState`, and `m_bCompleteSave` = !`m_bPartial`); the level is stored at the map
  change.
- `DisSeqAct_DiscardLevelState` (0x00b8e110) discards `m_LevelName`'s state;
  `DisSeqAct_DiscardAllLevelStates` (0x00b8e150) calls the same function (0x00a14df0) with no
  name: every level. It clears the game state's level list, a per-level record table (0x100
  bytes per entry keyed by level name) and a world-info list.
- What a level state holds is serialized natively (no `SaveGame` property flags in the
  script): `DisSerializableObject` is a native mixin used by the AI blackboard records and
  stimuli (so AI memory is saved), and the conversation and task-target systems have their
  own save structs (`FDisConvSaveData`, `FDisTaskTargetsSaveData`). The exact property list
  is open (U-343).

## 5. The port

- `crates/dishonored_save`: slots (the enum above), file names, saves folder (`--saves`,
  `DISHONORED_RS_SAVES`, else `SaveData` next to the settings file), a container (magic,
  RON header, deflated RON state; self-describing so fields added with `#[serde(default)]`
  keep older saves loading), the save list (newest first), last-save resolution, the autosave
  slot rotation, and `state_hash` (FNV-1a over the bincode encoding: equal hashes = equal
  bits).
- `dishonored_multiplayer::mission::save`: a save is the whole `MissionMatch` through `serde`
  on the existing structs (world with players, NPCs, pickups, projectiles; crowd brains,
  factions, alarms; level states of the maps left; mission interpreter and objectives;
  conversation runner with its RNG; Kismet runtime and host state; autosave state). Only the
  setup and the Kismet / dialog databases are skipped; the loader rebuilds the setup from the
  mission file and re-attaches them. The header stores the setup's hash; a save from another
  setup is refused.
- Autosaves as in section 3: `DisSeqAct_AutoSave` joins the pending autosave or the
  mission-start save; the match takes it when saving is allowed (port reasons: a player dead,
  a choice open, choking, blinking, possessing) and reports
  "Accepted" on the next tick; combat (a guard fighting a player) drops an autosave whose
  requesters lack `m_bEvenInCombat`; each map load queues one from code. The host writes the
  saves the match took (`save::take_requests`) into their slots.
- Runtime: F5 / F9 from the original bindings, Save and Load in the pause menu, Continue
  (last save) and Load in the launcher's missions page.
- `harness save`: see `tools/harness/src/save.rs`.
